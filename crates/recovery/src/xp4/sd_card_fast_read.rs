//! The fast FFBOOT read program placed in FFINIT's zero gap, and its bindings.
//!
//! FFINIT's read pointer selects the program, which reads the first block
//! through the boot ROM and the rest through the SD host, computing the
//! CRC32 of each chunk while SDMA reads the next. FFINIT's CRC32 routine
//! branches to the program's CRC32, which returns the CRC32 the reader
//! verified for exactly that extent and computes any other in full.
//! `ffinit_dram_ready`'s branch to `ffinit_load` enters the program's entry
//! hook, which turns the I-cache on for the load. `ffinit_load`'s call of
//! `ffinit_enter_ram`, made after FFINIT's CRC32 check passed, enters the
//! program's RAM-entry hook. When a packed FFBOOT stage is bound
//! (`bind_packed_stage`), that hook performs the stage's copies and zero runs
//! with the I-cache on, as the stage itself would with every cache off, and
//! RAM holds the complete image when FFBOOT is entered. The hook then turns
//! the I-cache off and continues in `ffinit_enter_ram`.
//!
//! The program sits at file offset 0x2000, between the DDR setup code and its
//! stack (code end + 0x400) and the template at 0x4000, so FFINIT keeps its
//! length. An X-T3 (5.11) booted a packed FFBOOT with the bound stage in about
//! 5 s to live view, instead of about 35 s to the green LED with the boot ROM
//! read (2026-11-10).
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
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range, sync::LazyLock};

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
/// Configuration words: host, boot ROM read, scratch, CRC32 table, expected
/// CRC32 word, then the packed stage's address, header offset, records
/// offset and size.
const CONFIG_WORDS: usize = 9;
const PACKED_STAGE_WORD: usize = 5;
/// The RAM-entry hook copies the packed stage's header and records to the
/// scratch at +0x2000 (the extent list, no longer needed), which leaves
/// 0x2000 bytes.
const PACKED_STAGE_LIMIT: u32 = 0x2000;
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
    /// Entry hook, reached from `ffinit_dram_ready` instead of `ffinit_load`.
    pub entry: u32,
    /// RAM-entry hook, called from `ffinit_load` instead of `ffinit_enter_ram`.
    pub enter_ram: u32,
    /// Words holding the addresses the hooks continue at.
    pub ffinit_load: u32,
    pub ffinit_enter_ram: u32,
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
        entry: at("sd_card_fast_read_entry")?,
        enter_ram: at("sd_card_fast_read_enter_ram")?,
        ffinit_load: at("sd_card_fast_read_ffinit_load")?,
        ffinit_enter_ram: at("sd_card_fast_read_ffinit_enter_ram")?,
    })
}

pub struct Bindings {
    pub rom_read: u32,
    pub workspace: u32,
    pub crc_table: u32,
    pub expected_crc32: u32,
    /// Address of FFINIT's CRC32 routine, redirected to the program's.
    pub template_crc32: u32,
    /// `ffinit_load`'s address range. Its call of `ffinit_enter_ram` is
    /// redirected to the RAM-entry hook.
    pub ffinit_load: Range<u32>,
    /// `ffinit_dram_ready`'s address range. Its branch to `ffinit_load` is
    /// redirected to the entry hook.
    pub ffinit_dram_ready: Range<u32>,
    pub ffinit_enter_ram: u32,
}

/// FFBOOT's packed stage as the RAM image holds it once loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackedStage {
    /// RAM address of the stage, which the reset literal enters.
    pub address: u32,
    /// Offsets of the stage's header (scratch, size, origin, continuation,
    /// vector, vector value, copy count, zero-run count) and of its records.
    pub header: u32,
    pub records: u32,
    /// Stage bytes, header and records included.
    pub size: u32,
}

