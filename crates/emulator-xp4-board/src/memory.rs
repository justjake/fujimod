//! Explicit physical ROM/SRAM windows alongside application DRAM.
use super::Backing;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub base: u64,
    pub size: usize,
    pub permissions: u32,
}
pub(super) struct Window {
    pub base: u64,
    pub data: Backing,
}

pub(super) fn allocate(regions: Vec<Region>, dram_size: usize) -> Result<Vec<Window>, String> {
    let mut windows: Vec<Window> = Vec::new();
    for region in regions {
        let end = region.base.checked_add(region.size as u64);
        if region.base % 4096 != 0
            || region.size == 0
            || region.size % 4096 != 0
            || region.base < dram_size as u64
            || !end.is_some_and(|end| end <= 1 << 32)
            || region.permissions == 0
            || region.permissions & !7 != 0
            || windows.iter().any(|other| {
                region.base < other.base + other.data.len() as u64 && other.base < end.unwrap()
            })
        {
            return Err("Invalid or overlapping ARM32 physical memory window".into());
        }
        windows.push(Window {
            base: region.base,
            data: Backing::zeros(region.size),
        });
    }
    Ok(windows)
}
