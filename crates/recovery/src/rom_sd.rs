//! EXR and Agate startup contracts located from the supplied update.
//! Selection is by processor and checked instruction/table grammar.
use crate::image::{
    self, Bss, Compression, Edit, HeaderWord, Packaging, Part, Plan, Region, Source,
};
use image::{Result, extent, require, word};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Processor {
    Exr,
    ExrPro,
    Xpp,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Located {
    pub plan: Plan,
    pub packaging: Packaging,
    pub missing: Vec<String>,
}
pub(super) fn source(
    data: &[u8],
    offset: usize,
    size: usize,
    compressed: bool,
) -> Result<(Source, Vec<u8>)> {
    let bytes = extent(data, offset, size)?;
    let decoded = if compressed {
        fujimod_firmware::decompress(bytes)?.0
    } else {
        bytes.to_vec()
    };
    Ok((
        Source {
            dat_offset: offset,
            size,
            compression: if compressed {
                Compression::Method4
            } else {
                Compression::Raw
            },
            decoded_size: decoded.len(),
            sha256: fujimod_firmware::sha256(&decoded),
            initialized_prefix: None,
        },
        decoded,
    ))
}
fn half(w: u32, register: u32, top: bool) -> Result<u32> {
    require(
        w & 0x0ff0f000 == (if top { 0x03400000 } else { 0x03000000 }) | register << 12,
        "Expected A32 MOVW/MOVT",
    )?;
    Ok((w >> 4 & 0xf000) | (w & 0xfff))
}
pub(super) fn address(data: &[u8], at: usize, register: u32) -> Result<usize> {
    Ok(
        (half(word(data, at)?, register, false)? | half(word(data, at + 4)?, register, true)? << 16)
            as usize,
    )
}
pub(super) fn call(data: &[u8], at: usize) -> Result<usize> {
    let w = word(data, at)?;
    require(w >> 24 == 0xeb, "Expected unconditional A32 BL")?;
    let target = at as i64 + 8 + ((((w & 0xffffff) as i32) << 8 >> 6) as i64);
    usize::try_from(target).map_err(|_| image::Error::Invalid("Call escapes loader".into()))
}
pub(super) fn edit(code: &[u8], at: usize, new: u32, what: &str) -> Result<Edit> {
    Ok(Edit {
        address: at,
        old: word(code, at)?,
        new,
        what: what.into(),
    })
}
pub(super) fn sequence(data: &[u8], at: usize, words: &[u32]) -> bool {
    words
        .iter()
        .enumerate()
        .all(|(i, w)| word(data, at + i * 4).ok() == Some(*w))
}
#[derive(Debug)]
struct Table {
    offset: usize,
    address: usize,
    rows: Vec<[u32; 9]>,
    arena: usize,
    arena_size: usize,
}
fn table(data: &[u8]) -> Result<Table> {
    let mut found = vec![];
    for marker in (0x80..data.len().saturating_sub(4)).step_by(4) {
        if word(data, marker)? != 0x1234 {
            continue;
        }
        for count in 2..25 {
            let Some(offset) = marker.checked_sub(0x40 + 36 * count) else {
                continue;
            };
            if offset + 0x400 > data.len() {
                continue;
            }
            let Some(base) = (word(data, offset)? as usize).checked_sub(0x40) else {
                continue;
            };
            if base & 0x3ff != 0 || word(data, offset + 12)? as usize != base + marker - offset {
                continue;
            }
            let Some(group) = (word(data, offset + 4)? as usize).checked_sub(base) else {
                continue;
            };
            if !(0x10..=0x3c0).contains(&group) {
                continue;
            }
            let bank = word(data, offset + group)? as usize;
            let arena = word(data, offset + group + 4)? as usize;
            let arena_size = word(data, offset + group + 8)? as usize;
            if !(base..base + 0x400).contains(&bank) || arena == 0 || arena_size == 0 {
                continue;
            }
            let mut rows = vec![];
            for n in 0..count {
                let mut row = [0; 9];
                for (i, w) in row.iter_mut().enumerate() {
                    *w = word(data, offset + 0x40 + n * 36 + i * 4)?;
                }
                rows.push(row);
            }
            let end = rows.pop().unwrap();
            if !matches!(end[0], 0xff | 0xffffffff)
                || end[1..].iter().any(|w| *w != 0)
                || rows[0][0] != 0
                || rows.iter().any(|r| r[8] > 1)
            {
                continue;
            }
            let normalized = base & !0x40000000;
            let address = if base & 0x40000000 != 0
                && rows.iter().any(|r| {
                    r[7] != 0
                        && normalized + 0x400 <= r[1] as usize
                        && (r[1] as usize) < normalized + 0x20000
                }) {
                normalized
            } else {
                base
            };
            // Every bank must lie in the arena and end in a complete sentinel.
            let mut p = bank - base;
            let mut banks = 0;
            while p + 24 <= 0x400 && !matches!(word(data, offset + p)?, 0xff | 0xffffffff) {
                let start = word(data, offset + p + 4)? as usize;
                let size = word(data, offset + p + 8)? as usize;
                if start < arena
                    || size == 0
                    || start
                        .checked_add(size)
                        .is_none_or(|e| e > arena + arena_size)
                    || word(data, offset + p + 16)? != 0
                {
                    break;
                }
                banks += 1;
                p += 24;
            }
            if banks > 0 && p + 24 <= 0x400 && matches!(word(data, offset + p)?, 0xff | 0xffffffff)
            {
                found.push(Table {
                    offset,
                    address,
                    rows,
                    arena,
                    arena_size,
                });
            }
        }
    }
    require(
        found.len() == 1,
        "No unique rooted nine-word section table with bounded overlay banks",
    )?;
    Ok(found.remove(0))
}
fn xpp_header(data: &[u8], bias: usize, loader: usize) -> Result<usize> {
    let prefix = extent(data, bias, loader - bias)?;
    let mut found = vec![];
    // DDR copy helper selects its initial configuration with LDRNE after
    // testing the initialization flag, then adds each ROM-compatible offset.
    for at in (12..prefix.len().saturating_sub(20)).step_by(4) {
        let w = word(prefix, at)?;
        let dual = w & 0xfffff000 == 0x159f2000
            && sequence(prefix, at - 12, &[0xe5913000, 0xe3530000])
            && sequence(prefix, at + 4, &[0xe0801005, 0xe0822006, 0xe1a03007]);
        let single = w & 0xfffff000 == 0xe59f2000
            && word(prefix, at - 4)? == 0xe0801005
            && sequence(
                prefix,
                at + 4,
                &[0xe0822006, 0xe1a03007, 0xe4924004, 0xe4814004, 0xe2533001],
            );
        if !dual && !single {
            continue;
        }
        let pointer = word(prefix, at + 8 + (w as usize & 0xfff))? as usize;
        let Some(native) = pointer.checked_sub(0xf0000000) else {
            continue;
        };
        let at = bias + native;
        let header = extent(data, at, 0x800)?;
        if !sequence(header, 0, &[0xffffffff, 0, 0x55555555, 0xaaaaaaaa]) {
            continue;
        }
        let count = word(header, 0x3c)? as usize;
        if count > 56 || 0x40 + count * 8 > 0x200 {
            continue;
        }
        if (0..count).any(|i| {
            word(header, 0x40 + i * 8).is_ok_and(|a| !(0xf0000000..0xfffff000).contains(&a))
        }) {
            continue;
        }
        found.push(at);
    }
    require(
        found.len() == 1,
        "No unique Agate DDR configuration selected by stock copy helper",
    )?;
    Ok(found[0])
}
fn xpp_edits(code: &[u8], table: &Table) -> Result<Vec<Edit>> {
    let mut found = vec![];
    for at in (0..code.len().saturating_sub(0x120)).step_by(4) {
        let Ok(flag) = address(code, at, 3) else {
            continue;
        };
        if !sequence(code, at + 8, &[0xe5933000, 0xe3530001])
            || address(code, at + 16, 3).ok() != Some(table.address)
            || !sequence(code, at + 24, &[0x13a02001, 0x15832008])
        {
            continue;
        }
        // The root pointer construction uses conditional MOVW/MOVT.
        let tests: Vec<_> = (at..(at + 0x100).min(code.len() - 16))
            .step_by(4)
            .filter(|p| {
                address(code, *p, 3).ok() == Some(flag)
                    && sequence(code, *p + 8, &[0xe5933000, 0xe3530001])
            })
            .collect();
        if tests.len() != 3 {
            continue;
        }
        let loads = tests[2] + 36;
        if (0..3).any(|i| call(code, loads + i * 4).is_err()) {
            continue;
        }
        found.push(tests);
    }
    require(
        found.len() == 1,
        "No unique Agate cold/preloaded table, header and image branches",
    )?;
    let mut edits: Vec<Edit> = found
        .remove(0)
        .into_iter()
        .map(|at| {
            edit(
                code,
                at + 12,
                0xe3530000,
                "select preloaded data while retaining cold reset mode",
            )
        })
        .collect::<Result<_>>()?;
    edits.push(xpp_flash_probe_edit(code)?);
    Ok(edits)
}

/// Cold startup probes the installed flash before testing the preloaded flag.
/// Keep controller initialization, but take the detector's existing no-match
/// configuration without consulting signatures or their pointed-to data.
fn xpp_flash_probe_edit(code: &[u8]) -> Result<Edit> {
    let mut probes = vec![];
    for at in (0..code.len().saturating_sub(0xdc)).step_by(4) {
        if !sequence(code, at, &[0xe92d4878, 0xe28db014, 0xe3a0020f])
            || !sequence(code, at + 0x58, &[0xe3a00014, 0xe34f0000])
            || !sequence(code, at + 0x68, &[0xe2803201, 0xe3530301])
            || word(code, at + 0x18)? != 0xe3a0024f
            || word(code, at + 0x28)? != 0xe3a0028f
            || word(code, at + 0x40)? != 0xe3a002cf
            || word(code, at + 0x98)? != 0xe3540e19
            || !sequence(
                code,
                at + 0xcc,
                &[0xe3a00000, 0xe8bd8878, 0xe3a00001, 0xe8bd8878],
            )
        {
            continue;
        }
        let reads = [0x0c, 0x1c, 0x2c, 0x44, 0x60, 0x80];
        let Ok(reader) = call(code, at + reads[0]) else {
            continue;
        };
        if reads
            .iter()
            .all(|offset| call(code, at + offset).ok() == Some(reader))
        {
            probes.push(at);
        }
    }
    require(probes.len() == 1, "No unique Agate flash signature probe")?;
    let mut callers = vec![];
    for at in (0..code.len().saturating_sub(28)).step_by(4) {
        let Ok(detector) = call(code, at) else {
            continue;
        };
        if address(code, at + 4, 3).is_err()
            || word(code, at + 12)? != 0xe58305bc
            || !sequence(code, detector, &[0xe92d4818, 0xe28db00c])
        {
            continue;
        }
        let direct = sequence(code, at + 16, &[0xe3500002]) && word(code, at + 24)? == 0xe3500003;
        let indexed = sequence(code, at + 16, &[0xe2403001, 0xe3530005, 0x979ff103]);
        let probe_calls = (detector..(detector + 0x200).min(code.len().saturating_sub(4)))
            .step_by(4)
            .filter(|p| call(code, *p).ok() == Some(probes[0]))
            .count();
        if (direct || indexed) && probe_calls >= 6 {
            callers.push(at);
        }
    }
    require(
        callers.len() == 1,
        "No unique Agate cold flash-mode selection",
    )?;
    edit(
        code,
        callers[0],
        0xe3a00000,
        "skip installed-flash signature probe for preloaded startup",
    )
}
fn exr_edits(code: &[u8], table: &Table, flash: usize, nand_free: bool) -> Result<Vec<Edit>> {
    let mut wrappers = vec![];
    for at in (0..code.len().saturating_sub(28)).step_by(4) {
        if address(code, at, 0).ok() == Some(flash)
            && address(code, at + 8, 1).ok() == Some(table.address)
            && word(code, at + 16)? == 0xe3a02001
            && call(code, at + 20).is_ok()
        {
            wrappers.push(at - 8);
        }
    }
    require(
        wrappers.len() == 1,
        "No unique EXR section table read wrapper",
    )?;
    let callers: Vec<_> = (0..code.len().saturating_sub(4))
        .step_by(4)
        .filter(|at| call(code, *at).ok() == Some(wrappers[0]))
        .collect();
    require(
        callers.len() == 1,
        "EXR table reader has ambiguous callers",
    )?;
    let mut pairs = vec![];
    for at in (0..code.len().saturating_sub(56)).step_by(4) {
        if !sequence(code, at, &[0xe5960014, 0xe5961004])
            || !sequence(code, at + 12, &[0xe5940014, 0xe5941004])
        {
            continue;
        }
        if call(code, at + 8).ok() != call(code, at + 20).ok() || call(code, at + 8).is_err() {
            continue;
        }
        // Cold resident-stream load follows the boot-status test.
        if sequence(code, at + 24, &[0xe1a04000])
            && sequence(code, at + 40, &[0xe3530000])
            && word(code, at + 44)? >> 24 == 0x1a
            && call(code, at + 48).is_ok()
            && word(code, at + 52)? == 0xe1a04000
        {
            pairs.push([at + 8, at + 20, at + 48]);
        }
    }
    require(
        pairs.len() == 1,
        "No unique EXR raw-kernel pair and resident stream load",
    )?;
    let mut result = vec![edit(
        code,
        callers[0],
        0xe3a00000,
        "retain preloaded section table",
    )?];
    for at in pairs.remove(0) {
        result.push(edit(
            code,
            at,
            0xe3a00000,
            "retain preloaded stock kernel/application",
        )?);
    }
    if nand_free {
        result.extend(crate::exr_loader::edits(code, callers[0])?);
    }
    Ok(result)
}

/// Locate mappings and packaging from fresh update bytes, without a body ID.
pub fn locate(processor: Processor, data: &[u8]) -> Result<Located> {
    let table = match table(data) {
        Ok(table) => table,
        Err(_) if processor == Processor::Exr => return crate::exr_load_maps::locate(data),
        Err(error) => return Err(error),
    };
    let vector: Vec<_> = (0..8).flat_map(|_| 0xe59ff018u32.to_le_bytes()).collect();
    let loaders: Vec<_> = (0..table.offset)
        .step_by(4)
        .filter(|at| data.get(*at..*at + 32) == Some(vector.as_slice()))
        .collect();
    require(!loaders.is_empty(), "No low-RAM vector-bearing loader")?;
    require(
        loaders.iter().all(|at| {
            word(data, *at + 32).is_ok_and(|entry| {
                (at - loaders[0]..at - loaders[0] + 0x10000).contains(&(entry as usize))
            })
        }),
        "Secondary loader reset targets disagree with their RAM placement",
    )?;
    let loader = loaders[0];
    let mut biases = vec![];
    for row in &table.rows {
        if row[8] != 0 || row[1] == 0 || row[2] < 32 {
            continue;
        }
        for at in (0..data.len().saturating_sub(32)).step_by(4) {
            if sequence(data, at, &[0, row[1], row[2]])
                && word(data, at + 28)? == 0xaaaaffff
                && let Some(bias) = at.checked_sub(row[5] as usize)
            {
                biases.push(bias);
            }
        }
    }
    require(
        !biases.is_empty() && biases.iter().all(|b| *b == biases[0]),
        "Raw kernel headers do not establish one flash/DAT bias",
    )?;
    let bias = biases[0];
    require(bias < loader, "DAT bias must precede the loader")?;
    let first = table
        .rows
        .iter()
        .filter(|r| {
            r[8] == 0
                && r[1] != 0
                && r[2] >= 32
                && word(data, bias + r[5] as usize + 28).ok() == Some(0xaaaaffff)
        })
        .map(|r| bias + r[5] as usize)
        .min()
        .unwrap();
    require(
        loader < first && first < table.offset,
        "Loader prefix is not bounded before the raw kernel/table",
    )?;
    let (spec, code) = source(data, loader, first - loader, false)?;
    let mut regions = vec![
        Region {
            name: "loader".into(),
            address: 0,
            source: spec,
        },
        Region {
            name: "section-table".into(),
            address: table.address,
            source: source(data, table.offset, 0x400, false)?.0,
        },
    ];
    let mut bss = vec![];
    let mut arenas = 0;
    for row in &table.rows {
        let [
            id,
            dest,
            len,
            zero,
            zero_len,
            flash,
            allocated,
            header,
            compressed,
        ] = *row;
        let extent = if dest == 0 && len != 0 && compressed == 0 {
            table.arena_size
        } else {
            allocated as usize
        };
        let (mut spec, decoded) = source(data, bias + flash as usize, extent, compressed == 1)?;
        let target = if dest == 0 && len != 0 {
            require(
                decoded.len() == table.arena_size,
                "Overlay arena extent differs",
            )?;
            arenas += 1;
            table.arena
        } else if dest != 0 && len != 0 {
            if compressed == 0 {
                spec = source(data, bias + flash as usize, len as usize, false)?.0;
            }
            if header != 0 {
                require(
                    sequence(&decoded, 0, &[0, dest, len]) && word(&decoded, 28)? == 0xaaaaffff,
                    "Section inner header differs",
                )?;
            }
            if zero_len > 0 {
                bss.push(Bss {
                    name: format!("section-{id}"),
                    address: zero as usize,
                    size: zero_len as usize,
                });
            }
            dest as usize
        } else {
            continue;
        };
        regions.push(Region {
            name: format!("section-{id}"),
            address: target,
            source: spec,
        });
    }
    require(arenas == 1, "No unique overlay arena section")?;
    let mut edits = if processor == Processor::Xpp {
        xpp_edits(&code, &table)?
    } else {
        exr_edits(
            &code,
            &table,
            table.offset - bias,
            processor == Processor::Exr,
        )?
    };
    edits.push(Edit {
        address: table.address + 8,
        old: word(data, table.offset + 8)?,
        new: 1,
        what: "preloaded section table".into(),
    });
    let size = regions
        .iter()
        .map(|r| r.address + r.source.decoded_size)
        .chain(bss.iter().map(|r| r.address + r.size))
        .max()
        .unwrap();
    let plan = Plan {
        size,
        entry: 0,
        regions,
        bss,
        edits,
    };
    // Check overlap, BSS and every old instruction before publishing a recipe.
    image::transform(data, &plan)?;
    let header = if processor == Processor::Xpp {
        xpp_header(data, bias, loader)?
    } else {
        bias
    };
    let header_size = if processor == Processor::Xpp {
        0x800
    } else {
        0x200
    };
    let bytes = extent(data, header, header_size)?;
    let signature = if processor == Processor::Xpp {
        vec![0xffffffff, 0, 0x55555555, 0xaaaaaaaa]
    } else {
        vec![0x00ffaa55, 0x01234567, 0x89abcdef]
    };
    require(
        sequence(bytes, 0, &signature),
        "Boot header signatures differ",
    )?;
    let header_edits = if processor == Processor::Xpp {
        vec![]
    } else {
        vec![Edit {
            address: 0x10,
            old: word(bytes, 0x10)?,
            new: word(bytes, 0x10)? | 1,
            what: "hold power through SD load".into(),
        }]
    };
    Ok(Located {
        plan,
        packaging: Packaging {
            header_parts: vec![Part {
                dat_offset: header,
                size: header_size,
            }],
            header_sha256: fujimod_firmware::sha256(bytes),
            header_words: signature
                .into_iter()
                .enumerate()
                .map(|(i, value)| HeaderWord {
                    offset: i * 4,
                    value,
                })
                .collect(),
            edits: header_edits,
            header_filename: "ffsys.bin".into(),
            image_filename: "ffboot.bin".into(),
            entry: 0,
            load_address: 0,
        },
        missing: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flash_probe_loader() -> Vec<u8> {
        let mut code = vec![0; 0x1000];
        let branch = |at: usize, target: usize| {
            0xeb000000 | (((target as i32 - at as i32 - 8) >> 2) as u32 & 0xffffff)
        };
        for (at, value) in [
            (0x80, branch(0x80, 0x200)),
            (0x84, 0xe3003000),
            (0x88, 0xe3403001),
            (0x8c, 0xe58305bc),
            (0x90, 0xe2403001),
            (0x94, 0xe3530005),
            (0x98, 0x979ff103),
            (0x200, 0xe92d4818),
            (0x204, 0xe28db00c),
            (0x400, 0xe92d4878),
            (0x404, 0xe28db014),
            (0x408, 0xe3a0020f),
            (0x418, 0xe3a0024f),
            (0x428, 0xe3a0028f),
            (0x440, 0xe3a002cf),
            (0x458, 0xe3a00014),
            (0x45c, 0xe34f0000),
            (0x468, 0xe2803201),
            (0x46c, 0xe3530301),
            (0x498, 0xe3540e19),
            (0x4cc, 0xe3a00000),
            (0x4d0, 0xe8bd8878),
            (0x4d4, 0xe3a00001),
            (0x4d8, 0xe8bd8878),
        ] {
            image::put(&mut code, at, value).unwrap();
        }
        for offset in [0x0c, 0x1c, 0x2c, 0x44, 0x60, 0x80] {
            image::put(&mut code, 0x400 + offset, branch(0x400 + offset, 0x600)).unwrap();
        }
        for at in (0x210..0x228).step_by(4) {
            image::put(&mut code, at, branch(at, 0x400)).unwrap();
        }
        code
    }
    #[test]
    fn cold_flash_probe_requires_the_signature_reader_and_its_mode_consumer() {
        let code = flash_probe_loader();
        let edit = xpp_flash_probe_edit(&code).unwrap();
        assert_eq!((edit.address, edit.new), (0x80, 0xe3a00000));
        for at in [
            0x80, 0x84, 0x8c, 0x94, 0x200, 0x210, 0x408, 0x41c, 0x45c, 0x498,
        ] {
            let mut bad = code.clone();
            image::put(&mut bad, at, 0).unwrap();
            assert!(
                xpp_flash_probe_edit(&bad).is_err(),
                "accepted changed word {at:#x}"
            );
        }
        let mut ambiguous = code.clone();
        ambiguous.copy_within(0x400..0x4dc, 0x700);
        // Relocate the six calls in the second candidate to the same reader.
        for offset in [0x0c, 0x1c, 0x2c, 0x44, 0x60, 0x80] {
            let at = 0x700 + offset;
            image::put(
                &mut ambiguous,
                at,
                0xeb000000 | (((0x600i32 - at as i32 - 8) >> 2) as u32 & 0xffffff),
            )
            .unwrap();
        }
        assert!(xpp_flash_probe_edit(&ambiguous).is_err());
    }
    fn rooted_table() -> Vec<u8> {
        let mut data = vec![0; 0x800];
        for (at, value) in [
            (0, 0x80040),
            (4, 0x80010),
            (12, 0x80088),
            (0x10, 0x803a0),
            (0x14, 0x100000),
            (0x18, 0x1000),
            (0x40, 0),
            (0x44, 0x40000),
            (0x48, 0x400),
            (0x64, 0xff),
            (0x88, 0x1234),
            (0x3a0, 1),
            (0x3a4, 0x100000),
            (0x3a8, 0x100),
            (0x3b8, 0xff),
        ] {
            image::put(&mut data, at, value).unwrap();
        }
        data
    }
    #[test]
    fn section_roots_complete_sentinel_and_overlay_bounds_are_required() {
        let data = rooted_table();
        let located = table(&data).unwrap();
        assert_eq!(
            (located.address, located.arena, located.rows.len()),
            (0x80000, 0x100000, 1)
        );
        for (at, value) in [(12, 0x80084), (0x68, 1), (0x3a8, 0x1001), (0x3b8, 0)] {
            let mut changed = data.clone();
            image::put(&mut changed, at, value).unwrap();
            assert!(
                table(&changed).is_err(),
                "accepted changed table word {at:#x}"
            );
        }
    }
    #[test]
    fn header_selection_follows_both_stock_copy_helper_forms() {
        for dual in [false, true] {
            let mut data = vec![0; 0x2000];
            let words: Vec<u32> = if dual {
                vec![
                    0xe5913000, 0xe3530000, 0x059f2000, 0x159f2040, 0xe0801005, 0xe0822006,
                    0xe1a03007,
                ]
            } else {
                vec![
                    0xe0801005, 0xe59f2048, 0xe0822006, 0xe1a03007, 0xe4924004, 0xe4814004,
                    0xe2533001,
                ]
            };
            for (i, w) in words.into_iter().enumerate() {
                image::put(&mut data, 0x100 + i * 4, w).unwrap();
            }
            image::put(&mut data, 0x154, 0xf0000800).unwrap();
            for (i, w) in [0xffffffff, 0, 0x55555555, 0xaaaaaaaa]
                .into_iter()
                .enumerate()
            {
                image::put(&mut data, 0x800 + i * 4, w).unwrap();
            }
            assert_eq!(xpp_header(&data, 0, 0x2000).unwrap(), 0x800);
            image::put(&mut data, 0x83c, 57).unwrap();
            assert!(xpp_header(&data, 0, 0x2000).is_err());
        }
    }
}
