use super::*;
use arm::Register as Arm;
use arm::thumb;

const CPSR_T: u32 = 1 << 5;
const XPSR_T: u32 = 1 << 24;
const APSR_IT: u32 = 0xfe0f_fc00;
const IPSR_MASK: u32 = 0x1ff;
const SPSEL: u32 = 1 << 1;
const FPSCR_MASK: u32 = 0xf7c0_009f;
const CP10_CP11: u32 = 0x00f0_0000;

const CODE_PAGE_BYTES: usize = 4096;
const CODE_WORD_BYTES: usize = 4;

pub(super) struct CodePage {
    backing: *const [u8; CODE_PAGE_BYTES],
    bytes: Box<[u8; CODE_PAGE_BYTES]>,
    fetched: [u64; CODE_PAGE_BYTES / CODE_WORD_BYTES / 64],
}

impl CoreShared {
    pub(super) fn watch_thumb_code(&self, address: u32, pointer: *const u8) {
        let offset = address as usize % CODE_PAGE_BYTES;
        let base = address - offset as u32;
        let mut pages = self.thumb_code.borrow_mut();
        let page = pages.entry(base).or_insert_with(|| {
            // M4 fetches are untranslated, and each backing mapping starts
            // and ends on a guest page boundary. Remapping drops these pointers.
            let backing = unsafe { pointer.sub(offset) }.cast::<[u8; CODE_PAGE_BYTES]>();
            CodePage {
                backing,
                bytes: Box::new(unsafe { backing.read() }),
                fetched: [0; CODE_PAGE_BYTES / CODE_WORD_BYTES / 64],
            }
        });
        let word = offset / CODE_WORD_BYTES;
        page.fetched[word / 64] |= 1 << (word % 64);
        page.bytes[offset..offset + CODE_WORD_BYTES]
            .copy_from_slice(&unsafe { pointer.cast::<[u8; CODE_WORD_BYTES]>().read() });
    }

