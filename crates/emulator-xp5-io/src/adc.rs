//! XP5 15-channel ADC sequencer. External resistor values are explicit inputs.
use fujimod_emulator_devices::bus::FastReadWord;
use fujimod_emulator_devices::{Cx, Device, parts::analog::Samples};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

pub const BASE: u64 = 0xff906000;
/// Register offsets served from mirrors without a device call. The sample
/// channels (`0x10..=0x48`) are not mirrored: their levels are shared with
/// other devices, which can change them without a register write here.
const MIRRORED: [u64; 7] = [0, 4, 8, 0xc, 0x4c, 0x50, 0x54];
#[derive(Clone, Serialize, Deserialize)]
struct Registers {
    enable: u32,
    config: [u32; 2],
    ack: u32,
    mask: u32,
    #[serde(default)]
    timing: u32,
}

#[derive(Serialize, Deserialize)]
struct State {
    #[serde(flatten)]
    registers: Registers,
    samples: [u16; 15],
}
pub struct Adc {
    registers: Registers,
    samples: Samples,
    /// Host overrides of the idle samples (`XT5_ADC`), read in place of `state.samples`. They are
    /// kept outside the checkpointed state and the manifest, so a snapshot loads with any value.
    overrides: [Option<u16>; 15],
    mirrors: [Arc<AtomicU32>; MIRRORED.len()],
}

/// Parse `XT5_ADC="CHANNEL=SAMPLE,..."`: idle samples that replace the manifest's for this run,
/// for example the position of a dial or switch read through an ADC channel. On the X-T5, channel
/// 3 is the drive dial (4095 = HDR, 3500 = single frame).
fn parse_overrides(spec: &str) -> Result<[Option<u16>; 15], String> {
    let mut overrides = [None; 15];
    for item in spec
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (channel, sample) = item
            .split_once('=')
            .ok_or("XT5_ADC needs CHANNEL=SAMPLE[,...]")?;
        let channel: usize = channel
            .trim()
            .parse()
            .map_err(|_| format!("XT5_ADC: invalid channel in {item}"))?;
        let sample: u16 = sample
            .trim()
            .parse()
            .map_err(|_| format!("XT5_ADC: invalid sample in {item}"))?;
        if sample > 4095 {
            return Err(format!("XT5_ADC: sample {sample} exceeds 12 bits"));
        }
        *overrides
            .get_mut(channel)
            .ok_or_else(|| format!("XT5_ADC: channel {channel} is outside 0..=14"))? = Some(sample);
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
        let adc = Self {
            registers: Registers {
                enable: 0,
                config: [0; 2],
                ack: 0,
                mask: 0,
                timing: 0,
            },
            samples: Samples::new(samples.to_vec())?,
            overrides,
            mirrors: std::array::from_fn(|_| Arc::new(AtomicU32::new(0))),
        };
        adc.publish_mirrors();
        Ok(adc)
    }

    fn register(&self, offset: u64) -> u32 {
        match offset {
            0 => self.registers.enable,
            4 | 8 => self.registers.config[(offset / 4 - 1) as usize],
            0xc => self.registers.ack,
            0x4c => self.registers.mask,
            0x50 => 8,
            0x54 => self.registers.timing,
            _ => unreachable!("ADC register {offset:#x} is not mirrored"),
        }
    }

    fn publish_mirrors(&self) {
        for (mirror, offset) in self.mirrors.iter().zip(MIRRORED) {
            mirror.store(self.register(offset), Ordering::Release);
        }
    }

    pub fn external_samples(&self) -> Samples {
        self.samples.clone()
    }

    fn snapshot(&self) -> State {
        State {
            registers: self.registers.clone(),
            samples: self
                .samples
                .values()
                .try_into()
                .expect("XP5 ADC has 15 channels"),
        }
    }

    fn validate_state(state: &State) -> Result<(), String> {
        if state.samples.iter().any(|&value| value > 4095) {
            return Err("ADC checkpoint sample exceeds 12-bit range".into());
        }
        Ok(())
    }

    fn restore_state(&mut self, state: State) -> Result<(), String> {
        Self::validate_state(&state)?;
        self.samples.replace(&state.samples)?;
        self.registers = state.registers;
        self.publish_mirrors();
        Ok(())
    }
}

