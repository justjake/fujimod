//! Privileged ARM state kept outside Dynarmic's application register file.
use super::{Core, ffi};
use fujimod_emulator_cpu::{Access, Cpu, arm};

const MODE: u32 = 0x1f;
const USER: u32 = 0x10;
const SYSTEM: u32 = 0x1f;

fn bank(cpsr: u32) -> Result<usize, String> {
    match cpsr & MODE {
        USER | SYSTEM => Ok(0),
        0x11 => Ok(1), // FIQ
        0x12 => Ok(2), // IRQ
        0x13 => Ok(3), // Supervisor
        0x16 => Ok(4), // Monitor
        0x17 => Ok(5), // Abort
        0x1b => Ok(6), // Undefined
        mode => Err(format!("Dynarmic does not support ARM mode {mode:#04x}")),
    }
}

#[derive(Clone, Default)]
pub(super) struct Banks {
    /// R13/R14 are banked by mode. User and System share a bank.
    sp_lr: [[u32; 2]; 7],
    /// FIQ banks R8–R12; all other modes share the other copy.
    high: [[u32; 5]; 2],
    saved_psr: [u32; 7],
}

impl Core {
    fn reg(&self, r: u32) -> u32 {
        unsafe { ffi::dynarmic_get_reg(self.inst, r) }
    }

    fn set_reg(&self, r: u32, value: u32) {
        unsafe { ffi::dynarmic_set_reg(self.inst, r, value) };
    }

    pub(super) fn set_cpsr(&self, value: u32) -> Result<(), String> {
        let old = self.cpsr();
        let previous = bank(old)?;
        let next = bank(value)?;
        if previous != next {
            let mut banks = self.banks.borrow_mut();
            banks.sp_lr[previous] = [self.reg(13), self.reg(14)];
            let [sp, lr] = banks.sp_lr[next];
            self.set_reg(13, sp);
            self.set_reg(14, lr);
            let previous_fiq = usize::from(previous == 1);
            let next_fiq = usize::from(next == 1);
            if previous_fiq != next_fiq {
                for index in 0..5 {
                    let r = 8 + index as u32;
                    banks.high[previous_fiq][index] = self.reg(r);
                    self.set_reg(r, banks.high[next_fiq][index]);
                }
            }
        }
        unsafe { ffi::dynarmic_set_cpsr(self.inst, value) };
        Ok(())
    }

    pub(super) fn set_fpexc(&self, value: u32) -> Result<(), String> {
        if value & !0x4000_0000 != 0 {
            return Err("Dynarmic only supports the FPEXC enable bit".into());
        }
        self.fpexc.set(value);
        Ok(())
    }

    fn vfp_permitted(&self) -> bool {
        let cpacr = unsafe { ffi::dynarmic_get_cp15(self.inst, 0, 1, 0, 2) };
        let privileged = self.cpsr() & MODE != USER;
        [20, 22].into_iter().all(|shift| {
            let access = (cpacr >> shift) & 3;
            access == 3 || (access == 1 && privileged)
        })
    }

    pub(super) fn vfp_accessible(&self) -> bool {
        self.fpexc.get() != 0 && self.vfp_permitted()
    }

    fn cpsr(&self) -> u32 {
        unsafe { ffi::dynarmic_get_cpsr(self.inst) }
    }

    fn spsr_bank(&self) -> Result<usize, String> {
        let bank = bank(self.cpsr())?;
        if bank == 0 {
            return Err("User and System ARM modes do not have an SPSR".into());
        }
        Ok(bank)
    }

    pub(super) fn spsr(&self) -> Result<u32, String> {
        Ok(self.banks.borrow().saved_psr[self.spsr_bank()?])
    }

    pub(super) fn set_spsr(&self, value: u32) -> Result<(), String> {
        let bank = self.spsr_bank()?;
        self.banks.borrow_mut().saved_psr[bank] = value;
        Ok(())
    }

