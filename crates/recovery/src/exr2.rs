//! EXR II's callback-free preloaded section-5 read, removed from the card image.
//!
//! On the preloaded branch the section is already in RAM. The stock caller
//! reads its compressed flash blocks into a staging buffer without a
//! decompression callback and keeps only the call's status; nothing reads the
//! staged bytes before the buffer is reused. The card replaces that call with
//! a successful status, so this path reads no internal flash and needs no RAM
//! beyond the stock image.
use crate::{
    image::{Plan, Result, put, require, word},
    rom_sd::{call, sequence},
};
pub mod lamp;
pub mod footer;
/// `mov r0, #0`: the read call's success status.
const SUCCESS: u32 = 0xe3a00000;

fn preload_call(ram: &[u8], plan: &Plan) -> Result<usize> {
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
            found.push(at + 28);
        }
    }
    require(
        found.len() == 1,
        "No unique EXR II preloaded section-5 read",
    )?;
    Ok(found[0])
}

pub(super) fn install(plan: &Plan, ram: &mut [u8]) -> Result<()> {
    put(ram, preload_call(ram, plan)?, SUCCESS)
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
    fn fixture(shift: usize) -> (Plan, Vec<u8>) {
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
            regions: vec![Region {
                name: "application".into(),
                address: 0x200000,
                source: source(0, 0x400),
            }],
        };
        (plan, ram)
    }
    #[test]
    fn both_block_geometries_replace_only_the_preloaded_read_with_success() {
        for shift in [14, 17] {
            let (plan, mut ram) = fixture(shift);
            assert_eq!(preload_call(&ram, &plan).unwrap(), 0x200200);
            let mut expected = ram.clone();
            put(&mut expected, 0x200200, SUCCESS).unwrap();
            install(&plan, &mut ram).unwrap();
            assert_eq!(ram, expected);
        }
    }
    #[test]
    fn changed_selector_or_geometry_refuses() {
        for address in [0x20017c, 0x200170, 0x200140] {
            let (plan, mut ram) = fixture(17);
            ram[address] ^= 1;
            assert!(install(&plan, &mut ram).is_err(), "{address:x}");
        }
    }
}
