//! Keys whose GPIO inputs also reach external interrupt lines.
//!
//! The X-T20 2.11 key scan (section 5 0xc8d5c8) evaluates kind 4 rows, the
//! shutter button's S1 and S2 (GPIO bank 0 bits 0 and 1, low when pressed),
//! only while their reported state is pressed: it reports the release. The
//! press reaches the UI through the rows' external interrupts, whose handler
//! 0xc8d0e0 toggles the row and calls key_event(index, 1) (0xc8cee4). The
//! rows name interrupt rows 20 and 21 (+0x12), which 0xed4720 maps to GIC 151
//! and 152, external lines 0 and 1.
//!
//! Each connected line is driven at its pin's level, inverted by the line's
//! polarity bit, once per poll period, so an input set from outside raises
//! the interrupt the firmware has armed. The sampling period is a model
//! choice.
use crate::resets::{ExternalLines, LINES};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_ip::socionext::gpio::SharedPins;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    poll_at: u64,
}
pub struct KeyLines {
    lines: ExternalLines,
    poll: u64,
    state: State,
}
impl KeyLines {
    /// Connect each (line, bank, pin) of `keys` to `lines`.
    pub fn new(
        keys: &[(usize, usize, usize)],
        poll: u64,
        pins: SharedPins,
        lines: ExternalLines,
    ) -> Result<Self, String> {
        if keys.is_empty() || poll == 0 || keys.iter().any(|&(line, ..)| line >= LINES) {
            return Err("Invalid key lines".into());
        }
        for &(line, bank, pin) in keys {
            lines.connect(pins.clone(), line, bank, pin)?;
        }
        Ok(Self {
            lines,
            poll,
            state: State { poll_at: poll },
        })
    }
}
impl Device for KeyLines {
    fn name(&self) -> &'static str {
        "xpp-key-lines"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _address: u64, _size: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _address: u64, _size: u8, _value: u64) {
        cx.unmodeled();
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        if now >= self.state.poll_at {
            self.state.poll_at = (now / self.poll + 1) * self.poll;
            self.lines.update(cx);
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        Some(self.state.poll_at)
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        Ok(())
    }
    fujimod_emulator_devices::checkpoint_state!(state: State, Self::check);
}
impl KeyLines {
    fn check(&self, state: &State) -> Result<(), String> {
        if state.poll_at % self.poll != 0 {
            return Err("Invalid key lines checkpoint".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resets::{FIRST_LINE_IRQ, Gates};
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    use fujimod_emulator_ip::socionext::gpio::Pins;

    fn at(keys: &mut KeyLines, h: &mut RecordingHost, now: u64) {
        h.now = now;
        keys.advance(&mut Cx::new(h, 0, 0, 0));
    }

    #[test]
    fn a_pressed_key_drives_its_external_line_at_the_next_poll() {
        let pins = Pins::new([0b11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let gates = Gates::new(0xfff0_a000).unwrap();
        let mut keys = KeyLines::new(
            &[(0, 0, 0), (1, 0, 1)],
            10,
            pins.clone(),
            gates.external_lines(),
        )
        .unwrap();
        let mut h = RecordingHost::default();
        // Released keys read high: both lines rise at the first poll.
        at(&mut keys, &mut h, 10);
        assert_eq!(h.irqs, [(FIRST_LINE_IRQ, true), (FIRST_LINE_IRQ + 1, true)]);
        // S1 pressed (bank 0 bit 0 low) between polls: line 0 falls at the next one.
        pins.lock().unwrap().set_input(0, 0, false);
        at(&mut keys, &mut h, 15);
        assert_eq!(h.irqs.len(), 2);
        at(&mut keys, &mut h, 20);
        assert_eq!(h.irqs[2..], [(FIRST_LINE_IRQ, false)]);
        assert_eq!(keys.next_deadline(20), Some(30));
        let saved = keys.save();
        keys.restore(&saved).unwrap();
        assert!(KeyLines::new(&[(0, 0, 0)], 10, pins, gates.external_lines()).is_err());
    }
}
