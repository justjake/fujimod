//! Checked ARM PMSAv7 permission aborts. Fault entry never changes region
//! permissions or supplies overlay bytes; the guest handler owns the retry.
use fujimod_emulator_cpu::{Cpu, Register, arm};

fn reg(cpu: &dyn Cpu, register: Register) -> Result<u32, String> {
    let mut bytes = [0; 4];
    if cpu.read_register(register, &mut bytes)? != 4 {
        return Err("non-ARM32 abort register".into());
    }
    Ok(u32::from_le_bytes(bytes))
}
fn cp(cpu: &dyn Cpu, crn: u32, crm: u32, opc2: u32, value: Option<u32>) -> Result<u32, String> {
    cpu.coprocessor(
        arm::Coprocessor {
            cp: 15,
            opc1: 0,
            crn,
            crm,
            opc2,
            secure: 0,
            wide: false,
        },
        value.map(u64::from),
    )
    .map(|v| v as u32)
}
#[derive(Clone, Copy)]
enum Operation {
    Read,
    Write,
    Execute,
}
fn denied(cpu: &dyn Cpu, address: u32, status: u32, operation: Operation) -> Result<bool, String> {
    let selected = cp(cpu, 6, 2, 0, None)?;
    let count = cp(cpu, 0, 0, 4, None)? >> 8 & 255;
    if !(1..=16).contains(&count) {
        return Err("unsupported PMSAv7 region count".into());
    }
    let result = (|| {
        for region in (0..count).rev() {
            cp(cpu, 6, 2, 0, Some(region))?;
            let base = cp(cpu, 6, 1, 0, None)? as u64;
            let size = cp(cpu, 6, 1, 2, None)?;
            let access = cp(cpu, 6, 1, 4, None)?;
            if size & 1 == 0 {
                continue;
            }
            let exponent = size >> 1 & 31;
            if exponent < 4 {
                return Err("invalid enabled PMSAv7 region size".into());
            }
            let length = 1u64 << (exponent + 1);
            if base & (length - 1) != 0 {
                return Err("misaligned PMSAv7 region base".into());
            }
            if u64::from(address) < base || u64::from(address) >= base + length {
                continue;
            }
            let subregion = (u64::from(address) - base) / (length / 8);
            if length >= 256 && size & (1 << (8 + subregion)) != 0 {
                continue;
            }
            let ap = access >> 8 & 7;
            if !matches!(ap, 0 | 1 | 2 | 3 | 5 | 6) {
                return Err("reserved PMSAv7 access permissions".into());
            }
            let user = status & 31 == 0x10;
            return Ok(match operation {
                Operation::Write => ap == 0 || matches!(ap, 5 | 6) || user && matches!(ap, 1 | 2),
                Operation::Read | Operation::Execute => {
                    ap == 0
                        || user && matches!(ap, 1 | 5)
                        || matches!(operation, Operation::Execute) && access & (1 << 12) != 0
                }
            });
        }
        Ok(false)
    })();
    let restore = cp(cpu, 6, 2, 0, Some(selected));
    restore?;
    result
}
fn transfer(cpu: &dyn Cpu, pc: u32, status: u32) -> Result<(u32, u32, bool), String> {
    let mut bytes = [0; 4];
    cpu.read_memory(pc as u64, &mut bytes)?;
    let word = u32::from_le_bytes(bytes);
    if word >> 28 == 15
        || word & 0x0c000000 != 0x04000000
        || word & (1 << 24) == 0 && word & (1 << 21) != 0
    {
        return Err("PMSAv7 data abort requires an ARM single data transfer".into());
    }
    let operand = |n: u32| {
        if n == 15 {
            Ok(pc.wrapping_add(8))
        } else {
            reg(cpu, arm::CORE[n as usize])
        }
    };
    let mut address = operand(word >> 16 & 15)?;
    if word & (1 << 24) != 0 {
        let offset = if word & (1 << 25) == 0 {
            word & 0xfff
        } else {
            if word & 16 != 0 {
                return Err("unsupported PMSAv7 transfer register shift".into());
            }
            let value = operand(word & 15)?;
            let shift = word >> 7 & 31;
            match word >> 5 & 3 {
                0 => value.wrapping_shl(shift),
                1 => {
                    if shift == 0 {
                        0
                    } else {
                        value >> shift
                    }
                }
                2 => ((value as i32) >> if shift == 0 { 31 } else { shift }) as u32,
                _ => {
                    if shift == 0 {
                        (status >> 29 & 1) << 31 | value >> 1
                    } else {
                        value.rotate_right(shift)
                    }
                }
            }
        };
        address = if word & (1 << 23) != 0 {
            address.wrapping_add(offset)
        } else {
            address.wrapping_sub(offset)
        };
    }
    Ok((
        address,
        if word & (1 << 22) != 0 { 1 } else { 4 },
        word & (1 << 20) == 0,
    ))
}
pub fn enter(cpu: &dyn Cpu, exception: arm::Exception) -> Result<(), String> {
    let old = reg(cpu, arm::CPSR)?;
    let pc = reg(cpu, arm::PC)?;
    let sctlr = cp(cpu, 1, 0, 0, None)?;
    if pc & 3 != 0
        || old & (0x20 | 0x01000000 | 0x200) != 0
        || !matches!(old & 31, 0x10 | 0x13 | 0x1f)
        || sctlr & 1 == 0
        || sctlr & ((1 << 30) | (1 << 25)) != 0
    {
        return Err("abort is outside the supported ARM/nonsecure PMSAv7 contract".into());
    }
    let data = match exception {
        arm::Exception::PrefetchAbort => false,
        arm::Exception::DataAbort => true,
        _ => return Err("unsupported PMSAv7 exception".into()),
    };
    let (address, size, write) = if data {
        transfer(cpu, pc, old)?
    } else {
        (pc, 4, false)
    };
    let protection = if !data {
        4
    } else if write {
        2
    } else {
        1
    };
    if address % size != 0
        || !cpu.memory_regions()?.iter().any(|r| {
            r.begin <= u64::from(address)
                && u64::from(address) + u64::from(size) - 1 <= r.end
                && r.permissions & protection != 0
        })
    {
        return Err("PMSAv7 abort is not backed by an aligned accessible host mapping".into());
    }
    if !denied(
        cpu,
        address,
        old,
        if !data {
            Operation::Execute
        } else if write {
            Operation::Write
        } else {
            Operation::Read
        },
    )? {
        return Err("native abort is not explained by an actual MPU permission denial".into());
    }
    cp(
        cpu,
        5,
        0,
        u32::from(!data),
        Some(0xd | (u32::from(write) << 11)),
    )?;
    cp(cpu, 6, 0, if data { 0 } else { 2 }, Some(address))?;
    let vector: u32 = if sctlr & (1 << 13) != 0 {
        0xffff0000
    } else {
        0
    };
    let next = (old & !(0x1f | 0x20 | 0x0600fc00 | 0x01000000 | 0x200)) | 0x17 | 0x180;
    cpu.write_register(arm::CPSR, &next.to_le_bytes())?;
    cpu.write_register(arm::SPSR, &old.to_le_bytes())?;
    cpu.write_register(
        arm::CORE[14],
        &pc.wrapping_add(if data { 8 } else { 4 }).to_le_bytes(),
    )?;
    cpu.write_register(
        arm::PC,
        &(vector + if data { 0x10 } else { 0xc }).to_le_bytes(),
    )?;
    Ok(())
}
