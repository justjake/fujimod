//! XP5 15-channel ADC sequencer. External resistor values are explicit inputs.
use fujimod_emulator_devices::bus::FastReadWord;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

const REGISTER_COUNT: usize = 22;

pub const BASE: u64 = 0xff906000;
#[derive(Serialize, Deserialize)]
struct State {
    enable: u32,
    config: [u32; 2],
    ack: u32,
    mask: u32,
    #[serde(default)]
    timing: u32,
    samples: [u16; 15],
}
pub struct Adc {
    state: State,
    /// Host overrides of the idle samples (`XT5_ADC`), read in place of `state.samples`. They are
    /// kept outside the checkpointed state and the manifest, so a snapshot loads with any value.
    overrides: [Option<u16>; 15],
    mirrors: [Arc<AtomicU32>; REGISTER_COUNT],
}

/// Parse `XT5_ADC="CHANNEL=SAMPLE,..."`: idle samples that replace the manifest's for this run,
/// for example the position of a dial or switch read through an ADC channel. On the X-T5, channel
/// 3 is the drive dial (4095 = HDR, 3500 = single frame).
fn parse_overrides(spec: &str) -> Result<[Option<u16>; 15], String> {
    let mut overrides = [None; 15];
    for item in spec.split(',').map(str::trim).filter(|item| !item.is_empty()) {
        let (channel, sample) = item.split_once('=').ok_or("XT5_ADC needs CHANNEL=SAMPLE[,...]")?;
        let channel: usize = channel.trim().parse().map_err(|_| format!("XT5_ADC: invalid channel in {item}"))?;
        let sample: u16 = sample.trim().parse().map_err(|_| format!("XT5_ADC: invalid sample in {item}"))?;
        if sample > 4095 {
            return Err(format!("XT5_ADC: sample {sample} exceeds 12 bits"));
        }
        *overrides.get_mut(channel).ok_or_else(|| format!("XT5_ADC: channel {channel} is outside 0..=14"))? =
            Some(sample);
    }
    Ok(overrides)
}
impl Adc {
    pub fn new(samples: [u16; 15]) -> Result<Self, String> {
        if samples.iter().any(|v| *v > 4095) {
            return Err("ADC samples must be 12-bit".into());
        }
        let overrides = match std::env::var("XT5_ADC") {
            Ok(spec) => parse_overrides(&spec)?,
            Err(_) => [None; 15],
        };
        let state = State {
            enable: 0,
            config: [0; 2],
            ack: 0,
            mask: 0,
            timing: 0,
            samples,
        };
        let mirrors = std::array::from_fn(|_| Arc::new(AtomicU32::new(0)));
        let adc = Self {
            state,
            overrides,
            mirrors,
        };
        adc.publish_mirrors();
        Ok(adc)
    }
}
impl Adc {
    fn publish_mirrors(&self) {
        self.mirrors[0].store(self.state.enable, Ordering::Release);
        self.mirrors[1].store(self.state.config[0], Ordering::Release);
        self.mirrors[2].store(self.state.config[1], Ordering::Release);
        self.mirrors[3].store(self.state.ack, Ordering::Release);
        for n in 0..15 {
            let sample = self.overrides[n].unwrap_or(self.state.samples[n]);
            self.mirrors[4 + n].store(u32::from(sample), Ordering::Release);
        }
        self.mirrors[19].store(self.state.mask, Ordering::Release);
        self.mirrors[20].store(8, Ordering::Release);
        self.mirrors[21].store(self.state.timing, Ordering::Release);
    }
    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.samples.iter().any(|&value| value > 4095) {
            return Err("ADC checkpoint sample exceeds 12-bit range".into());
        }
        Ok(())
    }
}

/// The idle external levels; a host override (`XT5_ADC`) wins while it is set.
impl fujimod_emulator_devices::capability::AnalogInputs for Adc {
    fn samples(&self) -> Vec<u16> {
        (0..15)
            .map(|n| self.overrides[n].unwrap_or(self.state.samples[n]))
            .collect()
    }
    fn set_sample(&mut self, channel: usize, value: u16) -> Result<(), String> {
        if value > 4095 {
            return Err("XP5 ADC input levels are 12-bit".into());
        }
        *self
            .state
            .samples
            .get_mut(channel)
            .ok_or("XP5 ADC channel is outside 0..=14")? = value;
        self.overrides[channel] = None;
        self.mirrors[4 + channel].store(u32::from(value), Ordering::Release);
        Ok(())
    }
}

