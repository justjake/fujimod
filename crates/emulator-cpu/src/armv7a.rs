//! ARMv7-A (AArch32, ARM state) architectural mechanisms, independent of a
//! host execution backend. Cluster identity, PMU and hint semantics belong to
//! the board; this module only names the instructions the board performs.
use super::{Access, Cpu, Register, arm, exclusive};

pub mod abort;
pub mod mmu;

/// Exception entries performed outside the guest's own instructions, for
/// comparing boards: IRQ, undefined (disabled VFP), data and prefetch abort.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Entries {
    pub irq: u64,
    pub undefined: u64,
    pub data_abort: u64,
    pub prefetch_abort: u64,
}
impl Entries {
    pub fn plus(self, other: Self) -> Self {
        Self {
            irq: self.irq + other.irq,
            undefined: self.undefined + other.undefined,
            data_abort: self.data_abort + other.data_abort,
            prefetch_abort: self.prefetch_abort + other.prefetch_abort,
        }
    }
}

/// Instructions the board performs itself: the MPIDR, CBAR and CCSIDR
/// identity reads, the PMU control and cycle counter (c9, c12/c13, op1 0),
/// and WFE/WFI/SEV.
/// The condition field is not decoded; the board evaluates it at entry.
/// MRC p15,0,Rt,c0,c0,5 (MPIDR) into a core register, any condition.
pub fn mpidr_read(word: u32) -> bool {
    word & 0x0fff_0fff == 0x0e10_0fb0 && (word >> 12) & 15 != 15
}

pub fn framework_instruction(word: u32) -> bool {
    let body = word & 0x0fff_ffff;
    if body & 0x0f00_0f10 == 0x0e00_0f10
        && (body >> 16) & 15 == 9
        && (body >> 21) & 7 == 0
        && matches!(body & 15, 12 | 13)
    {
        return true;
    }
    matches!(body & !0xf000, 0x0e10_0fb0 | 0x0e9f_0f10 | 0x0e30_0f10)
        || matches!(body, 0x0320_f002..=0x0320_f004)
}

/// ARM-state CLREX instruction.
pub const CLREX: u32 = 0xf57f_f01f;

/// ARM-state LDREX/STREX (any width) and CLREX.
pub fn exclusive_instruction(word: u32) -> bool {
    word & 0x0f90_0fff == 0x0190_0f9f || word & 0x0f90_0ff0 == 0x0180_0f90 || word == CLREX
}

/// Execute a decoded, condition-passing ARM exclusive against the cluster
/// monitor. The caller owns instruction accounting, interrupt boundaries,
/// and observation of ordinary stores while a reservation is live.
pub fn execute_exclusive(
    cpu: &dyn Cpu,
    word: u32,
    cpsr: u32,
    owner: exclusive::Owner,
    monitors: &exclusive::Shared,
) -> Result<(), String> {
    let scalar = |register| {
        let mut bytes = [0; 8];
        cpu.read_register(register, &mut bytes)?;
        Ok::<u64, String>(u64::from_le_bytes(bytes))
    };
    let set = |register, value: u64| cpu.write_register(register, &(value as u32).to_le_bytes());
    let mut monitors = monitors.lock().map_err(|_| "exclusive monitors poisoned")?;
    if word == CLREX {
        monitors.clear(owner);
        return Ok(());
    }
    if cpsr & 0x200 != 0 {
        return Err("big-endian ARMv7-A exclusives are unsupported".into());
    }
    let load = word & (1 << 20) != 0;
    let rn = ((word >> 16) & 15) as usize;
    let rd = ((word >> 12) & 15) as usize;
    let rt = (word & 15) as usize;
    let size = match (word >> 21) & 3 {
        0 => 4,
        1 => 8,
        2 => 1,
        _ => 2,
    };
    let pair = |r: usize| size == 8 && (r & 1 != 0 || r > 12);
    if rn == 15
        || rd == 15
        || (load && pair(rd))
        || (!load && (rt == 15 || rd == rn || rd == rt || pair(rt) || (size == 8 && rd == rt + 1)))
    {
        return Err(format!("unpredictable ARMv7-A exclusive {word:#010x}"));
    }
    let address = scalar(arm::CORE[rn])? as u32 as u64;
    if !address.is_multiple_of(size as u64) {
        return Err(format!("unaligned ARMv7-A exclusive at {address:#x}"));
    }
    let access = if load { Access::Read } else { Access::Write };
    let mapped = cpu.translate(address, access)?;
    let physical = cpu.exclusive_address(mapped, size, access)?;
    if load {
        let mut bytes = [0; 8];
        // This checks permissions and refuses MMIO without driving a device.
        cpu.read_virtual(address, access, &mut bytes[..size])?;
        monitors.load(owner, physical, size);
        let value = u64::from_le_bytes(bytes);
        set(arm::CORE[rd], value & 0xffff_ffff)?;
        if size == 8 {
            set(arm::CORE[rd + 1], value >> 32)?;
        }
    } else {
        let success = monitors.store(owner, physical, size);
        if success {
            let mut value = scalar(arm::CORE[rt])? & 0xffff_ffff;
            if size == 8 {
                value |= (scalar(arm::CORE[rt + 1])? & 0xffff_ffff) << 32;
            }
            cpu.write_memory(mapped, &value.to_le_bytes()[..size])?;
            monitors.written(Some((physical, size)))?;
        }
        set(arm::CORE[rd], u64::from(!success))?;
    }
    Ok(())
}

