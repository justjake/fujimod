//! The fast FFBOOT read program placed in FFINIT's zero gap, and its bindings.
//!
//! FFINIT's read pointer selects the program, which reads the first block
//! through the boot ROM and the rest through the SD host. FFINIT's CRC32
//! routine branches to the program's CRC32, which returns the CRC32 the
//! reader verified for exactly that extent and computes any other in full.
//! The program sits at file offset 0x2000, between the DDR setup code and its
//! stack (code end + 0x400) and the template at 0x4000, so FFINIT keeps its
//! length. An X-T3 booted this SD card in about 10 s instead of 35 s (2026-11-10).
//!
//! The program's read plan follows the fast loader of xt3-cfw
//! (https://github.com/johnborillo/xt3-cfw), distributed under this notice:
//!
//! MIT License
//!
//! Copyright (c) 2026 the xt3-cfw contributors
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.
use crate::image::{self, Result, put, require, word};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::LazyLock};

const PROGRAM: &[u8] = include_bytes!("../../assets/sd-boot-v1-xp4-sd-card-fast-read.generated.bin");
static LAYOUT: LazyLock<Layout> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../assets/sd-boot-v1-xp4-sd-card-fast-read.generated.json"
    ))
    .expect("checked XP4 SD card fast read layout")
});

/// The Topaz boot ROM's SD host register base (X100V ROM literal at
/// `0xfff032b4`; the X-T3 and X100V file ABIs are identical).
pub const TOPAZ_SD_HOST: u32 = 0xfffc4000;
/// DRAM scratch after the boot ROM's filesystem objects in FFINIT's workspace.
const SCRATCH_OFFSET: u32 = 0x1000;
const SCRATCH_BYTES: u32 = 0x4000;
/// File offset of the program inside FFINIT's zero gap (SRAM 0xfff12000 on
/// the Topaz boot ROM's 0xfff10000 load address).
pub const OFFSET: usize = 0x2000;
/// FFINIT's template, which follows the zero gap.
const TEMPLATE: usize = 0x4000;
/// The DDR setup code's stack above its own end (the X-T3 setup's SP literal
/// at file 0x9c is its code end + 0x400).
const DDR_SETUP_STACK: usize = 0x400;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    schema: u32,
    size: usize,
    symbols: BTreeMap<String, usize>,
    fixups: Vec<usize>,
}

/// Where the program sits in an FFINIT placed at `base`.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub read: u32,
    pub crc32: u32,
    pub config: u32,
    pub status: u32,
}

pub fn placement(base: u32) -> Result<Placement> {
    let layout = &*LAYOUT;
    require(
        layout.schema == 1 && layout.size == PROGRAM.len(),
        "Invalid SD card fast read layout",
    )?;
    let at = |name: &str| -> Result<u32> {
        let value = *layout
            .symbols
            .get(name)
            .ok_or_else(|| image::Error::Invalid(format!("Missing {name}")))?;
        require(value < layout.size, "SD card fast read symbol outside program")?;
        base.checked_add((OFFSET + value) as u32)
            .ok_or_else(|| image::Error::Invalid("SD card fast read placement overflow".into()))
    };
    Ok(Placement {
        read: at("sd_card_fast_read")?,
        crc32: at("sd_card_fast_read_crc32")?,
        config: at("sd_card_fast_read_config")?,
        status: at("sd_card_fast_read_status")?,
    })
}

pub struct Bindings {
    pub rom_read: u32,
    pub workspace: u32,
    pub crc_table: u32,
    pub expected_crc32: u32,
    /// Address of FFINIT's CRC32 routine, redirected to the program's.
    pub template_crc32: u32,
}

