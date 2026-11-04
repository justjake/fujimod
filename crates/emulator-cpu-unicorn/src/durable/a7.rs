//! ARMv7-A (Cortex-A7) banks, VFP/NEON registers and the system registers
//! that define address translation and exception state. Inspection uses a
//! detached core context; no execution or device access occurs.
use super::*;
#[cfg(test)]
mod tests;

const MODES: [u32; 8] = [0x10, 0x11, 0x12, 0x13, 0x16, 0x17, 0x1b, 0x1f];
/// (opc1, crn, crm, opc2). SCTLR is first: restoring it last enables the MMU
/// only after the table registers are in place.
const CP15: [(u32, u32, u32, u32); 24] = [
    (0, 1, 0, 0),  // SCTLR
    (0, 1, 0, 1),  // ACTLR
    (0, 1, 0, 2),  // CPACR
    (0, 1, 1, 0),  // SCR
    (0, 1, 1, 1),  // SDER
    (0, 1, 1, 2),  // NSACR
    (0, 2, 0, 0),  // TTBR0
    (0, 2, 0, 1),  // TTBR1
    (0, 2, 0, 2),  // TTBCR
    (0, 3, 0, 0),  // DACR
    (0, 5, 0, 0),  // DFSR
    (0, 5, 0, 1),  // IFSR
    (0, 5, 1, 0),  // ADFSR
    (0, 5, 1, 1),  // AIFSR
    (0, 6, 0, 0),  // DFAR
    (0, 6, 0, 2),  // IFAR
    (0, 10, 2, 0), // PRRR
    (0, 10, 2, 1), // NMRR
    (0, 12, 0, 0), // VBAR
    (0, 12, 0, 1), // MVBAR
    (0, 13, 0, 0), // FCSEIDR
    (0, 13, 0, 1), // CONTEXTIDR
    (0, 13, 0, 2), // TPIDRURW
    (2, 0, 0, 0),  // CSSELR
];
/// Registers restored after the table registers: user read-only thread id and
/// the physical address register.
const LATE: [(u32, u32, u32, u32); 2] = [(0, 13, 0, 3), (0, 7, 4, 0)];
const THREAD_PRIVILEGED: (u32, u32, u32, u32) = (0, 13, 0, 4);
const SCALARS: [Register; 3] = [
    Register::Arm32(arm::Register::Cpacr),
    Register::Arm32(arm::Register::Fpexc),
    Register::Arm32(arm::Register::Fpscr),
];
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    identity: [u32; 2],
    cpsr: u32,
    registers: [u32; 16],
    scalar: [u32; 3],
    vectors: [[u8; 16]; 16],
    banks: [Bank; 8],
    cp15: [[u32; 2]; 24],
    late: [[u32; 2]; 2],
    thread: [u32; 2],
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bank {
    registers: [u32; 7],
    spsr: Option<u32>,
}

fn cp(
    cpu: &Core,
    selector: (u32, u32, u32, u32),
    secure: u32,
    value: Option<u32>,
) -> Result<u32, String> {
    u32::try_from(cpu.cp15(selector, secure, value.map(u64::from))?)
        .map_err(|_| "Cortex-A7 system register exceeds 32 bits".into())
}
fn identity(cpu: &Core) -> Result<[u32; 2], String> {
    if cpu.model != Model::CortexA7 {
        return Err("CPU snapshot requires the Cortex-A7 model".into());
    }
    let mut fpsid = [0; 4];
    cpu.read_register(Register::Arm32(arm::Register::Fpsid), &mut fpsid)?;
    Ok([cp(cpu, (0, 0, 0, 0), 0, None)?, u32::from_le_bytes(fpsid)])
}
fn put(cpu: &Core, register: Register, value: u32) -> Result<(), String> {
    cpu.write_register(register, &value.to_le_bytes())
}
fn both(cpu: &Core, selector: (u32, u32, u32, u32)) -> Result<[u32; 2], String> {
    array((0..2).map(|secure| cp(cpu, selector, secure, None)))
}

