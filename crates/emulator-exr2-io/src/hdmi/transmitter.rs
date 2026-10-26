//! EXR II HDMI transmitter startup and DDC register path.
//!
//! Startup selector values and HPD bit6 control the modeled transmitter. The
//! attached sink supplies cable state and EDID bytes; status timing and DDC
//! completion use guest virtual time. No transmitter lock or pixel output is synthesized.
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::parts::hdmi_sink::HdmiSink;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const TX: u64 = 0xfff76000;
pub const DDC: u64 = 0xfff77000;
pub const BRIDGE: u64 = 0xfff68000;
pub const HPD_STATUS: u64 = TX + 0x37c;
pub const READY_STATUS: u64 = TX + 0x2dc;
pub const INT_STATUS: u64 = TX + 0x250;
pub const EDID_CONTROL: u64 = TX + 0x248;
pub const EDID_SEGMENT: u64 = TX + 0x310;
pub const EDID_OFFSET: u64 = TX + 0x314;
pub const EDID_WINDOW: u64 = DDC + 0x200;
const EDID_WINDOW_WORDS: usize = 128;

const TX_STORED_OFFSETS: &[u64] = &[
    0x000, 0x004, 0x008, 0x00c, 0x01c, 0x020, 0x024, 0x028, 0x02c, 0x030, 0x034, 0x040, 0x044,
    0x048, 0x04c, 0x050, 0x054, 0x058, 0x0c0, 0x0c4, 0x0c8, 0x0cc, 0x0d0, 0x0d4, 0x0d8, 0x0dc,
    0x0e0, 0x0e4, 0x0e8, 0x0f4, 0x0f8, 0x0fc, 0x104, 0x108, 0x114, 0x248, 0x254, 0x2bc, 0x310,
    0x314,
];
const DDC_STORED_OFFSETS: &[u64] = &[
    0x018, 0x024, 0x028, 0x02c, 0x034, 0x038, 0x03c, 0x040, 0x044, 0x050, 0x058, 0x084,
];

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default = "default_edid_ticks")]
    pub edid_ticks: u64,
}

const fn default_edid_ticks() -> u64 {
    2000
}

