//! AArch64 architectural mechanisms, independent of a host execution backend.
use super::Access;

/// Named architectural registers used by checked handoffs and inspection.
pub fn system_register(name: &str) -> Result<super::arm::SystemRegister, String> {
    let n = name.to_ascii_uppercase();
    let v = match n.as_str() {
        "MIDR_EL1" => 0xc000,
        "MPIDR_EL1" => 0xc005,
        "SCTLR_EL1" => 0xc080,
        "CPACR_EL1" => 0xc082,
        "TTBR0_EL1" => 0xc100,
        "TTBR1_EL1" => 0xc101,
        "TCR_EL1" => 0xc102,
        "SPSR_EL1" => 0xc200,
        "ELR_EL1" => 0xc201,
        "SP_EL0" => 0xc208,
        "ESR_EL1" => 0xc290,
        "FAR_EL1" => 0xc300,
        "MAIR_EL1" => 0xc510,
        "VBAR_EL1" => 0xc600,
        "TPIDR_EL1" => 0xc684,
        "TPIDR_EL0" => 0xde82,
        "TPIDRRO_EL0" => 0xde83,
        "CNTKCTL_EL1" => 0xc708,
        "CNTV_CTL_EL0" => 0xdf19,
        "CNTV_CVAL_EL0" => 0xdf1a,
        "SP_EL1" => 0xe208,
        "ICC_PMR_EL1" => 0xc230,
        "ICC_IGRPEN0_EL1" => 0xc666,
        "ICC_IGRPEN1_EL1" => 0xc667,
        "ICC_SRE_EL1" => 0xc665,
        _ => u16::from_str_radix(name.trim_start_matches("0x"), 16)
            .map_err(|_| format!("unknown sysreg {name}"))?,
    };
    Ok(super::arm::SystemRegister::from_encoding(v))
}
/// EL1 stage-1 translation inputs. Table reads must be observational RAM reads.
/// This walker implements little-endian, pre-LPA2 4/16/64 KiB translation with
/// up to 48-bit VA/PA, AP/AF, hierarchical permissions, WXN and PAN.
/// Features that would change these semantics are refused explicitly.
#[derive(Clone, Copy, Debug)]
pub struct TranslationState {
    pub sctlr: u64,
    pub tcr: u64,
    pub ttbr0: u64,
    pub ttbr1: u64,
    pub pstate: u64,
}
impl TranslationState {
    pub fn translate(
        self,
        va: u64,
        access: Access,
        read: impl FnMut(u64) -> Result<u64, String>,
    ) -> Result<u64, String> {
        self.walk(va, access, read).map(|(pa, _)| pa)
    }
    /// Physical address and optional MAIR attribute index. None denotes the
    /// MMU-disabled Device-memory regime, not an assumed Normal mapping.
    pub fn walk(
        self,
        mut va: u64,
        access: Access,
        mut read: impl FnMut(u64) -> Result<u64, String>,
    ) -> Result<(u64, Option<u8>), String> {
        let err = |why: &str| format!("AArch64 {access:?} at {va:#x}: {why}");
        if self.pstate & 0x1c != 0 && self.pstate & 0x1c != 4 {
            return Err(err("only AArch64 EL0/EL1 translation is supported"));
        }
        if self.sctlr & 1 == 0 {
            return Ok((va, None));
        }
        if self.sctlr & (1 << 25) != 0 {
            return Err(err("big-endian translation tables unsupported"));
        }
        // DS (LPA2), HA/HD (hardware descriptor mutation), TBI instruction
        // modifiers, E0PD and SVE MTE permission extensions are not modeled.
        if self.tcr & ((1 << 59) | (3 << 39) | (3 << 51) | (3 << 55)) != 0 {
            return Err(err("unsupported TCR translation extension"));
        }
        let upper = va & (1 << 55) != 0;
        let (sz, tg, disabled, tbi, hpd, ttbr) = if upper {
            (
                ((self.tcr >> 16) & 63) as u32,
                (self.tcr >> 30) & 3,
                self.tcr & (1 << 23) != 0,
                self.tcr & (1 << 38) != 0,
                self.tcr & (1 << 42) != 0,
                self.ttbr1,
            )
        } else {
            (
                (self.tcr & 63) as u32,
                (self.tcr >> 14) & 3,
                self.tcr & (1 << 7) != 0,
                self.tcr & (1 << 37) != 0,
                self.tcr & (1 << 41) != 0,
                self.ttbr0,
            )
        };
        if disabled {
            return Err(err("translation table walks disabled"));
        }
        if tbi {
            va = if upper {
                va | (0xff << 56)
            } else {
                va & !(0xff << 56)
            };
        }
        let (page, index): (u32, u32) = match (upper, tg) {
            (false, 0) | (true, 2) => (12, 9),
            (false, 2) | (true, 1) => (14, 11),
            (false, 1) | (true, 3) => (16, 13),
            _ => return Err("reserved AArch64 translation granule".into()),
        };
        let bits = 64 - sz;
        if bits < page + 1 || bits > 48 {
            return Err(format!("unsupported AArch64 VA width {bits}"));
        }
        let mask = (1u64 << bits) - 1;
        if (!upper && va & !mask != 0) || (upper && va & !mask != !mask) {
            return Err(format!("AArch64 VA {va:#x} outside configured range"));
        }
        let pa_bits = match (self.tcr >> 32) & 7 {
            0 => 32,
            1 => 36,
            2 => 40,
            3 => 42,
            4 => 44,
            5 => 48,
            _ => return Err("AArch64 PA widths above 48 bits unsupported".into()),
        };
        let pa_mask = (1u64 << pa_bits) - 1;
        let levels = (bits - page).div_ceil(index);
        let first = 4 - levels;
        let first_bits = bits - page - index * (levels - 1);
        let mut table = ttbr & 0x0000_ffff_ffff_ffff & !((1u64 << (first_bits + 3)) - 1);
        let (mut no_user, mut read_only, mut pxn, mut uxn) = (false, false, false, false);
        for level in first..4 {
            if table & !pa_mask != 0 {
                return Err("AArch64 table address exceeds IPS".into());
            }
            let shift = page + index * (3 - level);
            let nbits = if level == first { first_bits } else { index };
            let descriptor = read(table + ((va >> shift) & ((1u64 << nbits) - 1)) * 8)?;
            let kind = descriptor & 3;
            if kind & 1 != 0 && descriptor & (3 << 48) != 0 {
                return Err("AArch64 descriptor uses unsupported PA extension bits".into());
            }
            if kind == 3 && level < 3 {
                if descriptor & (((1u64 << page) - 1) & !0xfff) != 0 {
                    return Err("misaligned AArch64 table descriptor".into());
                }
                if !hpd {
                    no_user |= descriptor & (1 << 61) != 0;
                    read_only |= descriptor & (1 << 62) != 0;
                    pxn |= descriptor & (1 << 59) != 0;
                    uxn |= descriptor & (1 << 60) != 0;
                }
                table = descriptor & 0x0000_ffff_ffff_ffff & !((1 << page) - 1);
                continue;
            }
            if !((kind == 3 && level == 3)
                || (kind == 1 && level > 0 && level < 3 && !(page != 12 && level == 1)))
            {
                return Err(format!(
                    "invalid AArch64 L{level} descriptor {descriptor:#x} at {va:#x}"
                ));
            }
            if descriptor & (1 << 10) == 0 {
                return Err("AArch64 access flag fault".into());
            }
            if descriptor & (((1u64 << shift) - 1) & !0xfff) != 0 {
                return Err("misaligned AArch64 block/page descriptor".into());
            }
            let user = !no_user && descriptor & (1 << 6) != 0;
            let writable = !read_only && descriptor & (1 << 7) == 0;
            let el0 = self.pstate & 0xc == 0;
            if el0 && !user {
                return Err("AArch64 EL0 access denied".into());
            }
            if access == Access::Write && !writable {
                return Err("AArch64 write permission fault".into());
            }
            if !el0 && user && self.pstate & (1 << 22) != 0 && access != Access::Fetch {
                return Err("AArch64 PAN permission fault".into());
            }
            if access == Access::Fetch
                && (if el0 {
                    uxn || descriptor & (1 << 54) != 0
                } else {
                    pxn || descriptor & (1 << 53) != 0 || (user && writable)
                } || (self.sctlr & (1 << 19) != 0 && writable))
            {
                return Err("AArch64 execute permission fault".into());
            }
            let base = descriptor & 0x0000_ffff_ffff_ffff & !((1 << shift) - 1);
            if base & !pa_mask != 0 {
                return Err("AArch64 output address exceeds IPS".into());
            }
            return Ok((
                base | (va & ((1 << shift) - 1)),
                Some(((descriptor >> 2) & 7) as u8),
            ));
        }
        unreachable!()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exclusive {
    pub load: bool,
    pub acquire_release: bool,
    pub bytes: u8,
    pub pair: bool,
    pub base: u8,
    pub first: u8,
    pub second: u8,
    pub status: u8,
}
/// Base A64 exclusive accesses, including byte/halfword and register pairs.
/// Ordered non-exclusive and LSE encodings are deliberately excluded.
pub fn decode_exclusive(word: u32) -> Result<Option<Exclusive>, String> {
    if word & 0x3f80_0000 != 0x0800_0000 {
        return Ok(None);
    }
    let load = word & (1 << 22) != 0;
    let pair = word & (1 << 21) != 0;
    let size = (word >> 30) & 3;
    let status = ((word >> 16) & 31) as u8;
    let second = ((word >> 10) & 31) as u8;
    if (load && status != 31) || (!pair && second != 31) || (pair && size < 2) {
        return Err("reserved/LSE A64 exclusive encoding".into());
    }
    let base = ((word >> 5) & 31) as u8;
    let first = (word & 31) as u8;
    if (load && pair && first == second)
        || (!load
            && status != 31
            && (status == first || (pair && status == second) || (base != 31 && status == base)))
    {
        return Err("constrained-unpredictable A64 exclusive register overlap".into());
    }
    Ok(Some(Exclusive {
        load,
        acquire_release: word & (1 << 15) != 0,
        bytes: if pair {
            4 << size.saturating_sub(2)
        } else {
            1 << size
        },
        pair,
        base,
        first,
        second,
        status,
    }))
}

#[derive(Debug)]
pub struct Transfer {
    pub address: u64,
    pub register: u8,
    pub size: u8,
    pub signed: bool,
    pub wide: bool,
}
#[derive(Debug)]
pub struct LoadStore {
    pub load: bool,
    pub simd: bool,
    pub transfers: Vec<Transfer>,
    pub writeback: Option<(u8, u64)>,
}
fn sx(v: u64, b: u32) -> i64 {
    ((v << (64 - b)) as i64) >> (64 - b)
}
fn zr(register: &impl Fn(u8) -> Result<u64, String>, r: u8) -> Result<u64, String> {
    if r == 31 { Ok(0) } else { register(r) }
}

/// Scalar/pair/literal, register offsets, and integer LDAR/STLR.
/// SIMD transfers are for protected RAM; atomics/exclusives fail explicitly.
pub fn decode_load_store(
    word: u32,
    pc: u64,
    register: impl Fn(u8) -> Result<u64, String>,
) -> Result<LoadStore, String> {
    let w = word as u64;
    let rt = (w & 31) as u8;
    let rn = ((w >> 5) & 31) as u8;
    let size = 1u8 << ((w >> 30) & 3);
    let mut out = LoadStore {
        load: false,
        simd: w & (1 << 26) != 0,
        transfers: Vec::new(),
        writeback: None,
    };
    let (address, signed, wide, bytes);
    if w & 0x3b00_0000 == 0x1800_0000 {
        let opc = w >> 30;
        if opc == 3 {
            return Err("PRFM abort".into());
        }
        address = pc.wrapping_add_signed(sx((w >> 5) & 0x7ffff, 19) * 4);
        signed = !out.simd && opc == 2;
        wide = opc != 0;
        bytes = if out.simd {
            1 << (opc + 2)
        } else if opc == 1 {
            8
        } else {
            4
        };
        out.load = true;
    } else if w & 0x3a00_0000 == 0x2800_0000 {
        let opc = w >> 30;
        out.load = w & (1 << 22) != 0;
        if opc == 3 || (!out.simd && opc == 1 && !out.load) {
            return Err("reserved integer pair".into());
        }
        bytes = if out.simd {
            1 << (opc + 2)
        } else if opc == 2 {
            8
        } else {
            4
        };
        signed = !out.simd && opc == 1;
        wide = opc != 0;
        let base = register(rn)?;
        let delta = sx((w >> 15) & 127, 7) * bytes as i64;
        let mode = (w >> 23) & 3;
        address = if mode == 1 {
            base
        } else {
            base.wrapping_add_signed(delta)
        };
        if mode == 1 || mode == 3 {
            out.writeback = Some((rn, base.wrapping_add_signed(delta)));
        }
        out.transfers.push(Transfer {
            address: address.wrapping_add(bytes as u64),
            register: ((w >> 10) & 31) as u8,
            size: bytes,
            signed,
            wide,
        });
    } else if w & 0x3f9f_fc00 == 0x089f_fc00 {
        // LDAR/STLR, non-exclusive.
        out.load = w & (1 << 22) != 0;
        address = register(rn)?;
        bytes = size;
        signed = false;
        wide = size == 8;
    } else if w & 0x3a00_0000 == 0x3800_0000 {
        let opc = (w >> 22) & 3;
        out.load = if out.simd { opc & 1 != 0 } else { opc != 0 };
        signed = !out.simd && opc >= 2;
        wide = size == 8 || opc == 2;
        bytes = if out.simd && opc >= 2 { 16 } else { size };
        if (out.simd && opc >= 2 && size != 1)
            || (!out.simd && ((size == 8 && opc >= 2) || (size == 4 && opc == 3)))
        {
            return Err("reserved scalar load/store or PRFM".into());
        }
        let base = register(rn)?;
        if w & (1 << 24) != 0 {
            address = base.wrapping_add(((w >> 10) & 0xfff) * bytes as u64);
        } else if w & (1 << 21) != 0 {
            if (w >> 10) & 3 != 2 {
                return Err("atomic/reserved load/store".into());
            }
            let rm = ((w >> 16) & 31) as u8;
            let option = (w >> 13) & 7;
            let raw = zr(&register, rm)?;
            let offset = match option {
                2 => raw as u32 as u64,
                3 => raw,
                6 => raw as u32 as i32 as i64 as u64,
                7 => raw as i64 as u64,
                _ => return Err("reserved register offset extension".into()),
            };
            let shift = if w & (1 << 12) != 0 {
                bytes.trailing_zeros() as u64
            } else {
                0
            };
            address = base.wrapping_add(offset << shift);
        } else {
            let delta = sx((w >> 12) & 0x1ff, 9);
            let mode = (w >> 10) & 3;
            address = if mode == 1 {
                base
            } else {
                base.wrapping_add_signed(delta)
            };
            if mode == 1 || mode == 3 {
                out.writeback = Some((rn, base.wrapping_add_signed(delta)));
            }
        }
    } else {
        return Err(format!(
            "unsupported MMIO instruction {word:#010x} at {pc:#x}"
        ));
    }
    out.transfers.insert(
        0,
        Transfer {
            address,
            register: rt,
            size: bytes,
            signed,
            wide,
        },
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exclusive_encodings_and_refusals() {
        for (load, store, bytes, pair) in [
            (0x085f7c20, 0x08027c20, 1, false),
            (0x485ffc20, 0x4802fc20, 2, false),
            (0x885f7c20, 0x88027c20, 4, false),
            (0xc85ffc20, 0xc802fc20, 8, false),
            (0x887f0c20, 0x88220c20, 4, true),
            (0xc87f8c20, 0xc8228c20, 8, true),
        ] {
            let l = decode_exclusive(load).unwrap().unwrap();
            let s = decode_exclusive(store).unwrap().unwrap();
            assert_eq!(
                (l.load, s.load, l.bytes, s.bytes, l.pair, s.pair),
                (true, false, bytes, bytes, pair, pair)
            );
            assert_eq!((l.base, l.first, s.status), (1, 0, 2));
        }
        assert!(decode_exclusive(0x889ffc20).unwrap().is_none()); // STLR
        assert!(decode_exclusive(0x88007c20).is_err()); // STXR w0,w0,[x1]
        assert!(decode_exclusive(0x087f0c20).is_err()); // reserved pair size
        assert!(decode_exclusive(0x887f0020).is_err()); // LDXP w0,w0,[x1]
    }
    fn registers(r: u8) -> Result<u64, String> {
        Ok(if r == 31 {
            0x8000
        } else {
            0x1000 + r as u64 * 16
        })
    }
    #[test]
    fn indexed_and_pair() {
        let d = decode_load_store(0xf8408420, 0, registers).unwrap(); // ldr x0,[x1],#8
        assert_eq!(d.transfers[0].address, 0x1010);
        assert_eq!(d.writeback, Some((1, 0x1018)));
        let d = decode_load_store(0xa9bf7bfd, 0, registers).unwrap(); // stp x29,x30,[sp,#-16]!
        assert!(!d.load);
        assert_eq!(d.transfers[0].address, 0x7ff0);
        assert_eq!(d.transfers[1].address, 0x7ff8);
        assert_eq!(d.writeback, Some((31, 0x7ff0)));
        assert!(decode_load_store(0xc85f7c20, 0, registers).is_err()); // ldxr cannot be treated as ldr
    }
    #[test]
    fn signed_register_offset_and_ordered_access() {
        let d = decode_load_store(0xf8627820, 0, registers).unwrap(); // ldr x0,[x1,x2,lsl #3]
        assert_eq!(d.transfers[0].address, 0x1010 + 0x1020 * 8);
        let d = decode_load_store(0x39c00020, 0, registers).unwrap(); // ldrsb w0,[x1]
        assert!(d.transfers[0].signed);
        assert!(!d.transfers[0].wide);
        assert!(decode_load_store(0xc8dffc20, 0, registers).unwrap().load); // ldar x0,[x1]
        assert!(!decode_load_store(0xc89ffc20, 0, registers).unwrap().load); // stlr x0,[x1]
    }
    #[test]
    fn simd_ram_widths_and_writeback() {
        let d = decode_load_store(0x3d800420, 0, registers).unwrap(); // str q0,[x1,#16]
        assert!(d.simd && !d.load);
        assert_eq!((d.transfers[0].address, d.transfers[0].size), (0x1020, 16));
        let d = decode_load_store(0xad810420, 0, registers).unwrap(); // stp q0,q1,[x1,#32]!
        assert!(d.simd && !d.load);
        assert_eq!(
            (d.transfers[0].address, d.transfers[1].address),
            (0x1030, 0x1040)
        );
        assert_eq!(d.writeback, Some((1, 0x1030)));
        let d = decode_load_store(0x3cc1043f, 0, registers).unwrap(); // ldr q31,[x1],#16
        assert!(d.simd && d.load);
        assert_eq!(d.transfers[0].register, 31);
        assert_eq!(d.writeback, Some((1, 0x1020)));
    }
}

#[cfg(test)]
mod translation_tests {
    use super::*;
    use std::collections::BTreeMap;
    fn state() -> TranslationState {
        TranslationState {
            sctlr: 1,
            tcr: 32,
            ttbr0: 0x4000,
            ttbr1: 0,
            pstate: 5,
        }
    }
    fn walk(
        s: TranslationState,
        va: u64,
        access: Access,
        table: &BTreeMap<u64, u64>,
    ) -> Result<u64, String> {
        s.translate(va, access, |a| {
            table
                .get(&a)
                .copied()
                .ok_or_else(|| format!("not RAM: {a:#x}"))
        })
    }
    #[test]
    fn granules_upper_half_and_descriptor_refusals() {
        let mut s = state();
        let t = BTreeMap::from([(0x4000, 0x8003), (0x8000, 0xc003), (0xc008, 0xabc403)]);
        assert_eq!(walk(s, 0x1234, Access::Read, &t).unwrap(), 0xabc234);
        s.tcr = 32 | (2 << 14);
        let t = BTreeMap::from([(0x4000, 0x8003), (0x8000, 0xabc403)]);
        assert_eq!(walk(s, 0x1234, Access::Read, &t).unwrap(), 0xabd234);
        s.tcr = 32 | (1 << 14);
        s.ttbr0 = 0x10000;
        let t = BTreeMap::from([(0x10000, 0x20003), (0x20000, 0xab0403)]);
        assert_eq!(walk(s, 0x1234, Access::Read, &t).unwrap(), 0xab1234);
        s.tcr = 32 | (32 << 16) | (2 << 30) | (1 << 38);
        s.ttbr1 = 0x4000;
        let t = BTreeMap::from([(0x4018, 0x40000401)]);
        assert_eq!(
            walk(s, 0xabffffffc0001234, Access::Read, &t).unwrap(),
            0x40001234
        );
        s.tcr |= 1 << 23;
        assert!(walk(s, 0xffff_ffff_c0001234, Access::Read, &t).is_err());
        s = state();
        s.tcr |= 1 << 59;
        assert!(walk(s, 0, Access::Read, &t).is_err());
        s = state();
        assert!(walk(s, 1 << 32, Access::Read, &t).is_err());
        let t = BTreeMap::from([(0x4000, 0)]);
        assert!(walk(s, 0, Access::Read, &t).is_err());
        let t = BTreeMap::from([(0x4000, 0x1_0000_0003)]);
        assert!(walk(s, 0, Access::Read, &t).is_err());
    }
    #[test]
    fn access_flags_page_and_hierarchical_permissions() {
        let mut s = state();
        let mut t = BTreeMap::from([(0x4000, 0x8003), (0x8000, 0xc003), (0xc008, 0xabc403)]);
        assert!(walk(s, 0x1234, Access::Fetch, &t).is_ok());
        assert!(walk(s, 0x1234, Access::Write, &t).is_ok());
        *t.get_mut(&0xc008).unwrap() |= 1 << 7;
        assert!(walk(s, 0x1234, Access::Write, &t).is_err());
        assert!(walk(s, 0x1234, Access::Read, &t).is_ok());
        *t.get_mut(&0xc008).unwrap() |= 1 << 53;
        assert!(walk(s, 0x1234, Access::Fetch, &t).is_err());
        t.insert(0xc008, 0xabc003);
        assert!(walk(s, 0x1234, Access::Read, &t).is_err());
        t.insert(0xc008, 0xabc443);
        s.pstate = 0;
        assert!(walk(s, 0x1234, Access::Read, &t).is_ok());
        t.insert(0x4000, 0x8003 | (1 << 61));
        assert!(walk(s, 0x1234, Access::Read, &t).is_err());
        s.tcr |= 1 << 41;
        assert!(walk(s, 0x1234, Access::Read, &t).is_ok());
        s = state();
        t.insert(0x4000, 0x8003 | (1 << 62));
        assert!(walk(s, 0x1234, Access::Write, &t).is_err());
        t.insert(0x4000, 0x8003 | (1 << 59));
        t.insert(0xc008, 0xabc403);
        assert!(walk(s, 0x1234, Access::Fetch, &t).is_err());
        t.insert(0x4000, 0x8003);
        s.sctlr |= 1 << 19;
        assert!(walk(s, 0x1234, Access::Fetch, &t).is_err());
        s = state();
        s.pstate |= 1 << 22;
        t.insert(0xc008, 0xabc443);
        assert!(walk(s, 0x1234, Access::Read, &t).is_err());
    }
}

/// A-profile framework boundaries, independent of firmware or device maps.
pub fn framework_instruction(word: u32) -> bool {
    if word & 0xffd00000 == 0xd5100000 {
        let encoding = ((word >> 5) & 0xffff) as u16;
        let r = super::arm::SystemRegister::from_encoding(encoding);
        r.crn == 9
            || r.crn == 14
            || r.crn == 15
            || encoding == 0xc230
            || encoding == 0xf665
            || (r.op0 == 3 && r.op1 == 0 && r.crn == 12 && (8..=15).contains(&r.crm))
    } else {
        matches!(word, 0xd503205f | 0xd503207f | 0xd503209f | 0xd50320bf)
    }
}
/// Route an asserted EL1 IRQ/FIQ at an instruction boundary. Entry consumes no
/// instruction or virtual tick. Unsupported routing above EL1 refuses.
pub fn enter_interrupt(cpu: &dyn super::Cpu, irq: bool, fiq: bool) -> Result<bool, String> {
    use super::{Register, arm::SystemRegister};
    let read = |r| {
        let mut b = [0; 8];
        cpu.read_register(r, &mut b)?;
        Ok::<u64, String>(u64::from_le_bytes(b))
    };
    let sys = |r| Register::Arm64System(SystemRegister::from_encoding(r));
    let pstate = read(Register::Pstate)?;
    if !(irq && pstate & 0x80 == 0 || fiq && pstate & 0x40 == 0) {
        return Ok(false);
    }
    let fiq = fiq && pstate & 0x40 == 0;
    if (pstate >> 2) & 3 > 1
        || read(sys(0xf088))? & if fiq { 4 } else { 2 } != 0
        || read(sys(0xe088))? & if fiq { 8 } else { 0x10 } != 0
    {
        return Err("AArch64 interrupt routing above EL1 is unsupported".into());
    }
    let offset = match pstate & 15 {
        0 => 0x400,
        4 => 0,
        5 => 0x200,
        _ => return Err("AArch64 interrupt entry mode is unsupported".into()),
    } + if fiq { 0x100 } else { 0x80 };
    let vector = read(sys(0xc600))? + offset;
    let pc = read(Register::Pc)?;
    cpu.write_register(sys(0xc201), &pc.to_le_bytes())?;
    cpu.write_register(sys(0xc200), &pstate.to_le_bytes())?;
    cpu.write_register(
        Register::Pstate,
        &((pstate & 0xf0000000) | 0x3c5).to_le_bytes(),
    )?;
    cpu.write_register(Register::Pc, &vector.to_le_bytes())?;
    Ok(true)
}
