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
/// Word 13 of the function is the copy call (`bl copy`).
const COPY_WORD: usize = 13;
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
    /// RAM-0 byte offset of the copy call that follows (`bl copy`, the site of the copy stub), its
    /// stock word and the RAM-0 address of the stage's copy function. The two table copies call the
    /// same function.
    pub copy_site: usize,
    pub copy_old: u32,
    pub copy_fn: usize,
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
    let at = row0_at(table, table_ram)?;
    let dst = u64::from_le_bytes(table[at + 8..at + 16].try_into().unwrap());
    let flash = u64::from(word(table, at + 40)?);
    let size = word(table, at + 48)? as usize;
    require(word(table, at + 56)? == 0, "Section 0 is not stored raw")?;
    Ok((dst, flash, size))
}

/// The byte offset in `table` of the row for id 0.
fn row0_at(table: &[u8], table_ram: u64) -> Result<usize> {
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
    found.ok_or_else(|| image::Error::Invalid("A section table has no row for section 0".into()))
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
    let copy_site = function + 4 * COPY_WORD;
    let copy_fn = bl_target(reset, copy_site)?;
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
                && word(reset, at + 4 * 8)? == 0x5281_0002
                && bl_target(reset, at + 4 * 11)? == copy_fn,
            "A section table copy differs (translator, 0x800 length or copy function)",
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
        copy_site,
        copy_old: word(reset, copy_site)?,
        copy_fn,
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

// ---- The copy stub ----
//
// Instead of the one-word source change above, the copy call itself (`bl copy`) is replaced by a
// `bl` to a stub that the loader writes into dead padding of the stock reset image: the stub
// checks the DRAM copy at the moment of use, copies from it with the stage's own copy function,
// checks the destination, and copies from the flash instead on any difference. The padding is the
// run of `nop` words between the last helper of the reset entry (ending in `ret`) and the first
// exception vector table (the next 0x800 boundary): nothing falls into it, and the packager
// refuses the image unless nothing in it refers to the run either.

const RET: u32 = 0xd65f_03c0;
const NOP: u32 = 0xd503_201f;
/// The stub starts on this boundary inside the padding, and at least `SLED_MARGIN` bytes of padding
/// stay nops on each side of the written bytes.
const SLED_ALIGN: usize = 0x40;
const SLED_MARGIN: usize = 0x40;
/// A vector table sits on this boundary; the padding ends at one.
const VECTOR_ALIGN: usize = 0x800;
/// The loader's room for the stub bytes (`COPY_STUB_MAX` in `copy_stub.h`).
pub(super) const STUB_MAX: usize = 1024;
/// The hook build's room for the stub bytes (`COPY_STUB_MAX` under `XP5_HOOK`): its table has sixteen entries.
pub(super) const HOOK_STUB_MAX: usize = 1280;
/// Loop counts of one lamp pulse and one pause (two instructions per count): about a quarter of a
/// second at the speed of the loader's own code, longer where the stage's code runs slower.
pub(super) const STUB_PULSE_LOOPS: u64 = 8_000_000;
pub(super) const STUB_GAP_LOOPS: u64 = 4_000_000;

/// Every word of `image` read as an instruction or as data whose direct target or value lies in
/// `[lo, hi)`: branches, conditional and compare branches, test branches, `adr`, literal loads,
/// `adrp` pages that overlap the range, and 64-bit aligned data values. (32-bit data words are not
/// pointers in this stage: its address constants are 64-bit.) An over-approximation: a data word that
/// merely looks like a branch is reported too.
pub(super) fn references_into(image: &[u8], lo: usize, hi: usize) -> Vec<(usize, &'static str)> {
    references_into_at(image, 0, lo, hi, false)
}

/// As `references_into` for an image loaded at address `base` (`lo` and `hi` are addresses; the
/// reported offsets are offsets in the image). With `data32`, aligned 32-bit data words inside the range
/// are reported too.
pub(super) fn references_into_at(
    image: &[u8], base: usize, lo: usize, hi: usize, data32: bool,
) -> Vec<(usize, &'static str)> {
    let sx = |value: u32, bits: u32| (i64::from(value) << (64 - bits)) >> (64 - bits);
    let mut hits = Vec::new();
    for at in (0..image.len().saturating_sub(3)).step_by(4) {
        let w = word(image, at).unwrap_or(0);
        let direct = |offset: i64| usize::try_from((base + at) as i64 + offset).ok().filter(|t| (lo..hi).contains(t));
        let hit = if w & 0x7c00_0000 == 0x1400_0000 {
            direct(4 * sx(w & 0x03ff_ffff, 26)).map(|_| "branch")
        } else if w & 0xff00_0010 == 0x5400_0000 {
            direct(4 * sx((w >> 5) & 0x7_ffff, 19)).map(|_| "conditional branch")
        } else if w & 0x7e00_0000 == 0x3400_0000 {
            direct(4 * sx((w >> 5) & 0x7_ffff, 19)).map(|_| "compare and branch")
        } else if w & 0x7e00_0000 == 0x3600_0000 {
            direct(4 * sx((w >> 5) & 0x3fff, 14)).map(|_| "test and branch")
        } else if w & 0x9f00_0000 == 0x1000_0000 {
            let imm = (((w >> 5) & 0x7_ffff) << 2) | ((w >> 29) & 3);
            direct(sx(imm, 21)).map(|_| "adr")
        } else if w & 0x3b00_0000 == 0x1800_0000 {
            direct(4 * sx((w >> 5) & 0x7_ffff, 19)).map(|_| "literal load")
        } else if w & ADRP_MASK == ADRP {
            let imm = (((w >> 5) & 0x7_ffff) << 2) | ((w >> 29) & 3);
            let page = (((base + at) & !0xfff) as i64) + (sx(imm, 21) << 12);
            (page < hi as i64 && page + 0x1000 > lo as i64).then_some("adrp page")
        } else {
            None
        };
        if let Some(kind) = hit {
            hits.push((at, kind));
        }
    }
    for at in (0..image.len().saturating_sub(7)).step_by(8) {
        let value = u64::from_le_bytes(image[at..at + 8].try_into().unwrap());
        if (lo as u64..hi as u64).contains(&value) {
            hits.push((at, "64-bit data value"));
        }
    }
    if data32 {
        for at in (0..image.len().saturating_sub(3)).step_by(4) {
            let value = word(image, at).unwrap_or(0) as usize;
            if (lo..hi).contains(&value) {
                hits.push((at, "32-bit data value"));
            }
        }
    }
    hits
}

/// The RAM-0 byte offset for a stub of `bytes` bytes: inside the one run of `nop` words that starts
/// directly after a `ret` and ends on a vector-table boundary, on a `SLED_ALIGN` boundary, with
/// `SLED_MARGIN` bytes of nops left after it, and only if nothing in the image refers to the run.
pub(super) fn sled(reset: &[u8], bytes: usize) -> Result<usize> {
    sled_within(reset, bytes, STUB_MAX)
}

/// `sled` for a loader build whose room for the stub is `max` bytes.
pub(super) fn sled_within(reset: &[u8], bytes: usize, max: usize) -> Result<usize> {
    require(bytes != 0 && bytes % 4 == 0 && bytes <= max, "The copy stub's size is not usable")?;
    let words = reset.len() / 4;
    let mut runs = Vec::new();
    let mut n = 0;
    while n < words {
        if word(reset, 4 * n)? == NOP {
            let start = n;
            while n < words && word(reset, 4 * n)? == NOP {
                n += 1;
            }
            if start > 0 && word(reset, 4 * start - 4)? == RET && (4 * n) % VECTOR_ALIGN == 0 {
                runs.push((4 * start, 4 * n));
            }
        } else {
            n += 1;
        }
    }
    require(
        runs.len() == 1,
        "The reset stage's padding before its vector tables is not found exactly once",
    )?;
    let (start, end) = runs[0];
    let at = (start + SLED_ALIGN - 1) & !(SLED_ALIGN - 1);
    require(
        at >= start + 4 && at + bytes + SLED_MARGIN <= end,
        "The copy stub does not fit the reset stage's padding",
    )?;
    let hits = references_into(reset, start, end);
    require(
        hits.is_empty(),
        &format!("The reset stage refers to its padding at {:#x}", hits.first().map_or(0, |h| h.0)),
    )?;
    Ok(at)
}

/// `bl target` placed at `at`.
pub(super) fn bl_word(at: usize, target: usize) -> Result<u32> {
    let delta = target as i64 - at as i64;
    require(
        delta % 4 == 0 && (-(1i64 << 27)..(1i64 << 27)).contains(&delta),
        "The copy stub is out of range of the copy call",
    )?;
    Ok(0x9400_0000 | (((delta / 4) as u32) & 0x03ff_ffff))
}

/// The values of the stub's parameter block (the stub's own `copy_stub.S` documents each).
pub(super) struct StubValues {
    pub own_address: u64,
    pub dst: u64,
    pub flash: u64,
    pub bytes: u64,
    pub dram: u64,
    pub crc: u64,
    pub copy: u64,
    pub green: u64,
    pub red: u64,
    pub pin: u64,
    pub pulse: u64,
    pub gap: u64,
    pub enable: u64,
    pub enable_green: u64,
    pub enable_red: u64,
    pub mux: u64,
    pub ret: u64,
    pub restore: u64,
}

/// The stub image with its parameter block filled in. The template's block must be all zero and the
/// parameters must be where the layout says.
pub(super) fn fill_stub(template: &[u8], values: &StubValues) -> Result<Vec<u8>> {
    use super::stub_template as t;
    let params = t::XP5_STUB_PARAMS;
    require(
        template.len() == t::XP5_STUB_END
            && params + 176 == template.len()
            && template[params..].iter().all(|b| *b == 0),
        "The copy stub template changed",
    )?;
    let mut image = template.to_vec();
    for (offset, value) in [
        (t::XP5_STUB_P_SELF, values.own_address),
        (t::XP5_STUB_P_DST, values.dst),
        (t::XP5_STUB_P_FLASH, values.flash),
        (t::XP5_STUB_P_BYTES, values.bytes),
        (t::XP5_STUB_P_DRAM, values.dram),
        (t::XP5_STUB_P_CRC, values.crc),
        (t::XP5_STUB_P_COPY, values.copy),
        (t::XP5_STUB_P_GREEN, values.green),
        (t::XP5_STUB_P_RED, values.red),
        (t::XP5_STUB_P_PIN, values.pin),
        (t::XP5_STUB_P_PULSE, values.pulse),
        (t::XP5_STUB_P_GAP, values.gap),
        (t::XP5_STUB_P_ENABLE, values.enable),
        (t::XP5_STUB_P_ENABLE_GREEN, values.enable_green),
        (t::XP5_STUB_P_ENABLE_RED, values.enable_red),
        (t::XP5_STUB_P_MUX, values.mux),
        (t::XP5_STUB_P_RETURN, values.ret),
        (t::XP5_STUB_P_RESTORE, values.restore),
    ] {
        require(offset >= params && offset + 8 <= image.len(), "A copy stub parameter is outside its block")?;
        put64(&mut image, offset, value)?;
    }
    Ok(image)
}

/// The two bindings of a copy-stub loader: the redirect words (the copy call's stock and new word,
/// the CRC-32s of the stock and the patched RAM-0 image, the section, its flash address and CRC-32,
/// the two section tables) and the stub (where it goes and its bytes).
pub(super) struct StubBinding {
    pub redirect: Binding,
    pub offset: usize,
    pub params_offset: usize,
    pub image: Vec<u8>,
    pub crc32: u32,
}

/// The lamp the stub drives, as the prelude's `prepare_lamp()` and `show()` do: the PIO base, the two
/// ports and the pin (the registers are `pio + 64 * port`, the port-enable register `pio + 0x500`).
pub(super) struct StubLamp {
    pub pio_base: u64,
    pub green_port: u32,
    pub red_port: u32,
    pub pin: u32,
}

/// Plan the copy-stub loader's bindings. `destination` is where the read-only startup streams the
/// file (the DRAM copy of the section is at `destination + plan.dst`), `reset` the stock reset image,
/// `template` the stub image from its asset.
pub(super) fn stub_binding(
    plan: &Plan, reset: &[u8], inverted: &[u8], bias: usize, aperture: u64, destination: u64,
    guard_bytes: usize, file_bytes: usize, apply: bool, lamp: &StubLamp, template: &[u8],
) -> Result<StubBinding> {
    stub_binding_with(plan, reset, inverted, bias, aperture, destination, guard_bytes, file_bytes, apply, lamp, template, None)
}

/// `stub_binding`, with the CRC-32 that the stub compares the DRAM copy and the destination with given
/// (`stub_crc`: the CRC-32 of the section as the DRAM copy will hold it), instead of the flash range's.
/// The binding's own flash-range CRC-32 (what the loader measures the flash against) stays the flash's.
pub(super) fn stub_binding_with(
    plan: &Plan, reset: &[u8], inverted: &[u8], bias: usize, aperture: u64, destination: u64,
    guard_bytes: usize, file_bytes: usize, apply: bool, lamp: &StubLamp, template: &[u8],
    stub_crc: Option<u32>,
) -> Result<StubBinding> {
    require(
        destination % 0x1000 == 0
            && (READ_ONLY_FLOOR..READ_ONLY_CEILING).contains(&destination)
            && plan.dst + plan.bytes as u64 <= file_bytes as u64
            && destination + file_bytes as u64 <= READ_ONLY_CEILING,
        "The copy stub's source does not fit the read-only destination and the file",
    )?;
    require(
        plan.copy_old & BL.1 == BL.0 && plan.copy_site % 4 == 0 && plan.copy_site + 4 <= guard_bytes
            && plan.copy_fn % 4 == 0 && plan.copy_fn + 4 <= guard_bytes,
        "The reset stage's copy call is not a bl inside the RAM-0 guard",
    )?;
    let offset = sled(extent(reset, 0, guard_bytes)?, template.len())?;
    require(
        offset + template.len() + SLED_MARGIN <= guard_bytes
            && !(offset..offset + template.len()).contains(&plan.copy_site),
        "The copy stub overlaps the copy call or leaves the RAM-0 guard",
    )?;
    let stock = extent(reset, 0, guard_bytes)?;
    require(
        lamp.pin < 8 && lamp.green_port < 16 && lamp.red_port < 16 && lamp.green_port != lamp.red_port
            && lamp.green_port != 3 && lamp.red_port != 3,
        "The copy stub's lamp is not a usable pair of ports",
    )?;
    let section = extent(inverted, plan.flash_offset as usize + bias, plan.bytes)?;
    let crc = crc32(section);
    let image = fill_stub(
        template,
        &StubValues {
            own_address: offset as u64,
            dst: plan.dst,
            flash: aperture + plan.flash_offset,
            bytes: plan.bytes as u64,
            dram: destination + plan.dst,
            crc: u64::from(stub_crc.unwrap_or(crc)),
            copy: plan.copy_fn as u64,
            green: lamp.pio_base + 64 * u64::from(lamp.green_port),
            red: lamp.pio_base + 64 * u64::from(lamp.red_port),
            pin: 1u64 << lamp.pin,
            pulse: STUB_PULSE_LOOPS,
            gap: STUB_GAP_LOOPS,
            enable: lamp.pio_base + 0x500,
            enable_green: (1u64 << (lamp.green_port + 16)) | (1u64 << lamp.green_port),
            enable_red: (1u64 << (lamp.red_port + 16)) | (1u64 << lamp.red_port),
            mux: 3u64 << (2 * lamp.pin + 16),
            ret: plan.copy_site as u64 + 4,
            restore: (1u64 << (lamp.pin + 16)) | (1u64 << lamp.pin),
        },
    )?;
    let new = bl_word(plan.copy_site, offset)?;
    require(new != plan.copy_old, "The copy call word is the stock word")?;
    let mut patched = stock.to_vec();
    patched[offset..offset + image.len()].copy_from_slice(&image);
    patched[plan.copy_site..plan.copy_site + 4].copy_from_slice(&new.to_le_bytes());
    let mut words = [0u64; 16];
    words[0] = 1;
    words[1] = u64::from(apply);
    words[2] = plan.copy_site as u64;
    words[3] = u64::from(plan.copy_old);
    words[4] = u64::from(new);
    words[5] = u64::from(crc32(stock));
    words[6] = u64::from(crc32(&patched));
    words[7] = plan.dst;
    words[8] = plan.bytes as u64;
    words[9] = aperture + plan.flash_offset;
    words[10] = u64::from(crc);
    words[11] = plan.table_bytes as u64;
    for (n, &(_, flash)) in plan.tables.iter().enumerate() {
        let table = extent(inverted, flash as usize + bias, plan.table_bytes)?;
        words[12 + n] = aperture + flash;
        words[14 + n] = u64::from(crc32(table));
    }
    Ok(StubBinding {
        redirect: Binding(words),
        offset,
        params_offset: super::stub_template::XP5_STUB_PARAMS,
        crc32: crc32(&image),
        image,
    })
}

/// Write the stub binding into the loader at `at` after checking that the template is untouched
/// (`abi` 1, everything else zero: the struct is 48 + `STUB_MAX` + 32 bytes).
pub(super) fn bind_stub(program: &mut [u8], at: usize, stub: &StubBinding) -> Result<()> {
    bind_stub_within(program, at, stub, STUB_MAX)
}

/// `bind_stub` for a loader build whose room for the stub is `max` bytes.
pub(super) fn bind_stub_within(program: &mut [u8], at: usize, stub: &StubBinding, max: usize) -> Result<()> {
    let size = 48 + max + 32;
    let mut template = vec![0u8; size];
    template[..8].copy_from_slice(&1u64.to_le_bytes());
    require(extent(program, at, size)? == template, "XP5 copy-stub binding template changed")?;
    require(
        stub.image.len() % 4 == 0 && (16..=max).contains(&stub.image.len()),
        "The copy stub's size is not usable",
    )?;
    put64(program, at + 8, stub.offset as u64)?;
    put64(program, at + 16, stub.image.len() as u64)?;
    put64(program, at + 24, u64::from(NOP))?;
    put64(program, at + 32, u64::from(stub.crc32))?;
    put64(program, at + 40, stub.params_offset as u64)?;
    program[at + 48..at + 48 + stub.image.len()].copy_from_slice(&stub.image);
    Ok(())
}

// ---- The marker ----
//
// A change to the section itself. The loader (and the copy stub after it) still proves that the
// section the stage copies is the section the packager measured; what changes is that this section is
// no longer the flash's own: a small named set of words differs, namely the first instruction of the
// section's entry point (replaced by a `b` to a program) and the program itself, written into the
// run of zero words that the linker left in front of the section's first exception vector table. The
// program (`marker.S`) lights the lamp and returns to the instruction after the hook, so the
// section's own flow is unchanged. Nothing is changed in the zero tail of the section (the part
// past its declared size), which the stage's second core fills with zeros while the section is
// copied.

/// The most words the loader's binding can name (`MARKER_MAX_WORDS` in `marker.h`).
pub(super) const MARKER_MAX: usize = 96;
/// The program's parameter block (`MARKER_PARAMS` in `marker.h`).
pub(super) const MARKER_PARAMS: usize = 72;
/// The loader's binding: abi, CRC-32, code bytes, count, parameter offset, the words, four outputs.
pub(super) const MARKER_BINDING: usize = 40 + 12 * MARKER_MAX + 32;
/// The run of zero words that takes the program is at least this long (words), ends on a vector
/// table boundary, and is the only such run in the section's code and data.
const CAVE_RUN_MIN: usize = 512;
const CAVE_ALIGN: usize = 16;
/// At least this many bytes of the zero run stay zero after the program.
const CAVE_MARGIN: usize = 0x40;
/// Loop counts of one phase of the lamp: three phases of about a third of a second each on the
/// stage's code speed on a camera (about 4.4 M loops per second).
pub(super) const MARKER_PHASE_LOOPS: u64 = 1_600_000;

/// What the marker program's parameter block says (the program's `marker.S` documents each word).
pub(super) struct MarkerValues {
    pub green: u64,
    pub red: u64,
    pub pin: u64,
    pub enable: u64,
    pub enable_green: u64,
    pub enable_red: u64,
    pub mux: u64,
    pub restore: u64,
    pub phase: u64,
}

impl MarkerValues {
    /// The values for the prelude's lamp (the same as the copy stub's).
    pub(super) fn for_lamp(lamp: &StubLamp) -> Self {
        Self {
            green: lamp.pio_base + 64 * u64::from(lamp.green_port),
            red: lamp.pio_base + 64 * u64::from(lamp.red_port),
            pin: 1u64 << lamp.pin,
            enable: lamp.pio_base + 0x500,
            enable_green: (1u64 << (lamp.green_port + 16)) | (1u64 << lamp.green_port),
            enable_red: (1u64 << (lamp.red_port + 16)) | (1u64 << lamp.red_port),
            mux: 3u64 << (2 * lamp.pin + 16),
            restore: (1u64 << (lamp.pin + 16)) | (1u64 << lamp.pin),
            phase: MARKER_PHASE_LOOPS,
        }
    }
}

/// The changed section: every named word as (offset in the section, stock word, new word), ascending,
/// and the CRC-32s of the stock and the changed section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MarkerPlan {
    /// Offsets in the section of the hooked word and of the program.
    pub hook: usize,
    pub cave: usize,
    /// The program's parameter block, as an offset in the section.
    pub params_offset: usize,
    /// The section's declared size: every changed word lies below it.
    pub code_bytes: usize,
    pub words: Vec<(u32, u32, u32)>,
    pub stock_crc32: u32,
    pub modified_crc32: u32,
}

