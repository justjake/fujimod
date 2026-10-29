//! Cortex-A53 register access through the pinned Unicorn AArch64 ABI.
use super::*;

#[repr(C)]
struct SystemRegister {
    crn: u32,
    crm: u32,
    op0: u32,
    op1: u32,
    op2: u32,
    value: u64,
}

fn ordinary(register: Register) -> Result<(i32, usize), String> {
    Ok(match register {
        Register::X(n @ 0..=28) => (199 + i32::from(n), 8),
        Register::X(29) => (1, 8),
        Register::X(30) => (2, 8),
        Register::Sp => (4, 8),
        Register::Pc => (260, 8),
        Register::Pstate => (265, 4),
        Register::V(n @ 0..=31) => (104 + i32::from(n), 16),
        Register::Fpcr => (291, 4),
        Register::Fpsr => (292, 4),
        _ => return Err(format!("unsupported AArch64 register {register:?}")),
    })
}

fn selected_stack(pstate: u64) -> arm::SystemRegister {
    let el = (pstate >> 2) & 3;
    let encoding = if el == 0 || pstate & 1 == 0 {
        0xc208
    } else {
        match el {
            1 => 0xe208,
            2 => 0xe210,
            3 => 0xe218,
            _ => unreachable!(),
        }
    };
    arm::SystemRegister::from_encoding(encoding)
}

impl Core {
    fn aarch64_system(
        &self,
        register: arm::SystemRegister,
        value: Option<u64>,
    ) -> Result<u64, String> {
        let encoding = register.encoding()?;
        if encoding == 0xc000
            && let Some(value) = value
        {
            let actual = self.aarch64_system(register, None)?;
            return if actual == value {
                Ok(actual)
            } else {
                Err(format!(
                    "MIDR_EL1 is fixed by the CPU model: {actual:#x}, requested {value:#x}"
                ))
            };
        }
        let mut raw = SystemRegister {
            crn: register.crn.into(),
            crm: register.crm.into(),
            op0: register.op0.into(),
            op1: register.op1.into(),
            op2: register.op2.into(),
            value: value.unwrap_or(0),
        };
        let mut size = std::mem::size_of::<SystemRegister>();
        self.check(unsafe {
            if value.is_some() {
                (self.api.uc_reg_write2)(
                    self.raw,
                    290,
                    (&raw as *const SystemRegister).cast(),
                    &mut size,
                )
            } else {
                (self.api.uc_reg_read2)(
                    self.raw,
                    290,
                    (&mut raw as *mut SystemRegister).cast(),
                    &mut size,
                )
            }
        })?;
        Ok(raw.value)
    }

    pub(super) fn read_aarch64(&self, register: Register, out: &mut [u8]) -> Result<usize, String> {
        if let Register::Arm64System(system) = register {
            if out.len() < 8 {
                return Err("AArch64 system register requires eight bytes".into());
            }
            out[..8].copy_from_slice(&self.aarch64_system(system, None)?.to_le_bytes());
            return Ok(8);
        }
        let (id, width) = ordinary(register)?;
        if out.len() < width {
            return Err("AArch64 register exceeds supplied buffer".into());
        }
        let mut size = width;
        self.check(unsafe {
            (self.api.uc_reg_read2)(self.raw, id, out.as_mut_ptr().cast(), &mut size)
        })?;
        if size != width {
            return Err("AArch64 register width differs from pinned ABI".into());
        }
        Ok(size)
    }

