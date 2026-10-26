//! Four-byte power-peer frames: seven-bit register, write flag, data and checksum.
//! Fixed inputs, inert disables, stored registers and an RTC law are declared.
use crate::{
    rtc::{StagedClock, StagedConfig},
    spi::Input,
};
use fujimod_emulator_ip::socionext::gpio::SharedPins;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    kind: String,
    port: usize,
    pin: usize,
    active_high: bool,
    idle: bool,
    inputs: BTreeMap<u8, u16>,
    disable_writes: BTreeSet<(u8, u16)>,
    rtc: Option<StagedClock>,
    retained_registers: BTreeSet<u8>,
    stored: BTreeMap<u8, u16>,
    retained_written: BTreeSet<u8>,
    edge: u64,
    phase: u8,
    frame: [u8; 4],
    response: u16,
}

pub struct ChecksumPowerSerial {
    pins: SharedPins,
    state: State,
}

impl ChecksumPowerSerial {
    pub fn new(
        pins: SharedPins,
        port: usize,
        pin: usize,
        active_high: bool,
        idle: bool,
        inputs: BTreeMap<u8, u16>,
        disable_writes: Vec<(u8, u16)>,
        rtc: Option<StagedConfig>,
        retained_registers: Vec<u8>,
    ) -> Result<Self, String> {
        if port >= 16 || pin >= 16 || inputs.keys().any(|r| *r > 127) {
            return Err(
                "Checksum power SPI needs bounded registers and physical GPIO wiring".into(),
            );
        }
        let disables: BTreeSet<_> = disable_writes.iter().copied().collect();
        if disables.len() != disable_writes.len()
            || disables
                .iter()
                .any(|(register, value)| *register > 127 || *value != 0)
        {
            return Err("Checksum power SPI disable admissions require unique seven-bit registers and zero values".into());
        }
        if rtc.as_ref().is_some_and(|r| {
            inputs.keys().any(|k| r.owns(*k)) || disables.iter().any(|(k, _)| r.owns(*k))
        }) {
            return Err("Staged RTC overlaps fixed inputs or disable admissions".into());
        }
        let retained: BTreeSet<_> = retained_registers.iter().copied().collect();
        if retained.len() != retained_registers.len()
            || retained.iter().any(|register| {
                *register > 127
                    || disables.contains(&(*register, 0))
                    || rtc.as_ref().is_some_and(|r| r.owns(*register))
            })
        {
            return Err(
                "Stored power registers require unique seven-bit addresses without command overlap"
                    .into(),
            );
        }
        let rtc = rtc.map(StagedClock::new).transpose()?;
        Ok(Self {
            pins,
            state: State {
                kind: "checksum-power-registers".into(),
                port,
                pin,
                active_high,
                idle,
                inputs,
                disable_writes: disables,
                rtc,
                retained_registers: retained,
                stored: BTreeMap::new(),
                retained_written: BTreeSet::new(),
                edge: 0,
                phase: 0,
                frame: [0; 4],
                response: 0,
            },
        })
    }
}

