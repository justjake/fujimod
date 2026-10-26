//! EXR five-word and raw-chain load maps checked through their stock loaders.
use crate::{
    image::{self, Bss, Edit, HeaderWord, Packaging, Part, Plan, Region, Result, require, word},
    rom_sd::{Located, address, call, edit, sequence, source},
};

fn scalar(code: &[u8], at: usize, register: u32) -> Result<usize> {
    if let Ok(value) = address(code, at, register) {
        return Ok(value);
    }
    let instruction = word(code, at)?;
    require(
        instruction & 0xfffff000 == 0xe59f0000 | (register << 12),
        "Expected literal-loaded EXR address",
    )?;
    Ok(word(code, at + 8 + (instruction as usize & 0xfff))? as usize)
}

pub(super) fn locate(data: &[u8]) -> Result<Located> {
    let vectors: Vec<u8> = (0..8).flat_map(|_| 0xe59ff018u32.to_le_bytes()).collect();
    let loader = (0..data.len().saturating_sub(32))
        .step_by(4)
        .find(|at| data.get(*at..*at + 32) == Some(vectors.as_slice()))
        .ok_or_else(|| image::Error::Invalid("No older EXR vector-bearing loader".into()))?;
    let headers: Vec<_> = (0..loader)
        .step_by(4)
        .filter(|at| sequence(data, *at, &[0x00ffaa55, 0x01234567, 0x89abcdef]))
        .collect();
    require(!headers.is_empty(), "No older EXR boot header bank")?;
    let bias = headers[0];
    require(
        headers.iter().enumerate().all(|(i, at)| {
            *at == bias + i * 512 && data.get(*at..*at + 512) == data.get(bias..bias + 512)
        }) && headers.last().unwrap() + 512 == loader,
        "Older EXR boot header replicas differ",
    )?;
    let kernels: Vec<_> = (loader + 32..data.len().min(loader + 0x100000).saturating_sub(32))
        .step_by(4)
        .filter(|at| {
            word(data, *at).ok() == Some((*at - bias) as u32)
                && word(data, *at + 28).ok() == Some(0xaaaaffff)
                && word(data, *at + 12).ok() == word(data, *at + 4).ok().map(|v| v + 32)
                && matches!(word(data, *at + 4).ok(), Some(0x40000 | 0xd0000))
        })
        .collect();
    require(kernels.len() == 2, "No unique older EXR raw kernel pair")?;
    let first = kernels[0];
    let (loader_source, code) = source(data, loader, first - loader, false)?;
    let mut wrappers = vec![];
    for at in (8..code.len().saturating_sub(24)).step_by(4) {
        if word(&code, at + 16)? != 0xe3a02001 || call(&code, at + 20).is_err() {
            continue;
        }
        let (Ok(flash), Ok(destination)) = (address(&code, at, 0), address(&code, at + 8, 1))
        else {
            continue;
        };
        let offset = bias + flash;
        if offset + 0x400 > data.len() {
            continue;
        }
        let row: [u32; 5] = std::array::from_fn(|i| word(data, offset + i * 4).unwrap());
        if row[4] == 1
            && row[0] != 0
            && row[1] > 32
            && (row[2] as usize & !0x40000000) == (destination & !0x40000000) + 0x400
        {
            wrappers.push((at - 8, offset, destination & !0x40000000));
        }
    }
    if wrappers.is_empty() {
        return locate_raw(data, bias, loader, first, &kernels);
    }
    require(
        !wrappers.is_empty()
            && wrappers
                .iter()
                .all(|r| r.1 == wrappers[0].1 && r.2 == wrappers[0].2),
        "No unique five-word table selected by older EXR loaders",
    )?;
    let (_, offset, table_address) = wrappers[0];
    let mut regions = vec![
        Region {
            name: "loader".into(),
            address: 0,
            source: loader_source,
        },
        Region {
            name: "section-table".into(),
            address: table_address,
            source: source(data, offset, 0x400, false)?.0,
        },
    ];
    for (index, at) in kernels.iter().enumerate() {
        let destination = word(data, *at + 4)? as usize;
        let size = word(data, *at + 8)? as usize;
        require(
            size >= 32 && size < 0x10000,
            "Older EXR raw kernel extent differs",
        )?;
        regions.push(Region {
            name: format!("kernel-{index}"),
            address: destination,
            source: source(data, *at, size, false)?.0,
        });
    }
    let mut bss = vec![];
    let mut rows = vec![];
    let tail = &data[offset + 8 * 20..offset + 0x400];
    let padding = if tail[..20].iter().all(|value| *value == 0) {
        &tail[20..]
    } else {
        tail
    };
    require(
        padding.iter().all(|value| *value == 255),
        "Older EXR table has additional or malformed rows",
    )?;
    for index in 0..8 {
        let row: [u32; 5] =
            std::array::from_fn(|i| word(data, offset + index * 20 + i * 4).unwrap());
        require(row[4] <= 1, "Older EXR row compression differs")?;
        if rows.contains(&row) {
            continue;
        }
        rows.push(row);
        let [flash, packed, destination, size, compressed] = row;
        let destination = (destination & !0x40000000) as usize;
        if flash == 0 && packed == 0 {
            if size != 0 {
                bss.push(Bss {
                    name: format!("row-{index}"),
                    address: destination,
                    size: size as usize,
                });
            }
            continue;
        }
        require(
            destination != 0 && packed != 0,
            "Older EXR load row has no destination/extent",
        )?;
        let (spec, decoded) = source(
            data,
            bias + flash as usize,
            packed as usize,
            compressed == 1,
        )?;
        if compressed == 1 {
            require(
                decoded.len() == size as usize
                    && sequence(&decoded, 0, &[0, destination as u32, size])
                    && word(&decoded, 28)? == 0xaaaaffff,
                "Older EXR decoded header/extent differs",
            )?;
        } else {
            require(
                size == 0,
                "Older EXR raw resource row has an unexplained decoded extent",
            )?;
        }
        regions.push(Region {
            name: format!("row-{index}"),
            address: destination,
            source: spec,
        });
    }
    require(
        regions.len() == 8 && bss.len() == 2,
        "Older EXR five-word map is incomplete",
    )?;
    let mut edits = vec![];
    for (wrapper, _, _) in &wrappers {
        let callers: Vec<_> = (0..code.len().saturating_sub(4))
            .step_by(4)
            .filter(|at| call(&code, *at).ok() == Some(*wrapper))
            .collect();
        require(
            callers.len() == 1,
            "Older EXR table wrapper callers differ",
        )?;
        edits.push(edit(
            &code,
            callers[0],
            0xe3a00000,
            "retain preloaded five-word table",
        )?);
        edits.extend(five_word_nand_edits(&code, callers[0])?);
    }
    // Both core images declare their raw kernel load with adjacent R0/R1
    // literal loads. The resident-stream path follows a boot-status test.
    let mut kernel_calls = vec![];
    for at in (8..code.len().saturating_sub(4)).step_by(4) {
        if call(&code, at).is_err() {
            continue;
        }
        if kernels.iter().any(|offset| {
            [(8, 4), (16, 8)].iter().any(|&(a, b)| {
                at >= a
                    && scalar(&code, at - a, 0).ok() == Some(*offset - bias)
                    && scalar(&code, at - b, 1).ok()
                        == word(data, *offset + 4).ok().map(|v| v as usize)
            })
        }) {
            kernel_calls.push(at);
        }
    }
    require(
        kernel_calls.len() == 2,
        "Older EXR kernel calls are not uniquely checked",
    )?;
    for at in kernel_calls {
        edits.push(edit(
            &code,
            at,
            0xe3a00000,
            "retain preloaded older raw kernel",
        )?);
    }
    // The stream reader is uniquely a no-argument call followed by CMP and a
    // conditional load of the resident header, on each core.
    let mut streams = vec![];
    for at in (4..code.len().saturating_sub(16)).step_by(4) {
        if call(&code, at).is_ok()
            && word(&code, at + 4)? == 0xe3500000
            && word(&code, at + 8)? & 0xffff0000 == 0x059f0000
            && word(&code, at + 12)? >> 24 == 0x0a
        {
            streams.push(at);
        }
    }
    require(
        streams.len() == 2,
        "Older EXR resident-stream loads are not uniquely checked",
    )?;
    for at in streams {
        edits.push(edit(
            &code,
            at,
            0xe3a00000,
            "retain preloaded older resident stream",
        )?);
    }
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
    image::transform(data, &plan)?;
    let header = &data[bias..bias + 512];
    Ok(Located {
        plan,
        packaging: Packaging {
            header_parts: vec![Part {
                dat_offset: bias,
                size: 512,
            }],
            header_sha256: fujimod_firmware::sha256(header),
            header_words: [0x00ffaa55, 0x01234567, 0x89abcdef]
                .into_iter()
                .enumerate()
                .map(|(i, value)| HeaderWord {
                    offset: i * 4,
                    value,
                })
                .collect(),
            edits: vec![Edit {
                address: 16,
                old: word(header, 16)?,
                new: word(header, 16)? | 1,
                what: "hold power through SD load".into(),
            }],
            header_filename: "ffsys.bin".into(),
            image_filename: "ffboot.bin".into(),
            entry: 0,
            load_address: 0,
        },
        missing: vec!["five-word EXR stock updater acceptance".into()],
    })
}