/// `b target` placed at `at`.
pub(super) fn b_word(at: usize, target: usize) -> Result<u32> {
    Ok(bl_word(at, target)? ^ 0x8000_0000)
}

/// The declared size of section 0 in the row of both section tables (which have to agree), and the
/// address that follows it: the start of the section's zero-filled part.
pub(super) fn declared_size(plan: &Plan, inverted: &[u8], bias: usize) -> Result<usize> {
    let mut found = Vec::new();
    for &(table_ram, flash) in &plan.tables {
        let table = extent(inverted, flash as usize + bias, plan.table_bytes)?;
        let at = row0_at(table, table_ram)?;
        let declared = u64::from_le_bytes(table[at + 16..at + 24].try_into().unwrap());
        let bss = u64::from_le_bytes(table[at + 24..at + 32].try_into().unwrap());
        require(
            declared % 4 == 0 && declared != 0 && declared <= plan.bytes as u64 && bss == plan.dst + declared,
            "Section 0's declared size does not end where its zero-filled part begins",
        )?;
        found.push(declared as usize);
    }
    require(found[0] == found[1], "The two section tables declare section 0 differently")?;
    Ok(found[0])
}

/// The offset in the section of the one run of zero words that ends on a vector-table boundary and is
/// long enough to take `bytes` of program and leave `CAVE_MARGIN` zero bytes after it. Nothing in the
/// section may refer to the run (every branch, `adr`, literal load, `adrp` page and 32- or 64-bit data
/// value is checked, as for the copy stub's padding).
pub(super) fn cave(section: &[u8], base: usize, declared: usize, bytes: usize) -> Result<usize> {
    require(bytes != 0 && bytes % 4 == 0, "The marker's size is not usable")?;
    let words = declared / 4;
    let mut runs = Vec::new();
    let mut n = 0;
    while n < words {
        if word(section, 4 * n)? == 0 {
            let start = n;
            while n < words && word(section, 4 * n)? == 0 {
                n += 1;
            }
            if start > 0 && (base + 4 * n) % VECTOR_ALIGN == 0 && 4 * (n - start) >= CAVE_RUN_MIN {
                runs.push((4 * start, 4 * n));
            }
        } else {
            n += 1;
        }
    }
    require(
        runs.len() == 1,
        "The section's zero padding before its vector table is not found exactly once",
    )?;
    let (start, end) = runs[0];
    let at = (start + CAVE_ALIGN - 1) & !(CAVE_ALIGN - 1);
    require(
        at >= start + 4 && at + bytes + CAVE_MARGIN <= end,
        "The marker does not fit the section's zero padding",
    )?;
    let hits = references_into_at(extent(section, 0, declared)?, base, base + start, base + end, true);
    require(
        hits.is_empty(),
        &format!("The section refers to its zero padding at {:#x}", hits.first().map_or(0, |h| h.0)),
    )?;
    Ok(at)
}

/// Plan the changed section: the hook at the section's entry point, the program in the zero padding,
/// and the CRC-32s of the section before and after. `section` is the section as the flash holds it
/// (`plan.bytes` bytes), `template` the program from its asset.
pub(super) fn marker_plan(
    plan: &Plan, section: &[u8], declared: usize, template: &[u8], values: &MarkerValues,
) -> Result<MarkerPlan> {
    use super::marker_image as t;
    require(section.len() == plan.bytes && declared <= section.len(), "Section 0 is not the planned size")?;
    require(
        template.len() == t::XP5_MARKER_END
            && t::XP5_MARKER_PARAMS + MARKER_PARAMS == template.len()
            && template[t::XP5_MARKER_PARAMS..].iter().all(|b| *b == 0)
            && t::XP5_MARKER_RETURN + 4 <= t::XP5_MARKER_PARAMS
            && word(template, t::XP5_MARKER_RETURN)? == 0x1400_0000,
        "The marker template changed",
    )?;
    // The section's own header: its address, its declared size and its entry point.
    let header = |at: usize| Ok::<u64, image::Error>(u64::from_le_bytes(extent(section, at, 8)?.try_into().unwrap()));
    let entry = header(0x18)?;
    require(
        header(0x8)? == plan.dst && header(0x10)? == declared as u64,
        "Section 0's header does not name its own address and declared size",
    )?;
    let hook = entry
        .checked_sub(plan.dst)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n >= 0x20 && n % 4 == 0 && n + 4 <= declared)
        .ok_or_else(|| image::Error::Invalid("Section 0's entry point is not inside its code".into()))?;
    // The hook displaces one instruction, which the program executes first.
    require(
        word(section, hook)? == word(template, 0)?,
        "The section's entry instruction is not the one the marker displaces",
    )?;
    let cave = cave(section, plan.dst as usize, declared, template.len())?;
    require(
        extent(section, cave, template.len())?.iter().all(|b| *b == 0),
        "The marker's place in the section is not zero",
    )?;
    let mut program = template.to_vec();
    for (offset, value) in [
        (t::XP5_MARKER_P_GREEN, values.green),
        (t::XP5_MARKER_P_RED, values.red),
        (t::XP5_MARKER_P_PIN, values.pin),
        (t::XP5_MARKER_P_ENABLE, values.enable),
        (t::XP5_MARKER_P_ENABLE_GREEN, values.enable_green),
        (t::XP5_MARKER_P_ENABLE_RED, values.enable_red),
        (t::XP5_MARKER_P_MUX, values.mux),
        (t::XP5_MARKER_P_RESTORE, values.restore),
        (t::XP5_MARKER_P_PHASE, values.phase),
    ] {
        require(
            offset >= t::XP5_MARKER_PARAMS && offset + 8 <= program.len(),
            "A marker parameter is outside its block",
        )?;
        put64(&mut program, offset, value)?;
    }
    require(values.phase >= 1 && values.phase <= 1 << 28, "The marker's phase length is not usable")?;
    // The program returns to the instruction after the hook.
    image::put(&mut program, t::XP5_MARKER_RETURN, b_word(cave + t::XP5_MARKER_RETURN, hook + 4)?)?;
    let mut words = vec![(hook as u32, word(section, hook)?, b_word(hook, cave)?)];
    for at in (0..program.len()).step_by(4) {
        let new = word(&program, at)?;
        if new != 0 {
            words.push(((cave + at) as u32, word(section, cave + at)?, new));
        }
    }
    words.sort_unstable();
    require(
        words.len() <= MARKER_MAX
            && words.windows(2).all(|w| w[0].0 < w[1].0)
            && words.iter().all(|&(offset, stock, new)| offset as usize + 4 <= declared && stock != new),
        "The marker changes too many words, or one outside the section's code",
    )?;
    let mut modified = section.to_vec();
    for &(offset, _, new) in &words {
        modified[offset as usize..offset as usize + 4].copy_from_slice(&new.to_le_bytes());
    }
    // The part past the declared size is the stage's zero tail: not a byte of it changes.
    require(modified[declared..] == section[declared..], "The marker changed the zero tail")?;
    let stock_crc32 = crc32(section);
    let modified_crc32 = crc32(&modified);
    require(stock_crc32 != modified_crc32, "The changed section has the stock CRC-32")?;
    Ok(MarkerPlan {
        hook,
        cave,
        params_offset: cave + t::XP5_MARKER_PARAMS,
        code_bytes: declared,
        words,
        stock_crc32,
        modified_crc32,
    })
}