    pub(super) fn revalidate_thumb_code(&self) {
        for (&address, page) in self.thumb_code.borrow_mut().iter_mut() {
            // The board has stopped all writers, including hardware peers
            // that cannot report stores through the native code watcher.
            let current = unsafe { &*page.backing };
            if current == page.bytes.as_ref() {
                continue;
            }
            let changed = page.fetched.iter().enumerate().any(|(group, &bits)| {
                let mut words = bits;
                while words != 0 {
                    let offset = (group * 64 + words.trailing_zeros() as usize) * CODE_WORD_BYTES;
                    let range = offset..offset + CODE_WORD_BYTES;
                    if current[range.clone()] != page.bytes[range] {
                        return true;
                    }
                    words &= words - 1;
                }
                false
            });
            page.bytes.copy_from_slice(current);
            if changed {
                unsafe {
                    ffi::dynarmic_invalidate_cache(self.inst.get(), address, CODE_PAGE_BYTES)
                };
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Registers {
    // Dynarmic's R13 holds the active stack. Exchange it with this value
    // on a mode or SPSEL change so guest writes immediately reach the bank.
    inactive_sp: u32,
    control: u32,
    primask: u32,
    basepri: u32,
    faultmask: u32,
    ipsr: u32,
}

impl Registers {
    fn uses_psp(self) -> bool {
        self.ipsr == 0 && self.control & SPSEL != 0
    }
}

pub(super) struct Snapshot {
    registers: Registers,
    floating: [u32; 32],
    cpacr: u32,
}

impl Core {
    pub(super) fn read_mprofile_register(&self, register: Register) -> Result<u32, String> {
        let state = self.mprofile.get();
        let value = match register {
            Register::Pc => self.reg(15),
            Register::Sp => self.reg(13),
            Register::Arm32(Arm::R(n @ 0..=15)) => self.reg(n.into()),
            Register::Arm32(Arm::Msp | Arm::Psp) => {
                if state.uses_psp() == (register == Register::Arm32(Arm::Psp)) {
                    self.reg(13)
                } else {
                    state.inactive_sp
                }
            }
            Register::Arm32(Arm::Control) => state.control,
            Register::Arm32(Arm::Primask) => state.primask,
            Register::Arm32(Arm::Basepri) => state.basepri,
            Register::Arm32(Arm::Faultmask) => state.faultmask,
            Register::Arm32(Arm::Ipsr) => state.ipsr,
            Register::Arm32(Arm::Cpacr) => self.shared.thumb_cpacr.get(),
            Register::Arm32(Arm::Xpsr) => (self.cpsr() & APSR_IT) | XPSR_T | state.ipsr,
            Register::Arm32(Arm::Cpsr) | Register::Pstate => self.cpsr() & (APSR_IT | CPSR_T),
            Register::Arm32(Arm::S(n @ 0..=31)) => unsafe {
                ffi::dynarmic_get_s(self.inst, n.into())
            },
            Register::Arm32(Arm::Fpscr) => {
                (unsafe { ffi::dynarmic_get_fpscr(self.inst) }) & FPSCR_MASK
            }
            _ => return Err(format!("unsupported Cortex-M4 register {register:?}")),
        };
        Ok(value)
    }

    pub(super) fn write_mprofile_register(
        &self,
        register: Register,
        value: u32,
    ) -> Result<(), String> {
        let mut state = self.mprofile.get();
        let used_psp = state.uses_psp();
        match register {
            Register::Pc | arm::PC => self.set_reg(15, value & !1),
            Register::Sp | Register::Arm32(Arm::R(13)) => self.set_reg(13, value & !3),
            Register::Arm32(Arm::R(n @ 0..=14)) => self.set_reg(n.into(), value),
            Register::Arm32(Arm::Msp | Arm::Psp) => {
                if used_psp == (register == Register::Arm32(Arm::Psp)) {
                    self.set_reg(13, value & !3);
                } else {
                    state.inactive_sp = value & !3;
                }
            }
            Register::Arm32(Arm::Control) => state.control = value & 7,
            Register::Arm32(Arm::Primask) => state.primask = value & 1,
            Register::Arm32(Arm::Basepri) => state.basepri = value & 0xff,
            Register::Arm32(Arm::Faultmask) => state.faultmask = value & 1,
            Register::Arm32(Arm::Ipsr) => state.ipsr = value & IPSR_MASK,
            Register::Arm32(Arm::Cpacr) => self.set_thumb_cpacr(value),
            Register::Arm32(Arm::Xpsr) => {
                state.ipsr = value & IPSR_MASK;
                self.write_thumb_flags(value);
            }
            Register::Arm32(Arm::Cpsr) | Register::Pstate => self.write_thumb_flags(value),
            Register::Arm32(Arm::S(n @ 0..=31)) => unsafe {
                ffi::dynarmic_set_s(self.inst, n.into(), value);
            },
            Register::Arm32(Arm::Fpscr) => unsafe {
                ffi::dynarmic_set_fpscr(self.inst, value & FPSCR_MASK);
            },
            _ => return Err(format!("unsupported Cortex-M4 register {register:?}")),
        }
        if used_psp != state.uses_psp() {
            let active_sp = self.reg(13);
            self.set_reg(13, std::mem::replace(&mut state.inactive_sp, active_sp));
        }
        self.mprofile.set(state);
        Ok(())
    }

    fn write_thumb_flags(&self, value: u32) {
        let cpsr = (self.cpsr() & !APSR_IT) | CPSR_T | (value & APSR_IT);
        unsafe { ffi::dynarmic_set_cpsr(self.inst, cpsr) };
    }

    fn set_thumb_cpacr(&self, value: u32) {
        if self.shared.thumb_cpacr.replace(value & CP10_CP11) != value & CP10_CP11 {
            unsafe { ffi::dynarmic_clear_cache(self.inst) };
        }
    }

    pub(super) fn snapshot_mprofile(&self) -> Snapshot {
        Snapshot {
            registers: self.mprofile.get(),
            floating: std::array::from_fn(|i| unsafe { ffi::dynarmic_get_s(self.inst, i as u32) }),
            cpacr: self.shared.thumb_cpacr.get(),
        }
    }

    pub(super) fn restore_mprofile(&self, snapshot: &Snapshot) {
        self.mprofile.set(snapshot.registers);
        self.set_thumb_cpacr(snapshot.cpacr);
        for (i, value) in snapshot.floating.into_iter().enumerate() {
            unsafe { ffi::dynarmic_set_s(self.inst, i as u32, value) };
        }
    }

    pub(super) unsafe extern "C" fn thumb_batch_instruction(
        user_data: *mut c_void,
        first: u16,
    ) -> ffi::ThumbBatchInstruction {
        let shared = unsafe { &*user_data.cast::<CoreShared>() };
        if matches!(first & 0xfe00, 0xb400 | 0xbc00) && first & 0x01ff != 0 {
            return ffi::ThumbBatchInstruction::StackAccess;
        }
        // Native execution checks the Thumb state and EXC_RETURN after a
        // branch. BX need not stop at the board before it runs.
        if (thumb::mprofile_observed(first) && first & 0xff87 != 0x4700)
            || thumb::mprofile_unmasks(first)
            || matches!(first, 0xb400..=0xb5ff | 0xbc00..=0xbdff | 0xc000..=0xcfff
                | 0xe800..=0xe9ff | 0xec00..=0xedff)
            || (matches!(first, 0xee00..=0xefff) && shared.thumb_cpacr.get() != CP10_CP11)
        {
            ffi::ThumbBatchInstruction::Observe
        } else if matches!(first, 0x4800..=0x9fff | 0xf800..=0xf9ff) {
            ffi::ThumbBatchInstruction::SingleAccess
        } else {
            ffi::ThumbBatchInstruction::Ordinary
        }
    }

    /// Preflight an unconditional 16-bit PUSH or POP before its first
    /// transfer. The slow path replays the whole instruction.
    pub(super) unsafe extern "C" fn guard_thumb_stack(
        user_data: *mut c_void,
        instruction: u32,
        sp: u32,
    ) {
        let shared = unsafe { &*user_data.cast::<CoreShared>() };
        let write = instruction & 0x0800 == 0;
        let len = 4 * (instruction & 0x01ff).count_ones();
        let start = if write { sp.wrapping_sub(len) } else { sp };
        let access = if write { Access::Write } else { Access::Read };
        let room = 4096 - (start & 4095);
        if len <= room && start.is_multiple_of(4) && (!write || !shared.thumb_observe_stores.get())
        {
            let inline = shared.inline_page(start).addr();
            if inline != 0 && (!write || inline & 1 == 0) {
                return;
            }
            if let Some(NativeAccess::Ram(pointer, _)) =
                shared.native_access(start, len as usize, access)
                && (!write || !shared.code_tracker.overlaps(pointer, len as usize))
            {
                if cfg!(target_arch = "aarch64") {
                    shared.install_inline_page(start, pointer, access);
                }
                return;
            }
        }
        // The first access must reach the callback before any load can change
        // a register or any store can change memory.
        shared.clear_inline_page(start >> 12);
        shared.thumb_stack_rejected.set(true);
    }

    fn thumb_halfword(&self, pc: u32) -> Result<u16, ExecutionError> {
        let address = pc & !3;
        let word = self
            .shared
            .fetch(address)
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| ExecutionError::Memory {
                fault: Fault::FetchUnmapped,
                message: format!("Dynarmic Thumb instruction fetch at {pc:#010x}"),
            })?;
        Ok((word >> ((pc & 2) * 8)) as u16)
    }

    fn thumb_exit(&self) -> Option<Exit> {
        let pc = self.reg(15);
        let cpsr = self.cpsr();
        if cpsr & CPSR_T == 0 {
            unsafe { ffi::dynarmic_set_cpsr(self.inst, cpsr | CPSR_T) };
            Some(Self::thumb_fault(thumb::INVSTATE, pc))
        } else if pc >= thumb::EXC_RETURN_START {
            Some(Exit::MProfileExceptionReturn(pc | 1))
        } else {
            None
        }
    }

    fn thumb_fault(syndrome: u64, pc: u32) -> Exit {
        Exit::Trap(Trap {
            syndrome,
            virtual_address: u64::from(pc),
            physical_address: u64::from(pc),
        })
    }

    fn thumb_privileged(&self) -> bool {
        let state = self.mprofile.get();
        state.ipsr != 0 || state.control & 1 == 0
    }

    fn thumb_vfp_accessible(&self) -> bool {
        let cpacr = self.shared.thumb_cpacr.get();
        cpacr == CP10_CP11 || cpacr == 0x0050_0000 && self.thumb_privileged()
    }

    fn thumb_special(&self, first: u16, second: u16, cpsr: u32) -> Result<bool, String> {
        let mrs = first == 0xf3ef && second & 0xf000 == 0x8000;
        let msr = first & 0xfff0 == 0xf380 && second & 0xf000 == 0x8000;
        let cps = first & 0xffe8 == 0xb660;
        if !(mrs || msr || cps) {
            return Ok(false);
        }
        if !thumb::condition_passed(cpsr) {
            return Ok(true);
        }
        let privileged = self.thumb_privileged();
        let sysm = second as u8;
        let register = match sysm {
            8 => Some(Arm::Msp),
            9 => Some(Arm::Psp),
            16 => Some(Arm::Primask),
            17 | 18 => Some(Arm::Basepri),
            19 => Some(Arm::Faultmask),
            20 => Some(Arm::Control),
            _ => None,
        }
        .map(Register::Arm32);
        if mrs {
            let rt = u32::from((second >> 8) & 15);
            if matches!(rt, 13 | 15) {
                return Err("unpredictable M-profile MRS destination".into());
            }
            let value = if sysm <= 7 {
                let apsr = if sysm & 4 == 0 { cpsr & 0xf80f_0000 } else { 0 };
                let ipsr = if privileged && sysm & 1 != 0 {
                    self.mprofile.get().ipsr
                } else {
                    0
                };
                apsr | ipsr
            } else if let Some(register) = register.filter(|_| privileged || sysm == 20) {
                self.read_mprofile_register(register)?
            } else {
                0
            };
            self.set_reg(rt, value);
        } else if cps {
            if privileged {
                let mask = u32::from((first >> 4) & 1);
                if first & 2 != 0 {
                    self.write_mprofile_register(Register::Arm32(Arm::Primask), mask)?;
                }
                if first & 1 != 0 {
                    self.write_mprofile_register(Register::Arm32(Arm::Faultmask), mask)?;
                }
            }
        } else {
            let rn = u32::from(first & 15);
            if matches!(rn, 13 | 15) {
                return Err("unpredictable M-profile MSR source".into());
            }
            let mut value = self.reg(rn);
            if sysm <= 7 && sysm & 4 == 0 {
                let mask = if second & 0x0800 != 0 { 0xf800_0000 } else { 0 }
                    | if second & 0x0400 != 0 { 0x000f_0000 } else { 0 };
                self.write_thumb_flags((cpsr & !mask) | (value & mask));
            } else if let Some(register) = register.filter(|_| privileged) {
                if sysm == 18 {
                    value &= 0xff;
                    let basepri = self.mprofile.get().basepri;
                    if value == 0 || (basepri != 0 && value >= basepri) {
                        return Ok(true);
                    }
                }
                if sysm == 20 && self.mprofile.get().ipsr != 0 {
                    value = (value & !SPSEL) | (self.mprofile.get().control & SPSEL);
                }
                self.write_mprofile_register(register, value)?;
            }
        }
        Ok(true)
    }

    pub(super) fn run_thumb(&self, request: Run, batch: bool) -> Result<Exit, ExecutionError> {
        if self.run_limit.get().is_some() {
            return Err("nested Dynarmic execution is unsupported".into());
        }
        self.last_progress.set(0);
        if request.watchdog_us != 0 {
            return Err("Dynarmic does not support a host watchdog".into());
        }
        if request.instructions == Some(0) {
            return Ok(Exit::Stopped);
        }
        let begin = u32::try_from(request.begin).map_err(|_| "Dynarmic start exceeds 32 bits")?;
        self.set_reg(15, begin & !1);
        self.run_limit
            .set(Some(request.instructions.unwrap_or(usize::MAX) as u64));
        let _reset = ResetRunLimit(&self.run_limit, &self.shared.read_anchor);
        let remaining = || {
            self.run_limit
                .get()
                .unwrap()
                .saturating_sub(self.last_progress.get())
        };
        self.shared.device_accessed.set(false);
        self.shared.stopped.set(false);
        let single_entry = request.until.is_some()
            || self.shared.observers.borrow().iter().any(|(_, hook)| {
                matches!(
                    hook,
                    Hook::Instruction { .. } | Hook::Memory { read: true, .. }
                )
            });
        let observe_stores = self.shared.observers.borrow().iter().any(|(_, hook)| {
            matches!(hook, Hook::Memory { write: true, .. })
        });
        if self.shared.thumb_observe_stores.replace(observe_stores) != observe_stores {
            // Native store entries consult this guard even for pages already
            // cached as writable before the owner installed its observer.
            unsafe { ffi::dynarmic_observe_stores(self.inst, observe_stores) };
            if !observe_stores {
                self.shared.clear_inline_pages();
            }
        }
        self.shared.before_run(!single_entry);
        while remaining() != 0 {
            let pc = self.reg(15);
            if request.until == Some(u64::from(pc)) || self.shared.stopped.get() {
                break;
            }
            if let Some(exit) = self.thumb_exit() {
                return Ok(exit);
            }
            self.shared.flush_changed_code(false);
            if !single_entry {
                self.shared.thumb_stack_rejected.set(false);
                self.shared.batching.set(true);
                let mut executed = 0;
                let native_budget = remaining().min(i64::MAX as u64);
                let status = unsafe { ffi::dynarmic_run(self.inst, native_budget, &mut executed) };
                self.shared.batching.set(false);
                let replay = self.shared.replay_memory.take().is_some();
                if executed > remaining() {
                    return Err("Dynarmic Thumb instruction budget overrun".into());
                }
                self.last_progress.set(self.last_progress.get() + executed);
                if let Some(exit) = self.thumb_exit() {
                    return Ok(exit);
                }
                if !replay
                    && !matches!(
                        status,
                        ffi::DynarmicExitReason::Timeout | ffi::DynarmicExitReason::Replay
                    )
                {
                    return Self::finish_run(status);
                }
                if executed != 0 {
                    if batch {
                        break;
                    }
                    continue;
                }
                // The entry is observed, needs a slow access, or is in a
                // translated block larger than the remaining budget.
            }

            let pc = self.reg(15);
            let first = self.thumb_halfword(pc)?;
            let bytes = if first < 0xe800 { 2 } else { 4 };
            self.shared.observe_instruction(pc, bytes);
            if self.shared.stopped.get() || remaining() == 0 {
                break;
            }
            let second = if bytes == 4 {
                self.thumb_halfword(pc.wrapping_add(2))?
            } else {
                0
            };
            let cpsr = self.cpsr();
            if thumb::condition_passed(cpsr)
                && matches!(first, 0xec00..=0xefff)
                && matches!(second & 0x0f00, 0x0a00 | 0x0b00)
                && !self.thumb_vfp_accessible()
            {
                return Ok(Self::thumb_fault(thumb::NOCP, pc));
            }
            if self.thumb_special(first, second, cpsr)? {
                self.write_thumb_flags(thumb::advance_it(self.cpsr()));
                self.set_reg(15, pc.wrapping_add(bytes));
                self.last_progress.set(self.last_progress.get() + 1);
                if batch {
                    break;
                }
                continue;
            }

            self.shared.fault.replace(None);
            self.shared.abort.set(None);
            let mut executed = 0;
            self.shared.thumb_stepping.set(true);
            let status = unsafe { ffi::dynarmic_step(self.inst, &mut executed) };
            self.shared.thumb_stepping.set(false);
            if let Some(error) = self.shared.fault.take() {
                return Err(error);
            }
            self.last_progress.set(self.last_progress.get() + executed);
            if let Some(exit) = self.thumb_exit() {
                return Ok(exit);
            }
            if status != ffi::DynarmicExitReason::Timeout {
                return Self::finish_run(status);
            }
            if executed == 0 {
                return Err(format!("Dynarmic did not advance at {pc:#010x}").into());
            }
            if batch || self.shared.device_accessed.get() {
                break;
            }
        }
        Ok(Exit::Stopped)
    }
}
