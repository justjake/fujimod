//! The checked redirect of an XP5 reset stage's raw section load.
//!
//! The stock reset stage (the image the stock initializer copies to RAM 0) loads the section table
//! and then, in a function of its own, the first section: a raw copy of `size` bytes from the flash
//! to the section's DRAM address, `copy(dst, flash_address(offset), size)`. The function takes the
//! flash address from a one-instruction translator call. This module finds that function in the
//! stock reset image by its instructions (every word is checked, the match must be unique and the
//! function must have exactly one caller) and plans the one-word change that makes the stage copy
//! from the DRAM copy of the same section instead: the translator call becomes
//! `orr x0, x20, #destination`, where `x20` is the section's DRAM address and `destination` is
//! where the read-only startup streamed `ffboot.bin`, so the source is `destination + dst`, the
//! place the RAM image puts the section in the file.
//!
//! Nothing here changes what the stock stage copies, only where from: the plan is refused unless
//! the card's bytes of the section equal the flash's, both section tables (the main one and the
//! alternate one the stage may pick) name the same section, and the one word is encodable. The
//! loader repeats the comparison against the real flash on the camera before it changes anything.
use super::{put64, READ_ONLY_CEILING, READ_ONLY_FLOOR};
use crate::image::{self, extent, require, word, Result};
use crate::xp4::crc32;

/// A masked instruction word: `(value, mask)`.
type Pat = (u32, u32);
const BL: Pat = (0x9400_0000, 0xfc00_0000);
const ADRP: u32 = 0x9000_0000;
const ADRP_MASK: u32 = 0x9f00_0000;

/// The function that loads section 0: look the row up, copy `size` bytes from the flash address
/// of the row's flash offset to the row's DRAM address, return 0. Word 8 is the translator call.
const SECTION_LOAD: [Pat; 18] = [
    (0xa9be_7bfd, !0), // stp x29, x30, [sp, #-32]!
    (0x9100_03fd, !0), // mov x29, sp
    (0xa901_53f3, !0), // stp x19, x20, [sp, #16]
    (0x5280_0000, !0), // mov w0, #0           (section id 0)
    BL,                // bl section_lookup
    (0xaa00_03f3, !0), // mov x19, x0          (the row)
    (0xf940_0414, !0), // ldr x20, [x0, #8]    (row.destination)
    (0xb940_2800, !0), // ldr w0, [x0, #40]    (row.flash_offset)
    BL,                // bl flash_address     <- the change
    (0xd280_0003, !0), // mov x3, #0
    (0xb940_3262, !0), // ldr w2, [x19, #48]   (row.size)
    (0xaa00_03e1, !0), // mov x1, x0
    (0xaa14_03e0, !0), // mov x0, x20
    BL,                // bl copy
    (0xd280_0000, !0), // mov x0, #0
    (0xa941_53f3, !0), // ldp x19, x20, [sp, #16]
    (0xa8c2_7bfd, !0), // ldp x29, x30, [sp], #32
    (0xd65f_03c0, !0), // ret
];
const SITE_WORD: usize = 8;
/// `adrp x1, page; ldr x1, [x1, #..]; ldr x1, [x1, #8]; ldr x1, [x1, #48]; add x0, x1, w0, uxtw; ret`
const FLASH_ADDRESS: [Pat; 6] = [
    (0x9000_0001, 0x9f00_001f),
    (0xf942_5021, !0),
    (0xf940_0421, !0),
    (0xf940_1821, !0),
    (0x8b20_4020, !0),
    (0xd65f_03c0, !0),
];
/// The section table copy: `table_pointer(ram); copy(ram, flash_address(table_flash), 0x800)`.
const TABLE_COPY: [Pat; 12] = [
    (0x9000_0013, 0x9f00_001f), // adrp x19, page
    (0x9100_0273, 0xffc0_03ff), // add x19, x19, #lo
    (0xaa13_03e0, !0),          // mov x0, x19
    BL,                         // bl set_table
    (0x9000_0000, 0x9f00_001f), // adrp x0, page
    (0x9100_0000, 0xffc0_03ff), // add x0, x0, #lo
    BL,                         // bl flash_address
    (0xd280_0003, !0),          // mov x3, #0
    (0x5280_0002, 0xffe0_001f), // mov w2, #size
    (0xaa00_03e1, !0),          // mov x1, x0
    (0xaa13_03e0, !0),          // mov x0, x19
    BL,                         // bl copy
];
/// The section table's rows: id at 0, 0x40 bytes each, ended by an id of all ones.
const ROW: usize = 0x40;
const END: u32 = 0xffff_ffff;

