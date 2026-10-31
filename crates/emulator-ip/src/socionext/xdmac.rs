//! Bounded XDMAC direct-transfer mechanism. Register facts: Linux's Milbeaut
//! M10V driver, Fujitsu MB86R12 chapter 8, and the matching request controls
//! in Module manual ЮФКВ.431268.008РЭ, table 5.99. This is a related contract;
//! exact XP5 controller identity is unproved. Board maps, request selectors
//! and interrupt destinations belong to the board graph.
//!
//! This models causal transfers, with no bus-arbitration timing claim: a ready
//! beat runs at `now`. No timer is invented while waiting for a peripheral.
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::dma::{Direction, SharedPort};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const ENABLE: u32 = 1 << 28;
const SOFTWARE: u32 = 1 << 24;
const SERIAL_ACCESS: u32 = 1 << 15;
// XP5 companion-core DMA setup sets this from a request-descriptor flag; it does not change modeled data movement (session assumption).
const REQUEST_OPTION: u32 = 1 << 8;
const BLOCK: u32 = 1 << 12;
const ACK_WRITE: u32 = 1 << 4;
const NORMAL: u8 = 8;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Routing {
    requests: [u8; 2],
    high: [u32; 2],
    #[serde(skip)]
    enabled: [bool; 2],
}
pub type SharedRouting = Arc<Mutex<Routing>>;
impl Routing {
    pub fn request(&self, lane: usize) -> u8 {
        self.requests[lane]
    }
    pub fn high(&self, side: usize) -> u32 {
        self.high[side]
    }
    pub fn set_request(&mut self, lane: usize, request: u8) -> Result<(), String> {
        if lane >= 2 || request >= 64 {
            return Err("invalid XDMAC request selector".into());
        }
        if self.enabled[lane] && self.requests[lane] != request {
            return Err("XDMAC request change while enabled".into());
        }
        self.requests[lane] = request;
        Ok(())
    }
    pub fn set_high(&mut self, side: usize, value: u32) -> Result<(), String> {
        if side >= 2 || self.enabled.iter().any(|v| *v) && self.high[side] != value {
            return Err("XDMAC address-high change while enabled".into());
        }
        self.high[side] = value;
        Ok(())
    }
}

