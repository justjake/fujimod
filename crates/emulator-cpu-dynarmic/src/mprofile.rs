use super::*;
use arm::Register as Arm;

const CPSR_T: u32 = 1 << 5;
const XPSR_T: u32 = 1 << 24;
const APSR_IT: u32 = 0xfe0f_fc00;
const IPSR_MASK: u32 = 0x1ff;
const SPSEL: u32 = 1 << 1;
const FPSCR_MASK: u32 = 0xf7c0_009f;
const INVSTATE: u64 = 1 << 17; // UFSR.INVSTATE in the combined CFSR.

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

    pub(super) fn snapshot_mprofile(&self) -> Snapshot {
        Snapshot {
            registers: self.mprofile.get(),
            floating: std::array::from_fn(|i| unsafe { ffi::dynarmic_get_s(self.inst, i as u32) }),
        }
    }

    pub(super) fn restore_mprofile(&self, snapshot: &Snapshot) {
        self.mprofile.set(snapshot.registers);
        for (i, value) in snapshot.floating.into_iter().enumerate() {
            unsafe { ffi::dynarmic_set_s(self.inst, i as u32, value) };
        }
    }

    pub(super) fn run_thumb(
        &self,
        until: Option<u64>,
        remaining: impl Fn() -> u64,
    ) -> Result<Exit, ExecutionError> {
        self.shared.before_run(false);
        while remaining() != 0 {
            let pc = self.reg(15);
            if until == Some(u64::from(pc)) || self.shared.stopped.get() {
                break;
            }
            self.shared.flush_changed_code(false);
            if !self.shared.observers.borrow().is_empty() {
                let aligned = pc & !3;
                let first = self.shared.fetch(aligned).ok().and_then(Result::ok);
                if let Some(word) = first {
                    let halfword = (word >> ((pc & 2) * 8)) as u16;
                    let bytes = if halfword < 0xe800 { 2 } else { 4 };
                    self.shared.observe_instruction(pc, bytes);
                    if self.shared.stopped.get() || remaining() == 0 {
                        break;
                    }
                }
            }
            self.shared.fault.replace(None);
            let mut executed = 0;
            let status = unsafe { ffi::dynarmic_step(self.inst, &mut executed) };
            if let Some(error) = self.shared.fault.take() {
                return Err(error);
            }
            self.last_progress.set(self.last_progress.get() + executed);
            let cpsr = self.cpsr();
            if cpsr & CPSR_T == 0 {
                // An exchange to an even target completed. Report INVSTATE
                // at the target before dispatching any code there.
                unsafe { ffi::dynarmic_set_cpsr(self.inst, cpsr | CPSR_T) };
                let address = u64::from(self.reg(15));
                return Ok(Exit::Trap(Trap {
                    syndrome: INVSTATE,
                    virtual_address: address,
                    physical_address: address,
                }));
            }
            if status != ffi::DynarmicExitReason::Timeout {
                return Self::finish_run(status);
            }
            if executed == 0 {
                return Err(format!("Dynarmic did not advance at {pc:#010x}").into());
            }
        }
        Ok(Exit::Stopped)
    }
}
