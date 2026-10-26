//! Explicit physical ROM/SRAM windows alongside application DRAM.
use super::{Backing, Cluster};
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
    pub permissions: u32,
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
            permissions: region.permissions,
        });
    }
    Ok(windows)
}

impl Cluster {
    pub(super) fn memory_pointer(
        &self,
        address: u64,
        size: usize,
        permissions: u32,
    ) -> Result<*mut u8, String> {
        let end = address
            .checked_add(size as u64)
            .ok_or("Physical memory extent overflow")?;
        if end <= self.ram.len() as u64 {
            return Ok(unsafe { self.ram.as_ptr().add(address as usize) });
        }
        for window in &self.memory {
            if window.base <= address && end <= window.base + window.data.len() as u64 {
                if window.permissions & permissions != permissions {
                    return Err("Protected ARM32 physical memory".into());
                }
                return Ok(unsafe { window.data.as_ptr().add((address - window.base) as usize) });
            }
        }
        Err("Outside declared ARM32 physical memory".into())
    }
}
