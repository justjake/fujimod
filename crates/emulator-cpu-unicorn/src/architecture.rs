//! Paused architectural debugging. Bank selection occurs on a detached
//! context; the MPU debug selector is restored before inspection returns.
//! No execution, device accesses or execution-owner accounting occurs.
use super::*;
use serde_json::{Value, json};

impl Context {
    fn read(&self, register: Register) -> Result<Value, String> {
        let mut bytes = [0; 16];
        let mut size = bytes.len();
        check(&self.api, unsafe {
            (self.api.uc_context_reg_read2)(
                self.raw,
                register_id(register)?,
                bytes.as_mut_ptr().cast(),
                &mut size,
            )
        })?;
        match size {
            4 => Ok(json!(u32::from_le_bytes(bytes[..4].try_into().unwrap()))),
            8 => Ok(json!(u64::from_le_bytes(bytes[..8].try_into().unwrap()))),
            16 => Ok(json!(format!("{:032x}", u128::from_le_bytes(bytes)))),
            _ => Err("unsupported context register width".into()),
        }
    }
    fn scalar(&self, register: Register) -> Result<u32, String> {
        self.read(register)?
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or("context scalar exceeds 32 bits".into())
    }
    fn set_scalar(&self, register: Register, value: u32) -> Result<(), String> {
        let mut size = 4;
        check(&self.api, unsafe {
            (self.api.uc_context_reg_write2)(
                self.raw,
                register_id(register)?,
                value.to_le_bytes().as_ptr().cast(),
                &mut size,
            )
        })
    }
}
impl Core {
    fn cp15(
        &self,
        selector: (u32, u32, u32, u32),
        secure: u32,
        value: Option<u64>,
    ) -> Result<u64, String> {
        let (opc1, crn, crm, opc2) = selector;
        self.coprocessor(
            arm::Coprocessor {
                cp: 15,
                opc1,
                crn,
                crm,
                opc2,
                secure,
                wide: false,
            },
            value,
        )
    }
    pub(crate) fn architecture_state(&self) -> Result<Value, String> {
        if self.model != Model::CortexR5F {
            return Err("R-profile architecture inspection requires R5F".into());
        }
        let saved = self.snapshot(None)?;
        let context = saved
            .as_any()
            .downcast_ref::<Context>()
            .ok_or("missing Unicorn context")?;
        let cpsr = context.scalar(arm::CPSR)?;
        let mut result = json!({"cpsr":cpsr, "registers":(0..16).map(|i|context.read(arm::CORE[i])).collect::<Result<Vec<_>,_>>()?,
            "cpacr":context.read(Register::Arm32(arm::Register::Cpacr))?,
            "fpexc":context.read(Register::Arm32(arm::Register::Fpexc))?,
            "fpscr":context.read(Register::Arm32(arm::Register::Fpscr))?,
            "q":(0..16).map(|i| context.read(Register::Arm32(arm::Register::Q(i)))).collect::<Result<Vec<_>,_>>()?,
            "banked":{}, "cp15":{}});
        for mode in [0x10, 0x11, 0x12, 0x13, 0x17, 0x1b, 0x1f] {
            context.set_scalar(arm::CPSR, cpsr & !31 | mode)?;
            result["banked"][format!("{mode:x}")] = json!({
                "r8_r14":(8..15).map(|i|context.read(arm::CORE[i])).collect::<Result<Vec<_>,_>>()?,
                "spsr":if matches!(mode,0x10|0x1f) { None } else { Some(context.scalar(arm::SPSR)?) }});
        }
        context.set_scalar(arm::CPSR, cpsr)?;
        // Pinned whitelist from the retained R5F architectural export. No
        // probing of undefined coprocessor selectors is permitted.
        for (name, selector) in [
            ("sctlr", (0, 1, 0, 0)),
            ("dacr", (0, 3, 0, 0)),
            ("csselr", (2, 0, 0, 0)),
            ("dfsr", (0, 5, 0, 0)),
            ("ifsr", (0, 5, 0, 1)),
            ("dfar", (0, 6, 0, 0)),
            ("ifar", (0, 6, 0, 2)),
            ("vbar", (0, 12, 0, 0)),
            ("fcseidr", (0, 13, 0, 0)),
            ("contextidr", (0, 13, 0, 1)),
            ("tpidrurw", (0, 13, 0, 2)),
            ("tpidruro", (0, 13, 0, 3)),
            ("tpidrprw", (0, 13, 0, 4)),
            ("mair0", (0, 10, 2, 0)),
            ("mair1", (0, 10, 2, 1)),
            ("par", (0, 7, 4, 0)),
        ] {
            result["cp15"][name] =
                json!([self.cp15(selector, 0, None)?, self.cp15(selector, 1, None)?]);
        }
        let selected = self.cp15((0, 6, 2, 0), 0, None)?;
        // RGNR is an architectural debug selector, not a device access.
        // Raw Unicorn access neither executes instructions nor touches the
        // execution owner's PMU, clock or exclusive monitor. Always put the
        // original selector back, including when a region read is refused.
        let regions = (|| -> Result<Vec<Value>, String> {
            let mut regions = vec![];
            for index in 0..16 {
                self.cp15((0, 6, 2, 0), 0, Some(index))?;
                regions.push(json!([
                    self.cp15((0, 6, 1, 0), 0, None)?,
                    self.cp15((0, 6, 1, 2), 0, None)?,
                    self.cp15((0, 6, 1, 4), 0, None)?
                ]));
            }
            Ok(regions)
        })();
        self.cp15((0, 6, 2, 0), 0, Some(selected))?;
        let regions = regions?;
        result["mpu"] = json!({"selected":selected,"regions":regions});
        Ok(result)
    }
}