    pub(super) fn write_aarch64(&self, register: Register, bytes: &[u8]) -> Result<(), String> {
        if let Register::Arm64System(system) = register {
            let raw: [u8; 8] = bytes
                .try_into()
                .map_err(|_| "AArch64 system register requires eight bytes")?;
            self.aarch64_system(system, Some(u64::from_le_bytes(raw)))?;
            return Ok(());
        }
        let (id, width) = ordinary(register)?;
        if bytes.len() != width && !(width == 4 && bytes.len() == 8 && bytes[4..] == [0; 4]) {
            return Err("AArch64 register value has the wrong width".into());
        }
        if register == Register::Pc {
            // Preserve a redirected PC when a code observer stops its block.
            return self
                .check(unsafe { (self.api.uc_reg_write)(self.raw, id, bytes.as_ptr().cast()) });
        }
        if register == Register::Pstate {
            let mut current = [0; 8];
            self.read_aarch64(Register::Pstate, &mut current)?;
            let current_pstate = u32::from_le_bytes(current[..4].try_into().unwrap()) as u64;
            let requested_pstate = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as u64;
            let old_stack = selected_stack(current_pstate);
            let new_stack = selected_stack(requested_pstate);
            if old_stack.encoding()? != new_stack.encoding()? {
                // Unicorn's direct PSTATE write changes the selected bank
                // without performing the hardware SP save and restore.
                let mut stack = [0; 8];
                self.read_aarch64(Register::Sp, &mut stack)?;
                self.aarch64_system(old_stack, Some(u64::from_le_bytes(stack)))?;
                let mut pstate_size = width;
                self.check(unsafe {
                    (self.api.uc_reg_write2)(self.raw, id, bytes.as_ptr().cast(), &mut pstate_size)
                })?;
                let next_stack = self.aarch64_system(new_stack, None)?;
                let (sp_id, sp_width) = ordinary(Register::Sp)?;
                let mut sp_size = sp_width;
                self.check(unsafe {
                    (self.api.uc_reg_write2)(
                        self.raw,
                        sp_id,
                        (&next_stack as *const u64).cast(),
                        &mut sp_size,
                    )
                })?;
                return Ok(());
            }
        }
        let mut size = width;
        self.check(unsafe {
            (self.api.uc_reg_write2)(self.raw, id, bytes.as_ptr().cast(), &mut size)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(cpu: &Core, register: Register) -> u64 {
        let mut bytes = [0; 8];
        let size = cpu.read_register(register, &mut bytes).unwrap();
        assert!(matches!(size, 4 | 8));
        u64::from_le_bytes(bytes)
    }
    #[test]
    fn midr_handoff_verifies_the_immutable_cpu_model_identity() {
        let cpu = crate::test_support::library()
            .open(Model::ArmV8A, 1 << 20)
            .unwrap();
        let midr = Register::Arm64System(arm::SystemRegister::from_encoding(0xc000));
        let actual = value(&cpu, midr);
        cpu.write_register(midr, &actual.to_le_bytes()).unwrap();
        assert!(
            cpu.write_register(midr, &(actual ^ 1).to_le_bytes())
                .unwrap_err()
                .contains("fixed by the CPU model")
        );
    }
    #[test]
    fn physical_affinity_is_visible_to_el3_without_overwriting_x0() {
        let cpu = crate::test_support::library()
            .open(Model::ArmV8A, 1 << 20)
            .unwrap();
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        cpu.write_register(Register::Pstate, &0x3cdu64.to_le_bytes())
            .unwrap();
        cpu.write_register(Register::X(0), &123u64.to_le_bytes())
            .unwrap();
        let mpidr = Register::Arm64System(arm::SystemRegister::from_encoding(0xc005));
        cpu.write_register(mpidr, &0x80000103u64.to_le_bytes())
            .unwrap();
        assert_eq!(value(&cpu, mpidr), 0x80000103);
        assert_eq!(value(&cpu, Register::X(0)), 123);
        // MRS x1, MPIDR_EL1.
        cpu.write_memory(0x1000, &0xd53800a1u32.to_le_bytes())
            .unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: Some(0x1004),
            instructions: Some(1),
            watchdog_us: 1_000_000,
        })
        .unwrap();
        assert_eq!(value(&cpu, Register::X(1)), 0x80000103);
        assert!(cpu.write_register(mpidr, &u64::MAX.to_le_bytes()).is_err());
    }
    #[test]
    fn pstate_write_alone_changes_the_execution_exception_level() {
        let cpu = crate::test_support::library()
            .open(Model::ArmV8A, 1 << 20)
            .unwrap();
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        cpu.write_memory(0x1000, &0xd5384240u32.to_le_bytes())
            .unwrap();
        for (pstate, expected) in [(0x3cd, 12), (0x3c5, 4), (0x3cd, 12)] {
            cpu.write_register(Register::Pstate, &u64::to_le_bytes(pstate))
                .unwrap();
            cpu.run(Run {
                begin: 0x1000,
                until: Some(0x1004),
                instructions: Some(1),
                watchdog_us: 1_000_000,
            })
            .unwrap();
            assert_eq!(value(&cpu, Register::X(0)), expected);
        }
    }
    #[test]
    fn pstate_write_switches_and_restores_the_selected_stack_pointer_bank() {
        let cpu = crate::test_support::library()
            .open(Model::ArmV8A, 1 << 20)
            .unwrap();
        let sp_el0 = Register::Arm64System(arm::SystemRegister::from_encoding(0xc208));
        let sp_el1 = Register::Arm64System(arm::SystemRegister::from_encoding(0xe208));
        let el1t = 0x3c4u64;
        let el1h = 0x3c5u64;
        let stack_el0 = 0xaaaa0000u64;
        let stack_el1 = 0xbbbb0000u64;

        cpu.write_register(Register::Pstate, &el1t.to_le_bytes())
            .unwrap();
        cpu.write_register(Register::Sp, &stack_el0.to_le_bytes())
            .unwrap();
        cpu.write_register(sp_el1, &stack_el1.to_le_bytes())
            .unwrap();

        cpu.write_register(Register::Pstate, &el1h.to_le_bytes())
            .unwrap();
        assert_eq!(value(&cpu, Register::Sp), stack_el1);
        assert_eq!(value(&cpu, sp_el0), stack_el0);

        cpu.write_register(Register::Pstate, &el1t.to_le_bytes())
            .unwrap();
        assert_eq!(value(&cpu, Register::Sp), stack_el0);
        assert_eq!(value(&cpu, sp_el1), stack_el1);
    }
    #[test]
    fn a53_executes_el3_system_registers_and_eret_to_el1() {
        let cpu = crate::test_support::library()
            .open(Model::ArmV8A, 1 << 20)
            .unwrap();
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        let sys = |encoding| Register::Arm64System(arm::SystemRegister::from_encoding(encoding));
        cpu.write_register(Register::Pstate, &0x3cdu64.to_le_bytes())
            .unwrap();
        cpu.write_register(sys(0xf088), &0x401u64.to_le_bytes())
            .unwrap(); // SCR_EL3: AArch64 lower EL, NS.
        cpu.write_register(sys(0xe088), &0x80000000u64.to_le_bytes())
            .unwrap(); // HCR_EL2: AArch64 EL1.
        cpu.write_register(sys(0xf201), &0x1100u64.to_le_bytes())
            .unwrap(); // ELR_EL3.
        cpu.write_register(sys(0xf200), &0x3c5u64.to_le_bytes())
            .unwrap(); // SPSR_EL3: EL1h.
        // MRS x0,CurrentEL; ERET. At EL1, MRS x1,CurrentEL; ADD x2,x0,x1.
        cpu.write_memory(0x1000, &[0x40, 0x42, 0x38, 0xd5, 0xe0, 0x03, 0x9f, 0xd6])
            .unwrap();
        cpu.write_memory(0x1100, &[0x41, 0x42, 0x38, 0xd5, 0x02, 0x00, 0x01, 0x8b])
            .unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: Some(0x1108),
            instructions: Some(4),
            watchdog_us: 1_000_000,
        })
        .unwrap();
        assert_eq!(value(&cpu, Register::X(0)), 12);
        assert_eq!(value(&cpu, Register::X(1)), 4);
        assert_eq!(value(&cpu, Register::X(2)), 16);
        assert_eq!(value(&cpu, Register::Pstate) & 15, 5);
        let saved = cpu.snapshot(None).unwrap();
        cpu.write_register(Register::X(2), &99u64.to_le_bytes())
            .unwrap();
        cpu.restore(saved.as_ref()).unwrap();
        assert_eq!(value(&cpu, Register::X(2)), 16);
    }
}