/// What the stock reset image and the update say about the redirect. Addresses are RAM addresses
/// (the image is loaded at 0); flash offsets are offsets in the flash (the update's DAT offset is
/// the flash offset plus `Setup::reset_dat - RESET_FLASH`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Plan {
    /// RAM-0 byte offset of the translator call, and its stock word.
    pub patch_offset: usize,
    pub patch_old: u32,
    /// The register that holds the section's DRAM address at the site (`x20`).
    pub dst_register: u32,
    /// The section: DRAM address, length and flash offset, from the table row.
    pub dst: u64,
    pub bytes: usize,
    pub flash_offset: u64,
    /// The two section tables the stage may read: RAM address, flash offset, length.
    pub tables: [(u64, u64); 2],
    pub table_bytes: usize,
}

/// The flash offset of the stock reset image (the stock initializer copies `0xf0040000..` to
/// RAM 0). It anchors the update's DAT offsets.
pub(super) const RESET_FLASH: usize = 0x4_0000;

fn matches(words: &[u8], at: usize, pattern: &[Pat]) -> bool {
    pattern.iter().enumerate().all(|(n, &(value, mask))| {
        word(words, at + 4 * n).map(|w| w & mask == value).unwrap_or(false)
    })
}

/// Every offset of `image` (word aligned) where `pattern` matches.
fn find(image: &[u8], pattern: &[Pat]) -> Vec<usize> {
    (0..image.len().saturating_sub(4 * pattern.len() - 1))
        .step_by(4)
        .filter(|&at| matches(image, at, pattern))
        .collect()
}

fn bl_target(image: &[u8], at: usize) -> Result<usize> {
    let w = word(image, at)?;
    require(w & BL.1 == BL.0, "The reset stage's call is not a bl")?;
    let delta = (((w & 0x03ff_ffff) as i64) ^ 0x0200_0000) - 0x0200_0000;
    usize::try_from(at as i64 + delta * 4)
        .map_err(|_| image::Error::Invalid("The reset stage's call leaves the image".into()))
}

/// Target of `adrp` at `at` (the image is loaded at address 0, so the page is `at & !0xfff` plus
/// the immediate).
fn adrp_target(image: &[u8], at: usize) -> Result<u64> {
    let w = word(image, at)?;
    require(w & ADRP_MASK == ADRP, "Not an adrp")?;
    let imm = (((w >> 5) & 0x7ffff) << 2) | ((w >> 29) & 3);
    let imm = ((i64::from(imm) ^ 0x10_0000) - 0x10_0000) as i128;
    u64::try_from((((at & !0xfff) as i128) + (imm << 12)).max(-1))
        .map_err(|_| image::Error::Invalid("adrp leaves the address space".into()))
}

/// `adrp x, page` then `add x, x, #lo`.
fn page_offset(image: &[u8], adrp: usize) -> Result<u64> {
    let add = word(image, adrp + 4)?;
    require(add & 0xffc0_0000 == 0x9100_0000, "The page offset is not an add of an unshifted immediate")?;
    Ok(adrp_target(image, adrp)? + u64::from((add >> 10) & 0xfff))
}