    /// Handle status operations before the JIT translates them. A handled
    /// operation returns the next PC, including an exception return's target.
    pub(super) fn status_instruction(&self, pc: u32, word: u32) -> Result<Option<u32>, String> {
        let cpsr = self.cpsr();
        let user = cpsr & MODE == USER;
        let condition = word >> 28;
        let body = word & 0x0fff_ffff;
        let next = pc.wrapping_add(4);
        if condition == 15 {
            if word & 0xfff1_fe20 == 0xf100_0000 {
                if !user {
                    let imod = (word >> 18) & 3;
                    let flags = word & 0x1c0;
                    let change_mode = word & (1 << 17) != 0;
                    if imod == 1
                        || (imod == 0) != (flags == 0)
                        || (!change_mode && word & MODE != 0)
                    {
                        return Err("unpredictable ARM CPS operands".into());
                    }
                    let mut value = match imod {
                        2 => cpsr & !flags,
                        3 => cpsr | flags,
                        _ => cpsr,
                    };
                    if change_mode {
                        value = (value & !MODE) | (word & MODE);
                    }
                    self.set_cpsr(value)?;
                }
                return Ok(Some(next));
            }
            if word & 0xfe50_ffff == 0xf810_0a00 || word & 0xfe5f_ffe0 == 0xf84d_0500 {
                return Err("Dynarmic does not implement ARM RFE or SRS".into());
            }
            return Ok(None);
        }
        if !arm::condition_passes(condition, cpsr) {
            return Ok(None);
        }
        let vfp_status = body & 0x0fff_0fff;
        if matches!(vfp_status, 0x0ee8_0a10 | 0x0ef8_0a10) {
            let rd = (word >> 12) & 15;
            if rd == 15 || user || !self.vfp_permitted() {
                return Err("Dynarmic FPEXC access requires privileged VFP access".into());
            }
            if vfp_status == 0x0ee8_0a10 {
                self.set_fpexc(self.reg(rd))?;
            } else {
                self.set_reg(rd, self.fpexc.get());
            }
            return Ok(Some(next));
        }
        let saved = word & (1 << 22) != 0;
        if body & 0x0fbf_0fff == 0x010f_0000 {
            let rd = (word >> 12) & 15;
            if rd == 15 {
                return Err("unpredictable ARM MRS destination".into());
            }
            let value = if saved {
                self.spsr()?
            } else {
                cpsr & !0x0700_fc20
            };
            self.set_reg(rd, value);
            return Ok(Some(next));
        }
        let fields = (word >> 16) & 15;
        if fields != 0 && (body & 0x0fb0_fff0 == 0x0120_f000 || body & 0x0fb0_f000 == 0x0320_f000) {
            let operand = if word & (1 << 25) != 0 {
                (word & 255).rotate_right(2 * ((word >> 8) & 15))
            } else {
                let rn = word & 15;
                if rn == 15 {
                    return Err("unpredictable ARM MSR source".into());
                }
                self.reg(rn)
            };
            let mut mask = 0;
            for byte in 0..4 {
                if fields & (1 << byte) != 0 {
                    mask |= 0xff << (8 * byte);
                }
            }
            if saved {
                let old = self.spsr()?;
                self.set_spsr((old & !mask) | (operand & mask))?;
            } else {
                // Only ARMv7-A defined, non-execution-state CPSR bits are writable.
                mask &= if user { 0xf80f_0000 } else { 0xf80f_03df };
                self.set_cpsr((cpsr & !mask) | (operand & mask))?;
            }
            return Ok(Some(next));
        }
        let restore_flags = word & 0x0c10_f000 == 0x0010_f000
            && (word & (1 << 25) != 0 || word & 0x90 != 0x90)
            && !matches!((word >> 21) & 15, 8..=11);
        if restore_flags {
            let target = if body == 0x01b0_f00e {
                self.reg(14) // MOVS PC, LR
            } else if body & 0x0fff_f000 == 0x025e_f000 {
                let offset = (word & 255).rotate_right(2 * ((word >> 8) & 15));
                self.reg(14).wrapping_sub(offset) // SUBS PC, LR, #imm
            } else {
                return Err("Dynarmic does not implement this ARM ALU exception return".into());
            };
            let status = self.spsr()?;
            self.set_cpsr(status)?;
            return Ok(Some(target & if status & 0x20 == 0 { !3 } else { !1 }));
        }
        if word & 0x0e40_0000 == 0x0840_0000 {
            if word & 0x0010_8000 == 0x0010_8000 {
                return self.load_exception_frame(word).map(Some);
            }
            return Err("Dynarmic does not implement ARM user-bank multiple transfers".into());
        }
        Ok(None)
    }