/// Write the marker binding into the loader at `at` after checking that the template is untouched
/// (`abi` 1, everything else zero).
pub(super) fn bind_marker(program: &mut [u8], at: usize, marker: &MarkerPlan) -> Result<()> {
    let mut template = vec![0u8; MARKER_BINDING];
    template[..8].copy_from_slice(&1u64.to_le_bytes());
    require(extent(program, at, MARKER_BINDING)? == template, "XP5 marker binding template changed")?;
    require(
        !marker.words.is_empty() && marker.words.len() <= MARKER_MAX,
        "The marker names no words or too many",
    )?;
    put64(program, at + 8, u64::from(marker.modified_crc32))?;
    put64(program, at + 16, marker.code_bytes as u64)?;
    put64(program, at + 24, marker.words.len() as u64)?;
    put64(program, at + 32, marker.params_offset as u64)?;
    for (n, &(offset, stock, new)) in marker.words.iter().enumerate() {
        let w = at + 40 + 12 * n;
        for (k, value) in [offset, stock, new].into_iter().enumerate() {
            program[w + 4 * k..w + 4 * k + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    Ok(())
}

// ---- The OS section ----
//
// The stage's second load, the section with id 6 (the OS), is stored compressed: the stage's load
// function reads the flash range in chunks into a bounce buffer and feeds a decoding engine that
// writes the decoded section to the section's DRAM address. Nothing about that load changes here.
// The call of that function (`mov w0, #6; bl load`) is replaced by a `bl` to a stub that the loader
// writes into dead padding of the stock reset image. The stub calls the load function (so the engine's
// decode of this unit's flash is in the destination as on a stock boot), compares the decode with the
// DRAM copy of the section that the card supplied, word for word, and only if they are equal writes
// the DRAM copy over the destination and compares again. The packager checks, from the update alone,
// that the stage is the one it expects, that both section tables name the same row for the section,
// and that the card's bytes are the decode of the flash's compressed bytes by the software decoder
// (the hardware's own decode is what the stub compares with, on the camera).

/// The OS section's load function: look the row up, raw path (not taken for this section) or the
/// compressed path through the decoding engine (start, a chunked feed, wait). Every word is checked;
/// branch and page fields are not (the packager reads the targets it needs from them).
#[rustfmt::skip]
const OS_LOAD: [Pat; 63] = [
    (0xa9bc7bfd, 0xffffffff), // 0x2d50 
    (0x910003fd, 0xffffffff), // 0x2d54 
    (0x94000000, 0xfc000000), // 0x2d58 BL
    (0xb4000000, 0xff00001f), // 0x2d5c cbz/cbnz
    (0xa90153f3, 0xffffffff), // 0x2d60 
    (0xaa0003f3, 0xffffffff), // 0x2d64 
    (0xb9403800, 0xffffffff), // 0x2d68 
    (0x35000000, 0xff00001f), // 0x2d6c cbz/cbnz
    (0xf9400674, 0xffffffff), // 0x2d70 
    (0xb9402a60, 0xffffffff), // 0x2d74 
    (0x94000000, 0xfc000000), // 0x2d78 BL
    (0xd2800003, 0xffffffff), // 0x2d7c 
    (0xb9403262, 0xffffffff), // 0x2d80 
    (0xaa0003e1, 0xffffffff), // 0x2d84 
    (0xaa1403e0, 0xffffffff), // 0x2d88 
    (0x94000000, 0xfc000000), // 0x2d8c BL
    (0xd2800000, 0xffffffff), // 0x2d90 
    (0xa94153f3, 0xffffffff), // 0x2d94 
    (0xa8c47bfd, 0xffffffff), // 0x2d98 
    (0xd65f03c0, 0xffffffff), // 0x2d9c 
    (0x14000000, 0xfc000000), // 0x2da0 B
    (0xa9025bf5, 0xffffffff), // 0x2da4 
    (0xf9401676, 0xffffffff), // 0x2da8 
    (0x90000000, 0x9f00001f), // 0x2dac adrp
    (0xf940a015, 0xffffffff), // 0x2db0 
    (0xb9403274, 0xffffffff), // 0x2db4 
    (0xaa1503e1, 0xffffffff), // 0x2db8 
    (0xf9400660, 0xffffffff), // 0x2dbc 
    (0x94000000, 0xfc000000), // 0x2dc0 BL
    (0x12bfffc0, 0xffffffff), // 0x2dc4 
    (0x6b00029f, 0xffffffff), // 0x2dc8 
    (0x54000009, 0xff00001f), // 0x2dcc b.cond
    (0xf9001bf7, 0xffffffff), // 0x2dd0 
    (0x52a00053, 0xffffffff), // 0x2dd4 
    (0x2a1303f7, 0xffffffff), // 0x2dd8 
    (0x2a1603e0, 0xffffffff), // 0x2ddc 
    (0x94000000, 0xfc000000), // 0x2de0 BL
    (0xd2800003, 0xffffffff), // 0x2de4 
    (0x2a1703e2, 0xffffffff), // 0x2de8 
    (0xaa0003e1, 0xffffffff), // 0x2dec 
    (0xaa1503e0, 0xffffffff), // 0x2df0 
    (0x94000000, 0xfc000000), // 0x2df4 BL
    (0x914082b5, 0xffffffff), // 0x2df8 
    (0x914082d6, 0xffffffff), // 0x2dfc 
    (0x2a1303e0, 0xffffffff), // 0x2e00 
    (0x94000000, 0xfc000000), // 0x2e04 BL
    (0x11408273, 0xffffffff), // 0x2e08 
    (0x6b13029f, 0xffffffff), // 0x2e0c 
    (0x54000002, 0xff00001f), // 0x2e10 b.cond
    (0xf9401bf7, 0xffffffff), // 0x2e14 
    (0x72004294, 0xffffffff), // 0x2e18 
    (0x54000001, 0xff00001f), // 0x2e1c b.cond
    (0x94000000, 0xfc000000), // 0x2e20 BL
    (0xa9425bf5, 0xffffffff), // 0x2e24 
    (0x14000000, 0xfc000000), // 0x2e28 B
    (0x2a1603e0, 0xffffffff), // 0x2e2c 
    (0x94000000, 0xfc000000), // 0x2e30 BL
    (0xd2800003, 0xffffffff), // 0x2e34 
    (0x2a1403e2, 0xffffffff), // 0x2e38 
    (0xaa0003e1, 0xffffffff), // 0x2e3c 
    (0xaa1503e0, 0xffffffff), // 0x2e40 
    (0x94000000, 0xfc000000), // 0x2e44 BL
    (0x14000000, 0xfc000000), // 0x2e48 B
];
/// Words of the function that the packager reads targets from: the row lookup call, the raw path's
/// copy call and the compressed path's copy call.
const OS_LOOKUP_WORD: usize = 2;
const OS_RAW_COPY_WORD: usize = 15;
const OS_COMPRESSED_COPY_WORDS: [usize; 2] = [(0x2df4 - 0x2d50) / 4, (0x2e44 - 0x2d50) / 4];
/// The section the stub is for.
pub(super) const OS_SECTION_ID: u32 = 6;
/// `mov w0, #6`, the instruction before the call that is replaced.
const OS_SET_ID: u32 = 0x5280_0000 | (OS_SECTION_ID << 5);
/// The bytes the stub's copy function takes per call: the chunk of the stage's own loads.
pub(super) const OS_CHUNK: u64 = 0x2_0000;
/// Loop counts of one lamp pulse and one pause of the OS stub: a little over half a second and a
/// little under half a second at the speed of the stage's code on a camera (about 4.4 M loops per second).
pub(super) const OS_PULSE_LOOPS: u64 = 3_000_000;
pub(super) const OS_GAP_LOOPS: u64 = 2_000_000;
/// The stub's parameter block (`os_stub.S`) and the loader's binding of the OS (`os.h`).
pub(super) const OS_STUB_PARAMS: usize = 184;
pub(super) const OS_BINDING: usize = 96;

/// What the stock reset image and the update say about the OS section's load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OsPlan {
    /// RAM-0 byte offset of the call of the load function with the OS id, its stock word, and the load
    /// function's and the copy function's RAM-0 addresses.
    pub call_site: usize,
    pub call_old: u32,
    pub load_fn: usize,
    pub copy_fn: usize,
    /// The section: DRAM address, the decoded bytes the stage initializes (the rest of the engine's
    /// output is zero), the flash offset and length of the compressed bytes.
    pub dst: u64,
    pub head_bytes: usize,
    pub flash_offset: u64,
    pub comp_bytes: usize,
    /// The two section tables the stage may read: RAM address, flash offset; their length.
    pub tables: [(u64, u64); 2],
    pub table_bytes: usize,
}

/// The byte offset in `table` of the row for `id`.
fn row_at(table: &[u8], table_ram: u64, id: u32) -> Result<usize> {
    require(table.len() >= 0x100, "A section table is too short")?;
    let rows = u64::from_le_bytes(table[..8].try_into().unwrap());
    let first = rows
        .checked_sub(table_ram)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n >= 0x40 && n < table.len())
        .ok_or_else(|| image::Error::Invalid("A section table's row pointer leaves the table".into()))?;
    let mut found = None;
    for at in (first..table.len().saturating_sub(ROW - 1)).step_by(ROW) {
        let row = word(table, at)?;
        if row == END {
            break;
        }
        if row == id {
            require(found.is_none(), "A section table has two rows for the OS section")?;
            found = Some(at);
        }
    }
    found.ok_or_else(|| image::Error::Invalid("A section table has no row for the OS section".into()))
}

/// The OS section's row: DRAM address, the decoded bytes it initializes, the bss that follows them,
/// the flash offset and length of its compressed bytes (the row must say it is compressed).
fn os_row(table: &[u8], table_ram: u64) -> Result<(u64, u64, u64, u64, usize)> {
    let at = row_at(table, table_ram, OS_SECTION_ID)?;
    let q = |o: usize| u64::from_le_bytes(table[at + o..at + o + 8].try_into().unwrap());
    let (dst, declared, bss) = (q(8), q(16), q(24));
    let flash = u64::from(word(table, at + 40)?);
    let size = word(table, at + 48)? as usize;
    require(word(table, at + 56)? != 0, "The OS section is not stored compressed")?;
    require(bss == dst + declared, "The OS section's bss does not follow its initialized bytes")?;
    Ok((dst, declared, bss, flash, size))
}

/// Find the OS section's load in `reset`, the stock reset image, and check it against the update
/// (`inverted`, whose flash offset `n` is at `n + bias`) and the card's RAM image `ram`.
pub(super) fn locate_os(reset: &[u8], inverted: &[u8], bias: usize, ram: &[u8]) -> Result<OsPlan> {
    let loads = find(reset, &OS_LOAD);
    require(loads.len() == 1, "The reset stage's OS section load is not found exactly once")?;
    let load_fn = loads[0];
    // Its callers load sections with other ids too; exactly one of them passes the OS id.
    let callers: Vec<usize> = (0..reset.len() - 3)
        .step_by(4)
        .filter(|&at| word(reset, at).is_ok_and(|w| w & BL.1 == BL.0) && bl_target(reset, at).ok() == Some(load_fn))
        .collect();
    let ours: Vec<usize> = callers
        .iter()
        .copied()
        .filter(|&at| at >= 4 && word(reset, at - 4).is_ok_and(|w| w == OS_SET_ID))
        .collect();
    require(
        callers.len() == 3 && ours.len() == 1,
        "The reset stage's OS section load has not exactly one caller that passes the OS id (of three)",
    )?;
    let call_site = ours[0];
    let lookup = bl_target(reset, load_fn + 4 * OS_LOOKUP_WORD)?;
    let copy_fn = bl_target(reset, load_fn + 4 * OS_RAW_COPY_WORD)?;
    require(
        OS_COMPRESSED_COPY_WORDS.iter().all(|&w| bl_target(reset, load_fn + 4 * w).ok() == Some(copy_fn)),
        "The OS section's load calls more than one copy function",
    )?;
    // The two table copies call the same copy function, and their destinations are the two tables.
    let copies = find(reset, &TABLE_COPY);
    require(copies.len() == 2, "The reset stage's section table copies are not found twice")?;
    let mut tables = Vec::new();
    for &at in &copies {
        require(
            word(reset, at + 4 * 8)? == 0x5281_0002 && bl_target(reset, at + 4 * 11)? == copy_fn,
            "A section table copy differs (0x800 length or copy function)",
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
    require(lookup % 4 == 0 && lookup + 4 <= reset.len(), "The row lookup is outside the reset image")?;
    // The OS row is in the main table (the one at RAM 0x40000) only: the other table is the maintenance
    // table, which names other sections, and with it the stage never reaches the call that is replaced.
    // Both are read from the flash itself (the update).
    require(tables[0].0 == 0x4_0000, "The main section table is not at the stage's table address")?;
    let main = extent(inverted, tables[0].1 as usize + bias, table_bytes)?;
    let other = extent(inverted, tables[1].1 as usize + bias, table_bytes)?;
    require(
        row_at(other, tables[1].0, OS_SECTION_ID).is_err(),
        "The maintenance section table names the OS section",
    )?;
    let (dst, declared, _bss, flash_offset, comp_bytes) = os_row(main, tables[0].0)?;
    let head_bytes = declared as usize;
    require(
        head_bytes != 0 && head_bytes % 16 == 0 && dst % 16 == 0 && comp_bytes % 4 == 0 && comp_bytes != 0,
        "The OS section's row is not a whole number of 16-byte units",
    )?;
    // The cold table in the RAM image is the flash's main one.
    if let Some(&(table_ram, flash)) = tables.iter().find(|&&(r, _)| r == 0x4_0000) {
        require(
            extent(ram, table_ram as usize, table_bytes)? == extent(inverted, flash as usize + bias, table_bytes)?,
            "The RAM image's section table differs from the flash's",
        )?;
    }
    // The card's decoded section is the software decode of the flash's compressed bytes, and what the
    // engine writes after it is zero (so the stub need compare no more than the initialized bytes).
    let compressed = extent(inverted, flash_offset as usize + bias, comp_bytes)?;
    let decoded = fujimod_firmware::decompress(compressed)?.0;
    let head = extent(ram, dst as usize, head_bytes)?;
    require(
        decoded.len() >= head_bytes && decoded[..head_bytes] == *head,
        "The RAM image's OS section is not the decode of the flash's compressed bytes",
    )?;
    require(decoded[head_bytes..].iter().all(|b| *b == 0), "The decode of the OS section is not zero after its initialized bytes")?;
    Ok(OsPlan {
        call_site,
        call_old: word(reset, call_site)?,
        load_fn,
        copy_fn,
        dst,
        head_bytes,
        flash_offset,
        comp_bytes,
        tables: [tables[0], tables[1]],
        table_bytes,
    })
}

/// The values of the OS stub's parameter block (`os_stub.S` documents each).
pub(super) struct OsStubValues {
    pub own_address: u64,
    pub ret: u64,
    pub id: u64,
    pub func: u64,
    pub dst: u64,
    pub bytes: u64,
    pub dram: u64,
    pub copy: u64,
    pub chunk: u64,
    pub green: u64,
    pub red: u64,
    pub pin: u64,
    pub pulse: u64,
    pub gap: u64,
    pub enable: u64,
    pub enable_green: u64,
    pub enable_red: u64,
    pub mux: u64,
    pub restore: u64,
}

/// The stub image with its parameter block filled in. The template's block must be all zero and the
/// parameters must be where the layout says.
pub(super) fn fill_os_stub(template: &[u8], values: &OsStubValues) -> Result<Vec<u8>> {
    use super::os_stub_template as t;
    let params = t::XP5_OS_PARAMS;
    require(
        template.len() == t::XP5_OS_END
            && params + OS_STUB_PARAMS == template.len()
            && template[params..].iter().all(|b| *b == 0),
        "The OS stub template changed",
    )?;
    let mut image = template.to_vec();
    for (offset, value) in [
        (t::XP5_OS_P_SELF, values.own_address),
        (t::XP5_OS_P_RETURN, values.ret),
        (t::XP5_OS_P_ID, values.id),
        (t::XP5_OS_P_FUNC, values.func),
        (t::XP5_OS_P_DST, values.dst),
        (t::XP5_OS_P_BYTES, values.bytes),
        (t::XP5_OS_P_DRAM, values.dram),
        (t::XP5_OS_P_COPY, values.copy),
        (t::XP5_OS_P_CHUNK, values.chunk),
        (t::XP5_OS_P_GREEN, values.green),
        (t::XP5_OS_P_RED, values.red),
        (t::XP5_OS_P_PIN, values.pin),
        (t::XP5_OS_P_PULSE, values.pulse),
        (t::XP5_OS_P_GAP, values.gap),
        (t::XP5_OS_P_ENABLE, values.enable),
        (t::XP5_OS_P_ENABLE_GREEN, values.enable_green),
        (t::XP5_OS_P_ENABLE_RED, values.enable_red),
        (t::XP5_OS_P_MUX, values.mux),
        (t::XP5_OS_P_RESTORE, values.restore),
    ] {
        require(offset >= params && offset + 8 <= image.len(), "An OS stub parameter is outside its block")?;
        put64(&mut image, offset, value)?;
    }
    Ok(image)
}

/// The bindings of an OS loader: the redirect words (the call's stock and new word, the CRC-32s of the
/// stock and the patched RAM-0 image, the DRAM copy and the compressed flash range, the two section
/// tables), the OS words (`setup_sd_os`) and the stub.
pub(super) struct OsBinding {
    pub stub: StubBinding,
    pub os: [u64; 8],
}

/// Plan the OS loader's bindings. `destination` is where the read-only startup streams the file (the
/// DRAM copy of the section is the whole file: `head`, the section's initialized bytes), `reset` the
/// stock reset image, `template` the stub image from its asset.
pub(super) fn os_binding(
    plan: &OsPlan, head: &[u8], reset: &[u8], inverted: &[u8], bias: usize, aperture: u64, destination: u64,
    guard_bytes: usize, apply: bool, lamp: &StubLamp, template: &[u8],
) -> Result<OsBinding> {
    require(
        head.len() == plan.head_bytes
            && destination % 0x1000 == 0
            && (READ_ONLY_FLOOR..READ_ONLY_CEILING).contains(&destination)
            && destination + head.len() as u64 <= READ_ONLY_CEILING,
        "The OS section's DRAM copy does not fit the read-only destination",
    )?;
    require(
        plan.call_old & BL.1 == BL.0 && plan.call_site % 4 == 0 && plan.call_site + 4 <= guard_bytes
            && plan.copy_fn % 4 == 0 && plan.copy_fn + 4 <= guard_bytes && plan.load_fn % 4 == 0
            && plan.load_fn + 4 <= guard_bytes,
        "The reset stage's OS load call is not a bl inside the RAM-0 guard",
    )?;
    require(
        bl_target(reset, plan.call_site)? == plan.load_fn,
        "The OS load call does not call the load function",
    )?;
    let offset = sled(extent(reset, 0, guard_bytes)?, template.len())?;
    require(
        offset + template.len() + SLED_MARGIN <= guard_bytes
            && !(offset..offset + template.len()).contains(&plan.call_site),
        "The OS stub overlaps the call or leaves the RAM-0 guard",
    )?;
    let stock = extent(reset, 0, guard_bytes)?;
    require(
        lamp.pin < 8 && lamp.green_port < 16 && lamp.red_port < 16 && lamp.green_port != lamp.red_port
            && lamp.green_port != 3 && lamp.red_port != 3,
        "The OS stub's lamp is not a usable pair of ports",
    )?;
    let comp = extent(inverted, plan.flash_offset as usize + bias, plan.comp_bytes)?;
    let image = fill_os_stub(
        template,
        &OsStubValues {
            own_address: offset as u64,
            ret: plan.call_site as u64 + 4,
            id: u64::from(OS_SECTION_ID),
            func: plan.load_fn as u64,
            dst: plan.dst,
            bytes: head.len() as u64,
            dram: destination,
            copy: plan.copy_fn as u64,
            chunk: OS_CHUNK,
            green: lamp.pio_base + 64 * u64::from(lamp.green_port),
            red: lamp.pio_base + 64 * u64::from(lamp.red_port),
            pin: 1u64 << lamp.pin,
            pulse: OS_PULSE_LOOPS,
            gap: OS_GAP_LOOPS,
            enable: lamp.pio_base + 0x500,
            enable_green: (1u64 << (lamp.green_port + 16)) | (1u64 << lamp.green_port),
            enable_red: (1u64 << (lamp.red_port + 16)) | (1u64 << lamp.red_port),
            mux: 3u64 << (2 * lamp.pin + 16),
            restore: (1u64 << (lamp.pin + 16)) | (1u64 << lamp.pin),
        },
    )?;
    let new = bl_word(plan.call_site, offset)?;
    require(new != plan.call_old, "The OS call word is the stock word")?;
    let mut patched = stock.to_vec();
    patched[offset..offset + image.len()].copy_from_slice(&image);
    patched[plan.call_site..plan.call_site + 4].copy_from_slice(&new.to_le_bytes());
    let mut words = [0u64; 16];
    words[0] = 1;
    words[1] = u64::from(apply);
    words[2] = plan.call_site as u64;
    words[3] = u64::from(plan.call_old);
    words[4] = u64::from(new);
    words[5] = u64::from(crc32(stock));
    words[6] = u64::from(crc32(&patched));
    words[7] = 0; // the DRAM copy is the whole file
    words[8] = head.len() as u64;
    words[9] = aperture + plan.flash_offset;
    words[10] = u64::from(crc32(head));
    words[11] = plan.table_bytes as u64;
    for (n, &(_, flash)) in plan.tables.iter().enumerate() {
        let table = extent(inverted, flash as usize + bias, plan.table_bytes)?;
        words[12 + n] = aperture + flash;
        words[14 + n] = u64::from(crc32(table));
    }
    let os = [1, plan.comp_bytes as u64, u64::from(crc32(comp)), u64::from(OS_SECTION_ID), plan.dst, plan.copy_fn as u64, OS_CHUNK, 0];
    Ok(OsBinding {
        stub: StubBinding {
            redirect: Binding(words),
            offset,
            params_offset: super::os_stub_template::XP5_OS_PARAMS,
            crc32: crc32(&image),
            image,
        },
        os,
    })
}

/// Write the OS binding into the loader at `at` after checking that the template is untouched (`abi`
/// 1, everything else zero: the struct is 96 bytes).
pub(super) fn bind_os(program: &mut [u8], at: usize, os: &[u64; 8]) -> Result<()> {
    let mut template = vec![0u8; OS_BINDING];
    template[..8].copy_from_slice(&1u64.to_le_bytes());
    require(extent(program, at, OS_BINDING)? == template, "XP5 OS binding template changed")?;
    for (n, value) in os.iter().enumerate() {
        put64(program, at + 8 * n, *value)?;
    }
    Ok(())
}

// ---- Named words of the OS section: a metadata string, changed after the stage's own decode ----
//
// The same load as above (`mov w0, #6; bl load`) is bound to a different stub (`exif_stub.S`). It does not
// compare the decode with a copy from the card and copies nothing: it lets the stage decode, checks each
// named 32-bit word of the fresh decode against its bound stock value and only if all match writes the new
// values with the CPU and reads them back. The named words are the 16 bytes of one constant string that the
// OS copies into the Software field of every Exif block it writes, and the words around them that anchor the
// decode. The card's file is a small placeholder: the read-only startup needs a file to read, and the loader
// proves the compressed flash range, the section's row, the stage's call and the table of named words.

/// The stub's parameter block (`exif_stub.S`), the loader's binding of the named words (`exif.h`), and the
/// table of at most this many named words.
pub(super) const EXIF_STUB_PARAMS: usize = 296;
pub(super) const EXIF_BINDING: usize = 296;
pub(super) const EXIF_WORDS: usize = 8;
/// The string as the stock OS holds it (15 characters and the NUL: both places that use it copy 15 bytes), and
/// as the stub makes it. The same length, a NUL in the same place; only the first eight bytes change.
pub(super) const EXIF_STOCK_TEXT: &[u8; 16] = b"Digital Camera \0";
pub(super) const EXIF_NEW_TEXT: &[u8; 16] = b"FUJIMOD Camera \0";
/// `mov w2, #15`, which both of the OS's sites that copy the string execute right before its `adrp`.
const EXIF_COPY_LENGTH: u32 = 0x5280_01e2;
/// Words of context named on either side of the string, to anchor the decode (check-only).
const EXIF_ANCHORS: usize = 2;

/// One named word: its address in the destination, the value the hardware decode of the stock flash must hold
/// there, and the value it gets (equal to `stock` for a check-only word).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExifWord {
    pub address: u64,
    pub stock: u32,
    pub new: u32,
}

/// What the stock reset image and the update say about the OS section's load (`OsPlan`), where the string
/// is, and the table of named words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ExifPlan {
    pub os: OsPlan,
    /// The address of the string (the window: the only place a word changes).
    pub window: u64,
    pub words: Vec<ExifWord>,
    /// The two code sites (addresses of their `adrp`) that copy the string into the Exif block's Software field.
    pub sites: [u64; 2],
}

/// The addresses of the `adrp` words in `head` (loaded at `dst`) that, with the `add` right after them, build
/// the address `target`, and that are preceded by `mov w2, #15`. Every other kind of reference into
/// `[target, target + 16)` (a branch, `adr`, literal, data value) is an error: the string must be reached only
/// that way.
fn exif_sites(head: &[u8], dst: u64, target: u64) -> Result<Vec<u64>> {
    let hits = references_into_at(head, dst as usize, target as usize, target as usize + 16, true);
    require(
        hits.iter().all(|&(_, kind)| kind == "adrp page"),
        "The Exif string is reached by something other than an adrp and an add",
    )?;
    let mut sites = Vec::new();
    for at in (4..head.len().saturating_sub(7)).step_by(4) {
        let w = word(head, at)?;
        if w & ADRP_MASK != ADRP {
            continue;
        }
        let imm = (((w >> 5) & 0x7_ffff) << 2) | ((w >> 29) & 3);
        let signed = (i64::from(imm) << 43) >> 43;
        let page = ((((dst as i64) + at as i64) >> 12) + signed) << 12;
        if page != (target as i64 & !0xfff) {
            continue;
        }
        let add = word(head, at + 4)?;
        let rd = w & 31;
        let is_add = add & 0xffc0_0000 == 0x9100_0000 && (add >> 5) & 31 == rd && add & 31 == rd;
        if is_add && page as u64 + u64::from((add >> 10) & 0xfff) == target {
            require(word(head, at - 4)? == EXIF_COPY_LENGTH, "A use of the Exif string does not copy 15 bytes")?;
            sites.push(dst + at as u64);
        }
    }
    Ok(sites)
}

/// Find the string in the OS section's decode and name its words.
pub(super) fn locate_exif(reset: &[u8], inverted: &[u8], bias: usize, ram: &[u8]) -> Result<ExifPlan> {
    let os = locate_os(reset, inverted, bias, ram)?;
    let head = extent(ram, os.dst as usize, os.head_bytes)?;
    let found: Vec<usize> =
        (0..head.len().saturating_sub(15)).filter(|&at| head[at..at + 16] == EXIF_STOCK_TEXT[..]).collect();
    require(found.len() == 1, "The Exif string is not in the OS section exactly once")?;
    let at = found[0];
    require(
        at % 4 == 0 && at >= 4 * EXIF_ANCHORS && at + 16 + 4 * EXIF_ANCHORS <= head.len(),
        "The Exif string is not word aligned inside the section",
    )?;
    let window = os.dst + at as u64;
    let sites = exif_sites(head, os.dst, window)?;
    require(sites.len() == 2, "The Exif string is not used by exactly two sites")?;
    let mut words = Vec::new();
    for n in 0..(4 + 2 * EXIF_ANCHORS) {
        let offset = at - 4 * EXIF_ANCHORS + 4 * n;
        let stock = word(head, offset)?;
        let inside = (at..at + 16).contains(&offset);
        let new = if inside {
            u32::from_le_bytes(EXIF_NEW_TEXT[offset - at..offset - at + 4].try_into().unwrap())
        } else {
            stock
        };
        words.push(ExifWord { address: os.dst + offset as u64, stock, new });
    }
    require(words.iter().any(|w| w.stock != w.new), "The Exif string is already the new text")?;
    Ok(ExifPlan { os, window, words, sites: [sites[0], sites[1]] })
}

/// The values of the Exif stub's parameter block (`exif_stub.S` documents each).
pub(super) struct ExifStubValues {
    pub own_address: u64,
    pub ret: u64,
    pub id: u64,
    pub func: u64,
    pub dst: u64,
    pub bytes: u64,
    pub green: u64,
    pub red: u64,
    pub pin: u64,
    pub pulse: u64,
    pub gap: u64,
    pub enable: u64,
    pub enable_green: u64,
    pub enable_red: u64,
    pub mux: u64,
    pub restore: u64,
    pub words: Vec<ExifWord>,
}

/// The stub image with its parameter block filled in. The template's block must be all zero and the
/// parameters must be where the layout says.
pub(super) fn fill_exif_stub(template: &[u8], values: &ExifStubValues) -> Result<Vec<u8>> {
    use super::exif_stub_template as t;
    let params = t::XP5_EXIF_PARAMS;
    require(
        template.len() == t::XP5_EXIF_END
            && params + EXIF_STUB_PARAMS == template.len()
            && template[params..].iter().all(|b| *b == 0)
            && t::XP5_EXIF_P_TABLE + 16 * EXIF_WORDS == t::XP5_EXIF_OUT_STATUS
            && !values.words.is_empty()
            && values.words.len() <= EXIF_WORDS,
        "The Exif stub template changed",
    )?;
    let mut image = template.to_vec();
    for (offset, value) in [
        (t::XP5_EXIF_P_SELF, values.own_address),
        (t::XP5_EXIF_P_RETURN, values.ret),
        (t::XP5_EXIF_P_ID, values.id),
        (t::XP5_EXIF_P_FUNC, values.func),
        (t::XP5_EXIF_P_DST, values.dst),
        (t::XP5_EXIF_P_BYTES, values.bytes),
        (t::XP5_EXIF_P_GREEN, values.green),
        (t::XP5_EXIF_P_RED, values.red),
        (t::XP5_EXIF_P_PIN, values.pin),
        (t::XP5_EXIF_P_PULSE, values.pulse),
        (t::XP5_EXIF_P_GAP, values.gap),
        (t::XP5_EXIF_P_ENABLE, values.enable),
        (t::XP5_EXIF_P_ENABLE_GREEN, values.enable_green),
        (t::XP5_EXIF_P_ENABLE_RED, values.enable_red),
        (t::XP5_EXIF_P_MUX, values.mux),
        (t::XP5_EXIF_P_RESTORE, values.restore),
        (t::XP5_EXIF_P_COUNT, values.words.len() as u64),
    ] {
        require(offset >= params && offset + 8 <= image.len(), "An Exif stub parameter is outside its block")?;
        put64(&mut image, offset, value)?;
    }
    for (n, w) in values.words.iter().enumerate() {
        let at = t::XP5_EXIF_P_TABLE + 16 * n;
        put64(&mut image, at, w.address)?;
        put64(&mut image, at + 8, u64::from(w.stock) | (u64::from(w.new) << 32))?;
    }
    Ok(image)
}

/// The bindings of an Exif loader: the redirect words (the call's stock and new word, the CRC-32s of the stock and
/// the patched RAM-0 image, the file and the compressed flash range, the two section tables), the named-word
/// binding (`setup_sd_exif`, 37 words) and the stub.
pub(super) struct ExifBinding {
    pub stub: StubBinding,
    pub exif: Vec<u64>,
}

/// Plan the Exif loader's bindings. `file` is the placeholder the read-only startup streams to `destination`,
/// `reset` the stock reset image, `template` the stub image from its asset.
pub(super) fn exif_binding(
    plan: &ExifPlan, file: &[u8], reset: &[u8], inverted: &[u8], bias: usize, aperture: u64, destination: u64,
    guard_bytes: usize, apply: bool, lamp: &StubLamp, template: &[u8],
) -> Result<ExifBinding> {
    let os = &plan.os;
    require(
        !file.is_empty()
            && file.len() <= 0x1_0000
            && destination % 0x1000 == 0
            && (READ_ONLY_FLOOR..READ_ONLY_CEILING).contains(&destination)
            && destination + file.len() as u64 <= READ_ONLY_CEILING,
        "The placeholder file does not fit the read-only destination",
    )?;
    require(
        os.call_old & BL.1 == BL.0
            && os.call_site % 4 == 0
            && os.call_site >= 4
            && os.call_site + 4 <= guard_bytes
            && os.load_fn % 4 == 0
            && os.load_fn + 4 <= guard_bytes,
        "The reset stage's OS load call is not a bl inside the RAM-0 guard",
    )?;
    require(bl_target(reset, os.call_site)? == os.load_fn, "The OS load call does not call the load function")?;
    require(
        word(reset, os.call_site - 4)? == OS_SET_ID && word(reset, os.load_fn)? == OS_LOAD[0].0,
        "The reset stage's OS call does not pass the OS id to a function that starts as the load does",
    )?;
    require(
        plan.words.len() <= EXIF_WORDS
            && plan.words.windows(2).all(|p| p[0].address < p[1].address)
            && plan.words.iter().all(|w| {
                w.address % 4 == 0
                    && w.address >= os.dst
                    && w.address + 4 <= os.dst + os.head_bytes as u64
                    && (w.stock == w.new || (plan.window..plan.window + 16).contains(&w.address))
            }),
        "The named words are not ascending words inside the section, changing only inside the string",
    )?;
    let offset = sled(extent(reset, 0, guard_bytes)?, template.len())?;
    require(
        offset + template.len() + SLED_MARGIN <= guard_bytes && !(offset..offset + template.len()).contains(&os.call_site),
        "The Exif stub overlaps the call or leaves the RAM-0 guard",
    )?;
    let stock = extent(reset, 0, guard_bytes)?;
    require(
        lamp.pin < 8
            && lamp.green_port < 16
            && lamp.red_port < 16
            && lamp.green_port != lamp.red_port
            && lamp.green_port != 3
            && lamp.red_port != 3,
        "The Exif stub's lamp is not a usable pair of ports",
    )?;
    let comp = extent(inverted, os.flash_offset as usize + bias, os.comp_bytes)?;
    let image = fill_exif_stub(
        template,
        &ExifStubValues {
            own_address: offset as u64,
            ret: os.call_site as u64 + 4,
            id: u64::from(OS_SECTION_ID),
            func: os.load_fn as u64,
            dst: os.dst,
            bytes: os.head_bytes as u64,
            green: lamp.pio_base + 64 * u64::from(lamp.green_port),
            red: lamp.pio_base + 64 * u64::from(lamp.red_port),
            pin: 1u64 << lamp.pin,
            pulse: OS_PULSE_LOOPS,
            gap: OS_GAP_LOOPS,
            enable: lamp.pio_base + 0x500,
            enable_green: (1u64 << (lamp.green_port + 16)) | (1u64 << lamp.green_port),
            enable_red: (1u64 << (lamp.red_port + 16)) | (1u64 << lamp.red_port),
            mux: 3u64 << (2 * lamp.pin + 16),
            restore: (1u64 << (lamp.pin + 16)) | (1u64 << lamp.pin),
            words: plan.words.clone(),
        },
    )?;
    let new = bl_word(os.call_site, offset)?;
    require(new != os.call_old, "The OS call word is the stock word")?;
    let mut patched = stock.to_vec();
    patched[offset..offset + image.len()].copy_from_slice(&image);
    patched[os.call_site..os.call_site + 4].copy_from_slice(&new.to_le_bytes());
    let mut words = [0u64; 16];
    words[0] = 1;
    words[1] = u64::from(apply);
    words[2] = os.call_site as u64;
    words[3] = u64::from(os.call_old);
    words[4] = u64::from(new);
    words[5] = u64::from(crc32(stock));
    words[6] = u64::from(crc32(&patched));
    words[7] = 0; // the file is at offset 0 of the copy
    words[8] = file.len() as u64;
    words[9] = aperture + os.flash_offset;
    words[10] = u64::from(crc32(file));
    words[11] = os.table_bytes as u64;
    for (n, &(_, flash)) in os.tables.iter().enumerate() {
        let table = extent(inverted, flash as usize + bias, os.table_bytes)?;
        words[12 + n] = aperture + flash;
        words[14 + n] = u64::from(crc32(table));
    }
    let mut exif = vec![0u64; EXIF_BINDING / 8];
    for (n, v) in [
        1,
        aperture,
        os.comp_bytes as u64,
        u64::from(crc32(comp)),
        os.tables[0].0,
        u64::from(OS_SECTION_ID),
        os.dst,
        os.head_bytes as u64,
        plan.window,
        16,
        plan.words.len() as u64,
        u64::from(crc32(EXIF_STOCK_TEXT)),
        u64::from(crc32(EXIF_NEW_TEXT)),
    ]
    .into_iter()
    .enumerate()
    {
        exif[n] = v;
    }
    for (n, w) in plan.words.iter().enumerate() {
        exif[13 + 2 * n] = w.address;
        exif[14 + 2 * n] = u64::from(w.stock) | (u64::from(w.new) << 32);
    }
    Ok(ExifBinding {
        stub: StubBinding {
            redirect: Binding(words),
            offset,
            params_offset: super::exif_stub_template::XP5_EXIF_PARAMS,
            crc32: crc32(&image),
            image,
        },
        exif,
    })
}

/// Write the Exif binding into the loader at `at` after checking that the template is untouched (`abi`
/// 1, everything else zero: the struct is 296 bytes).
pub(super) fn bind_exif(program: &mut [u8], at: usize, exif: &[u64]) -> Result<()> {
    let mut template = vec![0u8; EXIF_BINDING];
    template[..8].copy_from_slice(&1u64.to_le_bytes());
    require(exif.len() * 8 == EXIF_BINDING, "The Exif binding has the wrong size")?;
    require(extent(program, at, EXIF_BINDING)? == template, "XP5 Exif binding template changed")?;
    for (n, value) in exif.iter().enumerate() {
        put64(program, at + 8 * n, *value)?;
    }
    Ok(())
}

// ---- E5b-6: a hook at the entry of one OS function, and a small program in alignment padding ----
//
// The same route as the Exif words above (the stage's load of the OS section is bound to a named-word stub: the
// decode finishes, every named word is checked against its stock value, and only then are the new values
// written), with thirteen other words. They make a branch of the first word of the one OS function that copies
// the Exif identity record (the record's Software text is eight bytes `Digital ` followed by `Camera ` and the
// model and version) into the OS's global, and a program of twelve words in alignment padding of the OS's code
// (in a run of zero and nop words after a `ret`: nothing refers to it, and the code is mapped executable by the
// OS's own memory map), which replaces `Digital ` by the new eight bytes in the record the function was given
// and only if it says `Digital `, then runs the displaced first instruction and returns into the function.

/// The stub's parameter block (`hook_stub.S`), the loader's binding of the named words (`hook.h`) and the
/// stub's table capacity.
pub(super) const HOOK_STUB_PARAMS: usize = 424;
pub(super) const HOOK_BINDING: usize = 424;
pub(super) const HOOK_TABLE: usize = 16;
/// The hook word and the twelve cave words.
pub(super) const HOOK_NAMED: usize = 13;
pub(super) const HOOK_CAVE_WORDS: usize = 12;
/// The function's first instruction, `stp x29, x30, [sp, #-16]!`, which the program runs in its place.
pub(super) const HOOK_DISPLACED: u32 = 0xa9bf_7bfd;
/// The text the program looks for at offset 0x30 of the record, and the text it writes there.
pub(super) const HOOK_OLD_TEXT: &[u8; 8] = b"Digital ";
pub(super) const HOOK_NEW_TEXT: &[u8; 8] = b"FUJIMOD ";
/// The words the packager requires of the padding run: at least this many, directly after a `ret`.
const HOOK_RUN_MIN_WORDS: usize = 14;
/// The OS function that copies the 0xa8-byte record at x0 into a global: nine words.
const HOOK_FUNCTION: [Pat; 9] = [
    (0xa9bf_7bfd, !0),          // stp x29, x30, [sp, #-16]!
    (0x9100_03fd, !0),          // mov x29, sp
    (0x5280_1502, !0),          // mov w2, #0xa8
    (0xaa00_03e1, !0),          // mov x1, x0
    (0x9000_0000, 0x9f00_001f), // adrp x0, page
    (0x9100_0000, 0xffc0_03ff), // add x0, x0, #lo
    BL,                         // bl memcpy
    (0xa8c1_7bfd, !0),          // ldp x29, x30, [sp], #16
    (0xd65f_03c0, !0),          // ret
];
/// The OS's own memory map of its image: `mov w2, #7; adrp x1, end; add x1, x1, #0; adrp x0, start; add x0, x0, #0;
/// bl map`. Flags 7 are read, write and execute: the code. The section's data after it is mapped without execute.
const HOOK_TEXT_MAP: [Pat; 6] = [
    (0x5280_00e2, !0),
    (0x9000_0001, 0x9f00_001f),
    (0x9100_0021, 0xffc0_03ff),
    (0x9000_0000, 0x9f00_001f),
    (0x9100_0000, 0xffc0_03ff),
    BL,
];

/// `b target` placed at `from` (a 26-bit word offset), or an error.
fn hook_branch(from: u64, to: u64) -> Result<u32> {
    let d = to as i64 - from as i64;
    require(d % 4 == 0 && (-(1i64 << 27)..(1i64 << 27)).contains(&d), "A branch of the hook program is out of range")?;
    Ok(0x1400_0000 | (((d >> 2) as u32) & 0x03ff_ffff))
}

/// The 19-bit word offset field of a PC-relative load or conditional branch.
fn hook_imm19(from: u64, to: u64) -> u32 {
    ((((to as i64 - from as i64) >> 2) as u32) & 0x7_ffff) << 5
}

/// The thirteen new words of the hook: the first is the branch at `hook`, then twelve words from `cave`: the two
/// eight-byte texts, and the program that is entered at `cave + 16`.
pub(super) fn hook_program(hook: u64, cave: u64, new_text: &[u8; 8]) -> Result<[u32; HOOK_NAMED]> {
    let old = u64::from_le_bytes(*HOOK_OLD_TEXT);
    let new = u64::from_le_bytes(*new_text);
    Ok([
        hook_branch(hook, cave + 16)?,
        old as u32,
        (old >> 32) as u32,
        new as u32,
        (new >> 32) as u32,
        0xf940_1810,                                      // ldr x16, [x0, #48]
        0x5800_0011 | hook_imm19(cave + 20, cave),        // ldr x17, <old text>
        0xeb11_021f,                                      // cmp x16, x17
        0x5400_0001 | hook_imm19(cave + 28, cave + 40),   // b.ne the displaced instruction
        0x5800_0011 | hook_imm19(cave + 32, cave + 8),    // ldr x17, <new text>
        0xf900_1811,                                      // str x17, [x0, #48]
        HOOK_DISPLACED,                                   // stp x29, x30, [sp, #-16]!
        hook_branch(cave + 44, hook + 4)?,                // b hook + 4
    ])
}

/// What the stock reset image and the update say about the OS section's load (`OsPlan`), where the hooked function
/// and the padding are, and the table of the thirteen named words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HookPlan {
    pub os: OsPlan,
    pub hook: u64,
    pub cave: u64,
    /// The end of the OS's executable range (the OS's own map of its image: start at the section, flags 7).
    pub text_end: u64,
    /// The calls of the hooked function (each preceded by `add x0, sp, #n`).
    pub callers: Vec<u64>,
    pub words: Vec<ExifWord>,
}

