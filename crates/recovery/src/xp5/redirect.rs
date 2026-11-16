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
    let sx = |value: u32, bits: u32| (i64::from(value) << (64 - bits)) >> (64 - bits);
    let mut hits = Vec::new();
    for at in (0..image.len().saturating_sub(3)).step_by(4) {
        let w = word(image, at).unwrap_or(0);
        let direct = |offset: i64| usize::try_from(at as i64 + offset).ok().filter(|t| (lo..hi).contains(t));
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
            let page = ((at & !0xfff) as i64) + (sx(imm, 21) << 12);
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
    hits
}

/// The RAM-0 byte offset for a stub of `bytes` bytes: inside the one run of `nop` words that starts
/// directly after a `ret` and ends on a vector-table boundary, on a `SLED_ALIGN` boundary, with
/// `SLED_MARGIN` bytes of nops left after it, and only if nothing in the image refers to the run.
pub(super) fn sled(reset: &[u8], bytes: usize) -> Result<usize> {
    require(bytes != 0 && bytes % 4 == 0 && bytes <= STUB_MAX, "The copy stub's size is not usable")?;
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
            crc: u64::from(crc),
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
    let size = 48 + STUB_MAX + 32;
    let mut template = vec![0u8; size];
    template[..8].copy_from_slice(&1u64.to_le_bytes());
    require(extent(program, at, size)? == template, "XP5 copy-stub binding template changed")?;
    require(
        stub.image.len() % 4 == 0 && (16..=STUB_MAX).contains(&stub.image.len()),
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
}