impl State {
    pub(super) fn capture(cpu: &Core, context: &Context) -> Result<Self, String> {
        let cpsr = context.get32(arm::CPSR)?;
        let registers = array(arm::CORE.map(|r| context.get32(r)))?;
        let scalar = array(SCALARS.map(|r| context.get32(r)))?;
        let vectors = array((0..16).map(|i| context.get(Register::Arm32(arm::Register::Q(i)))))?;
        let banks = (|| {
            array(MODES.map(|mode| {
                context.put32(arm::CPSR, cpsr & !31 | mode)?;
                Ok(Bank {
                    registers: array((8..15).map(|i| context.get32(arm::CORE[i])))?,
                    spsr: if matches!(mode, 0x10 | 0x1f) {
                        None
                    } else {
                        Some(context.get32(arm::SPSR)?)
                    },
                })
            }))
        })();
        let restored = context.put32(arm::CPSR, cpsr);
        let banks = banks?;
        restored?;
        Ok(Self {
            identity: identity(cpu)?,
            cpsr,
            registers,
            scalar,
            vectors,
            banks,
            cp15: array(CP15.map(|selector| both(cpu, selector)))?,
            late: array(LATE.map(|selector| both(cpu, selector)))?,
            thread: both(cpu, THREAD_PRIVILEGED)?,
        })
    }
    pub(super) fn validate(&self, cpu: &Core) -> Result<(), String> {
        if self.identity != identity(cpu)? {
            return Err("CPU snapshot architectural identity differs".into());
        }
        let mode = MODES
            .iter()
            .position(|&m| m == self.cpsr & 31)
            .ok_or("invalid Cortex-A7 snapshot processor mode")?;
        for (i, bank) in self.banks.iter().enumerate() {
            if bank.spsr.is_none() != matches!(MODES[i], 0x10 | 0x1f)
                || (MODES[i] != 0x11 && bank.registers[..5] != self.banks[0].registers[..5])
            {
                return Err("inconsistent Cortex-A7 snapshot register banks".into());
            }
        }
        if self.banks[0] != self.banks[7] || self.registers[8..15] != self.banks[mode].registers {
            return Err("inconsistent Cortex-A7 snapshot current/shared bank".into());
        }
        Ok(())
    }
    pub(super) fn install(&self, cpu: &Core) -> Result<(), String> {
        self.validate(cpu)?;
        // Translation controls first, the system control register last of
        // them: a table-walking configuration never sees empty tables.
        for (selector, values) in CP15.into_iter().zip(&self.cp15).skip(1) {
            for (secure, &value) in values.iter().enumerate() {
                cp(cpu, selector, secure as u32, Some(value))?;
            }
        }
        for (secure, &value) in self.cp15[0].iter().enumerate() {
            cp(cpu, CP15[0], secure as u32, Some(value))?;
        }
        for (selector, values) in LATE.into_iter().zip(&self.late) {
            for (secure, &value) in values.iter().enumerate() {
                cp(cpu, selector, secure as u32, Some(value))?;
            }
        }
        for (secure, &value) in self.thread.iter().enumerate() {
            cp(cpu, THREAD_PRIVILEGED, secure as u32, Some(value))?;
        }
        for (mode, bank) in MODES.into_iter().zip(&self.banks) {
            put(cpu, arm::CPSR, 0xd0 | mode)?;
            for (index, &value) in bank.registers.iter().enumerate() {
                put(cpu, arm::CORE[index + 8], value)?;
            }
            if let Some(spsr) = bank.spsr {
                put(cpu, arm::SPSR, spsr)?;
            }
        }
        for (register, &value) in SCALARS.into_iter().zip(&self.scalar) {
            put(cpu, register, value)?;
        }
        for (index, value) in self.vectors.iter().enumerate() {
            cpu.write_register(Register::Arm32(arm::Register::Q(index as u8)), value)?;
        }
        for (register, &value) in arm::CORE[..8].iter().zip(&self.registers[..8]) {
            put(cpu, *register, value)?;
        }
        // PC host writes select Thumb from bit 0. Apply the exact CPSR after it
        // to retain the saved instruction set and partially executed IT block.
        put(cpu, arm::PC, self.registers[15])?;
        put(cpu, arm::CPSR, self.cpsr)?;
        cpu.invalidate_code(None)
    }
}
