//! Architectural ARM registers; no execution-library numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum Register {
    R(u8),
    Cpsr,
    Spsr,
    Fpexc,
    Cpacr,
    Fpscr,
    Fpsid,
    D(u8),
    Q(u8),
    S(u8),
    Ipsr,
    Msp,
    Psp,
    Control,
    Xpsr,
    Primask,
    Basepri,
    Faultmask,
}
pub const CORE: [super::Register; 16] = [
    super::Register::Arm32(Register::R(0)),
    super::Register::Arm32(Register::R(1)),
    super::Register::Arm32(Register::R(2)),
    super::Register::Arm32(Register::R(3)),
    super::Register::Arm32(Register::R(4)),
    super::Register::Arm32(Register::R(5)),
    super::Register::Arm32(Register::R(6)),
    super::Register::Arm32(Register::R(7)),
    super::Register::Arm32(Register::R(8)),
    super::Register::Arm32(Register::R(9)),
    super::Register::Arm32(Register::R(10)),
    super::Register::Arm32(Register::R(11)),
    super::Register::Arm32(Register::R(12)),
    super::Register::Arm32(Register::R(13)),
    super::Register::Arm32(Register::R(14)),
    super::Register::Arm32(Register::R(15)),
];
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coprocessor {
    pub cp: u32,
    pub opc1: u32,
    pub crn: u32,
    pub crm: u32,
    pub opc2: u32,
    pub secure: u32,
    pub wide: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemRegister {
    pub op0: u8,
    pub op1: u8,
    pub crn: u8,
    pub crm: u8,
    pub op2: u8,
}
impl SystemRegister {
    /// Architectural op0:op1:CRn:CRm:op2 encoding, independent of an engine ABI.
    pub const fn from_encoding(value: u16) -> Self {
        Self {
            op0: (value >> 14) as u8,
            op1: ((value >> 11) & 7) as u8,
            crn: ((value >> 7) & 15) as u8,
            crm: ((value >> 3) & 15) as u8,
            op2: (value & 7) as u8,
        }
    }
    pub fn encoding(self) -> Result<u16, String> {
        if self.op0 > 3 || self.op1 > 7 || self.crn > 15 || self.crm > 15 || self.op2 > 7 {
            return Err("ARM64 system-register field exceeds its architectural width".into());
        }
        Ok((u16::from(self.op0) << 14)
            | (u16::from(self.op1) << 11)
            | (u16::from(self.crn) << 7)
            | (u16::from(self.crm) << 3)
            | u16::from(self.op2))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exception {
    UndefinedInstruction,
    SupervisorCall,
    PrefetchAbort,
    DataAbort,
    Irq,
    Fiq,
    Breakpoint,
    Unknown(u32),
}

pub const PC: super::Register = super::Register::Arm32(Register::R(15));
pub const CPSR: super::Register = super::Register::Arm32(Register::Cpsr);
pub const SPSR: super::Register = super::Register::Arm32(Register::Spsr);

pub fn condition_passes(condition: u32, cpsr: u32) -> bool {
    let n = cpsr & (1 << 31) != 0;
    let z = cpsr & (1 << 30) != 0;
    let c = cpsr & (1 << 29) != 0;
    let v = cpsr & (1 << 28) != 0;
    [
        z,
        !z,
        c,
        !c,
        n,
        !n,
        v,
        !v,
        c && !z,
        !c || z,
        n == v,
        n != v,
        !z && n == v,
        z || n != v,
        true,
        false,
    ][condition as usize]
}

pub mod thumb;
