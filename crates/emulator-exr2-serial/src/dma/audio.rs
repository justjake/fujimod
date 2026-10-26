//! Serial peripheral DMA channel setup and activation. No samples or transfer
//! completion are synthesized when the external FIFO has no data source.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
pub const BASE: u64 = 0xfff02040;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub ram_bytes: u64,
    pub max_transfer_bytes: u32,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: BTreeMap<u16, u32>,
    waiting_receive: bool,
    transmit_active: bool,
    activations: u64,
}
pub struct SerialDma {
    options: Options,
    state: State,
}
impl SerialDma {
    pub fn new(options: Options) -> Result<Self, String> {
        if options.ram_bytes == 0
            || options.ram_bytes > 0x40000000
            || options.max_transfer_bytes == 0
            || u64::from(options.max_transfer_bytes) > options.ram_bytes
        {
            return Err("invalid serial DMA RAM/transfer bounds".into());
        }
        Ok(Self {
            options,
            state: State::default(),
        })
    }
    fn offset(address: u64, size: u8) -> Option<u16> {
        let o = address.checked_sub(BASE)?;
        (size == 4
            && matches!(
                o,
                0 | 4
                    | 8
                    | 0xc
                    | 0x10
                    | 0x14
                    | 0x18
                    | 0x1c
                    | 0x24
                    | 0x28
                    | 0x2c
                    | 0x30
                    | 0x34
                    | 0x38
            ))
        .then_some(o as u16)
    }
    fn word(&self, o: u16) -> u32 {
        self.state.registers.get(&o).copied().unwrap_or(0)
    }
    fn ram_span(&self, address: u32, bytes: u32) -> bool {
        let address = match address {
            0..0x40000000 => u64::from(address),
            0x40000000..0x80000000 => u64::from(address - 0x40000000),
            _ => return false,
        };
        bytes > 0
            && bytes <= self.options.max_transfer_bytes
            && address
                .checked_add(bytes.into())
                .is_some_and(|end| end <= self.options.ram_bytes)
    }
    fn primary_descriptor(&self) -> Result<bool, String> {
        let (source, dest, length) = (self.word(0), self.word(4), self.word(8));
        let receive =
            matches!(source, 0xfffa0020 | 0xfffa1020) && self.word(0x2c) & 0x003fffff == 0x122011;
        let transmit =
            matches!(dest, 0xfffa0010 | 0xfffa1010) && self.word(0x2c) & 0x003fffff == 0x221011;
        if receive && self.ram_span(dest, length) && dest & 3 == 0 {
            return Ok(true);
        }
        if transmit && self.ram_span(source, length) {
            return Ok(false);
        }
        Err("serial DMA requires a bounded RAM descriptor and serial FIFO endpoint".into())
    }
}
impl SerialDma {
    fn validate_state(&self, state: State) -> Result<State, String> {
        if state
            .registers
            .iter()
            .any(|(o, _)| Self::offset(BASE + u64::from(*o), 4).is_none() || *o == 0x24)
        {
            return Err("invalid serial DMA register state".into());
        }
        if state.registers.iter().any(|(o, v)| {
            matches!(o, 0x30 | 0x34) && *v != 0
                || *o == 0x2c
                    && (*v & !0x50323011 != 0
                        || *v & 0x11 != 0x11
                        || !matches!(*v & 0x003fffff, 0x221011 | 0x122011))
        }) {
            return Err("invalid serial DMA transfer control state".into());
        }
        let candidate = Self {
            options: self.options.clone(),
            state,
        };
        if ((candidate.state.waiting_receive || candidate.state.transmit_active)
            && candidate.state.activations == 0)
            || (candidate.state.waiting_receive && candidate.primary_descriptor() != Ok(true))
            || (candidate.state.transmit_active && candidate.primary_descriptor() != Ok(false))
            || (candidate.state.waiting_receive && candidate.state.transmit_active)
        {
            return Err("invalid active serial DMA descriptor".into());
        }

        Ok(candidate.state)
    }
}
impl Device for SerialDma {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_state(saved.decode(self.name())?).map(|_| ())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let state = self.validate_state(saved.decode(self.name())?)?;
        self.state = state;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(json!({"options":self.options,"state":self.state}))
    }

    fn name(&self) -> &'static str {
        "serial-audio-dma"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(o) = Self::offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if cx.cpu != 0 {
            cx.fail("serial DMA register access requires CPU0");
            return 0;
        }
        if o == 0x24 {
            return if self.state.waiting_receive {
                5
            } else if self.state.transmit_active {
                4
            } else {
                0
            };
        }
        self.word(o).into()
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(o) = Self::offset(address, size) else {
            cx.unmodeled();
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.fail("serial DMA value exceeds 32 bits");
            return;
        };
        if cx.cpu != 0 || o == 0x24 {
            cx.fail("serial DMA requires CPU0 writable configuration registers");
            return;
        }
        if matches!(o, 0x30 | 0x34) && value != 0 {
            cx.fail("serial DMA linked/auxiliary descriptors are unsupported");
            return;
        }
        if o == 0x2c
            && (value & !0x50323011 != 0
                || value & 0x11 != 0x11
                || !matches!(value & 0x003fffff, 0x221011 | 0x122011))
        {
            cx.fail("unsupported serial DMA transfer control bits");
            return;
        }
        if o == 0x28 {
            match value {
                8 => {
                    self.state.waiting_receive = false;
                    self.state.transmit_active = false;
                }
                1 | 5 => {
                    let receive = match self.primary_descriptor() {
                        Ok(receive) => receive,
                        Err(e) => {
                            cx.fail(e);
                            return;
                        }
                    };
                    if self.state.waiting_receive || self.state.transmit_active {
                        cx.fail("serial DMA channel already active");
                        return;
                    }
                    let Some(n) = self.state.activations.checked_add(1) else {
                        cx.fail("serial DMA activation count exhausted");
                        return;
                    };
                    self.state.activations = n;
                    self.state.waiting_receive = receive;
                    self.state.transmit_active = !receive;
                }
                _ => {
                    cx.fail("unknown serial DMA activation control");
                    return;
                }
            }
        }
        if (self.state.waiting_receive || self.state.transmit_active)
            // The inactive buffer can be prepared one word at a time while
            // the primary descriptor waits for the external FIFO request.
            && !matches!(o, 0xc | 0x10 | 0x14 | 0x28)
            && value != self.word(o)
            && !(o == 0x2c && value & !0x50000000 == self.word(o) & !0x50000000)
        {
            cx.fail("serial DMA descriptor changed while waiting for external samples");
            return;
        }
        self.state.registers.insert(o, value);
    }
    fn save(&self) -> Value {
        json!(self.state)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let state = self.validate_state(state)?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
    #[test]
    fn relocated_receive_waits_for_external_samples_and_has_no_timer_or_dma() {
        let mut host = TestHost::new();
        for destination in [0x1000, 0x8000] {
            let mut d = SerialDma::new(Options {
                ram_bytes: 0x100000,
                max_transfer_bytes: 0x10000,
            })
            .unwrap();
            for (o, v) in [
                (0x2c, 0x40122011),
                (0, 0xfffa0020),
                (4, destination),
                (8, 128),
            ] {
                write(&mut d, &mut host, BASE + o, v);
            }
            assert_eq!(read(&mut d, &mut host, BASE + 0x24), 0);
            write(&mut d, &mut host, BASE + 0x28, 1);
            let saved = d.save();
            let checkpoint = d.checkpoint().unwrap();
            let mut restored = SerialDma::new(d.options.clone()).unwrap();
            restored.restore_checkpoint(&checkpoint).unwrap();
            assert_eq!(restored.fingerprint().unwrap(), d.fingerprint().unwrap());
            advance(&mut restored, &mut host, 10000);
            assert_eq!(restored.next_deadline(10000), None);
            assert_eq!(restored.save(), saved);
            advance(&mut d, &mut host, 10000);
            assert_eq!(read(&mut d, &mut host, BASE + 0x24), 5);
            assert_eq!(d.next_deadline(0), None);
            assert_eq!(d.save(), saved);
            assert!(host.effects.is_empty());
            write(&mut d, &mut host, BASE + 0x2c, 0x50122011);
            write(&mut d, &mut host, BASE + 4, destination);
            let before = d.save();
            let mut bad = before.clone();
            bad["registers"]["4"] = json!(0x5ffffff0u32);
            assert!(d.restore(&bad).is_err());
            assert_eq!(d.save(), before);
        }
    }
    #[test]
    fn inactive_buffer_staging_preserves_the_active_descriptor_and_keyframe() {
        // Buffer staging preserves the active descriptor and keyframe; this
        // test does not supply external FIFO samples, transmission or completion.
        let mut host = TestHost::new();
        for source in [0x1000, 0x8000] {
            let mut d = SerialDma::new(Options {
                ram_bytes: 0x100000,
                max_transfer_bytes: 0x10000,
            })
            .unwrap();
            for (o, v) in [
                (0x2c, 0x40221011),
                (0, source),
                (4, 0xfffa0010),
                (8, 1792),
                (0xc, source + 2048),
                (0x10, 0xfffa0010),
                (0x14, 960),
                (0x28, 1),
            ] {
                write(&mut d, &mut host, BASE + o, v);
            }
            write(&mut d, &mut host, BASE + 0x2c, 0x10221011);
            write(&mut d, &mut host, BASE + 0x14, 1792);
            assert_eq!(read(&mut d, &mut host, BASE + 0x14), 1792);
            assert_eq!(read(&mut d, &mut host, BASE + 8), 1792);
            assert_eq!(read(&mut d, &mut host, BASE + 0x24), 4);
            // An incomplete inactive address is not a DMA access. A keyframe
            // must retain these raw staging words instead of admitting them.
            write(&mut d, &mut host, BASE + 0xc, 0xffff_fffc);
            let before = d.fingerprint().unwrap();
            let saved = d.checkpoint().unwrap();
            d.validate_checkpoint(&saved).unwrap();
            assert_eq!(d.fingerprint().unwrap(), before);
            let mut restored = SerialDma::new(d.options.clone()).unwrap();
            restored.restore_checkpoint(&saved).unwrap();
            assert_eq!(restored.fingerprint().unwrap(), before);
            advance(&mut restored, &mut host, 10000);
            assert_eq!(restored.next_deadline(10000), None);
            assert_eq!(restored.fingerprint().unwrap(), before);
            let mut cx = host.cx();
            restored.write(&mut cx, BASE + 8, 4, 4096);
            assert!(cx.take_refusal().unwrap().contains("descriptor changed"));
            assert_eq!(restored.fingerprint().unwrap(), before);
            assert!(host.effects.is_empty() && host.irqs.is_empty());
        }
    }

    #[test]
    fn enable_requires_matching_control_and_refuses_before_effects() {
        let mut host = TestHost::new();
        let mut d = SerialDma::new(Options {
            ram_bytes: 0x100000,
            max_transfer_bytes: 0x10000,
        })
        .unwrap();
        for (o, v) in [(0, 0xfffa0020), (4, 0x1000), (8, 128)] {
            write(&mut d, &mut host, BASE + o, v);
        }
        let before = d.save();
        let mut cx = host.cx();
        d.write(&mut cx, BASE + 0x28, 4, 1);
        assert!(cx.take_refusal().is_some());
        assert_eq!(d.save(), before);
        assert!(host.effects.is_empty());
    }
}

