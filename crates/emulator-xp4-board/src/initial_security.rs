//! Construction-only Cortex-A7 security inputs. These select a declared
//! post-reset execution state; they do not model a ROM or grant guest writes.
use fujimod_emulator_cpu::{Cpu, Model, Register, arm};
use fujimod_emulator_cpu_unicorn::Core;
use serde::Deserialize;

const SCR_NS: u32 = 1;
const NSACR_CP10_CP11: u32 = (1 << 10) | (1 << 11);
const NSACR_NS_SMP: u32 = 1 << 18;

/// Explicit per-core security registers, applied before `initial_registers`.
///
/// This bounded input supports SCR.NS and NSACR.NS_SMP, plus matching CP10/CP11
/// permissions. Other SCR/NSACR bits and unequal CP10/CP11 permissions are
/// refused, not masked. In particular, it does not select interrupt routing,
/// virtualization traps, or a complete floating-point security policy.
/// ACTLR, CPACR and FPEXC are not enabled by this initializer.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialSecurity {
    pub scr: u32,
    pub nsacr: u32,
}

fn selector(opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn: 1,
        crm: 1,
        opc2,
        secure: 1,
        wide: false,
    }
}

pub(crate) fn is_cpsr(register: Register) -> bool {
    matches!(register, Register::Pstate | Register::Arm32(arm::Register::Cpsr))
}

impl InitialSecurity {
    pub(crate) fn validate(self) -> Result<(), String> {
        if self.scr & !SCR_NS != 0 {
            return Err("Initial Cortex-A7 SCR only supports the NS bit".into());
        }
        if self.nsacr & !(NSACR_NS_SMP | NSACR_CP10_CP11) != 0
            || !matches!(self.nsacr & NSACR_CP10_CP11, 0 | NSACR_CP10_CP11)
        {
            return Err("Initial Cortex-A7 NSACR requires supported bits and paired CP10/CP11 permissions".into());
        }
        Ok(())
    }

    pub(crate) fn validate_cpsr(self, cpsr: u32) -> Result<(), String> {
        let mode = cpsr & 31;
        if !matches!(mode, 0x10 | 0x11 | 0x12 | 0x13 | 0x16 | 0x17 | 0x1a | 0x1b | 0x1f)
            || (mode == 0x1a && self.scr & SCR_NS == 0)
        {
            return Err("Initial Cortex-A7 security state has an invalid CPSR mode".into());
        }
        Ok(())
    }

    fn check(self, cpu: &Core, scr: u32, cpsr: u32) -> Result<(), String> {
        let mut bytes = [0; 4];
        if cpu.read_register(arm::CPSR, &mut bytes)? != bytes.len()
            || u32::from_le_bytes(bytes) != cpsr
            || cpu.coprocessor(selector(0), None)? != u64::from(scr)
            || cpu.coprocessor(selector(2), None)? != u64::from(self.nsacr)
        {
            return Err("Initial Cortex-A7 security or execution-mode readback differs".into());
        }
        Ok(())
    }

    pub(crate) fn apply(self, cpu: &Core) -> Result<(), String> {
        if cpu.model() != Model::CortexA7 {
            return Err("Initial security state requires Cortex-A7".into());
        }
        self.validate()?;
        cpu.coprocessor(selector(2), Some(u64::from(self.nsacr)))?;
        // A direct Non-secure SVC -> Monitor/Hyp CPSR write can silently
        // retain SVC. First establish Secure SVC and rebuild execution flags,
        // then use Monitor to select the declared final SVC regime.
        cpu.coprocessor(selector(0), Some(0))?;
        cpu.write_register(arm::CPSR, &0xd3u32.to_le_bytes())?;
        self.check(cpu, 0, 0xd3)?;
        cpu.write_register(arm::CPSR, &0xd6u32.to_le_bytes())?;
        self.check(cpu, 0, 0xd6)?;
        cpu.coprocessor(selector(0), Some(u64::from(self.scr)))?;
        self.check(cpu, self.scr, 0xd6)?;
        cpu.write_register(arm::CPSR, &0xd3u32.to_le_bytes())?;
        self.verify(cpu, 0xd3)
    }

    pub(crate) fn verify(self, cpu: &Core, cpsr: u32) -> Result<(), String> {
        self.validate_cpsr(cpsr)?;
        // Check each requested CPSR write as well as the final state. This
        // rejects a backend-refused mode change before a later register write
        // could accidentally target the wrong bank. No guest is run here.
        self.check(cpu, self.scr, cpsr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_cpu_unicorn::Library;

    #[test]
    fn another_cpu_model_is_rejected_before_coprocessor_access() {
        let cpu = Library::linked().unwrap().open(Model::CortexR5F, 1 << 20).unwrap();
        let error = InitialSecurity { scr: 0, nsacr: 0 }.apply(&cpu).unwrap_err();
        assert_eq!(error, "Initial security state requires Cortex-A7");
    }
}
