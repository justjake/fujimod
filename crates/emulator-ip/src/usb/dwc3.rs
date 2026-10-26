//! Bounded DesignWare USB3 device controller, with explicit external bus inputs.
//!
//! Register/TRB/event encodings: Linux v6.12 drivers/usb/dwc3/core.h.
//! https://github.com/torvalds/linux/blob/v6.12/drivers/usb/dwc3/core.h
//! Only registers, hardware TRBs, DMA buffers, bus transactions and guest time
//! drive this model. PHY transport and firmware protocols are separate devices.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const DCTL: u32 = 0xc704;
const DSTS: u32 = 0xc70c;
const ACTIVE: u32 = 1 << 10;
const RESET: u32 = 1 << 30;
const RUN: u32 = 1 << 31;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub base: u64,
    pub irq: u32,
    pub command_ticks: u64,
    pub transaction_ticks: u64,
    /// DSTS negotiated speed: 0 high speed, 4 SuperSpeed.
    pub speed: u8,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Reply {
    Data(Vec<u8>),
    Ack,
    Nak,
    Stall,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    configured: bool,
    resource: bool,
    stalled: bool,
    max_packet: u16,
    events: u8,
    trb: Option<u64>,
    cursor: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Operation {
    Reset,
    Run(bool),
    Link(bool),
    BusReset,
    GlobalCommand {
        value: u32,
        parameter: u32,
    },
    Command {
        ep: usize,
        value: u32,
        parameters: [u32; 3],
    },
    Transaction {
        id: u64,
        ep: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    at: u64,
    order: u64,
    operation: Operation,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: BTreeMap<u32, u32>,
    endpoints: [Endpoint; 32],
    running: bool,
    connected: bool,
    control: Control,
    periodic_parameters: u32,
    ring_cursor: u32,
    next_order: u64,
    pending: Vec<Pending>,
    replies: BTreeMap<u64, Reply>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    remaining: u16,
    data_in: bool,
    status: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    config: Config,
    state: State,
}
pub struct Dwc3 {
    config: Config,
    state: State,
}
impl Dwc3 {
    pub fn new(config: Config) -> Result<Self, String> {
        if config.base & 4095 != 0
            || config.base.checked_add(0xcc00).is_none()
            || config.irq >= 1024
            || config.command_ticks == 0
            || config.transaction_ticks == 0
            || ![0, 4].contains(&config.speed)
        {
            return Err("Invalid DWC3 wiring, clock or negotiated speed".into());
        }
        Ok(Self {
            config,
            state: State {
                registers: BTreeMap::new(),
                endpoints: std::array::from_fn(|_| Endpoint::default()),
                running: false,
                connected: false,
                control: Control::default(),
                periodic_parameters: 0,
                ring_cursor: 0,
                next_order: 1,
                pending: vec![],
                replies: BTreeMap::new(),
            },
        })
    }
    fn reg(&self, offset: u32) -> u32 {
        self.state.registers.get(&offset).copied().unwrap_or(0)
    }
    fn queue(&mut self, now: u64, ticks: u64, operation: Operation) -> Result<u64, String> {
        if self.state.pending.len() + self.state.replies.len() >= 128 {
            return Err("DWC3 external transaction/command bound exceeded".into());
        }
        let at = now.checked_add(ticks).ok_or("DWC3 deadline overflow")?;
        let order = self.state.next_order;
        self.state.next_order = order
            .checked_add(1)
            .ok_or("DWC3 transaction counter overflow")?;
        self.state.pending.push(Pending {
            at,
            order,
            operation,
        });
        self.state.pending.sort_by_key(|p| (p.at, p.order));
        Ok(order)
    }
    /// An explicitly negotiated physical link input; not a synthetic USB reply.
    pub fn link(&mut self, now: u64, connected: bool) -> Result<(), String> {
        self.queue(now, self.config.transaction_ticks, Operation::Link(connected))
            .map(|_| ())
    }
    pub fn bus_reset(&mut self, now: u64) -> Result<(), String> {
        self.queue(now, self.config.transaction_ticks, Operation::BusReset)
            .map(|_| ())
    }
    /// A reliable USB transaction including its data handshake. IN consumes
    /// guest DMA and OUT supplies host bus bytes. CRC failure/retry is outside
    /// this bounded bus interface; no protocol payload is synthesized.
    pub fn transaction(
        &mut self,
        now: u64,
        ep: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> Result<u64, String> {
        if ep >= 32
            || (ep & 1 == 0) != out.is_some()
            || out.as_ref().is_some_and(|v| v.len() > 1024)
            || setup && (ep != 0 || out.as_ref().is_none_or(|v| v.len() != 8))
        {
            return Err("Invalid DWC3 physical endpoint or host packet".into());
        }
        // The serial allocated to the bus transaction is its observational key.
        let id = self.state.next_order;
        self.queue(
            now,
            self.config.transaction_ticks,
            Operation::Transaction { id, ep, setup, out },
        )
    }
    pub fn reply(&self, id: u64) -> Option<&Reply> {
        self.state.replies.get(&id)
    }
    /// Release a retained host result, separate from observational reads.
    /// The USB handshake was included in the completed bus transaction.
    pub fn consume_reply(&mut self, id: u64) -> Result<(), String> {
        self.state
            .replies
            .remove(&id)
            .map(|_| ())
            .ok_or("Unknown DWC3 host transaction".into())
    }
    fn event_word(&mut self, cx: &mut Cx<'_>, word: u32) -> Result<(), String> {
        let size = self.reg(0xc408) & 0xffff;
        let count = self.reg(0xc40c);
        if size < 4 || count > size - 4 {
            return Err("DWC3 event ring absent or full".into());
        }
        let address = u64::from(self.reg(0xc400)) | u64::from(self.reg(0xc404)) << 32;
        cx.ram_write(
            address
                .checked_add(u64::from(self.state.ring_cursor))
                .ok_or("DWC3 ring overflow")?,
            &word.to_le_bytes(),
        )?;
        self.state.ring_cursor = (self.state.ring_cursor + 4) % size;
        self.state.registers.insert(0xc40c, count + 4);
        Ok(())
    }
    fn device_event(&mut self, cx: &mut Cx<'_>, kind: u32) -> Result<(), String> {
        if self.reg(0xc708) & (1 << kind) != 0 {
            self.event_word(cx, 1 | kind << 8)?;
        }
        Ok(())
    }
    fn command(
        &mut self,
        cx: &mut Cx<'_>,
        ep: usize,
        value: u32,
        p: [u32; 3],
    ) -> Result<(), String> {
        let command = value & 15;
        let endpoint = &mut self.state.endpoints[ep];
        let mut resource = 0;
        match command {
            1 => {
                let max_packet = (p[2] >> 3) & 0x7ff;
                let kind = (p[2] >> 1) & 3;
                if max_packet == 0 || max_packet > 1024 || kind == 1 || (kind == 0) != (ep < 2) {
                    return Err("Unsupported DWC3 endpoint packet size or isochronous type".into());
                }
                endpoint.configured = true;
                endpoint.max_packet = max_packet as u16;
                endpoint.events = ((p[1] >> 8) & 7) as u8;
            }
            2 if p[2] == 1 && endpoint.configured => endpoint.resource = true,
            4 if endpoint.configured => endpoint.stalled = true,
            5 if endpoint.configured => endpoint.stalled = false,
            6 => {
                let address = u64::from(p[1]) | u64::from(p[2]) << 32;
                if !endpoint.configured
                    || !endpoint.resource
                    || endpoint.trb.is_some()
                    || address & 15 != 0
                {
                    return Err(
                        "DWC3 start transfer requires configured idle resource and aligned TRB"
                            .into(),
                    );
                }
                endpoint.trb = Some(address);
                endpoint.cursor = 0;
                resource = 1;
            }
            7 if endpoint.trb.is_some() && value >> 16 & 0x7f == 1 => resource = 1,
            8 => {
                endpoint.trb = None;
                endpoint.cursor = 0;
            }
            9 if ep == 0 => {
                if self.state.endpoints.iter().any(|e| e.trb.is_some()) {
                    return Err("DWC3 new configuration while transfers are active".into());
                }
            }
            _ => return Err(format!("Unsupported DWC3 endpoint command {command}")),
        }
        let mut result = value & !(ACTIVE | (15 << 12));
        if matches!(command, 6 | 7) {
            result = (result & !(0x7f << 16)) | resource << 16;
        }
        self.state.registers.insert(0xc80c + ep as u32 * 16, result);
        if value & (1 << 8) != 0 {
            self.event_word(cx, (ep as u32) << 1 | 7 << 6 | command << 24)?;
        }
        Ok(())
    }
    fn transfer(
        &mut self,
        cx: &mut Cx<'_>,
        ep: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> Result<Reply, String> {
        if !self.state.running || !self.state.connected || self.reg(0xc720) & (1 << ep) == 0 {
            return Ok(Reply::Nak);
        }
        if setup {
            self.state.endpoints[0].stalled = false;
            self.state.endpoints[1].stalled = false;
        }
        let endpoint = &self.state.endpoints[ep];
        if endpoint.stalled {
            return Ok(Reply::Stall);
        }
        let Some(trb) = endpoint.trb else {
            if endpoint.events & 4 != 0 {
                let phase = if ep < 2 {
                    if self.state.control.status { 2 } else { 1 }
                } else {
                    0
                };
                self.event_word(cx, (ep as u32) << 1 | 3 << 6 | phase << 12)?;
            }
            return Ok(Reply::Nak);
        };
        let bytes = cx.ram_bytes(trb, 16)?;
        let w: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        if w[3] & 1 == 0 {
            return Ok(Reply::Nak);
        }
        let kind = w[3] >> 4 & 63;
        if !(1..=5).contains(&kind) || w[3] & 4 != 0 || w[3] & 2 == 0 || (kind == 2) != setup {
            return Err("Unsupported DWC3 TRB type, chain or setup phase".into());
        }
        if (kind == 1) != (ep >= 2)
            || kind == 5
                && (self.state.control.status || (ep & 1 != 0) != self.state.control.data_in)
            || matches!(kind, 3 | 4)
                && (!self.state.control.status || (ep & 1 != 0) == self.state.control.data_in)
        {
            return Err("DWC3 TRB does not match the control bus phase".into());
        }
        let address = (u64::from(w[0]) | u64::from(w[1]) << 32)
            .checked_add(u64::from(endpoint.cursor))
            .ok_or("DWC3 packet address overflow")?;
        let remaining = (w[2] & 0xffffff)
            .checked_sub(endpoint.cursor)
            .ok_or("DWC3 TRB cursor exceeds its transfer length")?;
        let maximum = u32::from(endpoint.max_packet);
        let events = endpoint.events;
        if kind == 2 && remaining != 8
            || matches!(kind, 3 | 4) && remaining != 0
            || kind == 5 && remaining > u32::from(self.state.control.remaining)
        {
            return Err("DWC3 control TRB length disagrees with its bus phase".into());
        }
        let setup_bytes = if setup {
            out.as_deref().map(|v| v.to_vec())
        } else {
            None
        };
        let (reply, quantity) = if let Some(data) = out {
            if data.len() > remaining as usize || data.len() > maximum as usize {
                return Err("DWC3 OUT packet exceeds TRB or endpoint capacity".into());
            }
            if !data.is_empty() {
                cx.ram_write(address, &data)?;
            }
            (Reply::Ack, data.len() as u32)
        } else {
            let n = remaining.min(maximum);
            (Reply::Data(cx.ram_bytes(address, n as usize)?), n)
        };
        let left = remaining - quantity;
        let short = quantity < maximum;
        let complete = left == 0 || short;
        if kind == 2 {
            let setup = setup_bytes.ok_or("DWC3 SETUP needs eight bus bytes")?;
            let remaining = u16::from_le_bytes([setup[6], setup[7]]);
            self.state.control = Control {
                remaining,
                data_in: remaining != 0 && setup[0] & 0x80 != 0,
                status: remaining == 0,
            };
        } else if kind == 5 {
            self.state.control.remaining = self
                .state
                .control
                .remaining
                .checked_sub(quantity as u16)
                .ok_or("DWC3 control data exceeds SETUP length")?;
            if self.state.control.remaining == 0 || short {
                self.state.control.status = true;
            }
        } else if matches!(kind, 3 | 4) {
            self.state.control = Control::default();
        }
        if complete {
            cx.ram_write(trb + 8, &((w[2] & !0xffffff) | left).to_le_bytes())?;
            cx.ram_write(trb + 12, &(w[3] & !1).to_le_bytes())?;
            self.state.endpoints[ep].trb = None;
            self.state.endpoints[ep].cursor = 0;
            let status = (if short { 2 } else { 0 })
                | (if w[3] & (1 << 11) != 0 { 4 } else { 0 })
                | (if w[3] & 2 != 0 { 8 } else { 0 });
            if events & 1 != 0 {
                self.event_word(cx, (ep as u32) << 1 | 1 << 6 | status << 12)?;
            }
        } else {
            self.state.endpoints[ep].cursor = self.state.endpoints[ep]
                .cursor
                .checked_add(quantity)
                .ok_or("DWC3 transfer cursor overflow")?;
        }
        Ok(reply)
    }
    fn advance_inner(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        while self.state.pending.first().is_some_and(|p| p.at <= cx.now()) {
            let pending = self.state.pending.remove(0);
            match pending.operation {
                Operation::Reset => {
                    self.state.endpoints = std::array::from_fn(|_| Endpoint::default());
                    self.state.running = false;
                    self.state.control = Control::default();
                    self.state.periodic_parameters = 0;
                    self.state.ring_cursor = 0;
                    self.state.registers.clear();
                }
                Operation::Run(run) => {
                    let start = run && !self.state.running;
                    self.state.running = run;
                    if start && self.state.connected {
                        self.device_event(cx, 2)?;
                    }
                }
                Operation::Link(connected) => {
                    let changed = self.state.connected != connected;
                    self.state.connected = connected;
                    if self.state.running && changed {
                        self.device_event(cx, if connected { 2 } else { 0 })?;
                    }
                }
                Operation::BusReset => {
                    if !self.state.running || !self.state.connected {
                        return Err("DWC3 host reset without a running link".into());
                    }
                    for ep in &mut self.state.endpoints {
                        ep.trb = None;
                        ep.stalled = false;
                        ep.cursor = 0;
                    }
                    self.state.control = Control::default();
                    self.state
                        .registers
                        .insert(0xc700, self.reg(0xc700) & !(0x7f << 3));
                    self.device_event(cx, 1)?;
                }
                Operation::GlobalCommand { value, parameter } => {
                    self.state.periodic_parameters = parameter;
                    self.state.registers.insert(0xc714, value & !ACTIVE);
                }
                Operation::Command {
                    ep,
                    value,
                    parameters,
                } => self.command(cx, ep, value, parameters)?,
                Operation::Transaction { id, ep, setup, out } => {
                    let reply = self.transfer(cx, ep, setup, out)?;
                    self.state.replies.insert(id, reply);
                }
            }
        }
        cx.set_irq(
            self.config.irq,
            self.reg(0xc408) & (1 << 31) == 0 && self.reg(0xc40c) != 0,
        );
        Ok(())
    }
    fn offset(&self, address: u64, size: u8) -> Option<u32> {
        let offset = u32::try_from(address.checked_sub(self.config.base)?).ok()?;
        (size == 4
            && offset & 3 == 0
            && (matches!(
                offset,
                0xc100
                    | 0xc104
                    | 0xc110
                    | 0xc200
                    | 0xc2c0
                    | 0xc400
                    | 0xc404
                    | 0xc408
                    | 0xc40c
                    | 0xc700
                    | 0xc704
                    | 0xc708
                    | 0xc70c
                    | 0xc710
                    | 0xc714
                    | 0xc720
            ) || (0xc800..0xca00).contains(&offset)))
        .then_some(offset)
    }
}
impl Device for Dwc3 {
    fn name(&self) -> &'static str {
        "dwc3"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        self.advance(cx);
        if cx.refusal().is_some() {
            return 0;
        }
        if offset == DSTS {
            return u64::from(
                u32::from(self.config.speed)
                    | if self.state.connected { 0 } else { 4 << 18 }
                    | if self.state.running { 0 } else { 1 << 22 },
            );
        }
        u64::from(self.reg(offset))
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u32::MAX as u64 || offset == DSTS {
            cx.unmodeled();
            return;
        }
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        let value = value as u32;
        let result = (|| -> Result<(), String> {
            if self
                .state
                .pending
                .iter()
                .any(|p| matches!(p.operation, Operation::Reset))
            {
                return Err("DWC3 register programming during core reset".into());
            }
            match offset {
                0xc40c => {
                    if value & 3 != 0 || value > self.reg(offset) {
                        return Err("DWC3 event ACK exceeds pending bytes".into());
                    }
                    self.state
                        .registers
                        .insert(offset, self.reg(offset) - value);
                }
                0xc408 => {
                    let size = value & 0xffff;
                    if value & !0x8000fffc != 0
                        || size < 4
                        || size & 3 != 0
                        || self.reg(0xc40c) != 0
                    {
                        return Err("Invalid DWC3 event ring size or unread reconfiguration".into());
                    }
                    self.state.ring_cursor = 0;
                    self.state.registers.insert(offset, value);
                }
                0xc400 | 0xc404 => {
                    if self.reg(0xc40c) != 0 || offset == 0xc400 && value & 3 != 0 {
                        return Err("DWC3 event ring moved with unread events".into());
                    }
                    self.state.ring_cursor = 0;
                    self.state.registers.insert(offset, value);
                }
                DCTL => {
                    if value & !(RUN | RESET | 0x1e00) != 0 {
                        return Err("Unsupported DWC3 power, test or link-state request".into());
                    }
                    let old = self.reg(offset);
                    if value & RESET != 0 {
                        if self.state.running || value & RUN != 0 || !self.state.pending.is_empty()
                        {
                            return Err("Unsupported active DWC3 core reset".into());
                        }
                        self.queue(cx.now(), self.config.command_ticks, Operation::Reset)?;
                    } else if (old ^ value) & RUN != 0 {
                        if self
                            .state
                            .pending
                            .iter()
                            .any(|p| matches!(p.operation, Operation::Run(_)))
                        {
                            return Err("DWC3 run/stop transition is still active".into());
                        }
                        self.queue(
                            cx.now(),
                            self.config.command_ticks,
                            Operation::Run(value & RUN != 0),
                        )?;
                    }
                    self.state.registers.insert(offset, value);
                }
                0xc714 => {
                    if value & !0x40f != 0
                        || value & ACTIVE == 0
                        || value & 15 != 2
                        || self.reg(offset) & ACTIVE != 0
                    {
                        return Err("Unsupported or already active DWC3 generic command".into());
                    }
                    self.queue(
                        cx.now(),
                        self.config.command_ticks,
                        Operation::GlobalCommand {
                            value,
                            parameter: self.reg(0xc710),
                        },
                    )?;
                    self.state.registers.insert(offset, value);
                }
                0xc800..=0xc9fc if offset & 15 == 12 => {
                    if value & ACTIVE == 0 || self.reg(offset) & ACTIVE != 0 {
                        return Err("DWC3 endpoint command absent or already active".into());
                    }
                    let ep = ((offset - 0xc800) / 16) as usize;
                    let parameters = std::array::from_fn(|n| self.reg(offset - 12 + n as u32 * 4));
                    self.queue(
                        cx.now(),
                        self.config.command_ticks,
                        Operation::Command {
                            ep,
                            value,
                            parameters,
                        },
                    )?;
                    self.state.registers.insert(offset, value);
                }
                _ => {
                    if offset == 0xc708 && value & !7 != 0
                        || offset == 0xc110 && value & (1 << 11) != 0
                        || matches!(offset, 0xc200 | 0xc2c0) && value & (1 << 31) != 0
                    {
                        return Err(
                            "Unsupported DWC3 event source or external PHY/core reset".into()
                        );
                    }
                    self.state.registers.insert(offset, value);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            cx.fail(error);
        }
        cx.set_irq(
            self.config.irq,
            self.reg(0xc408) & (1 << 31) == 0 && self.reg(0xc40c) != 0,
        );
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(error) = self.advance_inner(cx) {
            cx.fail(error);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == 0 {
            self.advance(cx);
        } else {
            cx.fail("Unknown DWC3 event token");
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.pending.first().map(|p| p.at)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(Saved {
            config: self.config,
            state: self.state.clone(),
        })
        .unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let saved: Saved = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let state = &saved.state;
        let reg = |offset| state.registers.get(&offset).copied().unwrap_or(0);
        let ring_size = reg(0xc408) & 0xffff;
        let mut orders = std::collections::BTreeSet::new();
        let mut commands = std::collections::BTreeSet::new();
        let mut global_commands = 0;
        let mut resets = 0;
        let mut running = state.running;
        for p in &state.pending {
            if p.order == 0 || p.order >= state.next_order || !orders.insert(p.order) {
                return Err("Invalid DWC3 pending transaction order".into());
            }
            match &p.operation {
                Operation::Reset => {
                    resets += 1;
                    running = false;
                }
                Operation::Run(value) => running = *value,
                Operation::Command { ep, value, .. } => {
                    if *ep >= 32
                        || value & ACTIVE == 0
                        || !commands.insert(*ep)
                        || reg(0xc80c + *ep as u32 * 16) != *value
                    {
                        return Err("Invalid DWC3 pending endpoint command".into());
                    }
                }
                Operation::GlobalCommand { value, .. } => {
                    global_commands += 1;
                    if value & !0x40f != 0
                        || value & ACTIVE == 0
                        || value & 15 != 2
                        || reg(0xc714) != *value
                    {
                        return Err("Invalid DWC3 pending generic command".into());
                    }
                }
                Operation::Transaction { id, ep, setup, out } => {
                    if *id != p.order
                        || *ep >= 32
                        || (*ep & 1 == 0) != out.is_some()
                        || out.as_ref().is_some_and(|v| v.len() > 1024)
                        || *setup && (*ep != 0 || out.as_ref().is_none_or(|v| v.len() != 8))
                    {
                        return Err("Invalid DWC3 pending host packet".into());
                    }
                }
                _ => {}
            }
        }
        for (&id, reply) in &state.replies {
            if id == 0
                || id >= state.next_order
                || !orders.insert(id)
                || matches!(reply, Reply::Data(bytes) if bytes.len() > 1024)
            {
                return Err("Invalid DWC3 retained host reply".into());
            }
        }
        if saved.config != self.config
            || saved.state.pending.len() + saved.state.replies.len() > 128
            || saved.state.next_order == 0
            || saved
                .state
                .pending
                .windows(2)
                .any(|p| (p[0].at, p[0].order) >= (p[1].at, p[1].order))
            || saved.state.endpoints.iter().enumerate().any(|(ep, e)| {
                e.max_packet > 1024
                    || e.cursor > 0xffffff
                    || e.events > 7
                    || e.configured != (e.max_packet != 0)
                    || !e.configured && (e.resource || e.stalled || e.events != 0)
                    || e.trb.is_none() && e.cursor != 0
                    || e.trb.is_some() && (!e.configured || !e.resource)
                    || e.trb
                        .is_some_and(|a| a & 15 != 0 || a.checked_add(16).is_none())
                    || (reg(0xc80c + ep as u32 * 16) & ACTIVE != 0) != commands.contains(&ep)
            })
            || state.registers.keys().any(|&offset| {
                offset == DSTS
                    || self
                        .config
                        .base
                        .checked_add(u64::from(offset))
                        .and_then(|address| self.offset(address, 4))
                        .is_none()
            })
            || ring_size & 3 != 0
            || reg(0xc408) & !0x8000fffc != 0
            || reg(0xc708) & !7 != 0
            || reg(DCTL) & !(RUN | RESET | 0x1e00) != 0
            || reg(0xc400) & 3 != 0
            || reg(0xc40c) & 3 != 0
            || reg(0xc40c) > ring_size
            || state.ring_cursor & 3 != 0
            || if ring_size == 0 {
                state.ring_cursor != 0
            } else {
                state.ring_cursor >= ring_size
            }
            || global_commands > 1
            || (reg(0xc714) & ACTIVE != 0) != (global_commands == 1)
            || resets > 1
            || (reg(DCTL) & RESET != 0) != (resets == 1)
            || (reg(DCTL) & RUN != 0) != running
        {
            return Err("Invalid DWC3 checkpoint or changed physical wiring".into());
        }
        self.state = saved.state;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
