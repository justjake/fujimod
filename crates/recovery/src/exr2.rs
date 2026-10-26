//! EXR II's callback-free preloaded section read, sourced from the card image.
use crate::{
    image::{self, Plan, Result, extent, put, require, word},
    rom_sd::{call, sequence},
};
use serde::Deserialize;
const PROGRAM: &[u8] = include_bytes!("../assets/section-reader.generated.bin");
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    schema: usize,
    address: usize,
    size: usize,
    entry: usize,
    config: usize,
}

fn preload_call(ram: &[u8], plan: &Plan) -> Result<(usize, usize)> {
    let mut found = Vec::new();
    for region in &plan.regions {
        let end = region.address + region.source.decoded_size;
        for at in (region.address..end.saturating_sub(40)).step_by(4) {
            if !sequence(ram, at, &[0xe3a03001, 0xe58d3000, 0xe1a00007, 0xe1a01008])
                || (word(ram, at + 16)? & 0xfff0f000 != 0xe3002000
                    && word(ram, at + 16)? & 0xfffff000 != 0xe3a02000)
                || word(ram, at + 20)? & 0xfff0f000 != 0xe3402000
                || word(ram, at + 24)? != 0xe3a03000
                || word(ram, at + 32)? != 0xe1a04000
            {
                continue;
            }
            let read = call(ram, at + 28)?;
            require(
                plan.regions
                    .iter()
                    .any(|r| r.address <= read && read < r.address + r.source.decoded_size),
                "EXR II section reader is outside initialized code",
            )?;
            let mut selectors = Vec::new();
            for branch in (at.saturating_sub(0x100)..at).step_by(4) {
                let w = word(ram, branch)?;
                if w >> 24 != 0x1a || word(ram, branch - 4)? != 0xe3500000 {
                    continue;
                }
                let target = branch as i64 + 8 + (((w & 0xffffff) as i32) << 8 >> 6) as i64;
                if target == at as i64 {
                    selectors.push(branch);
                }
            }
            require(
                selectors.len() == 1,
                "No unique EXR II preloaded selector branch",
            )?;
            let selector = selectors[0];
            let shift = (word(ram, selector - 12)? >> 7 & 31) as usize;
            require(
                matches!(shift, 14 | 17)
                    && word(ram, selector - 12)? & !15 == 0xe1a08020 | (shift as u32) << 7
                    && matches!(word(ram, selector - 12)? & 15, 4 | 8),
                "EXR II section count is not a checked block shift",
            )?;
            require(
                (selector.saturating_sub(32)..selector - 12)
                    .step_by(4)
                    .any(|i| word(ram, i).ok() == Some(0xe1a07027 | (shift as u32) << 7)),
                "EXR II section offset uses a different block shift",
            )?;
            require(
                (at.saturating_sub(0x180)..selector)
                    .step_by(4)
                    .any(|i| sequence(ram, i, &[0xe3a00005]) && call(ram, i + 4).is_ok()),
                "EXR II caller does not select section 5",
            )?;
            found.push((at + 28, shift));
        }
    }
    require(
        found.len() == 1,
        "No unique EXR II preloaded section-5 read",
    )?;
    Ok(found[0])
}

