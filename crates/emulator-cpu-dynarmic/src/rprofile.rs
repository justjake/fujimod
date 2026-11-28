//! Cortex-R5F memory-protection state that does not exist in the A32 JIT.
use super::{Core, ffi};
use fujimod_emulator_cpu::{Access, arm, armv7r};
use serde::{Deserialize, Serialize};

const REGIONS: usize = 16;

#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct Mpu {
    selected: u8,
    regions: [armv7r::Region; REGIONS],
}

#[derive(Clone, Copy)]
pub(super) enum MpuRegister {
    Type,
    Number,
    Base,
    Size,
    Attributes,
}

impl MpuRegister {
    pub(super) fn decode(cp: arm::Coprocessor) -> Option<Self> {
        if cp.cp != 15 || cp.opc1 != 0 || cp.wide {
            return None;
        }
        Some(match (cp.crn, cp.crm, cp.opc2) {
            (0, 0, 4) => Self::Type,
            (6, 2, 0) => Self::Number,
            (6, 1, 0) => Self::Base,
            (6, 1, 2) => Self::Size,
            (6, 1, 4) => Self::Attributes,
            _ => return None,
        })
    }
}

impl Mpu {
    pub(super) fn valid(&self) -> bool {
        usize::from(self.selected) < REGIONS
    }

    fn read(&self, register: MpuRegister) -> u32 {
        let region = self.regions[usize::from(self.selected)];
        match register {
            MpuRegister::Type => (REGIONS as u32) << 8,
            MpuRegister::Number => u32::from(self.selected),
            MpuRegister::Base => region.base,
            MpuRegister::Size => region.size,
            MpuRegister::Attributes => region.attributes,
        }
    }

    fn write(&mut self, register: MpuRegister, value: u32) -> Result<bool, String> {
        let region = &mut self.regions[usize::from(self.selected)];
        let field = match register {
            MpuRegister::Type => return Err("MPUIR is read-only".into()),
            MpuRegister::Number => {
                if value >= REGIONS as u32 {
                    return Err(format!("MPU region {value} is outside the Cortex-R5F"));
                }
                self.selected = value as u8;
                return Ok(false);
            }
            MpuRegister::Base => &mut region.base,
            MpuRegister::Size => &mut region.size,
            MpuRegister::Attributes => &mut region.attributes,
        };
        Ok(std::mem::replace(field, value) != value)
    }

    /// Check both ends and every region boundary inside one guest page.
    pub(super) fn check(
        &self,
        sctlr: u32,
        cpsr: u32,
        address: u32,
        bytes: usize,
        access: Access,
    ) -> Result<(), u32> {
        let user = cpsr & 31 == 0x10;
        let begin = u64::from(address);
        let last = begin + bytes.saturating_sub(1) as u64;
        let last_address = u32::try_from(last).map_err(|_| 0x0d_u32)?;
        let check = |address| {
            armv7r::check(sctlr, &self.regions, address, access, user).map_err(
                |fault| match fault {
                    armv7r::ProtectionFault::Background => 0,
                    armv7r::ProtectionFault::Permission => 0x0d,
                },
            )
        };
        check(address)?;
        if sctlr & 1 != 0 {
            for boundary in self.regions.iter().flat_map(|region| region.boundaries()) {
                if begin < boundary && boundary < last {
                    check(boundary as u32)?;
                }
            }
        }
        if address != last_address {
            check(last_address)?;
        }
        Ok(())
    }
}

pub(super) fn transfer(word: u32) -> Option<(MpuRegister, u32, bool)> {
    if word & 0x0f00_0f10 != 0x0e00_0f10 {
        return None;
    }
    let register = MpuRegister::decode(arm::Coprocessor {
        cp: 15,
        opc1: (word >> 21) & 7,
        crn: (word >> 16) & 15,
        crm: word & 15,
        opc2: (word >> 5) & 7,
        secure: 0,
        wide: false,
    })?;
    Some((register, (word >> 12) & 15, word & (1 << 20) != 0))
}

impl Core {
    pub(super) fn mpu_register(
        &self,
        register: MpuRegister,
        value: Option<u32>,
    ) -> Result<u32, String> {
        let mpu = self.shared.mpu.as_ref().ok_or("Dynarmic has no MPU")?;
        if let Some(value) = value {
            let changed = mpu.borrow_mut().write(register, value)?;
            if changed {
                self.shared.mapping_changed();
            }
        }
        Ok(mpu.borrow().read(register))
    }

