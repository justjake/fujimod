//! FFF8A000 converter shared by every execution frontend. Register writes
//! select external levels and START schedules a conversion. Its due completion
//! publishes results and the done mask. Control +4 bit3 selects continuous
//! input: after startup, result reads sample the selected external levels.
//! One-shot conversions retain their START-time samples.
use fujimod_emulator_devices::{Cx, Device, parts::analog::Samples};
use serde::{Deserialize, Serialize};

pub const BASE: u64 = super::BASE as u64;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub conversion_ticks: u64,
    pub samples: [u16; 12],
}
#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    at: u64,
    channels: u32,
    samples: [u16; 12],
}
#[derive(Clone, Serialize, Deserialize)]
struct State {
    registers: [u32; 12],
    results: [u16; 12],
    pending: Option<Pending>,
}
pub struct Converter {
    delay: u64,
    state: State,
    external: Samples,
}
impl Converter {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.conversion_ticks == 0 {
            return Err("ADC conversion delay must be positive".into());
        }
        Ok(Self {
            delay: options.conversion_ticks,
            external: Samples::new(options.samples.to_vec())?,
            state: State {
                registers: [0; 12],
                results: options.samples,
                pending: None,
            },
        })
    }
    pub fn samples(&self) -> Vec<u16> {
        self.external.values()
    }
    pub fn external_inputs(&self) -> Samples {
        self.external.clone()
    }
    pub fn set_sample(&mut self, channel: usize, value: u16) -> Result<(), String> {
        self.external.set(channel, value)
    }
    fn offset(address: u64, size: u8) -> Option<usize> {
        let relative = address.checked_sub(BASE)?;
        if relative >= 0x60 {
            return None;
        }
        let offset = relative as usize;
        (size == 4
            && (matches!(offset, 0 | 4 | 8 | 12 | 0x24 | 0x28)
                || (0x30..0x60).contains(&offset) && offset.is_multiple_of(4)))
        .then_some(offset)
    }
    fn store(&mut self, now: u64, offset: usize, value: u32) -> Result<(), String> {
        if offset >= 0x30 {
            return Err("ADC result registers are read-only".into());
        }
        if offset == 0x24 {
            self.state.registers[9] &= !value;
            return Ok(());
        }
        if offset == 0 {
            match value {
                0x8000 => {
                    self.state.pending = None;
                    self.state.registers[9] = 0;
                }
                0x8004 => (),
                0x8005 => {
                    if self.state.pending.is_some() {
                        return Err("ADC start while busy".into());
                    }
                    let at = now.checked_add(self.delay).ok_or("ADC deadline overflow")?;
                    self.state.pending = Some(Pending {
                        at,
                        channels: self.state.registers[2] & 0xfff,
                        samples: self.samples().try_into().unwrap(),
                    });
                    self.state.registers[9] = 0;
                }
                _ => return Err(format!("unsupported ADC control {value:#x}")),
            }
        }
        self.state.registers[offset / 4] = value;
        Ok(())
    }
}
impl Device for Converter {
    fn analog_inputs(&self) -> Option<&(dyn fujimod_emulator_devices::capability::AnalogInputs + 'static)> {
        Some(self)
    }
    fn analog_inputs_mut(&mut self) -> Option<&mut (dyn fujimod_emulator_devices::capability::AnalogInputs + 'static)> {
        Some(self)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &(&self.state, self.samples()))
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (_, samples): (State, Vec<u16>) = saved.decode(self.name())?;
        if samples.len() != self.samples().len() {
            return Err("ADC snapshot channel count differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (state, samples): (State, Vec<u16>) = saved.decode(self.name())?;
        self.external.replace(&samples)?;
        self.state = state;
        Ok(())
    }
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-adc"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match Self::offset(address, size) {
            Some(offset) if offset >= 0x30 => {
                let channel = (offset - 0x30) / 4;
                // Continuous-input mode samples selected external levels after
                // startup. One-shot conversions retain their START-time sample.
                if self.state.registers[0] == 0x8005
                    && self.state.registers[1] & 8 != 0
                    && self.state.pending.is_none()
                    && self.state.registers[2] & (1 << channel) != 0
                {
                    self.external
                        .get(channel)
                        .expect("converter has twelve external channels") as u64
                } else {
                    self.state.results[channel] as u64
                }
            }
            Some(offset) => self.state.registers[offset / 4] as u64,
            None => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        match (Self::offset(address, size), u32::try_from(value)) {
            (Some(offset), Ok(word)) => {
                if let Err(error) = self.store(cx.now(), offset, word) {
                    cx.fail(error);
                }
            }
            _ => cx.unmodeled(),
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::offset(address, size).is_some()
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.pending.as_ref().map(|p| p.at)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self
            .state
            .pending
            .as_ref()
            .is_some_and(|p| p.at <= cx.now())
        {
            return;
        }
        let pending = self.state.pending.take().unwrap();
        for channel in 0..12 {
            if pending.channels & (1 << channel) != 0 {
                self.state.results[channel] = pending.samples[channel];
            }
        }
        self.state.registers[9] = pending.channels;
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"state":self.state,"external":self.external.values()})
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State =
            serde_json::from_value(value["state"].clone()).map_err(|e| e.to_string())?;
        let external: Vec<u16> =
            serde_json::from_value(value["external"].clone()).map_err(|e| e.to_string())?;
        self.external.replace(&external)?;
        self.state = state;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};
    struct Clock(u64);
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("ADC uses native deadlines")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("ADC has no IRQ output")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }
    #[test]
    fn continuous_mode_reads_menu_press_and_release_after_one_start_without_host_read_effects() {
        let mut adc = Converter::new(Options {
            conversion_ticks: 4,
            samples: [1023; 12],
        })
        .unwrap();
        adc.store(10, 4, 8).unwrap();
        adc.store(10, 8, 0x3ff).unwrap();
        adc.store(10, 0, 0x8005).unwrap();
        adc.advance(&mut Cx::new(&mut Clock(14), 0, 0, 0));
        adc.store(14, 0x24, 0x3ff).unwrap();
        for value in [837, 1023] {
            adc.external.set(8, value).unwrap();
            let before = adc.save();
            assert_eq!(
                adc.read(&mut Cx::new(&mut Clock(15), 0, 0, 0), BASE + 0x50, 4),
                value as u64
            );
            assert_eq!(adc.save(), before);
            assert_eq!(adc.next_deadline(15), None);
        }
    }
    #[test]
    fn conversion_latches_external_levels_and_finishes_only_at_its_exact_tick() {
        let mut adc = Converter::new(Options {
            conversion_ticks: 7,
            samples: [123; 12],
        })
        .unwrap();
        adc.store(10, 8, (1 << 2) | (1 << 8)).unwrap();
        adc.store(10, 0, 0x8005).unwrap();
        adc.set_sample(8, 987).unwrap();
        assert_eq!(adc.next_deadline(10), Some(17));
        let saved = adc.save();
        adc.advance(&mut Cx::new(&mut Clock(16), 0, 0, 0));
        assert_eq!(adc.save(), saved);
        adc.advance(&mut Cx::new(&mut Clock(17), 0, 0, 0));
        assert_eq!(adc.state.results[8], 123);
        assert_eq!(adc.samples()[8], 987);
        assert_eq!(adc.state.registers[9], 0x104);
        assert_eq!(adc.next_deadline(17), None);
        adc.restore(&saved).unwrap();
        adc.store(16, 0, 0x8000).unwrap();
        adc.advance(&mut Cx::new(&mut Clock(17), 0, 0, 0));
        assert_eq!(adc.state.registers[9], 0);
    }
}

impl fujimod_emulator_devices::capability::AnalogInputs for Converter {
    fn external_inputs(&self) -> fujimod_emulator_devices::parts::analog::Samples {
        Converter::external_inputs(self)
    }
    fn samples(&self) -> Vec<u16> {
        Converter::samples(self)
    }
    fn set_sample(&mut self, channel: usize, value: u16) -> std::result::Result<(), String> {
        Converter::set_sample(self, channel, value)
    }
}