#[cfg(test)]
mod large_ram_tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    #[test]
    fn descriptor_bounds_follow_declared_ram_inside_the_physical_address_mask() {
        // The checked X-T1 graph has 3FFF0000 bytes; the old 512-MiB
        // constructor cap refused the explicit serial DMA before any execution.
        let mut d = SerialDma::new(Options {
            ram_bytes: 0x3fff0000,
            max_transfer_bytes: 0x10000,
        })
        .unwrap();
        let mut host = TestHost::new();
        for (o, v) in [
            (0x2c, 0x40221011),
            (0, 0x3ffefc00),
            (4, 0xfffa0010),
            (8, 1024),
            (0x28, 1),
        ] {
            write(&mut d, &mut host, BASE + o, v);
        }
        assert_eq!(read(&mut d, &mut host, BASE + 0x24), 4);
        assert_eq!(d.next_deadline(0), None);
        assert!(d.ram_span(0x7ffefc00, 1024));
        assert!(!d.ram_span(0x80000000, 128));
        assert!(!d.ram_span(0xfffa0010, 128));
        write(&mut d, &mut host, BASE + 0x28, 8);
        write(&mut d, &mut host, BASE, 0x3ffefc04);
        let mut cx = host.cx();
        d.write(&mut cx, BASE + 0x28, 4, 1);
        assert_eq!(
            cx.refusal(),
            Some("serial DMA requires a bounded RAM descriptor and serial FIFO endpoint")
        );
        assert!(!d.state.transmit_active);
        assert!(host.effects.is_empty());
        assert!(
            SerialDma::new(Options {
                ram_bytes: 0x40000001,
                max_transfer_bytes: 1024,
            })
            .is_err()
        );
    }
}
