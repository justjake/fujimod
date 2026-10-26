//! Explicit, bounded CPU/bus observation. No device consumes these records.
pub mod calls;
use super::{Board, State, factory::MemoryMap, scalar};
use fujimod_emulator_cpu::{Access, Cpu, Hook, HookId, HookRange, Model, Register, arm};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    rc::{Rc, Weak},
};
type Span = (u64, u64);
#[derive(Default)]
pub struct Records {
    ranges: Vec<Span>,
    limit: usize,
    rows: Vec<Value>,
    violation: Option<String>,
}
impl Records {
    fn overlaps(&self, address: u64, bytes: u64) -> bool {
        self.ranges
            .iter()
            .any(|&(lo, hi)| address < hi && lo < address.saturating_add(bytes))
    }
    fn push(&mut self, row: Value) {
        if self.violation.is_some() {
            return;
        }
        if self.rows.len() == self.limit {
            self.violation = Some(format!(
                "Memory watch exceeded its {} record bound",
                self.limit
            ));
        } else {
            self.rows.push(row);
        }
    }
    pub(super) fn transfer(&mut self, source: &str, physical: u64, address: u64, bytes: usize) {
        if self.overlaps(physical, bytes as u64) {
            self.push(
                json!({"source":source,"physical":physical,"address":address,
                "size":bytes,"core":null,"pc":null,"instructions":null,"tick":null}),
            );
        }
    }
    pub fn state(&self) -> Value {
        json!({"enabled":true,"ranges":self.ranges,"records":self.rows,"violation":self.violation})
    }
}
pub struct MemoryWatch {
    pub records: Rc<RefCell<Records>>,
    contexts: Vec<Box<MemoryContext>>,
    hooks: Vec<(usize, HookId)>,
}
struct MemoryContext {
    state: Weak<RefCell<State>>,
    cpu: Weak<dyn Cpu>,
    records: Rc<RefCell<Records>>,
    maps: Vec<MemoryMap>,
    core: usize,
}
impl MemoryContext {
    fn record(
        &self,
        source: &str,
        address: u64,
        size: u64,
        value: Option<u64>,
    ) -> Result<(), String> {
        let (Some(state), Some(cpu)) = (self.state.upgrade(), self.cpu.upgrade()) else {
            return Ok(());
        };
        let state = state.borrow();
        let physical = state
            .decoders
            .observed_physical(self.core, address, size)
            .or_else(|| {
                self.maps
                    .iter()
                    .find(|m| {
                        m.address <= address
                            && address.saturating_add(size) <= m.address + m.size as u64
                    })
                    .map(|m| m.physical + address - m.address)
            });
        let Some(physical) = physical else {
            return Ok(());
        };
        let mut records = self.records.borrow_mut();
        if !records.overlaps(physical, size) {
            return Ok(());
        }
        // These ranges describe shared physical backing, not a CPU's private
        // TCM overlay. Refuse an ambiguous receipt rather than label it DRAM.
        for opc2 in 0..2 {
            let tcm = cpu.coprocessor(
                arm::Coprocessor {
                    cp: 15,
                    opc1: 0,
                    crn: 9,
                    crm: 1,
                    opc2,
                    secure: 0,
                    wide: false,
                },
                None,
            )?;
            if tcm & 1 != 0 {
                let base = tcm & 0xfffff000;
                let bytes = 512u64 << ((tcm >> 2) & 31);
                if address < base + bytes && base < address.saturating_add(size) {
                    return Err("Memory watch intersects a CPU-private TCM overlay".into());
                }
            }
        }
        let byte_order = if scalar(&*cpu, Register::Pstate)? & (1 << 9) != 0 {
            "big"
        } else {
            "little"
        };
        let before = if source != "cpu-fetch" {
            if !(1..=16).contains(&size) {
                return Err("Memory watch cannot capture this CPU access width".into());
            }
            let mut bytes = vec![0; size as usize];
            cpu.read_memory(address, &mut bytes)?;
            Some(bytes)
        } else {
            None
        };
        let value = if source == "cpu-read" && size <= 8 {
            let bytes = before.as_ref().unwrap();
            Some(if byte_order == "little" {
                bytes
                    .iter()
                    .rev()
                    .fold(0u64, |v, b| (v << 8) | u64::from(*b))
            } else {
                bytes.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b))
            })
        } else {
            value
        };
        let extra = cpu
            .instruction_progress()
            .unwrap_or(state.accounted[self.core])
            .saturating_sub(state.accounted[self.core]);
        let instructions = state.instructions.iter().sum::<u64>().saturating_add(extra);
        let elapsed = state
            .timeline
            .ratio()
            .advance_phase(state.timeline.phase(), extra)?
            .0;
        let tick = state
            .timeline
            .now()
            .saturating_add(elapsed.min(u64::MAX as u128) as u64);
        records.push(
            json!({"source":source,"address":address,"physical":physical,"size":size,
            "value":value,"before":before,"byte_order":byte_order,
            "core":self.core,"pc":scalar(&*cpu,Register::Pc)?,
            "instructions":instructions,"tick":tick}),
        );
        Ok(())
    }
    fn fail(&self, error: String) {
        self.records.borrow_mut().violation.get_or_insert(error);
    }
}
unsafe fn access(access: Access, address: u64, size: i32, value: i64, data: *mut c_void) {
    let context = unsafe { &*data.cast::<MemoryContext>() };
    let result = u64::try_from(size)
        .map_err(|_| "invalid observed memory width".into())
        .and_then(|size| {
            if size == 0 {
                return Err("invalid observed memory width".into());
            }
            context.record(
                if access == Access::Write {
                    "cpu-write"
                } else {
                    "cpu-read"
                },
                address,
                size,
                (access == Access::Write && size <= 8)
                    .then_some(value as u64 & (u64::MAX >> (64 - size.min(8) * 8))),
            )
        });
    if let Err(error) = result {
        context.fail(error);
    }
}
unsafe fn fetch(address: u64, size: u32, data: *mut c_void) {
    let context = unsafe { &*data.cast::<MemoryContext>() };
    if let Err(error) = context.record("cpu-fetch", address, size as u64, None) {
        context.fail(error);
    }
}
fn union(mut ranges: Vec<Span>) -> Vec<Span> {
    ranges.sort_unstable();
    let mut out: Vec<Span> = vec![];
    for (lo, hi) in ranges {
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
    pub fn transaction_watch_begin(&self, ranges: Vec<Span>, limit: usize) -> Result<(), String> {
        self.state
            .try_borrow_mut()
            .map_err(|_| "Bus transaction observation requires a paused board")?
            .bus
            .transaction_watch_begin(ranges, limit)
    }

    pub fn transaction_watch_end(&self) -> Result<super::bus::TransactionBatch, String> {
        self.state
            .try_borrow_mut()
            .map_err(|_| "Bus transaction observation requires a paused board")?
            .bus
            .transaction_watch_end()
    }

    pub fn transaction_watch_active(&self) -> Result<bool, String> {
        Ok(self
            .state
            .try_borrow()
            .map_err(|_| "Bus transaction observation requires a paused board")?
            .bus
            .transactions_active())
    }

    pub fn memory_watch_begin(
        &self,
        ranges: Vec<Span>,
        limit: usize,
        maps: Vec<MemoryMap>,
    ) -> Result<Box<MemoryWatch>, String> {
        if ranges.is_empty()
            || ranges.len() > 256
            || limit == 0
            || limit > 1 << 22
            || ranges.iter().any(|&(lo, hi)| lo >= hi || hi > 1 << 32)
            || self.cpus.iter().any(|cpu| cpu.model() != Model::CortexR5F)
        {
            return Err("Memory watch requires bounded ranges and R-profile processors".into());
        }
        let ranges = union(ranges);
        for &(lo, hi) in &ranges {
            let mut cursor = lo;
            while cursor < hi {
                let (base, backing) = self
                    .physical_memory
                    .regions()
                    .iter()
                    .find(|(base, b)| *base <= cursor && cursor < *base + b.len() as u64)
                    .ok_or("Memory watch range has no complete physical backing")?;
                cursor = hi.min(*base + backing.len() as u64);
            }
        }
        let records = Rc::new(RefCell::new(Records {
            ranges: ranges.clone(),
            limit,
            ..Default::default()
        }));
        let mut watch = Box::new(MemoryWatch {
            records: records.clone(),
            contexts: vec![],
            hooks: vec![],
        });
        for (core, cpu) in self.cpus.iter().enumerate() {
            let mut views = maps
                .iter()
                .filter(|m| m.core == core)
                .cloned()
                .collect::<Vec<_>>();
            views.extend(self.state.borrow().decoders.observation_maps(core));
            let mut spans = vec![];
            for map in &views {
                for &(lo, hi) in &ranges {
                    let start = lo.max(map.physical);
                    let end = hi.min(map.physical + map.size as u64);
                    if start < end {
                        spans.push((
                            (map.address + start - map.physical).saturating_sub(15),
                            map.address + end - map.physical,
                        ));
                    }
                }
            }
            watch.contexts.push(Box::new(MemoryContext {
                state: Rc::downgrade(&self.state),
                cpu: Rc::downgrade(cpu),
                records: records.clone(),
                maps: maps.iter().filter(|m| m.core == core).cloned().collect(),
                core,
            }));
            let data = (&**watch.contexts.last().unwrap() as *const MemoryContext)
                .cast_mut()
                .cast();
            for (begin, end) in union(spans) {
                for hook in [
                    Hook::Memory {
                        callback: access,
                        data,
                        read: true,
                        write: true,
                        range: HookRange::Inclusive {
                            begin,
                            end: end - 1,
                        },
                    },
                    Hook::Instruction {
                        callback: fetch,
                        data,
                        range: HookRange::Inclusive {
                            begin,
                            end: end - 1,
                        },
                    },
                ] {
                    match unsafe { cpu.install_hook(hook) } {
                        Ok(id) => watch.hooks.push((core, id)),
                        Err(error) => {
                            self.memory_watch_end(&mut watch)?;
                            return Err(error);
                        }
                    }
                }
            }
        }
        self.physical_memory
            .observation
            .replace(Some(Rc::downgrade(&records)));
        Ok(watch)
    }
    pub fn memory_watch_end(&self, watch: &mut MemoryWatch) -> Result<(), String> {
        for (core, hook) in watch.hooks.drain(..) {
            self.cpus[core].remove_hook(hook)?;
        }
        self.physical_memory.observation.replace(None);
        Ok(())
    }
}

/// Repeated entry observations stop before the selected instruction. Resume
/// executes that instruction once, even when it is itself a loop target.
pub(super) struct EntryPoints {
    addresses: Vec<u64>,
    pause: Rc<Cell<bool>>,
    event: RefCell<Option<u64>>,
    skip: Cell<Option<u64>>,
}
impl EntryPoints {
    fn hit(&self, pc: u64) -> bool {
        if self.addresses.binary_search(&pc).is_err() {
            return false;
        }
        if self.skip.get() == Some(pc) {
            self.skip.set(None);
            return false;
        }
        self.event.replace(Some(pc));
        self.skip.set(Some(pc));
        self.pause.set(true);
        true
    }
}
pub struct InstructionPoints {
    cpu: Weak<dyn Cpu>,
    entry: Rc<EntryPoints>,
    hooks: Vec<HookId>,
}
unsafe fn instruction(pc: u64, _: u32, data: *mut c_void) {
    let points = unsafe { &*data.cast::<InstructionPoints>() };
    if points.entry.hit(pc)
        && let Some(cpu) = points.cpu.upgrade()
    {
        let _ = cpu.stop_before_instruction();
    }
}
impl InstructionPoints {
    pub fn event(&self) -> Option<u64> {
        self.entry.event.borrow_mut().take()
    }
    pub fn resume(&self, current_pc: u64, observed_pc: u64) {
        self.entry
            .skip
            .set((current_pc == observed_pc).then_some(observed_pc));
    }
}
impl super::State {
    pub(super) fn observe_entry(
        &mut self,
        core: usize,
        pc: u64,
        cpu: &dyn Cpu,
    ) -> Result<bool, String> {
        self.entry_points[core].retain(|entry| entry.strong_count() != 0);
        for entry in self.entry_points[core].iter().filter_map(Weak::upgrade) {
            if entry.hit(pc) {
                cpu.stop()?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}
impl Board {
    pub fn instruction_points_begin(
        &self,
        core: usize,
        mut addresses: Vec<u64>,
    ) -> Result<Box<InstructionPoints>, String> {
        let cpu = self.cpu(core)?;
        if addresses.len() > 4096 || addresses.iter().any(|pc| pc & 1 != 0) {
            return Err("Instruction points require bounded aligned addresses".into());
        }
        addresses.sort_unstable();
        addresses.dedup();
        let mut points = Box::new(InstructionPoints {
            cpu: Rc::downgrade(&self.cpus[core]),
            entry: Rc::new(EntryPoints {
                addresses: addresses.clone(),
                pause: self.host_pause.clone(),
                event: RefCell::new(None),
                skip: Cell::new(None),
            }),
            hooks: vec![],
        });
        if !cpu.features().instruction_progress {
            // These processors use the shared all-instruction callback. Check
            // there before architectural emulation and instruction accounting.
            self.state.borrow_mut().entry_points[core].push(Rc::downgrade(&points.entry));
            return Ok(points);
        }
        let data = (&*points as *const InstructionPoints).cast_mut().cast();
        for pc in addresses {
            match unsafe {
                cpu.install_hook(Hook::Instruction {
                    callback: instruction,
                    data,
                    range: HookRange::Inclusive { begin: pc, end: pc },
                })
            } {
                Ok(id) => points.hooks.push(id),
                Err(error) => {
                    self.instruction_points_end(core, &mut points)?;
                    return Err(error);
                }
            }
        }
        Ok(points)
    }
    pub fn instruction_points_end(
        &self,
        core: usize,
        points: &mut InstructionPoints,
    ) -> Result<(), String> {
        self.cpu(core)?;
        self.state.borrow_mut().entry_points[core].retain(|entry| {
            entry
                .upgrade()
                .is_some_and(|entry| !Rc::ptr_eq(&entry, &points.entry))
        });
        for hook in points.hooks.drain(..) {
            self.cpu(core)?.remove_hook(hook)?;
        }
        Ok(())
    }
}

pub struct FunctionWatch {
    pub recorder: Rc<RefCell<calls::Recorder>>,
    contexts: Vec<Box<FunctionContext>>,
    hooks: Vec<(usize, HookId)>,
}
struct FunctionContext {
    state: Weak<RefCell<State>>,
    cpu: Weak<dyn Cpu>,
    recorder: Rc<RefCell<calls::Recorder>>,
    maps: Vec<MemoryMap>,
    core: usize,
    last: Cell<Option<(u64, u64)>>,
    error: RefCell<Option<String>>,
}
impl FunctionContext {
    fn observe(&self, pc: u64) -> Result<(), String> {
        let (Some(state), Some(cpu)) = (self.state.upgrade(), self.cpu.upgrade()) else {
            return Ok(());
        };
        let state = state.borrow();
        let canonical = |address: u64| {
            // Explicit instruction addresses also cover ROM shadows and
            // CPU-private views. Only resolve an alias when the raw address
            // itself is not selected by this observer.
            if self.recorder.borrow().watches(address) {
                return address;
            }
            state
                .decoders
                .observed_physical(self.core, address, 1)
                .or_else(|| {
                    self.maps
                        .iter()
                        .find(|m| m.address <= address && address < m.address + m.size as u64)
                        .map(|m| m.physical + address - m.address)
                })
                .unwrap_or(address)
        };
        let address = canonical(pc);
        if !self.recorder.borrow().watches(address) {
            return Ok(());
        }
        let extra = cpu
            .instruction_progress()
            .ok_or("function watch requires exact CPU progress")?
            .saturating_sub(state.accounted[self.core]);
        let instructions = state
            .total_instructions()?
            .checked_add(extra)
            .ok_or("function watch instruction overflow")?;
        // A host point can stop and resume this same instruction. Capture its
        // original registers once, before any explicit discovery substitution.
        if self.last.replace(Some((pc, instructions))) == Some((pc, instructions)) {
            return Ok(());
        }
        let mut registers = [0; 16];
        for (index, register) in registers.iter_mut().take(15).enumerate() {
            *register = scalar(&*cpu, arm::CORE[index])? as u32;
        }
        registers[15] = scalar(&*cpu, Register::Pstate)? as u32;
        let resume_pc = canonical(u64::from(registers[14]) & !1);
        self.recorder
            .borrow_mut()
            .observe(self.core, address, instructions, registers, resume_pc);
        Ok(())
    }
}
unsafe fn function(pc: u64, _: u32, data: *mut c_void) {
    let context = unsafe { &*data.cast::<FunctionContext>() };
    if let Err(error) = context.observe(pc) {
        context.error.borrow_mut().get_or_insert(error);
    }
}
impl FunctionWatch {
    pub fn check(&self) -> Result<(), String> {
        for context in &self.contexts {
            if let Some(error) = &*context.error.borrow() {
                return Err(error.clone());
            }
        }
        Ok(())
    }
}
impl Board {
    pub fn function_watch_begin(
        &self,
        recorder: calls::Recorder,
        maps: Vec<MemoryMap>,
    ) -> Result<Box<FunctionWatch>, String> {
        let recorder = Rc::new(RefCell::new(recorder));
        let mut watch = Box::new(FunctionWatch {
            recorder: recorder.clone(),
            contexts: vec![],
            hooks: vec![],
        });
        for (core, cpu) in self.cpus.iter().enumerate() {
            if cpu.model() != Model::CortexR5F {
                self.function_watch_end(&mut watch)?;
                return Err("Function watch requires R-profile processors".into());
            }
            watch.contexts.push(Box::new(FunctionContext {
                state: Rc::downgrade(&self.state),
                cpu: Rc::downgrade(cpu),
                recorder: recorder.clone(),
                maps: maps.iter().filter(|m| m.core == core).cloned().collect(),
                core,
                last: Cell::new(None),
                error: RefCell::new(None),
            }));
            let data = (&**watch.contexts.last().unwrap() as *const FunctionContext)
                .cast_mut()
                .cast();
            match unsafe {
                cpu.install_hook(Hook::Instruction {
                    callback: function,
                    data,
                    range: HookRange::All,
                })
            } {
                Ok(id) => watch.hooks.push((core, id)),
                Err(error) => {
                    self.function_watch_end(&mut watch)?;
                    return Err(error);
                }
            }
        }
        Ok(watch)
    }
    pub fn function_watch_end(&self, watch: &mut FunctionWatch) -> Result<(), String> {
        for (core, hook) in watch.hooks.drain(..) {
            self.cpus[core].remove_hook(hook)?;
        }
        Ok(())
    }
}