impl Default for Options {
    fn default() -> Self {
        Self {
            edid_ticks: default_edid_ticks(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct ReadRequest {
    segment: u32,
    offset: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    registers: BTreeMap<u64, u32>,
    window: Vec<u8>,
    edid_request: Option<ReadRequest>,
    edid_deadline: Option<u64>,
    generation: u64,
}

/// Board connections. The hot-plug output is a per-core interrupt source.
pub struct Wiring {
    pub hot_plug: InterruptLine,
}

pub struct Transmitter {
    state: State,
    sink: HdmiSink,
    edid_ticks: u64,
    hot_plug: (usize, u32),
}

impl Transmitter {
    pub fn new(options: Options, sink: HdmiSink, wiring: Wiring) -> Result<Self, String> {
        if options.edid_ticks == 0 {
            return Err("HDMI EDID delay must be positive".into());
        }
        let InterruptLine::Core { core, line } = wiring.hot_plug else {
            return Err("HDMI hot-plug output requires a per-core interrupt source".into());
        };
        let mut registers = BTreeMap::new();
        for offset in TX_STORED_OFFSETS {
            registers.insert(TX + offset, 0);
        }
        for index in 0..32 {
            registers.insert(TX + 0x17c + index * 4, 0);
        }
        for offset in DDC_STORED_OFFSETS {
            registers.insert(DDC + offset, 0);
        }
        for index in 0..7 {
            registers.insert(BRIDGE + index * 4, 0);
        }
        registers.insert(INT_STATUS, 0);
        Ok(Self {
            state: State {
                registers,
                window: vec![0; EDID_WINDOW_WORDS],
                edid_request: None,
                edid_deadline: None,
                generation: 0,
            },
            sink,
            edid_ticks: options.edid_ticks,
            hot_plug: (core, line),
        })
    }

    pub fn set_connected(&mut self, cx: &mut Cx<'_>, connected: bool) {
        let was_connected = self.sink.connected();
        if was_connected == connected {
            return;
        }
        self.sink.set_connected(connected);
        if connected {
            let (core, line) = self.hot_plug;
            if let Err(error) = cx.raise_irq(core, line) {
                cx.fail(format!(
                    "HDMI hot-plug interrupt could not be raised: {error}"
                ));
            }
        } else {
            self.state.edid_request = None;
            self.state.edid_deadline = None;
            self.state.generation = self.state.generation.wrapping_add(1);
            self.state.registers.insert(INT_STATUS, 0);
        }
    }

    fn is_stored(address: u64) -> bool {
        address == INT_STATUS
            || TX_STORED_OFFSETS
                .iter()
                .any(|offset| TX + *offset == address)
            || (TX + 0x17c..TX + 0x1fc).contains(&address) && (address - (TX + 0x17c)) % 4 == 0
            || DDC_STORED_OFFSETS
                .iter()
                .any(|offset| DDC + *offset == address)
            || (BRIDGE..BRIDGE + 28).contains(&address) && (address - BRIDGE) % 4 == 0
    }

    fn address_kind(address: u64, size: u8) -> Option<Kind> {
        if size != 4 || address & 3 != 0 {
            return None;
        }
        if address == HPD_STATUS || address == READY_STATUS {
            Some(Kind::ReadOnly)
        } else if address == INT_STATUS {
            Some(Kind::Status)
        } else if (EDID_WINDOW..EDID_WINDOW + EDID_WINDOW_WORDS as u64 * 4).contains(&address) {
            Some(Kind::Window)
        } else if Self::is_stored(address) {
            Some(Kind::Stored)
        } else {
            None
        }
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self
            .state
            .edid_deadline
            .is_some_and(|deadline| deadline <= cx.now())
        {
            return;
        }
        self.state.edid_deadline = None;
        let Some(request) = self.state.edid_request.take() else {
            return;
        };
        match self.sink.read_block(request.segment, request.offset) {
            Ok(block) => {
                self.state.window = block.to_vec();
                *self.state.registers.entry(INT_STATUS).or_default() |= 4;
            }
            Err(_) => {
                *self.state.registers.entry(INT_STATUS).or_default() |= 2;
            }
        }
    }

    fn valid_state(state: &State) -> bool {
        let expected = TX_STORED_OFFSETS.len() + 32 + DDC_STORED_OFFSETS.len() + 8;
        state.registers.len() == expected
            && state.window.len() == EDID_WINDOW_WORDS
            && state
                .registers
                .keys()
                .all(|address| Self::is_stored(*address))
            && state.edid_request.is_some() == state.edid_deadline.is_some()
            && (state.edid_request.is_none() || state.generation != 0)
            && state.registers.get(&HPD_STATUS).is_none()
            && state.registers.get(&READY_STATUS).is_none()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    ReadOnly,
    Status,
    Window,
    Stored,
}

impl Device for Transmitter {
    fn hdmi_output(&self) -> Option<&(dyn fujimod_emulator_devices::capability::HdmiOutput + 'static)> {
        Some(self)
    }
    fn hdmi_output_mut(&mut self) -> Option<&mut (dyn fujimod_emulator_devices::capability::HdmiOutput + 'static)> {
        Some(self)
    }
    fujimod_emulator_devices::checkpoint_fields!(state: State, sink: HdmiSink);
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-hdmi-transmitter"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(kind) = Self::address_kind(address, size) else {
            cx.unmodeled();
            return 0;
        };
        self.advance(cx);
        match kind {
            Kind::ReadOnly if address == HPD_STATUS => u64::from(self.sink.connected()) << 6,
            Kind::ReadOnly => {
                if self.sink.connected() {
                    3
                } else {
                    0
                }
            }
            Kind::Status | Kind::Stored => {
                u64::from(self.state.registers.get(&address).copied().unwrap_or(0))
            }
            Kind::Window => {
                let index = ((address - EDID_WINDOW) / 4) as usize;
                u64::from(self.state.window[index])
            }
        }
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(kind) = Self::address_kind(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u64::from(u32::MAX) {
            cx.fail("HDMI register value exceeds 32 bits");
            return;
        }
        let value = value as u32;
        self.advance(cx);
        match kind {
            Kind::ReadOnly | Kind::Window => {
                cx.fail("HDMI status and EDID window are read-only");
            }
            Kind::Status => {
                let status = self.state.registers.entry(INT_STATUS).or_default();
                *status &= !value;
            }
            Kind::Stored => {
                if address == TX
                    && !self.sink.connected()
                    && !matches!(value, 0x10 | 0x20 | 0x40 | 0x80)
                {
                    cx.fail(format!(
                        "unknown HDMI transmitter startup control {value:#x}"
                    ));
                    return;
                }
                if address == DDC + 0x40 && !self.sink.connected() && !matches!(value, 0 | 1) {
                    cx.fail(format!("unknown HDMI DDC startup enable {value:#x}"));
                    return;
                }
                if address == DDC + 0x50 && !self.sink.connected() && !matches!(value, 0 | 8) {
                    cx.fail(format!("unknown HDMI DDC startup control {value:#x}"));
                    return;
                }
                let starts_edid = address == EDID_OFFSET
                    && self
                        .state
                        .registers
                        .get(&EDID_CONTROL)
                        .copied()
                        .unwrap_or(0)
                        & 6
                        == 6;
                let deadline_and_generation = if starts_edid {
                    let Some(deadline) = cx.now().checked_add(self.edid_ticks) else {
                        cx.fail("HDMI EDID deadline overflow");
                        return;
                    };
                    let Some(generation) = self.state.generation.checked_add(1) else {
                        cx.fail("HDMI EDID generation overflow");
                        return;
                    };
                    Some((deadline, generation))
                } else {
                    None
                };
                self.state.registers.insert(address, value);
                if let Some((deadline, generation)) = deadline_and_generation {
                    self.state.generation = generation;
                    self.state.edid_request = Some(ReadRequest {
                        segment: self
                            .state
                            .registers
                            .get(&EDID_SEGMENT)
                            .copied()
                            .unwrap_or(0),
                        offset: value,
                    });
                    self.state.edid_deadline = Some(deadline);
                    cx.schedule(deadline, generation);
                }
            }
        }
    }

    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.edid_deadline
    }

    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.state.generation {
            self.advance(cx);
        }
    }

    fn save(&self) -> Value {
        json!({
            "state": self.state,
            "sink": self.sink.save(),
            "edid_ticks": self.edid_ticks,
        })
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["state"].clone())
            .map_err(|error| format!("invalid HDMI transmitter state: {error}"))?;
        if value["edid_ticks"].as_u64() != Some(self.edid_ticks) || !Self::valid_state(&state) {
            return Err("invalid HDMI transmitter checkpoint configuration".into());
        }
        self.sink.restore(&value["sink"])?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::parts::hdmi_sink::Options as SinkOptions;
    use fujimod_emulator_devices::{Host, Record};

    #[derive(Default)]
    struct Clock {
        now: u64,
        deadlines: Vec<(usize, u64, u64)>,
        irqs: Vec<(usize, u32)>,
    }

    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, device: usize, at: u64, token: u64) {
            self.deadlines.push((device, at, token));
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
        }
        fn raise_irq(&mut self, cpu: usize, line: u32) -> Result<(), String> {
            self.irqs.push((cpu, line));
            Ok(())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }

    fn write(
        model: &mut Transmitter,
        host: &mut Clock,
        address: u64,
        value: u64,
    ) -> Option<String> {
        let mut cx = Cx::new(host, 0, 0, 9);
        model.write(&mut cx, address, 4, value);
        cx.take_refusal()
    }

    fn read(model: &mut Transmitter, host: &mut Clock, address: u64) -> (u64, Option<String>) {
        let mut cx = Cx::new(host, 0, 0, 9);
        let value = model.read(&mut cx, address, 4);
        (value, cx.take_refusal())
    }

    fn transmitter(connected: bool, edid: Vec<u8>) -> Transmitter {
        let sink = HdmiSink::new(SinkOptions { edid, connected }).unwrap();
        Transmitter::new(
            Options { edid_ticks: 2 },
            sink,
            Wiring {
                hot_plug: InterruptLine::Core { core: 1, line: 17 },
            },
        )
        .unwrap()
    }

    #[test]
    fn startup_registers_follow_the_stock_selector_and_read_only_hpd_law() {
        let mut model = transmitter(false, Vec::new());
        let mut host = Clock::default();
        assert_eq!(read(&mut model, &mut host, HPD_STATUS), (0, None));
        for value in [0x10, 0x20, 0x40, 0x80] {
            assert_eq!(write(&mut model, &mut host, TX, value), None);
        }
        assert_eq!(write(&mut model, &mut host, DDC + 0x40, 1), None);
        assert_eq!(write(&mut model, &mut host, DDC + 0x50, 8), None);
        assert!(write(&mut model, &mut host, TX, 0x11).is_some());
        assert!(write(&mut model, &mut host, HPD_STATUS, 0).is_some());
    }

    #[test]
    fn connected_edid_reads_complete_only_at_the_guest_deadline() {
        let edid = (0..256).map(|value| value as u8).collect::<Vec<_>>();
        let mut model = transmitter(true, edid);
        let mut host = Clock {
            now: 5,
            ..Clock::default()
        };
        assert_eq!(read(&mut model, &mut host, HPD_STATUS), (0x40, None));
        assert_eq!(read(&mut model, &mut host, READY_STATUS), (3, None));
        assert_eq!(write(&mut model, &mut host, EDID_CONTROL, 0xc6), None);
        assert_eq!(write(&mut model, &mut host, EDID_SEGMENT, 0), None);
        assert_eq!(write(&mut model, &mut host, EDID_OFFSET, 0x80), None);
        assert_eq!(model.next_deadline(host.now), Some(7));
        assert_eq!(read(&mut model, &mut host, EDID_WINDOW), (0, None));
        host.now = 7;
        assert_eq!(read(&mut model, &mut host, EDID_WINDOW), (128, None));
        assert_eq!(read(&mut model, &mut host, INT_STATUS), (4, None));
    }

    #[test]
    fn hot_plug_output_requires_a_per_core_interrupt_source() {
        let sink = HdmiSink::new(SinkOptions {
            edid: Vec::new(),
            connected: false,
        })
        .unwrap();
        let shared = Wiring {
            hot_plug: InterruptLine::Shared(9),
        };
        assert!(Transmitter::new(Options { edid_ticks: 2 }, sink, shared).is_err());
    }

    #[test]
    fn hotplug_irq_and_unplug_cancel_pending_edid() {
        let mut model = transmitter(false, vec![0; 128]);
        let mut host = Clock {
            now: 10,
            ..Clock::default()
        };
        {
            let mut cx = Cx::new(&mut host, 0, 0, 9);
            model.set_connected(&mut cx, true);
        }
        assert_eq!(host.irqs, [(1, 17)]);
        write(&mut model, &mut host, EDID_CONTROL, 6);
        write(&mut model, &mut host, EDID_OFFSET, 0);
        assert_eq!(model.next_deadline(host.now), Some(12));
        {
            let mut cx = Cx::new(&mut host, 0, 0, 9);
            model.set_connected(&mut cx, false);
        }
        assert_eq!(model.next_deadline(host.now), None);
        assert_eq!(read(&mut model, &mut host, HPD_STATUS), (0, None));
    }

    #[test]
    fn checkpoint_keeps_a_pending_read_and_sink_configuration() {
        let mut model = transmitter(true, vec![0x5a; 128]);
        let mut host = Clock::default();
        write(&mut model, &mut host, EDID_CONTROL, 6);
        write(&mut model, &mut host, EDID_OFFSET, 0);
        let saved = model.save();
        let mut restored = transmitter(true, vec![0x5a; 128]);
        restored.restore(&saved).unwrap();
        assert_eq!(restored.next_deadline(0), Some(2));
        host.now = 2;
        assert_eq!(read(&mut restored, &mut host, EDID_WINDOW), (0x5a, None));
        assert!(
            restored
                .restore(&json!({"state":{},"sink":{},"edid_ticks":2}))
                .is_err()
        );
    }
}

impl fujimod_emulator_devices::capability::HdmiOutput for Transmitter {
    fn set_connected(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>, connected: bool) {
        Transmitter::set_connected(self, cx, connected)
    }
}
