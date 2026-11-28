//! Cortex-R5F memory-protection state that does not exist in the A32 JIT.
use super::{Core, Mapping, ffi};
use fujimod_emulator_cpu::{
    Access, arm,
    armv7r::{self, tcm},
};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

const REGIONS: usize = 16;

#[derive(Clone, Copy, Default)]
pub struct Config {
    pub tcm: tcm::Options,
    pub high_vectors: bool,
    pub cp14_identity: u32,
}

pub(super) struct Profile {
    pub mpu: RefCell<Mpu>,
    tcm: [tcm::Bank; 2],
    pub cp14_identity: Cell<u32>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Saved {
    mpu: Mpu,
    tcm: [SavedBank; 2],
    cp14_identity: u32,
}

#[derive(Serialize, Deserialize)]
struct SavedBank {
    register: u32,
    bytes: Vec<u8>,
}

impl Profile {
    pub fn new(config: Config) -> Result<Self, String> {
        Ok(Self {
            mpu: Default::default(),
            tcm: tcm::banks(config.tcm)?,
            cp14_identity: Cell::new(config.cp14_identity),
        })
    }

    pub fn tcm_region(&self, address: u64, bytes: usize) -> Option<(Mapping, usize)> {
        let end = address.checked_add(bytes as u64)?;
        self.tcm.iter().find_map(|bank| {
            let (base, size) = bank.extent()?;
            (base <= address && end <= base + size as u64).then(|| {
                (
                    Mapping::Backing {
                        bytes: size,
                        permissions: 7,
                        ptr: NonNull::new(bank.memory.as_ptr()).unwrap(),
                    },
                    (address - base) as usize,
                )
            })
        })
    }

    pub fn overlaps_tcm(&self, address: u64, bytes: usize) -> bool {
        self.tcm
            .iter()
            .filter_map(|bank| bank.extent())
            .any(|(base, size)| {
                base < address.saturating_add(bytes as u64) && address < base + size as u64
            })
    }

    pub fn capture(&self) -> Saved {
        Saved {
            mpu: self.mpu.borrow().clone(),
            cp14_identity: self.cp14_identity.get(),
            tcm: std::array::from_fn(|index| {
                let bank = &self.tcm[index];
                // The core is stopped, and each allocation contains every
                // byte of the bank regardless of its current window.
                let bytes =
                    unsafe { std::slice::from_raw_parts(bank.memory.as_ptr(), bank.memory.len()) }
                        .to_vec();
                SavedBank {
                    register: bank.register.get(),
                    bytes,
                }
            }),
        }
    }

    pub fn validate(
        &self,
        saved: &Saved,
        has_mmio: impl Fn(u64, usize) -> bool,
    ) -> Result<(), String> {
        if !saved.mpu.valid() {
            return Err("invalid saved MPU selector".into());
        }
        for (bank, saved) in self.tcm.iter().zip(&saved.tcm) {
            let length = bank.memory.len();
            if saved.bytes.len() != length
                || tcm::normalize(length, saved.register)? != saved.register
            {
                return Err("invalid saved TCM bank geometry".into());
            }
            check_tcm_mmio(saved.register, length, &has_mmio)?;
        }
        tcm::validate_disjoint(
            saved.tcm[0].bytes.len(),
            saved.tcm[0].register,
            saved.tcm[1].bytes.len(),
            saved.tcm[1].register,
        )
    }

    pub fn restore(&self, saved: &Saved) {
        self.mpu.borrow_mut().clone_from(&saved.mpu);
        self.cp14_identity.set(saved.cp14_identity);
        for (bank, saved) in self.tcm.iter().zip(&saved.tcm) {
            bank.register.set(saved.register);
            // Validation checked the length while the core was stopped.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    saved.bytes.as_ptr(),
                    bank.memory.as_ptr(),
                    saved.bytes.len(),
                );
            }
        }
    }
}

fn check_tcm_mmio(
    register: u32,
    length: usize,
    has_mmio: impl Fn(u64, usize) -> bool,
) -> Result<(), String> {
    if register & 1 != 0 && has_mmio(u64::from(register & 0xffff_f000), length) {
        return Err("TCM overlay of MMIO is unmodeled".into());
    }
    Ok(())
}

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

#[derive(Clone, Copy)]
pub(super) enum SystemRegister {
    Mpu(MpuRegister),
    Tcm(usize),
    Identity,
}

