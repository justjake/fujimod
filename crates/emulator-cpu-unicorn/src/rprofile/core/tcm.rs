//! TCM register transactions and stopped-processor mapping changes.
use super::*;
use crate::rprofile::tcm::{normalize, validate_disjoint};

impl Execution {
    pub(super) fn tcm_physical(&self, bank: usize) -> Result<u64, String> {
        (self.id.get() as u64)
            .checked_mul(0x1000000)
            .and_then(|p| p.checked_add(0x1_0000_0000 + bank as u64 * 0x800000))
            .ok_or("TCM physical identity exhausted".into())
    }
    pub(super) fn declare_tcm(&self, bank: usize) -> Result<(), String> {
        let memory = &self.tcm[bank].memory;
        if memory.len() == 0 {
            return Ok(());
        }
        let physical = self.tcm_physical(bank)?;
        self.memory
            .validate_mapping(memory.as_ptr() as usize, memory.len(), Some(physical))?;
        self.memory.declare(
            self.id.get(),
            physical,
            memory.as_ptr() as usize,
            memory.len(),
            Some(physical),
        );
        Ok(())
    }
    pub(super) fn map_tcm(&self, bank: usize) -> Result<(), String> {
        if let Some((address, bytes)) = self.tcm[bank].extent() {
            self.map_ram(Mapping {
                address,
                bytes,
                pointer: self.tcm[bank].memory.as_ptr() as usize,
                permissions: 7,
                physical: Some(self.tcm_physical(bank)?),
            })?;
        }
        Ok(())
    }
    fn validate_tcm(&self, bank: usize, value: u32) -> Result<u32, String> {
        let value = normalize(self.tcm[bank].memory.len(), value)?;
        let other = &self.tcm[1 - bank];
        validate_disjoint(
            self.tcm[bank].memory.len(),
            value,
            other.memory.len(),
            other.register.get(),
        )?;
        if value & 1 != 0 {
            let base = (value & 0xfffff000) as u64;
            let end = base + self.tcm[bank].memory.len() as u64;
            if self
                .mmio_hooks
                .borrow()
                .iter()
                .any(|(&p, &(n, _))| p < end && base < p + n as u64)
            {
                return Err("TCM overlay of MMIO is unmodeled".into());
            }
        }
        Ok(value)
    }
    pub(super) fn apply_tcm(&self, bank: usize, value: u32) -> Result<(), String> {
        let value = self.validate_tcm(bank, value)?;
        let b = &self.tcm[bank];
        if value == b.register.get() {
            return Ok(());
        }
        if let Some((old, bytes)) = b.extent() {
            self.cpu.unmap(old, bytes)?;
            self.memory.unmap(self.id.get(), old, bytes);
            for map in self
                .maps
                .borrow()
                .iter()
                .filter_map(|m| m.clip(old, old + bytes as u64))
            {
                self.map_ram(map)?;
            }
        }
        if value & 1 != 0 {
            let base = (value & 0xfffff000) as u64;
            let end = base + b.memory.len() as u64;
            for map in self.maps.borrow().iter().filter_map(|m| m.clip(base, end)) {
                self.cpu.unmap(map.address, map.bytes)?;
                self.memory.unmap(self.id.get(), map.address, map.bytes);
            }
        }
        b.register.set(value);
        self.map_tcm(bank)?;
        self.clear_exclusive()?;
        self.cpu.invalidate_code(None)
    }
    pub(super) fn tcm_instruction(
        &self,
        bank: usize,
        word: u32,
        address: u64,
        size: u32,
        cpsr: u32,
    ) -> Result<(), String> {
        if cpsr & 31 == 0x10 {
            return Err("TCM region access requires privileged ARM mode".into());
        }
        let target = ((word >> 12) & 15) as usize;
        if target == 15 {
            return Err("TCM transfer using PC is unmodeled".into());
        }
        if word & (1 << 20) != 0 {
            self.set(arm::CORE[target], self.tcm[bank].register.get())?;
        } else {
            let value = self.validate_tcm(bank, self.scalar(arm::CORE[target])?)?;
            if value != self.tcm[bank].register.get() {
                if self.tcm_pending.get().is_some() {
                    return Err("nested TCM remap is refused".into());
                }
                self.tcm_pending.set(Some((bank, value)));
                self.reset_budget();
                self.cpu.stop()?;
            }
        }
        self.advance(address, size, cpsr)
    }
}
