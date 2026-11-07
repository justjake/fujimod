//! Stock allocator and active-configuration pointer used by query execution.
use super::{
    exr2::wide_constants,
    flash::{Locator, one},
};
use crate::Lookup;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn cache_primitives<L: Lookup>(index: &L, firmware: &str) -> Result<(u64, u64), String> {
    const KERNEL0: u64 = 0x40000;
    const KERNEL0_BYTES: usize = 56224;
    let kernel = index
        .read(firmware, "kernel0", KERNEL0, KERNEL0_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or("missing EXR II CPU0 kernel")?;
    let mut primitives = Vec::new();
    for (size, hash) in [
        (
            0x214,
            "10aad5ca7b798181283de15647d4cae836a2feaf0ce7363f23b430a0855d316c",
        ),
        (
            0x28,
            "4f5e46f3fcacb11933fd0f5e8fc33ae5837f738e38902a0e475c040ac5897890",
        ),
    ] {
        let matches: Vec<_> = kernel
            .windows(size)
            .enumerate()
            .step_by(4)
            .filter_map(|(offset, bytes)| {
                (fujimod_firmware::sha256(bytes) == hash).then_some(KERNEL0 + offset as u64)
            })
            .collect();
        if matches.len() != 1 {
            return Err("missing or ambiguous EXR II kernel execution primitive".into());
        }
        primitives.push(matches[0]);
    }
    Ok((primitives[0], primitives[1]))
}

pub fn exr2_execution<L: Lookup>(index: &L, firmware: &str) -> Result<Value, String> {
    let locator = Locator {
        index,
        firmware,
        unit: "main".into(),
    };
    let allocate = locator.named("malloc", |r| {
        r.code.iter().filter(|i| i.mnemonic == "bl").count() == 1
    })?;
    let release = locator.named("free", |r| {
        r.code.iter().filter(|i| i.mnemonic == "bl").count() == 1
    })?;
    let configuration = locator.named("get_cfgdata_ram_base", |r| {
        wide_constants(r).len() == 1
            && r.code.len() <= 16
            && r.code.iter().all(|i| {
                matches!(
                    i.mnemonic.as_str(),
                    "str" | "add" | "movw" | "movt" | "ldr" | "ldm" | "pop" | "bx"
                )
            })
    })?;
    let pointer = one(
        wide_constants(&configuration),
        "active configuration pointer",
    )?;
    let mut queue = vec![
        (allocate.header.key.address, 0),
        (release.header.key.address, 0),
        (configuration.header.key.address, 2),
    ];
    let mut seen = BTreeSet::new();
    let mut checks = vec![];
    while let Some((address, depth)) = queue.pop() {
        if !seen.insert(address) {
            continue;
        }
        if seen.len() > 64 {
            return Err("allocator call graph exceeds bound".into());
        }
        let r = locator.routine(address)?;
        for &(lo, hi) in &r.header.ranges {
            checks.push(json!({"address":lo,"size":hi-lo}));
        }
        if depth < 2 {
            for i in &r.code {
                if i.mnemonic == "bl" {
                    queue.push((
                        super::flash::number(i.operands.trim_start_matches('#'))?,
                        depth + 1,
                    ));
                }
            }
        }
    }
    let (cache, barrier) = cache_primitives(index, firmware)?;
    checks.extend([
        json!({"address":cache,"size":0x214}),
        json!({"address":barrier,"size":0x28}),
    ]);
    let code_checks: Vec<_> = checks
        .iter()
        .enumerate()
        .map(|(ordinal, check)| {
            let address = check["address"].as_u64().ok_or("execution check address")?;
            let size = check["size"].as_u64().ok_or("execution check size")?;
            let unit = if (0x40000..0x50000).contains(&address) {
                "kernel0"
            } else {
                "main"
            };
            let bytes = index
                .read(firmware, unit, address, size as usize)
                .map_err(|error| error.to_string())?
                .ok_or("execution check lies outside indexed code")?;
            Ok(json!({
                "name":format!("exr2_execution_{ordinal}"),
                "address":address,
                "size":size,
                "sha256":fujimod_firmware::sha256(&bytes)
            }))
        })
        .collect::<Result<_, String>>()?;
    Ok(json!({
        "allocate":allocate.header.key.address,
        "release":release.header.key.address,
        "configuration":{"mechanism":if configuration.code.iter().any(|i| i.mnemonic == "ldr") {"pointer"} else {"fixed"},"address":pointer},
        "cache":{"data_clean":cache + 0x18,"instruction_invalidate":cache + 0xfc,"data_barrier":barrier,"instruction_barrier":cache},
        "code_ranges":checks,
        "code_checks":code_checks
    }))
}
