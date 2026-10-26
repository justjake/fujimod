//! ARMv7-A short-descriptor abort diagnosis and architectural entry.
//! Only architectural registers, instruction operands and page tables are
//! inputs. This never maps a page, repairs a descriptor or calls a device.
use super::{Cluster, ExceptionEntry};
use fujimod_emulator_cpu::{Access, arm};

struct Mmu {
    sctlr: u32,
    ttbcr: u32,
    ttbr0: u32,
    ttbr1: u32,
    dacr: u32,
    fcse: u32,
    user: bool,
}
impl Mmu {
    fn read(c: &Cluster, core: usize) -> Result<Self, String> {
        let sctlr = c.cp(core, 1, 0, 0, 0)? as u32;
        let ttbcr = c.cp(core, 2, 0, 0, 2)? as u32;
        if sctlr & 1 == 0 || sctlr & (1 << 23) == 0 || ttbcr & (1 << 31) != 0 {
            return Err("Abort diagnosis requires the ARMv7 short-descriptor MMU".into());
        }
        if sctlr & (1 << 25) != 0 {
            return Err("Big-endian ARM page-table walks are unsupported".into());
        }
        Ok(Self {
            sctlr,
            ttbcr,
            ttbr0: c.cp(core, 2, 0, 0, 0)? as u32,
            ttbr1: c.cp(core, 2, 0, 0, 1)? as u32,
            dacr: c.cp(core, 3, 0, 0, 0)? as u32,
            fcse: c.cp(core, 13, 0, 0, 0)? as u32,
            user: c.reg(core, arm::CPSR)? & 31 == 0x10,
        })
    }
    fn table_word(c: &Cluster, address: u32) -> Result<u32, String> {
        if u64::from(address) + 4 > c.ram.len() as u64 {
            return Err("Abort page-table walk outside physical RAM".into());
        }
        Ok(unsafe { std::ptr::read_unaligned(c.ram.as_ptr().add(address as usize).cast()) })
    }
    /// ARM ARM short-descriptor translation/domain/access-flag/permission
    /// fault encodings, including the domain in FSR[7:4]. None means this
    /// walk permits the access, so it cannot explain the reported abort.
    fn status(
        &self,
        c: &Cluster,
        address: u32,
        access: Access,
        user: bool,
    ) -> Result<Option<u32>, String> {
        let address = if address < 0x02000000 {
            address.wrapping_add(self.fcse)
        } else {
            address
        };
        let n = self.ttbcr & 7;
        let upper = n != 0 && address >> (32 - n) != 0;
        let (table, disabled) = if upper {
            (self.ttbr1 & 0xffffc000, self.ttbcr & (1 << 5) != 0)
        } else {
            (self.ttbr0 & (!0u32 << (14 - n)), self.ttbcr & (1 << 4) != 0)
        };
        if disabled {
            return Ok(Some(5));
        }
        let first = Self::table_word(c, table | ((address >> 18) & 0x3ffc))?;
        if first & 3 == 0 {
            return Ok(Some(5));
        }
        let page = first & 3 == 1;
        let domain = if !page && first & (1 << 18) != 0 {
            0
        } else {
            (first >> 5) & 15
        };
        let domain_access = (self.dacr >> (2 * domain)) & 3;
        let encode = |section, page_code| (if page { page_code } else { section }) | (domain << 4);
        if matches!(domain_access, 0 | 2) {
            return Ok(Some(encode(9, 11)));
        }
        let (ap, xn, pxn) = if page {
            let second = Self::table_word(c, (first & 0xfffffc00) | ((address >> 10) & 0x3fc))?;
            if second & 3 == 0 {
                return Ok(Some(encode(5, 7)));
            }
            let xn = if second & 3 == 1 {
                second & (1 << 15) != 0
            } else {
                second & 1 != 0
            };
            (
                ((second >> 4) & 3) | ((second >> 7) & 4),
                xn,
                first & 4 != 0,
            )
        } else {
            (
                ((first >> 10) & 3) | ((first >> 13) & 4),
                first & 16 != 0,
                first & 1 != 0,
            )
        };
        // Manager domains bypass access-flag and permission checks.
        if domain_access == 3 {
            return Ok(None);
        }
        if access == Access::Fetch && (xn || (pxn && !user)) {
            return Ok(Some(encode(13, 15)));
        }
        let afe = self.sctlr & (1 << 29) != 0;
        if afe && ap & 1 == 0 {
            return Ok(Some(encode(3, 6)));
        }
        let (read, write) = match if afe { ap | 1 } else { ap } {
            1 => (!user, !user),
            2 => (true, !user),
            3 => (true, true),
            5 => (!user, false),
            6 | 7 => (true, false),
            _ => (false, false),
        };
        let allowed = match access {
            Access::Read | Access::Fetch => read,
            Access::Write => write,
        };
        Ok((!allowed).then(|| encode(13, 15)))
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

impl Cluster {
    fn write_cp(&self, core: usize, crn: u32, opc2: u32, value: u32) -> Result<(), String> {
        self.cpus[core].coprocessor(
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
    fn abort_entry(
        &self,
        core: usize,
        pc: u32,
        address: u32,
        status: u32,
        access: Access,
    ) -> Result<(), String> {
        let fetch = access == Access::Fetch;
        self.write_cp(
            core,
            5,
            u32::from(fetch),
            status | if access == Access::Write { 1 << 11 } else { 0 },
        )?;
        self.write_cp(core, 6, if fetch { 2 } else { 0 }, address)?;
        self.enter_exception(
            core,
            if fetch {
                ExceptionEntry::PrefetchAbort
            } else {
                ExceptionEntry::DataAbort
            },
            pc,
        )?;
        unsafe {
            if fetch {
                (&mut (*self.state.get()).prefetch_aborts)[core] += 1;
            } else {
                (&mut (*self.state.get()).data_aborts)[core] += 1;
            }
        }
        Ok(())
    }
    pub(super) fn deliver_abort(&self, core: usize, fetch: bool) -> Result<(), String> {
        let pc = self.reg(core, arm::PC)?;
        let mmu = Mmu::read(self, core)?;
        if fetch {
            let status = mmu
                .status(self, pc, Access::Fetch, mmu.user)?
                .ok_or("Prefetch abort is not explained by the architectural page tables")?;
            return self.abort_entry(core, pc, pc, status, Access::Fetch);
        }
        let word = self.word(core, u64::from(pc))?;
        let transfer = transfer(self, core, pc, word)?;
        if transfer.address & (transfer.alignment - 1) != 0 {
            if mmu.sctlr & 2 != 0 || transfer.exclusive {
                return self.abort_entry(core, pc, transfer.address, 1, transfer.access);
            }
            return Err("Unaligned ARM data-abort diagnosis is unsupported".into());
        }
        let mut offset = 0;
        while offset < transfer.bytes {
            let address = transfer.address.wrapping_add(offset);
            if let Some(status) =
                mmu.status(self, address, transfer.access, mmu.user || transfer.user)?
            {
                return self.abort_entry(core, pc, address, status, transfer.access);
            }
            offset += (4096 - (address & 4095)).min(transfer.bytes - offset);
        }
        Err("Data abort is not explained by the architectural page tables".into())
    }
}