/// Count the `adrp` words in `head` (loaded at `dst`) that, with the `add` right after them, build an address in
/// `[lo, hi)`.
fn hook_adrp_add_into(head: &[u8], dst: u64, lo: u64, hi: u64) -> Result<usize> {
    let mut found = 0;
    for at in (0..head.len().saturating_sub(7)).step_by(4) {
        let w = word(head, at)?;
        if w & ADRP_MASK != ADRP {
            continue;
        }
        let imm = (((w >> 5) & 0x7_ffff) << 2) | ((w >> 29) & 3);
        let signed = (i64::from(imm) << 43) >> 43;
        let page = ((((dst as i64) + at as i64) >> 12) + signed) << 12;
        let add = word(head, at + 4)?;
        let rd = w & 31;
        if add & 0xffc0_0000 == 0x9100_0000 && (add >> 5) & 31 == rd && add & 31 == rd {
            let target = page as u64 + u64::from((add >> 10) & 0xfff);
            if (lo..hi).contains(&target) {
                found += 1;
            }
        }
    }
    Ok(found)
}

/// Find the hooked function, its callers, the OS's executable range and the padding in the OS section's decode,
/// and plan the thirteen words.
pub(super) fn locate_hook(reset: &[u8], inverted: &[u8], bias: usize, ram: &[u8]) -> Result<HookPlan> {
    let os = locate_os(reset, inverted, bias, ram)?;
    let head = extent(ram, os.dst as usize, os.head_bytes)?;
    // The function: found once, by its nine words.
    let found = find(head, &HOOK_FUNCTION);
    require(found.len() == 1, "The OS function that copies the Exif identity record is not found exactly once")?;
    let hook = os.dst + found[0] as u64;
    // Every use of it is a call that passes a pointer into the caller's own stack frame (`add x0, sp, #n`, n a multiple
    // of 8, so that the text at offset 0x30 is word aligned and writable): no tail jump, no address taken.
    let hits = references_into_at(head, os.dst as usize, hook as usize, hook as usize + 4, true);
    let mut callers = Vec::new();
    for (at, kind) in hits {
        if kind == "adrp page" {
            continue;
        }
        require(kind == "branch", "The hooked OS function is reached by something other than a call")?;
        let w = word(head, at)?;
        require(w & BL.1 == BL.0, "The hooked OS function is reached by a jump")?;
        let set = word(head, at.checked_sub(4).ok_or_else(|| image::Error::Invalid("A call of the hooked function has no predecessor".into()))?)?;
        require(
            set & 0xffc0_03ff == 0x9100_03e0 && ((set >> 10) & 0xfff) % 8 == 0,
            "A call of the hooked OS function does not pass a stack address",
        )?;
        callers.push(os.dst + at as u64);
    }
    require(
        hook_adrp_add_into(head, os.dst, hook, hook + 4)? == 0 && callers.len() >= 2,
        "The hooked OS function is not only called, or has fewer than two callers",
    )?;
    // The OS's own map of its image: [section, text_end) read, write and execute.
    let maps = find(head, &HOOK_TEXT_MAP);
    require(maps.len() == 1, "The OS's map of its code is not found exactly once")?;
    let page = |at: usize| -> Result<u64> {
        let adrp = word(head, at)?;
        let add = word(head, at + 4)?;
        let imm = (((adrp >> 5) & 0x7_ffff) << 2) | ((adrp >> 29) & 3);
        let signed = (i64::from(imm) << 43) >> 43;
        let page = ((((os.dst as i64) + at as i64) >> 12) + signed) << 12;
        Ok(page as u64 + u64::from((add >> 10) & 0xfff))
    };
    let end = page(maps[0] + 4)?;
    let start = page(maps[0] + 12)?;
    require(start == os.dst && end > start && end <= os.dst + os.head_bytes as u64, "The OS's code range is not what the plan needs")?;
    // The padding: runs of zero and nop words directly after a `ret`, inside the code range; the shortest that is
    // long enough (the first of equals) is the one.
    let words = head.len() / 4;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut n = 1;
    while n < words {
        let w = word(head, 4 * n)?;
        if (w == 0 || w == 0xd503_201f) && word(head, 4 * n - 4)? == 0xd65f_03c0 {
            let start = n;
            while n < words && matches!(word(head, 4 * n)?, 0 | 0xd503_201f) {
                n += 1;
            }
            if n - start >= HOOK_RUN_MIN_WORDS && os.dst + 4 * n as u64 <= end {
                runs.push((start, n));
            }
        } else {
            n += 1;
        }
    }
    runs.sort_by_key(|&(a, b)| (b - a, a));
    require(!runs.is_empty(), "There is no run of padding in the OS's code for the program")?;
    let (first, last) = runs[0];
    let run_lo = os.dst + 4 * first as u64;
    let run_hi = os.dst + 4 * last as u64;
    let cave = (run_lo + 4 + 7) & !7;
    require(cave + 4 * HOOK_CAVE_WORDS as u64 <= run_hi, "The padding run is too short for the program")?;
    require(
        references_into_at(head, os.dst as usize, run_lo as usize, run_hi as usize, true)
            .iter()
            .all(|&(_, kind)| kind == "adrp page")
            && hook_adrp_add_into(head, os.dst, run_lo, run_hi)? == 0,
        "Something refers to the padding the program would use",
    )?;
    require(hook < run_lo || hook >= run_hi, "The hooked function lies in the padding")?;
    let new = hook_program(hook, cave, HOOK_NEW_TEXT)?;
    let mut words_table = Vec::new();
    let at = |address: u64| (address - os.dst) as usize;
    words_table.push(ExifWord { address: hook, stock: word(head, at(hook))?, new: new[0] });
    for k in 0..HOOK_CAVE_WORDS {
        let address = cave + 4 * k as u64;
        let stock = word(head, at(address))?;
        require(stock == 0 || stock == 0xd503_201f, "A word of the program's padding is not zero or a nop")?;
        words_table.push(ExifWord { address, stock, new: new[1 + k] });
    }
    require(words_table[0].stock == HOOK_DISPLACED, "The hooked function does not start with the displaced instruction")?;
    Ok(HookPlan { os, hook, cave, text_end: end, callers, words: words_table })
}

/// The values of the hook stub's parameter block (`hook_stub.S` documents each).
pub(super) struct HookStubValues {
    pub own_address: u64,
    pub ret: u64,
    pub id: u64,
    pub func: u64,
    pub dst: u64,
    pub bytes: u64,
    pub green: u64,
    pub red: u64,
    pub pin: u64,
    pub pulse: u64,
    pub gap: u64,
    pub enable: u64,
    pub enable_green: u64,
    pub enable_red: u64,
    pub mux: u64,
    pub restore: u64,
    pub words: Vec<ExifWord>,
}