    fn load_exception_frame(&self, word: u32) -> Result<u32, String> {
        let status = self.spsr()?;
        bank(status)?;
        let registers = word & 0xffff;
        let rn = (word >> 16) & 15;
        let writeback = word & (1 << 21) != 0;
        if rn == 15 || (writeback && registers & (1 << rn) != 0) {
            return Err("unpredictable ARM LDM exception return operands".into());
        }
        let base = self.reg(rn);
        if !base.is_multiple_of(4) || self.cpsr() & 0x200 != 0 {
            return Err(
                "Dynarmic LDM exception return requires an aligned little-endian frame".into(),
            );
        }
        let bytes = registers.count_ones() * 4;
        let before = word & (1 << 24) != 0;
        let (mut address, updated_base) = if word & (1 << 23) != 0 {
            (
                base.wrapping_add(u32::from(before) * 4),
                base.wrapping_add(bytes),
            )
        } else {
            let updated = base.wrapping_sub(bytes);
            (updated.wrapping_add(u32::from(!before) * 4), updated)
        };
        let mut values = [0; 16];
        for (index, value) in values.iter_mut().enumerate() {
            if registers & (1 << index) != 0 {
                let mut bytes = [0; 4];
                self.read_virtual(address.into(), Access::Read, &mut bytes)?;
                *value = u32::from_le_bytes(bytes);
                address = address.wrapping_add(4);
            }
        }
        for (index, value) in values[..15].iter().enumerate() {
            if registers & (1 << index) != 0 {
                self.set_reg(index as u32, *value);
            }
        }
        if writeback {
            self.set_reg(rn, updated_base);
        }
        self.set_cpsr(status)?;
        Ok(values[15] & if status & 0x20 == 0 { !3 } else { !1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_cpu::Model;

    #[test]
    fn arm_status_instructions_switch_banks_and_return_from_irq() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.set_reg(13, 0x1000);
        cpu.set_reg(0, 0xd2);
        assert_eq!(
            cpu.status_instruction(0x2000, 0xe121_f000).unwrap(),
            Some(0x2004)
        );
        assert_eq!(cpu.cpsr(), 0xd2);
        assert_eq!(cpu.reg(13), 0);

        cpu.set_reg(13, 0x3000);
        cpu.set_spsr(0xd3).unwrap();
        cpu.set_reg(14, 0x4004);
        assert_eq!(
            cpu.status_instruction(0x2004, 0xe14f_1000).unwrap(),
            Some(0x2008)
        );
        assert_eq!(cpu.reg(1), 0xd3);
        assert_eq!(
            cpu.status_instruction(0x2008, 0xe25e_f004).unwrap(),
            Some(0x4000)
        );
        assert_eq!(cpu.cpsr(), 0xd3);
        assert_eq!(cpu.reg(13), 0x1000);

        cpu.write_register(arm::CPSR, &0xd2u32.to_le_bytes())
            .unwrap();
        assert_eq!(cpu.reg(13), 0x3000);
        assert_eq!(cpu.spsr().unwrap(), 0xd3);
    }

    #[test]
    fn ldm_return_restores_flags_and_writes_back_the_exception_stack() {
        let cpu = Core::new(Model::CortexA7).unwrap();
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        cpu.set_reg(13, 0x1100);
        cpu.set_spsr(0x6000_005f).unwrap();
        cpu.write_memory(0x1100, &0x1234_u32.to_le_bytes()).unwrap();
        cpu.write_memory(0x1104, &0x4200_u32.to_le_bytes()).unwrap();

        // ldmia sp!, {r0, pc}^
        assert_eq!(
            cpu.status_instruction(0x2000, 0xe8fd_8001).unwrap(),
            Some(0x4200)
        );
        assert_eq!(cpu.cpsr(), 0x6000_005f);
        assert_eq!(cpu.reg(0), 0x1234);
        assert_eq!(cpu.reg(13), 0);
        cpu.set_cpsr(0xd3).unwrap();
        assert_eq!(cpu.reg(13), 0x1108);
    }
}
