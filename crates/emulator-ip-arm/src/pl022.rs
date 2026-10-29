//! Bounded Motorola-master PL022 transport (ARM DDI 0194H).
//! FIFO/request state and serial clock edges are independent of executing code.
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::dma::{Direction, Port, Request, SharedPort};
use fujimod_emulator_parts_io::serial::{Frame, Wire};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const DEPTH: usize = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    pub source_hz: u64,
    pub ticks_per_second: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    at: u64,
    frame: Frame,
    token: Option<u64>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct State {
    control: u16,
    enable: u8,
    divider: u8,
    mask: u8,
    dma: u8,
    tx: VecDeque<u16>,
    rx: VecDeque<u16>,
    pending: Option<Pending>,
    clock_chain: Option<(u64, u64)>,
    idle_since: Option<u64>,
    timeout: bool,
    overrun: bool,
    requests: [Request; 4],
    irq_published: bool,
}
struct Engine {
    clock: Clock,
    state: State,
    wire: Box<dyn Wire>,
}
impl Engine {
    fn frame(&self, word: u16) -> Result<Frame, String> {
        let bits = (self.state.control & 15) as u8 + 1;
        if !(4..=16).contains(&bits) || self.state.control & 0x30 != 0 {
            return Err("PL022 supports Motorola frames of 4..16 bits".into());
        }
        Ok(Frame {
            bits,
            mode: ((self.state.control >> 6) & 3) as u8,
            word: word & (u16::MAX >> (16 - bits)),
        })
    }
    fn cycles(&self, bits: u64) -> Result<u64, String> {
        if self.state.divider < 2 {
            return Err("PL022 requires a valid clock divider".into());
        }
        bits.checked_mul(self.state.divider as u64)
            .and_then(|v| v.checked_mul((self.state.control as u64 >> 8) + 1))
            .ok_or_else(|| "PL022 clock cycle overflow".into())
    }
    fn at(&self, origin: u64, cycles: u64) -> Result<u64, String> {
        let ticks = (cycles as u128 * self.clock.ticks_per_second as u128)
            .div_ceil(self.clock.source_hz as u128);
        origin
            .checked_add(u64::try_from(ticks).map_err(|_| "PL022 clock range overflow")?)
            .ok_or_else(|| "PL022 deadline overflow".into())
    }
    fn start(&mut self, now: u64) -> Result<(), String> {
        if self.state.enable & 2 == 0 || self.state.pending.is_some() {
            return Ok(());
        }
        let Some(&word) = self.state.tx.front() else {
            self.state.clock_chain = None;
            return Ok(());
        };
        let frame = self.frame(word)?;
        let (origin, previous) = self.state.clock_chain.unwrap_or((now, 0));
        let cycles = previous
            .checked_add(self.cycles(frame.bits as u64)?)
            .ok_or("PL022 clock chain overflow")?;
        let at = self.at(origin, cycles)?;
        let token = if self.state.enable & 1 == 0 {
            Some(self.wire.begin(frame)?)
        } else {
            None
        };
        self.state.tx.pop_front();
        self.state.pending = Some(Pending { at, frame, token });
        self.state.clock_chain = Some((origin, cycles));
        self.state.idle_since = None;
        self.state.timeout = false;
        self.requests(now);
        Ok(())
    }
    fn request_levels(state: &State) -> [bool; 4] {
        [
            state.dma & 1 != 0 && !state.rx.is_empty(),
            state.dma & 1 != 0 && state.rx.len() >= DEPTH / 2,
            state.dma & 2 != 0 && state.tx.len() < DEPTH,
            state.dma & 2 != 0 && state.tx.len() <= DEPTH / 2,
        ]
    }
    fn requests(&mut self, now: u64) {
        for (i, high) in Self::request_levels(&self.state).into_iter().enumerate() {
            self.state.requests[i].update(now, high);
        }
    }
    fn timeout_at(&self) -> Option<u64> {
        if self.state.timeout || self.state.rx.is_empty() || self.state.enable & 2 == 0 {
            return None;
        }
        let origin = self.state.idle_since?;
        self.at(origin, self.cycles(32).ok()?).ok()
    }
    fn advance(&mut self, now: u64) -> Result<(), String> {
        while self.state.pending.as_ref().is_some_and(|p| p.at <= now) {
            let pending = self.state.pending.take().unwrap();
            let word = match pending.token {
                Some(token) => match self.wire.finish(token, pending.frame, pending.at) {
                    Ok(word) => word,
                    Err(error) => {
                        self.state.clock_chain = None;
                        return Err(error);
                    }
                },
                None => pending.frame.word,
            } & (u16::MAX >> (16 - pending.frame.bits));
            if self.state.rx.len() == DEPTH {
                self.state.overrun = true;
            } else {
                self.state.rx.push_back(word);
            }
            self.state.timeout = false;
            self.state.idle_since = Some(pending.at);
            self.requests(pending.at);
            self.start(pending.at)?;
        }
        if self.timeout_at().is_some_and(|at| at <= now) {
            self.state.timeout = true;
        }
        Ok(())
    }
    fn ris(&self) -> u8 {
        u8::from(self.state.overrun)
            | (u8::from(self.state.timeout) << 1)
            | (u8::from(self.state.rx.len() >= DEPTH / 2) << 2)
            | (u8::from(self.state.tx.len() <= DEPTH / 2) << 3)
    }
    fn irq(&self) -> bool {
        self.ris() & self.state.mask != 0
    }
    fn next(&self, now: u64) -> Option<u64> {
        if self.irq() != self.state.irq_published {
            return Some(now);
        }
        self.state
            .pending
            .as_ref()
            .map(|p| p.at)
            .into_iter()
            .chain(self.timeout_at())
            .min()
            .map(|at| at.max(now))
    }
    fn transmit(&mut self, now: u64, value: u16) -> Result<(), String> {
        if self.state.tx.len() == DEPTH {
            return Err("PL022 transmit FIFO is full".into());
        }
        let word = self.frame(value)?.word;
        self.state.tx.push_back(word);
        if let Err(e) = self.start(now) {
            self.state.tx.pop_back();
            return Err(e);
        }
        self.requests(now);
        Ok(())
    }
    fn receive(&mut self, now: u64) -> Result<u16, String> {
        let value = self
            .state
            .rx
            .pop_front()
            .ok_or("PL022 receive FIFO is empty")?;
        if self.state.rx.is_empty() {
            self.state.timeout = false;
            self.state.idle_since = None;
        }
        self.requests(now);
        Ok(value)
    }
    fn busy(&self) -> bool {
        self.state.pending.is_some() || !self.state.tx.is_empty()
    }
    fn read(&mut self, offset: u64, now: u64) -> Result<u64, String> {
        Ok(match offset {
            0 => self.state.control as u64,
            4 => self.state.enable as u64,
            8 => self.receive(now)? as u64,
            0xc => {
                u64::from(self.state.tx.is_empty())
                    | (u64::from(self.state.tx.len() < DEPTH) << 1)
                    | (u64::from(!self.state.rx.is_empty()) << 2)
                    | (u64::from(self.state.rx.len() == DEPTH) << 3)
                    | (u64::from(self.busy()) << 4)
            }
            0x10 => self.state.divider as u64,
            0x14 => self.state.mask as u64,
            0x18 => self.ris() as u64,
            0x1c => (self.ris() & self.state.mask) as u64,
            0x24 => self.state.dma as u64,
            _ => return Err("PL022 register is unmodeled".into()),
        })
    }
    fn write(&mut self, offset: u64, value: u64, now: u64) -> Result<(), String> {
        match offset {
            0 if value <= u16::MAX as u64 && value & 0x30 == 0 => {
                if self.state.enable & 2 != 0
                    || !self.state.rx.is_empty()
                    || !self.state.tx.is_empty()
                {
                    return Err("PL022 CR0 change requires disabled SSP and empty FIFOs".into());
                }
                self.state.control = value as u16;
            }
            4 if value & !0xb == 0 => {
                if self.state.pending.is_some() && value != self.state.enable as u64 {
                    return Err("PL022 control change during a frame is unmodeled".into());
                }
                if value & 2 == 0 && !self.state.rx.is_empty() {
                    return Err("PL022 disable with unread FIFO data is unmodeled".into());
                }
                let old = self.state.enable;
                self.state.enable = value as u8;
                if let Err(e) = self.start(now) {
                    self.state.enable = old;
                    return Err(e);
                }
            }
            8 => self.transmit(now, value as u16)?,
            0x10 if value <= 255 => {
                if self.state.enable & 2 != 0
                    || !self.state.rx.is_empty()
                    || !self.state.tx.is_empty()
                {
                    return Err("PL022 divider change requires disabled SSP and empty FIFOs".into());
                }
                self.state.divider = value as u8 & !1;
            }
            0x14 => self.state.mask = (value & 15) as u8,
            0x20 if value & !3 == 0 => {
                if value & 1 != 0 {
                    self.state.overrun = false;
                }
                if value & 2 != 0 {
                    self.state.timeout = false;
                    if self.state.pending.is_none() && !self.state.rx.is_empty() {
                        self.state.idle_since = Some(now);
                    }
                }
            }
            0x24 if value & !3 == 0 => self.state.dma = value as u8,
            _ => return Err("PL022 register value or operating mode is unmodeled".into()),
        }
        self.requests(now);
        Ok(())
    }
}