pub(super) fn install(inverted: &[u8], plan: &Plan, ram: &mut Vec<u8>) -> Result<()> {
    let layout: Layout =
        serde_json::from_str(include_str!("../assets/section-reader.generated.json"))
            .map_err(|e| image::Error::Invalid(e.to_string()))?;
    require(
        layout.schema == 1
            && layout.address == 0x100000
            && layout.size == PROGRAM.len()
            && layout.entry < PROGRAM.len()
            && layout.config + 16 <= PROGRAM.len(),
        "Invalid section-reader layout",
    )?;
    let (site, shift) = preload_call(ram, plan)?;
    let section = plan
        .regions
        .iter()
        .find(|r| r.name == "section5")
        .ok_or_else(|| image::Error::Invalid("Missing EXR II section 5".into()))?;
    let table = plan
        .regions
        .iter()
        .find(|r| r.name == "section table")
        .ok_or_else(|| image::Error::Invalid("Missing EXR II section table".into()))?;
    let row = table.source.dat_offset + 0x40 + 5 * 36;
    require(
        word(inverted, row)? == 5
            && word(inverted, row + 4)? as usize == section.address
            && word(inverted, row + 8)? as usize == section.source.decoded_size
            && word(inverted, row + 24)? as usize == section.source.size,
        "EXR II section-5 descriptor differs from its source mapping",
    )?;
    let offset = word(inverted, row + 20)? as usize;
    let unit = 1usize << shift;
    let size = section
        .source
        .size
        .checked_add(unit - 1)
        .map(|n| n & !(unit - 1))
        .ok_or_else(|| image::Error::Invalid("EXR II section size overflows".into()))?;
    require(
        offset.is_multiple_of(unit) && size > 0,
        "Unaligned EXR II section source",
    )?;
    let source = extent(inverted, section.source.dat_offset, size)?;
    let tail = (ram.len() + 3) & !3;
    require(
        tail.checked_add(size).is_some_and(|end| end < 0x40000000),
        "EXR II section escapes RAM",
    )?;
    for r in &plan.regions {
        require(
            layout.address + PROGRAM.len() <= r.address
                || r.address + r.source.decoded_size <= layout.address,
            "Section reader overlaps initialized firmware",
        )?;
    }
    for r in &plan.bss {
        require(
            layout.address + PROGRAM.len() <= r.address || r.address + r.size <= layout.address,
            "Section reader overlaps firmware BSS",
        )?;
    }
    require(
        extent(ram, layout.address, PROGRAM.len())?
            .iter()
            .all(|b| *b == 0),
        "Section-reader reservation is occupied",
    )?;
    let mut program = PROGRAM.to_vec();
    for (i, value) in [tail, size, offset, shift].into_iter().enumerate() {
        put(
            &mut program,
            layout.config + i * 4,
            u32::try_from(value)
                .map_err(|_| image::Error::Invalid("Section reader exceeds ARM32".into()))?,
        )?;
    }
    let delta = (layout.address + layout.entry) as i64 - site as i64 - 8;
    require(
        delta % 4 == 0 && (-0x2000000..0x2000000).contains(&delta),
        "Section-reader BL exceeds ARM range",
    )?;
    ram[layout.address..layout.address + program.len()].copy_from_slice(&program);
    put(ram, site, 0xeb000000 | ((delta >> 2) as u32 & 0xffffff))?;
    ram.resize(tail, 0);
    ram.extend_from_slice(source);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{Compression, Region, Source};
    fn source(at: usize, size: usize) -> Source {
        Source {
            dat_offset: at,
            size,
            compression: Compression::Raw,
            decoded_size: size,
            sha256: String::new(),
            initialized_prefix: None,
        }
    }
    fn fixture(shift: usize) -> (Vec<u8>, Plan, Vec<u8>) {
        let mut input = vec![0x73; 0x60080];
        for (offset, value) in [(0, 5), (4, 0x210000), (8, 256), (20, 0x40000), (24, 256)] {
            put(&mut input, 0x800 + 0x40 + 5 * 36 + offset, value).unwrap();
        }
        let mut ram = vec![0; 0x230000];
        for (at, w) in [
            (0x200140, 0xe3a00005),
            (0x200144, 0xeb00006d),
            (0x200160, 0xe1a07027 | ((shift as u32) << 7)),
            (0x200170, 0xe1a08024 | ((shift as u32) << 7)),
            (0x200174, 0xeb000061),
            (0x200178, 0xe3500000),
            (0x20017c, 0x1a000018),
            (0x2001e4, 0xe3a03001),
            (0x2001e8, 0xe58d3000),
            (0x2001ec, 0xe1a00007),
            (0x2001f0, 0xe1a01008),
            (0x2001f4, 0xe3002000),
            (0x2001f8, 0xe3442000),
            (0x2001fc, 0xe3a03000),
            (0x200200, 0xeb00003f),
            (0x200204, 0xe1a04000),
        ] {
            put(&mut ram, at, w).unwrap();
        }
        let plan = Plan {
            size: ram.len(),
            entry: 0,
            edits: vec![],
            bss: vec![],
            regions: vec![
                Region {
                    name: "section table".into(),
                    address: 0x80000,
                    source: source(0x800, 0x400),
                },
                Region {
                    name: "kernel0".into(),
                    address: 0x40000,
                    source: source(0x100, 32),
                },
                Region {
                    name: "application".into(),
                    address: 0x200000,
                    source: source(0, 0x400),
                },
                Region {
                    name: "section5".into(),
                    address: 0x210000,
                    source: source(0x40080, 256),
                },
            ],
        };
        (input, plan, ram)
    }
    #[test]
    fn both_block_geometries_preserve_the_stock_reader_and_append_exact_card_bytes() {
        for shift in [14, 17] {
            let (input, plan, mut ram) = fixture(shift);
            assert_eq!(preload_call(&ram, &plan).unwrap(), (0x200200, shift));
            let original = ram.clone();
            let tail = ram.len();
            install(&input, &plan, &mut ram).unwrap();
            let size = 1 << shift;
            assert_eq!(&ram[tail..], &input[0x40080..0x40080 + size]);
            assert_eq!(&ram[0x200304..0x200340], &original[0x200304..0x200340]);
            assert_eq!(call(&ram, 0x200200).unwrap(), 0x100000);
        }
    }
    #[test]
    fn changed_selector_geometry_or_occupied_reservation_refuses() {
        for address in [0x20017c, 0x200170, 0x200140, 0x100000] {
            let (input, plan, mut ram) = fixture(17);
            ram[address] ^= 1;
            assert!(install(&input, &plan, &mut ram).is_err(), "{address:x}");
        }
    }
}
