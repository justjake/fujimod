//! SPI1/2/3 channels at FFF84040/FFF84080/FFF840C0, stride 0x40. Offsets match
//! `spi`: busy +00, low-half W1C +0C, RX/TX levels +10/+14, RX/TX +18/+1C.
//! A channel owns its endpoint. Only polled configurations are supported;
//! IRQ/DMA configurations refuse. The shared controller owns route channel
//! instances and enforces exclusive use of the physical shifter and RX FIFO.
use fujimod_emulator_devices::parts::pins::{Pins, Selection};
use fujimod_emulator_parts_io::spi::{Endpoint, Shift};
use fujimod_emulator_devices::{Cx, Device};
use serde::Deserialize;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Transport {
    pub control: u32,
    pub configuration: u32,
    pub bits: u8,
    pub receive: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Queue {
    Single,
    Fifo,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub transports: Vec<Transport>,
    pub selection: Vec<Selection>,
    pub transfer_ticks: u64,
    pub queue: Queue,
}
pub struct Wiring {
    pub pins: Box<dyn Pins>,
    pub endpoint: Box<dyn Endpoint>,
}
#[derive(Clone, Copy, serde::Serialize, Deserialize)]
pub(super) struct Pending {
    shift: Shift,
    transport: Transport,
    due: u64,
}

pub struct SerialChannel {
    pub(super) options: Options,
    wiring: Wiring,
    registers: [u32; 8],
    pub(super) pending: Option<Pending>,
    tx: VecDeque<Shift>,
    pub(super) rx: VecDeque<u16>,
}

impl SerialChannel {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if !matches!(options.base, 0xfff84040 | 0xfff84080 | 0xfff840c0)
            || options.transfer_ticks == 0
            || options.transports.is_empty()
            || options.transports.len() > 16
            || options.selection.is_empty()
            || options.selection.len() > 2
        {
            return Err(
                "serial channel requires explicit transport, selection, and clock wiring".into(),
            );
        }
        if options.base == 0xfff84080
            && (options.queue != Queue::Single || options.transports.iter().any(|t| t.receive))
        {
            return Err("SPI2 supports only single transmit-only shifts".into());
        }
        for (i, t) in options.transports.iter().enumerate() {
            if t.control & 1 != 0
                || t.control & 0x80 == 0
                || !matches!(t.bits, 8 | 16)
                || options.transports[..i]
                    .iter()
                    .any(|v| (v.control, v.configuration) == (t.control, t.configuration))
            {
                return Err("invalid or ambiguous serial transport".into());
            }
        }
        for (i, s) in options.selection.iter().enumerate() {
            s.pin.validate()?;
            if options.selection[..i].iter().any(|v| v.pin == s.pin) {
                return Err("duplicate serial selection pad".into());
            }
        }
        Ok(Self {
            options,
            wiring,
            registers: [0; 8],
            pending: None,
            tx: VecDeque::with_capacity(16),
            rx: VecDeque::with_capacity(16),
        })
    }
    pub fn selected(&self) -> Result<bool, String> {
        for s in &self.options.selection {
            if !s.selected(&*self.wiring.pins)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(super) fn status_input(&mut self, register: u8, value: u8) -> Result<(), String> {
        self.wiring.endpoint.status_input(register, value)
    }
    pub(super) fn status_value(&self, register: u8) -> Result<u8, String> {
        self.wiring.endpoint.status_value(register)
    }
    pub(super) fn route(&self) -> Result<Transport, String> {
        if self.registers[2] != 0 || self.registers[3] & 0xffff0000 != 0 {
            return Err("serial IRQ/DMA configuration is unsupported".into());
        }
        self.options
            .transports
            .iter()
            .copied()
            .find(|t| t.control == self.registers[0] & !1 && t.configuration == self.registers[1])
            .ok_or_else(|| "unsupported attached serial transport".into())
    }
    fn start(&mut self, at: u64, shift: Shift, transport: Transport) -> Result<(), String> {
        self.wiring.endpoint.validate(shift)?;
        let due = at
            .checked_add(self.options.transfer_ticks)
            .ok_or("serial deadline overflow")?;
        self.pending = Some(Pending {
            shift,
            transport,
            due,
        });
        self.registers[3] &= !0x300;
        Ok(())
    }
    fn send(&mut self, now: u64, value: u64) -> Result<(), String> {
        let transport = self.route()?;
        if !self.selected()? {
            return Err("serial endpoint chip select is inactive".into());
        }
        if value >= 1 << transport.bits {
            return Err("serial word exceeds the selected width".into());
        }
        let shift = Shift {
            word: value as u16,
            bits: transport.bits,
            receive: transport.receive,
        };
        if let Some(p) = self.pending {
            if self.options.queue == Queue::Single {
                return Err("serial transmit while shift is busy".into());
            }
            if p.transport != transport {
                return Err("serial transport changed with queued shifts".into());
            }
            if self.tx.len() + 1 >= 16 {
                return Err("serial transmit FIFO is full".into());
            }
        }
        if shift.receive
            && self.rx.len() + self.tx.len() + usize::from(self.pending.is_some()) >= 16
        {
            return Err("serial receive FIFO capacity exceeded".into());
        }
        if self.pending.is_some() {
            self.tx.push_back(shift);
        } else {
            self.start(now, shift, transport)?;
        }
        self.registers[7] = value as u32;
        Ok(())
    }
    fn offset(&self, address: u64, size: u8) -> Option<usize> {
        let d = address.checked_sub(self.options.base)?;
        (size == 4 && d < 0x20 && d & 3 == 0).then_some(d as usize / 4)
    }
    fn run(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        while let Some(p) = self.pending {
            if cx.now() < p.due {
                break;
            }
            self.wiring.endpoint.advance(cx, p.due)?;
            if !self.selected()? || self.route()? != p.transport {
                return Err("serial selection/configuration changed before the final edge".into());
            }
            self.wiring.endpoint.validate(p.shift)?;
            // Transport status precedes endpoint effects. The part receives
            // exactly one completed frame, including transmit-only shifts.
            self.pending = None;
            self.registers[3] |= 0x300;
            let reply = self.wiring.endpoint.transfer(cx, p.shift, p.due)?;
            if p.shift.receive {
                if u32::from(reply) >= 1u32 << p.shift.bits {
                    return Err("serial endpoint reply exceeds its frame width".into());
                }
                self.rx.push_back(reply);
            }
            if let Some(shift) = self.tx.front().copied() {
                self.start(p.due, shift, p.transport)?;
                self.tx.pop_front();
            }
        }
        let now = cx.now();
        self.wiring.endpoint.advance(cx, now)
    }
}
impl Device for SerialChannel {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(self.registers, self.pending, &self.tx, &self.rx),
        )?
        .with_children(vec![self.wiring.endpoint.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let _: ([u32; 8], Option<Pending>, VecDeque<Shift>, VecDeque<u16>) =
            saved.decode(self.name())?;
        if saved.children().len() != 1 {
            return Err("serial snapshot endpoint missing".into());
        }
        self.wiring
            .endpoint
            .validate_checkpoint(&saved.children()[0])
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (registers, pending, tx, rx) = saved.decode(self.name())?;
        self.wiring
            .endpoint
            .restore_checkpoint(&saved.children()[0])?;
        self.registers = registers;
        self.pending = pending;
        self.tx = tx;
        self.rx = rx;
        Ok(())
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"registers":self.registers,"pending":self.pending.map(|p|
            serde_json::json!({"shift":p.shift,"control":p.transport.control,"configuration":p.transport.configuration,
                "bits":p.transport.bits,"receive":p.transport.receive,"due":p.due})),
            "tx":self.tx,"rx":self.rx,"part":self.wiring.endpoint.fingerprint()?}),
        )
    }
    fn name(&self) -> &'static str {
        "spi-channel"
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"registers":self.registers,"pending":self.pending.map(|p|serde_json::json!({"word":p.shift.word,"bits":p.shift.bits,"receive":p.shift.receive,"due":p.due})),
            "queued_words":self.tx.len(),"rx":self.rx,"part":self.wiring.endpoint.state()})
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        if cx.refusal().is_some() {
            return 0;
        }
        let Some(o) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        match o {
            0 => u64::from(self.registers[0] & !1 | u32::from(self.pending.is_some())),
            4 => u64::from(self.registers[4] & !31 | self.rx.len() as u32),
            5 => u64::from(self.registers[5] & !31 | self.tx.len() as u32),
            6 => match self.rx.pop_front() {
                Some(v) => u64::from(v),
                None => {
                    cx.fail("serial receive data has no completed modeled exchange");
                    0
                }
            },
            _ => u64::from(self.registers[o]),
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        let Some(o) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u32::MAX as u64 {
            cx.fail("serial registers require unsigned words");
            return;
        }
        match o {
            6 => cx.fail("serial receive data is read-only"),
            7 => {
                if let Err(e) = self.send(cx.now(), value) {
                    cx.fail(e);
                }
            }
            3 => self.registers[3] &= !(value as u32 & 0xffff),
            _ => {
                self.registers[o] = value as u32;
                if o == 2 && value & 0x10000000 != 0 {
                    self.pending = None;
                    self.tx.clear();
                    self.rx.clear();
                    self.registers[3] &= !0x300;
                }
            }
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        [
            self.pending.map(|p| p.due),
            self.wiring.endpoint.next_deadline(now),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.run(cx) {
            cx.fail(e);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
}