/// Data cache invalidate, clean, or clean-and-invalidate by set/way.
pub fn data_cache_set_way(word: u32) -> bool {
    word >> 28 != 15
        && word & 0x0fff_0ff0 == 0x0e07_0f50
        && matches!(word & 15, 6 | 10 | 14)
        && (word >> 12) & 15 != 15
}

/// Instructions after which a counted backend must end its block, so pending
/// interrupts and translation changes take effect at the next instruction:
/// CPSR writes and exception returns (`rprofile::arm_irq_mask_source` covers
/// the same encodings), supervisor calls and CP14/CP15 accesses other than
/// cache and TLB maintenance (CP15 c7/c8), which the board does not observe.
pub fn block_boundary(word: u32) -> bool {
    let cond = word >> 28;
    let coprocessor = (word >> 8) & 15;
    if cond != 15 {
        // MSR CPSR_c (register/immediate), MOVS/SUBS PC, LDM with ^ and SVC.
        if (word & 0x0ff0_fff0 == 0x0120_f000 || word & 0x0ff0_f000 == 0x0320_f000)
            && word & (1 << 16) != 0
            || matches!(word & 0x0df0_f000, 0x01b0_f000 | 0x0050_f000)
                && (word & (1 << 25) != 0 || word & 0x90 != 0x90)
            || word & 0x0e50_8000 == 0x0850_8000
            || word & 0x0f00_0000 == 0x0f00_0000
        {
            return true;
        }
    } else if word & 0xfff1_fe20 == 0xf100_0000 || word & 0xfe50_ffff == 0xf810_0a00 {
        return true; // CPS, RFE
    }
    // MCR/MRC, MCRR/MRRC, LDC/STC on CP14/CP15 (CP10/CP11 are VFP/NEON).
    let maintenance = word & 0x0f00_0f10 == 0x0e00_0f10 && matches!((word >> 16) & 15, 7 | 8);
    (word & 0x0f00_0010 == 0x0e00_0010 || word & 0x0e00_0000 == 0x0c00_0000)
        && coprocessor >= 14
        && !maintenance
}

/// Take an asserted IRQ at an instruction boundary, as the ARMv7-A board
/// profile does: LR_irq = PC + 4, vectors from SCTLR.V or VBAR. Returns false
/// when CPSR.I masks it. Thumb and big-endian exception entry are refused.
pub fn enter_irq(cpu: &dyn Cpu) -> Result<bool, String> {
    let read = |r| {
        let mut b = [0; 8];
        cpu.read_register(r, &mut b)?;
        Ok::<u32, String>(u64::from_le_bytes(b) as u32)
    };
    let write = |r, v: u32| cpu.write_register(r, &v.to_le_bytes());
    let cpsr = read(arm::CPSR)?;
    if cpsr & 0x80 != 0 {
        return Ok(false);
    }
    let cp = |crn| arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn,
        crm: 0,
        opc2: 0,
        secure: 0,
        wide: false,
    };
    let sctlr = cpu.coprocessor(cp(1), None)? as u32;
    if sctlr & ((1 << 30) | (1 << 25)) != 0 {
        return Err("Thumb/big-endian IRQ entry is unsupported".into());
    }
    let base = if sctlr & (1 << 13) != 0 {
        0xffff0000
    } else {
        cpu.coprocessor(cp(12), None)? as u32 & !31
    };
    let pc = read(Register::Pc)?;
    let next = (cpsr & !(0x1f | 0x20 | 0x0600fc00 | 0x01000000 | 0x200)) | 0x12 | 0x80 | 0x100;
    write(arm::CPSR, next)?;
    write(arm::SPSR, cpsr)?;
    write(arm::CORE[14], pc.wrapping_add(4))?;
    write(Register::Pc, base.wrapping_add(0x18))?;
    Ok(true)
}

/// ARM-state encodings targeting the VFP or Advanced SIMD unit.
pub fn is_vfp_instruction(word: u32) -> bool {
    let simd = word & 0xfe00_0000 == 0xf200_0000 || word & 0xff10_0000 == 0xf400_0000;
    simd || (word >> 28 != 15 && word & 0x0c00_0e00 == 0x0c00_0a00)
}

