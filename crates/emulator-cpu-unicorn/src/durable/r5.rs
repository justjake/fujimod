//! ARMv7-R banks and fixed R5F system-register selectors. Inspection uses a
//! detached core context; the live MPU debug selector is restored on every exit.
use super::*;
#[cfg(test)]
mod tests;

const MODES: [u32; 7] = [0x10, 0x11, 0x12, 0x13, 0x17, 0x1b, 0x1f];
const CP15: [(u32, u32, u32, u32); 16] = [
    (0, 1, 0, 0),
    (0, 3, 0, 0),
    (2, 0, 0, 0),
    (0, 5, 0, 0),
    (0, 5, 0, 1),
    (0, 6, 0, 0),
    (0, 6, 0, 2),
    (0, 12, 0, 0),
    (0, 13, 0, 0),
    (0, 13, 0, 1),
    (0, 13, 0, 2),
    (0, 13, 0, 3),
    (0, 13, 0, 4),
    (0, 10, 2, 0),
    (0, 10, 2, 1),
    (0, 7, 4, 0),
];
const MPU: [(u32, u32, u32, u32); 3] = [(0, 6, 1, 0), (0, 6, 1, 2), (0, 6, 1, 4)];
const SELECT: (u32, u32, u32, u32) = (0, 6, 2, 0);
const SCALARS: [Register; 4] = [
    Register::Arm32(arm::Register::Cpacr),
    Register::Arm32(arm::Register::Fpexc),
    Register::Arm32(arm::Register::Fpscr),
    Register::Diagnostic(Diagnostic::ExceptionSyndrome),
];
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    identity: [u32; 6],
    cpsr: u32,
    registers: [u32; 16],
    scalar: [u32; 4],
    vectors: [[u8; 16]; 16],
    banks: [Bank; 7],
    cp15: [[u32; 2]; 16],
    mpu_selected: u32,
    mpu: [[u32; 3]; 16],
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
    let (opc1, crn, crm, opc2) = selector;
    u32::try_from(cpu.coprocessor(
        arm::Coprocessor {
            cp: 15,
            opc1,
            crn,
            crm,
            opc2,
            secure,
            wide: false,
        },
        value.map(u64::from),
    )?)
    .map_err(|_| "R5F system register exceeds 32 bits".into())
}
fn identity(cpu: &Core) -> Result<[u32; 6], String> {
    if cpu.model != Model::CortexR5F {
        return Err("CPU snapshot requires the Cortex-R5F model".into());
    }
    let mut fpsid = [0; 4];
    cpu.read_register(Register::Arm32(arm::Register::Fpsid), &mut fpsid)?;
    Ok([
        cp(cpu, (0, 0, 0, 0), 0, None)?,
        cp(cpu, (0, 0, 0, 4), 0, None)?,
        cp(cpu, (0, 0, 0, 2), 0, None)?,
        cp(cpu, (0, 9, 1, 0), 0, None)? & 0x7c,
        cp(cpu, (0, 9, 1, 1), 0, None)? & 0x7c,
        u32::from_le_bytes(fpsid),
    ])
}
fn put(cpu: &Core, register: Register, value: u32) -> Result<(), String> {
    cpu.write_register(register, &value.to_le_bytes())
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
        let cp15 = array(
            CP15.map(|selector| array((0..2).map(|secure| cp(cpu, selector, secure, None)))),
        )?;
        let selected = cp(cpu, SELECT, 0, None)?;
        let mpu = (|| {
            array((0..16).map(|index| {
                cp(cpu, SELECT, 0, Some(index))?;
                array(MPU.map(|selector| cp(cpu, selector, 0, None)))
            }))
        })();
        let restored = cp(cpu, SELECT, 0, Some(selected));
        let mpu = mpu?;
        restored?;
        Ok(Self {
            identity: identity(cpu)?,
            cpsr,
            registers,
            scalar,
            vectors,
            banks,
            cp15,
            mpu_selected: selected,
            mpu,
        })
    }
    pub(super) fn validate(&self, cpu: &Core) -> Result<(), String> {
        if self.identity != identity(cpu)? {
            return Err("CPU snapshot architectural identity differs".into());
        }
        let mode = MODES
            .iter()
            .position(|&m| m == self.cpsr & 31)
            .ok_or("invalid R5F snapshot processor mode")?;
        if self.mpu_selected >= 16 {
            return Err("R5F snapshot MPU selector exceeds its region count".into());
        }
        for (i, bank) in self.banks.iter().enumerate() {
            if bank.spsr.is_none() != matches!(MODES[i], 0x10 | 0x1f)
                || (MODES[i] != 0x11 && bank.registers[..5] != self.banks[0].registers[..5])
            {
                return Err("inconsistent R5F snapshot register banks".into());
            }
        }
        if self.banks[0] != self.banks[6] || self.registers[8..15] != self.banks[mode].registers {
            return Err("inconsistent R5F snapshot current/shared bank".into());
        }
        if self.cp15[2].iter().any(|v| v & !0xf != 0)
            || self.cp15[7].iter().any(|v| v & 31 != 0)
            || self.cp15[15].iter().any(|v| v & !0xfffff6ff != 0)
        {
            return Err("R5F snapshot contains unrepresentable system-register bits".into());
        }
        Ok(())
    }
    pub(super) fn install(&self, cpu: &Core) -> Result<(), String> {
        self.validate(cpu)?;
        for (index, values) in self.mpu.iter().enumerate() {
            cp(cpu, SELECT, 0, Some(index as u32))?;
            for (selector, &value) in MPU.into_iter().zip(values) {
                cp(cpu, selector, 0, Some(value))?;
            }
        }
        cp(cpu, SELECT, 0, Some(self.mpu_selected))?;
        for (selector, values) in CP15.into_iter().zip(&self.cp15).skip(1) {
            for (secure, &value) in values.iter().enumerate() {
                cp(cpu, selector, secure as u32, Some(value))?;
            }
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
        for (secure, &value) in self.cp15[0].iter().enumerate() {
            cp(cpu, CP15[0], secure as u32, Some(value))?;
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
