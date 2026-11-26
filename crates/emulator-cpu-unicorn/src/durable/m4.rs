//! Cortex-M4 architectural state. The board separately retains NVIC, timing
//! and the exclusive reservation domain.
use super::*;
use arm::Register as Arm;

const SPECIAL: [Arm; 7] = [
    Arm::Msp,
    Arm::Psp,
    Arm::Primask,
    Arm::Basepri,
    Arm::Faultmask,
    Arm::Cpacr,
    Arm::Fpscr,
];
const THUMB: u32 = 1 << 24;
const XPSR_EXCEPTION_OR_IT: u32 = 0x0600_fdff;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    registers: [u32; 16],
    xpsr: u32,
    control: u32,
    special: [u32; SPECIAL.len()],
    floating: [u32; 32],
}

impl State {
    pub(super) fn capture(context: &Context) -> Result<Self, String> {
        let state = Self {
            registers: array(
                arm::CORE
                    .into_iter()
                    .map(|register| context.get32(register)),
            )?,
            xpsr: context.get32(Register::Arm32(Arm::Xpsr))?,
            control: context.get32(Register::Arm32(Arm::Control))?,
            special: array(SPECIAL.map(|register| context.get32(Register::Arm32(register))))?,
            floating: array((0..32).map(|index| context.get32(Register::Arm32(Arm::S(index)))))?,
        };
        state.check()?;
        Ok(state)
    }

    fn check(&self) -> Result<(), String> {
        // The portable path currently supports privileged thread checkpoints.
        // Handler stack switching and in-progress IT blocks require native state
        // beyond the writable xPSR surface.
        if self.xpsr & XPSR_EXCEPTION_OR_IT != 0
            || self.control & !6 != 0
            || self.registers[15] & 1 != 0
        {
            return Err(
                "Cortex-M4 checkpoint requires privileged thread mode outside an IT block".into(),
            );
        }
        let active = usize::from(self.control & 2 != 0);
        if self.registers[13] != self.special[active] {
            return Err("Cortex-M4 checkpoint active stack differs".into());
        }
        Ok(())
    }

    pub(super) fn validate(&self, cpu: &Core) -> Result<(), String> {
        if cpu.model != Model::CortexM4 {
            return Err("Cortex-M4 checkpoint processor model differs".into());
        }
        self.check()
    }

    pub(super) fn install(&self, cpu: &Core) -> Result<(), String> {
        self.validate(cpu)?;
        let put = |register, value: u32| cpu.write_register(register, &value.to_le_bytes());
        for (register, value) in SPECIAL.into_iter().zip(self.special) {
            put(Register::Arm32(register), value)?;
        }
        put(Register::Arm32(Arm::Control), self.control)?;
        for (register, value) in arm::CORE[..13].iter().copied().zip(self.registers) {
            put(register, value)?;
        }
        put(Register::Arm32(Arm::R(14)), self.registers[14])?;
        put(Register::Arm32(Arm::Xpsr), self.xpsr)?;
        for (index, value) in self.floating.into_iter().enumerate() {
            put(Register::Arm32(Arm::S(index as u8)), value)?;
        }
        put(
            Register::Pc,
            self.registers[15] | u32::from(self.xpsr & THUMB != 0),
        )?;
        cpu.invalidate_code(None)
    }
}