/// The stub image with its parameter block filled in. The template's block must be all zero and the
/// parameters must be where the layout says.
pub(super) fn fill_hook_stub(template: &[u8], values: &HookStubValues) -> Result<Vec<u8>> {
    use super::hook_stub_template as t;
    let params = t::XP5_HOOK_PARAMS;
    require(
        template.len() == t::XP5_HOOK_END
            && params + HOOK_STUB_PARAMS == template.len()
            && template[params..].iter().all(|b| *b == 0)
            && t::XP5_HOOK_P_TABLE + 16 * HOOK_TABLE == t::XP5_HOOK_OUT_STATUS
            && !values.words.is_empty()
            && values.words.len() <= HOOK_TABLE,
        "The hook stub template changed",
    )?;
    let mut image = template.to_vec();
    for (offset, value) in [
        (t::XP5_HOOK_P_SELF, values.own_address),
        (t::XP5_HOOK_P_RETURN, values.ret),
        (t::XP5_HOOK_P_ID, values.id),
        (t::XP5_HOOK_P_FUNC, values.func),
        (t::XP5_HOOK_P_DST, values.dst),
        (t::XP5_HOOK_P_BYTES, values.bytes),
        (t::XP5_HOOK_P_GREEN, values.green),
        (t::XP5_HOOK_P_RED, values.red),
        (t::XP5_HOOK_P_PIN, values.pin),
        (t::XP5_HOOK_P_PULSE, values.pulse),
        (t::XP5_HOOK_P_GAP, values.gap),
        (t::XP5_HOOK_P_ENABLE, values.enable),
        (t::XP5_HOOK_P_ENABLE_GREEN, values.enable_green),
        (t::XP5_HOOK_P_ENABLE_RED, values.enable_red),
        (t::XP5_HOOK_P_MUX, values.mux),
        (t::XP5_HOOK_P_RESTORE, values.restore),
        (t::XP5_HOOK_P_COUNT, values.words.len() as u64),
    ] {
        require(offset >= params && offset + 8 <= image.len(), "A hook stub parameter is outside its block")?;
        put64(&mut image, offset, value)?;
    }
    for (n, w) in values.words.iter().enumerate() {
        let at = t::XP5_HOOK_P_TABLE + 16 * n;
        put64(&mut image, at, w.address)?;
        put64(&mut image, at + 8, u64::from(w.stock) | (u64::from(w.new) << 32))?;
    }
    Ok(image)
}

/// The bindings of a hook loader: the redirect words (as the Exif build's), the named-word binding
/// (`setup_sd_hook`, 53 words) and the stub.
pub(super) struct HookBinding {
    pub stub: StubBinding,
    pub hook: Vec<u64>,
}

/// CRC-32 of the table's stock words and of its new words, little-endian in table order.
fn hook_crcs(words: &[ExifWord]) -> (u32, u32) {
    let stock: Vec<u8> = words.iter().flat_map(|w| w.stock.to_le_bytes()).collect();
    let new: Vec<u8> = words.iter().flat_map(|w| w.new.to_le_bytes()).collect();
    (crc32(&stock), crc32(&new))
}

/// Plan the hook loader's bindings (see `exif_binding`, which this follows).
pub(super) fn hook_binding(
    plan: &HookPlan, file: &[u8], reset: &[u8], inverted: &[u8], bias: usize, aperture: u64, destination: u64,
    guard_bytes: usize, apply: bool, lamp: &StubLamp, template: &[u8],
) -> Result<HookBinding> {
    let os = &plan.os;
    require(
        !file.is_empty()
            && file.len() <= 0x1_0000
            && destination % 0x1000 == 0
            && (READ_ONLY_FLOOR..READ_ONLY_CEILING).contains(&destination)
            && destination + file.len() as u64 <= READ_ONLY_CEILING,
        "The placeholder file does not fit the read-only destination",
    )?;
    require(
        os.call_old & BL.1 == BL.0
            && os.call_site % 4 == 0
            && os.call_site >= 4
            && os.call_site + 4 <= guard_bytes
            && os.load_fn % 4 == 0
            && os.load_fn + 4 <= guard_bytes,
        "The reset stage's OS load call is not a bl inside the RAM-0 guard",
    )?;
    require(bl_target(reset, os.call_site)? == os.load_fn, "The OS load call does not call the load function")?;
    require(
        word(reset, os.call_site - 4)? == OS_SET_ID && word(reset, os.load_fn)? == OS_LOAD[0].0,
        "The reset stage's OS call does not pass the OS id to a function that starts as the load does",
    )?;
    // The table is exactly the program: the words, their addresses and their order are recomputed here.
    let program = hook_program(plan.hook, plan.cave, HOOK_NEW_TEXT)?;
    require(
        plan.words.len() == HOOK_NAMED
            && plan.words.iter().enumerate().all(|(n, w)| {
                w.new == program[n]
                    && w.address == if n == 0 { plan.hook } else { plan.cave + 4 * (n as u64 - 1) }
                    && (if n == 0 { w.stock == HOOK_DISPLACED } else { w.stock == 0 || w.stock == 0xd503_201f })
            })
            && plan.hook % 4 == 0
            && plan.cave % 8 == 0
            && plan.hook >= os.dst
            && plan.hook + 4 <= os.dst + os.head_bytes as u64
            && plan.cave >= os.dst
            && plan.cave + 4 * HOOK_CAVE_WORDS as u64 <= plan.text_end
            && plan.text_end <= os.dst + os.head_bytes as u64
            && (plan.hook + 4 <= plan.cave || plan.cave + 4 * HOOK_CAVE_WORDS as u64 <= plan.hook),
        "The named words are not the hook program in the OS's code",
    )?;
    let offset = sled_within(extent(reset, 0, guard_bytes)?, template.len(), HOOK_STUB_MAX)?;
    require(
        offset + template.len() + SLED_MARGIN <= guard_bytes && !(offset..offset + template.len()).contains(&os.call_site),
        "The hook stub overlaps the call or leaves the RAM-0 guard",
    )?;
    let stock = extent(reset, 0, guard_bytes)?;
    require(
        lamp.pin < 8
            && lamp.green_port < 16
            && lamp.red_port < 16
            && lamp.green_port != lamp.red_port
            && lamp.green_port != 3
            && lamp.red_port != 3,
        "The hook stub's lamp is not a usable pair of ports",
    )?;
    let comp = extent(inverted, os.flash_offset as usize + bias, os.comp_bytes)?;
    let image = fill_hook_stub(
        template,
        &HookStubValues {
            own_address: offset as u64,
            ret: os.call_site as u64 + 4,
            id: u64::from(OS_SECTION_ID),
            func: os.load_fn as u64,
            dst: os.dst,
            bytes: os.head_bytes as u64,
            green: lamp.pio_base + 64 * u64::from(lamp.green_port),
            red: lamp.pio_base + 64 * u64::from(lamp.red_port),
            pin: 1u64 << lamp.pin,
            pulse: OS_PULSE_LOOPS,
            gap: OS_GAP_LOOPS,
            enable: lamp.pio_base + 0x500,
            enable_green: (1u64 << (lamp.green_port + 16)) | (1u64 << lamp.green_port),
            enable_red: (1u64 << (lamp.red_port + 16)) | (1u64 << lamp.red_port),
            mux: 3u64 << (2 * lamp.pin + 16),
            restore: (1u64 << (lamp.pin + 16)) | (1u64 << lamp.pin),
            words: plan.words.clone(),
        },
    )?;
    let new = bl_word(os.call_site, offset)?;
    require(new != os.call_old, "The OS call word is the stock word")?;
    let mut patched = stock.to_vec();
    patched[offset..offset + image.len()].copy_from_slice(&image);
    patched[os.call_site..os.call_site + 4].copy_from_slice(&new.to_le_bytes());
    let mut words = [0u64; 16];
    words[0] = 1;
    words[1] = u64::from(apply);
    words[2] = os.call_site as u64;
    words[3] = u64::from(os.call_old);
    words[4] = u64::from(new);
    words[5] = u64::from(crc32(stock));
    words[6] = u64::from(crc32(&patched));
    words[7] = 0; // the file is at offset 0 of the copy
    words[8] = file.len() as u64;
    words[9] = aperture + os.flash_offset;
    words[10] = u64::from(crc32(file));
    words[11] = os.table_bytes as u64;
    for (n, &(_, flash)) in os.tables.iter().enumerate() {
        let table = extent(inverted, flash as usize + bias, os.table_bytes)?;
        words[12 + n] = aperture + flash;
        words[14 + n] = u64::from(crc32(table));
    }
    let (stock_crc, new_crc) = hook_crcs(&plan.words);
    let mut hook = vec![0u64; HOOK_BINDING / 8];
    for (n, v) in [
        1,
        aperture,
        os.comp_bytes as u64,
        u64::from(crc32(comp)),
        os.tables[0].0,
        u64::from(OS_SECTION_ID),
        os.dst,
        os.head_bytes as u64,
        plan.hook,
        plan.cave,
        plan.words.len() as u64,
        u64::from(stock_crc),
        u64::from(new_crc),
    ]
    .into_iter()
    .enumerate()
    {
        hook[n] = v;
    }
    for (n, w) in plan.words.iter().enumerate() {
        hook[13 + 2 * n] = w.address;
        hook[14 + 2 * n] = u64::from(w.stock) | (u64::from(w.new) << 32);
    }
    Ok(HookBinding {
        stub: StubBinding {
            redirect: Binding(words),
            offset,
            params_offset: super::hook_stub_template::XP5_HOOK_PARAMS,
            crc32: crc32(&image),
            image,
        },
        hook,
    })
}