fn five_word_nand_edits(code: &[u8], table_call: usize) -> Result<Vec<Edit>> {
    let lo = (table_call.saturating_sub(0x400)..table_call)
        .rev()
        .find(|at| at % 4 == 0 && word(code, *at).is_ok_and(|w| w & 0xffffc800 == 0xe92d4800))
        .ok_or_else(|| image::Error::Invalid("No older EXR cold-start frame".into()))?;
    let hi = (table_call..(table_call + 0x200).min(code.len()))
        .step_by(4)
        .find(|at| word(code, *at).is_ok_and(|w| w & 0xffff8000 == 0xe8bd8000))
        .ok_or_else(|| image::Error::Invalid("No older EXR cold-start return".into()))?;
    let mut initializes = vec![];
    let mut scans = vec![];
    let mut probes = vec![];
    let mut closes = vec![];
    for at in (lo..hi).step_by(4) {
        let Ok(target) = call(code, at) else {
            continue;
        };
        if sequence(code, target, &[0xe92d4800, 0xe28db004]) && call(code, target + 8).is_ok() {
            if word(code, target + 12)? == 0xe8bd8800 && word(code, at + 4)? == 0xe3500000 {
                initializes.push(at);
            }
            if sequence(code, target + 12, &[0xe3a00000, 0xe8bd8800]) {
                closes.push(at);
            }
        }
        if sequence(code, target, &[0xe92d4800, 0xe28db004, 0xe24dd008]) {
            if sequence(code, target + 12, &[0xe3a03000, 0xe50b300c])
                && sequence(code, target + 0x24, &[0xe5813000, 0xe24b000c])
                && call(code, target + 0x2c).is_ok()
            {
                scans.push(target + 0x2c);
            } else if address(code, target + 12, 3).is_ok()
                && address(code, target + 20, 1).is_ok()
                && sequence(code, target + 28, &[0xe5831000, 0xe3a03000, 0xe50b300c])
                && sequence(code, target + 44, &[0xe50b2008, 0xe5813000, 0xe24b000c])
                && call(code, target + 56).is_ok()
            {
                scans.push(target + 56);
            }
        }
        if sequence(code, target, &[0xe92d4810, 0xe28db008])
            && (target + 8..(target + 0x60).min(code.len().saturating_sub(20)))
                .step_by(4)
                .any(|p| {
                    sequence(
                        code,
                        p,
                        &[0xe1d301be, 0xe30a3aaa, 0xe0500003, 0x13a00001, 0xe8bd8810],
                    )
                })
        {
            probes.push(at);
        }
    }
    require(
        initializes.len() == 1 && scans.len() == 1 && probes.len() == 1 && closes.len() == 1,
        "Older EXR NAND lifecycle is not uniquely checked in its cold-start frame",
    )?;
    [
        (initializes[0], "skip five-word EXR NAND initialization"),
        (
            scans[0],
            "retain empty five-word EXR bad-block list without OOB scan",
        ),
        (
            probes[0],
            "skip five-word EXR installed-image header probe",
        ),
        (closes[0], "skip five-word EXR NAND shutdown"),
    ]
    .into_iter()
    .map(|(at, what)| edit(code, at, 0xe3a00000, what))
    .collect()
}

