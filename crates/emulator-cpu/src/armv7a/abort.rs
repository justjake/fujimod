//! ARMv7-A short-descriptor abort diagnosis and architectural entry, from
//! the XP4 board (emulator-xp4-board/src/abort.rs). Unicorn reports guest
//! aborts without entering them or latching the fault status, so the status
//! is re-derived from architectural registers, the instruction's operands
//! and the page tables. This never maps a page, repairs a descriptor or
//! calls a device.
use super::mmu::Mmu;
use crate::{Access, Cpu, Register, arm};

struct Cluster<'a>(&'a dyn Cpu);
impl Cluster<'_> {
    fn reg(&self, _core: usize, r: Register) -> Result<u32, String> {
        let mut b = [0; 8];
        self.0.read_register(r, &mut b)?;
        Ok(u64::from_le_bytes(b) as u32)
    }
    fn cp(&self, _core: usize, crn: u32, crm: u32, opc1: u32, opc2: u32) -> Result<u64, String> {
        self.0.coprocessor(
            arm::Coprocessor {
                cp: 15,
                wide: false,
                secure: 0,
                crn,
                crm,
                opc1,
                opc2,
            },
            None,
        )
    }
    fn write_cp(&self, crn: u32, opc2: u32, value: u32) -> Result<(), String> {
        self.0.coprocessor(
            arm::Coprocessor {
                cp: 15,
                wide: false,
                secure: 0,
                crn,
                crm: 0,
                opc1: 0,
                opc2,
            },
            Some(u64::from(value)),
        )?;
        Ok(())
    }
    fn table_word(&self, address: u32) -> Result<u32, String> {
        let mut b = [0; 4];
        self.0
            .read_memory(u64::from(address), &mut b)
            .map_err(|e| format!("Abort page-table walk at {address:#x}: {e}"))?;
        Ok(u32::from_le_bytes(b))
    }

    fn word(&self, _core: usize, address: u64) -> Result<u32, String> {
        let mut b = [0; 4];
        self.0.read_virtual(address, Access::Fetch, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }
}

struct Transfer {
    address: u32,
    bytes: u32,
    access: Access,
    user: bool,
    alignment: u32,
    exclusive: bool,
}
fn operand(c: &Cluster, core: usize, register: usize, pc: u32) -> Result<u32, String> {
    if register == 15 {
        Ok(pc.wrapping_add(8))
    } else {
        c.reg(core, arm::CORE[register])
    }
}
fn transfer(c: &Cluster, core: usize, pc: u32, word: u32) -> Result<Transfer, String> {
    let rn = ((word >> 16) & 15) as usize;
    let base = operand(c, core, rn, pc)?;
    let pre = word & (1 << 24) != 0;
    let up = word & (1 << 23) != 0;
    let load = word & (1 << 20) != 0;
    let access = if load { Access::Read } else { Access::Write };
    let address = |offset: u32| {
        if !pre {
            base
        } else if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        }
    };
    let unprivileged = !pre && word & (1 << 21) != 0;
    let load_exclusive = word & 0x0f900fff == 0x01900f9f;
    let store_exclusive = word & 0x0f900ff0 == 0x01800f90;
    if load_exclusive || store_exclusive {
        let bytes = match (word >> 21) & 3 {
            0 => 4,
            1 => 8,
            2 => 1,
            _ => 2,
        };
        return Ok(Transfer {
            address: base,
            bytes,
            access: if load_exclusive {
                Access::Read
            } else {
                Access::Write
            },
            user: false,
            alignment: bytes,
            exclusive: true,
        });
    }
    if word & 0x0c000000 == 0x04000000 {
        let offset = if word & (1 << 25) == 0 {
            word & 0xfff
        } else {
            if word & 16 != 0 {
                return Err("Unsupported register-shifted ARM abort operand".into());
            }
            let value = operand(c, core, (word & 15) as usize, pc)?;
            let shift = (word >> 7) & 31;
            match (word >> 5) & 3 {
                0 => value << shift,
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
                        ((c.reg(core, arm::CPSR)? >> 29) & 1) << 31 | (value >> 1)
                    } else {
                        value.rotate_right(shift)
                    }
                }
            }
        };
        let bytes = if word & (1 << 22) != 0 { 1 } else { 4 };
        return Ok(Transfer {
            address: address(offset),
            bytes,
            access,
            user: unprivileged,
            alignment: bytes,
            exclusive: false,
        });
    }
    if word & 0x0e000090 == 0x00000090 && word & 0x60 != 0 {
        let offset = if word & (1 << 22) != 0 {
            ((word >> 4) & 0xf0) | (word & 15)
        } else {
            operand(c, core, (word & 15) as usize, pc)?
        };
        let (bytes, access) = match ((word >> 5) & 3, load) {
            (1, _) => (2, access),
            (2, true) => (1, Access::Read),
            (3, true) => (2, Access::Read),
            (2, false) => (8, Access::Read),
            (3, false) => (8, Access::Write),
            _ => unreachable!(),
        };
        return Ok(Transfer {
            address: address(offset),
            bytes,
            access,
            user: unprivileged,
            alignment: bytes.min(4),
            exclusive: false,
        });
    }
    if word & 0x0e000000 == 0x08000000 {
        let registers = word & 0xffff;
        let bytes = registers.count_ones() * 4;
        if bytes == 0 || rn == 15 || (load && registers & (1 << rn) != 0) {
            return Err("Unsupported/ambiguous ARM multiple-transfer abort operands".into());
        }
        let start = if up {
            base.wrapping_add(u32::from(pre) * 4)
        } else {
            base.wrapping_sub(bytes).wrapping_add(u32::from(!pre) * 4)
        };
        return Ok(Transfer {
            address: start,
            bytes,
            access,
            user: false,
            alignment: 4,
            exclusive: false,
        });
    }
    if word & 0x0e000000 == 0x0c000000 && matches!((word >> 8) & 15, 10 | 11) {
        let offset = (word & 255) * 4;
        let multiple = !pre || word & (1 << 21) != 0;
        if multiple {
            if offset == 0 || rn == 15 || !matches!((up, pre), (true, false) | (false, true)) {
                return Err("Unsupported VFP multiple-transfer abort operands".into());
            }
            return Ok(Transfer {
                address: if up { base } else { base.wrapping_sub(offset) },
                bytes: offset,
                access,
                user: false,
                alignment: 4,
                exclusive: false,
            });
        }
        return Ok(Transfer {
            address: address(offset),
            bytes: if word & 0x100 != 0 { 8 } else { 4 },
            access,
            user: false,
            alignment: 4,
            exclusive: false,
        });
    }
    Err(format!(
        "Unsupported ARM data-abort instruction {word:#010x}"
    ))
}

