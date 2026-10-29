//! Relocation validation adapted with permission from Tal Shiri's X-Turbo
//! (src/fuji/jig_relocations.py). ARM relocation rules follow the ELF ARM ABI.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    ArmAbs32,
    ArmRel32,
    ArmCall,
    ArmJump24,
    ArmMov32,
    Aarch64Abs32,
    Aarch64Abs64,
    Aarch64Call26,
    Aarch64Jump26,
    Aarch64Adrp,
    Aarch64AddLo12,
    Aarch64Ldst8Lo12,
    Aarch64Ldst16Lo12,
    Aarch64Ldst32Lo12,
    Aarch64Ldst64Lo12,
    Aarch64Ldst128Lo12,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    Image(u32),
    Hook(u32),
}
/// `value` is the resolved linked value (including its addend). `moves`
/// identifies a symbol that moves with the image. No pointer guessing occurs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Relocation {
    pub destination: Destination,
    pub kind: Kind,
    pub value: u64,
    pub moves: bool,
    /// The second instruction of a MOVW/MOVT pair, if applicable.
    #[serde(default)]
    pub pair: Option<Destination>,
}
pub(crate) fn moved(value: u64, delta: i128) -> Result<u64> {
    u64::try_from(i128::from(value) + delta)
        .map_err(|_| Error::Invalid("relocated address overflow"))
}
fn require(ok: bool, message: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(message))
    }
}
fn signed(value: i128, bits: u32) -> bool {
    -(1i128 << (bits - 1)) <= value && value < (1i128 << (bits - 1))
}
/// Encode one relocation at its relocated place. Values already include addends.
pub fn encode(kind: Kind, word: u64, value: u64, place: u64) -> Result<u64> {
    use Kind::*;
    let w = word as u32;
    let result = match kind {
        ArmAbs32 | Aarch64Abs32 => {
            u32::try_from(value).map_err(|_| Error::Invalid("ABS32 overflow"))?
        }
        ArmRel32 => (i128::from(value) - i128::from(place)) as u32,
        ArmCall | ArmJump24 => {
            require(
                w & 0x0e000000 == 0x0a000000 && w >> 28 != 15,
                "A32 branch opcode mismatch",
            )?;
            require(
                (w & 0x01000000 != 0) == (kind == ArmCall),
                "A32 branch link bit mismatch",
            )?;
            let distance = i128::from(value) - i128::from(place);
            require(
                distance % 4 == 0 && signed(distance, 26),
                "A32 branch out of range",
            )?;
            (w & 0xff000000) | ((distance >> 2) as u32 & 0xffffff)
        }
        Aarch64Abs64 => return Ok(value),
        Aarch64Call26 | Aarch64Jump26 => {
            let opcode = if kind == Aarch64Call26 {
                0x94000000
            } else {
                0x14000000
            };
            require(w & 0xfc000000 == opcode, "AArch64 branch opcode mismatch")?;
            let distance = i128::from(value) - i128::from(place);
            require(
                distance % 4 == 0 && signed(distance, 28),
                "AArch64 branch out of range",
            )?;
            opcode | ((distance >> 2) as u32 & 0x3ffffff)
        }
        Aarch64Adrp => {
            require(w & 0x9f000000 == 0x90000000, "ADRP opcode mismatch")?;
            let pages = i128::from(value >> 12) - i128::from(place >> 12);
            require(signed(pages, 21), "ADRP out of range")?;
            let immediate = pages as u32;
            (w & !0x60ffffe0) | ((immediate & 3) << 29) | (((immediate >> 2) & 0x7ffff) << 5)
        }
        Aarch64AddLo12 => {
            require(w & 0x7fc00000 == 0x11000000, "ADD opcode mismatch")?;
            (w & !0x003ffc00) | ((value as u32 & 4095) << 10)
        }
        Aarch64Ldst8Lo12 | Aarch64Ldst16Lo12 | Aarch64Ldst32Lo12 | Aarch64Ldst64Lo12
        | Aarch64Ldst128Lo12 => {
            let scale = match kind {
                Aarch64Ldst8Lo12 => 0,
                Aarch64Ldst16Lo12 => 1,
                Aarch64Ldst32Lo12 => 2,
                Aarch64Ldst64Lo12 => 3,
                _ => 4,
            };
            require(w & 0x3b000000 == 0x39000000, "Load/store opcode mismatch")?;
            if scale == 4 {
                require(
                    w & 0x04800000 == 0x04800000 && w >> 30 == 0,
                    "128-bit load/store opcode mismatch",
                )?;
            } else {
                require(
                    w >> 30 == scale && !(w & 0x04000000 != 0 && w & 0x00800000 != 0),
                    "Load/store scale mismatch",
                )?;
            }
            require(value % (1 << scale) == 0, "Unaligned load/store relocation")?;
            (w & !0x003ffc00) | (((value as u32 & 4095) >> scale) << 10)
        }
        ArmMov32 => return Err(Error::Invalid("MOV32 requires a pair")),
    };
    Ok(u64::from(result))
}
/// Relocate the complete pointer so carries and borrows cross the halves.
pub fn mov_pair(low: u32, high: u32, delta: i128) -> Result<(u32, u32)> {
    require(
        low & 0x0ff00000 == 0x03000000
            && high & 0x0ff00000 == 0x03400000
            && low >> 28 != 15
            && high >> 28 != 15
            && (low >> 12) & 15 != 15
            && low & 0xf000f000 == high & 0xf000f000,
        "MOVW/MOVT pair mismatch",
    )?;
    let immediate = |w: u32| ((w >> 4) & 0xf000) | (w & 0xfff);
    let value = (i128::from(immediate(low) | immediate(high) << 16) + delta) as u32;
    let replace = |w: u32, half: u32| (w & !0x000f0fff) | ((half & 0xf000) << 4) | (half & 0xfff);
    Ok((replace(low, value & 0xffff), replace(high, value >> 16)))
}
