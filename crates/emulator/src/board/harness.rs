//! Explicit isolated caller instrumentation above the common runtime.
//! These optional CPU observers never provide peripheral capabilities.
pub mod arm32;
pub mod arm64;
use super::{Board, scalar};
use fujimod_emulator_cpu::{Access, Cpu, Hook, HookId, HookRange, Register, arm};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    ffi::c_void,
    rc::{Rc, Weak},
};

type Span = (u64, u64);
#[derive(serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alias {
    pub address: u64,
    pub physical: u64,
    pub size: u64,
}
#[derive(serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Memory {
    pub aliases: Vec<Alias>,
    /// Select store starts. Architectural CPU/bus checks still validate the
    /// complete transfer, including faults at a region boundary.
    pub observed: Vec<Span>,
}
impl Memory {
    fn validate(&self) -> Result<(), String> {
        if self.aliases.len() > 1024
            || self.observed.len() > 1024
            || self
                .observed
                .iter()
                .any(|&(lo, hi)| lo >= hi || hi > 1 << 32)
        {
            return Err("invalid isolated caller memory observation ranges".into());
        }
        for (index, alias) in self.aliases.iter().enumerate() {
            if alias.size == 0
                || alias
                    .address
                    .checked_add(alias.size)
                    .is_none_or(|end| end > 1 << 32)
                || alias
                    .physical
                    .checked_add(alias.size)
                    .is_none_or(|end| end > 1 << 32)
                || self.aliases[..index].iter().any(|peer| {
                    alias.address < peer.address + peer.size
                        && peer.address < alias.address + alias.size
                })
            {
                return Err("invalid or overlapping isolated caller memory aliases".into());
            }
        }
        Ok(())
    }
    fn physical(&self, address: u64) -> u64 {
        self.aliases
            .iter()
            .find(|alias| alias.address <= address && address < alias.address + alias.size)
            .map_or(address, |alias| alias.physical + address - alias.address)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub core: usize,
    pub inject_interrupts: bool,
    pub deliver_faults: bool,
    pub memory: Memory,
    pub return_address: u64,
    pub breakpoints: Vec<u64>,
    pub stop_addresses: Vec<u64>,
    pub unarmed: Vec<u64>,
    pub ranges: Option<Vec<Span>>,
    pub vectors: Vec<Span>,
    pub vector_opcode: u32,
    pub stack: Span,
    pub allowed_stores: Option<Vec<Span>>,
    pub record_stores: bool,
    pub include_stack: bool,
    pub store_limit: usize,
    pub coverage: bool,
    pub count_ranges: Vec<Span>,
    pub trace: usize,
}
#[derive(Default)]
struct Observations {
    event: Option<Value>,
    skip_once: Option<u64>,
    entered: bool,
    executed: BTreeSet<u64>,
    stores: Vec<Value>,
    counts: Vec<u64>,
    trace: Vec<u64>,
    trace_total: u64,
    checked: u64,
    accepted: u64,
    vector_fetches: u64,
    range_fetches: Vec<u64>,
    vector_range_fetches: Vec<u64>,
    halt: Option<Value>,
    faults: Vec<Value>,
    irq_events: Vec<Value>,
}
pub struct Session {
    options: Options,
    cpu: Weak<dyn Cpu>,
    pause: Rc<Cell<bool>>,
    observed: RefCell<Observations>,
    hooks: Vec<HookId>,
}
fn contains(ranges: &[Span], address: u64, size: u64) -> Option<usize> {
    address.checked_add(size).and_then(|end| {
        ranges
            .iter()
            .position(|&(lo, hi)| lo <= address && end <= hi)
    })
}
impl Session {
    fn physical(&self, address: u64) -> u64 {
        self.options.memory.physical(address)
    }
    fn stop(&self, cpu: &dyn Cpu, event: Value, before: bool) {
        self.observed.borrow_mut().event = Some(event);
        self.pause.set(true);
        let result = if before {
            cpu.stop_before_instruction()
        } else {
            cpu.stop()
        };
        if let Err(error) = result {
            self.observed.borrow_mut().event = Some(json!({"kind":"failure","message":error}));
        }
    }
    fn instruction(&self, pc: u64, size: u32) -> Result<(), String> {
        let cpu = self
            .cpu
            .upgrade()
            .ok_or("isolated caller processor is gone")?;
        let physical = self.physical(pc);
        let mut o = self.observed.borrow_mut();
        let skipped = o.skip_once.take() == Some(pc);
        let unarmed = !o.entered && self.options.unarmed.contains(&pc);
        if !unarmed && pc == self.options.return_address {
            drop(o);
            self.stop(&*cpu, json!({"kind":"return","pc":pc}), true);
            return Ok(());
        }
        if !skipped && let Some(ranges) = &self.options.ranges {
            o.checked += 1;
            if let Some(index) = contains(ranges, physical, size as u64) {
                o.accepted += 1;
                o.range_fetches[index] += 1;
            } else {
                let vector = contains(&self.options.vectors, pc, size as u64);
                let mut bytes = [0; 4];
                cpu.read_memory(pc, &mut bytes)?;
                if let Some(index) = vector
                    && u32::from_le_bytes(bytes) == self.options.vector_opcode
                {
                    o.accepted += 1;
                    o.vector_fetches += 1;
                    o.vector_range_fetches[index] += 1;
                } else {
                    let registers = (0..4)
                        .map(|r| scalar(&*cpu, arm::CORE[r]))
                        .collect::<Result<Vec<_>, _>>()?;
                    let halt = json!({"kind":"blocked","reason":"instruction fetch outside declared code ranges",
                        "pc":pc,"size":size,"registers":registers,"opcode":u32::from_le_bytes(bytes)});
                    o.halt = Some(halt.clone());
                    drop(o);
                    self.stop(&*cpu,json!({"kind":"failure","message":"Isolated driver fetch guard: instruction fetch outside declared code ranges","fetch_guard":halt}),true);
                    return Ok(());
                }
            }
        }
        if !skipped && !unarmed && self.options.breakpoints.contains(&pc) {
            o.entered = true;
            drop(o);
            self.stop(&*cpu, json!({"kind":"breakpoint","pc":pc}), true);
            return Ok(());
        }
        o.entered = true;
        if self.options.coverage {
            o.executed.insert(pc);
        }
        for (index, &(lo, hi)) in self.options.count_ranges.iter().enumerate() {
            if lo <= physical && physical < hi {
                o.counts[index] += 1;
            }
        }
        if o.trace.len() < self.options.trace {
            o.trace.push(pc);
        }
        o.trace_total += 1;
        Ok(())
    }
    fn store(&self, address: u64, size: i32, value: i64) -> Result<(), String> {
        if !self.options.record_stores && self.options.allowed_stores.is_none() {
            return Ok(());
        }
        let cpu = self
            .cpu
            .upgrade()
            .ok_or("isolated caller processor is gone")?;
        let pc = scalar(&*cpu, Register::Pc)?;
        let width = u64::try_from(size).map_err(|_| "invalid observed store size")?;
        if !(1..=8).contains(&width) {
            return Err("invalid observed store width".into());
        }
        let physical = self.physical(address);
        // MMIO is recorded exclusively at the shared bus boundary.
        if !self
            .options
            .memory
            .observed
            .iter()
            .any(|&(lo, hi)| lo <= address && address < hi)
        {
            return Ok(());
        }
        let end = physical
            .checked_add(width)
            .ok_or("observed store overflow")?;
        let (lo, hi) = self.options.stack;
        let stack = lo <= physical && end <= hi;
        let straddle = !stack && physical < hi && lo < end;
        let value = value as u64 & (u64::MAX >> (64 - width * 8));
        let mut o = self.observed.borrow_mut();
        let violation = if straddle && !self.options.include_stack {
            Some(format!(
                "Store at {physical:#x} width {width} (pc {pc:#x}) straddles the caller stack window"
            ))
        } else if stack && !self.options.include_stack {
            return Ok(());
        } else if !stack
            && !straddle
            && self
                .options
                .allowed_stores
                .as_ref()
                .is_some_and(|r| contains(r, physical, width).is_none())
        {
            Some(format!(
                "Store outside allowed ranges at {physical:#x} width {width} value {value:#x} (pc {pc:#x})"
            ))
        } else if o.stores.len() >= self.options.store_limit {
            Some(format!(
                "Store log exceeded its {} entry bound at pc {pc:#x}",
                self.options.store_limit
            ))
        } else {
            None
        };
        if let Some(message) = violation {
            drop(o);
            self.stop(
                &*cpu,
                json!({"kind":"failure","message":format!("Isolated driver store log: {message}")}),
                false,
            );
        } else {
            let mut bytes = [0; 4];
            cpu.read_memory(pc, &mut bytes)?;
            let word = u32::from_le_bytes(bytes);
            let thumb = scalar(&*cpu, Register::Pstate)? & 32 != 0;
            let exclusive = if thumb {
                word & 0xfff0 == 0xe840
            } else {
                word & 0x0ff00ff0 == 0x01800f90
            };
            o.stores.push(
                json!({"physical":physical,"virtual":address,"width":width,"value":value,
                "pc":pc,"stack":stack,"exclusive":exclusive}),
            );
        }
        Ok(())
    }
    pub fn core(&self) -> usize {
        self.options.core
    }
    pub fn interrupt_input(&self) -> bool {
        self.options.inject_interrupts
    }
    /// A paused observer resumes its intercepted instruction atomically. An
    /// armed caller stop/return wins over entry to another exception.
    pub fn resume_policy(&self, pc: u64) -> Option<bool> {
        let o = self.observed.borrow();
        let step = o.skip_once == Some(pc);
        let unarmed = !o.entered && self.options.unarmed.contains(&pc);
        (step
            || !unarmed
                && (pc == self.options.return_address || self.options.stop_addresses.contains(&pc)))
        .then_some(step)
    }
    pub fn resume(&self, skip_once: Option<u64>) {
        let mut o = self.observed.borrow_mut();
        o.event = None;
        o.skip_once = skip_once;
    }
    pub fn state(&self) -> Value {
        let o = self.observed.borrow();
        json!({"memory":self.options.memory,"event":o.event,"faults":o.faults,"irq_events":o.irq_events,"executed":o.executed,"records":o.stores,"counts":o.counts,
            "trace":o.trace,"trace_total":o.trace_total,
            "guard":{"enabled":self.options.ranges.is_some(),"checked_fetches":o.checked,
                "accepted_fetches":o.accepted,"vector_fetches":o.vector_fetches,"range_fetches":o.range_fetches,
                "vector_range_fetches":o.vector_range_fetches,"ranges":self.options.ranges,
                "vector_ranges":self.options.vectors,"halted":o.halt.is_some(),"halt":o.halt}})
    }
}
unsafe fn instruction(pc: u64, size: u32, data: *mut c_void) {
    let s = unsafe { &*data.cast::<Session>() };
    if let Err(message) = s.instruction(pc, size)
        && let Some(cpu) = s.cpu.upgrade()
    {
        s.stop(&*cpu, json!({"kind":"failure","message":message}), true);
    }
}
unsafe fn store(_: Access, address: u64, size: i32, value: i64, data: *mut c_void) {
    let s = unsafe { &*data.cast::<Session>() };
    if let Err(message) = s.store(address, size, value)
        && let Some(cpu) = s.cpu.upgrade()
    {
        s.stop(&*cpu, json!({"kind":"failure","message":message}), false);
    }
}
unsafe fn exception(exception: arm::Exception, data: *mut c_void) {
    let s = unsafe { &*data.cast::<Session>() };
    let Some(cpu) = s.cpu.upgrade() else {
        return;
    };
    let pc = scalar(&*cpu, Register::Pc).unwrap_or(0);
    if exception == arm::Exception::Irq {
        s.observed
            .borrow_mut()
            .irq_events
            .push(json!({"name":"Architectural IRQ entry","cpu":s.options.core,"pc":pc}));
        return;
    }
    let name = match exception {
        arm::Exception::UndefinedInstruction => "Architectural undefined instruction entry",
        arm::Exception::PrefetchAbort => "Architectural MPU permission prefetch abort entry",
        arm::Exception::DataAbort => "Architectural MPU permission data abort entry",
        _ => "Architectural exception entry",
    };
    let fault_pc = scalar(&*cpu, arm::CORE[14]).unwrap_or(0).wrapping_sub(
        if exception == arm::Exception::DataAbort {
            8
        } else {
            4
        },
    );
    s.observed
        .borrow_mut()
        .faults
        .push(json!({"name":name,"cpu":s.options.core,"pc":pc,
        "fault_pc":format!("{fault_pc:#x}")}));
    if !s.options.deliver_faults {
        s.stop(
            &*cpu,
            json!({"kind":"failure","message":"Native synchronous fault in isolated driver"}),
            false,
        );
    }
}
impl Board {
    pub fn harness_begin(&self, options: Options) -> Result<Box<Session>, String> {
        let cpu = self
            .cpus
            .get(options.core)
            .ok_or("invalid isolated caller core")?;
        options.memory.validate()?;
        if options.store_limit == 0
            || options.store_limit > 1 << 24
            || options.trace > 1 << 20
            || options.count_ranges.len() > 256
            || options.breakpoints.len() > 4096
            || options.stack.0 >= options.stack.1
            || options
                .ranges
                .as_ref()
                .is_some_and(|r| r.iter().any(|&(lo, hi)| lo >= hi || hi > 1 << 32))
        {
            return Err("invalid isolated caller observation bounds".into());
        }
        let mut session = Box::new(Session {
            observed: RefCell::new(Observations {
                counts: vec![0; options.count_ranges.len()],
                range_fetches: vec![0; options.ranges.as_ref().map_or(0, Vec::len)],
                vector_range_fetches: vec![0; options.vectors.len()],
                ..Default::default()
            }),
            options,
            cpu: Rc::downgrade(cpu),
            pause: self.host_pause.clone(),
            hooks: vec![],
        });
        let data = (&*session as *const Session).cast_mut().cast();
        session.hooks.push(unsafe {
            cpu.install_hook(Hook::Instruction {
                callback: instruction,
                data,
                range: HookRange::All,
            })?
        });
        if session.options.record_stores || session.options.allowed_stores.is_some() {
            let hook = unsafe {
                cpu.install_hook(Hook::Memory {
                    callback: store,
                    data,
                    read: false,
                    write: true,
                    range: HookRange::All,
                })
            };
            match hook {
                Ok(hook) => session.hooks.push(hook),
                Err(error) => {
                    self.harness_end(&mut session)?;
                    return Err(error);
                }
            }
        }
        match unsafe {
            cpu.install_hook(Hook::Exception {
                callback: exception,
                data,
            })
        } {
            Ok(hook) => session.hooks.push(hook),
            Err(error) => {
                self.harness_end(&mut session)?;
                return Err(error);
            }
        }
        self.isolated_core.set(Some(session.options.core));
        self.state.borrow_mut().interrupt_delivery[session.options.core] =
            session.options.inject_interrupts;
        Ok(session)
    }
    pub fn harness_end(&self, session: &mut Session) -> Result<(), String> {
        let cpu = self.cpu(session.options.core)?;
        for hook in session.hooks.drain(..) {
            cpu.remove_hook(hook)?;
        }
        self.isolated_core.set(None);
        self.state.borrow_mut().interrupt_delivery.fill(true);
        Ok(())
    }
    /// Explicit single-caller scheduling input; no device sees this selection.
    pub fn harness_select_core(&self, core: usize) -> Result<(), String> {
        if core >= self.cpus.len() {
            return Err("invalid isolated caller core".into());
        }
        let mut s = self.state.borrow_mut();
        for c in 0..self.cpus.len() {
            s.released[c] = c == core;
            s.waiting[c] = false;
        }
        self.next_core.set(core);
        self.slice_left.set(0);
        Ok(())
    }
    pub fn harness_interrupt_delivery(&self, core: usize, allowed: bool) {
        self.state.borrow_mut().interrupt_delivery[core] = allowed;
    }
    pub fn harness_advance(&mut self, tick: u64, quantum: usize) -> Result<(), String> {
        let released = self.state.borrow().released.clone();
        self.state.borrow_mut().released.fill(false);
        let result = self.run_until(tick, quantum);
        self.state.borrow_mut().released = released;
        result
    }
    pub fn observe_transactions(&self, limit: Option<usize>) {
        let mut state = self.state.borrow_mut();
        state.bus.observe_transactions(limit);
        state.irq_observations = limit.map(|limit| (limit, vec![]));
    }
    pub fn transactions(&self) -> Value {
        let s = self.state.borrow();
        let (rows, omitted) = s.bus.transactions();
        json!({"transactions":rows,"omitted":omitted,"irq_events":s.irq_observations.as_ref().map(|(_,rows)| rows)})
    }
}
