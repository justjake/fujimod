//! Hypervisor.framework execution for AArch64 exact and native interactive runs.
//!
//! A Vm is one physical address space. Its Core values remain thread-affine;
//! callbacks execute on their owner, never on a native vCPU worker. Exact mode
//! uses host time only to cancel execution. Interactive progress is host
//! nanoseconds, and guest CNTVCT/CNTV use the host's native virtual counter and
//! timer.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

mod ffi;
mod machine;
mod worker;

use fujimod_emulator_cpu::*;
use machine::Machine;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, atomic::Ordering, mpsc};
use std::thread::JoinHandle;
use std::{any::Any, ptr::NonNull};
use worker::{Command, Event, Op, Value};

/// One process-wide HVF VM; create every processor through this value.
/// Map/host-memory mutations require all its CPUs to be stopped. Backing
/// mappings persist until explicitly unmapped or the last VM/CPU is dropped.
#[derive(Clone)]
pub struct Vm {
    machine: Arc<Machine>,
    _owner: PhantomData<Rc<()>>,
}
impl Vm {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            machine: Machine::create()?,
            _owner: PhantomData,
        })
    }
    pub fn cpu(&self, model: Model) -> Result<Core, String> {
        if model != Model::ArmV8A {
            return Err("HVF executes AArch64 ArmV8A processors only".into());
        }
        Core::create(self.machine.clone())
    }
}
#[derive(Clone, Copy)]
struct Window {
    end: u64,
    offset: u64,
    callbacks: Mmio,
    permissions: u32,
}
pub struct Core {
    worker: Option<JoinHandle<()>>,
    budget_timer: Option<worker::BudgetTimer>,
    commands: mpsc::Sender<Command>,
    machine: Arc<Machine>,
    shared: Arc<worker::Shared>,
    identity: Identity,
    native_cpu: u64,
    hooks: RefCell<BTreeMap<usize, Hook>>,
    next_hook: Cell<usize>,
    next_run: Cell<u64>,
    active_run: RefCell<Option<ActiveRun>>,
    mmio: RefCell<BTreeMap<u64, Window>>,
    running: Cell<bool>,
    instruction_hook: Cell<bool>,
    execution_policy: Cell<ExecutionPolicy>,
    _owner: PhantomData<Rc<()>>,
}
struct State {
    owner: Identity,
    registers: worker::Registers,
}
struct ActiveRun {
    token: RunToken,
    events: mpsc::Receiver<Event>,
    reply: mpsc::Receiver<Result<Value, String>>,
    pending_callback: Option<Event>,
    callback_error: Option<ExecutionError>,
    result: Option<Result<Exit, ExecutionError>>,
    watchdog: Option<Watchdog>,
}
impl Snapshot for State {
    fn export(&self) -> Result<Vec<u8>, String> {
        self.registers.export()
    }
    fn owner(&self) -> Identity {
        self.owner
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl Core {
    fn create(machine: Arc<Machine>) -> Result<Self, String> {
        let (commands, rx) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel();
        let shared = Arc::new(worker::Shared::default());
        let vm = machine.clone();
        let state = shared.clone();
        let worker = std::thread::Builder::new()
            .name("camera-hvf-vcpu".into())
            .spawn(move || worker::serve(vm, state, rx, ready_tx))
            .map_err(|e| e.to_string())?;
        match ready
            .recv()
            .map_err(|_| "HVF worker disconnected at creation")?
        {
            Ok(native_cpu) => {
                let budget_timer = match worker::BudgetTimer::new(native_cpu, shared.clone()) {
                    Ok(timer) => timer,
                    Err(error) => {
                        drop(commands);
                        let _ = worker.join();
                        return Err(error);
                    }
                };
                Ok(Self {
                    worker: Some(worker),
                    budget_timer: Some(budget_timer),
                    commands,
                    machine,
                    shared,
                    identity: Identity::allocate(),
                    native_cpu,
                    hooks: RefCell::new(BTreeMap::new()),
                    next_hook: Cell::new(0),
                    next_run: Cell::new(1),
                    active_run: RefCell::new(None),
                    mmio: RefCell::new(BTreeMap::new()),
                    running: Cell::new(false),
                    instruction_hook: Cell::new(false),
                    execution_policy: Cell::new(ExecutionPolicy::Exact),
                    _owner: PhantomData,
                })
            }
            Err(e) => {
                let _ = worker.join();
                Err(e)
            }
        }
    }
    fn rpc(&self, op: Op) -> Result<Value, String> {
        let (reply, rx) = mpsc::channel();
        self.commands
            .send(Command { op, reply })
            .map_err(|_| "HVF worker disconnected")?;
        rx.recv().map_err(|_| "HVF worker reply disconnected")?
    }
    fn validate_active_run(
        &self,
        run: RunToken,
    ) -> Result<std::cell::RefMut<'_, Option<ActiveRun>>, ExecutionError> {
        let active = self.active_run.borrow_mut();
        if run.owner() != self.identity
            || !active.as_ref().is_some_and(|active| active.token == run)
        {
            return Err(ExecutionError::Backend(
                "run token belongs to a different or inactive HVF run".into(),
            ));
        }
        Ok(active)
    }
    fn preempt_native_run(&self) {
        self.shared.stop.store(true, Ordering::Release);
        if self.shared.native_active.load(Ordering::Acquire) {
            unsafe {
                let _ = ffi::hv_vcpus_exit(&self.native_cpu, 1);
            }
        }
    }
    fn scalar(&self, r: Register) -> Result<u64, String> {
        let mut bytes = [0; 8];
        self.read_register(r, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }
    fn set_scalar(&self, r: Register, value: u64) -> Result<(), String> {
        self.write_register(r, &value.to_le_bytes())
    }
    fn selected(range: HookRange, address: u64) -> bool {
        match range {
            HookRange::All => true,
            HookRange::Inclusive { begin, end } => address >= begin && address <= end,
        }
    }
    fn has_instruction_hooks(&self) -> bool {
        self.hooks
            .borrow()
            .values()
            .any(|hook| matches!(hook, Hook::Instruction { .. }))
    }
    fn before(&self, pc: u64) {
        let hooks: Vec<_> = self
            .hooks
            .borrow()
            .iter()
            .filter_map(|(&key, hook)| match *hook {
                Hook::Instruction {
                    callback,
                    data,
                    range,
                } if Self::selected(range, pc) => Some((key, callback, data)),
                _ => None,
            })
            .collect();
        for (key, callback, data) in hooks {
            if self.shared.stop.load(Ordering::Acquire) {
                break;
            }
            if self.hooks.borrow().contains_key(&key) {
                self.instruction_hook.set(true);
                unsafe {
                    callback(pc, 4, data);
                }
                self.instruction_hook.set(false);
            }
            if self.scalar(Register::Pc).ok() != Some(pc) {
                break;
            }
        }
    }
    fn window(&self, address: u64, size: usize, access: Access) -> Result<(u64, Window), String> {
        let end = address
            .checked_add(size as u64)
            .ok_or("MMIO range overflow")?;
        let windows = self.mmio.borrow();
        let (&base, &w) = windows
            .range(..=address)
            .next_back()
            .filter(|(_, w)| end <= w.end)
            .ok_or_else(|| format!("unmapped {access:?} at {address:#x}+{size:#x}"))?;
        let required = match access {
            Access::Read => 1,
            Access::Write => 2,
            Access::Fetch => 4,
        };
        if w.permissions & required == 0 {
            return Err(format!("protected MMIO {access:?} at {address:#x}"));
        }
        Ok((base, w))
    }
    /// Only mapped MMIO data aborts are completed here. Unknown architectural
    /// traps return to the framework with the faulting PC unchanged.
    fn mmio_trap(&self, trap: Trap, complete_after_pause: bool) -> Result<bool, ExecutionError> {
        if !matches!(trap.syndrome >> 26, 0x24 | 0x25) {
            return Ok(false);
        }
        let pc = self.scalar(Register::Pc).map_err(ExecutionError::Backend)?;
        let mut word = [0; 4];
        self.read_virtual(pc, Access::Fetch, &mut word)
            .map_err(ExecutionError::Backend)?;
        let decoded = arm64::decode_load_store(u32::from_le_bytes(word), pc, |r| {
            self.scalar(if r == 31 {
                Register::Sp
            } else {
                Register::X(r)
            })
        })
        .map_err(ExecutionError::InvalidInstruction)?;
        // SIMD/pair transfers spanning RAM are handled by the runtime's
        // protected-memory path. Device callbacks carry scalar widths only.
        let access = if decoded.load {
            Access::Read
        } else {
            Access::Write
        };
        let mut transfers = Vec::new();
        for t in &decoded.transfers {
            let pa = self
                .translate(t.address, access)
                .map_err(ExecutionError::Backend)?;
            if self
                .machine
                .overlaps(pa, pa.saturating_add(t.size as u64))
                .map_err(ExecutionError::Backend)?
            {
                return Ok(false);
            }
            if decoded.simd || t.size > 8 {
                return Err(ExecutionError::InvalidInstruction(
                    "SIMD MMIO is unsupported".into(),
                ));
            }
            let mapping = self
                .window(pa, t.size as usize, access)
                .map_err(|message| ExecutionError::Memory {
                    fault: match (decoded.load, message.starts_with("protected MMIO")) {
                        (true, true) => Fault::ReadProtected,
                        (false, true) => Fault::WriteProtected,
                        (true, false) => Fault::ReadUnmapped,
                        (false, false) => Fault::WriteUnmapped,
                    },
                    message,
                })?;
            transfers.push((t, pa, mapping));
        }
        // Validate the whole operation before invoking any device callback.
        if transfers
            .first()
            .is_none_or(|(_, pa, _)| *pa != trap.physical_address)
        {
            return Err(ExecutionError::Backend(
                "HVF data-abort address disagrees with decoded instruction".into(),
            ));
        }
        let mut updates = Vec::new();
        // An async pause can arrive while a device callback is already in
        // flight. Finish that guest access so the next run cannot repeat its
        // side effect or lose a loaded value.
        for (t, pa, (base, w)) in transfers {
            let offset = w.offset + pa - base;
            if decoded.load {
                let raw =
                    unsafe { (w.callbacks.read)(offset, u32::from(t.size), w.callbacks.data) };
                if !complete_after_pause && self.shared.stop.load(Ordering::Acquire) {
                    return Ok(true);
                }
                let mask = u64::MAX >> (64 - u32::from(t.size) * 8);
                let mut value = raw & mask;
                if t.signed {
                    let shift = 64 - t.size as u32 * 8;
                    value = ((value << shift) as i64 >> shift) as u64;
                }
                if !t.wide {
                    value &= u32::MAX as u64;
                }
                if t.register != 31 {
                    updates.push((Register::X(t.register), value));
                }
            } else {
                let value = if t.register == 31 {
                    0
                } else {
                    self.scalar(Register::X(t.register))
                        .map_err(ExecutionError::Backend)?
                };
                let mask = u64::MAX >> (64 - u32::from(t.size) * 8);
                unsafe {
                    (w.callbacks.write)(offset, u32::from(t.size), value & mask, w.callbacks.data);
                }
                if !complete_after_pause && self.shared.stop.load(Ordering::Acquire) {
                    return Ok(true);
                }
            }
        }
        for (register, value) in updates {
            self.set_scalar(register, value)
                .map_err(ExecutionError::Backend)?;
        }
        if let Some((r, v)) = decoded.writeback {
            self.set_scalar(
                if r == 31 {
                    Register::Sp
                } else {
                    Register::X(r)
                },
                v,
            )
            .map_err(ExecutionError::Backend)?;
        }
        self.set_scalar(Register::Pc, pc.wrapping_add(4))
            .map_err(ExecutionError::Backend)?;
        Ok(true)
    }
    fn publish_device_ranges(&self) {
        self.machine.devices(
            self.identity,
            self.mmio
                .borrow()
                .iter()
                .map(|(&base, w)| (base, w.end))
                .collect(),
        );
    }
    fn device_range(address: u64, bytes: usize) -> Result<u64, String> {
        if bytes == 0 {
            return Err("empty MMIO range".into());
        }
        address
            .checked_add(bytes as u64)
            .ok_or_else(|| "MMIO range overflow".into())
    }
    fn mmio_change(
        &self,
        address: u64,
        bytes: usize,
        permissions: Option<u32>,
    ) -> Result<bool, String> {
        let end = Self::device_range(address, bytes)?;
        if let Some(p) = permissions {
            Machine::permissions(p)?;
        }
        let mut windows = self.mmio.borrow_mut();
        let keys: Vec<_> = windows
            .range(..end)
            .filter(|(_, w)| w.end > address)
            .map(|(&b, _)| b)
            .collect();
        if keys.is_empty() {
            return Ok(false);
        }
        // A change must be entirely backed by MMIO, without RAM or holes.
        let mut at = address;
        for &base in &keys {
            if base > at {
                return Err("MMIO map change crosses a hole".into());
            }
            at = windows[&base].end.min(end);
        }
        if at != end {
            return Err("MMIO map change crosses a hole".into());
        }
        for base in keys {
            let w = windows.remove(&base).unwrap();
            if base < address {
                windows.insert(base, Window { end: address, ..w });
            }
            if w.end > end {
                windows.insert(
                    end,
                    Window {
                        offset: w.offset + end - base,
                        ..w
                    },
                );
            }
            if let Some(p) = permissions {
                let begin = address.max(base);
                windows.insert(
                    begin,
                    Window {
                        end: w.end.min(end),
                        offset: w.offset + begin - base,
                        permissions: p,
                        ..w
                    },
                );
            }
        }
        drop(windows);
        self.publish_device_ranges();
        Ok(true)
    }
}
impl Cpu for Core {
    fn set_execution_policy(&self, policy: ExecutionPolicy) -> Result<(), String> {
        if self.running.get() {
            return Err("execution policy requires a stopped processor".into());
        }
        if policy == ExecutionPolicy::Interactive && self.has_instruction_hooks() {
            return Err("interactive execution cannot observe instructions".into());
        }
        self.execution_policy.set(policy);
        Ok(())
    }
    fn exclusive_domain(&self) -> Option<fujimod_emulator_cpu::exclusive::Shared> {
        Some(self.machine.monitors.clone())
    }
    fn identity(&self) -> Identity {
        self.identity
    }
    fn model(&self) -> Model {
        Model::ArmV8A
    }
    fn features(&self) -> Features {
        Features {
            asynchronous_runs: true,
            instruction_limit: true,
            shared_physical_maps: true,
            instruction_progress: true,
            architectural_exits: true,
            tighten_instruction_limit: true,
            backing_observation: true,
            ..Features::default()
        }
    }
    fn read_register(&self, register: Register, out: &mut [u8]) -> Result<usize, String> {
        match self.rpc(Op::Read(register))? {
            Value::Bytes(bytes) if out.len() >= bytes.len() => {
                out[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            Value::Bytes(bytes) => Err(format!("register buffer needs {} bytes", bytes.len())),
            _ => unreachable!(),
        }
    }
    fn write_register(&self, register: Register, bytes: &[u8]) -> Result<(), String> {
        self.rpc(Op::Write(register, bytes.to_vec()))?;
        Ok(())
    }
    fn coprocessor(&self, _: arm::Coprocessor, _: Option<u64>) -> Result<u64, String> {
        Err("AArch32 coprocessors are unsupported by HVF".into())
    }
    fn read_memory(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.machine.read(address, bytes)
    }
    fn translate(&self, address: u64, access: Access) -> Result<u64, String> {
        match self.rpc(Op::Translate(address, access))? {
            Value::Address(a) => Ok(a),
            _ => unreachable!(),
        }
    }
    fn read_virtual(&self, address: u64, access: Access, bytes: &mut [u8]) -> Result<(), String> {
        let mut pieces = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let va = address
                .checked_add(offset as u64)
                .ok_or("virtual read overflow")?;
            let n = (0x1000 - (va as usize & 0xfff)).min(bytes.len() - offset);
            let pa = self.translate(va, access)?;
            let mut b = vec![0; n];
            self.machine.read(pa, &mut b)?;
            let permission = match access {
                Access::Read => 1,
                Access::Write => 2,
                Access::Fetch => 4,
            };
            self.machine.check_access(pa, n, permission)?;
            pieces.push(b);
            offset += n;
        }
        let mut offset = 0;
        for b in pieces {
            bytes[offset..offset + b.len()].copy_from_slice(&b);
            offset += b.len();
        }
        Ok(())
    }
    fn write_memory(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        self.machine.write(address, bytes)
    }
    fn memory_written(&self, address: u64, bytes: usize) -> Result<(), String> {
        self.machine.written(Some((address, bytes)))
    }
    unsafe fn backing_written(
        &self,
        physical: u64,
        backing: NonNull<u8>,
        bytes: usize,
    ) -> Result<(), String> {
        self.machine.written(Some((physical, bytes)))?;
        unsafe { ffi::sys_icache_invalidate(backing.as_ptr().cast(), bytes) };
        Ok(())
    }
    fn memory_regions(&self) -> Result<Vec<MemoryRegion>, String> {
        let mut out = self.machine.regions()?;
        out.extend(self.mmio.borrow().iter().map(|(&begin, w)| MemoryRegion {
            begin,
            end: w.end - 1,
            permissions: w.permissions,
        }));
        out.sort_by_key(|m| m.begin);
        Ok(out)
    }
    fn checkpoint_backings(&self) -> Result<Vec<(NonNull<u8>, usize)>, String> {
        self.machine.checkpoint_backings()
    }
    unsafe fn map_backing(
        &self,
        address: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        let end = Machine::range(address, bytes)?;
        if self
            .mmio
            .borrow()
            .range(..end)
            .any(|(_, w)| w.end > address)
        {
            return Err("RAM mapping overlaps MMIO".into());
        }
        self.machine
            .map(address, bytes, permissions, backing.as_ptr() as usize)
    }
    fn map_zeroed(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        let end = Machine::range(address, bytes)?;
        if self
            .mmio
            .borrow()
            .range(..end)
            .any(|(_, w)| w.end > address)
        {
            return Err("RAM mapping overlaps MMIO".into());
        }
        self.machine.zeroed(address, bytes, permissions)
    }
    unsafe fn map_mmio(&self, address: u64, bytes: usize, callbacks: Mmio) -> Result<(), String> {
        let end = Self::device_range(address, bytes)?;
        if self.machine.overlaps(address, end)?
            || self
                .mmio
                .borrow()
                .range(..end)
                .any(|(_, w)| w.end > address)
        {
            return Err("MMIO mapping overlaps an existing range".into());
        }
        self.mmio.borrow_mut().insert(
            address,
            Window {
                end,
                offset: 0,
                callbacks,
                permissions: 3,
            },
        );
        self.publish_device_ranges();
        Ok(())
    }
    fn unmap(&self, address: u64, bytes: usize) -> Result<(), String> {
        if self.mmio_change(address, bytes, None)? {
            Ok(())
        } else {
            self.machine.change(address, bytes, None)
        }
    }
    fn protect(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        if self.mmio_change(address, bytes, Some(permissions))? {
            Ok(())
        } else {
            self.machine.change(address, bytes, Some(permissions))
        }
    }
    fn invalidate_code(&self, range: Option<(u64, u64)>) -> Result<(), String> {
        self.machine.invalidate(range)
    }
    fn run(&self, request: Run) -> Result<Exit, ExecutionError> {
        if self.running.replace(true) {
            return Err(ExecutionError::Backend(
                "cannot run a CPU recursively".into(),
            ));
        }
        struct Guard<'a>(&'a Cell<bool>);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _guard = Guard(&self.running);
        let policy = self.execution_policy.get();
        if policy == ExecutionPolicy::Interactive && self.has_instruction_hooks() {
            return Err(ExecutionError::Backend(
                "interactive execution cannot observe instructions".into(),
            ));
        }
        self.shared.stop.store(false, Ordering::Release);
        self.shared.deadline.store(false, Ordering::Release);
        let _watchdog = Watchdog::new(self.native_cpu, request.watchdog_us, self.shared.clone());
        let (events, event_rx) = mpsc::channel();
        let (reply, rx) = mpsc::channel();
        let observe = self.has_instruction_hooks();
        self.commands
            .send(Command {
                op: Op::Run(request, policy, observe, events),
                reply,
            })
            .map_err(|_| ExecutionError::Backend("HVF worker disconnected".into()))?;
        let mut error = None;
        while let Ok(event) = event_rx.recv() {
            match event {
                Event::Before(pc) => {
                    self.before(pc);
                    self.rpc(Op::Resume(true))
                        .map_err(ExecutionError::Backend)?;
                }
                Event::Trap(trap) => {
                    let handled = match self.mmio_trap(trap, false) {
                        Ok(handled) => handled,
                        Err(e) => {
                            error = Some(e);
                            false
                        }
                    };
                    self.rpc(Op::Resume(handled))
                        .map_err(ExecutionError::Backend)?;
                }
                Event::Done => break,
            }
        }
        if let Some(e) = error {
            return Err(e);
        }
        match rx
            .recv()
            .map_err(|_| ExecutionError::Backend("HVF run reply disconnected".into()))?
            .map_err(ExecutionError::Backend)?
        {
            Value::Exit(exit) => exit,
            _ => unreachable!(),
        }
    }
    fn start_run(&self, request: Run) -> Result<RunToken, ExecutionError> {
        if self.execution_policy.get() != ExecutionPolicy::Interactive {
            return Err(ExecutionError::Backend(
                "asynchronous HVF runs require interactive execution policy".into(),
            ));
        }
        if self.running.replace(true) {
            return Err(ExecutionError::Backend(
                "cannot run a CPU recursively".into(),
            ));
        }
        if self.has_instruction_hooks() {
            self.running.set(false);
            return Err(ExecutionError::Backend(
                "interactive execution cannot observe instructions".into(),
            ));
        }

        let sequence = self.next_run.get();
        self.next_run.set(sequence.wrapping_add(1).max(1));
        let token = RunToken::new(self.identity, sequence);
        self.shared.stop.store(false, Ordering::Release);
        self.shared.deadline.store(false, Ordering::Release);
        self.shared.progress.store(0, Ordering::Relaxed);
        self.shared.measured.store(false, Ordering::Relaxed);
        let watchdog = Watchdog::new(self.native_cpu, request.watchdog_us, self.shared.clone());
        let (events, event_rx) = mpsc::channel();
        let (reply, reply_rx) = mpsc::channel();
        if self
            .commands
            .send(Command {
                op: Op::Run(request, ExecutionPolicy::Interactive, false, events),
                reply,
            })
            .is_err()
        {
            self.running.set(false);
            drop(watchdog);
            return Err(ExecutionError::Backend("HVF worker disconnected".into()));
        }
        *self.active_run.borrow_mut() = Some(ActiveRun {
            token,
            events: event_rx,
            reply: reply_rx,
            pending_callback: None,
            callback_error: None,
            result: None,
            watchdog: Some(watchdog),
        });
        Ok(token)
    }

    fn poll_run(&self, run: RunToken) -> Result<RunStatus, ExecutionError> {
        let mut active = self.validate_active_run(run)?;
        let state = active.as_mut().unwrap();
        if state.pending_callback.is_some() {
            return Ok(RunStatus::CallbackPending);
        }
        if let Some(result) = &state.result {
            return Ok(RunStatus::Complete(result.clone()));
        }
        match state.events.try_recv() {
            Ok(event @ (Event::Before(_) | Event::Trap(_))) => {
                state.pending_callback = Some(event);
                Ok(RunStatus::CallbackPending)
            }
            Ok(Event::Done) => {
                let result = match state.reply.try_recv() {
                    Ok(Ok(Value::Exit(result))) => result,
                    Ok(Ok(_)) => Err(ExecutionError::Backend(
                        "HVF run returned an invalid completion value".into(),
                    )),
                    Ok(Err(error)) => Err(ExecutionError::Backend(error)),
                    Err(mpsc::TryRecvError::Empty) => Err(ExecutionError::Backend(
                        "HVF completion arrived before its run result".into(),
                    )),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Err(ExecutionError::Backend("HVF run reply disconnected".into()))
                    }
                };
                let result = state.callback_error.take().map_or(result, Err);
                state.result = Some(result.clone());
                state.watchdog.take();
                self.running.set(false);
                Ok(RunStatus::Complete(result))
            }
            Err(mpsc::TryRecvError::Empty) => Ok(RunStatus::Running),
            Err(mpsc::TryRecvError::Disconnected) => {
                let error = ExecutionError::Backend("HVF worker disconnected during run".into());
                state.result = Some(Err(error.clone()));
                state.watchdog.take();
                self.running.set(false);
                Ok(RunStatus::Complete(Err(error)))
            }
        }
    }

    fn request_pause(&self, run: RunToken) -> Result<(), ExecutionError> {
        let active = self.validate_active_run(run)?;
        if active.as_ref().unwrap().result.is_none() {
            self.preempt_native_run();
        }
        Ok(())
    }

    fn service_pending_callback(&self, run: RunToken) -> Result<(), ExecutionError> {
        let event = self
            .validate_active_run(run)?
            .as_mut()
            .unwrap()
            .pending_callback
            .take()
            .ok_or_else(|| {
                ExecutionError::Backend("HVF run has no parked callback to service".into())
            })?;
        let handled = match event {
            Event::Before(pc) => {
                self.before(pc);
                true
            }
            Event::Trap(trap) => match self.mmio_trap(trap, true) {
                Ok(handled) => handled,
                Err(error) => {
                    self.validate_active_run(run)?
                        .as_mut()
                        .unwrap()
                        .callback_error = Some(error);
                    false
                }
            },
            Event::Done => unreachable!("completion events cannot become callbacks"),
        };
        self.rpc(Op::Resume(handled))
            .map_err(ExecutionError::Backend)?;
        Ok(())
    }

    fn cancel_pending_callback(&self, run: RunToken) -> Result<(), ExecutionError> {
        let mut active = self.validate_active_run(run)?;
        let state = active.as_mut().unwrap();
        if state.pending_callback.take().is_none() {
            return Ok(());
        }
        self.preempt_native_run();
        drop(active);
        self.rpc(Op::Resume(false))
            .map_err(ExecutionError::Backend)?;
        Ok(())
    }

    fn set_run_wakeup(&self, wakeup: RunWakeup) -> Result<(), String> {
        if self.running.get() {
            return Err("cannot replace HVF run wakeup during an active run".into());
        }
        self.shared.set_run_wakeup(wakeup);
        Ok(())
    }
    fn tighten_instruction_limit(&self, total_entries: u64) -> Result<(), String> {
        if !self.running.get() {
            return Err("no active processor run".into());
        }
        if total_entries < self.shared.progress.load(Ordering::Acquire) {
            return Err("instruction limit precedes an entered instruction".into());
        }
        let changed = {
            let _budget = self
                .shared
                .budget_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = self.shared.limit.fetch_min(total_entries, Ordering::AcqRel);
            let changed = total_entries < previous;
            if changed {
                self.shared.budget_generation.fetch_add(1, Ordering::AcqRel);
            }
            changed
        };
        if changed {
            self.shared.notify_budget_change()?;
        }
        Ok(())
    }
    fn instruction_progress(&self) -> Option<u64> {
        self.shared
            .measured
            .load(Ordering::Relaxed)
            .then(|| self.shared.progress.load(Ordering::Relaxed))
    }
    fn stop(&self) -> Result<(), String> {
        self.preempt_native_run();
        Ok(())
    }
    fn stop_before_instruction(&self) -> Result<(), String> {
        if !self.instruction_hook.get() {
            return Err("instruction interception requires an instruction observer".into());
        }
        // The worker is paused before entry and checks stop before accounting
        // or executing the observed instruction.
        self.stop()
    }
    fn set_interrupt(&self, line: Interrupt, asserted: bool) -> Result<(), String> {
        self.rpc(Op::Interrupt(line, asserted))?;
        Ok(())
    }
    fn snapshot(&self, _previous: Option<&dyn Snapshot>) -> Result<Box<dyn Snapshot>, String> {
        match self.rpc(Op::Snapshot)? {
            Value::Registers(registers) => Ok(Box::new(State {
                owner: self.identity,
                registers,
            })),
            _ => unreachable!(),
        }
    }
    fn restore(&self, snapshot: &dyn Snapshot) -> Result<(), String> {
        let state = snapshot
            .as_any()
            .downcast_ref::<State>()
            .filter(|s| s.owner == self.identity)
            .ok_or("snapshot belongs to a different CPU")?;
        self.rpc(Op::Restore(state.registers.clone()))?;
        Ok(())
    }
    fn import_snapshot(&self, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        Ok(Box::new(State {
            owner: self.identity,
            registers: worker::Registers::import(bytes)?,
        }))
    }
    unsafe fn install_hook(&self, hook: Hook) -> Result<HookId, String> {
        // HVF has no watched-store hook. Accept synchronization hooks as markers
        // only: vCPUs share coherent RAM and execute exclusives natively.
        if !matches!(
            hook,
            Hook::Instruction { .. } | Hook::Synchronization { .. }
        ) {
            return Err("HVF supports instruction and synchronization hooks only".into());
        }
        if self.execution_policy.get() == ExecutionPolicy::Interactive
            && matches!(hook, Hook::Instruction { .. })
        {
            return Err("interactive execution cannot observe instructions".into());
        }
        let key = self.next_hook.get();
        self.next_hook
            .set(key.checked_add(1).ok_or("hook ID exhausted")?);
        self.hooks.borrow_mut().insert(key, hook);
        Ok(HookId {
            owner: self.identity,
            key,
        })
    }
    fn remove_hook(&self, hook: HookId) -> Result<(), String> {
        if hook.owner != self.identity {
            return Err("hook belongs to a different CPU".into());
        }
        self.hooks
            .borrow_mut()
            .remove(&hook.key)
            .ok_or("unknown hook")?;
        Ok(())
    }
    unsafe fn translation_hooks(&self, _: Option<TranslationHooks>) -> Result<(), String> {
        Err("HVF has no translation filter".into())
    }
    fn retry_block(&self, _: u32) -> Result<(), String> {
        Err("HVF has no translated block retry".into())
    }
    fn request_restart(&self) -> Result<(), String> {
        self.stop()
    }
    fn accounting(&self) -> Option<NonNull<BlockAccounting>> {
        None
    }
    unsafe fn observe_backings(&self, ranges: &[BackingObserver]) -> Result<(), String> {
        // Hardware RAM writes have no software dirty callback. Invalidate all
        // pages at each stopped rearm; equal immutable pages still share storage.
        // No observer pointer is retained by the VM or its worker threads.
        for range in ranges {
            let page = 1usize
                .checked_shl(range.page_shift)
                .ok_or("HVF observer page shift exceeds target")?;
            let words = range.bytes.div_ceil(page).div_ceil(64);
            for index in 0..words {
                unsafe {
                    (*range.bitmap.as_ptr().add(index)).fetch_or(u64::MAX, Ordering::Relaxed);
                }
            }
        }
        Ok(())
    }
    fn watch_stores(&self, _: u64, _: u64) -> Result<(), String> {
        Err("HVF watched stores are unsupported".into())
    }
    fn hook_watched(&self, _: HookId, _: bool) -> Result<(), String> {
        Err("HVF watched hooks are unsupported".into())
    }
    fn precise_store_pc(&self, _: bool) -> Result<(), String> {
        Err("HVF store observation is unsupported".into())
    }
    fn exit_counters(&self) -> Result<BTreeMap<&'static str, u64>, String> {
        Ok(BTreeMap::new())
    }
}
/// The SDK permits cancellation from another thread. Join the watchdog before
/// ending this run, so it cannot cancel a later run or a destroyed processor.
struct Watchdog {
    done: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    thread: Option<JoinHandle<()>>,
}
impl Watchdog {
    fn new(cpu: u64, micros: u64, shared: Arc<worker::Shared>) -> Self {
        let done = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let thread = if micros == 0 {
            None
        } else {
            let wait = done.clone();
            Some(std::thread::spawn(move || {
                let (lock, cv) = &*wait;
                let (finished, _) = cv
                    .wait_timeout_while(
                        lock.lock().unwrap(),
                        std::time::Duration::from_micros(micros),
                        |done| !*done,
                    )
                    .unwrap();
                if !*finished {
                    shared.stop.store(true, Ordering::Release);
                    unsafe {
                        let _ = ffi::hv_vcpus_exit(&cpu, 1);
                    }
                }
            }))
        };
        Self { done, thread }
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        let (lock, cv) = &*self.done;
        *lock.lock().unwrap() = true;
        cv.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        if let Some(mut active) = self.active_run.get_mut().take() {
            if active.result.is_none() {
                self.preempt_native_run();
                if active.pending_callback.take().is_some() {
                    let _ = self.rpc(Op::Resume(false));
                }
                loop {
                    match active.events.recv() {
                        Ok(Event::Before(_) | Event::Trap(_)) => {
                            let _ = self.rpc(Op::Resume(false));
                        }
                        Ok(Event::Done) | Err(_) => break,
                    }
                }
                let _ = active.reply.recv();
            }
            active.watchdog.take();
        }
        self.budget_timer.take();
        let _ = self.rpc(Op::Shutdown);
        if let Some(thread) = self.worker.take() {
            let _ = thread.join();
        }
        self.machine.devices(self.identity, Vec::new());
    }
}
