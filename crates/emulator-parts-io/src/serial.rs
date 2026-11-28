//! Wire frames delivered by a serial controller to a selected attached part.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Frame {
    pub bits: u8,
    /// Motorola SPI mode: bit0 CPHA, bit1 CPOL.
    pub mode: u8,
    pub word: u16,
}

pub trait Endpoint: Send {
    fn name(&self) -> &'static str;
    /// Validate before the controller accepts a transfer. No state changes.
    fn validate(&self, frame: Frame) -> Result<(), String>;
    fn reset_transaction(&mut self);
    /// The last serial clock edge arrived while chip select remained active.
    fn transfer(&mut self, frame: Frame, now_ns: u64) -> u16;
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Err(format!(
            "{} has no exact checkpoint implementation",
            self.name()
        ))
    }
    fn validate_checkpoint(&self, _: &Checkpoint) -> Result<(), String> {
        Err(format!(
            "{} has no exact checkpoint implementation",
            self.name()
        ))
    }
    fn restore_checkpoint(&mut self, _: &Checkpoint) -> Result<(), String> {
        Err(format!(
            "{} has no exact checkpoint implementation",
            self.name()
        ))
    }
    fn save(&self) -> Value;
    fn restore(&mut self, state: &Value) -> Result<(), String>;
    /// An external line that sets bits of one part register (for example a
    /// cable's supply detection). Parts without such inputs refuse it.
    fn set_input_bits(&mut self, _register: u8, _mask: u16, _value: u16) -> Result<(), String> {
        Err(format!("{} has no external register inputs", self.name()))
    }
    /// The current value of a register external lines drive.
    fn input_bits(&self, _register: u8) -> Result<u16, String> {
        Err(format!("{} has no external register inputs", self.name()))
    }
    /// Refuse a dial turn the part cannot count, without counting it.
    fn admit_steps(&self, _steps: i32) -> Result<(), String> {
        Err(format!("{} counts no dial", self.name()))
    }
    /// Count dial detents the guest reads later.
    fn count_steps(&mut self, _steps: i32) -> Result<(), String> {
        Err(format!("{} counts no dial", self.name()))
    }
}

/// A wire transaction may latch chip selection at the first serial edge.
/// Tokens contain physical wire state, never firmware or caller identities.
pub trait Wire: Send {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Err("serial wire has no exact checkpoint implementation".into())
    }
    fn validate_checkpoint(
        &self,
        _: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        Err("serial wire has no exact checkpoint implementation".into())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)
    }
    fn validate_pending(
        &self,
        _: &fujimod_emulator_devices::checkpoint::Checkpoint,
        _: u64,
        _: Frame,
    ) -> Result<(), String> {
        Err("serial wire has no pending-frame checkpoint implementation".into())
    }

    fn begin(&mut self, frame: Frame) -> Result<u64, String>;
    fn finish(&mut self, token: u64, frame: Frame, now: u64) -> Result<u16, String>;
    fn save(&self) -> Value;
    /// Validate before changing state; restoration does not perform transfers.
    fn restore(&mut self, state: &Value) -> Result<(), String>;
}
fn unconnected(frame: Frame) -> String {
    format!(
        "serial wire input is unconnected ({}-bit mode {} word {:#x})",
        frame.bits, frame.mode, frame.word
    )
}
pub struct Unconnected;
impl Wire for Unconnected {
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode("unconnected-serial-wire", &())
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        saved.decode::<()>("unconnected-serial-wire")?;
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("unconnected serial wire checkpoint differs".into());
        }
        Ok(())
    }
    // An unconnected wire refuses every frame, so none can be pending.
    fn begin(&mut self, frame: Frame) -> Result<u64, String> {
        Err(unconnected(frame))
    }
    fn finish(&mut self, _: u64, frame: Frame, _: u64) -> Result<u16, String> {
        Err(unconnected(frame))
    }
    fn save(&self) -> Value {
        Value::Null
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        if state.is_null() {
            Ok(())
        } else {
            Err("invalid unconnected serial wire state".into())
        }
    }
}

