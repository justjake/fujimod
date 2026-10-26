//! Wire frames delivered by a serial controller to a selected attached part.
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
    fn save(&self) -> Value;
    fn restore(&mut self, state: &Value) -> Result<(), String>;
}

/// A wire transaction may latch chip selection at the first serial edge.
/// Tokens contain physical wire state, never firmware or caller identities.
pub trait Wire: Send {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Err("serial wire has no exact checkpoint implementation".into())
    }
    fn validate_checkpoint(&self, _: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        Err("serial wire has no exact checkpoint implementation".into())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
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
pub struct Unconnected;
impl Wire for Unconnected {
    fn begin(&mut self, _: Frame) -> Result<u64, String> {
        Err("serial wire input is unconnected".into())
    }
    fn finish(&mut self, _: u64, _: Frame, _: u64) -> Result<u16, String> {
        Err("serial wire input is unconnected".into())
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

/// An explicitly fixed electrical input for a serial-wire scenario. This is
/// neither a peripheral protocol nor a response chosen from transmitted data.
pub struct InputLevel(pub bool);
impl Wire for InputLevel {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode("fixed-serial-input", &self.0)
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
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
