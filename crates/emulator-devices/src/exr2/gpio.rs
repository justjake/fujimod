//! FFF89000 GPIO ports with masked output writes and external input pad
//! levels. Edge wiring and reset levels are declared
//! board inputs, not measured electrical behavior. No CPU or software state
//! selects a pin transition or authorizes an interrupt.
use crate::{
    Cx, Device,
    bus::InterruptLine,
    parts::pins::{Pin, Pins},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PadState {
    pub latch: Option<u32>,
    pub direction: Option<u32>,
    pub enabled: Option<u32>,
    pub mux: Option<u32>,
}
impl PadState {
    pub fn input_enabled_for_mux(self, mux: u32) -> Result<bool, String> {
        if mux > 3 {
            return Err("GPIO pad mux selector is out of range".into());
        }
        Ok(self.direction == Some(0) && self.enabled == Some(1) && self.mux == Some(mux))
    }
    /// A board declares which mux function routes its GPIO output to a pad.
    /// Unknown/disabled/input/other-function pads do not drive a level.
    pub fn driven_for_mux(self, mux: u32) -> Result<Option<u32>, String> {
        if mux > 3 {
            return Err("GPIO pad mux selector is out of range".into());
        }
        Ok(
            if self.direction == Some(1) && self.enabled == Some(1) && self.mux == Some(mux) {
                self.latch
            } else {
                None
            },
        )
    }
}

/// Declared connector-input interpretation of the kernel's source selector
/// and selected two-bit sense field. Electrical polarity is unmeasured:
/// selector1 uses rising/falling/either edges; selector0 uses high/low levels.
/// Vector, priority and CPU exception entry remain the IRQ controller's job.
pub fn interrupt_signal(
    edge: bool,
    sense: u32,
    previous: bool,
    high: bool,
) -> Result<(bool, bool), String> {
    match (edge, sense) {
        (true, 1) => Ok((false, !previous && high)),
        (true, 2) => Ok((false, previous && !high)),
        (true, 3) => Ok((false, previous != high)),
        (false, 1) => Ok((true, high)),
        (false, 2) => Ok((true, !high)),
        _ => Err("unmodeled GPIO input interrupt sense".into()),
    }
}

pub const BASE: u64 = 0xfff89000;
const INPUT_QUEUE: usize = 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialOutput {
    pub pin: Pin,
    pub high: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub input_levels: [u32; 16],
    pub output_levels: Vec<InitialOutput>,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Rising,
    Falling,
    Both,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputIrq {
    pub pin: Pin,
    pub edge: Edge,
    pub outputs: Vec<InterruptLine>,
}
pub struct Wiring {
    pub irqs: Vec<InputIrq>,
}
#[derive(Serialize, Deserialize)]
struct State {
    registers: [[u32; 8]; 18],
    outputs: [u16; 18],
    known: [u16; 18],
    configuration_known: [[u16; 4]; 18],
    pending: VecDeque<(Pin, bool)>,
}
#[derive(Clone)]
pub struct Pads {
    state: Arc<Mutex<State>>,
    irqs: Arc<Vec<InputIrq>>,
}
impl crate::parts::pins::InputPins for Pads {
    fn level(&self, pin: Pin) -> Result<bool, String> {
        self.input_level(pin)
    }
}
impl Pads {
    fn input_pin(pin: Pin) -> Result<(), String> {
        pin.validate()?;
        if pin.port >= 16 {
            return Err("GPIO input port has no input-status word".into());
        }
        Ok(())
    }
    /// Observe all input-status words, including declared high strap bits.
    pub fn input_levels(&self) -> [u32; 16] {
        let state = self.state.lock().unwrap();
        std::array::from_fn(|port| state.registers[port][5])
    }
    pub fn input_level(&self, pin: Pin) -> Result<bool, String> {
        Self::input_pin(pin)?;
        Ok(self.state.lock().unwrap().registers[pin.port as usize][5] & (1 << pin.pin) != 0)
    }
    /// Observe output-latch and explicitly written pad configuration fields.
    pub fn pad_state(&self, pin: Pin) -> Result<PadState, String> {
        pin.validate()?;
        let s = self.state.lock().unwrap();
        let port = pin.port as usize;
        let bit = 1u16 << pin.pin;
        let field = |word: usize, mask: u16, shift: u8| {
            (s.configuration_known[port][word - 1] & mask == mask)
                .then(|| (s.registers[port][word] & u32::from(mask)) >> shift)
        };
        let mux_shift = 2 * (pin.pin % 8);
        Ok(PadState {
            latch: (s.known[port] & bit != 0).then(|| u32::from(s.outputs[port] & bit != 0)),
            direction: field(1, bit, pin.pin),
            enabled: field(2, bit, pin.pin),
            mux: field(3 + usize::from(pin.pin >= 8), 3u16 << mux_shift, mux_shift),
        })
    }
    /// The two-bit function latch selects GPIO (0) or a peripheral output.
    pub fn function(&self, pin: Pin) -> Result<u8, String> {
        pin.validate()?;
        let word = 3 + usize::from(pin.pin >= 8);
        Ok(
            ((self.state.lock().unwrap().registers[pin.port as usize][word] >> (2 * (pin.pin % 8)))
                & 3) as u8,
        )
    }
    /// Queue an external electrical change. The board wakes this GPIO node;
    /// observation alone never applies the change or delivers its IRQ.
    pub fn request(&self, pin: Pin, high: bool) -> Result<(), String> {
        Self::input_pin(pin)?;
        let mut s = self.state.lock().unwrap();
        if s.pending.len() == INPUT_QUEUE {
            return Err("GPIO input queue is full".into());
        }
        s.pending.push_back((pin, high));
        Ok(())
    }
    pub fn admit_requests(&self, changes: &[(Pin, bool)]) -> Result<(), String> {
        for &(pin, _) in changes {
            Self::input_pin(pin)?;
        }
        if self.state.lock().unwrap().pending.len() + changes.len() > INPUT_QUEUE {
            return Err("GPIO input queue is full".into());
        }
        Ok(())
    }
    pub fn request_many(&self, changes: &[(Pin, bool)]) -> Result<(), String> {
        self.admit_requests(changes)?;
        self.state
            .lock()
            .unwrap()
            .pending
            .extend(changes.iter().copied());
        Ok(())
    }
    fn drive(&self, cx: &mut Cx<'_>, pin: Pin, high: bool) -> Result<(), String> {
        Self::input_pin(pin)?;
        let previous = {
            let mut s = self.state.lock().unwrap();
            let word = &mut s.registers[pin.port as usize][5];
            let previous = *word & (1 << pin.pin) != 0;
            *word = (*word & !(1 << pin.pin)) | (u32::from(high) << pin.pin);
            previous
        };
        if previous == high {
            return Ok(());
        }
        for route in self.irqs.iter().filter(|r| r.pin == pin) {
            if matches!(route.edge, Edge::Both) || matches!(route.edge, Edge::Rising) == high {
                for &output in &route.outputs {
                    let InterruptLine::Core { core, line } = output else {
                        unreachable!()
                    };
                    cx.raise_irq(core, line)?;
                }
            }
        }
        Ok(())
    }
}
impl Pins for Pads {
    fn output_level(&self, pin: Pin) -> Result<Option<bool>, String> {
        pin.validate()?;
        let s = self.state.lock().unwrap();
        let bit = 1 << pin.pin;
        if s.known[pin.port as usize] & bit == 0 {
            return Ok(None);
        }
        Ok(Some(s.outputs[pin.port as usize] & bit != 0))
    }
    fn input(&mut self, cx: &mut Cx<'_>, pin: Pin, high: bool) -> Result<(), String> {
        self.drive(cx, pin, high)
    }
}
pub struct Gpio {
    pads: Pads,
}
impl Gpio {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        for (i, route) in wiring.irqs.iter().enumerate() {
            Pads::input_pin(route.pin)?;
            if route.outputs.is_empty() || wiring.irqs[..i].iter().any(|r| r.pin == route.pin) {
                return Err("GPIO interrupt requires a unique pad and physical outputs".into());
            }
            for (n, output) in route.outputs.iter().enumerate() {
                if !matches!(output, InterruptLine::Core { line: 0..128, .. })
                    || route.outputs[..n].contains(output)
                {
                    return Err("GPIO interrupt requires unique per-core source connections".into());
                }
            }
        }
        let mut s = State {
            registers: [[0; 8]; 18],
            outputs: [0; 18],
            known: [0; 18],
            configuration_known: [[0; 4]; 18],
            pending: VecDeque::with_capacity(INPUT_QUEUE),
        };
        for (port, word) in options.input_levels.into_iter().enumerate() {
            s.registers[port][5] = word;
        }
        for output in options.output_levels {
            output.pin.validate()?;
            let port = output.pin.port as usize;
            let bit = 1 << output.pin.pin;
            if s.known[port] & bit != 0 {
                return Err("duplicate initial GPIO output".into());
            }
            s.known[port] |= bit;
            if output.high {
                s.outputs[port] |= bit;
            }
        }
        Ok(Self {
            pads: Pads {
                state: Arc::new(Mutex::new(s)),
                irqs: Arc::new(wiring.irqs),
            },
        })
    }
    pub fn pads(&self) -> Pads {
        self.pads.clone()
    }
    fn locate(address: u64, size: u8) -> Option<(usize, usize)> {
        let offset = address.checked_sub(BASE)?;
        let (port, word) = ((offset / 0x40) as usize, ((offset % 0x40) / 4) as usize);
        (size == 4
            && offset < 18 * 0x40
            && offset % 4 == 0
            && port < 18
            && offset % 0x40 < 0x20
            && (port < 16 || word != 5))
            .then_some((port, word))
    }
}
impl Device for Gpio {
    fn checkpoint(&self) -> Result<crate::checkpoint::Checkpoint, String> {
        crate::checkpoint::Checkpoint::encode(
            self.name(),
            &*self.pads.state.lock().map_err(|_| "Gpio state poisoned")?,
        )
    }
    fn validate_checkpoint(&self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        let _: State = saved.decode(self.name())?;
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        *self.pads.state.lock().map_err(|_| "Gpio state poisoned")? =
            saved.decode::<State>(self.name())?;
        Ok(())
    }

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "gpio"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some((port, word)) = Self::locate(address, size) else {
            cx.unmodeled();
            return 0;
        };
        let s = self.pads.state.lock().unwrap();
        if word == 0 {
            u64::from(s.outputs[port])
        } else {
            u64::from(s.registers[port][word])
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some((port, word)) = Self::locate(address, size) else {
            cx.unmodeled();
            return;
        };
        if word == 5 {
            cx.fail("GPIO input status is read-only");
            return;
        }
        let mut s = self.pads.state.lock().unwrap();
        let value = value as u32;
        if (1..=4).contains(&word) {
            let mask = value >> 16;
            s.registers[port][word] = (s.registers[port][word] & !mask) | (value & mask);
            s.configuration_known[port][word - 1] |= mask as u16;
        } else if word != 0 {
            s.registers[port][word] = value;
        }
        if word == 0 {
            let mask = (value >> 16) as u16;
            s.outputs[port] = (s.outputs[port] & !mask) | (value as u16 & mask);
            s.known[port] |= mask;
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::locate(address, size).is_some()
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        (!self.pads.state.lock().unwrap().pending.is_empty()).then_some(now)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        loop {
            let next = self.pads.state.lock().unwrap().pending.pop_front();
            let Some((pin, high)) = next else {
                break;
            };
            if let Err(error) = self.pads.drive(cx, pin, high) {
                cx.fail(error);
                break;
            }
        }
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&*self.pads.state.lock().unwrap()).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::{TestHost, read, write};
    fn options() -> Options {
        Options {
            input_levels: [0xffff; 16],
            output_levels: vec![],
        }
    }
    #[test]
    fn function_masks_preserve_other_pads_in_both_function_words() {
        let mut gpio = Gpio::new(options(), Wiring { irqs: vec![] }).unwrap();
        let pads = gpio.pads();
        let mut host = TestHost::new();
        write(&mut gpio, &mut host, BASE + 0x40 + 0xc, 0x30001000);
        write(&mut gpio, &mut host, BASE + 0x40 + 0xc, 0x000c0008);
        assert_eq!(pads.function(Pin { port: 1, pin: 6 }).unwrap(), 1);
        assert_eq!(pads.function(Pin { port: 1, pin: 1 }).unwrap(), 2);
        write(&mut gpio, &mut host, BASE + 0x40 + 0x10, 0xc000c000);
        write(&mut gpio, &mut host, BASE + 0x40 + 0x10, 0x00030001);
        assert_eq!(pads.function(Pin { port: 1, pin: 15 }).unwrap(), 3);
        assert_eq!(pads.function(Pin { port: 1, pin: 8 }).unwrap(), 1);
        write(&mut gpio, &mut host, BASE + 0x40 + 0xc, 0x30000000);
        assert_eq!(pads.function(Pin { port: 1, pin: 6 }).unwrap(), 0);
        assert_eq!(pads.function(Pin { port: 1, pin: 1 }).unwrap(), 2);
    }

    #[test]
    fn masked_writes_publish_known_output_levels_and_preserve_other_pins() {
        let mut gpio = Gpio::new(options(), Wiring { irqs: vec![] }).unwrap();
        let pads = gpio.pads();
        let pin = Pin { port: 17, pin: 3 };
        assert_eq!(pads.output_level(pin).unwrap(), None);
        let mut host = TestHost::new();
        write(&mut gpio, &mut host, BASE + 17 * 0x40, 0x00080008);
        assert_eq!(pads.output_level(pin).unwrap(), Some(true));
        write(&mut gpio, &mut host, BASE + 17 * 0x40, 0x00040004);
        assert_eq!(pads.output_level(pin).unwrap(), Some(true));
        assert_eq!(
            pads.output_level(Pin { port: 17, pin: 2 }).unwrap(),
            Some(true)
        );
        write(&mut gpio, &mut host, BASE + 17 * 0x40, 0x00080000);
        assert_eq!(pads.output_level(pin).unwrap(), Some(false));
        assert_eq!(
            pads.output_level(Pin { port: 17, pin: 2 }).unwrap(),
            Some(true)
        );
        assert_eq!(read(&mut gpio, &mut host, BASE + 17 * 0x40), 4);
        let mut cx = host.cx();
        gpio.write(&mut cx, BASE + 0x14, 4, 0);
        assert_eq!(cx.refusal(), Some("GPIO input status is read-only"));
        assert!(Gpio::locate(BASE + 16 * 0x40 + 0x14, 4).is_none());
        assert!(Gpio::locate(BASE + (1 << 40), 4).is_none());
    }
    #[test]
    fn external_requests_wait_for_guest_work_and_part_edges_use_the_same_pads() {
        let pin = Pin { port: 0, pin: 2 };
        let mut gpio = Gpio::new(
            options(),
            Wiring {
                irqs: vec![InputIrq {
                    pin,
                    edge: Edge::Falling,
                    outputs: vec![
                        InterruptLine::Core { core: 0, line: 10 },
                        InterruptLine::Core { core: 1, line: 10 },
                    ],
                }],
            },
        )
        .unwrap();
        let mut pads = gpio.pads();
        let mut host = TestHost::new();
        pads.request(pin, false).unwrap();
        assert!(pads.input_level(pin).unwrap());
        assert_eq!(gpio.next_deadline(42), Some(42));
        let saved = gpio.save();
        assert_eq!(gpio.save(), saved);
        assert!(host.irqs.is_empty());
        host.now = 42;
        gpio.advance(&mut host.cx());
        assert!(!pads.input_level(pin).unwrap());
        assert_eq!(gpio.next_deadline(42), None);
        assert_eq!(host.irqs, [(0, 10), (1, 10)]);
        assert_eq!(read(&mut gpio, &mut host, BASE + 0x14), 0xfffb);
        pads.input(&mut host.cx(), pin, false).unwrap();
        pads.input(&mut host.cx(), pin, true).unwrap();
        assert_eq!(host.irqs.len(), 2);
        pads.input(&mut host.cx(), pin, false).unwrap();
        assert_eq!(host.irqs.len(), 4);
    }
    #[test]
    fn invalid_wiring_and_queue_exhaustion_refuse() {
        let pin = Pin { port: 0, pin: 0 };
        assert!(
            Gpio::new(
                options(),
                Wiring {
                    irqs: vec![InputIrq {
                        pin,
                        edge: Edge::Both,
                        outputs: vec![]
                    }]
                }
            )
            .is_err()
        );
        let mut o = options();
        o.output_levels = vec![
            InitialOutput { pin, high: false },
            InitialOutput { pin, high: true },
        ];
        assert!(Gpio::new(o, Wiring { irqs: vec![] }).is_err());
        let gpio = Gpio::new(options(), Wiring { irqs: vec![] }).unwrap();
        let pads = gpio.pads();
        assert!(pads.request(Pin { port: 16, pin: 0 }, false).is_err());
        for _ in 0..INPUT_QUEUE {
            pads.request(pin, false).unwrap();
        }
        assert!(pads.request(pin, true).is_err());
    }
}

#[cfg(test)]
#[path = "gpio/tests.rs"]
mod pad_tests;
