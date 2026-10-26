//! Passive host continuation boundaries. None of this state is modeled or saved.
use super::{Board, State, bus::Bus, factory::MemoryMap};
use fujimod_emulator_cpu::{Access, Cpu, Hook, HookId, HookRange};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    ffi::c_void,
    rc::{Rc, Weak},
};

#[derive(Default, serde::Serialize)]
pub struct ReadSet {
    pub stores: Vec<(u64, u64)>,
    pub devices: BTreeSet<usize>,
}
#[derive(Default)]
pub(super) struct Stops {
    pub stores: Vec<(u64, u64)>,
    /// Each watched range's bytes when the watch began; empty for a range too
    /// large to keep, which any store fires.
    pub values: Vec<Vec<u8>>,
    pub devices: BTreeSet<usize>,
    pub fired: bool,
}
/// Largest watched range whose bytes are kept to compare stores against.
const COMPARED_RANGE_BYTES: u64 = 1 << 20;
impl Stops {
    /// A store fires the watch only when it changes a watched byte: firmware
    /// that rewrites a status word with its current value does not end a wait.
    /// `written` holds the bytes at `address` after the store. `stores` is
    /// sorted and disjoint.
    pub fn store(&mut self, address: u64, written: &[u8]) {
        let end = address.saturating_add(written.len() as u64);
        let first = self.stores.partition_point(|&(_, hi)| hi <= address);
        self.fired |= self.stores[first..]
            .iter()
            .zip(&self.values[first..])
            .take_while(|&(&(lo, _), _)| lo < end)
            .any(|(&(lo, hi), before)| {
                let (from, to) = (address.max(lo), end.min(hi));
                before.is_empty()
                    || written[(from - address) as usize..(to - address) as usize]
                        != before[(from - lo) as usize..(to - lo) as usize]
            });
    }
}
pub(super) fn fired(stops: &RefCell<Stops>) -> bool {
    stops.borrow().fired
}
pub(super) fn finish_instruction(cpu: &dyn Cpu) -> Result<(), String> {
    let progress = cpu
        .instruction_progress()
        .ok_or("Continuation requires exact instruction progress")?;
    cpu.tighten_instruction_limit(progress)
}
/// Store hooks for one watched set. They stay installed after the
/// continuation ends, so the next wait over the same set (most consecutive
/// polls) reuses them instead of reinstalling every hook.
pub(super) struct StoreWatch {
    /// Each core's address views and hook spans the hooks were installed for.
    cores: Vec<(Vec<MemoryMap>, Vec<(u64, u64)>)>,
    contexts: Vec<Box<Context>>,
    hooks: Vec<(usize, Vec<HookId>)>,
    pub(super) active: bool,
}
struct Context {
    state: Weak<RefCell<State>>,
    cpu: Weak<dyn Cpu>,
    maps: Vec<MemoryMap>,
    core: usize,
}
unsafe fn written(_: Access, address: u64, size: i32, _: i64, data: *mut c_void) {
    let context = unsafe { &*data.cast::<Context>() };
    let (Some(state), Some(cpu)) = (context.state.upgrade(), context.cpu.upgrade()) else {
        return;
    };
    let result = (|| {
        let size = u64::try_from(size).map_err(|_| "Invalid continuation store width")?;
        let state = state.borrow();
        // Retained hooks outside a continuation observe nothing.
        if state.continuation.borrow().stores.is_empty() {
            return Ok(());
        }
        let physical = state
            .decoders
            .observed_physical(context.core, address, size)
            .or_else(|| {
                context
                    .maps
                    .iter()
                    .find(|m| {
                        m.address <= address
                            && address.saturating_add(size) <= m.address + m.size as u64
                    })
                    .map(|m| m.physical + address - m.address)
            });
        if let Some(physical) = physical {
            let mut buffer = [0; 64];
            let mut large = vec![];
            let written = match buffer.get_mut(..size as usize) {
                Some(bytes) => bytes,
                None => {
                    large.resize(size as usize, 0);
                    &mut large[..]
                }
            };
            cpu.read_memory(address, written)?;
            state.continuation.borrow_mut().store(physical, written);
            if fired(&state.continuation) {
                finish_instruction(&*cpu)?;
            }
        }
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        state.borrow_mut().fail(error);
        let _ = cpu.stop();
    }
}
/// Record a half-open span, joining it to the previous one when they overlap
/// or touch: a predicate that reads memory in order keeps a single span.
fn add_span(spans: &mut Vec<(u64, u64)>, (lo, hi): (u64, u64)) {
    if let Some(last) = spans.last_mut()
        && lo <= last.1
        && last.0 <= hi
    {
        last.0 = last.0.min(lo);
        last.1 = last.1.max(hi);
    } else {
        spans.push((lo, hi));
    }
}
fn union(mut spans: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    spans.sort_unstable();
    let mut out: Vec<(u64, u64)> = vec![];
    for (lo, hi) in spans {
        if let Some(last) = out.last_mut()
            && lo <= last.1
        {
            last.1 = last.1.max(hi);
        } else {
            out.push((lo, hi));
        }
    }
    out
}
impl Board {
    pub fn read_set_begin(&self) -> Result<(), String> {
        let mut reads = self.read_set.borrow_mut();
        if reads.is_some() {
            return Err("A predicate read set is already active".into());
        }
        *reads = Some(ReadSet::default());
        Ok(())
    }
    pub fn read_set_end(&self) -> Result<ReadSet, String> {
        let mut reads = self
            .read_set
            .borrow_mut()
            .take()
            .ok_or("No predicate read set is active")?;
        reads.stores = union(reads.stores);
        Ok(reads)
    }
    pub(super) fn track_all_devices(&self, bus: &Bus) {
        if let Some(reads) = self.read_set.borrow_mut().as_mut() {
            reads.devices.extend(0..bus.len());
        }
    }
    pub(super) fn track_device_read(&self, id: usize) {
        if let Some(reads) = self.read_set.borrow_mut().as_mut() {
            reads.devices.insert(id);
        }
    }
    pub(super) fn track_device_kind(&self, kind: &str, bus: &Bus) {
        if let Some(reads) = self.read_set.borrow_mut().as_mut() {
            reads.devices.extend(bus.device_ids(kind));
        }
    }
    pub(super) fn track_physical_read(&self, address: u64, bytes: usize) -> Result<(), String> {
        if let Some(reads) = self.read_set.borrow_mut().as_mut() {
            for span in self.physical_memory.physical_ranges(address, bytes)? {
                add_span(&mut reads.stores, span);
            }
        }
        Ok(())
    }
    pub(super) fn track_cpu_read(
        &self,
        core: usize,
        address: u64,
        bytes: usize,
    ) -> Result<(), String> {
        if self.read_set.borrow().is_some() {
            let physical = self.cpu(core)?.translate(address, Access::Read)?;
            self.track_physical_read(physical, bytes)?;
        }
        Ok(())
    }
    pub fn continuation_begin(
        &mut self,
        stores: Vec<(u64, u64)>,
        devices: BTreeSet<usize>,
        maps: Vec<MemoryMap>,
    ) -> Result<(), String> {
        if self.continuation.as_ref().is_some_and(|watch| watch.active) {
            return Err("A continuation watch is already active".into());
        }
        if stores.len() > 4096
            || stores.iter().any(|&(lo, hi)| lo >= hi)
            || devices
                .iter()
                .any(|&id| id >= self.state.borrow().bus.len())
        {
            return Err("Invalid continuation watches".into());
        }
        if self.cpus.iter().any(|cpu| {
            !cpu.features().instruction_progress || !cpu.features().tighten_instruction_limit
        }) {
            return Err("Continuation watches require exact instruction limits".into());
        }
        let stores = union(stores);
        for &(lo, hi) in &stores {
            let mut cursor = lo;
            while cursor < hi {
                let (base, backing) = self
                    .physical_memory
                    .regions()
                    .iter()
                    .find(|(base, backing)| {
                        *base <= cursor && cursor < *base + backing.len() as u64
                    })
                    .ok_or("Continuation store range has no complete physical backing")?;
                cursor = hi.min(*base + backing.len() as u64);
            }
        }
        let stops = self.state.borrow().continuation.clone();
        let values = stores
            .iter()
            .map(|&(lo, hi)| {
                if hi - lo > COMPARED_RANGE_BYTES {
                    return Ok(vec![]);
                }
                let mut bytes = vec![0; (hi - lo) as usize];
                self.physical_memory.read_host(lo, &mut bytes)?;
                Ok(bytes)
            })
            .collect::<Result<Vec<_>, String>>()?;
        *stops.borrow_mut() = Stops {
            stores: stores.clone(),
            values,
            devices,
            fired: false,
        };
        let cores = (0..self.cpus.len())
            .map(|core| {
                let mut views = maps
                    .iter()
                    .filter(|m| m.core == core)
                    .cloned()
                    .collect::<Vec<_>>();
                views.extend(self.state.borrow().decoders.observation_maps(core));
                let mut spans = vec![];
                for map in &views {
                    for &(lo, hi) in &stores {
                        let begin = lo.max(map.physical);
                        let end = hi.min(map.physical + map.size as u64);
                        if begin < end {
                            spans.push((
                                (map.address + begin - map.physical).saturating_sub(15),
                                map.address + end - map.physical,
                            ));
                        }
                    }
                }
                (views, union(spans))
            })
            .collect::<Vec<_>>();
        self.physical_memory
            .continuation
            .replace(Some(Rc::downgrade(&stops)));
        self.state.borrow_mut().bus.continuation(Some(stops));
        if let Some(watch) = self.continuation.as_mut()
            && watch.cores == cores
        {
            watch.active = true;
            return Ok(());
        }
        let result = self.release_store_watch().and_then(|()| {
            let mut watch = Box::new(StoreWatch {
                cores: cores.clone(),
                contexts: vec![],
                hooks: vec![],
                active: true,
            });
            let installed = (|| {
                for (core, (views, spans)) in cores.into_iter().enumerate() {
                    let cpu = &self.cpus[core];
                    watch.contexts.push(Box::new(Context {
                        state: Rc::downgrade(&self.state),
                        cpu: Rc::downgrade(cpu),
                        maps: views,
                        core,
                    }));
                    let data = (&**watch.contexts.last().unwrap() as *const Context)
                        .cast_mut()
                        .cast();
                    let hooks = spans
                        .into_iter()
                        .map(|(begin, end)| Hook::CompletedStore {
                            callback: written,
                            data,
                            range: HookRange::Inclusive {
                                begin,
                                end: end - 1,
                            },
                        })
                        .collect::<Vec<_>>();
                    if !hooks.is_empty() {
                        watch
                            .hooks
                            .push((core, unsafe { cpu.install_hooks(&hooks) }?));
                    }
                }
                Ok::<_, String>(())
            })();
            self.continuation = Some(watch);
            installed
        });
        if result.is_err() {
            self.continuation_end()?;
            self.release_store_watch()?;
        }
        result
    }
    /// End the active continuation. Its hooks stay installed for the next
    /// continuation over the same set; `release_store_watch` removes them.
    pub fn continuation_end(&mut self) -> Result<bool, String> {
        if let Some(watch) = self.continuation.as_mut() {
            watch.active = false;
        }
        self.physical_memory.continuation.replace(None);
        let mut state = self.state.borrow_mut();
        state.bus.continuation(None);
        let fired = state.continuation.borrow().fired;
        *state.continuation.borrow_mut() = Stops::default();
        Ok(fired)
    }
    /// Remove the hooks retained from the last continuation.
    pub(super) fn release_store_watch(&mut self) -> Result<(), String> {
        if let Some(watch) = self.continuation.take() {
            for (core, hooks) in &watch.hooks {
                self.cpus[*core].remove_hooks(hooks)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "continuation_tests.rs"]
mod tests;
