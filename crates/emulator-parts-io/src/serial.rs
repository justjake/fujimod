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
impl Wire for Dedicated {
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

/// A single hardwired serial slave with no chip-select line: the controller's
/// native frame select is always asserted, so every frame reaches the one
/// endpoint. For a bus carrying exactly one device addressed in-band by the
/// clocked command (gfx100rf SPI instance 2's config ROM), where the driver
/// toggles no GPIO select, unlike a multiplexed GPIO-selected bus.
pub struct Direct {
    endpoint: Box<dyn Endpoint>,
    generation: u64,
    active: Option<(u64, Frame)>,
}
impl Direct {
    pub fn new(endpoint: Box<dyn Endpoint>) -> Self {
        Self {
            endpoint,
            generation: 0,
            active: None,
        }
    }
}
impl Wire for Direct {
    fn begin(&mut self, frame: Frame) -> Result<u64, String> {
        if self.active.is_some() {
            return Err("serial wire already shifting".into());
        }
        self.endpoint.validate(frame)?;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("serial token overflow")?;
        self.active = Some((self.generation, frame));
        Ok(self.generation)
    }
    fn finish(&mut self, token: u64, frame: Frame, now: u64) -> Result<u16, String> {
        let Some((generation, active)) = self.active else {
            return Err("serial wire has no active frame".into());
        };
        if token != generation || frame != active {
            return Err("serial frame token differs".into());
        }
        self.active = None;
        Ok(self.endpoint.transfer(frame, now))
    }
    fn save(&self) -> Value {
        serde_json::json!({
            "kind": "direct",
            "generation": self.generation,
            "active": self.active.map(|(t, f)| serde_json::json!({"token": t, "frame": f})),
            "endpoint": {"kind": self.endpoint.name(), "state": self.endpoint.save()},
        })
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        if state["kind"] != "direct" || state["endpoint"]["kind"] != self.endpoint.name() {
            return Err("direct serial wire layout differs".into());
        }
        let generation = state["generation"]
            .as_u64()
            .ok_or("direct serial wire generation missing")?;
        let active = if state["active"].is_null() {
            None
        } else {
            let token = state["active"]["token"]
                .as_u64()
                .ok_or("direct serial wire active token missing")?;
            let frame: Frame = serde_json::from_value(state["active"]["frame"].clone())
                .map_err(|e| e.to_string())?;
            if generation == 0 || token != generation {
                return Err("invalid direct serial wire active frame".into());
            }
            self.endpoint.validate(frame)?;
            Some((token, frame))
        };
        self.endpoint.restore(&state["endpoint"]["state"])?;
        self.generation = generation;
        self.active = active;
        Ok(())
    }
}