    pub(super) fn mpu_instruction(&self, word: u32, user: bool) -> Result<bool, String> {
        if self.shared.mpu.is_none() {
            return Ok(false);
        }
        let Some((register, gpr, read)) = transfer(word) else {
            return Ok(false);
        };
        if user || gpr == 15 {
            return Err(
                "MPU transfer requires privileged mode and a general-purpose register".into(),
            );
        }
        if read {
            let value = self.mpu_register(register, None)?;
            unsafe { ffi::dynarmic_set_reg(self.inst, gpr, value) };
        } else {
            let value = unsafe { ffi::dynarmic_get_reg(self.inst, gpr) };
            self.mpu_register(register, Some(value))?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_cpu::{Cpu, Exit, Model, Run};

    fn cp15(crn: u32, crm: u32, opc2: u32) -> arm::Coprocessor {
        arm::Coprocessor {
            cp: 15,
            opc1: 0,
            crn,
            crm,
            opc2,
            secure: 0,
            wide: false,
        }
    }

    #[test]
    fn guest_programs_a_region_and_a_store_takes_a_permission_abort() {
        let cpu = Core::new(Model::CortexR5F).unwrap();
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        cpu.set_armv7a_owner_exclusives(true).unwrap();
        cpu.coprocessor(cp15(6, 1, 2), Some(0x3f)).unwrap();
        cpu.coprocessor(cp15(6, 1, 4), Some(3 << 8)).unwrap();
        cpu.coprocessor(cp15(1, 0, 0), Some(1)).unwrap();
        for (reg, value) in [1u32, 0x2000, 9, 6 << 8].iter().enumerate() {
            cpu.write_register(arm::CORE[reg], &value.to_le_bytes())
                .unwrap();
        }
        for (offset, word) in [
            0xee06_0f12u32, // mcr p15, 0, r0, c6, c2, 0 (RGNR)
            0xee06_1f11,    // mcr p15, 0, r1, c6, c1, 0 (DRBAR)
            0xee06_2f51,    // mcr p15, 0, r2, c6, c1, 2 (DRSR)
            0xee06_3f91,    // mcr p15, 0, r3, c6, c1, 4 (DRACR)
            0xee16_5f91,    // mrc p15, 0, r5, c6, c1, 4
            0xe581_0000,    // str r0, [r1]
        ]
        .into_iter()
        .enumerate()
        {
            cpu.write_memory(0x1000 + offset as u64 * 4, &word.to_le_bytes())
                .unwrap();
        }
        cpu.set_loop_window(true).unwrap();
        let run = |begin, count| {
            cpu.run(Run {
                begin,
                until: None,
                watchdog_us: 0,
                instructions: Some(count),
            })
            .unwrap()
        };
        run(0x1000, 5);
        assert_eq!(cpu.instruction_progress(), Some(5));
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 5) }, 6 << 8);
        assert!(
            cpu.read_virtual(0x1ffc, Access::Write, &mut [0; 8])
                .is_err()
        );
        assert_eq!(cpu.read_virtual(0x2000, Access::Read, &mut [0; 4]), Ok(()));
        assert_eq!(cpu.read_virtual(0x2020, Access::Write, &mut [0; 4]), Ok(()));

        let snapshot = cpu.snapshot(None).unwrap();
        cpu.coprocessor(cp15(6, 1, 2), Some(0)).unwrap();
        assert_eq!(cpu.read_virtual(0x2000, Access::Write, &mut [0; 4]), Ok(()));
        let imported = cpu.import_snapshot(&snapshot.export().unwrap()).unwrap();
        cpu.restore(imported.as_ref()).unwrap();
        cpu.set_loop_window(true).unwrap();
        assert!(matches!(run(0x1014, 2), Exit::Stopped));
        assert_eq!(
            cpu.coprocessor(cp15(5, 0, 0), None).unwrap(),
            (1 << 11) | 0x0d
        );
        assert_eq!(cpu.coprocessor(cp15(6, 0, 0), None).unwrap(), 0x2000);
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 15) }, 0x10);
        let mut bytes = [1; 4];
        cpu.read_memory(0x2000, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 4]);
    }
}
