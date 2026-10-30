//! Physical bus decoding and backing, independent of CPU-private mappings.
mod checkpoint;
use fujimod_emulator_cpu::{Cpu, exclusive, memory::Backing};
use std::{
    cell::{Ref, RefCell},
    ptr::NonNull,
    rc::Rc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusMapping {
    pub address: u64,
    pub physical: u64,
    pub size: usize,
    /// Read=1, write=2. Privileged host loading bypasses these bus permissions.
    pub permissions: u32,
}
impl BusMapping {
    pub fn ram(address: u64, size: usize) -> Self {
        Self {
            address,
            physical: address,
            size,
            permissions: 3,
        }
    }
    fn end(self) -> u64 {
        self.address + self.size as u64
    }
}
pub(super) fn validate_mappings(
    mappings: &[BusMapping],
    regions: &[(u64, u64)],
) -> Result<(), String> {
    for (i, map) in mappings.iter().enumerate() {
        if map.size == 0
            || map.address.checked_add(map.size as u64).is_none()
            || map.physical.checked_add(map.size as u64).is_none()
            || map.permissions & !3 != 0
        {
            return Err("invalid physical bus memory mapping".into());
        }
        if mappings[..i]
            .iter()
            .any(|p| p.address < map.end() && map.address < p.end())
        {
            return Err("overlapping physical bus memory mappings".into());
        }
        let end = map.physical + map.size as u64;
        let mut cursor = map.physical;
        while cursor < end {
            let (_, next) = regions
                .iter()
                .find(|&&(lo, hi)| lo <= cursor && cursor < hi)
                .ok_or("physical bus mapping has no complete backing")?;
            cursor = end.min(*next);
        }
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum Access {
    Host,
    Read,
    Write,
}
#[derive(Default)]
pub struct Memory {
    pub(super) continuation: RefCell<Option<std::rc::Weak<RefCell<super::continuation::Stops>>>>,
    regions: Vec<(u64, Backing)>,
    mappings: RefCell<Vec<BusMapping>>,
    trackers: Vec<fujimod_emulator_cpu::memory::pages::Tracker>,
    pub(super) observation: RefCell<Option<std::rc::Weak<RefCell<super::observation::Records>>>>,
    pub(super) preservation:
        RefCell<Option<std::rc::Weak<RefCell<super::preservation::PreservationRecords>>>>,
}
#[derive(Clone)]
pub(super) struct Checkpoint {
    regions: Vec<(u64, fujimod_emulator_cpu::memory::pages::Image)>,
    mappings: Vec<BusMapping>,
}
impl Memory {
    pub(super) fn physical_ranges(
        &self,
        address: u64,
        bytes: usize,
    ) -> Result<Vec<(u64, u64)>, String> {
        self.chunks(address, bytes, Access::Host)?
            .map(|chunk| chunk.map(|(physical, _, len)| (physical, physical + len as u64)))
            .collect()
    }
    fn observe_transfer(&self, source: &str, address: u64, bytes: usize) {
        let Some(observer) = self
            .observation
            .borrow()
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
        else {
            return;
        };
        let Ok(chunks) = self.chunks(address, bytes, Access::Host) else {
            return;
        };
        let mut offset = 0u64;
        for chunk in chunks {
            let Ok((physical, _, len)) = chunk else {
                continue;
            };
            observer
                .borrow_mut()
                .transfer(source, physical, address + offset, len);
            offset += len as u64;
        }
    }
    fn observe_preservation_write(
        &self,
        source: super::preservation::PreservationBusSource,
        address: u64,
        bytes: usize,
    ) {
        if bytes == 0 {
            return;
        }
        let Some(observer) = self
            .preservation
            .borrow()
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
        else {
            return;
        };
        let Ok(chunks) = self.chunks(address, bytes, Access::Host) else {
            observer.borrow_mut().unresolved();
            return;
        };
        let mut offset = 0u64;
        for chunk in chunks {
            let Ok((physical, _, len)) = chunk else {
                observer.borrow_mut().unresolved();
                return;
            };
            let Ok(physical) = super::preservation::PhysicalAddress::new(physical) else {
                observer.borrow_mut().unresolved();
                return;
            };
            let Ok(logical) = super::preservation::CpuAddress::new(address + offset) else {
                observer.borrow_mut().unresolved();
                return;
            };
            let Ok(chunk_size) = super::preservation::WriteSize::new(len) else {
                observer
                    .borrow_mut()
                    .fault(super::preservation::PreservationFault::UnsupportedAccess);
                return;
            };
            if observer.borrow().overlaps(physical, chunk_size) {
                observer
                    .borrow_mut()
                    .bus_write(source, physical, logical, chunk_size);
                return;
            }
            offset += len as u64;
        }
    }

    pub(super) fn rearm(&self, cpus: &[Rc<dyn Cpu>]) -> Result<(), String> {
        for cpu in cpus {
            for (pointer, bytes) in cpu.checkpoint_backings()? {
                let start = pointer.as_ptr() as usize;
                let end = start
                    .checked_add(bytes)
                    .ok_or("keyframe backing overflow")?;
                if !self.regions.iter().any(|(_, b)| {
                    b.as_ptr() as usize <= start && end <= b.as_ptr() as usize + b.len()
                }) {
                    return Err("processor has RAM outside whole-board keyframe ownership".into());
                }
            }
        }
        let ranges = self
            .regions
            .iter()
            .zip(&self.trackers)
            .filter_map(|((_, b), t)| t.observer(NonNull::new(b.as_ptr()).unwrap()))
            .collect::<Vec<_>>();
        for cpu in cpus {
            unsafe {
                cpu.observe_backings(&ranges)?;
            }
        }
        Ok(())
    }
    pub(super) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            regions: self
                .regions
                .iter()
                .zip(&self.trackers)
                .map(|((base, b), t)| (*base, unsafe { t.capture(b.as_ptr()) }))
                .collect(),
            mappings: self.mappings.borrow().clone(),
        }
    }
    pub(super) fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        if self.regions.len() != saved.regions.len()
            || self
                .regions
                .iter()
                .zip(&saved.regions)
                .any(|((base, b), (old, image))| base != old || b.len() != image.len())
        {
            return Err("keyframe physical memory geometry differs".into());
        }
        validate_mappings(
            &saved.mappings,
            &self
                .regions
                .iter()
                .map(|(base, b)| (*base, *base + b.len() as u64))
                .collect::<Vec<_>>(),
        )
    }
    pub(super) fn restore_checkpoint(
        &self,
        saved: &Checkpoint,
        cpus: &[Rc<dyn Cpu>],
        domain: &exclusive::Shared,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let preservation = self
            .preservation
            .borrow()
            .as_ref()
            .and_then(std::rc::Weak::upgrade);
        for (((base, b), t), (_, image)) in
            self.regions.iter().zip(&self.trackers).zip(&saved.regions)
        {
            let changed = unsafe { t.restore(image, b.as_ptr()) }?;
            for (offset, bytes) in changed {
                let pointer = unsafe { NonNull::new_unchecked(b.as_ptr().add(offset)) };
                let physical = *base + offset as u64;
                if let Some(observer) = &preservation {
                    let typed = (|| {
                        Ok::<_, String>((
                            super::preservation::PhysicalAddress::new(physical)?,
                            super::preservation::CpuAddress::new(physical)?,
                            super::preservation::WriteSize::new(bytes)?,
                        ))
                    })();
                    match typed {
                        Ok((physical, address, size)) => observer.borrow_mut().bus_write(
                            super::preservation::PreservationBusSource::HostWrite,
                            physical,
                            address,
                            size,
                        ),
                        Err(_) => observer
                            .borrow_mut()
                            .fault(super::preservation::PreservationFault::UnsupportedAccess),
                    }
                }
                for cpu in cpus {
                    unsafe {
                        cpu.backing_written(physical, pointer, bytes)?;
                    }
                }
                domain
                    .lock()
                    .map_err(|_| "exclusive monitors poisoned")?
                    .written(Some((physical, bytes)))?;
            }
        }
        self.mappings.borrow_mut().clone_from(&saved.mappings);
        Ok(())
    }
    fn mark(&self, pointer: NonNull<u8>, bytes: usize) -> Result<(), String> {
        let start = pointer.as_ptr() as usize;
        for ((_, b), t) in self.regions.iter().zip(&self.trackers) {
            let base = b.as_ptr() as usize;
            if base <= start && start - base <= b.len() && bytes <= b.len() - (start - base) {
                return t.mark(start - base, bytes);
            }
        }
        Err("dirty write has no physical backing".into())
    }

    pub(super) fn fingerprint(&self) -> serde_json::Value {
        self.regions.iter().map(|(base, backing)| {
            let bytes = unsafe { std::slice::from_raw_parts(backing.as_ptr(), backing.len()) };
            serde_json::json!({"base":base,"contents":fujimod_emulator_devices::fingerprint::bytes(bytes)})
        }).collect()
    }
    pub fn new(
        mut regions: Vec<(u64, Backing)>,
        mut mappings: Vec<BusMapping>,
    ) -> Result<Self, String> {
        regions.sort_by_key(|r| r.0);
        let mut previous_end = 0;
        let mut spans = Vec::with_capacity(regions.len());
        for (base, backing) in &regions {
            let end = base
                .checked_add(backing.len() as u64)
                .ok_or("physical memory range overflow")?;
            if backing.len() == 0 || *base < previous_end {
                return Err("empty or overlapping physical memory".into());
            }
            previous_end = end;
            spans.push((*base, end));
        }
        validate_mappings(&mappings, &spans)?;
        mappings.sort_by_key(|m| m.address);
        let trackers = regions
            .iter()
            .map(|(_, b)| fujimod_emulator_cpu::memory::pages::Tracker::new(b.len()))
            .collect();
        Ok(Self {
            regions,
            mappings: RefCell::new(mappings),
            trackers,
            observation: Default::default(),
            preservation: Default::default(),
            continuation: Default::default(),
        })
    }
    pub(super) fn clear_preservation(
        &self,
        records: &Rc<RefCell<super::preservation::PreservationRecords>>,
    ) {
        let mut slot = self.preservation.borrow_mut();
        let active = slot
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
            .is_some_and(|current| Rc::ptr_eq(&current, records));
        if active {
            *slot = None;
        }
    }
    pub(super) fn regions(&self) -> &[(u64, Backing)] {
        &self.regions
    }
    pub(super) fn replace_views(&self, replacements: &[BusMapping]) -> Result<(), String> {
        let mut mappings = self.mappings.borrow_mut();
        // Geometry and backing were validated for every choice before execution.
        // Check all targets before changing any view.
        for replacement in replacements {
            if !mappings
                .iter()
                .any(|m| m.address == replacement.address && m.size == replacement.size)
            {
                return Err("decoder physical bus window disappeared".into());
            }
        }
        for replacement in replacements {
            *mappings
                .iter_mut()
                .find(|m| m.address == replacement.address)
                .unwrap() = *replacement;
        }
        Ok(())
    }
    pub(super) fn state(&self) -> serde_json::Value {
        self.mappings.borrow().iter().map(|m| serde_json::json!({
            "address":m.address,"physical":m.physical,"size":m.size,"permissions":m.permissions
        })).collect()
    }
    fn chunks(&self, address: u64, bytes: usize, access: Access) -> Result<Chunks<'_>, String> {
        let end = address
            .checked_add(bytes as u64)
            .ok_or("physical memory access overflow")?;
        let chunks = Chunks {
            memory: self,
            mappings: self.mappings.borrow(),
            address,
            end,
            access,
        };
        // Reject the complete range before any copy or monitor notification.
        for chunk in chunks.clone() {
            chunk?;
        }
        Ok(chunks)
    }
    fn read(&self, address: u64, bytes: &mut [u8], access: Access) -> Result<(), String> {
        let mut offset = 0;
        for chunk in self.chunks(address, bytes.len(), access)? {
            let (_, pointer, len) = chunk?;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pointer.as_ptr(),
                    bytes.as_mut_ptr().add(offset),
                    len,
                );
            }
            offset += len;
        }
        Ok(())
    }
    pub fn read_host(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.read(address, bytes, Access::Host)?;
        self.observe_transfer("host-read", address, bytes.len());
        Ok(())
    }
    pub(super) fn read_dma(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.read(address, bytes, Access::Read)?;
        self.observe_transfer("model-bus-read", address, bytes.len());
        Ok(())
    }
    fn write(
        &self,
        cpus: &[Rc<dyn Cpu>],
        domain: &exclusive::Shared,
        address: u64,
        bytes: &[u8],
        access: Access,
    ) -> Result<(), String> {
        let mut offset = 0;
        for chunk in self.chunks(address, bytes.len(), access)? {
            let (physical, pointer, len) = chunk?;
            self.mark(pointer, len)?;
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr().add(offset), pointer.as_ptr(), len);
                for cpu in cpus {
                    cpu.backing_written(physical, pointer, len)?;
                }
            }
            domain
                .lock()
                .map_err(|_| "exclusive monitors poisoned")?
                .written(Some((physical, len)))?;
            offset += len;
        }
        Ok(())
    }
    pub(super) fn write_host(
        &self,
        cpus: &[Rc<dyn Cpu>],
        domain: &exclusive::Shared,
        address: u64,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.observe_preservation_write(
            super::preservation::PreservationBusSource::HostWrite,
            address,
            bytes.len(),
        );
        self.write(cpus, domain, address, bytes, Access::Host)?;
        self.observe_transfer("host-write", address, bytes.len());
        Ok(())
    }
    pub(super) fn write_dma(
        &self,
        cpus: &[Rc<dyn Cpu>],
        domain: &exclusive::Shared,
        address: u64,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.observe_preservation_write(
            super::preservation::PreservationBusSource::ModelBusWrite,
            address,
            bytes.len(),
        );
        self.write(cpus, domain, address, bytes, Access::Write)?;
        self.observe_transfer("model-bus-write", address, bytes.len());
        if let Some(stops) = self
            .continuation
            .borrow()
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
        {
            let mut offset = 0;
            for (physical, _, len) in self.chunks(address, bytes.len(), Access::Host)?.flatten() {
                stops
                    .borrow_mut()
                    .store(physical, &bytes[offset..offset + len]);
                offset += len;
            }
        }
        Ok(())
    }
}
struct Chunks<'a> {
    memory: &'a Memory,
    mappings: Ref<'a, Vec<BusMapping>>,
    address: u64,
    end: u64,
    access: Access,
}
impl Clone for Chunks<'_> {
    fn clone(&self) -> Self {
        Self {
            memory: self.memory,
            mappings: Ref::clone(&self.mappings),
            address: self.address,
            end: self.end,
            access: self.access,
        }
    }
}
impl Iterator for Chunks<'_> {
    type Item = Result<(u64, NonNull<u8>, usize), String>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.address == self.end {
            return None;
        }
        let address = self.address;
        let Some(map) = self
            .mappings
            .iter()
            .find(|m| m.address <= address && address < m.end())
        else {
            self.address = self.end;
            return Some(Err(format!("no physical bus memory at {address:#x}")));
        };
        let bit = match self.access {
            Access::Host => 0,
            Access::Read => 1,
            Access::Write => 2,
        };
        if map.permissions & bit != bit {
            self.address = self.end;
            return Some(Err(format!(
                "physical bus memory denies DMA access at {address:#x}"
            )));
        }
        let physical = map.physical + (address - map.address);
        let (base, backing) = self
            .memory
            .regions
            .iter()
            .find(|(base, b)| *base <= physical && physical < *base + b.len() as u64)
            .expect("validated physical bus backing");
        let len = (self.end.min(map.end()) - address).min(*base + backing.len() as u64 - physical)
            as usize;
        let pointer = unsafe { backing.as_ptr().add((physical - base) as usize) };
        self.address += len as u64;
        Some(Ok((physical, NonNull::new(pointer).unwrap(), len)))
    }
}
