//! Bounded EXR II byte-wide synchronous serial FIFO and DMA request wires.
//!
//! The model admits the C0/8/09090000 transport in mode 3. FIFO capacity
//! and frame duration are explicit model laws.
//! The attached wire supplies received bits. No lens protocol is built in.
use fujimod_emulator_devices::dma::{Direction, Port, Request, SharedPort};
use fujimod_emulator_parts_io::serial::{Frame, Wire};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const DMA_ENABLE: u32 = 0x09090000;
const FIFO_RESET: u32 = 0x10000000;
const CAPACITY: usize = 16;

#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    at: u64,
    token: u64,
    word: u8,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct State {
    control: u32,
    format: u32,
    enable: u32,
    status: u32,
    thresholds: u32,
    tx: VecDeque<u8>,
    rx: VecDeque<u8>,
    pending: Option<Pending>,
    transmit_request: Request,
    receive_request: Request,
    shifted: u64,
}
struct Engine {
    frame_ticks: u64,
    state: State,
    wire: Box<dyn Wire>,
}
impl Engine {
    fn validate_state(state: &State) -> Result<(), String> {
        if state.control & !0xc0 != 0
            || !matches!(state.format, 0 | 8)
            || state.enable & !DMA_ENABLE != 0
            || state.status & !0x300 != 0
            || !matches!(state.thresholds, 0 | 0x400)
            || state.tx.len() > CAPACITY
            || state.rx.len() > CAPACITY
        {
            return Err("invalid EXR II serial checkpoint configuration".into());
        }
        let configured = state.control & !0x80 == 0x40
            && state.format == 8
            && state.enable == DMA_ENABLE
            && state.thresholds == 0x400;
        if (state.pending.is_some()
            && (!configured || state.control & 0x80 == 0 || state.rx.len() == CAPACITY))
            || state.transmit_request.since().is_some() != (configured && state.tx.len() < 4)
            || state.receive_request.since().is_some() != (configured && !state.rx.is_empty())
        {
            return Err("invalid EXR II serial checkpoint FIFO/request state".into());
        }
        Ok(())
    }

