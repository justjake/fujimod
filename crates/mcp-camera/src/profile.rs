//! Collected execution profiles: instructions per guest basic block.
use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

#[derive(Clone, Copy, Default)]
struct Block {
    entries: u64,
    instructions: u64,
}

/// One collection, keyed by processor and block address.
pub struct Profile {
    id: String,
    camera: String,
    file: String,
    blocks: BTreeMap<(u64, u64), Block>,
    total: u64,
}

impl Profile {
    /// Parse a board guest profile and write it to `path`.
    pub fn new(id: &str, camera: &str, raw: Value, path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        let text = serde_json::to_vec(&raw).map_err(|error| error.to_string())?;
        std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut blocks = BTreeMap::new();
        let mut total = 0u64;
        for core in raw["cores"].as_array().ok_or("profile has no cores")? {
            if let Some(error) = core.get("error") {
                return Err(format!("processor profile failed: {error}"));
            }
            let index = core["core"].as_u64().ok_or("profile core has no index")?;
            for block in core["blocks"]
                .as_array()
                .ok_or("profile core has no blocks")?
            {
                let field = |name: &str| block[name].as_u64().unwrap_or(0);
                let instructions = field("idle_instructions") + field("key_instructions");
                total += instructions;
                let entry = blocks
                    .entry((index, field("address")))
                    .or_insert(Block::default());
                entry.entries += field("entries");
                entry.instructions += instructions;
            }
        }
        Ok(Self {
            id: id.into(),
            camera: camera.into(),
            file: path.display().to_string(),
            blocks,
            total,
        })
    }

    pub fn summary(&self, limit: usize) -> Value {
        let mut ranked: Vec<_> = self.blocks.iter().collect();
        ranked.sort_by_key(|(_, block)| std::cmp::Reverse(block.instructions));
        let rows: Vec<_> = ranked
            .into_iter()
            .take(limit)
            .map(|(&(core, address), block)| {
                json!({
                    "core": core,
                    "block": format!("{address:#x}"),
                    "instructions": block.instructions,
                    "entries": block.entries,
                    "share": share(block.instructions, self.total),
                })
            })
            .collect();
        json!({
            "profile": self.id,
            "camera": self.camera,
            "file": self.file,
            "total_instructions": self.total,
            "blocks": self.blocks.len(),
            "top": rows,
        })
    }
}

fn share(part: u64, total: u64) -> String {
    if total == 0 {
        return "0%".into();
    }
    format!("{:.2}%", part as f64 * 100.0 / total as f64)
}

/// Blocks whose executed instructions changed most between two profiles.
pub fn diff(before: &Profile, after: &Profile, limit: usize) -> Value {
    let mut keys: Vec<_> = before
        .blocks
        .keys()
        .chain(after.blocks.keys())
        .copied()
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let mut rows: Vec<_> = keys
        .into_iter()
        .map(|key| {
            let a = before.blocks.get(&key).copied().unwrap_or_default();
            let b = after.blocks.get(&key).copied().unwrap_or_default();
            (key, a, b, b.instructions as i128 - a.instructions as i128)
        })
        .filter(|row| row.3 != 0 || row.1.entries != row.2.entries)
        .collect();
    let only_after = rows.iter().filter(|row| row.1.entries == 0).count();
    let only_before = rows.iter().filter(|row| row.2.entries == 0).count();
    rows.sort_by_key(|row| std::cmp::Reverse(row.3.abs()));
    let top: Vec<_> = rows
        .into_iter()
        .take(limit)
        .map(|((core, address), a, b, delta)| {
            json!({
                "core": core,
                "block": format!("{address:#x}"),
                "before": a.instructions,
                "after": b.instructions,
                "delta": delta,
                "entries_before": a.entries,
                "entries_after": b.entries,
            })
        })
        .collect();
    json!({
        "before": before.id,
        "after": after.id,
        "total_before": before.total,
        "total_after": after.total,
        "total_delta": after.total as i128 - before.total as i128,
        "blocks_only_before": only_before,
        "blocks_only_after": only_after,
        "top": top,
    })
}