/// A wire whose one part is always selected: the link has no chip select.
pub struct Dedicated(pub Box<dyn Endpoint>);
impl Dedicated {
    fn part_checkpoint<'a>(&self, saved: &'a Checkpoint) -> Result<&'a Checkpoint, String> {
        let part: String = saved.decode("dedicated-serial-wire")?;
        match saved.children() {
            [child] if part == self.0.name() && saved.blobs().is_empty() => Ok(child),
            _ => Err("dedicated serial wire differs from checkpoint".into()),
        }
    }
}
impl Wire for Dedicated {
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(Checkpoint::encode("dedicated-serial-wire", &self.0.name())?
            .with_children(vec![self.0.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        self.0.validate_checkpoint(self.part_checkpoint(saved)?)
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        let part = self.part_checkpoint(saved)?;
        self.0.validate_checkpoint(part)?;
        self.0.restore_checkpoint(part)
    }
    fn validate_pending(&self, saved: &Checkpoint, token: u64, frame: Frame) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        if token != 0 {
            return Err("invalid dedicated pending serial token".into());
        }
        self.0.validate(frame)
    }
    fn begin(&mut self, frame: Frame) -> Result<u64, String> {
        self.0.validate(frame)?;
        Ok(0)
    }
    fn finish(&mut self, token: u64, frame: Frame, now: u64) -> Result<u16, String> {
        if token != 0 {
            return Err("invalid dedicated serial token".into());
        }
        Ok(self.0.transfer(frame, now))
    }
    fn save(&self) -> Value {
        serde_json::json!({"kind": "dedicated", "part": self.0.name(), "state": self.0.save()})
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        if state["kind"] != "dedicated" || state["part"] != self.0.name() {
            return Err("dedicated serial wire differs from checkpoint".into());
        }
        self.0.restore(&state["state"])
    }
}

/// An explicitly fixed electrical input for a serial-wire scenario. This is
/// neither a peripheral protocol nor a response chosen from transmitted data.
pub struct InputLevel(pub bool);
impl Wire for InputLevel {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode("fixed-serial-input", &self.0)
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let high: bool = saved.decode("fixed-serial-input")?;
        if high != self.0 || !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("fixed serial input checkpoint differs".into());
        }
        Ok(())
    }
    fn validate_pending(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
        token: u64,
        frame: Frame,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        if token != 0 || !(4..=16).contains(&frame.bits) || frame.mode > 3 {
            return Err("invalid fixed-input pending serial frame".into());
        }
        Ok(())
    }

    fn begin(&mut self, frame: Frame) -> Result<u64, String> {
        if !(4..=16).contains(&frame.bits) || frame.mode > 3 {
            return Err("unsupported fixed-input serial frame".into());
        }
        Ok(0)
    }
    fn finish(&mut self, token: u64, frame: Frame, _: u64) -> Result<u16, String> {
        self.begin(frame)?;
        if token != 0 {
            return Err("invalid fixed-input serial token".into());
        }
        Ok(if self.0 {
            u16::MAX >> (16 - frame.bits)
        } else {
            0
        })
    }
    fn save(&self) -> Value {
        serde_json::json!({"kind":"fixed-input","high":self.0})
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        if state == &self.save() {
            Ok(())
        } else {
            Err("fixed serial input differs from checkpoint".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A part whose whole state is the count of words it has received.
    struct Counter(u32);
    impl Endpoint for Counter {
        fn name(&self) -> &'static str {
            "counter"
        }
        fn validate(&self, frame: Frame) -> Result<(), String> {
            if frame.bits == 8 {
                Ok(())
            } else {
                Err("counter takes 8-bit frames".into())
            }
        }
        fn reset_transaction(&mut self) {}
        fn transfer(&mut self, _: Frame, _: u64) -> u16 {
            self.0 += 1;
            self.0 as u16
        }
        fn checkpoint(&self) -> Result<Checkpoint, String> {
            Checkpoint::encode("counter", &self.0)
        }
        fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
            saved.decode::<u32>("counter").map(drop)
        }
        fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
            self.0 = saved.decode("counter")?;
            Ok(())
        }
        fn save(&self) -> Value {
            Value::from(self.0)
        }
        fn restore(&mut self, _: &Value) -> Result<(), String> {
            Ok(())
        }
    }

    const BYTE: Frame = Frame {
        bits: 8,
        mode: 0,
        word: 0,
    };

    #[test]
    fn an_unconnected_wire_checkpoints_and_refuses_pending_frames() {
        let saved = Unconnected.checkpoint().unwrap();
        Unconnected.validate_checkpoint(&saved).unwrap();
        Unconnected.restore_checkpoint(&saved).unwrap();
        assert!(Unconnected.validate_pending(&saved, 0, BYTE).is_err());
        let other = InputLevel(true).checkpoint().unwrap();
        assert!(Unconnected.validate_checkpoint(&other).is_err());
    }

    #[test]
    fn a_dedicated_wire_checkpoints_its_part() {
        let mut wire = Dedicated(Box::new(Counter(0)));
        let token = wire.begin(BYTE).unwrap();
        wire.finish(token, BYTE, 0).unwrap();
        let saved = wire.checkpoint().unwrap();
        let mut copy = Dedicated(Box::new(Counter(0)));
        copy.restore_checkpoint(&saved).unwrap();
        assert!(copy.checkpoint().unwrap().diff(&saved).is_none());
        copy.validate_pending(&saved, 0, BYTE).unwrap();
        assert!(copy.validate_pending(&saved, 1, BYTE).is_err());
        let wide = Frame { bits: 16, ..BYTE };
        assert!(copy.validate_pending(&saved, 0, wide).is_err());
        assert!(Unconnected.validate_checkpoint(&saved).is_err());
        assert!(
            wire.validate_checkpoint(&Unconnected.checkpoint().unwrap())
                .is_err()
        );
    }
}