/// The idle external levels; a host override (`XT5_ADC`) wins while it is set.
impl fujimod_emulator_devices::capability::AnalogInputs for Adc {
    fn samples(&self) -> Vec<u16> {
        let mut levels = self.samples.values();
        for (level, override_value) in levels.iter_mut().zip(self.overrides) {
            *level = override_value.unwrap_or(*level);
        }
        levels
    }
    fn set_sample(&mut self, channel: usize, value: u16) -> Result<(), String> {
        if value > 4095 {
            return Err("XP5 ADC input levels are 12-bit".into());
        }
        self.samples.set(channel, value)?;
        self.overrides[channel] = None;
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
    fn fast_reads(&self, window_base: u64, window_size: u64) -> Vec<FastReadWord> {
        let Some(window_end) = window_base.checked_add(window_size) else {
            return Vec::new();
        };
        self.mirrors
            .iter()
            .zip(MIRRORED)
            .filter_map(|(mirror, offset)| {
                let address = BASE.checked_add(offset)?;
                (address >= window_base && address.checked_add(4)? <= window_end)
                    .then(|| FastReadWord::atomic_u32(address - window_base, mirror.clone()))
            })
            .collect()
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(BASE);
        if size != 4 || offset & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        match offset {
            0 | 4 | 8 | 0xc | 0x4c | 0x50 | 0x54 => u64::from(self.register(offset)),
            0x10..=0x48 => {
                let n = ((offset - 0x10) / 4) as usize;
                self.overrides[n]
                    .unwrap_or_else(|| self.samples.get(n).expect("XP5 ADC has 15 channels"))
                    as u64
            }
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(BASE);
        if size != 4 {
            cx.unmodeled();
            return;
        }
        match offset {
            0 => {
                self.registers.enable = value as u32;
                if value & 1 != 0 {
                    self.registers.ack |= 1;
                }
            }
            4 | 8 => self.registers.config[(offset / 4 - 1) as usize] = value as u32,
            0xc => self.registers.ack = value as u32 & 1,
            0x4c => self.registers.mask = value as u32 & 0x7fff,
            0x54 => {
                if value > 15 {
                    cx.fail("unmodeled ADC +54 configuration outside stock 4-bit range");
                } else {
                    self.registers.timing = value as u32;
                }
            }
            _ => cx.unmodeled(),
        }
        self.publish_mirrors();
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.snapshot())
    }

    fn validate_checkpoint(
        &self,
        checkpoint: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        Self::validate_state(&checkpoint.decode(self.name())?)
    }

    fn restore_checkpoint(
        &mut self,
        checkpoint: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.restore_state(checkpoint.decode(self.name())?)
    }

    fn save(&self) -> Value {
        json!({"adc":self.snapshot()})
    }

    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let state =
            serde_json::from_value(v["adc"].clone()).map_err(|e| format!("ADC checkpoint: {e}"))?;
        self.restore_state(state)
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
    fn wired_samples_follow_the_adc_checkpoint() {
        use fujimod_emulator_devices::capability::AnalogInputs;

        let mut adc = Adc::new([4095; 15]).unwrap();
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let encoder_levels = adc.external_samples();
        encoder_levels.set(8, 0).unwrap();
        assert_eq!(adc.samples()[8], 0);
        assert_eq!(adc.read(&mut cx, BASE + 0x30, 4), 0);
        let saved = adc.checkpoint().unwrap();
        encoder_levels.set(8, 4095).unwrap();
        adc.restore_checkpoint(&saved).unwrap();
        assert_eq!(encoder_levels.get(8).unwrap(), 0);

        adc.set_sample(11, 3439).unwrap();
        assert_eq!(encoder_levels.get(11).unwrap(), 3439);
        assert_eq!(adc.save()["adc"]["samples"][8], 0);
        assert_eq!(adc.save()["adc"]["enable"], 0);
    }

    #[test]
    fn fast_reads_match_register_reads_and_leave_samples_to_the_device() {
        let mut adc = Adc::new([4095; 15]).unwrap();
        let fast: std::collections::BTreeMap<u64, Arc<AtomicU32>> = adc
            .fast_reads(BASE, 0x100)
            .into_iter()
            .map(|word| {
                let fujimod_emulator_devices::bus::FastReadValue::AtomicU32(atom) = word.value
                else {
                    panic!("expected AtomicU32 at {:#x}", word.offset);
                };
                (word.offset, atom)
            })
            .collect();
        assert_eq!(fast.keys().copied().collect::<Vec<_>>(), MIRRORED);
        let mut host = RecordingHost::default();
        let check = |adc: &mut Adc, host: &mut RecordingHost| {
            for offset in MIRRORED {
                let mut cx = Cx::new(host, 0, 0, 0);
                let expected = adc.read(&mut cx, BASE + offset, 4);
                assert_eq!(u64::from(fast[&offset].load(Ordering::Acquire)), expected);
            }
        };
        check(&mut adc, &mut host);
        for (offset, value) in [(0, 1), (4, 0x55), (8, 0xaa), (0xc, 0), (0x4c, 0x7fff), (0x54, 9)] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            adc.write(&mut cx, BASE + offset, 4, value);
            check(&mut adc, &mut host);
        }
        let saved = adc.checkpoint().unwrap();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        adc.write(&mut cx, BASE + 4, 4, 0);
        adc.restore_checkpoint(&saved).unwrap();
        check(&mut adc, &mut host);
        assert_eq!(fast[&4].load(Ordering::Acquire), 0x55);
    }

    #[test]
    fn sample_overrides_are_validated() {
        assert_eq!(parse_overrides("").unwrap(), [None; 15]);
        assert_eq!(parse_overrides("14=0").unwrap()[14], Some(0));
        assert!(parse_overrides("15=1").is_err());
        assert!(parse_overrides("3=4096").is_err());
        assert!(parse_overrides("3").is_err());
    }
}
