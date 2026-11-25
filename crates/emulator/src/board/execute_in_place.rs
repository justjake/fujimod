//! Instruction fetches from device windows that offer execution
//! (`ExecuteInPlace`), such as a serial-flash aperture.
//!
//! A fetch from a device window faults, because device windows are mapped as
//! MMIO. When the page's device offers execution, the board reads the page
//! once through the device's ordinary reads, in the device's state at the
//! fetch, so a read mode that answers a miss pattern or refuses reads does the
//! same for the fetch. It then maps that copy read-execute on every processor
//! that maps the page and resumes the slice. A page that is not wholly one
//! such device's window is not executable, and the fault stands.
//!
//! The copy holds while the device's epoch stands. At each slice start, pages
//! whose device moved its epoch return to the device's MMIO, so the next fetch
//! reads the new contents. Keyframes save and restore with every page
//! returned: the copies are host state, derived from device state.
use super::{Binding, Board, XipPage, read, write};
use fujimod_emulator_cpu::Mmio;

const PAGE: u64 = 0x1000;

impl Board {
    /// Copy the device page the fetch at `pc` (or the halfword after it,
    /// for an instruction that crosses into the next page) needs and map it
    /// for execution. False when neither page is an executable device page.
    pub(super) fn execute_in_place(&self, core: usize, pc: u64) -> Result<bool, String> {
        let pc = pc & !1;
        for address in [pc, pc + 2] {
            let Some(physical) = self.mmio_physical(core, address & !(PAGE - 1)) else {
                continue;
            };
            if self.xip.borrow().contains_key(&physical) {
                continue;
            }
            let Some((device, epoch)) = self.state.borrow().bus.execute_in_place(physical) else {
                continue;
            };
            let mut data = Box::new([0u8; PAGE as usize]);
            {
                let mut state = self.state.borrow_mut();
                let cpus = self.cpus.clone();
                state.with_host(&cpus, |bus, host| {
                    for (index, word) in data.chunks_exact_mut(4).enumerate() {
                        let value = bus
                            .access(host, core, pc, physical + 4 * index as u64, 4, None)
                            .map_err(|error| {
                                format!("Execute-in-place page {physical:#x} read refused: {error}")
                            })?;
                        word.copy_from_slice(&(value as u32).to_le_bytes());
                    }
                    Ok(())
                })?;
            }
            let mut maps = Vec::new();
            for &(owner, base, bytes, window) in &self.mmio {
                if physical < window || physical + PAGE > window + bytes as u64 {
                    continue;
                }
                let virtual_page = base + (physical - window);
                let cpu = &self.cpus[owner];
                cpu.unmap(virtual_page, PAGE as usize)
                    .map_err(|e| format!("Unmap execute-in-place page {virtual_page:#x}: {e}"))?;
                // SAFETY: the copy lives in `xip` until every map of it is
                // removed (`return_page`) or the board drops its processors.
                unsafe {
                    cpu.map_backing(
                        virtual_page,
                        PAGE as usize,
                        5,
                        std::ptr::NonNull::new(data.as_mut_ptr()).unwrap(),
                    )
                }
                .map_err(|e| format!("Map execute-in-place page {virtual_page:#x}: {e}"))?;
                maps.push((owner, virtual_page));
            }
            self.xip.borrow_mut().insert(physical, XipPage { data, device, epoch, maps });
            return Ok(true);
        }
        Ok(false)
    }

    /// Return every page whose device moved its epoch to the device.
    pub(super) fn refresh_execute_in_place(&self) -> Result<(), String> {
        if self.xip.borrow().is_empty() {
            return Ok(());
        }
        let stale: Vec<u64> = {
            let state = self.state.borrow();
            self.xip
                .borrow()
                .iter()
                .filter(|(_, page)| state.bus.execute_in_place_epoch(page.device) != Some(page.epoch))
                .map(|(&physical, _)| physical)
                .collect()
        };
        for physical in stale {
            self.return_page(physical)?;
        }
        Ok(())
    }

    /// Return every execute-in-place page to its device.
    pub(super) fn return_execute_in_place(&self) -> Result<(), String> {
        let pages: Vec<u64> = self.xip.borrow().keys().copied().collect();
        for physical in pages {
            self.return_page(physical)?;
        }
        Ok(())
    }

    fn return_page(&self, physical: u64) -> Result<(), String> {
        let Some(page) = self.xip.borrow_mut().remove(&physical) else {
            return Ok(());
        };
        for &(core, virtual_page) in &page.maps {
            let cpu = &self.cpus[core];
            cpu.unmap(virtual_page, PAGE as usize)
                .map_err(|e| format!("Unmap execute-in-place page {virtual_page:#x}: {e}"))?;
            let mut bindings = self.xip_bindings.borrow_mut();
            let binding = bindings.entry((core, virtual_page)).or_insert_with(|| {
                let state = self.state.borrow();
                Box::new(Binding {
                    state: self.state.clone(),
                    callback_service_time: state.callback_service_time.clone(),
                    cpus: std::rc::Rc::downgrade(&self.cpus),
                    mmio_callback_counts: self.mmio_callback_counts.clone(),
                    core,
                    base: physical,
                    published_reads: Vec::new(),
                    published_hits: std::cell::Cell::new(0),
                })
            });
            let data = (&**binding as *const Binding).cast_mut().cast();
            // SAFETY: the binding lives in `xip_bindings` as long as the board.
            unsafe { cpu.map_mmio(virtual_page, PAGE as usize, Mmio { read, write, data }) }
                .map_err(|e| format!("Return execute-in-place page {virtual_page:#x} to its device: {e}"))?;
        }
        drop(page.data);
        Ok(())
    }

    /// The physical address `core` maps as device MMIO at `virtual_page`.
    fn mmio_physical(&self, core: usize, virtual_page: u64) -> Option<u64> {
        self.mmio.iter().find_map(|&(owner, base, bytes, physical)| {
            (owner == core && virtual_page >= base && virtual_page + PAGE <= base + bytes as u64)
                .then(|| physical + (virtual_page - base))
        })
    }
}