/// The section table's row for id 0: DRAM address, flash offset, size, and whether it is raw.
fn row0(table: &[u8], table_ram: u64) -> Result<(u64, u64, usize)> {
    require(table.len() >= 0x100, "A section table is too short")?;
    let rows = u64::from_le_bytes(table[..8].try_into().unwrap());
    let first = rows
        .checked_sub(table_ram)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n >= 0x40 && n < table.len())
        .ok_or_else(|| image::Error::Invalid("A section table's row pointer leaves the table".into()))?;
    let mut found = None;
    for at in (first..table.len().saturating_sub(ROW - 1)).step_by(ROW) {
        let id = word(table, at)?;
        if id == END {
            break;
        }
        if id == 0 {
            require(found.is_none(), "A section table has two rows for section 0")?;
            found = Some(at);
        }
    }
    let at = found.ok_or_else(|| image::Error::Invalid("A section table has no row for section 0".into()))?;
    let dst = u64::from_le_bytes(table[at + 8..at + 16].try_into().unwrap());
    let flash = u64::from(word(table, at + 40)?);
    let size = word(table, at + 48)? as usize;
    require(word(table, at + 56)? == 0, "Section 0 is not stored raw")?;
    Ok((dst, flash, size))
}

/// Find the redirect in `reset`, the stock reset image, and check it against the update
/// (`inverted`, whose flash offset `n` is at `n + bias`) and the card's RAM image `ram`.
pub(super) fn locate(reset: &[u8], inverted: &[u8], bias: usize, ram: &[u8]) -> Result<Plan> {
    let sites = find(reset, &SECTION_LOAD);
    require(sites.len() == 1, "The reset stage's section load is not found exactly once")?;
    let function = sites[0];
    // Exactly one caller, so the change reaches one path.
    let callers = (0..reset.len() - 3)
        .step_by(4)
        .filter(|&at| word(reset, at).is_ok_and(|w| w & BL.1 == BL.0) && bl_target(reset, at).ok() == Some(function))
        .count();
    require(callers == 1, "The reset stage's section load has not exactly one caller")?;
    let site = function + 4 * SITE_WORD;
    let translator = bl_target(reset, site)?;
    require(
        matches(reset, translator, &FLASH_ADDRESS),
        "The reset stage's flash address function differs",
    )?;
    // The two table copies call the same translator.
    let copies = find(reset, &TABLE_COPY);
    require(copies.len() == 2, "The reset stage's section table copies are not found twice")?;
    let mut tables = Vec::new();
    for &at in &copies {
        require(
            bl_target(reset, at + 4 * 6)? == translator
                && word(reset, at + 4 * 8)? == 0x5281_0002,
            "A section table copy differs (translator or 0x800 length)",
        )?;
        tables.push((page_offset(reset, at)?, page_offset(reset, at + 4 * 4)?));
    }
    tables.sort();
    require(
        tables.iter().all(|&(ram, flash)| ram % 0x800 == 0 && flash % 0x1000 == 0)
            && tables[0].0 + 0x800 == tables[1].0
            && tables[0].1 != tables[1].1,
        "The section tables are not two adjacent tables of two flash tables",
    )?;
    let table_bytes = 0x800;
    // Row 0 of each table, read from the flash itself (the update), must be one and the same row.
    let mut rows = Vec::new();
    for &(table_ram, flash) in &tables {
        let nor = extent(inverted, flash as usize + bias, table_bytes)?;
        rows.push(row0(nor, table_ram)?);
    }
    require(rows[0] == rows[1], "The two section tables name section 0 differently")?;
    let (dst, flash_offset, bytes) = rows[0];
    require(bytes != 0 && bytes % 4 == 0 && dst % 4 == 0, "Section 0's row is not a whole number of words")?;
    // The card's copy sits at its own address in the RAM image and has to equal the flash.
    let nor = extent(inverted, flash_offset as usize + bias, bytes)?;
    require(
        extent(ram, dst as usize, bytes)? == nor,
        "The RAM image's section 0 differs from the flash's",
    )?;
    // The cold table in the RAM image (the one the preloaded stage reads) is the flash's main one.
    let main = tables.iter().find(|&&(r, _)| r == 0x4_0000).copied();
    if let Some((table_ram, flash)) = main {
        require(
            extent(ram, table_ram as usize, table_bytes)? == extent(inverted, flash as usize + bias, table_bytes)?,
            "The RAM image's section table differs from the flash's",
        )?;
    }
    Ok(Plan {
        patch_offset: site,
        patch_old: word(reset, site)?,
        dst_register: 20,
        dst,
        bytes,
        flash_offset,
        tables: [tables[0], tables[1]],
        table_bytes,
    })
}

