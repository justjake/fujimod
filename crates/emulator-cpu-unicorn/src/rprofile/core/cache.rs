//! Data-cache accounting for this processor's loads, stores and cache
//! maintenance instructions.
use super::*;
use crate::rprofile::data_cache::{self, Operation, Policy};

fn system(crn: u32, crm: u32, opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn,
        crm,
        opc2,
        secure: 0,
        wide: false,
    }
}

impl Execution {
    /// The cache operation of an MCR word, when this processor models a cache.
    pub(super) fn data_cache_operation(&self, word: u32, thumb: bool) -> Option<Operation> {
        let word = if thumb { word.rotate_left(16) } else { word };
        if self.data_cache.borrow().is_none()
            || word & 0x0f10_0f10 != 0x0e00_0f10
            || !thumb && word >> 28 == 15
        {
            return None;
        }
        data_cache::operation(
            (word >> 21) & 7,
            (word >> 16) & 15,
            word & 15,
            (word >> 5) & 7,
        )
    }

    /// Account for a cache maintenance or attribute write before it executes.
    pub(super) fn data_cache_instruction(
        &self,
        word: u32,
        opc1: u32,
        crn: u32,
        crm: u32,
        opc2: u32,
        cpsr: u32,
    ) -> Result<(), String> {
        let Some(geometry) = self.data_cache.borrow().as_ref().map(|c| c.geometry()) else {
            return Ok(());
        };
        let Some(operation) = data_cache::operation(opc1, crn, crm, opc2) else {
            return Ok(());
        };
        if !self.thumb.get() && !arm::condition_passes(word >> 28, cpsr) {
            return Ok(());
        }
        let target = ((word >> 12) & 15) as usize;
        let value = || -> Result<u32, String> {
            if target == 15 {
                return Err("R-profile cache maintenance from PC is unmodeled".into());
            }
            self.scalar(arm::CORE[target])
        };
        let cache =
            || std::cell::RefMut::map(self.data_cache.borrow_mut(), |c| c.as_mut().unwrap());
        match operation {
            Operation::Attributes => self.cache_spans_stale.set(true),
            Operation::CleanLine => {
                // Lines exist only at write-back or write-through addresses;
                // maintenance through an uncached alias reaches no line.
                if let Some((physical, policy)) = self.cache_policy(value()? as u64)?
                    && policy != Policy::NonCacheable
                {
                    cache().clean_line(physical);
                }
            }
            Operation::CleanSet => {
                let set = (value()? >> geometry.line_bytes.trailing_zeros()) & (geometry.sets - 1);
                cache().clean_set(set as usize);
            }
            Operation::CleanAll => cache().clean_all(),
        }
        Ok(())
    }

    /// The physical address and cache policy of a processor address, or
    /// `None` outside external RAM.
    fn cache_policy(&self, address: u64) -> Result<Option<(u64, Policy)>, String> {
        if self.cache_spans_stale.get() {
            self.refresh_cache_spans()?;
        }
        Ok(self
            .cache_spans
            .borrow()
            .iter()
            .find(|span| span.begin <= address && address < span.end)
            .map(|span| (span.physical + (address - span.begin), span.policy)))
    }

    fn refresh_cache_spans(&self) -> Result<(), String> {
        let cp = |crn, crm, opc2, value| self.cpu.coprocessor(system(crn, crm, opc2), value);
        let sctlr = cp(1, 0, 0, None)? as u32;
        let mut regions = Vec::new();
        if sctlr & 1 != 0 {
            // The region number register selects the region the other MPU
            // registers show; restore the guest's selection afterward.
            let count = (cp(0, 0, 4, None)? >> 8) & 0xff;
            let selected = cp(6, 2, 0, None)?;
            for region in 0..count {
                cp(6, 2, 0, Some(region))?;
                regions.push((
                    cp(6, 1, 0, None)? as u32,
                    cp(6, 1, 2, None)? as u32,
                    cp(6, 1, 4, None)? as u32,
                ));
            }
            cp(6, 2, 0, Some(selected))?;
        }
        let maps: Vec<(u64, u64, u64)> = self
            .maps
            .borrow()
            .iter()
            .filter_map(|map| Some((map.address, map.end(), map.physical?)))
            .collect();
        *self.cache_spans.borrow_mut() = data_cache::spans(sctlr, &regions, &maps);
        self.cache_spans_stale.set(false);
        Ok(())
    }

    pub(super) fn data_access(
        &self,
        access: Access,
        address: u64,
        size: i32,
    ) -> Result<(), String> {
        if self.data_cache.borrow().is_none() || access == Access::Fetch {
            return Ok(());
        }
        let bytes = usize::try_from(size).map_err(|_| "invalid R-profile access width")?;
        if bytes == 0 || self.overlaps_tcm(address, bytes) {
            return Ok(());
        }
        let Some((physical, policy)) = self.cache_policy(address)? else {
            return Ok(());
        };
        let write = access == Access::Write;
        let hazard = self.data_cache.borrow_mut().as_mut().unwrap().access(
            write,
            policy,
            physical,
            bytes,
            || self.scalar(arm::PC),
        )?;
        let Some(hazard) = hazard else {
            return Ok(());
        };
        let cache = self.data_cache.borrow();
        let line = cache.as_ref().unwrap().line_address(hazard.line);
        let line_bytes = cache.as_ref().unwrap().geometry().line_bytes;
        let holder = if hazard.holder.core == self.id.get() {
            "its own data cache".to_string()
        } else {
            format!("processor {}'s data cache", hazard.holder.core)
        };
        Err(format!(
            "data-cache coherence hazard: processor {} {} {bytes} byte(s) at {address:#x} \
             ({policy:?}, pc {:#x}) in physical line {line:#x}..{:#x}, which {holder} holds \
             dirty since its store at pc {:#x}; that cache must clean the line before \
             another agent uses its memory, or the data must use an uncached alias",
            self.id.get(),
            if write { "writes" } else { "reads" },
            self.scalar(arm::PC)?,
            line + line_bytes as u64,
            hazard.holder.pc,
        ))
    }
}