/// Take the undefined-instruction exception for a VFP/Advanced SIMD
/// instruction at PC while CPACR or FPEXC.EN disables it, as the hardware
/// does and the X-Processor Pro kernel relies on to enable its FP context
/// lazily. Covers conditional CP10/CP11 encodings and the unconditional
/// Advanced SIMD data-processing (0xf2/0xf3) and element/structure
/// load/store (0xf4, bit 20 clear) encodings. Returns false for any other
/// instruction, so the engine's refusal stands. Thumb entry is refused.
pub fn enter_disabled_vfp(cpu: &dyn Cpu) -> Result<bool, String> {
    let read = |r| {
        let mut b = [0; 8];
        cpu.read_register(r, &mut b)?;
        Ok::<u32, String>(u64::from_le_bytes(b) as u32)
    };
    let write = |r, v: u32| cpu.write_register(r, &v.to_le_bytes());
    let cpsr = read(arm::CPSR)?;
    if cpsr & 0x20 != 0 {
        return Ok(false);
    }
    let pc = read(Register::Pc)?;
    let mut bytes = [0; 4];
    cpu.read_virtual(pc.into(), super::Access::Fetch, &mut bytes)?;
    let word = u32::from_le_bytes(bytes);
    if !is_vfp_instruction(word) {
        return Ok(false);
    }
    let cpacr = read(Register::Arm32(arm::Register::Cpacr))?;
    let privileged = cpsr & 31 != 0x10;
    let accessible = [20, 22].into_iter().all(|shift| {
        let access = (cpacr >> shift) & 3;
        access == 3 || access == 1 && privileged
    });
    if accessible && read(Register::Arm32(arm::Register::Fpexc))? & 0x4000_0000 != 0 {
        return Ok(false);
    }
    let cp = |crn| arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn,
        crm: 0,
        opc2: 0,
        secure: 0,
        wide: false,
    };
    let sctlr = cpu.coprocessor(cp(1), None)? as u32;
    if sctlr & ((1 << 30) | (1 << 25)) != 0 {
        return Err("Thumb/big-endian undefined exception entry is unsupported".into());
    }
    let base = if sctlr & (1 << 13) != 0 {
        0xffff0000
    } else {
        cpu.coprocessor(cp(12), None)? as u32 & !31
    };
    let next = (cpsr & !(0x1f | 0x20 | 0x0600fc00 | 0x01000000 | 0x200)) | 0x1b | 0x80;
    write(arm::CPSR, next)?;
    write(arm::SPSR, cpsr)?;
    write(arm::CORE[14], pc.wrapping_add(4))?;
    write(Register::Pc, base.wrapping_add(4))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_instructions_match_the_board_profile() {
        for word in [
            0xee10_0fb0u32, // mrc p15, 0, r0, c0, c0, 5 (MPIDR)
            0xee9f_1f10,    // mrc p15, 4, r1, c15, c0, 0 (CBAR)
            0xee30_0f10,    // mrc p15, 1, r0, c0, c0, 0 (CCSIDR)
            0xee19_0f1c,    // mrc p15, 0, r0, c9, c12, 0 (PMCR)
            0xee09_0f3c,    // mcr p15, 0, r0, c9, c12, 1 (PMCNTENSET)
            0xee19_2f1d,    // mrc p15, 0, r2, c9, c13, 0 (PMCCNTR)
            0xe320_f003,    // wfi
            0x1320_f002,    // wfene
            0xe320_f004,    // sev
        ] {
            assert!(framework_instruction(word), "{word:#010x}");
        }
        for word in [
            0xee11_0f10u32, // mrc p15, 0, r0, c1, c0, 0 (SCTLR)
            0xee19_0f1e,    // mrc p15, 0, r0, c9, c14, 0 (PMUSERENR)
            0xe320_f000,    // nop
            0xe320_f001,    // yield
            0xe1a0_0000,    // mov r0, r0
        ] {
            assert!(!framework_instruction(word), "{word:#010x}");
        }
    }

    #[test]
    fn boundaries_cover_status_writes_returns_and_system_coprocessors() {
        for word in [
            0xe121_f000u32, // msr cpsr_c, r0
            0xe321_f0d3,    // msr cpsr_c, #0xd3
            0xe1b0_f00e,    // movs pc, lr
            0xe25e_f004,    // subs pc, lr, #4
            0xe8fd_9fff,    // ldmia sp!, {r0-r12, pc}^
            0xef00_0000,    // svc #0
            0xf108_0080,    // cpsie i
            0xf10c_0080,    // cpsid i
            0xf8bd_0a00,    // rfeia sp!
            0xee11_0f10,    // mrc p15 (SCTLR)
            0xec51_0f0e,    // mrrc p15 (CNTPCT)
        ] {
            assert!(block_boundary(word), "{word:#010x}");
        }
        for word in [
            0xe12f_ff1e, // bx lr
            0xe8bd_8000, // ldmia sp!, {pc}
            0xe14f_0000, // mrs r0, spsr
            0xe169_f000, // msr spsr_fc, r0
            0xe150_0001, // cmp r0, r1
            0xeeb0_0a40, // vmov.f32 s0, s0 (CP10)
            0xed90_0b00, // vldr d0, [r0] (CP11)
            0xe320_f003, // wfi (framework, not a boundary)
            0x0e07_bf5e, // mcreq p15, 0, r11, c7, c14, 2 (DCCISW)
            0xee07_0f3e, // mcr p15, 0, r0, c7, c14, 1 (DCCIMVAC)
            0xee08_0f17, // mcr p15, 0, r0, c8, c7, 0 (TLBIALL)
        ] {
            assert!(!block_boundary(word), "{word:#010x}");
        }
    }
}
