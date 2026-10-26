//! XP5 polled SPI controller. GPIO selection routes wire frames to attached
//! parts; FIFO readiness follows serial clock edges on the guest timeline.
use fujimod_emulator_devices::xp5::pins::SharedPins;
use fujimod_emulator_parts_io::serial::{Endpoint, Frame};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Select {
    pub port: usize,
    pub pin: usize,
    pub active_high: bool,
}
impl Select {
    fn active(self, pins: &fujimod_emulator_devices::xp5::pins::Pins) -> bool {
        (pins.output_level(self.port, self.pin) != 0) == self.active_high
    }
}
pub struct Attachment {
    select: Select,
    endpoint: Box<dyn Endpoint>,
}
impl Attachment {
    pub fn new(select: Select, endpoint: Box<dyn Endpoint>) -> Self {
        Self { select, endpoint }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct WireState {
    generation: u64,
    last_endpoint: Option<usize>,
    last_epoch: u64,
    active: Option<WireFrame>,
}
#[derive(Serialize, Deserialize)]
struct WireFrame {
    endpoint: usize,
    epoch: u64,
    frame: Frame,
}
/// GPIO chip selection and attached part traffic for the common serial engine.
pub struct GpioWire {
    pins: SharedPins,
    attachments: Vec<Attachment>,
    state: WireState,
}
impl GpioWire {
    pub fn new(pins: SharedPins, attachments: Vec<Attachment>) -> Result<Self, String> {
        if attachments.is_empty()
            || attachments.iter().enumerate().any(|(i, a)| {
                a.select.port >= 20
                    || a.select.pin >= 16
                    || attachments[..i]
                        .iter()
                        .any(|b| b.select.port == a.select.port && b.select.pin == a.select.pin)
            })
        {
            return Err("GPIO serial wire requires distinct valid chip selects".into());
        }
        Ok(Self {
            pins,
            attachments,
            state: WireState::default(),
        })
    }
}
impl fujimod_emulator_parts_io::serial::Wire for GpioWire {
    fn begin(&mut self, frame: Frame) -> Result<u64, String> {
        if self.state.active.is_some() {
            return Err("serial wire already shifting".into());
        }
        let pins = self.pins.lock().map_err(|_| "GPIO pins poisoned")?;
        let selected: Vec<_> = self
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, a)| a.select.active(&pins))
            .map(|(i, _)| i)
            .collect();
        let [endpoint] = selected.as_slice() else {
            return Err("serial wire requires exactly one selected part".into());
        };
        let endpoint = *endpoint;
        let select = self.attachments[endpoint].select;
        let epoch = pins.output_rising_edges(select.port, select.pin);
        drop(pins);
        if self.state.last_endpoint != Some(endpoint) || self.state.last_epoch != epoch {
            self.attachments[endpoint].endpoint.reset_transaction();
        }
        self.attachments[endpoint].endpoint.validate(frame)?;
        self.state.generation = self
            .state
            .generation
            .checked_add(1)
            .ok_or("serial token overflow")?;
        self.state.last_endpoint = Some(endpoint);
        self.state.last_epoch = epoch;
        self.state.active = Some(WireFrame {
            endpoint,
            epoch,
            frame,
        });
        Ok(self.state.generation)
    }
    fn finish(&mut self, token: u64, frame: Frame, now: u64) -> Result<u16, String> {
        let active = self
            .state
            .active
            .as_ref()
            .ok_or("serial wire has no active frame")?;
        if token != self.state.generation || frame != active.frame {
            return Err("serial frame token differs".into());
        }
        let pins = self.pins.lock().map_err(|_| "GPIO pins poisoned")?;
        let attachment = &self.attachments[active.endpoint];
        if !attachment.select.active(&pins)
            || pins.output_rising_edges(attachment.select.port, attachment.select.pin)
                != active.epoch
        {
            self.state.active = None;
            return Err("serial chip select changed before the final edge".into());
        }
        drop(pins);
        let endpoint = self.state.active.take().unwrap().endpoint;
        Ok(self.attachments[endpoint].endpoint.transfer(frame, now))
    }
    fn save(&self) -> Value {
        json!({"state":self.state,"parts":self.attachments.iter().map(|a|json!({"select":a.select,"kind":a.endpoint.name(),"state":a.endpoint.save()})).collect::<Vec<_>>()})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let state: WireState =
            serde_json::from_value(v["state"].clone()).map_err(|e| e.to_string())?;
        let parts = v["parts"]
            .as_array()
            .filter(|p| p.len() == self.attachments.len())
            .ok_or("serial wire layout differs")?;
        if state.last_endpoint.is_some_and(|i| i >= parts.len())
            || state.active.as_ref().is_some_and(|p| {
                p.endpoint >= parts.len()
                    || state.generation == 0
                    || state.last_endpoint != Some(p.endpoint)
                    || state.last_epoch != p.epoch
            })
        {
            return Err("invalid serial wire checkpoint".into());
        }
        for (a, p) in self.attachments.iter().zip(parts) {
            if p["select"] != json!(a.select) || p["kind"] != a.endpoint.name() {
                return Err("serial part wiring differs".into());
            }
        }
        let previous: Vec<_> = self.attachments.iter().map(|a| a.endpoint.save()).collect();
        let result = (|| {
            for (a, p) in self.attachments.iter_mut().zip(parts) {
                a.endpoint.restore(&p["state"])?;
            }
            if let Some(p) = &state.active {
                self.attachments[p.endpoint].endpoint.validate(p.frame)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            for (a, p) in self.attachments.iter_mut().zip(previous) {
                a.endpoint.restore(&p)?;
            }
            return Err(error);
        }
        self.state = state;
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    at: u64,
    token: u64,
    endpoint: usize,
    cs_epoch: u64,
    word: u16,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    control: u32,
    enable: u32,
    divider: u32,
    irq_enable: u32,
    causes: u32,
    rx: Option<u16>,
    pending: Option<Pending>,
    generation: u64,
    last_endpoint: Option<usize>,
    last_cs_epoch: u64,
}
pub struct Spi {
    base: u64,
    state: State,
    pins: SharedPins,
    source_hz: u64,
    attachments: Vec<Attachment>,
}
impl Spi {
    pub fn new(
        base: u64,
        pins: SharedPins,
        source_hz: u64,
        attachments: Vec<Attachment>,
    ) -> Result<Self, String> {
        if source_hz == 0
            || attachments.is_empty()
            || attachments.iter().enumerate().any(|(i, a)| {
                a.select.port >= 20
                    || a.select.pin >= 16
                    || attachments[..i]
                        .iter()
                        .any(|b| b.select.port == a.select.port && b.select.pin == a.select.pin)
            })
        {
            return Err("SPI requires a source clock and distinct valid GPIO chip selects".into());
        }
        Ok(Self {
            base,
            state: State::default(),
            pins,
            source_hz,
            attachments,
        })
    }
    fn frame(&self, word: u16) -> Frame {
        Frame {
            bits: ((self.state.control & 15) + 1) as u8,
            mode: ((self.state.control >> 6) & 3) as u8,
            word,
        }
    }
    fn start(&mut self, cx: &mut Cx<'_>, value: u64) -> Result<(), String> {
        if self.state.enable & 2 == 0 || self.state.divider == 0 {
            return Err("SPI transfer requires an enabled master and divider".into());
        }
        if self.state.pending.is_some() || self.state.rx.is_some() {
            return Err("SPI streaming beyond one polled word is not modeled".into());
        }
        let frame = self.frame(value as u16);
        if value >= (1u64 << frame.bits) {
            return Err("SPI TX exceeds configured word length".into());
        }
        let pins = self.pins.lock().unwrap();
        let selected: Vec<_> = self
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, a)| a.select.active(&pins))
            .map(|(i, _)| i)
            .collect();
        if selected.len() != 1 {
            return Err(format!(
                "SPI requires exactly one selected endpoint, found {selected:?}"
            ));
        }
        let endpoint = selected[0];
        let select = self.attachments[endpoint].select;
        let epoch = pins.output_rising_edges(select.port, select.pin);
        drop(pins);
        self.attachments[endpoint].endpoint.validate(frame)?;
        let cycles = u128::from(frame.bits)
            * u128::from(self.state.divider)
            * u128::from((self.state.control >> 8 & 255) + 1);
        let ns = (cycles * 1_000_000_000).div_ceil(u128::from(self.source_hz));
        let at = cx
            .now()
            .checked_add(u64::try_from(ns).map_err(|_| "SPI period overflow")?)
            .ok_or("SPI deadline overflow")?;
        if self.state.last_endpoint != Some(endpoint) || self.state.last_cs_epoch != epoch {
            self.attachments[endpoint].endpoint.reset_transaction();
        }
        self.state.last_endpoint = Some(endpoint);
        self.state.last_cs_epoch = epoch;
        self.state.generation += 1;
        self.state.pending = Some(Pending {
            at,
            token: self.state.generation,
            endpoint,
            cs_epoch: epoch,
            word: frame.word,
        });
        cx.schedule(at, self.state.generation);
        Ok(())
    }
}
impl Device for Spi {
    fn name(&self) -> &'static str {
        "xp5-spi"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if size != 4 {
            cx.unmodeled();
            return 0;
        }
        self.advance(cx);
        match address.wrapping_sub(self.base) {
            0 => self.state.control as u64,
            4 => self.state.enable as u64,
            8 => self.state.rx.take().unwrap_or(0) as u64,
            0xc => {
                if self.state.pending.is_some() {
                    0x10
                } else {
                    3 | (u64::from(self.state.rx.is_some()) << 2)
                }
            }
            0x10 => self.state.divider as u64,
            0x14 => self.state.causes as u64,
            0x24 => self.state.irq_enable as u64,
            0x20 => 0,
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if size != 4 {
            cx.unmodeled();
            return;
        }
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        let offset = address.wrapping_sub(self.base);
        if self.state.pending.is_some() && offset != 0x14 {
            cx.fail("SPI register change while shifting is not modeled");
            return;
        }
        match offset {
            0 if value & !0xffcf == 0 && value & 15 >= 3 => self.state.control = value as u32,
            4 if value & !0xa == 0 => self.state.enable = value as u32,
            8 => {
                if let Err(reason) = self.start(cx, value) {
                    cx.fail(reason);
                }
            }
            0x10 if (2..=254).contains(&value) && value & 1 == 0 => {
                self.state.divider = value as u32
            }
            0x14 if value & !15 == 0 => self.state.causes &= !(value as u32),
            0x24 if value == 0 => self.state.irq_enable = 0,
            0x20 if value & !3 == 0 => {
                if value & 1 != 0 {
                    self.state.rx = None;
                }
            }
            0 | 4 | 0x10 | 0x14 | 0x20 | 0x24 => {
                cx.fail(format!("unsupported SPI register {offset:#x}={value:#x}"))
            }
            _ => cx.unmodeled(),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let Some(p) = self
            .state
            .pending
            .as_ref()
            .filter(|p| p.at <= cx.now())
            .cloned()
        else {
            return;
        };
        let pins = self.pins.lock().unwrap();
        let select = self.attachments[p.endpoint].select;
        if !select.active(&pins)
            || pins.output_rising_edges(select.port, select.pin) != p.cs_epoch
            || self
                .attachments
                .iter()
                .enumerate()
                .any(|(i, a)| i != p.endpoint && a.select.active(&pins))
        {
            cx.fail("SPI chip select changed before the last clock edge");
            return;
        }
        drop(pins);
        let frame = self.frame(p.word);
        let reply = self.attachments[p.endpoint].endpoint.transfer(frame, p.at);
        self.state.pending = None;
        self.state.rx = Some(reply);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.pending.as_ref().map(|p| p.at)
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if self
            .state
            .pending
            .as_ref()
            .is_some_and(|p| p.token == token)
        {
            self.advance(cx);
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        size == 4 && address == self.base + 0xc
    }
    fn save(&self) -> Value {
        json!({"base":self.base,"source_hz":self.source_hz,"controller":self.state,
            "endpoints":self.attachments.iter().map(|a|json!({"select":a.select,"kind":a.endpoint.name(),"state":a.endpoint.save()})).collect::<Vec<_>>()})
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        if state["base"].as_u64() != Some(self.base)
            || state["source_hz"].as_u64() != Some(self.source_hz)
        {
            return Err("SPI checkpoint clock/address differs".into());
        }
        let endpoints = state["endpoints"]
            .as_array()
            .filter(|a| a.len() == self.attachments.len())
            .ok_or("SPI checkpoint endpoint layout differs")?;
        for (a, s) in self.attachments.iter().zip(endpoints) {
            if s["select"] != json!(a.select) || s["kind"] != a.endpoint.name() {
                return Err("SPI checkpoint endpoint wiring differs".into());
            }
        }
        let decoded: State =
            serde_json::from_value(state["controller"].clone()).map_err(|e| e.to_string())?;
        if decoded
            .last_endpoint
            .is_some_and(|i| i >= self.attachments.len())
            || decoded.pending.as_ref().is_some_and(|p| {
                p.endpoint >= self.attachments.len() || p.token != decoded.generation
            })
            || decoded.rx.is_some() && decoded.pending.is_some()
            || decoded.control & !0xffcf != 0
            || decoded.control != 0 && decoded.control & 15 < 3
            || decoded.enable & !0xa != 0
            || decoded.divider != 0
                && (!(2..=254).contains(&decoded.divider) || decoded.divider & 1 != 0)
            || decoded.irq_enable != 0
            || decoded.causes != 0
        {
            return Err("invalid SPI checkpoint transfer state".into());
        }
        if let Some(p) = &decoded.pending {
            if decoded.enable & 2 == 0 || decoded.divider == 0 {
                return Err("SPI checkpoint has a transfer without a serial clock".into());
            }
            self.attachments[p.endpoint].endpoint.validate(Frame {
                bits: ((decoded.control & 15) + 1) as u8,
                mode: ((decoded.control >> 6) & 3) as u8,
                word: p.word,
            })?;
        }
        for (a, s) in self.attachments.iter_mut().zip(endpoints) {
            a.endpoint.restore(&s["state"])?;
        }
        self.state = decoded;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_parts_io::{power_controller::PowerController, serial_calibration::CalibrationRegisters};
    use crate::gpio::Gpio;
    use fujimod_emulator_devices::xp5::pins::Pins;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    const BASE: u64 = 0xff910000;
    fn fixture(miso: bool) -> (Spi, Gpio, RecordingHost) {
        let pins = Pins::shared();
        let gpio = Gpio::new(0xff907000, pins.clone(), false, vec![]).unwrap();
        let spi = Spi::new(
            BASE,
            pins,
            48_000_000,
            vec![
                Attachment::new(
                    Select {
                        port: 12,
                        pin: 10,
                        active_high: true,
                    },
                    Box::new(PowerController::new()),
                ),
                Attachment::new(
                    Select {
                        port: 12,
                        pin: 8,
                        active_high: false,
                    },
                    Box::new(CalibrationRegisters::new(miso)),
                ),
            ],
        )
        .unwrap();
        (spi, gpio, RecordingHost::default())
    }
    fn pins(gpio: &mut Gpio, host: &mut RecordingHost, power: bool, calibration: bool) {
        let bits = (u64::from(power) << 10) | (u64::from(!calibration) << 8);
        let mut cx = Cx::new(host, 71, 0xdeadbeef, 6);
        gpio.write(&mut cx, 0xff907300, 4, (0x500 << 16) | bits);
        assert!(cx.refusal().is_none());
    }
    fn write(spi: &mut Spi, host: &mut RecordingHost, offset: u64, word: u64) {
        let mut cx = Cx::new(host, 37, 0xcafebabe, 6);
        spi.write(&mut cx, BASE + offset, 4, word);
        assert!(cx.refusal().is_none(), "{:?}", cx.refusal());
    }
    fn setup(spi: &mut Spi, host: &mut RecordingHost, control: u64, divider: u64) {
        write(spi, host, 0, control);
        write(spi, host, 0x10, divider);
        write(spi, host, 4, 10);
    }
    fn transfer(spi: &mut Spi, host: &mut RecordingHost, word: u16) -> u64 {
        write(spi, host, 8, word.into());
        let at = spi.next_deadline(host.now).unwrap();
        let saved = spi.save();
        host.now = at - 1;
        spi.advance(&mut Cx::new(host, 0, 0, 6));
        assert_eq!(spi.save(), saved);
        assert_eq!(spi.read(&mut Cx::new(host, 0, 0, 6), BASE + 0xc, 4), 0x10);
        host.now = at;
        let token = spi.state.generation;
        spi.event(&mut Cx::new(host, 0, 0, 6), token);
        assert_eq!(spi.next_deadline(host.now), None);
        assert_eq!(spi.read(&mut Cx::new(host, 0, 0, 6), BASE + 0xc, 4), 7);
        let done = spi.save();
        spi.event(&mut Cx::new(host, 0, 0, 6), token);
        assert_eq!(spi.save(), done);
        spi.read(&mut Cx::new(host, 0, 0, 6), BASE + 8, 4)
    }
    #[test]
    fn physical_selects_route_calibration_writes_and_power_packets_without_caller_knowledge() {
        for miso in [false, true] {
            let (mut spi, mut gpio, mut host) = fixture(miso);
            pins(&mut gpio, &mut host, false, true);
            setup(&mut spi, &mut host, 15, 6);
            let channels = [0, 1, 0x155, 0x280, 0x3fe, 0x3ff];
            for (i, value) in channels.into_iter().enumerate() {
                let before = host.now;
                assert_eq!(
                    transfer(&mut spi, &mut host, ((i as u16 + 2) << 12) | (value << 2)),
                    if miso { 0xffff } else { 0 }
                );
                assert_eq!(host.now - before, 2_000); // 16 bits * divider6 /48MHz
            }
            transfer(&mut spi, &mut host, 0xb381);
            let saved = spi.save();
            assert_eq!(
                saved["endpoints"][1]["state"]["registers"]["channels"],
                json!(channels)
            );
            assert_eq!(
                saved["endpoints"][1]["state"]["registers"]["control"],
                0x381
            );
            assert_eq!(saved["endpoints"][1]["state"]["registers"]["transfers"], 7);
            // The same controller now addresses the independent power part.
            pins(&mut gpio, &mut host, true, false);
            setup(&mut spi, &mut host, 0xc7, 32);
            let reply = [3, 0, 0, !3u8].map(|n| transfer(&mut spi, &mut host, n.into()));
            assert_eq!(reply, [3, 2, 88, (!93u8).into()]);
            assert_eq!(spi.save()["endpoints"][1], saved["endpoints"][1]);
            assert!(host.irqs.is_empty());
        }
    }
    #[test]
    fn invalid_modes_words_and_selects_refuse_without_data_or_deadlines() {
        let (mut spi, mut gpio, mut host) = fixture(false);
        for (power, calibration, control, word) in [
            (false, false, 15, 0x2280),
            (true, true, 15, 0x2280),
            (true, false, 15, 0x2280),
            (false, true, 0xc7, 0x92),
            (false, true, 15, 0x2281),
            (false, true, 15, 0x8000),
        ] {
            pins(&mut gpio, &mut host, power, calibration);
            setup(&mut spi, &mut host, control, 6);
            let before = spi.save();
            let events = host.events.len();
            let mut cx = Cx::new(&mut host, 99, 0x14d3b70, 6);
            spi.write(&mut cx, BASE + 8, 4, word);
            assert!(cx.refusal().is_some());
            assert_eq!(spi.save(), before);
            assert_eq!(host.events.len(), events);
        }
    }
    #[test]
    fn chip_select_cycles_reset_power_packets_and_abort_incomplete_words() {
        let (mut spi, mut gpio, mut host) = fixture(false);
        pins(&mut gpio, &mut host, true, false);
        setup(&mut spi, &mut host, 0xc7, 32);
        transfer(&mut spi, &mut host, 0x92);
        pins(&mut gpio, &mut host, false, false);
        pins(&mut gpio, &mut host, true, false);
        assert_eq!(
            [3, 0, 0, !3u8].map(|n| transfer(&mut spi, &mut host, n.into())),
            [3, 2, 88, (!93u8).into()]
        );
        write(&mut spi, &mut host, 8, 0x92);
        let at = spi.next_deadline(host.now).unwrap();
        pins(&mut gpio, &mut host, false, false);
        pins(&mut gpio, &mut host, true, false);
        host.now = at;
        let before = spi.save();
        let mut cx = Cx::new(&mut host, 0, 0, 6);
        spi.advance(&mut cx);
        assert!(cx.refusal().unwrap().contains("chip select changed"));
        assert_eq!(spi.save(), before);
    }
    #[test]
    fn pending_checkpoint_preserves_clock_phase_and_endpoint_state() {
        let (mut spi, mut gpio, mut host) = fixture(false);
        pins(&mut gpio, &mut host, false, true);
        setup(&mut spi, &mut host, 15 | (3 << 8), 6);
        write(&mut spi, &mut host, 8, 0x2280);
        assert_eq!(spi.next_deadline(0), Some(8_000));
        let saved = spi.save();
        spi.restore(&saved).unwrap();
        assert_eq!(spi.save(), saved);
        host.now = 8_001;
        spi.advance(&mut Cx::new(&mut host, 0, 0, 6));
        assert_eq!(
            spi.save()["endpoints"][1]["state"]["registers"]["channels"][0],
            0xa0
        );
        assert!(spi.restore(&json!({})).is_err());
        assert!(host.irqs.is_empty());
    }
}