impl Device for Adc {
    fn analog_inputs(
        &self,
    ) -> Option<&(dyn fujimod_emulator_devices::capability::AnalogInputs + 'static)> {
        Some(self)
    }
    fn analog_inputs_mut(
        &mut self,
    ) -> Option<&mut (dyn fujimod_emulator_devices::capability::AnalogInputs + 'static)> {
        Some(self)
    }
    fn name(&self) -> &'static str {
        "adc-sequencer-input"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(BASE);
        if size != 4 || offset & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        match offset {
            0 => self.state.enable as u64,
            4 | 8 => self.state.config[(offset / 4 - 1) as usize] as u64,
            0xc => self.state.ack as u64,
            0x10..=0x48 => {
                let n = ((offset - 0x10) / 4) as usize;
                self.overrides[n].unwrap_or(self.state.samples[n]) as u64
            }
            0x4c => self.state.mask as u64,
            0x50 => 8,
            0x54 => self.state.timing as u64,
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn fast_reads(&self, window_base: u64, window_size: u64) -> Vec<FastReadWord> {
        let Some(window_end) = window_base.checked_add(window_size) else {
            return Vec::new();
        };
        let mut words = Vec::with_capacity(REGISTER_COUNT);
        for (index, mirror) in self.mirrors.iter().enumerate() {
            let offset = index as u64 * 4;
            if let Some(address) = BASE.checked_add(offset)
                && let Some(end) = address.checked_add(4)
                && address >= window_base
                && end <= window_end
            {
                words.push(FastReadWord::atomic_u32(
                    address - window_base,
                    mirror.clone(),
                ));
            }
        }
        words
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(BASE);
        if size != 4 {
            cx.unmodeled();
            return;
        }
        match offset {
            0 => {
                self.state.enable = value as u32;
                if value & 1 != 0 {
                    self.state.ack |= 1;
                }
                self.publish_mirrors();
            }
            4 | 8 => {
                self.state.config[(offset / 4 - 1) as usize] = value as u32;
                self.publish_mirrors();
            }
            0xc => {
                self.state.ack = value as u32 & 1;
                self.publish_mirrors();
            }
            0x4c => {
                self.state.mask = value as u32 & 0x7fff;
                self.publish_mirrors();
            }
            0x54 => {
                if value > 15 {
                    cx.fail("unmodeled ADC +54 configuration outside stock 4-bit range");
                } else {
                    self.state.timing = value as u32;
                    self.publish_mirrors();
                }
            }
            _ => cx.unmodeled(),
        }
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err(format!("{} snapshot has unexpected parts", self.name()));
        }
        let state: State = saved.decode(self.name())?;
        self.validate_state(&state)
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.state = saved.decode(self.name())?;
        self.publish_mirrors();
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
    fn save(&self) -> Value {
        json!({"adc":self.state})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let state: State =
            serde_json::from_value(v["adc"].clone()).map_err(|e| format!("ADC checkpoint: {e}"))?;
        if state.samples.iter().any(|v| *v > 4095) {
            return Err("ADC checkpoint sample exceeds 12-bit range".into());
        }
        self.state = state;
        self.publish_mirrors();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn adc_reads_declared_external_levels_without_firmware_ram() {
        let mut samples = [4095; 15];
        samples[7] = 2241;
        let mut adc = Adc::new(samples).unwrap();
        let mut host = RecordingHost::default(); // No RAM, no private config tables.
        let mut cx = Cx::new(&mut host, 3, 0xdeadbeef, 0);
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 7 * 4, 4), 2241);
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 11 * 4, 4), 4095);
        use fujimod_emulator_devices::capability::AnalogInputs;
        adc.set_sample(11, 3472).unwrap();
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 11 * 4, 4), 3472);
        assert!(cx.refusal().is_none());
        let saved = adc.save();
        adc.set_sample(11, 4095).unwrap();
        adc.restore(&saved).unwrap();
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 11 * 4, 4), 3472);
        // A host override replaces the idle sample and is not part of the saved state.
        adc.overrides = parse_overrides("3=3500, 7=100").unwrap();
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 3 * 4, 4), 3500);
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 7 * 4, 4), 100);
        assert_eq!(adc.save()["adc"]["samples"][7], 2241);
        let checkpoint = adc.checkpoint().unwrap();
        adc.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(adc.read(&mut cx, BASE + 0x10 + 3 * 4, 4), 3500);
    }
    #[test]
    fn sample_overrides_are_validated() {
        assert_eq!(parse_overrides("").unwrap(), [None; 15]);
        assert_eq!(parse_overrides("14=0").unwrap()[14], Some(0));
        assert!(parse_overrides("15=1").is_err());
        assert!(parse_overrides("3=4096").is_err());
        assert!(parse_overrides("3").is_err());
    }

    #[test]
    fn fast_reads_match_device_read_across_writes_samples_overrides_and_restores() {
        use fujimod_emulator_devices::bus::FastReadValue;
        use fujimod_emulator_devices::capability::AnalogInputs;
        use std::collections::BTreeMap;

        let mut samples = [4095; 15];
        samples[3] = 1200;
        samples[7] = 2241;
        let mut adc = Adc::new(samples).unwrap();
        let fast: BTreeMap<u64, Arc<AtomicU32>> = adc
            .fast_reads(BASE, 0x100)
            .into_iter()
            .map(|word| {
                let FastReadValue::AtomicU32(atom) = word.value else {
                    panic!("expected AtomicU32 at {:#x}", word.offset);
                };
                (word.offset, atom)
            })
            .collect();
        let offsets: Vec<u64> = (0u64..=0x54).step_by(4).collect();
        assert_eq!(fast.keys().copied().collect::<Vec<_>>(), offsets);

        let mut host = RecordingHost::default();
        let assert_parity = |adc: &mut Adc, host: &mut RecordingHost| {
            for &offset in &offsets {
                let mut cx = Cx::new(host, 0, 0, 0);
                let expected = adc.read(&mut cx, BASE + offset, 4);
                assert!(!cx.is_unmodeled(), "offset {offset:#x}");
                assert_eq!(
                    u64::from(fast[&offset].load(Ordering::Acquire)),
                    expected,
                    "offset {offset:#x}"
                );
            }
        };
        assert_parity(&mut adc, &mut host);

        let initial_checkpoint = adc.checkpoint().unwrap();
        let initial_json = adc.save();

        for (offset, value) in [
            (0x00, 1),
            (0x04, 0x1234),
            (0x08, 0x5678),
            (0x4c, 0x7fff),
            (0x54, 9),
        ] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            adc.write(&mut cx, BASE + offset, 4, value);
            assert!(cx.refusal().is_none());
        }
        adc.set_sample(11, 3472).unwrap();
        assert_parity(&mut adc, &mut host);

        // Host overrides take precedence over restored samples.
        adc.overrides = parse_overrides("3=3500").unwrap();
        adc.restore(&initial_json).unwrap();
        assert_eq!(fast[&(0x10 + 3 * 4)].load(Ordering::Acquire), 3500);
        assert_parity(&mut adc, &mut host);

        adc.set_sample(3, 900).unwrap();
        assert_eq!(fast[&(0x10 + 3 * 4)].load(Ordering::Acquire), 900);
        assert_parity(&mut adc, &mut host);

        adc.overrides = parse_overrides("7=100").unwrap();
        adc.restore_checkpoint(&initial_checkpoint).unwrap();
        assert_eq!(fast[&(0x10 + 7 * 4)].load(Ordering::Acquire), 100);
        assert_parity(&mut adc, &mut host);

        // Window clipping works for subwindows.
        let clipped: Vec<u64> = adc
            .fast_reads(BASE + 0x48, 0x10)
            .into_iter()
            .map(|w| w.offset)
            .collect();
        assert_eq!(clipped, vec![0x00, 0x04, 0x08, 0x0c]);
    }
}