/// Enter the prefetch (`fetch`) or data abort for the instruction at PC that
/// the engine reported: latch IFSR/IFAR or DFSR/DFAR, then take the
/// exception to abort mode with IRQ and asynchronous aborts masked.
pub fn enter(cpu: &dyn Cpu, fetch: bool) -> Result<(), String> {
    let c = Cluster(cpu);
    let core = 0;
    let pc = c.reg(core, arm::PC)?;
    let sctlr = c.cp(core, 1, 0, 0, 0)? as u32;
    let (address, status, access) = if sctlr & 1 == 0 {
        // With the MMU off every data access is Strongly-ordered (ARMv7-A
        // ARM B3.2.1), so the engine's only data abort is an alignment
        // fault. Camera run 2026-11-10: an X-T3 hung before its LED on SD
        // card code whose unaligned loads ran with the MMU off.
        if fetch {
            return Err("Prefetch abort with the MMU off is unmodeled".into());
        }
        let transfer = transfer(&c, core, pc, c.word(core, u64::from(pc))?)?;
        if transfer.address & (transfer.alignment - 1) == 0 {
            return Err("Data abort with the MMU off is not an alignment fault".into());
        }
        (transfer.address, 1, transfer.access)
    } else if fetch {
        let mmu = Mmu::read(
            |crn, crm, opc2| c.cp(core, crn, crm, 0, opc2).map(|v| v as u32),
            c.reg(core, arm::CPSR)?,
        )?;
        let status = mmu
            .translate(pc, Access::Fetch, mmu.user, |a| c.table_word(a))?
            .err()
            .ok_or("Prefetch abort is not explained by the architectural page tables")?;
        (pc, status, Access::Fetch)
    } else {
        let mmu = Mmu::read(
            |crn, crm, opc2| c.cp(core, crn, crm, 0, opc2).map(|v| v as u32),
            c.reg(core, arm::CPSR)?,
        )?;
        let word = c.word(core, u64::from(pc))?;
        let transfer = transfer(&c, core, pc, word)?;
        if transfer.address & (transfer.alignment - 1) != 0 {
            if mmu.sctlr & 2 == 0 && !transfer.exclusive {
                return Err("Unaligned ARM data-abort diagnosis is unsupported".into());
            }
            (transfer.address, 1, transfer.access)
        } else {
            let mut found = None;
            let mut offset = 0;
            while offset < transfer.bytes {
                let address = transfer.address.wrapping_add(offset);
                if let Err(status) =
                    mmu.translate(address, transfer.access, mmu.user || transfer.user, |a| {
                        c.table_word(a)
                    })?
                {
                    found = Some((address, status, transfer.access));
                    break;
                }
                offset += (4096 - (address & 4095)).min(transfer.bytes - offset);
            }
            found.ok_or("Data abort is not explained by the architectural page tables")?
        }
    };
    enter_reported(cpu, address, status, access)
}

/// Latch a fault from a CPU backend that already knows the failed access.
pub fn enter_reported(
    cpu: &dyn Cpu,
    address: u32,
    status: u32,
    access: Access,
) -> Result<(), String> {
    let c = Cluster(cpu);
    let core = 0;
    let pc = c.reg(core, arm::PC)?;
    let sctlr = c.cp(core, 1, 0, 0, 0)? as u32;
    let fetch = access == Access::Fetch;
    c.write_cp(
        5,
        u32::from(fetch),
        status | if access == Access::Write { 1 << 11 } else { 0 },
    )?;
    c.write_cp(6, if fetch { 2 } else { 0 }, address)?;
    let old = c.reg(core, arm::CPSR)?;
    if old & 0x20 != 0 || sctlr & ((1 << 30) | (1 << 25)) != 0 {
        return Err("Unsupported Thumb/big-endian ARMv7-A exception entry".into());
    }
    let base = if sctlr & (1 << 13) != 0 {
        0xffff0000
    } else {
        c.cp(core, 12, 0, 0, 0)? as u32 & !31
    };
    let (offset, return_offset) = if fetch { (0x0c, 4) } else { (0x10, 8) };
    let new = (old & !(0x1f | 0x20 | 0x0600fc00 | 0x01000000 | 0x200)) | 0x17 | 0x180;
    let write = |r, v: u32| cpu.write_register(r, &v.to_le_bytes());
    write(arm::CPSR, new)?;
    write(arm::SPSR, old)?;
    write(arm::CORE[14], pc.wrapping_add(return_offset))?;
    write(Register::Pc, base + offset)
}