    fn configured(&self) -> bool {
        self.state.control & !0x80 == 0x40
            && self.state.format == 8
            && self.state.enable == DMA_ENABLE
            && self.state.thresholds == 0x400
    }
    fn requests(&mut self, now: u64) {
        let configured = self.configured();
        // Only the observed one-byte RX and bounded TX FIFO request law is
        // selected. Other threshold configurations refuse at their write.
        self.state
            .transmit_request
            .update(now, configured && self.state.tx.len() < 4);
        self.state
            .receive_request
            .update(now, configured && !self.state.rx.is_empty());
    }
    fn start(&mut self, now: u64) -> Result<(), String> {
        if self.state.pending.is_some() || self.state.control & 0x80 == 0 {
            return Ok(());
        }
        let Some(&word) = self.state.tx.front() else {
            return Ok(());
        };
        if !self.configured() {
            return Err("EXR II byte serial transport is not configured".into());
        }
        if self.state.rx.len() == CAPACITY {
            return Err("EXR II byte serial receive FIFO overflow".into());
        }
        let at = now
            .checked_add(self.frame_ticks)
            .ok_or("EXR II serial clock overflow")?;
        let token = self.wire.begin(Frame {
            bits: 8,
            mode: 3,
            word: word as u16,
        })?;
        self.state.tx.pop_front();
        self.state.pending = Some(Pending { at, token, word });
        Ok(())
    }
    fn transmit(&mut self, now: u64, value: u8) -> Result<(), String> {
        if !self.configured() {
            return Err("EXR II byte serial transport is not configured".into());
        }
        if self.state.tx.len() == CAPACITY {
            return Err("EXR II byte serial transmit FIFO overflow".into());
        }
        self.state.tx.push_back(value);
        self.start(now)?;
        self.requests(now);
        Ok(())
    }
    fn receive(&mut self, now: u64) -> Result<u8, String> {
        let value = self
            .state
            .rx
            .pop_front()
            .ok_or("EXR II byte serial receive FIFO underflow")?;
        self.requests(now);
        Ok(value)
    }
    fn advance(&mut self, now: u64) -> Result<(), String> {
        while self.state.pending.as_ref().is_some_and(|p| p.at <= now) {
            let p = self.state.pending.as_ref().unwrap();
            let (at, token, word) = (p.at, p.token, p.word);
            let value = self.wire.finish(
                token,
                Frame {
                    bits: 8,
                    mode: 3,
                    word: word as u16,
                },
                at,
            )?;
            if value > 255 {
                return Err("EXR II byte serial wire returned a wider frame".into());
            }
            self.state.pending = None;
            self.state.rx.push_back(value as u8);
            self.state.status |= 0x300;
            self.state.shifted += 1;
            // Preserve each serial edge when the host services several due
            // frames late; scheduler call frequency cannot change the clock.
            self.start(at)?;
            self.requests(at);
        }
        Ok(())
    }
    fn write(&mut self, offset: u64, value: u32, now: u64) -> Result<(), String> {
        match offset {
            0 => {
                if value & !0xc1 != 0
                    || self.state.pending.is_some() && value & !1 != self.state.control
                {
                    return Err(
                        "EXR II byte serial control or active reconfiguration is unmodeled".into(),
                    );
                }
                self.state.control = value & !1;
                self.start(now)?;
            }
            4 => {
                if !matches!(value, 0 | 8)
                    || self.state.pending.is_some() && value != self.state.format
                {
                    return Err(
                        "EXR II byte serial format or active clock change is unmodeled".into(),
                    );
                }
                self.state.format = value;
            }
            8 => {
                if value & !(DMA_ENABLE | FIFO_RESET) != 0 {
                    return Err("EXR II byte serial enable bits are unmodeled".into());
                }
                if value & FIFO_RESET != 0 {
                    self.state.pending = None;
                    self.state.tx.clear();
                    self.state.rx.clear();
                    self.state.status = 0;
                } else if self.state.pending.is_some() && value != self.state.enable {
                    return Err("EXR II byte serial request enables changed during a frame".into());
                }
                self.state.enable = value & DMA_ENABLE;
            }
            0xc => self.state.status &= !(value & 0xffff),
            0x10 | 0x14 if value & !31 == 0 && value & 31 == 1 => {}
            0x10 | 0x14 => return Err("EXR II byte serial FIFO configuration is unmodeled".into()),
            0x18 => return Err("EXR II byte serial receive port is read-only".into()),
            0x1c => {
                if value > 255 {
                    return Err("EXR II byte serial transmit value is wider than a byte".into());
                }
                self.transmit(now, value as u8)?;
            }
            0x20 => {
                if !matches!(value, 0 | 0x400)
                    || self.state.pending.is_some() && value != self.state.thresholds
                {
                    return Err("EXR II byte serial DMA thresholds are unmodeled".into());
                }
                self.state.thresholds = value;
            }
            _ => return Err("EXR II byte serial register is unmodeled".into()),
        }
        self.requests(now);
        Ok(())
    }
}
struct DmaPort(Arc<Mutex<Engine>>);
impl Port for DmaPort {
    fn request(&self, direction: Direction, burst: bool) -> Option<u64> {
        if burst {
            return None;
        }
        let e = self.0.lock().ok()?;
        match direction {
            Direction::Transmit => e.state.transmit_request.since(),
            Direction::Receive => e.state.receive_request.since(),
        }
    }
    fn transmit(&mut self, now: u64, bytes: u8, value: u64) -> Result<(), String> {
        if bytes != 1 || value > 255 {
            return Err("EXR II serial DMA transmit width/value is unmodeled".into());
        }
        let mut e = self.0.lock().map_err(|_| "EXR II serial state poisoned")?;
        if !e.state.transmit_request.since().is_some_and(|at| at <= now) {
            return Err("EXR II serial transmit without a DMA request".into());
        }
        e.transmit(now, value as u8)
    }
    fn receive(&mut self, now: u64, bytes: u8) -> Result<u64, String> {
        if bytes != 1 {
            return Err("EXR II serial DMA receive width is unmodeled".into());
        }
        let mut e = self.0.lock().map_err(|_| "EXR II serial state poisoned")?;
        if !e.state.receive_request.since().is_some_and(|at| at <= now) {
            return Err("EXR II serial receive without a DMA request".into());
        }
        Ok(e.receive(now)? as u64)
    }
}
pub struct ByteFifo {
    base: u64,
    engine: Arc<Mutex<Engine>>,
}
impl ByteFifo {
    pub fn new(base: u64, frame_ticks: u64, wire: Box<dyn Wire>) -> Result<Self, String> {
        if base & 0x3f != 0 || base.checked_add(0x24).is_none() || frame_ticks == 0 {
            return Err("invalid EXR II byte serial wiring or declared clock".into());
        }
        Ok(Self {
            base,
            engine: Arc::new(Mutex::new(Engine {
                frame_ticks,
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
        (size == 4 && offset <= 0x20 && offset & 3 == 0).then_some(offset)
    }
}
impl Device for ByteFifo {
    fn name(&self) -> &'static str {
        "exr2-byte-serial"
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        let e = self.engine.lock().unwrap();
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(self.base, e.frame_ticks, &e.state),
        )?
        .with_children(vec![e.wire.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (base, ticks, state): (u64, u64, State) = saved.decode(self.name())?;
        let e = self.engine.lock().unwrap();
        if base != self.base
            || ticks != e.frame_ticks
            || saved.children().len() != 1
            || !saved.blobs().is_empty()
        {
            return Err("byte serial checkpoint wiring differs".into());
        }
        Engine::validate_state(&state)?;
        e.wire.validate_checkpoint(&saved.children()[0])?;
        if let Some(p) = &state.pending {
            e.wire.validate_pending(
                &saved.children()[0],
                p.token,
                Frame {
                    bits: 8,
                    mode: 3,
                    word: p.word as u16,
                },
            )?;
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, _, state): (u64, u64, State) = saved.decode(self.name())?;
        let mut e = self.engine.lock().unwrap();
        e.wire.restore_checkpoint(&saved.children()[0])?;
        e.state = state;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        let mut e = self.engine.lock().unwrap();
        (match offset {
            0 => e.state.control | u32::from(e.state.pending.is_some()),
            4 => e.state.format,
            8 => e.state.enable,
            0xc => e.state.status,
            0x10 => e.state.rx.len() as u32,
            0x14 => e.state.tx.len() as u32,
            0x18 => match e.receive(cx.now()) {
                Ok(v) => v as u32,
                Err(error) => {
                    cx.fail(error);
                    0
                }
            },
            0x1c => {
                cx.unmodeled();
                0
            }
            0x20 => e.state.thresholds,
            _ => unreachable!(),
        }) as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if let Err(error) = self
            .engine
            .lock()
            .unwrap()
            .write(offset, value as u32, cx.now())
        {
            cx.fail(error);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(error) = self.engine.lock().unwrap().advance(cx.now()) {
            cx.fail(error);
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.engine
            .lock()
            .unwrap()
            .state
            .pending
            .as_ref()
            .map(|p| p.at)
    }
    fn save(&self) -> Value {
        let e = self.engine.lock().unwrap();
        json!({"base":self.base,"frame_ticks":e.frame_ticks,"state":e.state,"wire":e.wire.save()})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State =
            serde_json::from_value(value["state"].clone()).map_err(|e| e.to_string())?;
        let mut e = self.engine.lock().unwrap();
        if value["base"] != self.base || value["frame_ticks"] != e.frame_ticks {
            return Err("invalid EXR II serial checkpoint configuration".into());
        }
        Engine::validate_state(&state)?;
        e.wire.restore(&value["wire"])?;
        e.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
