//! TMD2772-class ambient-light and proximity sensor (AMS TMD2772 register map),
//! as an eye sensor beside a viewfinder.
//!
//! Command bytes carry the register in bits 4:0 (bit 7 marks a command, bits
//! 6:5 the transaction type). Written configuration is kept and reads back.
//! With a wired interrupt, a raw external sample feeds the proximity ADC and
//! its programmable threshold and persistence logic. Ambient channels are
//! dark. Timing follows the proximity-only cycle in the TMD2772 datasheet
//! (ams DS000310, Fig. 22), with quiescent conversions skipped.
use crate::i2c::Registers;
use fujimod_emulator_devices::{
    Cx, capability::ProximityInput, checkpoint::Checkpoint, parts::i2c::TARGET_EVENT,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proximity {
    registers: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input: Option<Input>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    irq: u32,
    raw: u16,
    sample: Option<u16>,
    outside: u8,
    pending: bool,
    asserted: bool,
    due: Option<u64>,
}

const ENABLE: usize = 0x00;
const PON: u8 = 1 << 0;
const PEN: u8 = 1 << 2;
const WEN: u8 = 1 << 3;
const PIEN: u8 = 1 << 5;
const MIN_STEP_NS: u64 = 2_730_000;
const MAX_SAMPLE: u16 = 1023;
impl Default for Proximity {
    fn default() -> Self {
        Self {
            registers: vec![0; 0x20],
            input: None,
        }
    }
}
impl Proximity {
    pub fn with_interrupt(irq: u32) -> Self {
        let mut device = Self::default();
        device.registers[0x02..=0x03].fill(0xff);
        device.input = Some(Input {
            irq,
            raw: 0,
            sample: None,
            outside: 0,
            pending: false,
            asserted: false,
            due: None,
        });
        device
    }

    const ID: u8 = 0x12;
    const STATUS: u8 = 0x13;

    fn advance(&mut self, cx: &mut Cx<'_>, conversion: bool) {
        let Some(input) = self.input.as_mut() else {
            return;
        };
        let regs = &self.registers;
        let enable = regs[ENABLE];
        if enable & (PON | PEN) != (PON | PEN) {
            input.sample = None;
            input.outside = 0;
            input.due = None;
        } else {
            let low = u16::from_le_bytes([regs[0x08], regs[0x09]]);
            let high = u16::from_le_bytes([regs[0x0a], regs[0x0b]]);
            let persistence = regs[0x0c] >> 4;
            let outside = persistence == 0 || input.raw < low || (low <= high && input.raw > high);
            if conversion && input.due.is_some_and(|due| due <= cx.now()) {
                input.due = None;
                input.sample = Some(input.raw);
                if outside && !input.pending {
                    input.outside += 1;
                    input.pending = input.outside >= persistence;
                }
                if !outside || input.pending {
                    input.outside = 0;
                }
            }
            // Identical conversions in range do not change any visible state.
            let needs_sample = input.sample != Some(input.raw)
                || (!input.pending && (outside || input.outside != 0));
            if !needs_sample {
                input.due = None;
            } else if input.due.is_none() {
                let mut steps = 2 + (256 - u64::from(regs[0x02]));
                if enable & WEN != 0 {
                    let scale = if regs[0x0d] & 2 != 0 { 12 } else { 1 };
                    steps += scale * (256 - u64::from(regs[0x03]));
                }
                let due = cx.now() + steps * MIN_STEP_NS + 16_000 * u64::from(regs[0x0e]);
                input.due = Some(due);
                cx.schedule(due, TARGET_EVENT);
            }
        }
        let asserted = input.pending && enable & (PON | PEN | PIEN) == (PON | PEN | PIEN);
        if asserted != input.asserted {
            input.asserted = asserted;
            cx.set_irq(input.irq, asserted);
        }
    }

    fn validate_state(&self, state: &Self) -> Result<(), String> {
        if state.registers.len() != 0x20
            || self.input.as_ref().map(|input| input.irq)
                != state.input.as_ref().map(|input| input.irq)
            || state.input.as_ref().is_some_and(|input| {
                input.raw > MAX_SAMPLE
                    || input.sample.is_some_and(|value| value > MAX_SAMPLE)
                    || input.outside >= 15
                    || (input.asserted && !input.pending)
            })
        {
            return Err("invalid TMD2772 checkpoint".into());
        }
        Ok(())
    }
}
impl Registers for Proximity {
    fn name(&self) -> &'static str {
        "tmd2772-proximity"
    }
    fn select(&mut self, byte: u8) -> u8 {
        if let (0xe5 | 0xe7, Some(input)) = (byte, self.input.as_mut()) {
            input.pending = false;
            input.outside = 0;
        }
        byte & 0x1f
    }
    fn proximity_input(&self) -> Option<&(dyn ProximityInput + 'static)> {
        self.input.as_ref().map(|_| self as &dyn ProximityInput)
    }
    fn proximity_input_mut(&mut self) -> Option<&mut (dyn ProximityInput + 'static)> {
        if self.input.is_some() {
            Some(self)
        } else {
            None
        }
    }
    fn update(&mut self, cx: &mut Cx<'_>, conversion: bool) {
        self.advance(cx, conversion);
    }
    fn write(&mut self, register: u8, value: u8) -> Result<(), String> {
        if let Some(slot) = self.registers.get_mut(register as usize) {
            *slot = value;
        }
        Ok(())
    }
    fn read(&mut self, register: u8) -> Result<u8, String> {
        Ok(match register {
            Self::ID => 0x39,
            Self::STATUS => self.input.as_ref().map_or(0x03, |input| {
                (u8::from(input.sample.is_some()) << 1) | (u8::from(input.pending) << 5)
            }),
            0x14..=0x17 => 0,
            0x18..=0x19 => self
                .input
                .as_ref()
                .and_then(|input| input.sample)
                .unwrap_or(0)
                .to_le_bytes()[usize::from(register - 0x18)],
            r => self.registers.get(r as usize).copied().unwrap_or(0),
        })
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), self)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: Self = saved.decode(self.name())?;
        self.validate_state(&state)
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        *self = saved.decode(self.name())?;
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        let restored: Self = serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        self.validate_state(&restored)?;
        *self = restored;
        Ok(())
    }
}

