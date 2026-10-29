//! R-profile architectural execution mechanisms, independent of devices and PyO3.
//! Instruction encodings follow the pinned Cortex-R5 Unicorn translator.

use fujimod_emulator_cpu::{Cpu, Model, Register, arm};
use std::cell::Cell;

pub mod abort;
pub mod core;
pub mod tcm;
#[cfg(not(target_os = "emscripten"))]
pub mod worker;

thread_local! {
    static APPROXIMATE_GUEST_TICK: Cell<Option<u64>> = const { Cell::new(None) };
}

pub(crate) fn approximate_guest_tick() -> Option<u64> {
    APPROXIMATE_GUEST_TICK.with(Cell::get)
}

pub(crate) fn publish_guest_tick(tick: Option<u64>) {
    APPROXIMATE_GUEST_TICK.with(|current| current.set(tick));
}

pub(crate) struct GuestTickScope;
impl GuestTickScope {
    pub(crate) fn new() -> Self {
        publish_guest_tick(None);
        Self
    }
}
impl Drop for GuestTickScope {
    fn drop(&mut self) {
        publish_guest_tick(None);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrqEntry {
    pub pc: u32,
    pub cpsr: u32,
    pub vector: u32,
}

/// Architectural IRQ entry, separated from the controller's selection and
/// acknowledge protocol. Entry consumes no guest instruction or virtual tick.
pub fn enter_irq(cpu: &dyn Cpu) -> Result<Option<IrqEntry>, String> {
    if cpu.model() != Model::CortexR5F {
        return Err("R-profile IRQ entry requires a Cortex-R5F processor".into());
    }
    let read = |register| -> Result<u32, String> {
        let mut bytes = [0; 4];
        if cpu.read_register(register, &mut bytes)? != 4 {
            return Err("R-profile register width is not 32 bits".into());
        }
        Ok(u32::from_le_bytes(bytes))
    };
    let cpsr = read(arm::CPSR)?;
    if cpsr & 0x80 != 0 {
        return Ok(None);
    }
    let sctlr = cpu.coprocessor(
        arm::Coprocessor {
            cp: 15,
            opc1: 0,
            crn: 1,
            crm: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        },
        None,
    )? as u32;
    if sctlr & ((1 << 30) | (1 << 25)) != 0 {
        return Err("Thumb/big-endian exception entry is not implemented".into());
    }
    let pc = read(arm::PC)?;
    let vector: u32 = if sctlr & (1 << 13) != 0 {
        0xffff0018
    } else {
        0x18
    };
    let next = (cpsr & !(0x1f | 0x20 | 0x0600fc00 | 0x01000000 | 0x200)) | 0x12 | 0x80 | 0x100;
    cpu.write_register(arm::CPSR, &next.to_le_bytes())?;
    cpu.write_register(arm::SPSR, &cpsr.to_le_bytes())?;
    cpu.write_register(arm::CORE[14], &pc.wrapping_add(4).to_le_bytes())?;
    cpu.write_register(arm::PC, &vector.to_le_bytes())?;
    Ok(Some(IrqEntry { pc, cpsr, vector }))
}

/// Unicorn reports disabled ARM VFP operations as invalid instructions.
/// Deliver the architectural undefined exception so the guest grants access,
/// enables/restores its FP state and retries through its own vector.
pub fn enter_disabled_vfp(cpu: &dyn Cpu) -> Result<bool, String> {
    let read = |register| -> Result<u32, String> {
        let mut bytes = [0; 4];
        if cpu.read_register(register, &mut bytes)? != 4 {
            return Err("R-profile exception register width is not 32 bits".into());
        }
        Ok(u32::from_le_bytes(bytes))
    };
    let old = read(arm::CPSR)?;
    if old & 0x20 != 0 {
        return Ok(false);
    }
    let pc = read(arm::PC)?;
    let mut bytes = [0; 4];
    cpu.read_memory(pc as u64, &mut bytes)?;
    let word = u32::from_le_bytes(bytes);
    if word >> 28 == 15
        || !matches!((word >> 8) & 15, 10 | 11)
        || !matches!(word & 0x0e000000, 0x0c000000 | 0x0e000000)
        || !arm::condition_passes(word >> 28, old)
    {
        return Ok(false);
    }
    let access = read(Register::Arm32(arm::Register::Cpacr))?;
    let accessible = [20, 22].into_iter().all(|shift| {
        let value = access >> shift & 3;
        value == 3 || value == 1 && old & 31 != 0x10
    });
    if accessible && read(Register::Arm32(arm::Register::Fpexc))? & 0x40000000 != 0 {
        return Ok(false);
    }
    let sctlr = cpu.coprocessor(
        arm::Coprocessor {
            cp: 15,
            opc1: 0,
            crn: 1,
            crm: 0,
            opc2: 0,
            secure: 0,
            wide: false,
        },
        None,
    )? as u32;
    if sctlr & ((1 << 30) | (1 << 25)) != 0 {
        return Err("Thumb/big-endian undefined exception entry is not implemented".into());
    }
    let vector: u32 = if sctlr & (1 << 13) != 0 {
        0xffff0004
    } else {
        4
    };
    let next = (old & !(0x1f | 0x20 | 0x0600fc00 | 0x01000000 | 0x200)) | 0x1b | 0x80;
    cpu.write_register(arm::CPSR, &next.to_le_bytes())?;
    cpu.write_register(arm::SPSR, &old.to_le_bytes())?;
    cpu.write_register(arm::CORE[14], &pc.wrapping_add(4).to_le_bytes())?;
    cpu.write_register(arm::PC, &vector.to_le_bytes())?;
    Ok(true)
}

/// Predict only CPSR.I; the processor executes the instruction itself. Operand
/// readers belong to the CPU owner. The frame reader must be observational and
/// refuse MMIO, so an RFE prediction never performs a device access early.
pub fn predict_irq_mask(
    word: u32,
    size: u32,
    thumb: bool,
    mut register: impl FnMut(Register) -> Result<u32, String>,
    mut memory: impl FnMut(u32, &mut [u8; 4]) -> Result<(), String>,
) -> Result<Option<u32>, String> {
    let source = if thumb {
        thumb_irq_mask_source(word, size)
    } else {
        arm_irq_mask_source(word)
    };
    let Some(source) = source else {
        return Ok(None);
    };
    let cpsr = register(arm::CPSR)?;
    // Privileged control writes are ignored in USR. Unicorn retains its
    // instruction behavior for undefined exception returns from USR.
    if cpsr & 31 == 0x10 {
        return Ok(None);
    }
    let condition = if thumb {
        let it = ((cpsr >> 25) & 3) | ((cpsr >> 8) & 0xfc);
        if it != 0 { it >> 4 } else { 14 }
    } else {
        word >> 28
    };
    if condition < 14 && !arm::condition_passes(condition, cpsr) {
        return Ok(None);
    }
    let value = match source {
        IrqMaskSource::Register(reg) => {
            let value = register(arm::CORE[reg])?;
            if reg == 15 {
                value.wrapping_add(if thumb { 4 } else { 8 })
            } else {
                value
            }
        }
        IrqMaskSource::Immediate(value) => value,
        IrqMaskSource::SavedStatus => register(arm::SPSR)?,
        IrqMaskSource::ReturnFrame { base, offset } => {
            let address = register(arm::CORE[base])?.wrapping_add_signed(offset);
            let mut bytes = [0; 4];
            memory(address, &mut bytes)?;
            if cpsr & 0x200 != 0 {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            }
        }
    };
    Ok(Some(value & 0x80))
}

/// Sources of CPSR.I in the Cortex-R5 instruction set implemented by the
/// pinned Unicorn translator (translate.c and decode-{a32,t16,t32}.inc.c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqMaskSource {
    Register(usize),
    Immediate(u32),
    SavedStatus,
    ReturnFrame { base: usize, offset: i32 },
}

pub fn arm_irq_mask_source(word: u32) -> Option<IrqMaskSource> {
    if word >> 28 != 15 {
        if word & 0x0ff0fff0 == 0x0120f000 && word & (1 << 16) != 0 {
            return Some(IrqMaskSource::Register((word & 15) as usize));
        }
        if word & 0x0ff0f000 == 0x0320f000 && word & (1 << 16) != 0 {
            let immediate = (word & 255).rotate_right(((word >> 8) & 15) * 2);
            return Some(IrqMaskSource::Immediate(immediate));
        }
        // Unicorn implements ALU exception return for MOVS and SUBS to PC.
        // CMP/TST and SPSR writes must not be mistaken for CPSR writes.
        if matches!(word & 0x0df0f000, 0x01b0f000 | 0x0050f000)
            && (word & (1 << 25) != 0 || word & 0x90 != 0x90)
            || word & 0x0e508000 == 0x08508000
        {
            return Some(IrqMaskSource::SavedStatus);
        }
    } else {
        if word & 0xfff1fe20 == 0xf1000000 && word & (1 << 19) != 0 && word & (1 << 7) != 0 {
            return Some(IrqMaskSource::Immediate((word >> 11) & 0x80));
        }
        if word & 0xfe50ffff == 0xf8100a00 {
            let offset = match (word >> 23) & 3 {
                0 => 0,  // DA: PC at base-4, CPSR at base
                1 => 4,  // IA
                2 => -4, // DB
                _ => 8,  // IB
            };
            return Some(IrqMaskSource::ReturnFrame {
                base: ((word >> 16) & 15) as usize,
                offset,
            });
        }
    }
    None
}

pub fn thumb_irq_mask_source(word: u32, size: u32) -> Option<IrqMaskSource> {
    if size == 2 {
        return (word & 0xffe8 == 0xb660 && word & 2 != 0)
            .then_some(IrqMaskSource::Immediate((word << 3) & 0x80));
    }
    // Instruction bytes put the first halfword in the low bits; the pinned
    // T32 decoder numbers fields in first-halfword/high-bit order.
    let word = word.rotate_left(16);
    if word & 0xfff0f0ff == 0xf3808000 && word & (1 << 8) != 0 {
        return Some(IrqMaskSource::Register(((word >> 16) & 15) as usize));
    }
    if word & 0xfffff800 == 0xf3af8000 && word & (1 << 10) != 0 && word & (1 << 6) != 0 {
        return Some(IrqMaskSource::Immediate((word >> 2) & 0x80));
    }
    if word & 0xffffff00 == 0xf3de8f00 && word & 255 != 0 {
        return Some(IrqMaskSource::SavedStatus); // SUBS PC,LR,#imm (not ERET)
    }
    let offset = match word & 0xffd0ffff {
        0xe810c000 => -4, // RFEDB
        0xe990c000 => 4,  // RFEIA
        _ => return None,
    };
    Some(IrqMaskSource::ReturnFrame {
        base: ((word >> 16) & 15) as usize,
        offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn processor() -> crate::Core {
        crate::test_support::library()
            .open(Model::CortexR5F, 1024 * 1024)
            .unwrap()
    }

    fn scalar(cpu: &dyn Cpu, r: Register) -> u32 {
        let mut bytes = [0; 4];
        cpu.read_register(r, &mut bytes).unwrap();
        u32::from_le_bytes(bytes)
    }

    fn system_control(cpu: &dyn Cpu, value: Option<u64>) -> u64 {
        cpu.coprocessor(
            arm::Coprocessor {
                cp: 15,
                opc1: 0,
                crn: 1,
                crm: 0,
                opc2: 0,
                secure: 0,
                wide: false,
            },
            value,
        )
        .unwrap()
    }

    #[test]
    fn actual_irq_entry_and_guest_return_preserve_status_banks_and_vector_selection() {
        for high_vectors in [false, true] {
            let cpu = processor();
            cpu.map_zeroed(0, 0x4000, 7).unwrap();
            cpu.map_zeroed(0xffff0000, 0x1000, 7).unwrap();
            let vector = if high_vectors { 0xffff0018 } else { 0x18 };
            cpu.write_memory(vector, &0xe25ef004u32.to_le_bytes())
                .unwrap(); // subs pc,lr,#4
            let control = system_control(&cpu, None) & !((1 << 13) | (1 << 25) | (1 << 30));
            system_control(&cpu, Some(control | if high_vectors { 1 << 13 } else { 0 }));
            let cpsr = 0x60000253u32;
            cpu.write_register(arm::CPSR, &cpsr.to_le_bytes()).unwrap();
            cpu.write_register(arm::CORE[14], &0x3000u32.to_le_bytes())
                .unwrap();
            cpu.write_register(arm::PC, &0x1000u32.to_le_bytes())
                .unwrap();
            assert_eq!(
                enter_irq(&cpu).unwrap(),
                Some(IrqEntry {
                    pc: 0x1000,
                    cpsr,
                    vector: vector as u32
                })
            );
            assert_eq!(scalar(&cpu, arm::PC), vector as u32);
            assert_eq!(scalar(&cpu, arm::CPSR), 0x600001d2);
            assert_eq!(scalar(&cpu, arm::SPSR), cpsr);
            assert_eq!(scalar(&cpu, arm::CORE[14]), 0x1004);
            cpu.run(fujimod_emulator_cpu::Run {
                begin: vector,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
            assert_eq!(scalar(&cpu, arm::PC), 0x1000);
            assert_eq!(scalar(&cpu, arm::CPSR), cpsr);
            assert_eq!(scalar(&cpu, arm::CORE[14]), 0x3000);
        }
    }

    #[test]
    fn masked_or_unsupported_exception_entry_preserves_the_stopped_processor() {
        let cpu = processor();
        cpu.write_register(arm::CPSR, &0xd3u32.to_le_bytes())
            .unwrap();
        cpu.write_register(arm::PC, &0x1000u32.to_le_bytes())
            .unwrap();
        assert_eq!(enter_irq(&cpu).unwrap(), None);
        assert_eq!(scalar(&cpu, arm::CPSR), 0xd3);
        assert_eq!(scalar(&cpu, arm::PC), 0x1000);
        let control = system_control(&cpu, None);
        for bit in [25, 30] {
            cpu.write_register(arm::CPSR, &0x53u32.to_le_bytes())
                .unwrap();
            system_control(&cpu, Some(control | (1 << bit)));
            assert!(
                enter_irq(&cpu)
                    .unwrap_err()
                    .contains("exception entry is not implemented")
            );
            assert_eq!(scalar(&cpu, arm::CPSR), 0x53);
            assert_eq!(scalar(&cpu, arm::PC), 0x1000);
        }
    }

    #[test]
    fn prediction_respects_conditions_privilege_and_architectural_pc() {
        let predict = |word, cpsr, value| {
            predict_irq_mask(
                word,
                4,
                false,
                |r| Ok(if r == arm::CPSR { cpsr } else { value }),
                |_, _| panic!("unexpected frame operand"),
            )
        };
        assert_eq!(predict(0xe121f001, 0xd3, 0x53).unwrap(), Some(0));
        assert_eq!(predict(0x0121f001, 0xd3, 0x53).unwrap(), None);
        assert_eq!(predict(0xe121f001, 0xd0, 0x53).unwrap(), None);
        assert_eq!(predict(0xe121f00f, 0xd3, 0x20078).unwrap(), Some(0x80));
        assert_eq!(
            predict_irq_mask(
                0xe2800001,
                4,
                false,
                |_| panic!("ordinary instruction read a register"),
                |_, _| panic!("ordinary instruction read memory")
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn prediction_reads_status_frame_once_and_propagates_refusal() {
        for big_endian in [false, true] {
            let mut reads = 0;
            let result = predict_irq_mask(
                0xf8910a00,
                4,
                false,
                |r| {
                    Ok(if r == arm::CPSR {
                        0xd3 | if big_endian { 0x200 } else { 0 }
                    } else {
                        0x30000
                    })
                },
                |address, out| {
                    reads += 1;
                    assert_eq!(address, 0x30004);
                    *out = if big_endian {
                        0x53u32.to_be_bytes()
                    } else {
                        0x53u32.to_le_bytes()
                    };
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(result, Some(0));
            assert_eq!(reads, 1);
        }
        assert_eq!(
            predict_irq_mask(
                0xf8910a00,
                4,
                false,
                |_| Ok(0xd3),
                |_, _| Err("frame touches MMIO".into())
            )
            .unwrap_err(),
            "frame touches MMIO"
        );
    }

    #[test]
    fn mask_decode_distinguishes_control_writes_from_flags_and_saved_status() {
        assert_eq!(
            arm_irq_mask_source(0xe121f001),
            Some(IrqMaskSource::Register(1))
        );
        assert_eq!(
            arm_irq_mask_source(0xe321f053),
            Some(IrqMaskSource::Immediate(0x53))
        );
        assert_eq!(
            arm_irq_mask_source(0xe321f14c),
            Some(IrqMaskSource::Immediate(0x13))
        );
        for word in [0xe128f001, 0xe161f001, 0xe361f053, 0xf102001f, 0xf1080040] {
            assert_eq!(arm_irq_mask_source(word), None, "{word:#x}");
        }
        assert_eq!(
            arm_irq_mask_source(0xf1080080),
            Some(IrqMaskSource::Immediate(0))
        );
        assert_eq!(
            arm_irq_mask_source(0xf10c0080),
            Some(IrqMaskSource::Immediate(0x80))
        );
        for word in [0xe1b0f00e, 0xe25ef004, 0xe8d18000] {
            assert_eq!(arm_irq_mask_source(word), Some(IrqMaskSource::SavedStatus));
        }
    }

    #[test]
    fn return_frame_offsets_follow_architectural_addressing_modes() {
        for (word, offset) in [
            (0xf8110a00, 0),
            (0xf8910a00, 4),
            (0xf9110a00, -4),
            (0xf9b10a00, 8),
        ] {
            assert_eq!(
                arm_irq_mask_source(word),
                Some(IrqMaskSource::ReturnFrame { base: 1, offset })
            );
        }
        for (word, offset) in [(0xe991c000u32, 4), (0xe831c000, -4)] {
            assert_eq!(
                thumb_irq_mask_source(word.rotate_left(16), 4),
                Some(IrqMaskSource::ReturnFrame { base: 1, offset })
            );
        }
    }

    #[test]
    fn thumb_decode_uses_memory_halfword_order_and_excludes_eret() {
        assert_eq!(
            thumb_irq_mask_source(0xb662, 2),
            Some(IrqMaskSource::Immediate(0))
        );
        assert_eq!(
            thumb_irq_mask_source(0xb672, 2),
            Some(IrqMaskSource::Immediate(0x80))
        );
        for (word, source) in [
            (0xf3818100u32, IrqMaskSource::Register(1)),
            (0xf3af8440, IrqMaskSource::Immediate(0)),
            (0xf3af8640, IrqMaskSource::Immediate(0x80)),
            (0xf3de8f04, IrqMaskSource::SavedStatus),
        ] {
            assert_eq!(thumb_irq_mask_source(word.rotate_left(16), 4), Some(source));
        }
        assert_eq!(
            thumb_irq_mask_source(0xf3de8f00u32.rotate_left(16), 4),
            None
        );
    }
}