/// The oldest variant loads contiguous raw application headers rather than
/// a five-word compressed-stream table. Follow the checked header chain and
/// retain the existing normal-application handoff, suppressing NAND probes.
fn locate_raw(
    data: &[u8],
    bias: usize,
    loader: usize,
    first: usize,
    kernels: &[usize],
) -> Result<Located> {
    let mut chains = vec![];
    for at in (first..data.len().saturating_sub(32)).step_by(4) {
        let destination = word(data, at + 4)? as usize;
        let size = word(data, at + 8)? as usize;
        if size < 32
            || destination <= 0xd0000
            || word(data, at)? as usize != at - bias
            || word(data, at + 12)? as usize != destination + 32
            || word(data, at + 28)? != 0xaaaaffff
            || word(data, at + 20)? == 0
        {
            continue;
        }
        let Some(next) = at.checked_add(size).filter(|v| v + 32 < data.len()) else {
            continue;
        };
        if word(data, next)? as usize == next - bias
            && word(data, next + 4)? as usize == destination + size
            && word(data, next + 12)? as usize == destination + size + 32
            && word(data, next + 28)? == 0xaaaaffff
        {
            chains.push((at, next));
        }
    }
    require(
        chains.len() == 1,
        "No unique older EXR raw application header chain",
    )?;
    let (main, extension) = chains[0];
    let main_flash = main - bias;
    let destination = word(data, main + 4)? as usize;
    let (loader_source, code) = source(data, loader, first - loader, false)?;
    let mut regions = vec![Region {
        name: "loader".into(),
        address: 0,
        source: loader_source,
    }];
    for (name, at) in kernels
        .iter()
        .enumerate()
        .map(|(i, at)| (format!("kernel-{i}"), *at))
        .chain([
            ("application".into(), main),
            ("application-extension".into(), extension),
        ])
    {
        let size = word(data, at + 8)? as usize;
        require(size >= 32, "Older EXR raw image is truncated")?;
        regions.push(Region {
            name,
            address: word(data, at + 4)? as usize,
            source: source(data, at, size, false)?.0,
        });
    }
    let mut edits = vec![];
    let mut kernel_calls = vec![];
    let mut resident_calls = vec![];
    for at in (20..code.len().saturating_sub(4)).step_by(4) {
        if call(&code, at).is_err() {
            continue;
        }
        if kernels.iter().any(|offset| {
            address(&code, at - 16, 0).ok() == Some(*offset - bias)
                && address(&code, at - 8, 1).ok()
                    == word(data, *offset + 4).ok().map(|v| v as usize)
        }) {
            kernel_calls.push(at);
        }
        if address(&code, at - 20, 0).ok() == Some(main_flash)
            && address(&code, at - 12, 1).ok() == Some(destination)
            && word(&code, at - 4)? == 0xe3a02000
        {
            resident_calls.push(at);
        }
    }
    require(
        kernel_calls.len() == 2 && resident_calls.len() == 2,
        "Older raw EXR loader image calls differ",
    )?;
    for at in kernel_calls.iter().chain(&resident_calls) {
        edits.push(edit(
            &code,
            *at,
            0xe3a00000,
            "retain preloaded raw EXR kernel/application",
        )?);
    }
    let mut lifecycle = vec![];
    for at in (0..code.len().saturating_sub(0x80)).step_by(4) {
        if call(&code, at).is_err()
            || !sequence(&code, at + 4, &[0xe3500000, 0x0a000004])
            || call(&code, at + 0x20).is_err()
            || !sequence(&code, at + 0x24, &[0xe3500000, 0x0a000004])
            || !sequence(&code, at + 0x40, &[0xe3540000, 0x13a00000])
        {
            continue;
        }
        let Ok(helper) = call(&code, at + 0x58) else {
            continue;
        };
        if !kernel_calls.contains(&(helper + 0x20))
            || call(&code, at + 0x50).is_err()
            || call(&code, at + 0x78).is_err()
        {
            continue;
        }
        let setup = call(&code, at + 0x20)?;
        require(
            sequence(
                &code,
                setup,
                &[0xe92d4800, 0xe28db004, 0xe24dd008, 0xe3a03000, 0xe50b300c],
            ) && sequence(&code, setup + 0x24, &[0xe5813000, 0xe24b000c])
                && call(&code, setup + 0x2c).is_ok(),
            "Older raw EXR empty bad-block list setup differs",
        )?;
        lifecycle.push(at);
        for (site, what) in [
            (at, "skip raw EXR NAND initialization"),
            (
                setup + 0x2c,
                "retain empty raw EXR bad-block list without OOB scan",
            ),
            (
                at + 0x50,
                "retain normal preloaded raw EXR application policy without NAND probes",
            ),
            (at + 0x78, "skip raw EXR NAND shutdown"),
        ] {
            edits.push(edit(&code, site, 0xe3a00000, what)?);
        }
    }
    require(
        lifecycle.len() == 2,
        "Older raw EXR NAND lifecycle is not checked for both cores",
    )?;
    let size = regions
        .iter()
        .map(|r| r.address + r.source.decoded_size)
        .max()
        .unwrap();
    let plan = Plan {
        size,
        entry: 0,
        regions,
        bss: vec![],
        edits,
    };
    image::transform(data, &plan)?;
    let header = &data[bias..bias + 512];
    Ok(Located {
        plan,
        packaging: Packaging {
            header_parts: vec![Part {
                dat_offset: bias,
                size: 512,
            }],
            header_sha256: fujimod_firmware::sha256(header),
            header_words: [0x00ffaa55, 0x01234567, 0x89abcdef]
                .into_iter()
                .enumerate()
                .map(|(i, value)| HeaderWord {
                    offset: i * 4,
                    value,
                })
                .collect(),
            edits: vec![Edit {
                address: 16,
                old: word(header, 16)?,
                new: word(header, 16)? | 1,
                what: "hold power through SD load".into(),
            }],
            header_filename: "ffsys.bin".into(),
            image_filename: "ffboot.bin".into(),
            entry: 0,
            load_address: 0,
        },
        missing: vec!["raw EXR stock updater acceptance".into()],
    })
}
