//! VM-wide stage-2 mappings. All CPUs must be stopped during map/host writes.
use super::ffi;
use fujimod_emulator_cpu::exclusive::{Monitors, Owner, Saved, Shared};
use fujimod_emulator_cpu::{Identity, MemoryRegion};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, Weak};

pub const PAGE: usize = 0x4000;
static LIVE: Mutex<Weak<Machine>> = Mutex::new(Weak::new());
unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        size: usize,
        protection: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, size: usize) -> i32;
}
struct Allocation {
    pointer: usize,
    size: usize,
}
impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe {
            munmap(self.pointer as *mut c_void, self.size);
        }
    }
}
#[derive(Clone)]
struct Mapping {
    end: u64,
    pointer: usize,
    permissions: u32,
    _allocation: Option<Arc<Allocation>>,
}
pub struct Machine {
    maps: Mutex<BTreeMap<u64, Mapping>>,
    devices: Mutex<BTreeMap<Identity, Vec<(u64, u64)>>>,
    pub(super) monitors: Shared,
}
impl Machine {
    pub fn create() -> Result<Arc<Self>, String> {
        let mut live = LIVE.lock().map_err(|_| "HVF VM ownership lock poisoned")?;
        if live.strong_count() != 0 {
            return Err(
                "HVF permits one VM per process; create processors on the existing Vm".into(),
            );
        }
        unsafe {
            ffi::check(ffi::hv_vm_create(std::ptr::null()), "create VM")?;
        }
        let vm = Arc::new(Self {
            maps: Mutex::new(BTreeMap::new()),
            devices: Mutex::new(BTreeMap::new()),
            monitors: Arc::new(Mutex::new(Monitors::default())),
        });
        *live = Arc::downgrade(&vm);
        Ok(vm)
    }
    pub fn devices(&self, owner: Identity, ranges: Vec<(u64, u64)>) {
        let mut devices = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        if ranges.is_empty() {
            devices.remove(&owner);
        } else {
            devices.insert(owner, ranges);
        }
    }
    fn device_overlap(&self, address: u64, end: u64) -> bool {
        self.devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .flatten()
            .any(|&(a, b)| a < end && b > address)
    }
    pub fn range(address: u64, size: usize) -> Result<u64, String> {
        if size == 0 || address % PAGE as u64 != 0 || size % PAGE != 0 {
            return Err(format!(
                "HVF mappings require nonempty {PAGE:#x}-aligned ranges"
            ));
        }
        address
            .checked_add(size as u64)
            .ok_or_else(|| "HVF mapping range overflow".into())
    }
    pub fn permissions(permissions: u32) -> Result<(), String> {
        if permissions & !7 != 0 {
            Err("unknown memory permission bits".into())
        } else {
            Ok(())
        }
    }
    pub fn map(
        &self,
        address: u64,
        size: usize,
        permissions: u32,
        pointer: usize,
    ) -> Result<(), String> {
        self.map_owned(address, size, permissions, pointer, None)
    }
    fn map_owned(
        &self,
        address: u64,
        size: usize,
        permissions: u32,
        pointer: usize,
        allocation: Option<Arc<Allocation>>,
    ) -> Result<(), String> {
        let end = Self::range(address, size)?;
        Self::permissions(permissions)?;
        if self.device_overlap(address, end) {
            return Err("RAM mapping overlaps a device on another CPU".into());
        }
        if pointer == 0 || pointer % PAGE != 0 {
            return Err("HVF backing must be 16 KiB aligned".into());
        }
        let mut maps = self.maps.lock().map_err(|_| "HVF maps poisoned")?;
        if let Some(m) = maps.get(&address) {
            if m.end == end && m.pointer == pointer && m.permissions == permissions {
                return Ok(());
            }
        }
        if maps
            .range(..end)
            .any(|(&base, m)| base < end && m.end > address)
        {
            return Err("HVF physical mapping overlaps an existing range".into());
        }
        unsafe {
            ffi::check(
                ffi::hv_vm_map(pointer as *mut c_void, address, size, permissions as u64),
                "map",
            )?;
        }
        maps.insert(
            address,
            Mapping {
                end,
                pointer,
                permissions,
                _allocation: allocation,
            },
        );
        Ok(())
    }
    pub fn zeroed(&self, address: u64, size: usize, permissions: u32) -> Result<(), String> {
        Self::range(address, size)?;
        let pointer = unsafe { mmap(std::ptr::null_mut(), size, 3, 0x1002, -1, 0) };
        if pointer as isize == -1 {
            return Err("HVF guest backing allocation failed".into());
        }
        let allocation = Arc::new(Allocation {
            pointer: pointer as usize,
            size,
        });
        self.map_owned(
            address,
            size,
            permissions,
            pointer as usize,
            Some(allocation),
        )
    }
    pub fn regions(&self) -> Result<Vec<MemoryRegion>, String> {
        Ok(self
            .maps
            .lock()
            .map_err(|_| "HVF maps poisoned")?
            .iter()
            .map(|(&base, m)| MemoryRegion {
                begin: base,
                end: m.end - 1,
                permissions: m.permissions,
            })
            .collect())
    }
    pub fn checkpoint_backings(&self) -> Result<Vec<(std::ptr::NonNull<u8>, usize)>, String> {
        self.maps
            .lock()
            .map_err(|_| "HVF maps poisoned")?
            .iter()
            .map(|(&base, mapping)| {
                let pointer = std::ptr::NonNull::new(mapping.pointer as *mut u8)
                    .ok_or("HVF map has a null backing")?;
                Ok((pointer, (mapping.end - base) as usize))
            })
            .collect()
    }
    pub fn overlaps(&self, address: u64, end: u64) -> Result<bool, String> {
        Ok(self
            .maps
            .lock()
            .map_err(|_| "HVF maps poisoned")?
            .range(..end)
            .any(|(_, m)| m.end > address))
    }
    fn segments(&self, address: u64, size: usize) -> Result<Vec<(usize, usize)>, String> {
        let end = address
            .checked_add(size as u64)
            .ok_or("HVF host access range overflow")?;
        let maps = self.maps.lock().map_err(|_| "HVF maps poisoned")?;
        let mut at = address;
        let mut pieces = Vec::new();
        while at < end {
            let (&base, m) = maps
                .range(..=at)
                .next_back()
                .filter(|(_, m)| at < m.end)
                .ok_or_else(|| {
                    format!("HVF host access at {at:#x} is not RAM; device inspection is refused")
                })?;
            let n = (end.min(m.end) - at) as usize;
            pieces.push((m.pointer + (at - base) as usize, n));
            at += n as u64;
        }
        Ok(pieces)
    }
    pub fn read(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        let pieces = self.segments(address, bytes.len())?;
        let mut offset = 0;
        for (pointer, n) in pieces {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pointer as *const u8,
                    bytes.as_mut_ptr().add(offset),
                    n,
                );
            }
            offset += n;
        }
        Ok(())
    }
    pub fn check_access(&self, address: u64, size: usize, permission: u32) -> Result<(), String> {
        let end = address
            .checked_add(size as u64)
            .ok_or("physical access range overflow")?;
        let maps = self.maps.lock().map_err(|_| "HVF maps poisoned")?;
        let mut at = address;
        while at < end {
            let (_, m) = maps
                .range(..=at)
                .next_back()
                .filter(|(_, m)| at < m.end)
                .ok_or("physical access is not RAM")?;
            if m.permissions & permission == 0 {
                return Err("physical mapping denies access".into());
            }
            at = end.min(m.end);
        }
        Ok(())
    }
    pub fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let pieces = self.segments(address, bytes.len())?;
        self.written(Some((address, bytes.len())))?;
        let mut offset = 0;
        for (pointer, n) in pieces {
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr().add(offset), pointer as *mut u8, n);
                ffi::sys_icache_invalidate(pointer as *mut c_void, n);
            }
            offset += n;
        }
        Ok(())
    }
    pub fn written(&self, range: Option<(u64, usize)>) -> Result<(), String> {
        self.monitors
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .written(range)
    }
    pub fn has_exclusive(&self) -> bool {
        self.monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active()
    }
    pub fn exclusive_load(
        &self,
        core: Owner,
        address: u64,
        bytes: &mut [u8],
    ) -> Result<(), String> {
        let mut monitors = self
            .monitors
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?;
        self.read(address, bytes)?;
        monitors.load(core, address, bytes.len());
        Ok(())
    }
    pub fn exclusive_store(&self, core: Owner, address: u64, bytes: &[u8]) -> Result<bool, String> {
        let success = self
            .monitors
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .store(core, address, bytes.len());
        if success {
            self.write(address, bytes)?;
        }
        Ok(success)
    }
    pub fn clear_exclusive(&self, core: Owner) {
        self.monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear(core);
    }
    pub fn save_exclusive(&self, core: Owner) -> Saved {
        self.monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .save(core)
    }
    pub fn restore_exclusive(&self, core: Owner, saved: Saved) {
        self.monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .restore(core, saved);
    }
    pub fn change(
        &self,
        address: u64,
        size: usize,
        permissions: Option<u32>,
    ) -> Result<(), String> {
        let end = Self::range(address, size)?;
        if let Some(p) = permissions {
            Self::permissions(p)?;
        }
        // Validate the entire operation before touching native mappings.
        self.segments(address, size)?;
        self.written(None)?;
        let mut maps = self.maps.lock().map_err(|_| "HVF maps poisoned")?;
        unsafe {
            ffi::check(
                match permissions {
                    Some(p) => ffi::hv_vm_protect(address, size, p as u64),
                    None => ffi::hv_vm_unmap(address, size),
                },
                "change mapping",
            )?;
        }
        let keys: Vec<_> = maps
            .range(..end)
            .filter(|(_, m)| m.end > address)
            .map(|(&base, _)| base)
            .collect();
        for base in keys {
            let m = maps.remove(&base).unwrap();
            if base < address {
                maps.insert(
                    base,
                    Mapping {
                        end: address,
                        ..m.clone()
                    },
                );
            }
            if m.end > end {
                maps.insert(
                    end,
                    Mapping {
                        pointer: m.pointer + (end - base) as usize,
                        ..m.clone()
                    },
                );
            }
            if let Some(p) = permissions {
                maps.insert(
                    base.max(address),
                    Mapping {
                        end: m.end.min(end),
                        pointer: m.pointer + (address.saturating_sub(base)) as usize,
                        permissions: p,
                        ..m
                    },
                );
            }
        }
        Ok(())
    }
    pub fn invalidate(&self, range: Option<(u64, u64)>) -> Result<(), String> {
        match range {
            Some((start, end)) => {
                let size = usize::try_from(end.checked_sub(start).ok_or("inverted code range")?)
                    .map_err(|_| "code range too large")?;
                for (pointer, n) in self.segments(start, size)? {
                    unsafe {
                        ffi::sys_icache_invalidate(pointer as *mut c_void, n);
                    }
                }
            }
            None => {
                let maps = self.maps.lock().map_err(|_| "HVF maps poisoned")?;
                for (&base, m) in maps.iter() {
                    unsafe {
                        ffi::sys_icache_invalidate(
                            m.pointer as *mut c_void,
                            (m.end - base) as usize,
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
impl Drop for Machine {
    fn drop(&mut self) {
        // Destruction precedes dropping owned backing; every vCPU retains an Arc.
        let _guard = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let _ = ffi::hv_vm_destroy();
        }
    }
}
