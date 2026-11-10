//! Agate's resident executable transfers and unchanged stock data-read paths.
use crate::{
    Proof,
    image::{self, Plan, Result, extent, hash, put, require, word},
    rom_sd::{call, sequence},
};
use serde::{Deserialize, Serialize};
const IMAGE: &[u8] = include_bytes!("../assets/xpp-preloaded.generated.bin");
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    schema: usize,
    link_address: usize,
    size: usize,
    config: usize,
    preloaded: usize,
    config_reference: usize,
}
#[derive(Debug, Serialize)]
pub struct Preload {
    pub caller: Proof,
    pub call: Proof,
    pub block: usize,
    pub count: usize,
    pub output: usize,
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
    pub preloads: Vec<Preload>,
    pub padding: Vec<image::Region>,
    pub cache_maintenance: Proof,
    pub enabled_byte: usize,
    pub preloaded_byte: usize,
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
    // Keep the stock progress wrapper and its cache-publication operation.
    let mut progress = Vec::new();
    let mut mains = Vec::new();
    for region in &plan.regions {
        for at in (region.address
            ..(region.address + region.source.decoded_size).saturating_sub(0x178))
            .step_by(4)
        {
            if sequence(
                ram,
                at,
                &[
                    0xe92d4ff0, 0xe28db020, 0xe24dd014, 0xe1a04000, 0xe1a08001, 0xe1a06002,
                    0xe1a07003,
                ],
            ) && call(ram, at + 0x80).ok() == Some(descriptor)
                && sequence(ram, at + 0x40, &[0xe1a00804])
                && call(ram, at + 0x44).ok() == Some(translator)
            {
                progress.push(at);
            }
            if sequence(ram, at, &[0xe92d49f0, 0xe28db018, 0xe24dd00c, 0xe3a00006]) {
                mains.push(at);
            }
        }
    }
    require(
        progress.len() == 1 && mains.len() == 1,
        "No unique Agate MAIN preload contract",
    )?;
    let progress = progress[0];
    let main = mains[0];
    let cache = call(ram, progress + 0x3c)?;
    require(
        sequence(
            ram,
            cache,
            &[
                0xe92d4800, 0xe28db004, 0xe24dd008, 0xe3a03000, 0xe50b300c, 0xe50b3008,
            ],
        ) && super::rom_sd::address(ram, cache + 0x18, 0).is_ok()
            && sequence(ram, cache + 0x20, &[0xe24b100c])
            && sequence(ram, cache + 0x28, &[0xe24bd004, 0xe8bd8800]),
        "Agate cache publication ABI differs",
    )?;
    let enabled = super::rom_sd::address(ram, progress + 0x1c, 3)?;
    require(
        sequence(ram, progress + 0x24, &[0xe5d33000, 0xe3530001]),
        "Agate firmware service gate differs",
    )?;
    let selector = call(ram, main + 0x8c)?;
    require(
        sequence(ram, selector, &[0xe52db004, 0xe28db000])
            && super::rom_sd::address(ram, selector + 8, 3)? == table.address
            && sequence(
                ram,
                selector + 0x10,
                &[0xe5d30008, 0xe24bd000, 0xe49db004, 0xe12fff1e],
            )
            && sequence(ram, main + 0x90, &[0xe3500000, 0x1a00001b])
            && sequence(
                ram,
                main + 0x108,
                &[0xe3a03001, 0xe58d3000, 0xe1a00008, 0xe1a01007],
            )
            && sequence(ram, main + 0x120, &[0xe3a03000])
            && call(ram, main + 0x124)? == progress
            && call(ram, main + 0xd4)? == progress
            && sequence(ram, main + 0x128, &[0xe1a06000, 0xe3550000, 0x0a000005])
            && sequence(ram, main + 0x16c, &[0xe1a00006, 0xe24bd018, 0xe8bd89f0]),
        "Agate preloaded MAIN branch differs",
    )?;
    let low = word(ram, main + 0x118)?;
    let scratch = if low & 0xfffff000 == 0xe3a02000 {
        (low & 255).rotate_right((low >> 8 & 15) * 2) as usize
    } else {
        super::rom_sd::address(ram, main + 0x118, 2)? & 0xffff
    };
    let high = word(ram, main + 0x11c)?;
    require(
        high & 0xfff0f000 == 0xe3402000,
        "Agate MAIN scratch address differs",
    )?;
    let scratch = scratch | (((high >> 4 & 0xf000) | (high & 0xfff)) as usize) << 16;
    let row = table.source.dat_offset + 0x40 + 6 * 36;
    require(
        word(inverted, row)? == 6 && word(inverted, row + 32)? == 1,
        "Agate MAIN is not the preloaded compressed section",
    )?;
    let block = word(inverted, row + 20)? as usize;
    let count = word(inverted, row + 24)? as usize;
    require(
        block.is_multiple_of(0x10000)
            && count > 0
            && block.checked_add(count).is_some_and(|end| end <= limit),
        "Agate MAIN flash extent differs",
    )?;
    let lookup = call(ram, main + 0x10)?;
    let mut preloads = vec![Preload {
        caller: pin(ram, main, 0x178)?,
        call: pin(ram, main + 0x124, 4)?,
        block: block >> 16,
        count: (count + 0xffff) >> 16,
        output: scratch,
    }];
    let mut padding = Vec::new();
    for id in [1, 2] {
        let mut raw = Vec::new();
        for region in &plan.regions {
            for at in (region.address
                ..(region.address + region.source.decoded_size).saturating_sub(0x4c))
                .step_by(4)
            {
                if sequence(
                    ram,
                    at,
                    &[0xe92d4800, 0xe28db004, 0xe24dd008, 0xe3a00000 | id],
                ) && call(ram, at + 16).ok() == Some(lookup)
                    && sequence(
                        ram,
                        at + 20,
                        &[
                            0xe2503000, 0x1a000000, 0xeafffffe, 0xe1d301b6, 0xe5931018, 0xe2811cff,
                            0xe28110ff, 0xe5932004, 0xe3a03001, 0xe58d3000, 0xe1a01821, 0xe3a03000,
                        ],
                    )
                    && call(ram, at + 0x44).ok() == Some(progress)
                    && sequence(ram, at + 0x48, &[0xe24bd004, 0xe8bd8800])
                {
                    raw.push(at);
                }
            }
        }
        require(
            raw.len() == 1,
            "No unique Agate raw-section preload contract",
        )?;
        require(
            raw[0] >= section.address
                && raw[0] + 0x50 <= section.address + section.source.decoded_size,
            "Agate raw loader is outside resident executable storage",
        )?;
        let row = table.source.dat_offset + 0x40 + id as usize * 36;
        let dest = word(inverted, row + 4)? as usize;
        let len = word(inverted, row + 8)? as usize;
        let flash = word(inverted, row + 20)? as usize;
        let allocated = word(inverted, row + 24)? as usize;
        require(
            word(inverted, row)? == id
                && word(inverted, row + 32)? == 0
                && len > 0
                && len <= allocated
                && flash.is_multiple_of(0x10000)
                && flash
                    .checked_add((allocated + 0xffff) & !0xffff)
                    .is_some_and(|end| end <= limit)
                && plan.regions.iter().any(|r| {
                    r.address == dest
                        && r.source.dat_offset == bias + flash
                        && r.source.decoded_size == len
                        && matches!(r.source.compression, image::Compression::Raw)
                }),
            "Agate raw preload source differs",
        )?;
        let transferred = (allocated + 0xffff) & !0xffff;
        if transferred > len {
            let address = dest + len;
            let size = transferred - len;
            require(
                plan.regions.iter().all(|r| {
                    address + size <= r.address || r.address + r.source.decoded_size <= address
                }) && plan
                    .bss
                    .iter()
                    .all(|r| address + size <= r.address || r.address + r.size <= address),
                "Agate raw allocation padding overlaps another image or BSS",
            )?;
            let dat_offset = bias + flash + len;
            let bytes = extent(inverted, dat_offset, size)?;
            padding.push(image::Region {
                name: format!("section-{id} allocation padding"),
                address,
                source: image::Source {
                    dat_offset,
                    size,
                    compression: image::Compression::Raw,
                    decoded_size: size,
                    sha256: fujimod_firmware::sha256(bytes),
                    initialized_prefix: None,
                },
            });
        }
        preloads.push(Preload {
            caller: pin(ram, raw[0], 0x50)?,
            call: pin(ram, raw[0] + 0x44, 4)?,
            block: flash >> 16,
            count: (allocated + 0xffff) >> 16,
            output: dest,
        });
    }
    Ok(Reads {
        byte_reader: pin(ram, found[0], 68)?,
        translator: pin(ram, translator, 24)?,
        copy_wrapper: pin(ram, copy, 52)?,
        descriptor_reader: pin(ram, descriptor, copy - descriptor)?,
        call_sites: calls,
        pointer_sites: pointers,
        dat_offset: bias,
        namespace_size: limit,
        preloads,
        padding,
        cache_maintenance: pin(ram, cache, 0x30)?,
        enabled_byte: enabled,
        preloaded_byte: table.address + 8,
    })
}
pub fn locate(inverted: &[u8], plan: &Plan) -> Result<Reads> {
    locate_ram(inverted, plan, &image::transform(inverted, plan)?)
}
pub(super) fn install(inverted: &[u8], plan: &Plan, ram: &mut [u8]) -> Result<()> {
    let reads = locate_ram(inverted, plan, ram)?;
    let storage = &reads.preloads[1].caller;
    let address = storage.address;
    let layout: Layout =
        serde_json::from_str(include_str!("../assets/xpp-preloaded.generated.json"))
            .map_err(|e| image::Error::Invalid(e.to_string()))?;
    require(
        layout.schema == 4
            && layout.link_address == 0x100000
            && layout.size == IMAGE.len()
            && layout.config + 12 <= IMAGE.len()
            && layout.preloaded < IMAGE.len()
            && layout.config_reference + 4 <= IMAGE.len()
            && IMAGE.len() <= storage.size
            && word(IMAGE, layout.config_reference)? as usize
                == layout.link_address + layout.config,
        "Invalid Agate preload helper layout",
    )?;
    // Reuse the original raw-section-1 loader's executable storage. Its full
    // stock body is pinned above; no free-gap or allocator reservation is used.
    hash(extent(ram, address, storage.size)?, &storage.sha256)?;
    for p in &reads.preloads {
        hash(extent(ram, p.call.address, 4)?, &p.call.sha256)?;
    }
    for p in &reads.padding {
        require(
            extent(ram, p.address, p.source.decoded_size)?
                .iter()
                .all(|b| *b == 0),
            "Agate raw allocation padding is occupied",
        )?;
    }
    let mut program = IMAGE.to_vec();
    for (i, v) in [
        reads.cache_maintenance.address,
        reads.enabled_byte,
        reads.preloaded_byte,
    ]
    .into_iter()
    .enumerate()
    {
        put(
            &mut program,
            layout.config + i * 4,
            u32::try_from(v).map_err(|_| {
                image::Error::Invalid("Agate preload configuration exceeds ARM32".into())
            })?,
        )?;
    }
    put(
        &mut program,
        layout.config_reference,
        (address + layout.config) as u32,
    )?;
    for (i, p) in reads.preloads.iter().enumerate() {
        if i == 1 {
            continue; // Its entire original body becomes the compiled helper.
        }
        let delta = (address + layout.preloaded) as i64 - p.call.address as i64 - 8;
        require(
            delta % 4 == 0 && (-0x2000000..0x2000000).contains(&delta),
            "Agate preload hook is out of BL range",
        )?;
        put(
            ram,
            p.call.address,
            0xeb000000 | ((delta >> 2) as u32 & 0xffffff),
        )?;
    }
    ram[address..address + program.len()].copy_from_slice(&program);
    for p in &reads.padding {
        let bytes = p.source.read(inverted)?;
        ram[p.address..p.address + bytes.len()].copy_from_slice(&bytes);
    }
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
        let mut input = vec![0x5a; 0x20100];
        for (off, val) in [(0, 5), (4, 0x200000), (20, 0xf00)] {
            put(&mut input, 0x400 + 0x40 + 5 * 36 + off, val).unwrap();
        }
        let mut ram = vec![0; 0x240000];
        for (off, value) in [(0, 6), (20, 0x10000), (24, 0x10000), (32, 1)] {
            put(&mut input, 0x400 + 0x40 + 6 * 36 + off, value).unwrap();
        }
        for (address, words) in [
            (
                0x200400,
                vec![
                    0xe92d4ff0, 0xe28db020, 0xe24dd014, 0xe1a04000, 0xe1a08001, 0xe1a06002,
                    0xe1a07003,
                ],
            ),
            (
                0x20041c,
                vec![0xe3003000, 0xe3403023, 0xe5d33000, 0xe3530001],
            ),
            (
                0x20043c,
                vec![
                    branch(0x20043c, 0x200500),
                    0xe1a00804,
                    branch(0x200444, 0x200080),
                ],
            ),
            (0x200480, vec![branch(0x200480, 0x2000c0)]),
            (
                0x200500,
                vec![
                    0xe92d4800,
                    0xe28db004,
                    0xe24dd008,
                    0xe3a03000,
                    0xe50b300c,
                    0xe50b3008,
                    0xe3000000,
                    0xe3400002,
                    0xe24b100c,
                    branch(0x200524, 0x200900),
                    0xe24bd004,
                    0xe8bd8800,
                ],
            ),
            (
                0x200600,
                vec![0xe92d49f0, 0xe28db018, 0xe24dd00c, 0xe3a00006],
            ),
            (
                0x20068c,
                vec![branch(0x20068c, 0x200800), 0xe3500000, 0x1a00001b],
            ),
            (0x2006d4, vec![branch(0x2006d4, 0x200400)]),
            (
                0x200708,
                vec![
                    0xe3a03001,
                    0xe58d3000,
                    0xe1a00008,
                    0xe1a01007,
                    0xe3a02a02,
                    0xe3402600,
                    0xe3a03000,
                    branch(0x200724, 0x200400),
                    0xe1a06000,
                    0xe3550000,
                    0x0a000005,
                ],
            ),
            (0x20076c, vec![0xe1a00006, 0xe24bd018, 0xe8bd89f0]),
            (
                0x200800,
                vec![
                    0xe52db004, 0xe28db000, 0xe3003000, 0xe3403008, 0xe5d30008, 0xe24bd000,
                    0xe49db004, 0xe12fff1e,
                ],
            ),
            (0x200380, vec![0xe3a00000]),
        ] {
            for (i, value) in words.into_iter().enumerate() {
                put(&mut ram, address + i * 4, value).unwrap();
            }
        }
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
        put(&mut ram, 0x200610, branch(0x200610, 0x200880)).unwrap();
        for (id, at, dest) in [(1, 0x200900, 0x220000), (2, 0x200980, 0x230000)] {
            for (i, w) in [
                0xe92d4800,
                0xe28db004,
                0xe24dd008,
                0xe3a00000 | id,
                branch(at + 16, 0x200880),
                0xe2503000,
                0x1a000000,
                0xeafffffe,
                0xe1d301b6,
                0xe5931018,
                0xe2811cff,
                0xe28110ff,
                0xe5932004,
                0xe3a03001,
                0xe58d3000,
                0xe1a01821,
                0xe3a03000,
                branch(at + 0x44, 0x200400),
                0xe24bd004,
                0xe8bd8800,
            ]
            .into_iter()
            .enumerate()
            {
                put(&mut ram, at + i * 4, w).unwrap();
            }
            for (off, value) in [
                (0, id),
                (4, dest),
                (8, 32),
                (20, 0x10000),
                (24, 0x10000),
                (32, 0),
            ] {
                put(&mut input, 0x400 + 0x40 + id as usize * 36 + off, value).unwrap();
            }
        }
        let mut plan = Plan {
            size: ram.len(),
            entry: 0,
            bss: vec![],
            edits: vec![image::Edit {
                address: 0x200380,
                old: branch(0x200380, 0x200900),
                new: 0xe3a00000,
                what: "skip installed-flash signature probe for preloaded startup".into(),
            }],
            regions: vec![
                Region {
                    name: "section-table".into(),
                    address: 0x80000,
                    source: source(0x400, 0x400),
                },
                Region {
                    name: "section-5".into(),
                    address: 0x200000,
                    source: source(0x1000, 0xc00),
                },
            ],
        };
        for dest in [0x220000, 0x230000] {
            plan.regions.push(Region {
                name: format!("raw-{dest:x}"),
                address: dest,
                source: source(0x10100, 32),
            });
        }
        ram[0x80000..0x80400].copy_from_slice(&input[0x400..0x800]);
        input[0x1000..0x1c00].copy_from_slice(&ram[0x200000..0x200c00]);
        (input, plan, ram)
    }
    #[test]
    fn preloaded_allocations_include_padding_and_preserve_stock_data_paths() {
        let (input, plan, mut ram) = fixture();
        ram[0x100000..0x108000].fill(0xa5);
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
            vec![0x200128, 0x200280, 0x200480]
        );
        assert_eq!(reads.pointer_sites[0].address, 0x200300);
        install(&input, &plan, &mut ram).unwrap();
        let layout: Layout =
            serde_json::from_str(include_str!("../assets/xpp-preloaded.generated.json")).unwrap();
        assert_eq!(ram.len(), original.len());
        let c: Vec<_> = (0..3)
            .map(|i| {
                word(
                    &ram,
                    reads.preloads[1].caller.address + layout.config + i * 4,
                )
                .unwrap() as usize
            })
            .collect();
        assert_eq!(
            c,
            vec![
                reads.cache_maintenance.address,
                reads.enabled_byte,
                reads.preloaded_byte
            ]
        );
        assert_eq!(reads.preloads.len(), 3);
        for (index, p) in reads.preloads.iter().enumerate() {
            if index != 1 {
                assert_eq!(
                    call(&ram, p.call.address).unwrap(),
                    reads.preloads[1].caller.address + layout.preloaded
                );
            }
            ram[p.call.address..p.call.address + 4]
                .copy_from_slice(&original[p.call.address..p.call.address + 4]);
        }
        assert_eq!(&ram[0x2000c0..0x200100], &original[0x2000c0..0x200100]);
        for p in reads.call_sites.iter().chain(&reads.pointer_sites) {
            assert_eq!(
                &ram[p.address..p.address + p.size],
                &original[p.address..p.address + p.size]
            );
        }
        assert_eq!(reads.padding.len(), 2);
        for p in &reads.padding {
            let expected = p.source.read(&input).unwrap();
            assert_eq!(&ram[p.address..p.address + expected.len()], expected);
            ram[p.address..p.address + expected.len()]
                .copy_from_slice(&original[p.address..p.address + expected.len()]);
        }
        let address = reads.preloads[1].caller.address;
        ram[address..address + IMAGE.len()]
            .copy_from_slice(&original[address..address + IMAGE.len()]);
        assert_eq!(&ram[..original.len()], original);
    }
    #[test]
    fn changed_read_abi_translation_or_raw_loader_body_refuses() {
        for at in [0x200080, 0x2000c0, 0x200100, 0x200900, 0x220020] {
            let (input, plan, mut ram) = fixture();
            ram[at] ^= 1;
            assert!(install(&input, &plan, &mut ram).is_err(), "{at:x}");
        }
    }
}
