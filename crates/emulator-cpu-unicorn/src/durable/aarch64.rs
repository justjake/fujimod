//! ARMv8-A general, SIMD and architectural system state. Inspection reads the
//! stopped core; no execution or device access occurs.
use super::*;
#[cfg(test)]
mod tests;

const MIDR: u16 = 0xc000;
/// System state this backend can restore. Device acknowledgments, read-only
/// registers (including AFSR0/1 and AMAIR_EL1) and debug instrumentation are
/// excluded. SCTLR comes first so restore enables translation only after the
/// table registers are in place.
const SYSTEM: [u16; 20] = [
    0xc080, 0xc082, 0xc100, 0xc101, 0xc102, 0xc200, 0xc201, 0xc208, 0xc290, 0xc300, 0xc3a0,
    0xc510, 0xc600, 0xc681, 0xc684, 0xc708, 0xd000, 0xde82, 0xde83, 0xe208,
];
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    midr: u32,
    pstate: u32,
    registers: [u64; 31],
    sp: u64,
    pc: u64,
    fpcr: u32,
    fpsr: u32,
    vectors: [[u8; 16]; 32],
    system: [u64; SYSTEM.len()],
}

fn system(encoding: u16) -> Register {
    Register::Arm64System(arm::SystemRegister::from_encoding(encoding))
}
fn scalar(cpu: &Core, register: Register) -> Result<u64, String> {
    let mut bytes = [0; 8];
    let size = cpu.read_register(register, &mut bytes)?;
    if !matches!(size, 4 | 8) {
        return Err("ARMv8-A register width differs".into());
    }
    Ok(u64::from_le_bytes(bytes))
}
fn put32(cpu: &Core, register: Register, value: u32) -> Result<(), String> {
    cpu.write_register(register, &value.to_le_bytes())
}
fn put64(cpu: &Core, register: Register, value: u64) -> Result<(), String> {
    cpu.write_register(register, &value.to_le_bytes())
}
fn vector(cpu: &Core, index: u8) -> Result<[u8; 16], String> {
    let mut value = [0; 16];
    if cpu.read_register(Register::V(index), &mut value)? != 16 {
        return Err("ARMv8-A vector register width differs".into());
    }
    Ok(value)
}

impl State {
    pub(super) fn capture(cpu: &Core, _context: &Context) -> Result<Self, String> {
        Ok(Self {
            midr: scalar(cpu, system(MIDR))? as u32,
            pstate: scalar(cpu, Register::Pstate)? as u32,
            registers: array((0..31u8).map(|index| scalar(cpu, Register::X(index))))?,
            sp: scalar(cpu, Register::Sp)?,
            pc: scalar(cpu, Register::Pc)?,
            fpcr: scalar(cpu, Register::Fpcr)? as u32,
            fpsr: scalar(cpu, Register::Fpsr)? as u32,
            vectors: array((0..32u8).map(|index| vector(cpu, index)))?,
            system: array(SYSTEM.iter().map(|&encoding| scalar(cpu, system(encoding))))?,
        })
    }
    pub(super) fn validate(&self, cpu: &Core) -> Result<(), String> {
        if self.midr != scalar(cpu, system(MIDR))? as u32 {
            return Err("CPU snapshot architectural identity differs".into());
        }
        if self.pstate & 0x12 != 0 {
            return Err("invalid ARMv8-A snapshot processor mode".into());
        }
        Ok(())
    }
    pub(super) fn install(&self, cpu: &Core) -> Result<(), String> {
        self.validate(cpu)?;
        // PSTATE selects the SP bank, so it must precede the stack pointer.
        put32(cpu, Register::Pstate, self.pstate)?;
        for (index, &value) in self.registers.iter().enumerate() {
            put64(cpu, Register::X(index as u8), value)?;
        }
        put64(cpu, Register::Sp, self.sp)?;
        for (index, value) in self.vectors.iter().enumerate() {
            cpu.write_register(Register::V(index as u8), value)?;
        }
        put32(cpu, Register::Fpcr, self.fpcr)?;
        put32(cpu, Register::Fpsr, self.fpsr)?;
        // Translation controls first and SCTLR last: a table-walking
        // configuration never sees empty tables.
        for (encoding, &value) in SYSTEM.iter().zip(&self.system).skip(1) {
            put64(cpu, system(*encoding), value).map_err(|error| {
                format!("ARMv8-A snapshot system register {encoding:#06x} restore: {error}")
            })?;
        }
        put64(cpu, system(SYSTEM[0]), self.system[0])
            .map_err(|error| format!("ARMv8-A snapshot SCTLR_EL1 restore: {error}"))?;
        put64(cpu, Register::Pc, self.pc)?;
        cpu.invalidate_code(None)
    }
}