/// Place the program at `OFFSET` in `output` (FFINIT placed at `base`, its
/// DDR setup code `code_size` bytes long) and redirect the template's CRC32
/// routine. The caller selects the program as FFINIT's read.
pub fn install(output: &mut [u8], base: u32, code_size: usize, bindings: &Bindings) -> Result<Placement> {
    let placed = placement(base)?;
    require(
        code_size + DDR_SETUP_STACK <= OFFSET && OFFSET + PROGRAM.len() <= TEMPLATE,
        "SD card fast read does not fit FFINIT's zero gap",
    )?;
    require(
        output.len() > TEMPLATE && extent_zero(output, OFFSET, PROGRAM.len())?,
        "FFINIT's zero gap is occupied",
    )?;
    let scratch = bindings
        .workspace
        .checked_add(SCRATCH_OFFSET)
        .filter(|s| s.checked_add(SCRATCH_BYTES).is_some() && s.is_multiple_of(512))
        .ok_or_else(|| image::Error::Invalid("Invalid SD card fast read scratch".into()))?;
    let mut program = PROGRAM.to_vec();
    let config = (placed.config - base) as usize - OFFSET;
    require(
        extent_zero(&program, config, 20)?,
        "SD card fast read configuration is occupied",
    )?;
    for &at in &LAYOUT.fixups {
        let value = word(&program, at)?;
        require((value as usize) < program.len(), "SD card fast read fixup outside program")?;
        put(&mut program, at, value + base + OFFSET as u32)?;
    }
    for (index, value) in [
        TOPAZ_SD_HOST,
        bindings.rom_read,
        scratch,
        bindings.crc_table,
        bindings.expected_crc32,
    ]
    .into_iter()
    .enumerate()
    {
        put(&mut program, config + index * 4, value)?;
    }
    output[OFFSET..OFFSET + program.len()].copy_from_slice(&program);
    let site = bindings
        .template_crc32
        .checked_sub(base)
        .map(|s| s as usize)
        .filter(|s| *s >= TEMPLATE && s + 4 <= output.len() && s.is_multiple_of(4))
        .ok_or_else(|| image::Error::Invalid("FFINIT CRC32 routine outside FFINIT".into()))?;
    let delta = i64::from(placed.crc32) - i64::from(bindings.template_crc32) - 8;
    require(delta % 4 == 0, "SD card fast read CRC32 branch is misaligned")?;
    put(output, site, 0xea000000 | ((delta >> 2) as u32 & 0xffffff))?;
    Ok(placed)
}

fn extent_zero(bytes: &[u8], at: usize, size: usize) -> Result<bool> {
    Ok(image::extent(bytes, at, size)?.iter().all(|b| *b == 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings(base: u32) -> Bindings {
        Bindings {
            rom_read: 0xfff025e4,
            workspace: 0x03cb0000,
            crc_table: base + 0x4380,
            expected_crc32: base + 0x4788,
            template_crc32: base + 0x4158,
        }
    }

    /// A template-shaped FFINIT: DDR setup code, a zero gap, the template.
    fn ffinit() -> Vec<u8> {
        let mut output = vec![0x11; 0x9f68];
        output[0x1ac0..TEMPLATE].fill(0);
        output
    }

    #[test]
    fn program_fills_the_zero_gap_and_is_reached_from_the_template_crc32() {
        let base = 0xfff10000;
        let mut output = ffinit();
        let placed = install(&mut output, base, 0x1ac0, &bindings(base)).unwrap();
        assert_eq!(output.len(), 0x9f68);
        let config = (placed.config - base) as usize;
        let words: Vec<u32> = (0..5).map(|i| word(&output, config + i * 4).unwrap()).collect();
        assert_eq!(words, [0xfffc4000, 0xfff025e4, 0x03cb1000, base + 0x4380, base + 0x4788]);
        let end = OFFSET + PROGRAM.len();
        for &at in &LAYOUT.fixups {
            let value = word(&output, OFFSET + at).unwrap();
            assert!((base + OFFSET as u32..base + end as u32).contains(&value));
        }
        let branch = word(&output, 0x4158).unwrap();
        assert_eq!(branch >> 24, 0xea);
        let target = (0x4158 + 8 + (((branch & 0xffffff) << 8) as i32 >> 6)) as u32;
        assert_eq!(base + target, placed.crc32);
        // Outside the program only the redirected word changes.
        let before = ffinit();
        let changed: Vec<usize> = (0..output.len())
            .filter(|&i| output[i] != before[i] && !(OFFSET..end).contains(&i))
            .collect();
        assert_eq!(changed, [0x4158, 0x4159, 0x415a, 0x415b]);
    }

    #[test]
    fn occupied_gap_long_setup_code_unaligned_scratch_and_outside_crc32_site_refuse() {
        let base = 0xfff10000;
        let mut occupied = ffinit();
        occupied[OFFSET + 0x100] = 1;
        assert!(install(&mut occupied, base, 0x1ac0, &bindings(base)).is_err());
        assert!(install(&mut ffinit(), base, 0x1c01, &bindings(base)).is_err());
        let mut b = bindings(base);
        b.workspace = 0x03cb0004;
        assert!(install(&mut ffinit(), base, 0x1ac0, &b).is_err());
        let mut b = bindings(base);
        b.template_crc32 = base + 0x3000;
        assert!(install(&mut ffinit(), base, 0x1ac0, &b).is_err());
    }
}