/// `orr x<rd>, x<rn>, #mask` for a mask that is one run of ones (64-bit element, N = 1).
pub(super) fn orr_immediate(rd: u32, rn: u32, mask: u64) -> Option<u32> {
    let start = mask.trailing_zeros();
    if mask == 0 || mask == u64::MAX || !((mask >> start) + 1).is_power_of_two() {
        return None;
    }
    let ones = mask.count_ones();
    let immr = (64 - start) % 64;
    Some(0xb240_0000 | (immr << 16) | ((ones - 1) << 10) | (rn << 5) | rd)
}

/// The binding words the loader's `setup_sd_redirect` takes (its first sixteen words; the last
/// four are the loader's outputs).
pub(super) struct Binding(pub [u64; 16]);

/// Plan the loader's binding: the changed word for a DRAM copy at `destination`, the stock and
/// patched CRC-32 of the guard's extent, and the CRC-32s of everything the loader compares
/// against the flash. `aperture` is the flash aperture address, `reset` the stock reset image
/// (its first `guard_bytes`), `file_bytes` the length of the file the loader will read.
pub(super) fn binding(
    plan: &Plan, reset: &[u8], inverted: &[u8], bias: usize, aperture: u64, destination: u64,
    guard_bytes: usize, file_bytes: usize, apply: bool,
) -> Result<Binding> {
    // The one word: the copy's source is the section's own address ORed into the destination, so
    // the destination's bits must be clear in the section's address.
    require(
        destination % 0x1000 == 0
            && (READ_ONLY_FLOOR..READ_ONLY_CEILING).contains(&destination)
            && plan.dst & destination == 0
            && plan.dst + plan.bytes as u64 <= file_bytes as u64
            && destination + file_bytes as u64 <= READ_ONLY_CEILING,
        "The redirect's source does not fit the read-only destination and the file",
    )?;
    let new = orr_immediate(0, plan.dst_register, destination).ok_or_else(|| {
        image::Error::Invalid("The read-only destination is not a single run of ones".into())
    })?;
    require(new != plan.patch_old, "The redirect word is the stock word")?;
    let stock = extent(reset, 0, guard_bytes)?;
    require(plan.patch_offset + 4 <= guard_bytes, "The changed word is outside the RAM-0 guard")?;
    let mut patched = stock.to_vec();
    patched[plan.patch_offset..plan.patch_offset + 4].copy_from_slice(&new.to_le_bytes());
    let section = extent(inverted, plan.flash_offset as usize + bias, plan.bytes)?;
    let mut words = [0u64; 16];
    words[0] = 1;
    words[1] = u64::from(apply);
    words[2] = plan.patch_offset as u64;
    words[3] = u64::from(plan.patch_old);
    words[4] = u64::from(new);
    words[5] = u64::from(crc32(stock));
    words[6] = u64::from(crc32(&patched));
    words[7] = plan.dst;
    words[8] = plan.bytes as u64;
    words[9] = aperture + plan.flash_offset;
    words[10] = u64::from(crc32(section));
    words[11] = plan.table_bytes as u64;
    for (n, &(_, flash)) in plan.tables.iter().enumerate() {
        let table = extent(inverted, flash as usize + bias, plan.table_bytes)?;
        words[12 + n] = aperture + flash;
        words[14 + n] = u64::from(crc32(table));
    }
    Ok(Binding(words))
}

