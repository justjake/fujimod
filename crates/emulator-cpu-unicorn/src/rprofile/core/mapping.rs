//! Owned external RAM mappings underneath processor-private TCM windows.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mapping {
    pub address: u64,
    pub bytes: usize,
    pub pointer: usize,
    pub permissions: u32,
    pub physical: Option<u64>,
}
impl Mapping {
    fn validate(self) -> Result<(), String> {
        if self.bytes == 0
            || !self.address.is_multiple_of(4096)
            || !self.bytes.is_multiple_of(4096)
            || self
                .address
                .checked_add(self.bytes as u64)
                .is_none_or(|end| end > 1 << 32)
            || self.permissions & !7 != 0
        {
            return Err("invalid R-profile RAM mapping".into());
        }
        if self.physical.is_some_and(|physical| {
            !physical.is_multiple_of(4096)
                || physical
                    .checked_add(self.bytes as u64)
                    .is_none_or(|end| end > 1 << 32)
        }) {
            return Err("invalid R-profile physical RAM identity".into());
        }
        Ok(())
    }
    pub fn end(self) -> u64 {
        self.address + self.bytes as u64
    }
    pub fn clip(self, begin: u64, end: u64) -> Option<Self> {
        let begin = self.address.max(begin);
        let end = self.end().min(end);
        (begin < end).then(|| Self {
            address: begin,
            bytes: (end - begin) as usize,
            pointer: self.pointer + (begin - self.address) as usize,
            physical: self.physical.map(|p| p + begin - self.address),
            ..self
        })
    }
}
impl Execution {
    pub(super) fn overlaps_tcm(&self, address: u64, bytes: usize) -> bool {
        self.tcm
            .iter()
            .filter_map(|b| b.extent())
            .any(|(p, n)| p < address.saturating_add(bytes as u64) && address < p + n as u64)
    }
    pub(super) fn map_ram(&self, map: Mapping) -> Result<(), String> {
        unsafe {
            self.cpu.map_backing(
                map.address,
                map.bytes,
                map.permissions,
                NonNull::new(map.pointer as *mut u8).ok_or("null R-profile RAM backing")?,
            )?;
        }
        self.memory.map(
            self.id.get(),
            map.address,
            map.pointer,
            map.bytes,
            map.physical,
        );
        self.memory.apply_watches()
    }
    pub(super) fn map_external(&self, map: Mapping) -> Result<(), String> {
        self.memory.require_stopped()?;
        map.validate()?;
        if self
            .maps
            .borrow()
            .iter()
            .any(|m| m.address < map.end() && map.address < m.end())
            || self
                .mmio_hooks
                .borrow()
                .iter()
                .any(|(&p, &(n, _))| p < map.end() && map.address < p + n as u64)
        {
            return Err("overlapping R-profile external memory mapping".into());
        }
        self.memory
            .validate_mapping(map.pointer, map.bytes, map.physical)?;
        self.maps.borrow_mut().push(map);
        self.memory.declare(
            self.id.get(),
            map.address,
            map.pointer,
            map.bytes,
            map.physical,
        );
        self.external_parts(map, |part| self.map_ram(part))
    }
    fn external_parts(
        &self,
        map: Mapping,
        mut f: impl FnMut(Mapping) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut tcm = [self.tcm[0].extent(), self.tcm[1].extent()];
        tcm.sort_unstable();
        let mut cursor = map.address;
        for (base, bytes) in tcm.into_iter().flatten() {
            if base >= map.end() || base + bytes as u64 <= cursor {
                continue;
            }
            if let Some(part) = map.clip(cursor, base) {
                f(part)?;
            }
            if base < map.end() && cursor < base + bytes as u64 {
                cursor = cursor.max(base + bytes as u64);
            }
        }
        if let Some(part) = map.clip(cursor, map.end()) {
            f(part)?;
        }
        Ok(())
    }
    pub(super) fn replace_external(&self, map: Mapping) -> Result<(), String> {
        self.memory.require_stopped()?;
        map.validate()?;
        self.memory
            .validate_mapping(map.pointer, map.bytes, map.physical)?;
        // Preflight the whole external underlay, including bytes hidden by TCM.
        let mut cursor = map.address;
        while cursor < map.end() {
            let next = self
                .maps
                .borrow()
                .iter()
                .find(|m| m.address <= cursor && cursor < m.end())
                .copied()
                .ok_or("physical replacement has no complete external RAM underlay")?;
            cursor = next.end().min(map.end());
        }
        self.external_parts(map, |part| {
            self.cpu.unmap(part.address, part.bytes)?;
            self.memory.unmap(self.id.get(), part.address, part.bytes);
            Ok(())
        })?;
        self.remove_external(map.address, map.bytes);
        self.map_external(map)?;
        self.cpu.invalidate_code(None)
    }
    pub(super) fn remove_external(&self, address: u64, bytes: usize) {
        let end = address + bytes as u64;
        let mut maps = self.maps.borrow_mut();
        let mut i = 0;
        while i < maps.len() {
            let map = maps[i];
            if map.clip(address, end).is_none() {
                i += 1;
                continue;
            }
            let left = map.clip(map.address, address);
            let right = map.clip(end, map.end());
            match (left, right) {
                (Some(left), Some(right)) => {
                    maps[i] = left;
                    maps.push(right);
                    i += 1;
                }
                (Some(part), None) | (None, Some(part)) => {
                    maps[i] = part;
                    i += 1;
                }
                (None, None) => {
                    maps.swap_remove(i);
                }
            }
        }
        self.memory.undeclare(self.id.get(), address, bytes);
    }
}