#[cfg(test)]
mod interception_tests {
    use super::*;
    unsafe fn intercept(_: u64, _: u32, data: *mut c_void) {
        let cpu = unsafe { &*data.cast::<Core>() };
        cpu.stop_before_instruction().unwrap();
        assert!(cpu.stop_before_instruction().is_err());
    }
    #[test]
    fn observer_interception_keeps_pc_and_memory_before_the_instruction() {
        let cpu = crate::test_support::library()
            .open(Model::ArmV8A, 1 << 20)
            .unwrap();
        cpu.map_zeroed(0x1000, 0x2000, 7).unwrap();
        cpu.write_register(Register::Pstate, &0x3cdu64.to_le_bytes())
            .unwrap();
        cpu.write_register(Register::X(0), &0x2000u64.to_le_bytes())
            .unwrap();
        cpu.write_register(Register::X(1), &0x12345678u64.to_le_bytes())
            .unwrap();
        // STR x1,[x0].
        cpu.write_memory(0x1000, &0xf9000001u32.to_le_bytes())
            .unwrap();
        let hook = unsafe {
            cpu.install_hook(Hook::Instruction {
                range: HookRange::All,
                callback: intercept,
                data: (&cpu as *const Core).cast_mut().cast(),
            })
        }
        .unwrap();
        assert!(cpu.stop_before_instruction().is_err());
        cpu.run(Run {
            begin: 0x1000,
            until: None,
            instructions: Some(1),
            watchdog_us: 1_000_000,
        })
        .unwrap();
        let mut pc = [0; 8];
        cpu.read_register(Register::Pc, &mut pc).unwrap();
        assert_eq!(u64::from_le_bytes(pc), 0x1000);
        let mut bytes = [1; 8];
        cpu.read_memory(0x2000, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 8]);
        cpu.remove_hook(hook).unwrap();
    }
}

#[cfg(test)]
mod accounting_tests;