#[derive(Clone)]
pub struct Connection {
    pub address: u64,
    pub direction: Direction,
    pub port: SharedPort,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Channel {
    count: u32,
    source: u32,
    destination: u32,
    source_shape: u32,
    destination_shape: u32,
    link: u32,
    control: u32,
    status: u8,
    remaining: u64,
    counter_exhausted: bool,
    started: bool,
    irq_published: bool,
}
impl Channel {
    fn enabled(&self) -> bool {
        self.control & ENABLE != 0
    }
    fn width(&self) -> u8 {
        1 << ((self.source_shape >> 16) & 3)
    }
    fn irq(&self) -> bool {
        self.status != 0 && self.control & if self.status == NORMAL { 1 } else { 2 } != 0
    }
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    control: u32,
    channels: [Channel; 2],
}
pub struct Pair {
    base: u64,
    state: State,
    routing: SharedRouting,
    connections: BTreeMap<u8, Connection>,
    interrupts: [Vec<InterruptLine>; 2],
}
impl Pair {
    pub fn new(
        base: u64,
        routing: SharedRouting,
        connections: BTreeMap<u8, Connection>,
        interrupts: [Vec<InterruptLine>; 2],
    ) -> Result<Self, String> {
        if base.checked_add(0x1000).is_none() || connections.keys().any(|&v| v >= 63) {
            return Err("invalid XDMAC pair wiring".into());
        }
        Ok(Self {
            base,
            state: State::default(),
            routing,
            connections,
            interrupts,
        })
    }
    fn connection(&self, lane: usize, c: &Channel) -> Result<Option<&Connection>, String> {
        let routing = self.routing.lock().map_err(|_| "XDMAC routing poisoned")?;
        if routing.high != [0; 2] {
            return Err("XDMAC high addresses are unmodeled".into());
        }
        let factor = (c.control >> 20) & 15;
        if factor == 1 {
            if c.control & (BLOCK | ACK_WRITE) != 0 || routing.requests[lane] != 63 {
                return Err("XDMAC software request mode is unmodeled".into());
            }
            return Ok(None);
        }
        if factor != lane as u32 + 2 || c.control & (BLOCK | ACK_WRITE) != BLOCK | ACK_WRITE {
            return Err("XDMAC requires a per-block request and write acknowledgment".into());
        }
        self.connections
            .get(&routing.requests[lane])
            .map(Some)
            .ok_or_else(|| "XDMAC request has no connected peripheral".into())
    }
    fn validate(&self, lane: usize, c: &Channel) -> Result<(), String> {
        // SA serializes each source access before its destination access.
        // Every beat here already performs that ordered data movement.
        if c.counter_exhausted
            || c.control
                & !(ENABLE | SOFTWARE | 0xf00000 | SERIAL_ACCESS | REQUEST_OPTION | BLOCK | ACK_WRITE | 3)
                != 0
            || c.source_shape & !0x30004 != 0
            || c.destination_shape & !0x30004 != 0
            || (c.source_shape ^ c.destination_shape) & 0x30000 != 0
            || c.link != 0
        {
            return Err("XDMAC control, burst, shape or linked mode is unmodeled".into());
        }
        let width = c.width() as u64;
        if !(c.count as u64 + 1).is_multiple_of(width)
            || !(c.source as u64).is_multiple_of(width)
            || !(c.destination as u64).is_multiple_of(width)
            || c.source_shape & 4 == 0 && c.source as u64 + c.count as u64 + 1 > 1 << 32
            || c.destination_shape & 4 == 0 && c.destination as u64 + c.count as u64 + 1 > 1 << 32
        {
            return Err("XDMAC sparse/unaligned/wrapping transfer is unmodeled".into());
        }
        if let Some(port) = self.connection(lane, c)? {
            let (address, fixed) = match port.direction {
                Direction::Receive => (c.source, c.source_shape & 4 != 0),
                Direction::Transmit => (c.destination, c.destination_shape & 4 != 0),
            };
            if address as u64 != port.address || !fixed {
                return Err("XDMAC peripheral address does not match its request".into());
            }
        }
        Ok(())
    }
    fn ready(&self, lane: usize, c: &Channel) -> Result<bool, String> {
        if !c.enabled() || self.state.control & ENABLE == 0 {
            return Ok(false);
        }
        match self.connection(lane, c)? {
            None => Ok(c.control & SOFTWARE != 0),
            Some(p) => Ok(p
                .port
                .lock()
                .map_err(|_| "XDMAC peripheral port poisoned")?
                .request(p.direction, false)
                .is_some()),
        }
    }
    fn publish(&mut self, cx: &mut Cx<'_>, lane: usize) {
        let c = &mut self.state.channels[lane];
        let high = c.irq();
        if high == c.irq_published {
            return;
        }
        c.irq_published = high;
        if self.interrupts[lane].is_empty() && high {
            cx.unmodeled();
        }
        for &line in &self.interrupts[lane] {
            if let Err(e) = line.drive(cx, high) {
                cx.fail(e);
            }
        }
    }
    fn beat(&mut self, cx: &mut Cx<'_>, lane: usize) -> Result<(), String> {
        let c = &self.state.channels[lane];
        let width = c.width();
        let mut bytes = [0; 8];
        match self.connection(lane, c)? {
            Some(p) if p.direction == Direction::Receive => {
                // Check destination before consuming an irreversible FIFO word.
                cx.ram_read(c.destination as u64, &mut bytes[..width as usize])?;
                let word = p
                    .port
                    .lock()
                    .map_err(|_| "XDMAC port poisoned")?
                    .receive(cx.now(), width)?;
                bytes = word.to_le_bytes();
                cx.ram_write(c.destination as u64, &bytes[..width as usize])?;
            }
            Some(p) => {
                cx.ram_read(c.source as u64, &mut bytes[..width as usize])?;
                p.port.lock().map_err(|_| "XDMAC port poisoned")?.transmit(
                    cx.now(),
                    width,
                    u64::from_le_bytes(bytes),
                )?;
            }
            None => {
                cx.ram_read(c.source as u64, &mut bytes[..width as usize])?;
                cx.ram_write(c.destination as u64, &bytes[..width as usize])?;
            }
        }
        let c = &mut self.state.channels[lane];
        c.remaining -= width as u64;
        c.started = true;
        if c.source_shape & 4 == 0 {
            c.source = c.source.wrapping_add(width as u32);
        }
        if c.destination_shape & 4 == 0 {
            c.destination = c.destination.wrapping_add(width as u32);
        }
        c.count = c.remaining.saturating_sub(1) as u32;
        if c.remaining == 0 {
            c.control &= !(ENABLE | SOFTWARE);
            c.status = NORMAL;
            c.started = false;
            c.counter_exhausted = true;
            self.routing.lock().unwrap().enabled[lane] = false;
        }
        Ok(())
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        (size == 4 && offset <= 0x60 && offset & 3 == 0).then_some(offset)
    }
    fn write_register(&mut self, offset: u64, value: u32) -> Result<(), String> {
        if offset == 0 {
            if value & !ENABLE != 0 {
                return Err("XDMAC global control is unmodeled".into());
            }
            if value & ENABLE == 0 {
                for (lane, c) in self.state.channels.iter_mut().enumerate() {
                    if c.enabled() {
                        c.control &= !(ENABLE | SOFTWARE);
                        c.status = 2;
                        c.started = false;
                        self.routing.lock().unwrap().enabled[lane] = false;
                    }
                }
            }
            self.state.control = value;
            return Ok(());
        }
        let lane = usize::from(offset >= 0x40);
        let register = offset - lane as u64 * 0x30;
        let c = &self.state.channels[lane];
        if register == 0x30 {
            // IS is clear-by-zero, not W1C. Read-only status bits are ignored.
            if value & 15 != 0 {
                return Err("XDMAC interrupt status must be cleared with zero".into());
            }
            self.state.channels[lane].status = 0;
            return Ok(());
        }
        if c.enabled() {
            if register != 0x28 || (value ^ c.control) & !ENABLE != 0 {
                return Err("XDMAC configuration change while enabled".into());
            }
            if value & ENABLE == 0 {
                let c = &mut self.state.channels[lane];
                c.control = value & !SOFTWARE;
                c.status = 2;
                c.started = false;
                self.routing.lock().unwrap().enabled[lane] = false;
            }
            return Ok(());
        }
        if register == 0x28 && value & ENABLE != 0 {
            let mut candidate = c.clone();
            candidate.control = value;
            self.validate(lane, &candidate)?;
            candidate.remaining = candidate.count as u64 + 1;
            candidate.counter_exhausted = false;
            candidate.started = false;
            self.state.channels[lane] = candidate;
            self.routing.lock().unwrap().enabled[lane] = true;
            return Ok(());
        }
        let c = &mut self.state.channels[lane];
        match register {
            0x10 => {
                c.count = value;
                c.counter_exhausted = false;
            }
            0x14 => c.source = value,
            0x18 => c.destination = value,
            0x1c => c.source_shape = value,
            0x20 => c.destination_shape = value,
            0x24 => c.link = value,
            0x28 => c.control = value,
            _ => return Err("XDMAC register is unmodeled".into()),
        }
        Ok(())
    }
}
impl Device for Pair {
    fn name(&self) -> &'static str {
        "xdmac-pair"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if offset == 0 {
            return (self.state.control
                | (u32::from(self.state.channels.iter().any(|c| c.started)) << 16))
                as u64;
        }
        let lane = usize::from(offset >= 0x40);
        let c = &self.state.channels[lane];
        let value = match offset - lane as u64 * 0x30 {
            // The final visible counter encoding is not established by the
            // current references; do not invent saturation or wraparound.
            0x10 if !c.counter_exhausted => c.count,
            0x14 => c.source,
            0x18 => c.destination,
            0x1c => c.source_shape,
            0x20 => c.destination_shape,
            0x24 => c.link,
            0x28 => c.control,
            0x30 => c.status as u32 | (u32::from(c.started) << 16),
            _ => {
                cx.unmodeled();
                return 0;
            }
        };
        value as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u32::MAX as u64 {
            cx.unmodeled();
        } else if let Err(e) = self.write_register(offset, value as u32) {
            cx.unsupported(e);
        }
        for lane in 0..2 {
            self.publish(cx, lane);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        for lane in 0..2 {
            // One hardware beat per service call. A still-ready channel has
            // another immediate deadline; serial clocks supply future ones.
            match self.ready(lane, &self.state.channels[lane]) {
                Ok(true) => {
                    if let Err(e) = self.beat(cx, lane) {
                        // Retire the failed channel without a normal completion.
                        self.state.channels[lane].control &= !(ENABLE | SOFTWARE);
                        self.state.channels[lane].started = false;
                        self.routing.lock().unwrap().enabled[lane] = false;
                        cx.unsupported(e);
                    }
                }
                Ok(false) => {}
                Err(e) => cx.unsupported(e),
            }
            self.publish(cx, lane);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.state
            .channels
            .iter()
            .enumerate()
            .any(|(lane, c)| c.irq() != c.irq_published || self.ready(lane, c).unwrap_or(true))
            .then_some(now)
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.offset(address, size).is_some()
    }
    fn save(&self) -> Value {
        let ports: Vec<_> = self
            .connections
            .iter()
            .map(|(&request, p)| (request, p.address, p.direction))
            .collect();
        json!({"base":self.base,"interrupts":self.interrupts,"state":self.state,
            "routing":*self.routing.lock().unwrap(),"ports":ports})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let interrupts: [Vec<InterruptLine>; 2] =
            serde_json::from_value(value["interrupts"].clone()).map_err(|e| e.to_string())?;
        let state: State =
            serde_json::from_value(value["state"].clone()).map_err(|e| e.to_string())?;
        let mut routing: Routing =
            serde_json::from_value(value["routing"].clone()).map_err(|e| e.to_string())?;
        let ports: Vec<_> = self
            .connections
            .iter()
            .map(|(&request, p)| (request, p.address, p.direction))
            .collect();
        if value["base"].as_u64() != Some(self.base)
            || interrupts != self.interrupts
            || state.control & !ENABLE != 0
            || value["ports"] != json!(ports)
            || routing.requests.iter().any(|&r| r >= 64)
        {
            return Err("XDMAC checkpoint wiring or global state differs".into());
        }
        let candidate = Self::new(
            self.base,
            Arc::new(Mutex::new(routing.clone())),
            self.connections.clone(),
            interrupts,
        )?;
        for (lane, c) in state.channels.iter().enumerate() {
            if !matches!(c.status, 0 | 2 | 8)
                || c.remaining > 1 << 32
                || c.started && (!c.enabled() || state.control & ENABLE == 0)
                || c.counter_exhausted && (c.remaining != 0 || c.enabled())
            {
                return Err("invalid XDMAC checkpoint state".into());
            }
            if c.enabled() {
                candidate.validate(lane, c)?;
                if c.remaining == 0 || c.remaining != c.count as u64 + 1 {
                    return Err("invalid XDMAC checkpoint progress".into());
                }
            }
        }
        routing.enabled = state.channels.each_ref().map(|c| c.enabled());
        *self.routing.lock().unwrap() = routing;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