impl SystemRegister {
    pub fn decode(cp: arm::Coprocessor) -> Option<Self> {
        if let Some(reg) = MpuRegister::decode(cp) {
            return Some(Self::Mpu(reg));
        }
        if cp.wide || cp.opc1 != 0 {
            return None;
        }
        match (cp.cp, cp.crn, cp.crm, cp.opc2) {
            (15, 9, 1, 0 | 1) => Some(Self::Tcm(cp.opc2 as usize)),
            (14, 2, 0, 0) => Some(Self::Identity),
            _ => None,
        }
    }
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
        let check = |address| -> Result<(), u32> {
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

fn transfer(word: u32) -> Option<(SystemRegister, u32, bool)> {
    if word & 0x0f00_0010 != 0x0e00_0010 {
        return None;
    }
    let register = SystemRegister::decode(arm::Coprocessor {
        cp: (word >> 8) & 15,
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
    pub(super) fn rprofile_register(
        &self,
        register: SystemRegister,
        value: Option<u32>,
    ) -> Result<u32, String> {
        let profile = self
            .shared
            .rprofile
            .as_ref()
            .ok_or("Dynarmic has no R-profile")?;
        match register {
            SystemRegister::Mpu(register) => {
                if let Some(value) = value
                    && profile.mpu.borrow_mut().write(register, value)?
                {
                    self.shared.mapping_changed();
                }
                Ok(profile.mpu.borrow().read(register))
            }
            SystemRegister::Tcm(index) => {
                let bank = &profile.tcm[index];
                if let Some(value) = value {
                    let value = tcm::normalize(bank.memory.len(), value)?;
                    let other = &profile.tcm[1 - index];
                    tcm::validate_disjoint(
                        bank.memory.len(),
                        value,
                        other.memory.len(),
                        other.register.get(),
                    )?;
                    check_tcm_mmio(value, bank.memory.len(), |base, length| {
                        self.shared.has_mmio(base, length)
                    })?;
                    if bank.register.replace(value) != value {
                        self.shared.mapping_changed();
                    }
                }
                Ok(bank.register.get())
            }
            SystemRegister::Identity => {
                if value.is_some() {
                    return Err("CP14 processor identity is read-only".into());
                }
                Ok(profile.cp14_identity.get())
            }
        }
    }

    pub(super) fn rprofile_instruction(&self, word: u32, user: bool) -> Result<bool, String> {
        if self.shared.rprofile.is_none() {
            return Ok(false);
        }
        let Some((register, gpr, read)) = transfer(word) else {
            return Ok(false);
        };
        if user || gpr == 15 {
            return Err(
                "R-profile system transfer requires privileged mode and a general-purpose register"
                    .into(),
            );
        }
        if read {
            let value = self.rprofile_register(register, None)?;
            unsafe { ffi::dynarmic_set_reg(self.inst, gpr, value) };
        } else {
            let value = unsafe { ffi::dynarmic_get_reg(self.inst, gpr) };
            self.rprofile_register(register, Some(value))?;
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

    #[test]
    fn moving_atcm_preserves_private_code_and_reveals_the_underlying_ram() {
        let cpu = Core::new_rprofile(
            Config {
                cp14_identity: 0x4321,
                tcm: tcm::Options {
                    atcm: tcm::BankOptions {
                        bytes: 4096,
                        reset: 0x2000,
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();
        cpu.set_armv7a_owner_exclusives(true).unwrap();
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        cpu.write_memory(0x2000, &0x12345678u32.to_le_bytes())
            .unwrap();
        cpu.write_memory(0x1000, &0xee09_0f31u32.to_le_bytes())
            .unwrap(); // MCR ATCM.
        cpu.write_memory(0x1004, &0xee12_2e10u32.to_le_bytes())
            .unwrap(); // MRC CP14 identity.
        let run = |begin, count| {
            cpu.set_loop_window(true).unwrap();
            cpu.run(Run {
                begin,
                until: None,
                watchdog_us: 0,
                instructions: Some(count),
            })
            .unwrap();
        };
        let remap = |address: u32| {
            cpu.write_register(arm::CORE[0], &address.to_le_bytes())
                .unwrap();
            run(0x1000, 2);
        };
        remap(0x2001);
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 2) }, 0x4321);
        cpu.write_memory(0x2000, &0xe3a0_3007u32.to_le_bytes())
            .unwrap(); // mov r3, #7
        let saved = cpu.snapshot(None).unwrap().export().unwrap();
        remap(0x3001);
        run(0x3000, 1);
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 3) }, 7);
        let mut bytes = [0; 4];
        cpu.read_memory(0x2000, &mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes), 0x12345678);
        cpu.write_memory(0x3000, &0xe3a0_3009u32.to_le_bytes())
            .unwrap(); // mov r3, #9
        let imported = cpu.import_snapshot(&saved).unwrap();
        cpu.restore(imported.as_ref()).unwrap();
        run(0x2000, 1);
        assert_eq!(unsafe { ffi::dynarmic_get_reg(cpu.inst, 3) }, 7);
    }
}