struct DmaPort(Arc<Mutex<Engine>>);
impl Port for DmaPort {
    fn request(&self, direction: Direction, burst: bool) -> Option<u64> {
        let e = self.0.lock().unwrap();
        let i = usize::from(direction == Direction::Transmit) * 2 + usize::from(burst);
        e.state.requests[i].since()
    }
    fn receive(&mut self, now: u64, bytes: u8) -> Result<u64, String> {
        if !matches!(bytes, 1 | 2) {
            return Err("PL022 DMA width is unmodeled".into());
        }
        let mut e = self.0.lock().map_err(|_| "PL022 state poisoned")?;
        let word = e.receive(now)? as u64;
        Ok(word & (u64::MAX >> (64 - bytes as u32 * 8)))
    }
    fn transmit(&mut self, now: u64, bytes: u8, value: u64) -> Result<(), String> {
        if !matches!(bytes, 1 | 2) || value >> (bytes as u32 * 8) != 0 {
            return Err("PL022 DMA width/value is unmodeled".into());
        }
        self.0
            .lock()
            .map_err(|_| "PL022 state poisoned")?
            .transmit(now, value as u16)
    }
}
pub struct Controller {
    base: u64,
    engine: Arc<Mutex<Engine>>,
    interrupts: Vec<InterruptLine>,
}
impl Controller {
    pub fn new(
        base: u64,
        clock: Clock,
        interrupts: Vec<InterruptLine>,
        wire: Box<dyn Wire>,
    ) -> Result<Self, String> {
        if clock.source_hz == 0 || clock.ticks_per_second == 0 || base.checked_add(0x1000).is_none()
        {
            return Err("invalid PL022 clock or aperture".into());
        }
        Ok(Self {
            base,
            interrupts,
            engine: Arc::new(Mutex::new(Engine {
                clock,
                state: State::default(),
                wire,
            })),
        })
    }
    pub fn port(&self) -> SharedPort {
        Arc::new(Mutex::new(DmaPort(self.engine.clone())))
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        (matches!(size, 2 | 4) && offset <= 0x24 && offset & 3 == 0).then_some(offset)
    }
    fn publish(&self, e: &mut Engine, cx: &mut Cx<'_>) {
        let high = e.irq();
        if high == e.state.irq_published {
            return;
        }
        e.state.irq_published = high;
        if self.interrupts.is_empty() && high {
            cx.unmodeled();
        }
        for &line in &self.interrupts {
            if let Err(error) = line.drive(cx, high) {
                cx.fail(error);
            }
        }
    }
}
impl Device for Controller {
    fn name(&self) -> &'static str {
        "pl022"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        let mut e = self.engine.lock().unwrap();
        let result = e.advance(cx.now()).and_then(|_| e.read(offset, cx.now()));
        self.publish(&mut e, cx);
        match result {
            Ok(value) => value,
            Err(_) => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        let mut e = self.engine.lock().unwrap();
        let result = e
            .advance(cx.now())
            .and_then(|_| e.write(offset, value, cx.now()));
        self.publish(&mut e, cx);
        if result.is_err() {
            cx.unmodeled();
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut e = self.engine.lock().unwrap();
        if e.advance(cx.now()).is_err() {
            cx.unmodeled();
        }
        self.publish(&mut e, cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.engine.lock().unwrap().next(now)
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.offset(address, size)
            .is_some_and(|o| matches!(o, 0 | 4 | 0xc | 0x10 | 0x14 | 0x18 | 0x1c | 0x24))
    }
    fn save(&self) -> Value {
        let e = self.engine.lock().unwrap();
        json!({"clock":e.clock,"base":self.base,"interrupts":self.interrupts,"state":e.state,"wire":e.wire.save()})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let clock: Clock =
            serde_json::from_value(value["clock"].clone()).map_err(|e| e.to_string())?;
        let interrupts: Vec<InterruptLine> =
            serde_json::from_value(value["interrupts"].clone()).map_err(|e| e.to_string())?;
        let state: State =
            serde_json::from_value(value["state"].clone()).map_err(|e| e.to_string())?;
        let mut e = self.engine.lock().map_err(|_| "PL022 state poisoned")?;
        if clock != e.clock
            || interrupts != self.interrupts
            || value["base"].as_u64() != Some(self.base)
            || state.tx.len() > DEPTH
            || state.rx.len() > DEPTH
            || state.control & 0x30 != 0
            || state.enable & !0xb != 0
            || state.divider & 1 != 0
            || state.mask & !15 != 0
            || state.dma & !3 != 0
            || state.pending.as_ref().is_some_and(|p| {
                !(4..=16).contains(&p.frame.bits) || p.frame.mode > 3 || state.enable & 2 == 0
            })
            || state.pending.is_some() && state.divider < 2
            || state.pending.is_some() != state.clock_chain.is_some()
            || !state.rx.is_empty() && state.enable & 2 == 0
            || state.idle_since.is_some() && (state.pending.is_some() || state.rx.is_empty())
            || state.timeout && (state.rx.is_empty() || state.pending.is_some())
            || state
                .requests
                .iter()
                .zip(Engine::request_levels(&state))
                .any(|(r, level)| r.since().is_some() != level)
        {
            return Err("invalid PL022 checkpoint state or wiring".into());
        }
        if let Some(pending) = &state.pending {
            let (origin, cycles) = state.clock_chain.unwrap();
            if cycles == 0
                || e.at(origin, cycles)? != pending.at
                || pending.frame.bits != (state.control & 15) as u8 + 1
                || pending.frame.mode != ((state.control >> 6) & 3) as u8
                || pending.frame.word > u16::MAX >> (16 - pending.frame.bits)
                || pending.token.is_none() != (state.enable & 1 != 0)
            {
                return Err("invalid PL022 pending frame".into());
            }
        }
        e.wire.restore(&value["wire"])?;
        e.state = state;
        Ok(())
    }
}
#[cfg(test)]
mod tests;