/// Write the hook binding into the loader at `at` after checking that the template is untouched (`abi` 1,
/// everything else zero: the struct is 424 bytes).
pub(super) fn bind_hook(program: &mut [u8], at: usize, hook: &[u64]) -> Result<()> {
    let mut template = vec![0u8; HOOK_BINDING];
    template[..8].copy_from_slice(&1u64.to_le_bytes());
    require(hook.len() * 8 == HOOK_BINDING, "The hook binding has the wrong size")?;
    require(extent(program, at, HOOK_BINDING)? == template, "XP5 hook binding template changed")?;
    for (n, value) in hook.iter().enumerate() {
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
        // The helper before the padding ends in `ret`; the padding is nops up to the first vector
        // table (a branch at the next 0x800 boundary).
        put(&mut reset, 0x1c0, RET).unwrap();
        for at in (0x1c4..0x800).step_by(4) {
            put(&mut reset, at, NOP).unwrap();
        }
        put(&mut reset, 0x800, 0x1400_0000).unwrap();
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

    // ---- the copy stub ----

    use crate::xp5::stub_template as stub;
    const GUARD: usize = 0xb000;
    fn lamp() -> StubLamp {
        StubLamp { pio_base: 0xff90_7000, green_port: 8, red_port: 6, pin: 2 }
    }
    fn stub_plan() -> (Plan, Vec<u8>, Vec<u8>) {
        let (reset, update, ram) = fixture();
        (locate(&reset, &update, BIAS, &ram).unwrap(), reset, update)
    }

    #[test]
    fn the_plan_names_the_copy_call_and_the_copy_function_the_table_copies_share() {
        let (plan, reset, _) = stub_plan();
        assert_eq!((plan.copy_site, plan.copy_fn), (FUNCTION + 52, COPY));
        assert_eq!(plan.copy_old, bl(FUNCTION + 52, COPY));
        assert_eq!(word(&reset, plan.copy_site).unwrap(), plan.copy_old);
    }

    #[test]
    fn a_table_copy_that_calls_another_copy_function_is_refused() {
        let (reset, update, ram) = fixture();
        let mut other = reset.clone();
        put(&mut other, MAIN_COPY + 44, bl(MAIN_COPY + 44, COPY + 0x40)).unwrap();
        assert!(locate(&other, &update, BIAS, &ram).is_err());
    }

    #[test]
    fn the_stub_goes_into_the_padding_before_the_vector_tables() {
        let (reset, _, _) = fixture();
        // The padding is 0x1c4..0x800; the stub starts on the next 0x40 boundary with room after it.
        assert_eq!(sled(&reset, 872).unwrap(), 0x200);
        assert_eq!(sled(&reset, 0x400 - 0x40).unwrap(), 0x200);
        for bad in [0, 2, 1028, 0x7c0 - 0x200 + 4, 2048] {
            assert!(sled(&reset, bad).is_err(), "{bad}");
        }
        // Nothing in the fixture's stage refers to the padding.
        assert!(references_into(&reset, 0x1c4, 0x800).is_empty());
        // A word that is not a nop inside the run shortens it so that the stub no longer fits.
        let mut broken = reset.clone();
        put(&mut broken, 0x2c0, 0xaa1f_03e1).unwrap();
        assert!(sled(&broken, 872).is_err());
        // Without the `ret` before it, or not ending at a vector table, the run is not the padding.
        let mut no_ret = reset.clone();
        put(&mut no_ret, 0x1c0, NOP).unwrap();
        assert!(sled(&no_ret, 872).is_err());
        let mut short = reset.clone();
        put(&mut short, 0x7fc, 0xaa1f_03e1).unwrap();
        assert!(sled(&short, 872).is_err());
    }

    #[test]
    fn a_stage_that_refers_to_its_padding_is_refused() {
        let (reset, _, _) = fixture();
        let refers = |put_it: &dyn Fn(&mut Vec<u8>)| {
            let mut image = reset.clone();
            put_it(&mut image);
            sled(&image, 872).is_err()
        };
        // A branch, a conditional branch, a compare branch, a test branch, an adr, a literal load, an
        // adrp whose page holds the padding, and a 64-bit pointer: each into the padding.
        assert!(refers(&|i| put(i, 0x3000, bl(0x3000, 0x400)).unwrap()));
        assert!(refers(&|i| put(i, 0x3000, 0x1400_0000 | (((0x300i64 - 0x3000) / 4) as u32 & 0x03ff_ffff)).unwrap()));
        assert!(refers(&|i| put(i, 0x3000, 0x5400_0000 | ((((0x300i64 - 0x3000) / 4) as u32 & 0x7_ffff) << 5)).unwrap()));
        assert!(refers(&|i| put(i, 0x3000, 0xb400_0000 | ((((0x300i64 - 0x3000) / 4) as u32 & 0x7_ffff) << 5)).unwrap()));
        assert!(refers(&|i| put(i, 0x3000, 0xb600_0000 | ((((0x300i64 - 0x3000) / 4) as u32 & 0x3fff) << 5)).unwrap()));
        let adr = |at: usize, target: usize| {
            let imm = (target as i64 - at as i64) as u32 & 0x1f_ffff;
            0x1000_0000 | ((imm & 3) << 29) | ((imm >> 2) << 5)
        };
        assert!(refers(&|i| put(i, 0x3000, adr(0x3000, 0x300)).unwrap()));
        assert!(refers(&|i| put(i, 0x3000, 0x5800_0000 | ((((0x300i64 - 0x3000) / 4) as u32 & 0x7_ffff) << 5)).unwrap()));
        assert!(refers(&|i| put(i, 0x3000, adrp(0x3000, 0, 0x0)).unwrap()));
        assert!(refers(&|i| i[0x3008..0x3010].copy_from_slice(&0x400u64.to_le_bytes())));
        // Harmless look-alikes: small 32-bit constants, a branch just outside, an adrp to another page.
        let mut fine = reset.clone();
        put(&mut fine, 0x3000, 0x0000_0600).unwrap();
        put(&mut fine, 0x3004, bl(0x3004, 0x1c0)).unwrap();
        put(&mut fine, 0x3008, bl(0x3008, 0x800)).unwrap();
        put(&mut fine, 0x300c, adrp(0x300c, 0, 0x1000)).unwrap();
        fine[0x3010..0x3018].copy_from_slice(&0x1c0u64.to_le_bytes());
        fine[0x3018..0x3020].copy_from_slice(&0x800u64.to_le_bytes());
        assert_eq!(sled(&fine, 872).unwrap(), 0x200);
    }

    #[test]
    fn the_call_word_is_a_bl_to_the_stub() {
        assert_eq!(bl_word(0x2d04, 0x200).unwrap(), 0x9400_0000 | ((-(0x2d04i64 - 0x200) / 4) as u32 & 0x03ff_ffff));
        assert_eq!(bl_word(0x2d04, 0x2d08).unwrap(), 0x9400_0001);
        assert!(bl_word(0x2d04, 0x2d05).is_err());
        assert!(bl_word(0x2d04, 0x1000_0000).is_err());
    }

    #[test]
    fn the_stub_binding_carries_the_call_word_the_stub_bytes_and_every_crc() {
        let (plan, reset, update) = stub_plan();
        let file = 0x8_1000 + SECTION_BYTES;
        let b = stub_binding(&plan, &reset, &update, BIAS, 0xf000_0000, 0x3000_0000, GUARD, file, true, &lamp(), stub::IMAGE)
            .unwrap();
        let w = b.redirect.0;
        let new = bl_word(FUNCTION + 52, 0x200).unwrap();
        assert_eq!((w[0], w[1], w[2], w[3], w[4]), (1, 1, (FUNCTION + 52) as u64, u64::from(bl(FUNCTION + 52, COPY)), u64::from(new)));
        assert_eq!((w[7], w[8], w[9]), (SECTION_DST, SECTION_BYTES as u64, 0xf000_0000 + SECTION_FLASH as u64));
        let section = &update[SECTION_FLASH + BIAS..SECTION_FLASH + BIAS + SECTION_BYTES];
        assert_eq!(w[10], u64::from(crc32(section)));
        assert_eq!((w[11], w[12], w[13]), (0x800, 0xf064_0000, 0xf012_0000));
        // The stub: the asset with its parameter block filled in, nothing else changed.
        assert_eq!(b.offset, 0x200);
        assert_eq!(b.image.len(), stub::IMAGE.len());
        let params = stub::XP5_STUB_PARAMS;
        assert_eq!(b.image[..params], stub::IMAGE[..params]);
        let q = |offset: usize| u64::from_le_bytes(b.image[offset..offset + 8].try_into().unwrap());
        assert_eq!(q(stub::XP5_STUB_P_SELF), 0x200);
        assert_eq!((q(stub::XP5_STUB_P_DST), q(stub::XP5_STUB_P_FLASH), q(stub::XP5_STUB_P_BYTES)),
            (SECTION_DST, 0xf000_0000 + SECTION_FLASH as u64, SECTION_BYTES as u64));
        assert_eq!(q(stub::XP5_STUB_P_DRAM), 0x3000_0000 + SECTION_DST);
        assert_eq!(q(stub::XP5_STUB_P_CRC), u64::from(crc32(section)));
        assert_eq!(q(stub::XP5_STUB_P_COPY), COPY as u64);
        assert_eq!((q(stub::XP5_STUB_P_GREEN), q(stub::XP5_STUB_P_RED), q(stub::XP5_STUB_P_PIN)), (0xff90_7200, 0xff90_7180, 4));
        assert_eq!((q(stub::XP5_STUB_P_PULSE), q(stub::XP5_STUB_P_GAP)), (STUB_PULSE_LOOPS, STUB_GAP_LOOPS));
        assert_eq!(
            (q(stub::XP5_STUB_P_ENABLE), q(stub::XP5_STUB_P_ENABLE_GREEN), q(stub::XP5_STUB_P_ENABLE_RED), q(stub::XP5_STUB_P_MUX)),
            (0xff90_7500, 0x0100_0100, 0x0040_0040, 0x0030_0000)
        );
        assert_eq!((q(stub::XP5_STUB_P_RETURN), q(stub::XP5_STUB_P_RESTORE)), ((FUNCTION + 56) as u64, 0x0004_0004));
        assert_eq!(b.params_offset, stub::XP5_STUB_PARAMS);
        assert!(b.image[stub::XP5_STUB_OUT_STATUS..].iter().all(|v| *v == 0), "outputs start at zero");
        assert_eq!(b.crc32, crc32(&b.image));
        // The patched image: the stock image with the stub in the padding and the call word replaced.
        let mut patched = reset[..GUARD].to_vec();
        patched[0x200..0x200 + b.image.len()].copy_from_slice(&b.image);
        patched[FUNCTION + 52..FUNCTION + 56].copy_from_slice(&new.to_le_bytes());
        assert_eq!((w[5], w[6]), (u64::from(crc32(&reset[..GUARD])), u64::from(crc32(&patched))));
        assert_ne!(w[5], w[6]);
        // Only those bytes differ from the stock image.
        let changed: Vec<usize> = (0..GUARD).filter(|&n| patched[n] != reset[n]).collect();
        assert!(changed.iter().all(|n| (0x200..0x200 + b.image.len()).contains(n) || (FUNCTION + 52..FUNCTION + 56).contains(n)));
        // Verify-only is the same binding with apply 0.
        let v = stub_binding(&plan, &reset, &update, BIAS, 0xf000_0000, 0x3000_0000, GUARD, file, false, &lamp(), stub::IMAGE)
            .unwrap();
        assert_eq!((v.redirect.0[1], &v.redirect.0[2..]), (0, &w[2..]));
        assert_eq!((v.offset, v.image, v.crc32), (b.offset, b.image, b.crc32));
    }

    #[test]
    fn the_stub_binding_refuses_what_does_not_fit() {
        let (plan, reset, update) = stub_plan();
        let file = 0x8_1000 + SECTION_BYTES;
        let make = |destination: u64, guard: usize, file: usize, template: &[u8]| {
            stub_binding(&plan, &reset, &update, BIAS, 0xf000_0000, destination, guard, file, true, &lamp(), template)
        };
        assert!(make(0x3000_0000, GUARD, file, stub::IMAGE).is_ok());
        for bad in [0x2000_0000, 0x3000_0004, READ_ONLY_CEILING, 0x2445_9000 - 0x1000] {
            assert!(make(bad, GUARD, file, stub::IMAGE).is_err(), "{bad:#x}");
        }
        assert!(make(0x3000_0000, GUARD, file - 4, stub::IMAGE).is_err(), "the file ends inside the section");
        assert!(make(0x3000_0000, 0x2d00, file, stub::IMAGE).is_err(), "the guard ends before the call");
        // A stub template that is not the asset (a changed parameter block or size).
        let mut dirty = stub::IMAGE.to_vec();
        dirty[stub::XP5_STUB_P_DST] = 1;
        assert!(make(0x3000_0000, GUARD, file, &dirty).is_err());
        assert!(make(0x3000_0000, GUARD, file, &stub::IMAGE[8..]).is_err());
        // A copy call that is not a bl.
        let mut plan2 = plan;
        plan2.copy_old = 0xaa1f_03e1;
        assert!(stub_binding(&plan2, &reset, &update, BIAS, 0xf000_0000, 0x3000_0000, GUARD, file, true, &lamp(), stub::IMAGE).is_err());
    }

    // ---- the marker ----

    use crate::xp5::marker_image as marker;
    const MARKER_DECLARED: usize = 0x1800;

    /// A synthetic section 0: a header (address, declared size, entry point), code, a literal, zero padding up to a
    /// 0x800 boundary (the first vector table follows), more code and the zero tail.
    pub(in crate::xp5) fn marker_section() -> Vec<u8> {
        let mut s = vec![0u8; SECTION_BYTES];
        s[8..16].copy_from_slice(&SECTION_DST.to_le_bytes());
        s[16..24].copy_from_slice(&(MARKER_DECLARED as u64).to_le_bytes());
        s[24..32].copy_from_slice(&(SECTION_DST + 0x40).to_le_bytes());
        for at in (0x20..MARKER_DECLARED).step_by(4) {
            put(&mut s, at, 0xd280_0001 | ((at as u32 / 4) & 0xff) << 5).unwrap();
        }
        put(&mut s, 0x40, 0xaa00_03f4).unwrap();               // mov x20, x0: the entry instruction
        for at in (0x5fc..0x800).step_by(4) {
            put(&mut s, at, 0).unwrap();                       // the zero padding, up to the vector table at 0x800
        }
        put(&mut s, 0x5f8, 0x000a_03d8).unwrap();              // the last literal
        s
    }

    /// `stage`, with the marker section instead of the patterned one, and both tables declaring its size.
    pub(in crate::xp5) fn marker_stage(bias: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (reset, mut update, mut ram) = stage(bias);
        let section = marker_section();
        update[SECTION_FLASH + bias..SECTION_FLASH + bias + SECTION_BYTES].copy_from_slice(&section);
        ram[0x8_1000..].copy_from_slice(&section);
        for (flash, ram_at) in [(0x64_0000usize, Some(0x4_0000usize)), (0x12_0000, None)] {
            let row = flash + bias + 0x50;
            update[row + 16..row + 24].copy_from_slice(&(MARKER_DECLARED as u64).to_le_bytes());
            update[row + 24..row + 32].copy_from_slice(&(SECTION_DST + MARKER_DECLARED as u64).to_le_bytes());
            if let Some(at) = ram_at {
                ram[at + 0x50 + 16..at + 0x50 + 24].copy_from_slice(&(MARKER_DECLARED as u64).to_le_bytes());
                ram[at + 0x50 + 24..at + 0x50 + 32].copy_from_slice(&(SECTION_DST + MARKER_DECLARED as u64).to_le_bytes());
            }
        }
        (reset, update, ram)
    }

    fn marker_values() -> MarkerValues {
        MarkerValues::for_lamp(&lamp())
    }
    fn marker_plan_of(section: &[u8]) -> Result<MarkerPlan> {
        let (reset, update, ram) = marker_stage(BIAS);
        let plan = locate(&reset, &update, BIAS, &ram).unwrap();
        let declared = declared_size(&plan, &update, BIAS).unwrap();
        marker_plan(&plan, section, declared, marker::IMAGE, &marker_values())
    }

    #[test]
    fn the_sections_declared_size_comes_from_both_tables() {
        let (reset, update, ram) = marker_stage(BIAS);
        let plan = locate(&reset, &update, BIAS, &ram).unwrap();
        assert_eq!(declared_size(&plan, &update, BIAS).unwrap(), MARKER_DECLARED);
        // The tables disagree, or the zero-filled part does not follow the declared size.
        let mut other = update.clone();
        other[0x12_0000 + BIAS + 0x50 + 16] ^= 0x10;
        assert!(declared_size(&plan, &other, BIAS).is_err());
        let mut other = update.clone();
        other[0x64_0000 + BIAS + 0x50 + 24] ^= 0x10;
        assert!(declared_size(&plan, &other, BIAS).is_err());
        // The fixture without declared sizes (the copy stub's) has none.
        let (reset, plain, ram) = fixture();
        let plan = locate(&reset, &plain, BIAS, &ram).unwrap();
        assert!(declared_size(&plan, &plain, BIAS).is_err());
    }

    #[test]
    fn the_marker_hooks_the_entry_and_puts_its_program_in_the_zero_padding() {
        let section = marker_section();
        let m = marker_plan_of(&section).unwrap();
        // The hook is the entry point, its stock word the instruction the program starts with.
        assert_eq!((m.hook, m.cave, m.code_bytes), (0x40, 0x600, MARKER_DECLARED));
        assert_eq!(m.words[0], (0x40, 0xaa00_03f4, b_word(0x40, 0x600).unwrap()));
        assert_eq!(m.params_offset, 0x600 + marker::XP5_MARKER_PARAMS);
        // Every other changed word is the program: stock zero, new word the program's, ascending, and inside the padding.
        assert!(m.words.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(m.words[1..].iter().all(|&(offset, stock, new)| stock == 0 && new != 0 && (0x600..0x800).contains(&(offset as usize))));
        assert_eq!(m.words[1].2, 0xaa00_03f4);                  // the displaced instruction, first in the program
        // The program returns to the instruction after the hook.
        let ret = marker::XP5_MARKER_RETURN;
        let at = 0x600 + ret;
        assert_eq!(m.words.iter().find(|w| w.0 as usize == at).unwrap().2, b_word(at, 0x44).unwrap());
        // The parameter block carries the lamp, and the phase length.
        let word64 = |offset: usize| {
            let lo = m.words.iter().find(|w| w.0 as usize == m.params_offset + offset).map_or(0, |w| u64::from(w.2));
            let hi = m.words.iter().find(|w| w.0 as usize == m.params_offset + offset + 4).map_or(0, |w| u64::from(w.2));
            lo | hi << 32
        };
        assert_eq!(word64(marker::XP5_MARKER_P_GREEN - marker::XP5_MARKER_PARAMS), 0xff90_7200);
        assert_eq!(word64(marker::XP5_MARKER_P_RED - marker::XP5_MARKER_PARAMS), 0xff90_7180);
        assert_eq!(word64(marker::XP5_MARKER_P_PIN - marker::XP5_MARKER_PARAMS), 4);
        assert_eq!(word64(marker::XP5_MARKER_P_PHASE - marker::XP5_MARKER_PARAMS), MARKER_PHASE_LOOPS);
        // The changed section differs from the stock one in exactly those words, and not at all in the zero tail.
        let mut modified = section.clone();
        for &(offset, _, new) in &m.words {
            modified[offset as usize..offset as usize + 4].copy_from_slice(&new.to_le_bytes());
        }
        assert_eq!((m.stock_crc32, m.modified_crc32), (crc32(&section), crc32(&modified)));
        assert_ne!(m.stock_crc32, m.modified_crc32);
        let changed: Vec<usize> = (0..section.len()).step_by(4).filter(|&n| section[n..n + 4] != modified[n..n + 4]).collect();
        assert_eq!(changed, m.words.iter().map(|w| w.0 as usize).collect::<Vec<_>>());
        assert!(changed.iter().all(|&n| n + 4 <= MARKER_DECLARED));
        assert!(m.words.len() <= MARKER_MAX);
    }

    #[test]
    fn the_marker_is_refused_where_its_place_or_its_hook_is_not_what_it_proves() {
        let section = marker_section();
        let refused = |change: &dyn Fn(&mut Vec<u8>)| {
            let mut other = section.clone();
            change(&mut other);
            marker_plan_of(&other).is_err()
        };
        assert!(marker_plan_of(&section).is_ok());
        // The entry instruction is not the one the program displaces; the entry point is outside the code or in the header.
        assert!(refused(&|s| put(s, 0x40, 0xaa00_03f5).unwrap()));
        assert!(refused(&|s| s[24..32].copy_from_slice(&(SECTION_DST + 0x1800).to_le_bytes())));
        assert!(refused(&|s| s[24..32].copy_from_slice(&(SECTION_DST + 0x10).to_le_bytes())));
        assert!(refused(&|s| s[24..32].copy_from_slice(&(SECTION_DST + 0x42).to_le_bytes())));
        // The header names another address or declared size.
        assert!(refused(&|s| s[8..16].copy_from_slice(&0x8_2000u64.to_le_bytes())));
        assert!(refused(&|s| s[16..24].copy_from_slice(&0x1000u64.to_le_bytes())));
        // The padding is not zero all through (the run shrinks below the minimum), or does not end on a vector boundary,
        // or there are two such runs.
        assert!(refused(&|s| put(s, 0x700, 0xd503_201f).unwrap()));
        assert!(refused(&|s| put(s, 0x7fc, 0xd503_201f).unwrap()));
        assert!(refused(&|s| put(s, 0x800, 0).unwrap()));
        assert!(refused(&|s| {
            for at in (0x1000..0x1800).step_by(4) {
                put(s, at, 0).unwrap();
            }
        }));
        // A reference into the padding, of each kind: branch, literal load, adr, adrp page, 64-bit and 32-bit values.
        let adr = |at: usize, target: usize| {
            let imm = (target as i64 - at as i64) as u32 & 0x1f_ffff;
            0x1000_0000 | ((imm & 3) << 29) | ((imm >> 2) << 5)
        };
        assert!(refused(&|s| put(s, 0x100, 0x1400_0000 | (((0x700i64 - 0x100) / 4) as u32 & 0x03ff_ffff)).unwrap()));
        assert!(refused(&|s| put(s, 0x100, 0x5800_0000 | ((((0x700i64 - 0x100) / 4) as u32 & 0x7_ffff) << 5)).unwrap()));
        assert!(refused(&|s| put(s, 0x100, adr(0x100, 0x700)).unwrap()));
        assert!(refused(&|s| s[0x108..0x110].copy_from_slice(&0x8_1700u64.to_le_bytes())));
        assert!(refused(&|s| put(s, 0x100, 0x8_1700).unwrap()));
        // Look-alikes that are fine: a branch just in front of and behind the padding, a small constant.
        let mut fine = section.clone();
        put(&mut fine, 0x100, 0x1400_0000 | (((0x5f8i64 - 0x100) / 4) as u32 & 0x03ff_ffff)).unwrap();
        put(&mut fine, 0x104, 0x1400_0000 | (((0x800i64 - 0x104) / 4) as u32 & 0x03ff_ffff)).unwrap();
        put(&mut fine, 0x108, 0x600).unwrap();
        assert!(marker_plan_of(&fine).is_ok());
    }

    #[test]
    fn the_marker_binding_is_the_template_with_the_names_written_in() {
        let m = marker_plan_of(&marker_section()).unwrap();
        let mut program = vec![0u8; 0x100 + MARKER_BINDING + 0x100];
        program[0x100..0x108].copy_from_slice(&1u64.to_le_bytes());
        bind_marker(&mut program, 0x100, &m).unwrap();
        let q = |at: usize| u64::from_le_bytes(program[0x100 + at..0x100 + at + 8].try_into().unwrap());
        assert_eq!((q(0), q(8), q(16), q(24), q(32)), (1, u64::from(m.modified_crc32), MARKER_DECLARED as u64, m.words.len() as u64, m.params_offset as u64));
        for (n, &(offset, stock, new)) in m.words.iter().enumerate() {
            let at = 0x100 + 40 + 12 * n;
            let w = |k: usize| u32::from_le_bytes(program[at + 4 * k..at + 4 * k + 4].try_into().unwrap());
            assert_eq!((w(0), w(1), w(2)), (offset, stock, new));
        }
        // Everything after the last named word stays zero (the other names and the four outputs).
        assert!(program[0x100 + 40 + 12 * m.words.len()..].iter().all(|b| *b == 0));
        // A template that was touched is refused.
        let mut touched = program.clone();
        touched[0x100 + 8] = 1;
        assert!(bind_marker(&mut touched, 0x100, &m).is_err());
        let mut none = MarkerPlan { words: Vec::new(), ..m.clone() };
        let mut fresh = vec![0u8; 0x100 + MARKER_BINDING];
        fresh[0x100..0x108].copy_from_slice(&1u64.to_le_bytes());
        assert!(bind_marker(&mut fresh, 0x100, &none).is_err());
        none.words = vec![(0, 0, 1); MARKER_MAX + 1];
        assert!(bind_marker(&mut fresh, 0x100, &none).is_err());
    }

    #[test]
    fn the_marker_asset_is_a_position_independent_template() {
        let image = marker::IMAGE;
        assert_eq!((image.len() % 8, marker::XP5_MARKER_ENTRY, marker::XP5_MARKER_END), (0, 0, image.len()));
        assert_eq!(marker::XP5_MARKER_PARAMS + MARKER_PARAMS, image.len());
        assert!(image[marker::XP5_MARKER_PARAMS..].iter().all(|b| *b == 0));
        // The first instruction is the displaced `mov x20, x0`; the return is a placeholder `b .`.
        assert_eq!(word(image, 0).unwrap(), 0xaa00_03f4);
        assert_eq!(word(image, marker::XP5_MARKER_RETURN).unwrap(), 0x1400_0000);
        // The parameter offsets are the block's nine words, each once.
        let mut offsets = vec![
            marker::XP5_MARKER_P_GREEN, marker::XP5_MARKER_P_RED, marker::XP5_MARKER_P_PIN, marker::XP5_MARKER_P_ENABLE,
            marker::XP5_MARKER_P_ENABLE_GREEN, marker::XP5_MARKER_P_ENABLE_RED, marker::XP5_MARKER_P_MUX,
            marker::XP5_MARKER_P_RESTORE, marker::XP5_MARKER_P_PHASE,
        ];
        offsets.sort_unstable();
        assert!(offsets.windows(2).all(|w| w[1] == w[0] + 8));
        assert_eq!(offsets[0], marker::XP5_MARKER_PARAMS);
    }

    // ---- The OS section ----

    const OS_FUNCTION: usize = 0x2d50;
    const OS_LOOKUP: usize = 0x2f0c;
    pub(in crate::xp5) const OS_DST: u64 = 0x4_2000;
    pub(in crate::xp5) const OS_HEAD: usize = 0x2000;
    const OS_FLASH: usize = 0x20_0000;

    /// A compressed stream of the codec's own format that holds `data` as literals only.
    fn literal_stream(data: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        for chunk in data.chunks(127) {
            body.push(chunk.len() as u8);
            body.extend_from_slice(chunk);
        }
        let packed = 20 + body.len();
        let mut stream = Vec::new();
        for value in [data.len() as u32, packed as u32, packed.div_ceil(0x4000) as u32, 0x4000, 4] {
            stream.extend_from_slice(&value.to_le_bytes());
        }
        stream.extend_from_slice(&body);
        stream.resize(stream.len().div_ceil(4) * 4, 0);
        stream
    }
    fn os_table(ram: u64, rows: &[(u32, u64, u64, u64, u32, u32, u32)]) -> Vec<u8> {
        let mut table = vec![0u8; 0x800];
        table[..8].copy_from_slice(&(ram + 0x50).to_le_bytes());
        for (n, &(id, dst, declared, bss, flash, size, flag)) in rows.iter().enumerate() {
            let at = 0x50 + n * ROW;
            table[at..at + 4].copy_from_slice(&id.to_le_bytes());
            table[at + 8..at + 16].copy_from_slice(&dst.to_le_bytes());
            table[at + 16..at + 24].copy_from_slice(&declared.to_le_bytes());
            table[at + 24..at + 32].copy_from_slice(&bss.to_le_bytes());
            table[at + 40..at + 44].copy_from_slice(&flash.to_le_bytes());
            table[at + 48..at + 52].copy_from_slice(&size.to_le_bytes());
            table[at + 56..at + 60].copy_from_slice(&flag.to_le_bytes());
        }
        let end = 0x50 + rows.len() * ROW;
        table[end..end + 4].copy_from_slice(&END.to_le_bytes());
        table
    }
    /// A synthetic stock reset image, update and RAM image around the OS load function's words: the
    /// function, its three callers (ids 1, 2 and 6), the two table copies, a compressed OS section.
    pub(in crate::xp5) fn os_stage(bias: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        os_stage_with(bias, &|_| {})
    }
    /// `os_stage` with a change to the decoded section's initialized bytes (before they are compressed).
    pub(in crate::xp5) fn os_stage_with(bias: usize, edit: &dyn Fn(&mut Vec<u8>)) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut reset = vec![0u8; 0x1_4000];
        put(&mut reset, 0x1c0, RET).unwrap();
        for at in (0x1c4..0x800).step_by(4) {
            put(&mut reset, at, NOP).unwrap();
        }
        put(&mut reset, 0x800, 0x1400_0000).unwrap();
        put_pattern(&mut reset, OS_FUNCTION, &OS_LOAD);
        for (word, target) in [
            (OS_LOOKUP_WORD, OS_LOOKUP),
            (OS_RAW_COPY_WORD, COPY),
            (OS_COMPRESSED_COPY_WORDS[0], COPY),
            (OS_COMPRESSED_COPY_WORDS[1], COPY),
        ] {
            put(&mut reset, OS_FUNCTION + 4 * word, bl(OS_FUNCTION + 4 * word, target)).unwrap();
        }
        for (at, id) in [(0x28a8, 6u32), (0x28f8, 1), (0x2904, 2)] {
            put(&mut reset, at, 0x5280_0000 | (id << 5)).unwrap();
            put(&mut reset, at + 4, bl(at + 4, OS_FUNCTION)).unwrap();
        }
        table_copy(&mut reset, MAIN_COPY, 0x4_0000, 0x64_0000);
        table_copy(&mut reset, ALT_COPY, 0x4_0800, 0x12_0000);
        let mut head = vec![0u8; OS_HEAD];
        for (n, b) in head.iter_mut().enumerate() {
            *b = (n * 13 + 5) as u8;
        }
        edit(&mut head);
        let mut decoded = head.clone();
        decoded.extend(std::iter::repeat(0u8).take(0x1000));
        let stream = literal_stream(&decoded);
        let mut update = vec![0u8; 0x70_0000 + bias];
        let main = os_table(
            0x4_0000,
            &[
                (0, 0x8_1000, 0x1000, 0x8_2000, 0xc_0000, 0x1000, 0),
                (6, OS_DST, OS_HEAD as u64, OS_DST + OS_HEAD as u64, OS_FLASH as u32, stream.len() as u32, 1),
            ],
        );
        let alt = os_table(
            0x4_0800,
            &[(0, 0x8_1000, 0x1000, 0x8_2000, 0xc_0000, 0x1000, 0), (1, 0x88_1000, 0, 0xa8_0000, 0x14_0000, 0x1000, 1)],
        );
        update[0x64_0000 + bias..0x64_0800 + bias].copy_from_slice(&main);
        update[0x12_0000 + bias..0x12_0800 + bias].copy_from_slice(&alt);
        update[OS_FLASH + bias..OS_FLASH + bias + stream.len()].copy_from_slice(&stream);
        let mut ram = vec![0u8; OS_DST as usize + OS_HEAD];
        ram[0x4_0000..0x4_0800].copy_from_slice(&main);
        ram[OS_DST as usize..].copy_from_slice(&head);
        (reset, update, ram)
    }

    #[test]
    fn the_os_load_is_found_by_its_words_and_its_one_caller_with_the_os_id() {
        let (reset, update, ram) = os_stage(BIAS);
        let plan = locate_os(&reset, &update, BIAS, &ram).unwrap();
        assert_eq!(plan.call_site, 0x28ac);
        assert_eq!(plan.call_old, bl(0x28ac, OS_FUNCTION));
        assert_eq!((plan.load_fn, plan.copy_fn), (OS_FUNCTION, COPY));
        assert_eq!((plan.dst, plan.head_bytes, plan.flash_offset), (OS_DST, OS_HEAD, OS_FLASH as u64));
        assert_eq!(plan.tables, [(0x4_0000, 0x64_0000), (0x4_0800, 0x12_0000)]);
    }

    #[test]
    fn the_os_load_is_refused_where_the_stage_or_the_bytes_differ() {
        let (reset, update, ram) = os_stage(BIAS);
        let refused = |reset: &[u8], update: &[u8], ram: &[u8], what: &str| {
            let error = locate_os(reset, update, BIAS, ram).unwrap_err().to_string();
            assert!(error.contains(what), "{error}");
        };
        let mut changed = reset.clone();
        put(&mut changed, OS_FUNCTION + 4 * 20, 0xd503_201f).unwrap();
        refused(&changed, &update, &ram, "not found exactly once");
        // A second caller that passes the OS id.
        let mut changed = reset.clone();
        put(&mut changed, 0x28f8, 0x5280_0000 | (6 << 5)).unwrap();
        refused(&changed, &update, &ram, "exactly one caller");
        // The card's bytes are not the decode of the flash's compressed bytes.
        let mut changed = ram.clone();
        changed[OS_DST as usize + 7] ^= 1;
        refused(&reset, &update, &changed, "not the decode");
        // The decode has a non-zero byte after the initialized ones (which the stub does not compare).
        let mut changed = update.clone();
        let mut decoded = ram[OS_DST as usize..].to_vec();
        decoded.extend(std::iter::repeat(0u8).take(0x1000));
        *decoded.last_mut().unwrap() = 1;
        let stream = literal_stream(&decoded);
        changed[OS_FLASH + BIAS..OS_FLASH + BIAS + stream.len()].copy_from_slice(&stream);
        refused(&reset, &changed, &ram, "not zero");
        // The maintenance table names the OS section too.
        let mut changed = update.clone();
        let alt = os_table(0x4_0800, &[(6, OS_DST, OS_HEAD as u64, OS_DST + OS_HEAD as u64, OS_FLASH as u32, 0x100, 1)]);
        changed[0x12_0000 + BIAS..0x12_0800 + BIAS].copy_from_slice(&alt);
        refused(&reset, &changed, &ram, "maintenance");
        // The row is not compressed.
        let mut changed = update.clone();
        let main = os_table(0x4_0000, &[(6, OS_DST, OS_HEAD as u64, OS_DST + OS_HEAD as u64, OS_FLASH as u32, 0x100, 0)]);
        changed[0x64_0000 + BIAS..0x64_0800 + BIAS].copy_from_slice(&main);
        refused(&reset, &changed, &ram, "not stored compressed");
    }

    fn os_lamp() -> StubLamp {
        StubLamp { pio_base: 0xff90_7000, green_port: 8, red_port: 6, pin: 2 }
    }
    /// An OS stub template: zero code, a zero parameter block, laid out as `os_stub.S` lays it out.
    fn os_template() -> Vec<u8> {
        super::super::os_stub_template::IMAGE.to_vec()
    }

    #[test]
    fn the_os_binding_carries_the_call_word_the_stub_bytes_and_every_crc() {
        let (reset, update, ram) = os_stage(BIAS);
        let plan = locate_os(&reset, &update, BIAS, &ram).unwrap();
        let head = ram[OS_DST as usize..].to_vec();
        let guard = 0x1_4000;
        let template = os_template();
        let binding =
            os_binding(&plan, &head, &reset[..guard], &update, BIAS, 0xf000_0000, 0x3000_0000, guard, true, &os_lamp(), &template)
                .unwrap();
        let redirect = binding.stub.redirect.0;
        let stub_at = binding.stub.offset;
        assert_eq!(stub_at % SLED_ALIGN, 0);
        // The call word is a bl from the call site to the stub, and the stock word was a bl to the load.
        let new = redirect[4] as u32;
        assert_eq!(new, bl(0x28ac, stub_at));
        assert_eq!(redirect[2], 0x28ac);
        assert_eq!(redirect[3], u64::from(plan.call_old));
        assert_eq!((redirect[7], redirect[8]), (0, OS_HEAD as u64));
        assert_eq!(redirect[9], 0xf000_0000 + OS_FLASH as u64);
        assert_eq!(redirect[10], u64::from(crc32(&head)));
        // The patched image is the stock image with the stub and the call word and nothing else.
        let mut patched = reset[..guard].to_vec();
        patched[stub_at..stub_at + binding.stub.image.len()].copy_from_slice(&binding.stub.image);
        patched[0x28ac..0x28b0].copy_from_slice(&new.to_le_bytes());
        assert_eq!(redirect[6], u64::from(crc32(&patched)));
        assert_ne!(redirect[5], redirect[6]);
        // The stub's parameter block holds the bound facts.
        let word64 = |offset: usize| u64::from_le_bytes(binding.stub.image[offset..offset + 8].try_into().unwrap());
        use super::super::os_stub_template as t;
        assert_eq!(word64(t::XP5_OS_P_SELF), stub_at as u64);
        assert_eq!(word64(t::XP5_OS_P_RETURN), 0x28b0);
        assert_eq!(word64(t::XP5_OS_P_ID), 6);
        assert_eq!(word64(t::XP5_OS_P_FUNC), OS_FUNCTION as u64);
        assert_eq!(word64(t::XP5_OS_P_DST), OS_DST);
        assert_eq!(word64(t::XP5_OS_P_BYTES), OS_HEAD as u64);
        assert_eq!(word64(t::XP5_OS_P_DRAM), 0x3000_0000);
        assert_eq!(word64(t::XP5_OS_P_COPY), COPY as u64);
        assert_eq!(word64(t::XP5_OS_P_CHUNK), OS_CHUNK);
        assert_eq!(word64(t::XP5_OS_OUT_STATUS), 0);
        // The loader's OS words: the compressed range's length and CRC-32, the id, the destination.
        let comp = &update[OS_FLASH + BIAS..OS_FLASH + BIAS + plan.comp_bytes];
        assert_eq!(binding.os[..7], [1, plan.comp_bytes as u64, u64::from(crc32(comp)), 6, OS_DST, COPY as u64, OS_CHUNK]);
        // Verify-only binds the same words with `apply` clear.
        let verify =
            os_binding(&plan, &head, &reset[..guard], &update, BIAS, 0xf000_0000, 0x3000_0000, guard, false, &os_lamp(), &template)
                .unwrap();
        assert_eq!(verify.stub.redirect.0[1], 0);
        assert_eq!(verify.stub.redirect.0[2..], redirect[2..]);
    }

    #[test]
    fn the_os_binding_refuses_what_does_not_fit() {
        let (reset, update, ram) = os_stage(BIAS);
        let plan = locate_os(&reset, &update, BIAS, &ram).unwrap();
        let head = ram[OS_DST as usize..].to_vec();
        let guard = 0x1_4000;
        let template = os_template();
        let go = |plan: &OsPlan, head: &[u8], destination: u64, lamp: &StubLamp| {
            os_binding(plan, head, &reset[..guard], &update, BIAS, 0xf000_0000, destination, guard, true, lamp, &template)
        };
        assert!(go(&plan, &head, 0x3000_0000, &os_lamp()).is_ok());
        assert!(go(&plan, &head, 0x3000_0004, &os_lamp()).is_err());
        assert!(go(&plan, &head, 0x1000, &os_lamp()).is_err());
        assert!(go(&plan, &head[..head.len() - 16], 0x3000_0000, &os_lamp()).is_err());
        let mut lamp = os_lamp();
        lamp.red_port = lamp.green_port;
        assert!(go(&plan, &head, 0x3000_0000, &lamp).is_err());
        // A call word that is not a bl.
        let mut changed = plan.clone();
        changed.call_old = 0xd503_201f;
        assert!(go(&changed, &head, 0x3000_0000, &os_lamp()).is_err());
        // A template whose parameter block is not zero.
        let mut dirty = template.clone();
        let last = dirty.len() - 1;
        dirty[last] = 1;
        assert!(os_binding(&plan, &head, &reset[..guard], &update, BIAS, 0xf000_0000, 0x3000_0000, guard, true, &os_lamp(), &dirty).is_err());
    }

    #[test]
    fn the_os_binding_is_written_into_a_template_and_only_a_template() {
        let mut program = vec![0u8; 0x200];
        program[0x100..0x108].copy_from_slice(&1u64.to_le_bytes());
        let words = [1u64, 0x720000, 0xdead_beef, 6, 0x11c1000, 0xa090, 0x20000, 0];
        bind_os(&mut program, 0x100, &words).unwrap();
        for (n, w) in words.iter().enumerate() {
            assert_eq!(u64::from_le_bytes(program[0x100 + 8 * n..0x108 + 8 * n].try_into().unwrap()), *w);
        }
        assert!(bind_os(&mut program, 0x100, &words).is_err(), "a bound template is refused");
    }

    // ---- the Exif string: the named words of the OS section ----

    /// Where the test decode keeps the string, and the two sites that copy it.
    pub(in crate::xp5) const EXIF_AT: usize = 0x100;
    const EXIF_SITE_AT: [usize; 2] = [0x400, 0x800];

    /// `os_stage` with the string in the decode and two sites that build its address after `mov w2, #15`.
    pub(in crate::xp5) fn exif_stage(bias: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        os_stage_with(bias, &|head: &mut Vec<u8>| {
            head[EXIF_AT..EXIF_AT + 16].copy_from_slice(EXIF_STOCK_TEXT);
            for at in EXIF_SITE_AT {
                put(head, at, EXIF_COPY_LENGTH).unwrap();
                put(head, at + 4, adrp(OS_DST as usize + at + 4, 1, OS_DST + EXIF_AT as u64)).unwrap();
                put(head, at + 8, 0x9100_0021 | ((((OS_DST as u32 + EXIF_AT as u32) & 0xfff) as u32) << 10)).unwrap();
            }
        })
    }

    #[test]
    fn the_string_its_two_sites_and_the_named_words_are_found_in_the_decode() {
        let (reset, update, ram) = exif_stage(BIAS);
        let plan = locate_exif(&reset, &update, BIAS, &ram).unwrap();
        assert_eq!(plan.window, OS_DST + EXIF_AT as u64);
        assert_eq!(plan.sites, [OS_DST + 0x404, OS_DST + 0x804]);
        // two words of context, the string's four words, two of context: eight words, ascending, four before and after changing nothing
        assert_eq!(plan.words.len(), 8);
        assert!(plan.words.windows(2).all(|p| p[1].address == p[0].address + 4));
        assert_eq!(plan.words[0].address, plan.window - 8);
        let changed: Vec<&ExifWord> = plan.words.iter().filter(|w| w.stock != w.new).collect();
        assert_eq!(changed.iter().map(|w| w.address).collect::<Vec<_>>(), [plan.window, plan.window + 4]);
        assert_eq!(changed.iter().map(|w| (w.stock, w.new)).collect::<Vec<_>>(), [(0x6967_6944, 0x494a_5546), (0x206c_6174, 0x2044_4f4d)]);
        // the window's bytes, assembled from the table, are the two texts
        let bytes = |pick: fn(&ExifWord) -> u32| -> Vec<u8> {
            plan.words[2..6].iter().flat_map(|w| pick(w).to_le_bytes()).collect()
        };
        assert_eq!(bytes(|w| w.stock), EXIF_STOCK_TEXT);
        assert_eq!(bytes(|w| w.new), EXIF_NEW_TEXT);
        assert_eq!(plan.os, locate_os(&reset, &update, BIAS, &ram).unwrap());
    }

    #[test]
    fn the_string_is_refused_where_it_is_not_alone_or_not_reached_the_way_the_proof_needs() {
        let go = |edit: &dyn Fn(&mut Vec<u8>)| {
            let (reset, update, ram) = os_stage_with(BIAS, &|head: &mut Vec<u8>| {
                head[EXIF_AT..EXIF_AT + 16].copy_from_slice(EXIF_STOCK_TEXT);
                for at in EXIF_SITE_AT {
                    put(head, at, EXIF_COPY_LENGTH).unwrap();
                    put(head, at + 4, adrp(OS_DST as usize + at + 4, 1, OS_DST + EXIF_AT as u64)).unwrap();
                    put(head, at + 8, 0x9100_0021 | ((((OS_DST as u32 + EXIF_AT as u32) & 0xfff) as u32) << 10)).unwrap();
                }
                edit(head);
            });
            locate_exif(&reset, &update, BIAS, &ram).map(|_| ()).map_err(|e| e.to_string())
        };
        assert!(go(&|_| {}).is_ok());
        // no string
        assert!(go(&|h| h[EXIF_AT] ^= 1).unwrap_err().contains("exactly once"));
        // a second copy of the string
        assert!(go(&|h| h[0x1000..0x1010].copy_from_slice(EXIF_STOCK_TEXT)).unwrap_err().contains("exactly once"));
        // a pointer to the string in the data (a 64-bit value)
        assert!(go(&|h| h[0x1100..0x1108].copy_from_slice(&(OS_DST + EXIF_AT as u64 + 4).to_le_bytes()))
            .unwrap_err()
            .contains("something other than an adrp"));
        // a branch into it
        assert!(go(&|h| put(h, 0x1200, 0x1400_0000 | (((EXIF_AT as u32).wrapping_sub(0x1200)) >> 2 & 0x03ff_ffff)).unwrap())
            .unwrap_err()
            .contains("something other than an adrp"));
        // a site that does not copy 15 bytes
        assert!(go(&|h| put(h, EXIF_SITE_AT[0], 0x5280_0202).unwrap()).unwrap_err().contains("15 bytes"));
        // one site only
        assert!(go(&|h| put(h, EXIF_SITE_AT[1] + 8, 0xd503_201f).unwrap()).unwrap_err().contains("exactly two"));
        // three sites
        assert!(go(&|h| {
            put(h, 0xc00, EXIF_COPY_LENGTH).unwrap();
            put(h, 0xc04, adrp(OS_DST as usize + 0xc04, 1, OS_DST + EXIF_AT as u64)).unwrap();
            put(h, 0xc08, 0x9100_0021 | ((((OS_DST as u32 + EXIF_AT as u32) & 0xfff) as u32) << 10)).unwrap();
        })
        .unwrap_err()
        .contains("exactly two"));
        // a string that is not word aligned
        assert!(go(&|h| {
            h[EXIF_AT..EXIF_AT + 16].copy_from_slice(&[0x55; 16]);
            h[EXIF_AT + 1..EXIF_AT + 17].copy_from_slice(EXIF_STOCK_TEXT);
        })
        .is_err());
    }

    fn exif_template() -> Vec<u8> {
        super::super::exif_stub_template::IMAGE.to_vec()
    }

    #[test]
    fn the_exif_binding_carries_the_call_word_the_stub_bytes_the_table_and_every_crc() {
        let (reset, update, ram) = exif_stage(BIAS);
        let plan = locate_exif(&reset, &update, BIAS, &ram).unwrap();
        let file = vec![0x5au8; 4096];
        let guard = 0x1_4000;
        let template = exif_template();
        let make = |apply: bool, plan: &ExifPlan, file: &[u8], destination: u64, lamp: &StubLamp, template: &[u8]| {
            exif_binding(plan, file, &reset[..guard], &update, BIAS, 0xf000_0000, destination, guard, apply, lamp, template)
        };
        let binding = make(true, &plan, &file, 0x3000_0000, &os_lamp(), &template).unwrap();
        let redirect = binding.stub.redirect.0;
        let stub_at = binding.stub.offset;
        let new = redirect[4] as u32;
        assert_eq!(new, bl(0x28ac, stub_at));
        assert_eq!((redirect[2], redirect[3]), (0x28ac, u64::from(plan.os.call_old)));
        // the file is the placeholder: its length and CRC-32; the compressed range is bound, not the decode
        assert_eq!((redirect[7], redirect[8], redirect[10]), (0, 4096, u64::from(crc32(&file))));
        assert_eq!(redirect[9], 0xf000_0000 + OS_FLASH as u64);
        let mut patched = reset[..guard].to_vec();
        patched[stub_at..stub_at + binding.stub.image.len()].copy_from_slice(&binding.stub.image);
        patched[0x28ac..0x28b0].copy_from_slice(&new.to_le_bytes());
        assert_eq!(redirect[6], u64::from(crc32(&patched)));
        // the stub's parameter block: the bound facts and the table
        let word64 = |offset: usize| u64::from_le_bytes(binding.stub.image[offset..offset + 8].try_into().unwrap());
        use super::super::exif_stub_template as t;
        assert_eq!(word64(t::XP5_EXIF_P_SELF), stub_at as u64);
        assert_eq!((word64(t::XP5_EXIF_P_RETURN), word64(t::XP5_EXIF_P_ID), word64(t::XP5_EXIF_P_FUNC)), (0x28b0, 6, OS_FUNCTION as u64));
        assert_eq!((word64(t::XP5_EXIF_P_DST), word64(t::XP5_EXIF_P_BYTES), word64(t::XP5_EXIF_P_COUNT)), (OS_DST, OS_HEAD as u64, 8));
        for (n, w) in plan.words.iter().enumerate() {
            assert_eq!(word64(t::XP5_EXIF_P_TABLE + 16 * n), w.address);
            assert_eq!(word64(t::XP5_EXIF_P_TABLE + 16 * n + 8), u64::from(w.stock) | (u64::from(w.new) << 32));
        }
        assert_eq!(word64(t::XP5_EXIF_OUT_STATUS), 0);
        // the loader's words: the compressed range, the section, the window, the two window CRC-32s, the table
        let comp = &update[OS_FLASH + BIAS..OS_FLASH + BIAS + plan.os.comp_bytes];
        let e = &binding.exif;
        assert_eq!(e.len(), 37);
        assert_eq!(
            e[..13],
            [1, 0xf000_0000, plan.os.comp_bytes as u64, u64::from(crc32(comp)), 0x4_0000, 6, OS_DST, OS_HEAD as u64, plan.window, 16, 8,
             u64::from(crc32(EXIF_STOCK_TEXT)), u64::from(crc32(EXIF_NEW_TEXT))]
        );
        assert_eq!(e[13], plan.words[0].address);
        assert!(e[29..].iter().all(|v| *v == 0));
        // verify-only binds the same words with `apply` clear
        let verify = make(false, &plan, &file, 0x3000_0000, &os_lamp(), &template).unwrap();
        assert_eq!(verify.stub.redirect.0[1], 0);
        assert_eq!(verify.stub.redirect.0[2..], redirect[2..]);
        assert_eq!(verify.exif, binding.exif);
        // what it refuses: a destination that does not fit, an empty or a huge file, the same port for both colours,
        // a template whose parameter block is not zero, a table that changes a word outside the string, or is not ascending
        assert!(make(true, &plan, &file, 0x3000_0004, &os_lamp(), &template).is_err());
        assert!(make(true, &plan, &file, 0x1000, &os_lamp(), &template).is_err());
        assert!(make(true, &plan, &[], 0x3000_0000, &os_lamp(), &template).is_err());
        assert!(make(true, &plan, &vec![0u8; 0x2_0000], 0x3000_0000, &os_lamp(), &template).is_err());
        let mut lamp = os_lamp();
        lamp.red_port = lamp.green_port;
        assert!(make(true, &plan, &file, 0x3000_0000, &lamp, &template).is_err());
        let mut dirty = template.clone();
        let last = dirty.len() - 1;
        dirty[last] = 1;
        assert!(make(true, &plan, &file, 0x3000_0000, &os_lamp(), &dirty).is_err());
        let mut outside = plan.clone();
        outside.words[0].new ^= 1;
        assert!(make(true, &outside, &file, 0x3000_0000, &os_lamp(), &template).is_err());
        let mut unsorted = plan.clone();
        unsorted.words.swap(0, 1);
        assert!(make(true, &unsorted, &file, 0x3000_0000, &os_lamp(), &template).is_err());
        let mut many = plan.clone();
        many.words.push(ExifWord { address: plan.words[7].address + 4, stock: 0, new: 0 });
        assert!(make(true, &many, &file, 0x3000_0000, &os_lamp(), &template).is_err());
        let mut call = plan.clone();
        call.os.call_old = 0xd503_201f;
        assert!(make(true, &call, &file, 0x3000_0000, &os_lamp(), &template).is_err());
    }

    #[test]
    fn the_exif_binding_is_written_into_a_template_and_only_a_template() {
        let mut program = vec![0u8; 0x400];
        program[0x100..0x108].copy_from_slice(&1u64.to_le_bytes());
        let mut words = vec![0u64; 37];
        words[0] = 1;
        words[1] = 0xf000_0000;
        words[8] = 0x19c_26a8;
        bind_exif(&mut program, 0x100, &words).unwrap();
        assert_eq!(u64::from_le_bytes(program[0x108..0x110].try_into().unwrap()), 0xf000_0000);
        assert!(bind_exif(&mut program, 0x100, &words).is_err(), "a bound template is refused");
        let mut other = vec![0u8; 0x400];
        other[0x100..0x108].copy_from_slice(&1u64.to_le_bytes());
        assert!(bind_exif(&mut other, 0x100, &words[..36]).is_err());
    }
    // ---- the hook: a branch at one OS function and a program in padding ----

    /// Where the test decode keeps the hooked function, its two calls, the OS's map of its code and the padding.
    pub(in crate::xp5) const HOOK_AT: usize = 0x300;
    const HOOK_CALL_AT: [usize; 2] = [0x500, 0x600];
    const HOOK_MAP_AT: usize = 0x900;
    const HOOK_PAD_AT: usize = 0xc00;
    /// The end of the test OS's code, relative to the section.
    pub(in crate::xp5) const HOOK_TEXT_END: u64 = 0x1800;

    pub(in crate::xp5) fn hook_stage(bias: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        hook_stage_with(bias, &|_| {})
    }
    /// `os_stage` with the hooked function, two calls of it, the OS's code map and a run of padding in the decode.
    pub(in crate::xp5) fn hook_stage_with(bias: usize, edit: &dyn Fn(&mut Vec<u8>)) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        os_stage_with(bias, &|head: &mut Vec<u8>| {
            let base = OS_DST as usize;
            let function = [
                0xa9bf_7bfd,
                0x9100_03fd,
                0x5280_1502,
                0xaa00_03e1,
                adrp(base + HOOK_AT + 16, 0, 0x263_c000),
                0x9100_0000 | (0xd48 << 10),
                bl(base + HOOK_AT + 24, base + 0x1f00),
                0xa8c1_7bfd,
                0xd65f_03c0,
            ];
            for (n, w) in function.into_iter().enumerate() {
                put(head, HOOK_AT + 4 * n, w).unwrap();
            }
            for (k, at) in HOOK_CALL_AT.into_iter().enumerate() {
                put(head, at, 0x9100_03e0 | ((0x38 + 0x10 * k as u32) << 10)).unwrap(); // add x0, sp, #0x38 (or #0x48)
                put(head, at + 4, bl(base + at + 4, base + HOOK_AT)).unwrap();
            }
            // mov w2, #7; adrp x1, end; add x1, x1, #lo; adrp x0, start; add x0, x0, #0; bl map
            let end = OS_DST + HOOK_TEXT_END;
            put(head, HOOK_MAP_AT, 0x5280_00e2).unwrap();
            put(head, HOOK_MAP_AT + 4, adrp(base + HOOK_MAP_AT + 4, 1, end)).unwrap();
            put(head, HOOK_MAP_AT + 8, 0x9100_0021 | (((end & 0xfff) as u32) << 10)).unwrap();
            put(head, HOOK_MAP_AT + 12, adrp(base + HOOK_MAP_AT + 12, 0, OS_DST)).unwrap();
            put(head, HOOK_MAP_AT + 16, 0x9100_0000).unwrap();
            put(head, HOOK_MAP_AT + 20, bl(base + HOOK_MAP_AT + 20, base + 0x1f40)).unwrap();
            // a ret, sixteen zero words, seven nops, then code
            put(head, HOOK_PAD_AT - 4, 0xd65f_03c0).unwrap();
            for n in 0..16 {
                put(head, HOOK_PAD_AT + 4 * n, 0).unwrap();
            }
            for n in 16..23 {
                put(head, HOOK_PAD_AT + 4 * n, 0xd503_201f).unwrap();
            }
            put(head, HOOK_PAD_AT + 4 * 23, 0xb400_0d82).unwrap();
            edit(head);
        })
    }

    /// The thirteen words as the packager's own assembler is not allowed to be the only author of them: the
    /// program written out by hand for the test addresses.
    fn hook_reference(hook: u64, cave: u64) -> [u32; 13] {
        let b = |from: u64, to: u64| 0x1400_0000 | ((((to as i64 - from as i64) >> 2) as u32) & 0x03ff_ffff);
        let lit = |from: u64, to: u64| 0x5800_0011 | ((((to as i64 - from as i64) >> 2) as u32 & 0x7_ffff) << 5);
        [
            b(hook, cave + 16),
            0x6967_6944,
            0x206c_6174,
            0x494a_5546,
            0x2044_4f4d,
            0xf940_1810,
            lit(cave + 20, cave),
            0xeb11_021f,
            0x5400_0001 | ((((cave + 40) as i64 - (cave + 28) as i64) >> 2) as u32) << 5,
            lit(cave + 32, cave + 8),
            0xf900_1811,
            0xa9bf_7bfd,
            b(cave + 44, hook + 4),
        ]
    }

    #[test]
    fn the_hook_program_is_the_one_written_out_by_hand_for_the_real_addresses() {
        // the addresses of the real OS section: the function at 0x1425498, the padding at 0x16bea88
        let words = hook_program(0x142_5498, 0x16b_ea88, HOOK_NEW_TEXT).unwrap();
        assert_eq!(
            words,
            [
                0x140a_6580, 0x6967_6944, 0x206c_6174, 0x494a_5546, 0x2044_4f4d, 0xf940_1810, 0x58ff_ff71, 0xeb11_021f,
                0x5400_0061, 0x58ff_ff51, 0xf900_1811, 0xa9bf_7bfd, 0x17f5_9a7a,
            ]
        );
        assert_eq!(words, hook_reference(0x142_5498, 0x16b_ea88));
        // out of range: a branch of more than 128 MiB
        assert!(hook_program(0x142_5498, 0x142_5498 + (1 << 27), HOOK_NEW_TEXT).is_err());
    }

    #[test]
    fn the_hooked_function_its_calls_the_code_range_and_the_padding_are_found_in_the_decode() {
        let (reset, update, ram) = hook_stage(BIAS);
        let plan = locate_hook(&reset, &update, BIAS, &ram).unwrap();
        assert_eq!(plan.hook, OS_DST + HOOK_AT as u64);
        assert_eq!(plan.callers, [OS_DST + 0x504, OS_DST + 0x604]);
        assert_eq!(plan.text_end, OS_DST + HOOK_TEXT_END);
        // the padding starts at 0xc00; the program goes at the next 8-aligned address after its first word
        assert_eq!(plan.cave, (OS_DST + HOOK_PAD_AT as u64 + 4 + 7) & !7);
        assert_eq!(plan.words.len(), 13);
        assert_eq!(plan.words[0].address, plan.hook);
        assert_eq!(plan.words[0].stock, HOOK_DISPLACED);
        for (n, w) in plan.words.iter().enumerate().skip(1) {
            assert_eq!(w.address, plan.cave + 4 * (n as u64 - 1));
            assert!(w.stock == 0 || w.stock == 0xd503_201f);
        }
        let reference = hook_reference(plan.hook, plan.cave);
        assert_eq!(plan.words.iter().map(|w| w.new).collect::<Vec<_>>(), reference);
        assert_eq!(plan.os, locate_os(&reset, &update, BIAS, &ram).unwrap());
    }

    #[test]
    fn the_hook_is_refused_where_the_function_the_calls_the_code_range_or_the_padding_are_not_as_proven() {
        let go = |edit: &dyn Fn(&mut Vec<u8>)| {
            let (reset, update, ram) = hook_stage_with(BIAS, edit);
            locate_hook(&reset, &update, BIAS, &ram).map(|_| ()).map_err(|e| e.to_string())
        };
        assert!(go(&|_| {}).is_ok());
        // the function changed (its length, or not found twice)
        assert!(go(&|h| put(h, HOOK_AT + 8, 0x5280_1502 + 0x20).unwrap()).unwrap_err().contains("exactly once"));
        assert!(go(&|h| {
            for n in 0..9 {
                let w = word(h, HOOK_AT + 4 * n).unwrap();
                put(h, 0x1000 + 4 * n, if n == 6 { 0x9400_0000 | 0x20 } else { w }).unwrap();
            }
        })
        .unwrap_err()
        .contains("exactly once"));
        // a call that does not pass a stack address, one that passes an address that is not a multiple of eight
        assert!(go(&|h| put(h, HOOK_CALL_AT[0], 0xd503_201f).unwrap()).unwrap_err().contains("stack address"));
        assert!(go(&|h| put(h, HOOK_CALL_AT[1], 0x9100_03e0 | (0x3c << 10)).unwrap()).unwrap_err().contains("stack address"));
        // only one call
        assert!(go(&|h| put(h, HOOK_CALL_AT[1] + 4, 0xd503_201f).unwrap()).unwrap_err().contains("fewer than two"));
        // a tail jump to the function, a pointer to it, a branch from elsewhere
        assert!(go(&|h| put(h, 0xe00, 0x1400_0000 | (((HOOK_AT as u32).wrapping_sub(0xe00) >> 2) & 0x03ff_ffff)).unwrap())
            .unwrap_err()
            .contains("by a jump"));
        assert!(go(&|h| h[0xe08..0xe10].copy_from_slice(&(OS_DST + HOOK_AT as u64).to_le_bytes()))
            .unwrap_err()
            .contains("something other than a call"));
        // the code range is not mapped once, does not start at the section, or ends beyond it
        assert!(go(&|h| put(h, HOOK_MAP_AT, 0xd503_201f).unwrap()).unwrap_err().contains("not found exactly once"));
        assert!(go(&|h| put(h, HOOK_MAP_AT + 12, adrp(OS_DST as usize + HOOK_MAP_AT + 12, 0, OS_DST + 0x1000)).unwrap())
            .unwrap_err()
            .contains("code range"));
        // the padding: too short, not after a ret, a non-zero word in the middle, referred to
        assert!(go(&|h| put(h, HOOK_PAD_AT + 4 * 10, 0x1234_5678).unwrap()).unwrap_err().contains("no run of padding"));
        assert!(go(&|h| put(h, HOOK_PAD_AT - 4, 0xd503_201f).unwrap()).unwrap_err().contains("no run of padding"));
        assert!(go(&|h| {
            put(h, 0xe00, 0x1400_0000 | (((HOOK_PAD_AT as u32 + 8).wrapping_sub(0xe00) >> 2) & 0x03ff_ffff)).unwrap();
        })
        .unwrap_err()
        .contains("refers to the padding"));
        assert!(go(&|h| put(h, 0xe00, 0x1800_0000 | ((((HOOK_PAD_AT as u32 + 8).wrapping_sub(0xe00) >> 2) & 0x7_ffff) << 5)).unwrap())
            .unwrap_err()
            .contains("refers to the padding"));
        assert!(go(&|h| {
            put(h, 0xe00, adrp(OS_DST as usize + 0xe00, 3, OS_DST + HOOK_PAD_AT as u64 + 8)).unwrap();
            put(h, 0xe04, 0x9100_0063 | ((((HOOK_PAD_AT + 8) & 0xfff) as u32) << 10)).unwrap();
        })
        .unwrap_err()
        .contains("refers to the padding"));
    }

    fn hook_template() -> Vec<u8> {
        super::super::hook_stub_template::IMAGE.to_vec()
    }

    #[test]
    fn the_hook_binding_carries_the_call_word_the_stub_bytes_the_table_and_every_crc() {
        let (reset, update, ram) = hook_stage(BIAS);
        let plan = locate_hook(&reset, &update, BIAS, &ram).unwrap();
        let file = vec![0x5au8; 4096];
        let guard = 0x1_4000;
        let template = hook_template();
        let make = |apply: bool, plan: &HookPlan, file: &[u8], destination: u64, lamp: &StubLamp, template: &[u8]| {
            hook_binding(plan, file, &reset[..guard], &update, BIAS, 0xf000_0000, destination, guard, apply, lamp, template)
        };
        let binding = make(true, &plan, &file, 0x3000_0000, &os_lamp(), &template).unwrap();
        let redirect = binding.stub.redirect.0;
        let stub_at = binding.stub.offset;
        let new = redirect[4] as u32;
        assert_eq!(new, bl(0x28ac, stub_at));
        assert_eq!((redirect[2], redirect[3]), (0x28ac, u64::from(plan.os.call_old)));
        assert_eq!((redirect[7], redirect[8], redirect[10]), (0, 4096, u64::from(crc32(&file))));
        assert_eq!(redirect[9], 0xf000_0000 + OS_FLASH as u64);
        let mut patched = reset[..guard].to_vec();
        patched[stub_at..stub_at + binding.stub.image.len()].copy_from_slice(&binding.stub.image);
        patched[0x28ac..0x28b0].copy_from_slice(&new.to_le_bytes());
        assert_eq!(redirect[6], u64::from(crc32(&patched)));
        // the stub's parameter block: the bound facts and the table
        let word64 = |offset: usize| u64::from_le_bytes(binding.stub.image[offset..offset + 8].try_into().unwrap());
        use super::super::hook_stub_template as t;
        assert_eq!(word64(t::XP5_HOOK_P_SELF), stub_at as u64);
        assert_eq!((word64(t::XP5_HOOK_P_RETURN), word64(t::XP5_HOOK_P_ID), word64(t::XP5_HOOK_P_FUNC)), (0x28b0, 6, OS_FUNCTION as u64));
        assert_eq!((word64(t::XP5_HOOK_P_DST), word64(t::XP5_HOOK_P_BYTES), word64(t::XP5_HOOK_P_COUNT)), (OS_DST, OS_HEAD as u64, 13));
        for (n, w) in plan.words.iter().enumerate() {
            assert_eq!(word64(t::XP5_HOOK_P_TABLE + 16 * n), w.address);
            assert_eq!(word64(t::XP5_HOOK_P_TABLE + 16 * n + 8), u64::from(w.stock) | (u64::from(w.new) << 32));
        }
        assert!((13..16).all(|n| word64(t::XP5_HOOK_P_TABLE + 16 * n) == 0 && word64(t::XP5_HOOK_P_TABLE + 16 * n + 8) == 0));
        assert_eq!(word64(t::XP5_HOOK_OUT_STATUS), 0);
        // the loader's words
        let comp = &update[OS_FLASH + BIAS..OS_FLASH + BIAS + plan.os.comp_bytes];
        let stock: Vec<u8> = plan.words.iter().flat_map(|w| w.stock.to_le_bytes()).collect();
        let newb: Vec<u8> = plan.words.iter().flat_map(|w| w.new.to_le_bytes()).collect();
        let h = &binding.hook;
        assert_eq!(h.len(), 53);
        assert_eq!(
            h[..13],
            [1, 0xf000_0000, plan.os.comp_bytes as u64, u64::from(crc32(comp)), 0x4_0000, 6, OS_DST, OS_HEAD as u64, plan.hook, plan.cave, 13,
             u64::from(crc32(&stock)), u64::from(crc32(&newb))]
        );
        assert_eq!(h[13], plan.words[0].address);
        assert!(h[13 + 26..].iter().all(|v| *v == 0));
        // verify-only binds the same words with `apply` clear
        let verify = make(false, &plan, &file, 0x3000_0000, &os_lamp(), &template).unwrap();
        assert_eq!(verify.stub.redirect.0[1], 0);
        assert_eq!(verify.stub.redirect.0[2..], redirect[2..]);
        assert_eq!(verify.hook, binding.hook);
        // what it refuses
        assert!(make(true, &plan, &file, 0x3000_0004, &os_lamp(), &template).is_err());
        assert!(make(true, &plan, &file, 0x1000, &os_lamp(), &template).is_err());
        assert!(make(true, &plan, &[], 0x3000_0000, &os_lamp(), &template).is_err());
        assert!(make(true, &plan, &vec![0u8; 0x2_0000], 0x3000_0000, &os_lamp(), &template).is_err());
        let mut lamp = os_lamp();
        lamp.red_port = lamp.green_port;
        assert!(make(true, &plan, &file, 0x3000_0000, &lamp, &template).is_err());
        let mut dirty = template.clone();
        let last = dirty.len() - 1;
        dirty[last] = 1;
        assert!(make(true, &plan, &file, 0x3000_0000, &os_lamp(), &dirty).is_err());
        // a table that is not the program: one new word changed, one stock word, an address, the order, the count, the hook word's stock
        for edit in [
            (|p: &mut HookPlan| p.words[6].new ^= 1) as fn(&mut HookPlan),
            |p| p.words[3].new ^= 0x20,
            |p| p.words[5].stock = 0x1234_5678,
            |p| p.words[1].stock = 0xdead_beef,
            |p| p.words[0].stock ^= 1,
            |p| p.words[2].address += 4,
            |p| p.words.swap(5, 6),
            |p| {
                p.words.pop();
            },
            |p| p.words.push(ExifWord { address: p.cave + 48, stock: 0, new: 0 }),
            |p| p.cave += 8,
            |p| p.hook += 4,
            |p| p.text_end = p.cave + 8,
        ] {
            let mut bad = plan.clone();
            edit(&mut bad);
            assert!(make(true, &bad, &file, 0x3000_0000, &os_lamp(), &template).is_err());
        }
        let mut call = plan.clone();
        call.os.call_old = 0xd503_201f;
        assert!(make(true, &call, &file, 0x3000_0000, &os_lamp(), &template).is_err());
    }

    #[test]
    fn the_hook_binding_is_written_into_a_template_and_only_a_template() {
        let mut program = vec![0u8; 0x400];
        program[0x100..0x108].copy_from_slice(&1u64.to_le_bytes());
        let mut words = vec![0u64; 53];
        words[0] = 1;
        words[1] = 0xf000_0000;
        words[8] = 0x142_5498;
        bind_hook(&mut program, 0x100, &words).unwrap();
        assert_eq!(u64::from_le_bytes(program[0x108..0x110].try_into().unwrap()), 0xf000_0000);
        assert!(bind_hook(&mut program, 0x100, &words).is_err(), "a bound template is refused");
        let mut other = vec![0u8; 0x400];
        other[0x100..0x108].copy_from_slice(&1u64.to_le_bytes());
        assert!(bind_hook(&mut other, 0x100, &words[..52]).is_err());
    }
}
