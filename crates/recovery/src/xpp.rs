//! Agate's read-only descriptor path and card-file namespace.
use crate::{
    Proof,
    image::{self, Plan, Result, extent, hash, put, require, word},
    rom_sd::{call, sequence},
};
use serde::{Deserialize, Serialize};
const IMAGE: &[u8] = include_bytes!("../assets/xpp-reader.generated.bin");
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    schema: usize,
    address: usize,
    size: usize,
    entry: usize,
    config: usize,
}
#[derive(Debug, Serialize)]
pub struct Reads {
    pub byte_reader: Proof,
    pub translator: Proof,
    pub copy_wrapper: Proof,
    pub descriptor_reader: Proof,
    pub call_sites: Vec<Proof>,
    pub pointer_sites: Vec<Proof>,
    pub dat_offset: usize,
    pub namespace_size: usize,
}
fn pin(ram: &[u8], address: usize, size: usize) -> Result<Proof> {
    Ok(Proof {
        address,
        size,
        sha256: fujimod_firmware::sha256(extent(ram, address, size)?),
    })
}
fn half(w: u32, top: bool) -> Result<u32> {
    require(
        w & 0xfff0f000 == if top { 0xe3403000 } else { 0xe3003000 },
        "Agate byte limit is not MOVW/MOVT R3",
    )?;
    Ok((w >> 4 & 0xf000) | (w & 0xfff))
}
fn locate_ram(inverted: &[u8], plan: &Plan, ram: &[u8]) -> Result<Reads> {
    let mut found = Vec::new();
    for region in &plan.regions {
        for at in (region.address..(region.address + region.source.decoded_size).saturating_sub(68))
            .step_by(4)
        {
            if sequence(ram, at, &[0xe92d4830, 0xe28db00c, 0xe1a05001, 0xe1a04002])
                && word(ram, at + 24)? == 0xe1500003
                && word(ram, at + 28)? == 0x8a000006
                && sequence(
                    ram,
                    at + 36,
                    &[0xe1a01000, 0xe1a00005, 0xe1a02004, 0xe3a03001],
                )
                && sequence(ram, at + 56, &[0xe8bd8830, 0xe3e0001d, 0xe8bd8830])
            {
                found.push(at);
            }
        }
    }
    require(found.len() == 1, "No unique Agate firmware byte reader")?;
    let at = found[0];
    let limit =
        (half(word(ram, at + 16)?, false)? | half(word(ram, at + 20)?, true)? << 16) as usize + 1;
    let translator = call(ram, at + 32)?;
    require(
        sequence(
            ram,
            translator,
            &[
                0xe52db004, 0xe28db000, 0xe280020f, 0xe24bd000, 0xe49db004, 0xe12fff1e,
            ],
        ),
        "Agate firmware translator differs",
    )?;
    let copy = call(ram, at + 52)?;
    require(
        sequence(
            ram,
            copy,
            &[
                0xe92d4800, 0xe28db004, 0xe24dd010, 0xe50b0014, 0xe50b1010, 0xe50b200c, 0xe54b3008,
                0xe3a03001, 0xe54b3007, 0xe24b0014,
            ],
        ) && sequence(ram, copy + 44, &[0xe24bd004, 0xe8bd8800]),
        "Agate descriptor wrapper differs",
    )?;
    let descriptor = call(ram, copy + 40)?;
    require(
        sequence(
            ram,
            descriptor,
            &[
                0xe92d4ff0, 0xe28db020, 0xe24dd014, 0xe5905000, 0xe5904004, 0xe5906008, 0xe5d0700c,
                0xe5d0a00d,
            ],
        ),
        "Agate read-only descriptor ABI differs",
    )?;
    require(
        copy > descriptor && copy - descriptor <= 0x1000,
        "Agate read descriptor body bound differs",
    )?;
    let table = plan
        .regions
        .iter()
        .find(|r| r.name == "section-table")
        .ok_or_else(|| image::Error::Invalid("Missing Agate section table".into()))?;
    let section = plan
        .regions
        .iter()
        .find(|r| r.name == "section-5")
        .ok_or_else(|| image::Error::Invalid("Missing Agate section-5 mapping".into()))?;
    let row = table.source.dat_offset + 0x40 + 5 * 36;
    require(
        word(inverted, row)? == 5 && word(inverted, row + 4)? as usize == section.address,
        "Agate section-5 source root differs",
    )?;
    let bias = section
        .source
        .dat_offset
        .checked_sub(word(inverted, row + 20)? as usize)
        .ok_or_else(|| image::Error::Invalid("Agate DAT bias underflows".into()))?;
    let size = inverted
        .len()
        .checked_sub(bias)
        .and_then(|n| n.checked_add(0xffff))
        .map(|n| n & !0xffff)
        .ok_or_else(|| image::Error::Invalid("Agate firmware namespace overflows".into()))?;
    require(
        size >= limit && limit <= 0x8000000,
        "Agate DAT namespace differs from the firmware read partition",
    )?;
    let mut calls = Vec::new();
    let mut pointers = Vec::new();
    for region in &plan.regions {
        for at in (region.address..region.address + region.source.decoded_size - 3).step_by(4) {
            let w = word(ram, at)?;
            let target = at as i64 + 8 + ((((w & 0xffffff) as i32) << 8 >> 6) as i64);
            if w & 0x0f000000 == 0x0b000000
                && w >> 28 < 15
                && usize::try_from(target).ok() == Some(descriptor)
            {
                calls.push(pin(ram, at, 4)?);
            }
            if w as usize == descriptor {
                pointers.push(pin(ram, at, 4)?);
            }
        }
    }
    require(
        !calls.is_empty(),
        "Agate descriptor reader has no checked calls",
    )?;
    Ok(Reads {
        byte_reader: pin(ram, found[0], 68)?,
        translator: pin(ram, translator, 24)?,
        copy_wrapper: pin(ram, copy, 52)?,
        descriptor_reader: pin(ram, descriptor, copy - descriptor)?,
        call_sites: calls,
        pointer_sites: pointers,
        dat_offset: bias,
        namespace_size: limit,
    })
}
pub fn locate(inverted: &[u8], plan: &Plan) -> Result<Reads> {
    locate_ram(inverted, plan, &image::transform(inverted, plan)?)
}
pub(super) fn install(inverted: &[u8], plan: &Plan, ram: &mut Vec<u8>) -> Result<()> {
    let reads = locate_ram(inverted, plan, ram)?;
    let layout: Layout = serde_json::from_str(include_str!("../assets/xpp-reader.generated.json"))
        .map_err(|e| image::Error::Invalid(e.to_string()))?;
    require(
        layout.schema == 1
            && layout.address == 0x100000
            && layout.size == IMAGE.len()
            && layout.entry < IMAGE.len()
            && layout.config + 32 <= IMAGE.len(),
        "Invalid Agate card reader layout",
    )?;
    for region in &plan.regions {
        require(
            layout.address + IMAGE.len() <= region.address
                || region.address + region.source.decoded_size <= layout.address,
            "Agate card reader overlaps initialized firmware",
        )?;
    }
    for region in &plan.bss {
        require(
            layout.address + IMAGE.len() <= region.address
                || region.address + region.size <= layout.address,
            "Agate card reader overlaps firmware BSS",
        )?;
    }
    require(
        extent(ram, layout.address, IMAGE.len())?
            .iter()
            .all(|b| *b == 0),
        "Agate card reader reservation is occupied",
    )?;
    let tail = (ram.len() + 3) & !3;
    let total = tail
        .checked_add(reads.namespace_size.max(inverted.len() - reads.dat_offset))
        .ok_or_else(|| image::Error::Invalid("Agate OS image overflows".into()))?;
    require(total < 0x40000000, "Agate OS image escapes RAM")?;
    let mut program = IMAGE.to_vec();
    // Agate ROM's drive-number ABI and mounted Petit-FatFs object.
    for (i, v) in [
        0xfff02548,
        0xfff027b8,
        0xfff02880,
        0xfff0df0c,
        tail,
        reads.namespace_size,
        total,
        reads.descriptor_reader.address,
    ]
    .into_iter()
    .enumerate()
    {
        put(
            &mut program,
            layout.config + i * 4,
            u32::try_from(v).map_err(|_| {
                image::Error::Invalid("Agate reader configuration exceeds ARM32".into())
            })?,
        )?;
    }
    let entry = layout.address + layout.entry;
    for site in &reads.call_sites {
        hash(extent(ram, site.address, site.size)?, &site.sha256)?;
        let delta = entry as i64 - site.address as i64 - 8;
        require(
            delta % 4 == 0 && (-0x2000000..0x2000000).contains(&delta),
            "Agate read hook is out of BL range",
        )?;
        let opcode = word(ram, site.address)? & 0xff000000;
        put(ram, site.address, opcode | ((delta >> 2) as u32 & 0xffffff))?;
    }
    for site in &reads.pointer_sites {
        hash(extent(ram, site.address, 4)?, &site.sha256)?;
        put(ram, site.address, entry as u32)?;
    }
    ram[layout.address..layout.address + program.len()].copy_from_slice(&program);
    ram.resize(tail, 0);
    ram.extend_from_slice(&inverted[reads.dat_offset..]);
    ram.resize(total, 255);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{Compression, Region, Source};
    fn branch(at: usize, to: usize) -> u32 {
        0xeb000000 | (((to as i64 - at as i64 - 8) >> 2) as u32 & 0xffffff)
    }
    fn fixture() -> (Vec<u8>, Plan, Vec<u8>) {
        let mut input = vec![0x5a; 0x18100];
        for (off, val) in [(0, 5), (4, 0x210000), (20, 0x1000)] {
            put(&mut input, 0x400 + 0x40 + 5 * 36 + off, val).unwrap();
        }
        let mut ram = vec![0; 0x240000];
        for (base, words) in [
            (
                0x200000,
                vec![
                    0xe92d4830,
                    0xe28db00c,
                    0xe1a05001,
                    0xe1a04002,
                    0xe30f3fff,
                    0xe3403001,
                    0xe1500003,
                    0x8a000006,
                    branch(0x200020, 0x200080),
                    0xe1a01000,
                    0xe1a00005,
                    0xe1a02004,
                    0xe3a03001,
                    branch(0x200034, 0x200100),
                    0xe8bd8830,
                    0xe3e0001d,
                    0xe8bd8830,
                ],
            ),
            (
                0x200080,
                vec![
                    0xe52db004, 0xe28db000, 0xe280020f, 0xe24bd000, 0xe49db004, 0xe12fff1e,
                ],
            ),
            (
                0x2000c0,
                vec![
                    0xe92d4ff0, 0xe28db020, 0xe24dd014, 0xe5905000, 0xe5904004, 0xe5906008,
                    0xe5d0700c, 0xe5d0a00d,
                ],
            ),
            (
                0x200100,
                vec![
                    0xe92d4800,
                    0xe28db004,
                    0xe24dd010,
                    0xe50b0014,
                    0xe50b1010,
                    0xe50b200c,
                    0xe54b3008,
                    0xe3a03001,
                    0xe54b3007,
                    0xe24b0014,
                    branch(0x200128, 0x2000c0),
                    0xe24bd004,
                    0xe8bd8800,
                ],
            ),
        ] {
            for (i, w) in words.into_iter().enumerate() {
                put(&mut ram, base + i * 4, w).unwrap();
            }
        }
        put(&mut ram, 0x200280, branch(0x200280, 0x2000c0)).unwrap();
        put(&mut ram, 0x200300, 0x2000c0).unwrap();
        let source = |at, size| Source {
            dat_offset: at,
            size,
            compression: Compression::Raw,
            decoded_size: size,
            sha256: String::new(),
            initialized_prefix: None,
        };
        let plan = Plan {
            size: ram.len(),
            entry: 0,
            bss: vec![],
            edits: vec![],
            regions: vec![
                Region {
                    name: "application".into(),
                    address: 0x200000,
                    source: source(0, 0x400),
                },
                Region {
                    name: "section-table".into(),
                    address: 0x80000,
                    source: source(0x400, 0x400),
                },
                Region {
                    name: "section-5".into(),
                    address: 0x210000,
                    source: source(0x1100, 32),
                },
            ],
        };
        (input, plan, ram)
    }
    #[test]
    fn all_descriptor_references_redirect_while_the_stock_reader_and_other_code_are_preserved() {
        let (input, plan, mut ram) = fixture();
        let original = ram.clone();
        let reads = locate_ram(&input, &plan, &ram).unwrap();
        assert_eq!(reads.dat_offset, 0x100);
        assert_eq!(reads.namespace_size, 0x20000);
        assert_eq!(
            reads
                .call_sites
                .iter()
                .map(|p| p.address)
                .collect::<Vec<_>>(),
            vec![0x200128, 0x200280]
        );
        assert_eq!(reads.pointer_sites[0].address, 0x200300);
        install(&input, &plan, &mut ram).unwrap();
        assert_eq!(
            &ram[original.len()..original.len() + input.len() - 0x100],
            &input[0x100..]
        );
        assert!(
            ram[original.len() + input.len() - 0x100..]
                .iter()
                .all(|b| *b == 255)
        );
        assert_eq!(&ram[0x2000c0..0x200100], &original[0x2000c0..0x200100]);
        for p in reads.call_sites.iter().chain(&reads.pointer_sites) {
            ram[p.address..p.address + 4].copy_from_slice(&original[p.address..p.address + 4]);
        }
        ram[0x100000..0x100000 + IMAGE.len()]
            .copy_from_slice(&original[0x100000..0x100000 + IMAGE.len()]);
        assert_eq!(&ram[..original.len()], original);
    }
    #[test]
    fn changed_read_abi_translation_or_occupied_program_refuses() {
        for at in [0x200080, 0x2000c0, 0x200100, 0x100000] {
            let (input, plan, mut ram) = fixture();
            ram[at] ^= 1;
            assert!(install(&input, &plan, &mut ram).is_err(), "{at:x}");
        }
    }
}