impl ProximityInput for Proximity {
    fn admit_sample(&self, _: u8, sample: u16) -> Result<(), String> {
        if self.input.is_none() || sample > MAX_SAMPLE {
            return Err("TMD2772 needs a wired input and a 10-bit proximity sample".into());
        }
        Ok(())
    }

    fn set_sample(&mut self, cx: &mut Cx<'_>, address: u8, sample: u16) -> Result<(), String> {
        self.admit_sample(address, sample)?;
        let input = self.input.as_mut().unwrap();
        if input.raw != sample {
            input.raw = sample;
            self.advance(cx, false);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_sensor_reports_identity_and_an_empty_finder() {
        let mut sensor = Proximity::default();
        // Command bytes are 0xA0 | register (auto-increment type).
        assert_eq!(sensor.select(0xa0 | 0x12), 0x12);
        assert_eq!(sensor.read(0x12).unwrap(), 0x39);
        assert_eq!(
            sensor.read(0x13).unwrap() & 0x22,
            0x02,
            "PVALID set, PINT clear"
        );
        assert_eq!(
            [sensor.read(0x18).unwrap(), sensor.read(0x19).unwrap()],
            [0, 0]
        );
        // Configuration writes are retained and read back.
        sensor.write(0x0e, 0x0c).unwrap();
        assert_eq!(sensor.read(0x0e).unwrap(), 0x0c);
    }

    #[test]
    fn timed_samples_drive_threshold_interrupts_and_replay_a_pending_conversion() {
        use fujimod_emulator_devices::test_host::recording::RecordingHost;

        let mut host = RecordingHost::default();
        let mut sensor = Proximity::with_interrupt(122);
        // Await two consecutive readings above 662.
        for (register, value) in [(0x0a, 150), (0x0b, 2), (0x0c, 0x20)] {
            sensor.write(register, value).unwrap();
        }
        let cx = &mut Cx::new(&mut host, 0, 0, 2);
        assert!(sensor.set_sample(cx, 0x39, 1024).is_err());
        sensor.set_sample(cx, 0x39, 1023).unwrap();
        assert!(host.events.is_empty(), "conversion disabled");
        assert_eq!(sensor.read(0x13).unwrap(), 0);

        sensor.write(0, PON | PEN | PIEN).unwrap();
        sensor.update(&mut Cx::new(&mut host, 0, 0, 2), false);
        assert_eq!(host.events, [(2, 8_190_000, TARGET_EVENT)]);
        host.now = 8_189_999;
        sensor.update(&mut Cx::new(&mut host, 0, 0, 2), true);
        assert_eq!(sensor.read(0x13).unwrap(), 0);
        host.now += 1;
        sensor.update(&mut Cx::new(&mut host, 0, 0, 2), true);
        assert_eq!(sensor.read(0x13).unwrap(), 2);
        assert!(host.irqs.is_empty());
        assert_eq!(host.events[1], (2, 16_380_000, TARGET_EVENT));

        let saved = sensor.checkpoint().unwrap();
        let mut restored = Proximity::with_interrupt(122);
        restored.restore_checkpoint(&saved).unwrap();
        assert!(
            Proximity::with_interrupt(123)
                .validate_checkpoint(&saved)
                .is_err()
        );
        host.now = 16_380_000;
        let mut replay_host = RecordingHost {
            now: host.now,
            ..Default::default()
        };
        for (sensor, host) in [(&mut restored, &mut host), (&mut sensor, &mut replay_host)] {
            sensor.update(&mut Cx::new(host, 0, 0, 2), true);
            assert_eq!(host.irqs, [(122, true)]);
            assert_eq!(sensor.read(0x13).unwrap(), 0x22);
            assert_eq!(
                [sensor.read(0x18).unwrap(), sensor.read(0x19).unwrap()],
                [0xff, 3]
            );
            // The guest rearms the low threshold when it sees a near sample.
            for (register, value) in [(0x08, 176), (0x09, 1), (0x0a, 0xff), (0x0b, 0xff)] {
                sensor.write(register, value).unwrap();
            }
            sensor.select(0xe5);
            sensor.update(&mut Cx::new(host, 0, 0, 2), false);
            assert_eq!(host.irqs, [(122, true), (122, false)]);
            sensor
                .set_sample(&mut Cx::new(host, 0, 0, 2), 0x39, 0)
                .unwrap();
            for _ in 0..2 {
                host.now += 8_190_000;
                sensor.update(&mut Cx::new(host, 0, 0, 2), true);
            }
            assert_eq!(host.irqs, [(122, true), (122, false), (122, true)]);
            assert_eq!(sensor.read(0x18).unwrap(), 0);
        }
        assert_eq!(sensor.checkpoint().unwrap(), restored.checkpoint().unwrap());
    }
}
