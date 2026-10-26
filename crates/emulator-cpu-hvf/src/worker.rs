//! A vCPU never leaves its owning thread. Callbacks execute on the Cpu owner;
//! while paused for one, this worker serves register/snapshot/stop RPCs.
use super::{ffi, machine::Machine};
use fujimod_emulator_cpu::{arm64::TranslationState, *};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
mod portable;

pub enum Op {
    Read(Register),
    Write(Register, Vec<u8>),
    Translate(u64, Access),
    Snapshot,
    Restore(Registers),
    Interrupt(Interrupt, bool),
    Run(Run, bool, mpsc::Sender<Event>),
    Resume(bool),
    Shutdown,
}
pub enum Value {
    Empty,
    Bytes(Vec<u8>),
    Address(u64),
    Registers(Registers),
    Exit(Result<Exit, ExecutionError>),
}
pub struct Command {
    pub op: Op,
    pub reply: mpsc::Sender<Result<Value, String>>,
}
pub enum Event {
    Before(u64),
    Trap(Trap),
    Done,
}
#[derive(Clone)]
pub struct Registers {
    core: [u64; 35],
    vectors: [[u8; 16]; 32],
    system: Vec<(u16, u64)>,
    interrupts: [bool; 2],
    exclusive: fujimod_emulator_cpu::exclusive::Saved,
}
// Architecturally writable EL1 state. Excludes device/GIC acknowledgments,
// read-only feature registers and the backend's debug instrumentation.
const SYSTEM: &[u16] = &[
    0xc080, 0xc082, 0xc100, 0xc101, 0xc102, 0xc200, 0xc201, 0xc208, 0xc288, 0xc289, 0xc290, 0xc300,
    0xc3a0, 0xc510, 0xc518, 0xc600, 0xc681, 0xc684, 0xc708, 0xd000, 0xde82, 0xde83, 0xe208,
];
pub struct Shared {
    pub stop: AtomicBool,
    pub progress: AtomicU64,
    pub limit: AtomicU64,
    pub measured: AtomicBool,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            stop: AtomicBool::new(false),
            progress: AtomicU64::new(0),
            limit: AtomicU64::new(u64::MAX),
            measured: AtomicBool::new(false),
        }
    }
}
struct Processor {
    cpu: u64,
    monitor: fujimod_emulator_cpu::exclusive::Owner,
    exit: *const ffi::Exit,
    vm: Arc<Machine>,
    shared: Arc<Shared>,
    interrupts: [bool; 2],
}
impl Processor {
    fn create(vm: Arc<Machine>, shared: Arc<Shared>) -> Result<Self, String> {
        let mut cpu = 0;
        let mut exit = std::ptr::null();
        unsafe {
            ffi::check(
                ffi::hv_vcpu_create(&mut cpu, &mut exit, std::ptr::null()),
                "create vCPU",
            )?;
        }
        let monitor = vm
            .monitors
            .lock()
            .map_err(|_| "exclusive monitors poisoned".to_string())
            .and_then(|mut monitors| monitors.register(64));
        let monitor = match monitor {
            Ok(monitor) => monitor,
            Err(error) => {
                unsafe {
                    let _ = ffi::hv_vcpu_destroy(cpu);
                }
                return Err(error);
            }
        };
        let p = Self {
            cpu,
            monitor,
            exit,
            vm,
            shared,
            interrupts: [false; 2],
        };
        unsafe {
            ffi::check(
                ffi::hv_vcpu_set_trap_debug_exceptions(cpu, true),
                "trap debug exceptions",
            )?;
            ffi::check(
                ffi::hv_vcpu_set_trap_debug_reg_accesses(cpu, true),
                "trap debug registers",
            )?;
            ffi::check(
                ffi::hv_vcpu_set_vtimer_mask(cpu, true),
                "mask host virtual timer",
            )?;
        }
        p.set_sys(ffi::MDSCR, 1)?;
        p.set_sys(ffi::SCTLR, 0x30d00800)?;
        p.set_reg(ffi::PSTATE, 0x3c5)?;
        Ok(p)
    }
    fn reg(&self, r: u32) -> Result<u64, String> {
        let mut v = 0;
        unsafe {
            ffi::check(ffi::hv_vcpu_get_reg(self.cpu, r, &mut v), "read register")?;
        }
        Ok(v)
    }
    fn set_reg(&self, r: u32, v: u64) -> Result<(), String> {
        unsafe { ffi::check(ffi::hv_vcpu_set_reg(self.cpu, r, v), "write register") }
    }
    fn sys(&self, r: u16) -> Result<u64, String> {
        let mut v = 0;
        unsafe {
            ffi::check(
                ffi::hv_vcpu_get_sys_reg(self.cpu, r, &mut v),
                "read system register",
            )?;
        }
        Ok(v)
    }
    fn set_sys(&self, r: u16, v: u64) -> Result<(), String> {
        unsafe {
            ffi::check(
                ffi::hv_vcpu_set_sys_reg(self.cpu, r, v),
                "write system register",
            )
        }
    }
    fn sp(&self) -> Result<u16, String> {
        Ok(if self.reg(ffi::PSTATE)? & 1 == 0 {
            ffi::SP0
        } else {
            ffi::SP1
        })
    }
    fn id(r: Register) -> Option<u32> {
        match r {
            Register::X(n @ 0..=30) => Some(n as u32),
            Register::Pc => Some(ffi::PC),
            Register::Pstate => Some(ffi::PSTATE),
            Register::Fpcr => Some(ffi::FPCR),
            Register::Fpsr => Some(ffi::FPSR),
            _ => None,
        }
    }
    fn framework_register(id: u16) -> bool {
        let r = arm::SystemRegister::from_encoding(id);
        r.crn == 15
            || matches!(id, 0xce82 | 0xce83)
            || r.crn == 14
            || r.crn == 9
            || (r.crn == 13 && r.op1 == 3 && r.crm >= 2)
            || (r.crn == 2 && r.op1 == 3 && r.crm == 4 && r.op2 <= 1)
            || id == 0xc230
            || (r.op0 == 3 && r.op1 == 0 && r.crn == 12 && (8..=15).contains(&r.crm))
    }
    fn read(&self, r: Register) -> Result<Vec<u8>, String> {
        if let Some(id) = Self::id(r) {
            let value = self.reg(id)?
                & if id == ffi::PSTATE {
                    !(1 << 21)
                } else {
                    u64::MAX
                };
            return Ok(value.to_le_bytes().to_vec());
        }
        match r {
            Register::Sp => Ok(self.sys(self.sp()?)?.to_le_bytes().to_vec()),
            Register::Arm64System(r) => {
                let id = r.encoding()?;
                if id == ffi::MDSCR {
                    return Ok(0u64.to_le_bytes().to_vec());
                }
                if Self::framework_register(id) {
                    return Err("architectural device registers belong to the framework".into());
                }
                Ok(self.sys(id)?.to_le_bytes().to_vec())
            }
            Register::V(n @ 0..=31) => {
                let mut value = [0; 16];
                unsafe {
                    ffi::check(
                        ffi::camera_hvf_get_simd(self.cpu, n as u32, value.as_mut_ptr()),
                        "read SIMD",
                    )?;
                }
                Ok(value.to_vec())
            }
            _ => Err(format!("HVF does not support register {r:?}")),
        }
    }
    fn write(&self, r: Register, bytes: &[u8]) -> Result<(), String> {
        if let Register::V(n @ 0..=31) = r {
            if bytes.len() != 16 {
                return Err("SIMD register requires 16 bytes".into());
            }
            return unsafe {
                ffi::check(
                    ffi::camera_hvf_set_simd(self.cpu, n as u32, bytes.as_ptr()),
                    "write SIMD",
                )
            };
        }
        if bytes.len() != 8 {
            return Err("AArch64 scalar register requires eight bytes".into());
        }
        let value = u64::from_le_bytes(bytes.try_into().unwrap());
        if let Some(id) = Self::id(r) {
            if id == ffi::PSTATE && (value & !0xf0000fcd != 0 || !matches!(value & 0x1f, 0 | 4 | 5))
            {
                return Err("HVF supports AArch64 EL0/EL1 PSTATE with BTYPE only".into());
            }
            return self.set_reg(id, value);
        }
        match r {
            Register::Sp => self.set_sys(self.sp()?, value),
            Register::Arm64System(r) => {
                let id = r.encoding()?;
                if id == ffi::MDSCR {
                    return if value == 0 {
                        Ok(())
                    } else {
                        Err("guest debug state is unsupported".into())
                    };
                }
                if Self::framework_register(id) {
                    return Err("architectural device registers belong to the framework".into());
                }
                self.set_sys(id, value)
            }
            _ => Err(format!("HVF does not support register {r:?}")),
        }
    }
    fn translation(&self) -> Result<TranslationState, String> {
        Ok(TranslationState {
            sctlr: self.sys(ffi::SCTLR)?,
            tcr: self.sys(ffi::TCR)?,
            ttbr0: self.sys(0xc100)?,
            ttbr1: self.sys(0xc101)?,
            pstate: self.reg(ffi::PSTATE)?,
        })
    }
    fn table_word(&self, address: u64) -> Result<u64, String> {
        let mut bytes = [0; 8];
        self.vm.read(address, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }
    fn translate(&self, va: u64, access: Access) -> Result<u64, String> {
        self.translation()?
            .translate(va, access, |a| self.table_word(a))
    }
    fn exclusive(&self, pc: u64, d: arm64::Exclusive) -> Result<(), String> {
        let address = if d.base == 31 {
            self.sys(self.sp()?)?
        } else {
            self.reg(d.base as u32)?
        };
        let size = d.bytes as usize * if d.pair { 2 } else { 1 };
        if address % size as u64 != 0 || (d.base == 31 && address % 16 != 0) {
            return Err("unaligned A64 exclusive access is unsupported".into());
        }
        let access = if d.load { Access::Read } else { Access::Write };
        let (pa, attr) = self
            .translation()?
            .walk(address, access, |a| self.table_word(a))?;
        let attr = attr.ok_or("exclusive access requires an MMU Normal-memory mapping")?;
        let mair = (self.sys(0xc510)? >> (attr * 8)) & 255;
        if mair & 0xf0 == 0 || mair & 15 == 0 || self.sys(ffi::SCTLR)? & (3 << 24) != 0 {
            return Err("Device/big-endian exclusive access is unsupported".into());
        }
        if self.translate(address + size as u64 - 1, access)? != pa + size as u64 - 1 {
            return Err("noncontiguous exclusive translation".into());
        }
        self.vm.check_access(pa, size, if d.load { 1 } else { 2 })?;
        let mut bytes = [0; 16];
        if d.load {
            self.vm
                .exclusive_load(self.monitor, pa, &mut bytes[..size])?;
            for (i, rt) in [d.first, d.second]
                .into_iter()
                .take(if d.pair { 2 } else { 1 })
                .enumerate()
            {
                if rt != 31 {
                    let mut value = [0; 8];
                    value[..d.bytes as usize]
                        .copy_from_slice(&bytes[i * d.bytes as usize..(i + 1) * d.bytes as usize]);
                    self.set_reg(rt as u32, u64::from_le_bytes(value))?;
                }
            }
            if d.acquire_release {
                std::sync::atomic::fence(Ordering::Acquire);
            }
        } else {
            for (i, rt) in [d.first, d.second]
                .into_iter()
                .take(if d.pair { 2 } else { 1 })
                .enumerate()
            {
                let value = if rt == 31 { 0 } else { self.reg(rt as u32)? };
                bytes[i * d.bytes as usize..(i + 1) * d.bytes as usize]
                    .copy_from_slice(&value.to_le_bytes()[..d.bytes as usize]);
            }
            if d.acquire_release {
                std::sync::atomic::fence(Ordering::Release);
            }
            let success = self.vm.exclusive_store(self.monitor, pa, &bytes[..size])?;
            if d.status != 31 {
                self.set_reg(d.status as u32, u64::from(!success))?;
            }
        }
        self.set_reg(ffi::PC, pc.wrapping_add(4))
    }
    fn observe_native_write(&self, word: u32, pc: u64) -> Result<(), String> {
        if !self.vm.has_exclusive() {
            return Ok(());
        }
        if word & 0x0a00_0000 == 0x0800_0000 {
            // Decode known scalar/SIMD/pair transfers. Unsupported memory
            // families conservatively lose every reservation before entry.
            match arm64::decode_load_store(word, pc, |r| {
                if r == 31 {
                    self.sys(self.sp()?)
                } else {
                    self.reg(r as u32)
                }
            }) {
                Ok(d) if !d.load => {
                    for t in d.transfers {
                        let mut at = t.address;
                        let mut left = t.size as usize;
                        while left != 0 {
                            let n = left.min(0x1000 - (at & 0xfff) as usize);
                            match self.translate(at, Access::Write) {
                                Ok(pa) => self.vm.written(Some((pa, n)))?,
                                Err(_) => self.vm.written(None)?,
                            }
                            at = at.wrapping_add(n as u64);
                            left -= n;
                        }
                    }
                }
                Ok(_) => (),
                Err(_) => self.vm.written(None)?,
            }
        }
        if word & 0xffff_ffe0 == 0xd50b_7420 {
            self.vm.written(None)?;
        } // DC ZVA
        if word == 0xd69f_03e0 || word & 0xff00_0000 == 0xd400_0000 {
            self.vm.clear_exclusive(self.monitor); // ERET / synchronous exception
        }
        Ok(())
    }
    fn snapshot(&self) -> Result<Registers, String> {
        let mut out = Registers {
            core: [0; 35],
            vectors: [[0; 16]; 32],
            system: Vec::new(),
            interrupts: self.interrupts,
            exclusive: self.vm.save_exclusive(self.monitor),
        };
        for (r, v) in out.core.iter_mut().enumerate() {
            *v = self.reg(r as u32)?;
        }
        out.core[ffi::PSTATE as usize] &= !(1 << 21);
        for (r, v) in out.vectors.iter_mut().enumerate() {
            unsafe {
                ffi::check(
                    ffi::camera_hvf_get_simd(self.cpu, r as u32, v.as_mut_ptr()),
                    "snapshot SIMD",
                )?;
            }
        }
        for &r in SYSTEM {
            out.system.push((r, self.sys(r)?));
        }
        Ok(out)
    }
    fn restore(&mut self, state: Registers) -> Result<(), String> {
        for (r, v) in state.core.iter().enumerate() {
            self.set_reg(r as u32, *v)?;
        }
        for (r, v) in state.vectors.iter().enumerate() {
            unsafe {
                ffi::check(
                    ffi::camera_hvf_set_simd(self.cpu, r as u32, v.as_ptr()),
                    "restore SIMD",
                )?;
            }
        }
        for (r, v) in state.system {
            self.set_sys(r, v)?;
        }
        self.interrupts = state.interrupts;
        self.vm.restore_exclusive(self.monitor, state.exclusive);
        Ok(())
    }
    fn deliver_interrupt(&self) -> Result<(), String> {
        let pstate = self.reg(ffi::PSTATE)? & !(1 << 21);
        let fiq = self.interrupts[1] && pstate & (1 << 6) == 0;
        let irq = self.interrupts[0] && pstate & (1 << 7) == 0;
        if !fiq && !irq {
            return Ok(());
        }
        self.vm.clear_exclusive(self.monitor);
        if pstate & !0xf0000fcd != 0 {
            return Err(
                "HVF interrupt entry supports AArch64 EL0/EL1 PSTATE with BTYPE only".into(),
            );
        }
        let mode = pstate & 0x1f;
        let offset = match mode {
            0 => 0x400,
            4 => 0,
            5 => 0x200,
            _ => return Err("HVF IRQ delivery supports AArch64 EL0/EL1 only".into()),
        } + if fiq { 0x100 } else { 0x80 };
        self.set_sys(0xc200, pstate)?;
        self.set_sys(0xc201, self.reg(ffi::PC)?)?;
        self.set_reg(ffi::PC, self.sys(0xc600)?.wrapping_add(offset))?;
        self.set_reg(ffi::PSTATE, (pstate & 0xf0000000) | 0x3c5)
    }
    fn paused(&mut self, commands: &mpsc::Receiver<Command>) -> Result<bool, String> {
        loop {
            let c = commands
                .recv()
                .map_err(|_| "HVF owner disconnected in callback")?;
            if let Op::Resume(handled) = c.op {
                let _ = c.reply.send(Ok(Value::Empty));
                return Ok(handled);
            }
            if matches!(c.op, Op::Run(..) | Op::Shutdown) {
                let _ = c
                    .reply
                    .send(Err("cannot run/destroy a CPU inside its callback".into()));
            } else {
                let result = self.execute(c.op);
                let _ = c.reply.send(result);
            }
        }
    }
    fn run(
        &mut self,
        request: Run,
        observe: bool,
        events: &mpsc::Sender<Event>,
        commands: &mpsc::Receiver<Command>,
    ) -> Result<Exit, ExecutionError> {
        self.shared.progress.store(0, Ordering::Relaxed);
        self.shared.measured.store(true, Ordering::Relaxed);
        self.shared.stop.store(false, Ordering::Release);
        if request.instructions == Some(0) {
            return Ok(Exit::Stopped);
        }
        if request.begin & 3 != 0 {
            return Err(ExecutionError::InvalidInstruction(
                "unaligned AArch64 entry PC".into(),
            ));
        }
        self.set_reg(ffi::PC, request.begin)?;
        let until = request
            .watchdog_us
            .checked_sub(1)
            .map(|_| Instant::now() + Duration::from_micros(request.watchdog_us));
        let limit = request.instructions.map(|n| n as u64).unwrap_or(u64::MAX);
        self.shared.limit.store(limit, Ordering::Release);
        while self.shared.progress.load(Ordering::Relaxed)
            < self.shared.limit.load(Ordering::Acquire)
        {
            if self.shared.stop.load(Ordering::Acquire)
                || until.is_some_and(|at| Instant::now() >= at)
            {
                return Ok(Exit::Stopped);
            }
            self.deliver_interrupt()?;
            let pc = self.reg(ffi::PC)?;
            if request.until == Some(pc) {
                return Ok(Exit::Stopped);
            }
            if observe {
                events
                    .send(Event::Before(pc))
                    .map_err(|_| "HVF observer disconnected")?;
                self.paused(commands)?;
                if self.shared.stop.load(Ordering::Acquire) {
                    return Ok(Exit::Stopped);
                }
                if self.reg(ffi::PC)? != pc {
                    continue;
                }
            }
            // Clock/PMU and ICC accesses belong to the framework. Decode them
            // before native entry so host clocks or a host GIC cannot leak in.
            let mut bytes = [0; 4];
            let pa = self.translate(pc, Access::Fetch)?;
            self.vm
                .read(pa, &mut bytes)
                .map_err(|message| ExecutionError::Memory {
                    fault: Fault::FetchUnmapped,
                    message,
                })?;
            self.vm
                .check_access(pa, 4, 4)
                .map_err(|message| ExecutionError::Memory {
                    fault: Fault::FetchProtected,
                    message,
                })?;
            let word = u32::from_le_bytes(bytes);
            let hint = match word {
                0xd503207f => Some(fujimod_emulator_cpu::Hint::WaitForInterrupt),
                0xd503205f => Some(fujimod_emulator_cpu::Hint::WaitForEvent),
                0xd503209f => Some(fujimod_emulator_cpu::Hint::SendEvent),
                0xd50320bf => Some(fujimod_emulator_cpu::Hint::SendEventLocal),
                _ => None,
            };
            if let Some(hint) = hint {
                self.set_reg(ffi::PC, pc.wrapping_add(4))?;
                self.shared.progress.fetch_add(1, Ordering::Relaxed);
                return Ok(Exit::Hint(hint));
            }
            if let Some(d) =
                arm64::decode_exclusive(word).map_err(ExecutionError::InvalidInstruction)?
            {
                self.exclusive(pc, d)?;
                self.shared.progress.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if word & 0xffff_f0ff == 0xd503_305f {
                // CLREX #imm
                self.vm.clear_exclusive(self.monitor);
                self.set_reg(ffi::PC, pc.wrapping_add(4))?;
                self.shared.progress.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            {
                if word & 0xffd00000 == 0xd5100000 && Self::framework_register((word >> 5) as u16) {
                    let syndrome = (0x18 << 26)
                        | (1 << 25)
                        | ((((word >> 19) & 3) as u64) << 20)
                        | ((((word >> 5) & 7) as u64) << 17)
                        | ((((word >> 16) & 7) as u64) << 14)
                        | ((((word >> 12) & 15) as u64) << 10)
                        | (((word & 31) as u64) << 5)
                        | ((((word >> 8) & 15) as u64) << 1)
                        | ((word >> 21) & 1) as u64;
                    self.shared.progress.fetch_add(1, Ordering::Relaxed);
                    let trap = Trap {
                        syndrome,
                        virtual_address: 0,
                        physical_address: 0,
                    };
                    events
                        .send(Event::Trap(trap))
                        .map_err(|_| "HVF trap owner disconnected")?;
                    if !self.paused(commands)? {
                        return Ok(Exit::Trap(trap));
                    }
                    continue;
                }
            }
            self.observe_native_write(word, pc)?;
            let exit = loop {
                let pstate = self.reg(ffi::PSTATE)?;
                self.set_reg(ffi::PSTATE, pstate | (1 << 21))?;
                unsafe {
                    ffi::check(ffi::hv_vcpu_run(self.cpu), "run")?;
                }
                self.set_reg(ffi::PSTATE, self.reg(ffi::PSTATE)? & !(1 << 21))?;
                let exit = unsafe { *self.exit };
                // The SDK latches cancellation issued while stopped for the
                // next entry. Drain that old latch without repeating entry
                // observations. An active cancellation sets stop first.
                if exit.reason != 0 || self.shared.stop.load(Ordering::Acquire) {
                    break exit;
                }
            };
            match exit.reason {
                0 if self.shared.stop.load(Ordering::Acquire) => {
                    self.shared.measured.store(false, Ordering::Relaxed);
                    return Err(
                        "host canceled a native instruction; its entry count is unavailable".into(),
                    );
                }

                1 => {
                    self.shared.progress.fetch_add(1, Ordering::Relaxed);
                    if exit.exception.syndrome >> 26 == 0x32 {
                        continue;
                    }
                    let trap = Trap {
                        syndrome: exit.exception.syndrome,
                        virtual_address: exit.exception.virtual_address,
                        physical_address: exit.exception.physical_address,
                    };
                    events
                        .send(Event::Trap(trap))
                        .map_err(|_| "HVF trap owner disconnected")?;
                    if !self.paused(commands)? {
                        return Ok(Exit::Trap(trap));
                    }
                }
                2 => return Err("masked host virtual timer unexpectedly activated".into()),
                n => {
                    return Err(ExecutionError::Backend(format!(
                        "unknown HVF exit reason {n}"
                    )));
                }
            }
        }
        Ok(Exit::Stopped)
    }
    fn execute(&mut self, op: Op) -> Result<Value, String> {
        match op {
            Op::Read(r) => Ok(Value::Bytes(self.read(r)?)),
            Op::Write(r, b) => {
                self.write(r, &b)?;
                Ok(Value::Empty)
            }
            Op::Translate(a, k) => Ok(Value::Address(self.translate(a, k)?)),
            Op::Snapshot => Ok(Value::Registers(self.snapshot()?)),
            Op::Restore(s) => {
                self.restore(s)?;
                Ok(Value::Empty)
            }
            Op::Interrupt(line, level) => {
                self.interrupts[usize::from(line == Interrupt::Fiq)] = level;
                Ok(Value::Empty)
            }
            _ => Err("invalid HVF command in this state".into()),
        }
    }
}
impl Drop for Processor {
    fn drop(&mut self) {
        self.vm
            .monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unregister(self.monitor);
        unsafe {
            let _ = ffi::hv_vcpu_destroy(self.cpu);
        }
    }
}
pub fn serve(
    vm: Arc<Machine>,
    shared: Arc<Shared>,
    commands: mpsc::Receiver<Command>,
    ready: mpsc::Sender<Result<u64, String>>,
) {
    let mut p = match Processor::create(vm, shared) {
        Ok(p) => p,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(p.cpu));
    while let Ok(c) = commands.recv() {
        match c.op {
            Op::Shutdown => {
                let _ = c.reply.send(Ok(Value::Empty));
                break;
            }
            Op::Run(request, observe, events) => {
                let result = p.run(request, observe, &events, &commands);
                let _ = events.send(Event::Done);
                let _ = c.reply.send(Ok(Value::Exit(result)));
            }
            op => {
                let result = p.execute(op);
                let _ = c.reply.send(result);
            }
        }
    }
}
