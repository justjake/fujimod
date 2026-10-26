//! Stock allocator and active-configuration pointer used by query execution.
use super::{
    exr2::wide_constants,
    flash::{Locator, one},
};
use crate::Lookup;
use serde_json::{Value, json};
use std::collections::BTreeSet;

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
    Ok(
        json!({"allocate":allocate.header.key.address,"release":release.header.key.address,
        "configuration":{"mechanism":if configuration.code.iter().any(|i| i.mnemonic == "ldr") {"pointer"} else {"fixed"},"address":pointer},"code_ranges":checks}),
    )
}