/// Place the program at `OFFSET` in `output` (FFINIT placed at `base`, its
/// DDR setup code `code_size` bytes long), redirect the template's CRC32
/// routine and route `ffinit_load`'s entry and its call of `ffinit_enter_ram`
/// through the program's hooks. The caller selects the program as FFINIT's
/// read. No packed stage is bound.
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
        extent_zero(&program, config, CONFIG_WORDS * 4)?,
        "SD card fast read configuration is occupied",
    )?;
    let load_word = (placed.ffinit_load - base) as usize - OFFSET;
    let enter_ram_word = (placed.ffinit_enter_ram - base) as usize - OFFSET;
    require(
        extent_zero(&program, load_word, 4)? && extent_zero(&program, enter_ram_word, 4)?,
        "SD card fast read continuation words are occupied",
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
    put(&mut program, load_word, bindings.ffinit_load.start)?;
    put(&mut program, enter_ram_word, bindings.ffinit_enter_ram)?;
    let load_branch = branch_site(
        output,
        base,
        &bindings.ffinit_dram_ready,
        bindings.ffinit_load.start,
        false,
    )?;
    let enter_ram_call = branch_site(
        output,
        base,
        &bindings.ffinit_load,
        bindings.ffinit_enter_ram,
        true,
    )?;
    output[OFFSET..OFFSET + program.len()].copy_from_slice(&program);
    put(output, load_branch, branch(base + load_branch as u32, placed.entry, false)?)?;
    put(output, enter_ram_call, branch(base + enter_ram_call as u32, placed.enter_ram, true)?)?;
    let site = bindings
        .template_crc32
        .checked_sub(base)
        .map(|s| s as usize)
        .filter(|s| *s >= TEMPLATE && s + 4 <= output.len() && s.is_multiple_of(4))
        .ok_or_else(|| image::Error::Invalid("FFINIT CRC32 routine outside FFINIT".into()))?;
    put(output, site, branch(bindings.template_crc32, placed.crc32, false)?)?;
    Ok(placed)
}

/// Record FFBOOT's packed stage in a fast-read FFINIT placed at `base`, so
/// FFINIT performs the stage before entering RAM. The stage still runs
/// itself when the reset literal no longer enters it.
pub fn bind_packed_stage(output: &mut [u8], base: u32, stage: &PackedStage) -> Result<()> {
    let placed = placement(base)?;
    require(
        stage.size > 0
            && stage.size <= PACKED_STAGE_LIMIT
            && stage.size.is_multiple_of(32)
            && stage.address.is_multiple_of(32)
            && stage.header.is_multiple_of(4)
            && stage.header.checked_add(32).is_some_and(|end| end <= stage.size)
            && stage.records.is_multiple_of(4)
            && stage.records <= stage.size,
        "Invalid packed FFBOOT stage",
    )?;
    let config = (placed.config - base) as usize;
    require(
        output.len() > TEMPLATE && word(output, config)? == TOPAZ_SD_HOST,
        "FFINIT has no SD card fast read",
    )?;
    let words = config + PACKED_STAGE_WORD * 4;
    require(
        extent_zero(output, words, 16)?,
        "FFINIT already records a packed FFBOOT stage",
    )?;
    for (index, value) in [stage.address, stage.header, stage.records, stage.size]
        .into_iter()
        .enumerate()
    {
        put(output, words + index * 4, value)?;
    }
    Ok(())
}

/// ARM `b` or `bl` at `site` to `target`.
fn branch(site: u32, target: u32, link: bool) -> Result<u32> {
    let delta = i64::from(target) - i64::from(site) - 8;
    require(
        delta % 4 == 0 && (-(1 << 25)..(1 << 25)).contains(&delta),
        "SD card fast read branch out of range",
    )?;
    Ok(if link { 0xeb000000 } else { 0xea000000 } | ((delta >> 2) as u32 & 0xffffff))
}

