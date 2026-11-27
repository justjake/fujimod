//! ARMv7-A stage-1, short-descriptor page table walks.
use crate::Access;

pub struct Mmu {
    pub sctlr: u32,
    ttbcr: u32,
    ttbr0: u32,
    ttbr1: u32,
    dacr: u32,
    fcse: u32,
    pub user: bool,
}
impl Mmu {
    pub fn read(
        mut cp15: impl FnMut(u32, u32, u32) -> Result<u32, String>,
        cpsr: u32,
    ) -> Result<Self, String> {
        let sctlr = cp15(1, 0, 0)?;
        let ttbcr = cp15(2, 0, 2)?;
        if sctlr & 1 == 0 || sctlr & (1 << 23) == 0 || ttbcr & (1 << 31) != 0 {
            return Err("ARMv7 walk requires an enabled short-descriptor MMU".into());
        }
        if sctlr & (1 << 25) != 0 {
            return Err("Big-endian ARM page-table walks are unsupported".into());
        }
        Ok(Self {
            sctlr,
            ttbcr,
            ttbr0: cp15(2, 0, 0)?,
            ttbr1: cp15(2, 0, 1)?,
            dacr: cp15(3, 0, 0)?,
            fcse: cp15(13, 0, 0)? & 0xfe00_0000,
            user: cpsr & 31 == 0x10,
        })
    }

    /// Read descriptors from physical RAM. The nested error is the short-
    /// descriptor fault status, including the domain in FSR[7:4].
    pub fn translate(
        &self,
        address: u32,
        access: Access,
        user: bool,
        mut table_word: impl FnMut(u32) -> Result<u32, String>,
    ) -> Result<Result<u64, u32>, String> {
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
            return Ok(Err(5));
        }
        let first = table_word(
            table | ((address >> 18) & if upper { 0x3ffc } else { (0x3fff >> n) & !3 }),
        )?;
        if first & 3 == 0 {
            return Ok(Err(5));
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
            return Ok(Err(encode(9, 11)));
        }
        let (ap, xn, pxn, physical) = if page {
            let second = table_word((first & 0xfffffc00) | ((address >> 10) & 0x3fc))?;
            if second & 3 == 0 {
                return Ok(Err(encode(5, 7)));
            }
            let large = second & 3 == 1;
            let (xn, mask) = if large {
                (second & (1 << 15) != 0, 0xffff)
            } else {
                (second & 1 != 0, 0xfff)
            };
            (
                ((second >> 4) & 3) | ((second >> 7) & 4),
                xn,
                first & 4 != 0,
                u64::from((second & !mask) | (address & mask)),
            )
        } else {
            let at = u64::from(address);
            let physical = if first & (1 << 18) != 0 {
                // Supersections assemble the extended PA from two fields.
                (u64::from(first & 0x00f0_0000) << 12)
                    | (u64::from(first & 0x0000_01e0) << 31)
                    | u64::from(first & 0xff00_0000)
                    | (at & 0xff_ffff)
            } else {
                u64::from(first & 0xfff0_0000) | (at & 0xf_ffff)
            };
            (
                ((first >> 10) & 3) | ((first >> 13) & 4),
                first & 16 != 0,
                first & 1 != 0,
                physical,
            )
        };
        // Manager domains bypass access-flag and permission checks.
        if domain_access == 3 {
            return Ok(Ok(physical));
        }
        if access == Access::Fetch && (xn || (pxn && !user)) {
            return Ok(Err(encode(13, 15)));
        }
        let afe = self.sctlr & (1 << 29) != 0;
        if afe && ap & 1 == 0 {
            return Ok(Err(encode(3, 6)));
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
        Ok(if allowed {
            Ok(physical)
        } else {
            Err(encode(13, 15))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_table_base_and_index_follow_ttbcr_n() {
        let mmu = Mmu::read(
            |crn, crm, opc2| {
                Ok(match (crn, crm, opc2) {
                    (1, 0, 0) => (1 << 23) | 1,
                    (2, 0, 0) => 0x6000,
                    (2, 0, 2) => 1,
                    (3, 0, 0) => 1,
                    _ => 0,
                })
            },
            0x13,
        )
        .unwrap();

        let read = |address| {
            assert_eq!(address, 0x6004);
            Ok(0x3000_0000 | (3 << 10) | 2)
        };
        assert_eq!(
            mmu.translate(0x0018_0123, Access::Read, false, read)
                .unwrap(),
            Ok(0x3008_0123)
        );
        assert_eq!(
            mmu.translate(0x0018_0123, Access::Fetch, false, |a| Ok(read(a)? | 16))
                .unwrap(),
            Err(13)
        );
    }
}