/// Write the binding into the loader at `at` after checking that the template is untouched.
pub(super) fn bind(program: &mut [u8], at: usize, binding: &Binding) -> Result<()> {
    require(
        extent(program, at, 160)? == [1u64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
            .map(u64::to_le_bytes)
            .concat(),
        "XP5 checked-redirect binding template changed",
    )?;
    for (n, value) in binding.0.iter().enumerate() {
        put64(program, at + 8 * n, *value)?;
    }
    Ok(())
}

#[cfg(test)]
pub(in crate::xp5) mod tests {
    use super::*;
    use crate::image::put;

    fn bl(at: usize, target: usize) -> u32 {
        0x9400_0000 | ((((target as i64 - at as i64) / 4) as u32) & 0x03ff_ffff)
    }
    fn adrp(at: usize, rd: u32, target: u64) -> u32 {
        let pages = (target as i64 >> 12) - ((at as i64) >> 12);
        0x9000_0000 | (((pages as u32) & 3) << 29) | ((((pages as u32) >> 2) & 0x7ffff) << 5) | rd
    }

    const FUNCTION: usize = 0x2cd0;
    const TRANSLATOR: usize = 0xa7d8;
    const COPY: usize = 0xa090;
    const MAIN_COPY: usize = 0x27b4;
    const ALT_COPY: usize = 0x27e8;
    const BIAS: usize = 0x2bc;
    pub(in crate::xp5) const E2E_SECTION_BYTES: usize = SECTION_BYTES;
    const SECTION_FLASH: usize = 0xc_0000;
    const SECTION_DST: u64 = 0x8_1000;
    const SECTION_BYTES: usize = 0x2000;

    fn put_pattern(image: &mut [u8], at: usize, pattern: &[Pat]) {
        for (n, &(value, _)) in pattern.iter().enumerate() {
            put(image, at + 4 * n, value).unwrap();
        }
    }
    fn table_copy(image: &mut [u8], at: usize, ram: u64, flash: u64) {
        put_pattern(image, at, &TABLE_COPY);
        put(image, at, adrp(at, 19, ram)).unwrap();
        put(image, at + 4, 0x9100_0273 | (((ram & 0xfff) as u32) << 10)).unwrap();
        put(image, at + 12, bl(at + 12, 0x2ef0)).unwrap();
        put(image, at + 16, adrp(at + 16, 0, flash)).unwrap();
        put(image, at + 20, 0x9100_0000 | (((flash & 0xfff) as u32) << 10)).unwrap();
        put(image, at + 24, bl(at + 24, TRANSLATOR)).unwrap();
        put(image, at + 32, 0x5281_0002).unwrap();
        put(image, at + 44, bl(at + 44, COPY)).unwrap();
    }
    fn table(ram: u64, id0: [u32; 3]) -> Vec<u8> {
        let mut table = vec![0u8; 0x800];
        table[..8].copy_from_slice(&(ram + 0x50).to_le_bytes());
        let mut row = |n: usize, id: u32, dst: u64, flash: u32, size: u32| {
            let at = 0x50 + n * ROW;
            table[at..at + 4].copy_from_slice(&id.to_le_bytes());
            table[at + 8..at + 16].copy_from_slice(&dst.to_le_bytes());
            table[at + 40..at + 44].copy_from_slice(&flash.to_le_bytes());
            table[at + 48..at + 52].copy_from_slice(&size.to_le_bytes());
        };
        row(0, 0, u64::from(id0[0]), id0[1], id0[2]);
        row(1, 3, 0x68_1000, 0x2b6_0000, 0x3c_0000);
        row(2, END, 0, 0, 0);
        table
    }
    /// A synthetic stock reset image, update and RAM image around the real function's words.
    fn fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        stage(BIAS)
    }
    /// The stage, the update (flash offset `n` at `n + bias`) and the RAM image around it.
    pub(in crate::xp5) fn stage(bias: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut reset = vec![0u8; 0xb000];
        put_pattern(&mut reset, FUNCTION, &SECTION_LOAD);
        put(&mut reset, FUNCTION + 16, bl(FUNCTION + 16, 0x2f0c)).unwrap();
        put(&mut reset, FUNCTION + 32, bl(FUNCTION + 32, TRANSLATOR)).unwrap();
        put(&mut reset, FUNCTION + 52, bl(FUNCTION + 52, COPY)).unwrap();
        put_pattern(&mut reset, TRANSLATOR, &FLASH_ADDRESS);
        put(&mut reset, TRANSLATOR, adrp(TRANSLATOR, 1, 0x13000)).unwrap();
        // The single caller.
        put(&mut reset, 0x285c, bl(0x285c, FUNCTION)).unwrap();
        table_copy(&mut reset, MAIN_COPY, 0x4_0000, 0x64_0000);
        table_copy(&mut reset, ALT_COPY, 0x4_0800, 0x12_0000);
        let mut update = vec![0u8; 0x70_0000 + bias];
        let main = table(0x4_0000, [SECTION_DST as u32, SECTION_FLASH as u32, SECTION_BYTES as u32]);
        let alt = table(0x4_0800, [SECTION_DST as u32, SECTION_FLASH as u32, SECTION_BYTES as u32]);
        update[0x64_0000 + bias..0x64_0800 + bias].copy_from_slice(&main);
        update[0x12_0000 + bias..0x12_0800 + bias].copy_from_slice(&alt);
        for n in 0..SECTION_BYTES {
            update[SECTION_FLASH + bias + n] = (n * 7 + 1) as u8;
        }
        let mut ram = vec![0u8; 0x8_1000 + SECTION_BYTES];
        ram[0x4_0000..0x4_0800].copy_from_slice(&main);
        ram[0x8_1000..].copy_from_slice(&update[SECTION_FLASH + bias..SECTION_FLASH + bias + SECTION_BYTES]);
        (reset, update, ram)
    }

    #[test]
    fn orr_immediate_matches_the_encodings_the_stage_itself_uses() {
        // `orr x1, x0, #0xf0000000` is 0xb2640c01 in the stage; ours are the same family.
        assert_eq!(orr_immediate(1, 0, 0xf000_0000), Some(0xb264_0c01));
        assert_eq!(orr_immediate(0, 20, 0x3000_0000), Some(0xb264_0680));
        assert_eq!(orr_immediate(0, 20, 0x3000_0001), None);
        assert_eq!(orr_immediate(0, 20, 0), None);
        assert_eq!(orr_immediate(0, 20, u64::MAX), None);
    }

    #[test]
    fn the_redirect_is_found_in_a_stage_that_has_it_once() {
        let (reset, update, ram) = fixture();
        let plan = locate(&reset, &update, BIAS, &ram).unwrap();
        assert_eq!(plan.patch_offset, FUNCTION + 32);
        assert_eq!(plan.patch_old, bl(FUNCTION + 32, TRANSLATOR));
        assert_eq!((plan.dst, plan.flash_offset, plan.bytes), (SECTION_DST, SECTION_FLASH as u64, SECTION_BYTES));
        assert_eq!(plan.tables, [(0x4_0000, 0x64_0000), (0x4_0800, 0x12_0000)]);
    }

    #[test]
    fn a_stage_that_differs_is_refused() {
        let (reset, update, ram) = fixture();
        // Any changed word of the function, the translator or a table copy.
        for at in [FUNCTION, FUNCTION + 4 * 6, FUNCTION + 4 * 10, TRANSLATOR + 4, MAIN_COPY + 4, ALT_COPY + 4 * 8] {
            let mut changed = reset.clone();
            let w = word(&changed, at).unwrap();
            put(&mut changed, at, w ^ 0x20).unwrap();
            assert!(locate(&changed, &update, BIAS, &ram).is_err(), "{at:#x}");
        }
        // A second caller, or none.
        let mut two = reset.clone();
        put(&mut two, 0x3000, bl(0x3000, FUNCTION)).unwrap();
        assert!(locate(&two, &update, BIAS, &ram).is_err());
        let mut none = reset.clone();
        put(&mut none, 0x285c, 0xd503_201f).unwrap();
        assert!(locate(&none, &update, BIAS, &ram).is_err());
        // The function twice.
        let mut twice = reset.clone();
        put_pattern(&mut twice, 0x3400, &SECTION_LOAD);
        assert!(locate(&twice, &update, BIAS, &ram).is_err());
    }

    #[test]
    fn the_cards_section_and_both_tables_have_to_equal_the_flash() {
        let (reset, update, ram) = fixture();
        let mut bad = ram.clone();
        bad[0x8_1000 + 17] ^= 1;
        assert!(locate(&reset, &update, BIAS, &bad).is_err());
        let mut bad = ram.clone();
        bad[0x4_0000 + 0x58] ^= 1;
        assert!(locate(&reset, &update, BIAS, &bad).is_err());
        // The alternate table names another flash offset for section 0.
        let mut other = update.clone();
        other[0x12_0000 + BIAS + 0x50 + 40] ^= 0x10;
        assert!(locate(&reset, &other, BIAS, &ram).is_err());
        // A compressed section 0.
        let mut compressed = update.clone();
        compressed[0x64_0000 + BIAS + 0x50 + 56] = 1;
        assert!(locate(&reset, &compressed, BIAS, &ram).is_err());
    }

    #[test]
    fn the_binding_carries_the_one_word_the_crcs_and_the_flash_addresses() {
        let (reset, update, ram) = fixture();
        let plan = locate(&reset, &update, BIAS, &ram).unwrap();
        let file = 0x8_1000 + SECTION_BYTES;
        let b = binding(&plan, &reset, &update, BIAS, 0xf000_0000, 0x3000_0000, 0x4000, file, true).unwrap();
        let w = b.0;
        assert_eq!((w[0], w[1], w[2], w[3]), (1, 1, plan.patch_offset as u64, u64::from(plan.patch_old)));
        assert_eq!(w[4], 0xb264_0680);
        assert_eq!((w[7], w[8], w[9]), (SECTION_DST, SECTION_BYTES as u64, 0xf000_0000 + SECTION_FLASH as u64));
        assert_eq!(w[10], u64::from(crc32(&ram[0x8_1000..])));
        assert_eq!((w[11], w[12], w[13]), (0x800, 0xf064_0000, 0xf012_0000));
        let mut patched = reset[..0x4000].to_vec();
        patched[plan.patch_offset..plan.patch_offset + 4].copy_from_slice(&0xb264_0680u32.to_le_bytes());
        assert_eq!((w[5], w[6]), (u64::from(crc32(&reset[..0x4000])), u64::from(crc32(&patched))));
        assert_ne!(w[5], w[6]);
        // Verify-only is the same binding with apply 0.
        let v = binding(&plan, &reset, &update, BIAS, 0xf000_0000, 0x3000_0000, 0x4000, file, false).unwrap();
        assert_eq!((v.0[1], &v.0[2..]), (0, &w[2..]));
    }

    #[test]
    fn the_binding_refuses_a_destination_that_does_not_fit() {
        let (reset, update, ram) = fixture();
        let plan = locate(&reset, &update, BIAS, &ram).unwrap();
        let file = 0x8_1000 + SECTION_BYTES;
        let make = |destination: u64, guard: usize, file: usize| {
            binding(&plan, &reset, &update, BIAS, 0xf000_0000, destination, guard, file, true)
        };
        assert!(make(0x3000_0000, 0x4000, file).is_ok());
        // Outside the window, unaligned, not a run of ones, overlapping the section's address bits.
        for bad in [0x2000_0000, 0x3000_0004, 0x3000_0000 + 0x1000_0000, 0x3000_1000, 0x4000_0000] {
            assert!(make(bad, 0x4000, file).is_err(), "{bad:#x}");
        }
        assert!(make(0x3000_0000, 0x4000, file - 4).is_err(), "the file ends inside the section");
        assert!(make(0x3000_0000, FUNCTION + 32, file).is_err(), "the word is outside the guard");
    }
}