impl Input for ChecksumPowerSerial {
    fn exchange(&mut self, bits: u32, tx: u16, tick: u64) -> Result<u16, String> {
        let s = &mut self.state;
        let (level, edge) = self.pins.lock().unwrap().output(s.port, s.pin);
        if edge != s.edge {
            s.phase = 0;
            s.edge = edge;
        }
        if level != s.active_high {
            return Ok(if s.idle { u16::MAX >> (16 - bits) } else { 0 });
        }
        if bits != 8 {
            return Err("Checksum power SPI requires eight-bit serial words".into());
        }
        s.frame[usize::from(s.phase)] = tx as u8;
        let header = s.frame[0];
        let register = header & 127;
        let reply = match s.phase {
            0 => {
                s.response = if header & 128 == 0 {
                    if let Some(rtc) = s.rtc.as_ref().filter(|r| r.config.owns(register)) {
                        rtc.read(register, tick)?
                    } else if s.retained_registers.contains(&register) {
                        *s.stored
                            .get(&register)
                            .or_else(|| s.inputs.get(&register))
                            .unwrap_or(&0)
                    } else {
                        *s.inputs.get(&register).ok_or_else(|| {
                            format!("Unsupported checksum power SPI read register{register}")
                        })?
                    }
                } else {
                    0
                };
                0
            }
            1 => s.response >> 8,
            2 => s.response & 255,
            _ => {
                if s.frame
                    .iter()
                    .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
                    != 255
                {
                    return Err("Invalid checksum power SPI request checksum".into());
                }
                if header & 128 != 0 {
                    let value = u16::from_be_bytes([s.frame[1], s.frame[2]]);
                    if let Some(rtc) = s.rtc.as_mut().filter(|r| r.config.owns(register)) {
                        if register == rtc.config.control && value == rtc.retained_control_value() {
                            if s.retained_registers.is_empty()
                                || s.retained_written != s.retained_registers
                            {
                                return Err("Retained control commit requires fresh writes to every declared word".into());
                            }
                            rtc.commit_retained(value, tick)?;
                            s.retained_written.clear();
                        } else {
                            rtc.write(register, value, tick)?;
                        }
                        u16::from(s.frame[3])
                    } else if s.retained_registers.contains(&register) {
                        // Register storage only; this supplies no commit,
                        // reset retention or power-domain transition.
                        s.stored.insert(register, value);
                        s.retained_written.insert(register);
                        u16::from(s.frame[3])
                    } else if s.disable_writes.contains(&(register, value)) {
                        // No countdown/alarm is implemented or armed. A declared
                        // disable leaves it inert; this does not admit readback
                        // or any enabling/status transition.
                        u16::from(s.frame[3])
                    } else if s.disable_writes.contains(&(register, 0)) {
                        return Err(format!(
                            "Checksum power SPI countdown/alarm enable is not modeled: register{register} value0x{value:04x}"
                        ));
                    } else {
                        return Err(format!(
                            "Unsupported checksum power SPI write register{register} value0x{value:04x}"
                        ));
                    }
                } else {
                    u16::from(
                        !register
                            .wrapping_add((s.response >> 8) as u8)
                            .wrapping_add(s.response as u8),
                    )
                }
            }
        };
        s.phase = (s.phase + 1) % 4;
        Ok(reply)
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let s = &self.state;
        if state.kind != s.kind
            || state.port != s.port
            || state.pin != s.pin
            || state.active_high != s.active_high
            || state.idle != s.idle
            || state.inputs != s.inputs
            || state.disable_writes != s.disable_writes
            || state.retained_registers != s.retained_registers
            || state
                .stored
                .keys()
                .any(|r| !s.retained_registers.contains(r))
            || state
                .retained_written
                .iter()
                .any(|r| !s.retained_registers.contains(r) || !state.stored.contains_key(r))
            || state.phase > 3
        {
            return Err("Invalid checksum power SPI checkpoint or physical input".into());
        }
        match (&state.rtc, &s.rtc) {
            (Some(saved), Some(current)) => saved.validate_restore(current)?,
            (None, None) => {}
            _ => return Err("Staged RTC selection differs from checkpoint".into()),
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_ip::socionext::gpio::{Gpio, Pins};
    const BASE: u64 = 0x10000;
    use fujimod_emulator_devices::{Cx, Device, Host, Record};

    struct Clock;
    impl Host for Clock {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
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
    fn selected() -> (ChecksumPowerSerial, Gpio) {
        let pins = Pins::new([0; 16]);
        let peer = ChecksumPowerSerial::new(
            pins.clone(),
            12,
            5,
            true,
            false,
            [(0, 0), (18, 0x1234), (65, 0xabcd)].into(),
            vec![],
            None,
            vec![],
        )
        .unwrap();
        let mut gpio = Gpio::new(BASE, pins).unwrap();
        select(&mut gpio, true);
        (peer, gpio)
    }
    fn select(gpio: &mut Gpio, high: bool) {
        gpio.write(
            &mut Cx::new(&mut Clock, 0, 0, 0),
            BASE + 12 * 64,
            4,
            (1 << 21) | if high { 1 << 5 } else { 0 },
        );
    }
    fn frame(peer: &mut ChecksumPowerSerial, bytes: [u16; 4]) -> Result<Vec<u16>, String> {
        bytes.into_iter().map(|b| peer.exchange(8, b, 0)).collect()
    }

    #[test]
    fn checksum_is_the_fourth_byte_and_register_address_is_seven_bits() {
        let (mut peer, _) = selected();
        assert_eq!(frame(&mut peer, [0, 0, 0, 255]).unwrap(), [0, 0, 0, 255]);
        assert_eq!(
            frame(&mut peer, [18, 0, 0, 237]).unwrap(),
            [0, 0x12, 0x34, 0xa7]
        );
        assert_eq!(
            frame(&mut peer, [65, 0, 0, 190]).unwrap(),
            [0, 0xab, 0xcd, 0x46]
        );
        assert!(
            peer.exchange(8, 127, 0)
                .unwrap_err()
                .contains("read register127")
        );
    }

    #[test]
    fn partial_frames_resume_and_chip_select_aborts_them() {
        let (mut peer, mut gpio) = selected();
        peer.exchange(8, 18, 0).unwrap();
        let saved = peer.save();
        peer.exchange(8, 0, 0).unwrap();
        peer.restore(&saved).unwrap();
        assert_eq!(peer.exchange(8, 0, 0).unwrap(), 0x12);
        assert_eq!(peer.exchange(8, 0, 0).unwrap(), 0x34);
        assert_eq!(peer.exchange(8, 237, 0).unwrap(), 0xa7);
        peer.exchange(8, 18, 0).unwrap();
        select(&mut gpio, false);
        assert_eq!(peer.exchange(8, 255, 0).unwrap(), 0);
        select(&mut gpio, true);
        assert_eq!(frame(&mut peer, [0, 0, 0, 255]).unwrap(), [0, 0, 0, 255]);
        let saved = peer.save();
        assert_eq!(peer.save(), saved);
        let mut wrong = saved.clone();
        wrong["inputs"]["0"] = 1.into();
        assert!(peer.restore(&wrong).is_err());
    }

    #[test]
    fn bad_checksum_and_unsupported_write_have_no_register_effect() {
        let (mut peer, mut gpio) = selected();
        assert!(
            frame(&mut peer, [0, 0, 0, 0])
                .unwrap_err()
                .contains("checksum")
        );
        select(&mut gpio, false);
        select(&mut gpio, true);
        assert_eq!(
            frame(&mut peer, [0x92, 0x12, 0x34, 0x27]).unwrap_err(),
            "Unsupported checksum power SPI write register18 value0x1234"
        );
        select(&mut gpio, false);
        select(&mut gpio, true);
        assert_eq!(
            frame(&mut peer, [18, 0, 0, 237]).unwrap(),
            [0, 0x12, 0x34, 0xa7]
        );
    }

    #[test]
    fn stored_registers_use_declared_initial_values_and_resume_partial_writes() {
        fn packet(header: u16, value: u16) -> [u16; 4] {
            let high = value >> 8;
            let low = value & 255;
            [header, high, low, !(header + high + low) & 255]
        }
        let pins = Pins::new([0; 16]);
        let mut gpio = Gpio::new(BASE, pins.clone()).unwrap();
        let mut peer = ChecksumPowerSerial::new(
            pins,
            12,
            5,
            true,
            false,
            [(65, 0x1234)].into(),
            vec![],
            None,
            vec![65, 66],
        )
        .unwrap();
        select(&mut gpio, true);
        assert_eq!(
            frame(&mut peer, packet(65, 0)).unwrap(),
            [0, 0x12, 0x34, 0x78]
        );
        assert_eq!(frame(&mut peer, packet(66, 0)).unwrap(), [0, 0, 0, 0xbd]);
        assert!(
            frame(&mut peer, [0xc2, 0xab, 0xcd, 0])
                .unwrap_err()
                .contains("checksum")
        );
        assert_eq!(peer.save()["stored"], serde_json::json!({}));
        assert_eq!(peer.save()["retained_written"], serde_json::json!([]));
        select(&mut gpio, false);
        select(&mut gpio, true);
        let write = packet(0xc2, 0xabcd);
        peer.exchange(8, write[0], 0).unwrap();
        peer.exchange(8, write[1], 0).unwrap();
        let partial = peer.save();
        peer.exchange(8, write[2], 0).unwrap();
        assert_eq!(peer.exchange(8, write[3], 0).unwrap(), write[3]);
        let complete = peer.save();
        assert_eq!(complete["stored"], serde_json::json!({"66":0xabcd}));
        assert_eq!(complete["retained_written"], serde_json::json!([66]));
        peer.restore(&partial).unwrap();
        assert_eq!(peer.save()["stored"], serde_json::json!({}));
        assert_eq!(peer.save()["retained_written"], serde_json::json!([]));
        peer.exchange(8, write[2], 0).unwrap();
        peer.exchange(8, write[3], 0).unwrap();
        assert_eq!(peer.save(), complete);
        assert_eq!(
            frame(&mut peer, packet(66, 0)).unwrap(),
            [0, 0xab, 0xcd, 0x45]
        );
        let saved = peer.save();
        let mut invalid = saved.clone();
        invalid["stored"]["67"] = 1.into();
        assert!(peer.restore(&invalid).is_err());
        invalid = saved.clone();
        invalid["retained_registers"] = serde_json::json!([65]);
        assert!(peer.restore(&invalid).is_err());
        invalid = saved.clone();
        invalid["retained_written"] = serde_json::json!([67]);
        assert!(peer.restore(&invalid).is_err());
        invalid = saved.clone();
        invalid["retained_written"] = serde_json::json!([65]);
        assert!(peer.restore(&invalid).is_err());
        assert_eq!(peer.save(), saved);
        assert!(
            peer.exchange(8, 67, 0)
                .unwrap_err()
                .contains("read register67")
        );
    }

    #[test]
    fn stored_register_configuration_refuses_duplicates_and_command_overlap() {
        for (retained, disables) in [
            (vec![65, 65], vec![]),
            (vec![128], vec![]),
            (vec![65], vec![(65, 0)]),
        ] {
            assert!(
                ChecksumPowerSerial::new(
                    Pins::new([0; 16]),
                    12,
                    5,
                    true,
                    false,
                    BTreeMap::new(),
                    disables,
                    None,
                    retained,
                )
                .is_err()
            );
        }
        let rtc = StagedConfig {
            control: 100,
            read_low: 101,
            read_high: 102,
            stage_low: 103,
            stage_high: 104,
            commit_value: 0x202,
            valid_mask: 2,
            retained_valid_mask: 1,
            strobe: crate::rtc::StrobeWindow {
                mask: 0x200,
                set_through_ticks: 2,
                clear_from_ticks: 5,
            },
            initial_control: 0,
            initial_seconds: 0,
            ticks_per_second: 10,
        };
        assert!(
            ChecksumPowerSerial::new(
                Pins::new([0; 16]),
                12,
                5,
                true,
                false,
                BTreeMap::new(),
                vec![],
                Some(rtc),
                vec![100],
            )
            .is_err()
        );
    }

    #[test]
    fn declared_disable_acknowledges_only_zero_without_admitting_readback_or_enable() {
        let pins = Pins::new([0; 16]);
        let mut gpio = Gpio::new(BASE, pins.clone()).unwrap();
        let mut peer = ChecksumPowerSerial::new(
            pins,
            12,
            5,
            true,
            false,
            [(0, 0)].into(),
            vec![(65, 0)],
            None,
            vec![],
        )
        .unwrap();
        select(&mut gpio, true);
        assert_eq!(
            frame(&mut peer, [0xc1, 0, 0, 0x3e]).unwrap(),
            [0, 0, 0, 0x3e]
        );
        let saved = peer.save();
        let mut changed = saved.clone();
        changed["disable_writes"] = serde_json::json!([[64, 0]]);
        assert!(peer.restore(&changed).is_err());
        peer.restore(&saved).unwrap();
        assert!(
            frame(&mut peer, [0xc1, 0, 1, 0x3d])
                .unwrap_err()
                .contains("enable is not modeled")
        );
        select(&mut gpio, false);
        select(&mut gpio, true);
        assert!(
            peer.exchange(8, 65, 0)
                .unwrap_err()
                .contains("read register65")
        );
        select(&mut gpio, false);
        select(&mut gpio, true);
        assert_eq!(
            frame(&mut peer, [0xc2, 0, 0, 0x3d]).unwrap_err(),
            "Unsupported checksum power SPI write register66 value0x0000"
        );
        select(&mut gpio, false);
        select(&mut gpio, true);
        assert!(
            frame(&mut peer, [0xc1, 0, 0, 0])
                .unwrap_err()
                .contains("checksum")
        );
        for invalid in [vec![(65, 1)], vec![(65, 0), (65, 0)], vec![(128, 0)]] {
            assert!(
                ChecksumPowerSerial::new(
                    Pins::new([0; 16]),
                    12,
                    5,
                    true,
                    false,
                    [(0, 0)].into(),
                    invalid,
                    None,
                    vec![],
                )
                .is_err()
            );
        }
    }
}