/// File offset of the one `b`/`bl` to `target` inside `function`.
fn branch_site(
    output: &[u8],
    base: u32,
    function: &Range<u32>,
    target: u32,
    link: bool,
) -> Result<usize> {
    let mut sites = Vec::new();
    let mut address = function.start;
    while address < function.end {
        let at = address
            .checked_sub(base)
            .map(|a| a as usize)
            .filter(|a| *a >= TEMPLATE && a + 4 <= output.len())
            .ok_or_else(|| image::Error::Invalid("FFINIT function outside FFINIT".into()))?;
        if word(output, at)? == branch(address, target, link)? {
            sites.push(at);
        }
        address += 4;
    }
    match sites.as_slice() {
        [site] => Ok(*site),
        _ => Err(image::Error::Invalid(format!(
            "Expected one branch to {target:#x} in FFINIT {:#x}..{:#x}",
            function.start, function.end
        ))),
    }
}

fn extent_zero(bytes: &[u8], at: usize, size: usize) -> Result<bool> {
    Ok(image::extent(bytes, at, size)?.iter().all(|b| *b == 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0xfff10000;
    /// `ffinit_dram_ready`'s `b ffinit_load` and `ffinit_load`'s
    /// `bl ffinit_enter_ram` in the template.
    const LOAD_BRANCH: usize = 0x4330;
    const ENTER_RAM_CALL: usize = 0x42ec;

    fn bindings(base: u32) -> Bindings {
        Bindings {
            rom_read: 0xfff025e4,
            workspace: 0x03cb0000,
            crc_table: base + 0x4380,
            expected_crc32: base + 0x4788,
            template_crc32: base + 0x4158,
            ffinit_load: base + 0x4198..base + 0x4300,
            ffinit_dram_ready: base + 0x4320..base + 0x4338,
            ffinit_enter_ram: base + 0x4340,
        }
    }

    /// A template-shaped FFINIT: DDR setup code, a zero gap, the template
    /// with its two branches into FFINIT's load and RAM entry.
    fn ffinit() -> Vec<u8> {
        let mut output = vec![0x11; 0x9f68];
        output[0x1ac0..TEMPLATE].fill(0);
        put(&mut output, LOAD_BRANCH, 0xeaffff98).unwrap();
        put(&mut output, ENTER_RAM_CALL, 0xeb000013).unwrap();
        output
    }

    fn target(output: &[u8], site: usize) -> u32 {
        let branch = word(output, site).unwrap();
        BASE + (site as i64 + 8 + i64::from(((branch & 0xffffff) << 8) as i32 >> 6)) as u32
    }

    #[test]
    fn program_fills_the_zero_gap_and_is_reached_from_the_template() {
        let mut output = ffinit();
        let placed = install(&mut output, BASE, 0x1ac0, &bindings(BASE)).unwrap();
        assert_eq!(output.len(), 0x9f68);
        let config = (placed.config - BASE) as usize;
        let words: Vec<u32> = (0..CONFIG_WORDS)
            .map(|i| word(&output, config + i * 4).unwrap())
            .collect();
        assert_eq!(
            words,
            [0xfffc4000, 0xfff025e4, 0x03cb1000, BASE + 0x4380, BASE + 0x4788, 0, 0, 0, 0]
        );
        let end = OFFSET + PROGRAM.len();
        for &at in &LAYOUT.fixups {
            let value = word(&output, OFFSET + at).unwrap();
            assert!((BASE + OFFSET as u32..BASE + end as u32).contains(&value));
        }
        assert_eq!(word(&output, 0x4158).unwrap() >> 24, 0xea);
        assert_eq!(target(&output, 0x4158), placed.crc32);
        assert_eq!(word(&output, LOAD_BRANCH).unwrap() >> 24, 0xea);
        assert_eq!(target(&output, LOAD_BRANCH), placed.entry);
        assert_eq!(word(&output, ENTER_RAM_CALL).unwrap() >> 24, 0xeb);
        assert_eq!(target(&output, ENTER_RAM_CALL), placed.enter_ram);
        // The hooks continue at the template's own targets.
        assert_eq!(word(&output, (placed.ffinit_load - BASE) as usize).unwrap(), BASE + 0x4198);
        assert_eq!(
            word(&output, (placed.ffinit_enter_ram - BASE) as usize).unwrap(),
            BASE + 0x4340
        );
        // Outside the program only the three redirected words change.
        let before = ffinit();
        let changed: Vec<usize> = (0..output.len())
            .filter(|&i| output[i] != before[i] && !(OFFSET..end).contains(&i))
            .map(|i| i & !3)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(changed, [0x4158, ENTER_RAM_CALL, LOAD_BRANCH]);
    }

    #[test]
    fn occupied_gap_long_setup_code_unaligned_scratch_and_outside_crc32_site_refuse() {
        let mut occupied = ffinit();
        occupied[OFFSET + 0x100] = 1;
        assert!(install(&mut occupied, BASE, 0x1ac0, &bindings(BASE)).is_err());
        assert!(install(&mut ffinit(), BASE, 0x1c01, &bindings(BASE)).is_err());
        let mut b = bindings(BASE);
        b.workspace = 0x03cb0004;
        assert!(install(&mut ffinit(), BASE, 0x1ac0, &b).is_err());
        let mut b = bindings(BASE);
        b.template_crc32 = BASE + 0x3000;
        assert!(install(&mut ffinit(), BASE, 0x1ac0, &b).is_err());
    }

    #[test]
    fn a_missing_or_repeated_template_branch_refuses() {
        let mut missing = ffinit();
        put(&mut missing, LOAD_BRANCH, 0xe1a00000).unwrap();
        assert!(install(&mut missing, BASE, 0x1ac0, &bindings(BASE)).is_err());
        let mut repeated = ffinit();
        // A second `bl ffinit_enter_ram` four bytes earlier in ffinit_load.
        put(&mut repeated, ENTER_RAM_CALL - 4, 0xeb000014).unwrap();
        assert!(install(&mut repeated, BASE, 0x1ac0, &bindings(BASE)).is_err());
        let mut b = bindings(BASE);
        b.ffinit_enter_ram = BASE + 0x4348;
        assert!(install(&mut ffinit(), BASE, 0x1ac0, &b).is_err());
    }

    fn stage() -> PackedStage {
        PackedStage { address: 0x104000, header: 0x1a0, records: 0x1c0, size: 0x480 }
    }

    #[test]
    fn a_packed_stage_is_recorded_once_after_the_base_configuration() {
        let mut output = ffinit();
        let placed = install(&mut output, BASE, 0x1ac0, &bindings(BASE)).unwrap();
        let installed = output.clone();
        bind_packed_stage(&mut output, BASE, &stage()).unwrap();
        let config = (placed.config - BASE) as usize;
        let words: Vec<u32> = (PACKED_STAGE_WORD..CONFIG_WORDS)
            .map(|i| word(&output, config + i * 4).unwrap())
            .collect();
        assert_eq!(words, [0x104000, 0x1a0, 0x1c0, 0x480]);
        let changed: Vec<usize> = (0..output.len())
            .filter(|&i| output[i] != installed[i])
            .collect();
        assert!(changed.iter().all(|&i| (config + 20..config + 36).contains(&i)));
        assert!(bind_packed_stage(&mut output, BASE, &stage()).is_err());
    }

    #[test]
    fn invalid_stages_and_an_ffinit_without_the_program_refuse() {
        let mut output = ffinit();
        install(&mut output, BASE, 0x1ac0, &bindings(BASE)).unwrap();
        for invalid in [
            PackedStage { size: 0, ..stage() },
            PackedStage { size: 0x470, ..stage() },
            PackedStage { size: PACKED_STAGE_LIMIT + 32, ..stage() },
            PackedStage { address: 0x104010, ..stage() },
            PackedStage { header: 0x462, ..stage() },
            PackedStage { header: 0x480, ..stage() },
            PackedStage { records: 0x484, ..stage() },
        ] {
            assert!(bind_packed_stage(&mut output.clone(), BASE, &invalid).is_err(), "{invalid:?}");
        }
        assert!(bind_packed_stage(&mut ffinit(), BASE, &stage()).is_err());
    }
}
