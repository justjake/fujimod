//! Opaque USB endpoint/FIFO transport with declared access widths and DMA
//! behavior. No descriptors or higher-level host commands are interpreted.
//! Packet capacities, delay and IRQ wiring are inputs.
use fujimod_emulator_devices::{Cx, Device, bus::InterruptLine};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
pub const CORE: u64 = 0xfff54000;
pub const PHY: u64 = 0xfff55000;
const HALF: &[usize] = &[
    4, 8, 0x2c, 0x30, 0x34, 0x40, 0x44, 0x48, 0x54, 0x58, 0x5c, 0x68, 0x6c, 0x70, 0x80, 0x88, 0x90,
    0x94, 0x98, 0x9c, 0xac, 0x184, 0x194, 0x1ec, 0x1f0, 0x1f4, 0x1f8, 0x218, 0x21c,
];
const WORD: &[usize] = &[
    0x180, 0x188, 0x18c, 0x190, 0x194, 0x19c, 0x1108, 0x110c, 0x1114, 0x111c, 0x1120, 0x1124,
    0x1128, 0x112c,
];
const DMA: &[usize] = &[4, 12, 16, 0x20, 0x24, 0x28, 0x44];
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct EndpointOptions {
    pub transfer_ticks: u64,
    pub packet_bytes: usize,
    pub dma_limit: usize,
    pub out_endpoints: Vec<(u8, usize)>,
    pub in_endpoints: Vec<(u8, usize)>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct Wiring {
    pub irq: InterruptLine,
    pub dma_irq: InterruptLine,
    pub ram_bytes: u64,
}
/// Bus progress observed by a host, without DMA payloads.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct BusStatus {
    pub options: EndpointOptions,
    pub wiring: Wiring,
    pub registers: Vec<u32>,
    pub rx_length: [usize; 2],
    pub rx_position: [usize; 2],
    pub setup_length: usize,
    pub setup_position: usize,
    pub tx_packets: Vec<usize>,
    pub tx_bytes: Vec<usize>,
    pub active_dma: bool,
}
#[derive(serde::Serialize)]
struct Queue {
    bytes: VecDeque<u8>,
    packets: VecDeque<usize>,
    limit: usize,
}
impl<'de> serde::Deserialize<'de> for Queue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Saved {
            bytes: VecDeque<u8>,
            packets: VecDeque<usize>,
            limit: usize,
        }
        let mut saved = Saved::deserialize(deserializer)?;
        if !(64..=(1 << 20) + 512).contains(&saved.limit)
            || saved.bytes.len() > saved.limit
            || saved.packets.len() > saved.limit / 32 + 1
            || saved
                .packets
                .iter()
                .try_fold(0usize, |sum, n| sum.checked_add(*n))
                != Some(saved.bytes.len())
        {
            return Err(serde::de::Error::custom(
                "invalid USB packet queue snapshot",
            ));
        }
        saved.bytes.reserve(saved.limit - saved.bytes.len());
        saved
            .packets
            .reserve(saved.limit / 32 + 1 - saved.packets.len());
        Ok(Self {
            bytes: saved.bytes,
            packets: saved.packets,
            limit: saved.limit,
        })
    }
}
impl Queue {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(limit),
            packets: VecDeque::with_capacity(limit / 32 + 1),
            limit,
        }
    }
    fn push(&mut self, payload: &[u8]) -> Result<(), String> {
        if self.bytes.len() + payload.len() > self.limit
            || self.packets.len() >= self.limit / 32 + 1
        {
            return Err("USB IN queue is full".into());
        }
        self.bytes.extend(payload);
        self.packets.push_back(payload.len());
        Ok(())
    }
    fn clear(&mut self) {
        self.bytes.clear();
        self.packets.clear();
    }
    fn take(&mut self, size: usize) -> Result<Vec<u8>, String> {
        let length = *self
            .packets
            .front()
            .ok_or("USB IN endpoint has no packet")?;
        let n = length.min(size);
        let bytes = self.bytes.drain(..n).collect();
        if n == length {
            self.packets.pop_front();
        } else {
            *self.packets.front_mut().unwrap() -= n;
        }
        Ok(bytes)
    }
}
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Direction {
    In,
    Out,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Transfer {
    channel: usize,
    direction: Direction,
    address: u64,
    exact: usize,
    received: usize,
    due: Option<u64>,
}
pub struct Endpoint {
    options: EndpointOptions,
    wiring: Wiring,
    regs: Box<[u32]>,
    dma: [[u32; 18]; 2],
    rx: [[u8; 512]; 2],
    rx_length: [usize; 2],
    rx_position: [usize; 2],
    setup: [u8; 8],
    setup_length: usize,
    setup_position: usize,
    staging: [[u8; 512]; 3],
    staged: [usize; 3],
    tx: [Queue; 3],
    /// Status packets committed to the full FIFO port 2 and lost.
    dropped_status: u64,
    partial: usize,
    fresh: [bool; 2],
    active: Option<Transfer>,
    scratch: Box<[u8]>,
}
impl Endpoint {
    pub fn new(options: EndpointOptions, wiring: Wiring) -> Result<Self, String> {
        if options.transfer_ticks == 0
            || !matches!(options.packet_bytes, 64 | 512)
            || !(64..=1 << 20).contains(&options.dma_limit)
            || wiring.ram_bytes == 0
            || wiring.ram_bytes > 0x40000000
        {
            return Err("Invalid USB packet, DMA or timing inputs".into());
        }
        for (rows, input, max_channel) in [
            (&options.out_endpoints, false, 1),
            (&options.in_endpoints, true, 2),
        ] {
            if rows.is_empty()
                || rows.iter().any(|&(ep, ch)| {
                    ep & 0x70 != 0 || (ep & 0x80 != 0) != input || ch > max_channel
                })
            {
                return Err("Invalid USB endpoint wiring".into());
            }
            for (i, row) in rows.iter().enumerate() {
                if rows[..i].iter().any(|r| r.0 == row.0 || r.1 == row.1) {
                    return Err("Duplicate USB endpoint or FIFO port".into());
                }
            }
        }
        let mut regs = vec![0; 0x1130 / 4].into_boxed_slice();
        regs[0x9c / 4] = if options.packet_bytes == 512 {
            0x4000
        } else {
            0
        };
        let limit = options.dma_limit;
        Ok(Self {
            options,
            wiring,
            regs,
            dma: [[0; 18]; 2],
            rx: [[0; 512]; 2],
            rx_length: [0; 2],
            rx_position: [0; 2],
            setup: [0; 8],
            setup_length: 0,
            setup_position: 0,
            staging: [[0; 512]; 3],
            staged: [0; 3],
            tx: [Queue::new(4096), Queue::new(limit + 512), Queue::new(2048)],
            dropped_status: 0,
            partial: 4,
            fresh: [false; 2],
            active: None,
            scratch: vec![0; limit].into_boxed_slice(),
        })
    }
    fn reg(&self, o: usize) -> u32 {
        self.regs[o / 4]
    }
    fn put(&mut self, o: usize, v: u32) {
        self.regs[o / 4] = v;
    }
    fn signal(&self, cx: &mut Cx<'_>) -> Result<(), String> {
        let high = [(0x2c, 0x40), (0x30, 0x44), (0x34, 0x48)]
            .iter()
            .any(|&(s, m)| self.reg(s) & !self.reg(m) & 65535 != 0);
        self.wiring.irq.drive(cx, high)
    }
    fn flag(&mut self, cx: &mut Cx<'_>, o: usize, bits: u32) -> Result<(), String> {
        self.put(o, self.reg(o) | bits);
        self.signal(cx)
    }
    /// Observe bus progress without copying DMA payloads or consuming packets.
    pub fn bus_status(&self) -> BusStatus {
        BusStatus {
            options: self.options.clone(),
            wiring: self.wiring.clone(),
            registers: self.regs.to_vec(),
            rx_length: self.rx_length,
            rx_position: self.rx_position,
            setup_length: self.setup_length,
            setup_position: self.setup_position,
            tx_packets: self.tx.iter().map(|q| q.packets.len()).collect(),
            tx_bytes: self.tx.iter().map(|q| q.bytes.len()).collect(),
            active_dma: self.active.is_some(),
        }
    }
    pub fn host_reset(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.active.is_some() {
            return Err("USB bus reset during active DMA unsupported".into());
        }
        self.rx_length = [0; 2];
        self.rx_position = [0; 2];
        self.setup_length = 0;
        self.setup_position = 0;
        self.staged = [0; 3];
        for q in &mut self.tx {
            q.clear();
        }
        self.flag(cx, 0x34, 0x1000)
    }
    pub fn host_setup(&mut self, cx: &mut Cx<'_>, packet: &[u8]) -> Result<(), String> {
        if packet.len() != 8 || self.setup_position < self.setup_length {
            return Err("USB setup requires eight bytes and empty FIFO".into());
        }
        self.setup.copy_from_slice(packet);
        self.setup_length = 8;
        self.setup_position = 0;
        self.flag(cx, 0x2c, 0x2000)
    }
    pub fn host_out(&mut self, cx: &mut Cx<'_>, endpoint: u8, packet: &[u8]) -> Result<(), String> {
        let channel = self
            .options
            .out_endpoints
            .iter()
            .find(|r| r.0 == endpoint)
            .ok_or("Unknown USB OUT endpoint")?
            .1;
        let capacity = if channel == 0 {
            64
        } else {
            self.options.packet_bytes
        };
        if packet.len() > capacity || channel == 1 && packet.is_empty() {
            return Err("USB OUT packet extent unsupported".into());
        }
        if self.rx_position[channel] < self.rx_length[channel] {
            return Err("USB OUT FIFO occupied".into());
        }
        self.rx[channel][..packet.len()].copy_from_slice(packet);
        self.rx_length[channel] = packet.len();
        self.rx_position[channel] = 0;
        self.put(0x184 + channel * 16, packet.len() as u32);
        if channel == 1
            && self
                .active
                .as_ref()
                .is_some_and(|a| a.direction == Direction::Out)
        {
            self.receive(cx)
        } else if channel == 0
            && packet.is_empty()
            && self.setup_length != 0
            && self.setup[0] & 0x80 != 0
        {
            self.flag(cx, 0x34, 2)
        } else {
            self.flag(cx, 0x2c, if channel == 0 { 1 } else { 4 })
        }
    }
    pub fn host_in(
        &mut self,
        cx: &mut Cx<'_>,
        endpoint: u8,
        size: usize,
    ) -> Result<Vec<u8>, String> {
        let port = self
            .options
            .in_endpoints
            .iter()
            .find(|r| r.0 == endpoint)
            .ok_or("Unknown USB IN endpoint")?
            .1;
        if size == 0 {
            return Err("USB IN extent must be positive".into());
        }
        let packet = self.tx[port].take(size)?;
        if self.tx[port].packets.is_empty() {
            if port == 0 && packet.is_empty() && self.setup_length != 0 && self.setup[0] & 0x80 == 0
            {
                self.flag(cx, 0x34, 2)?;
            } else {
                self.flag(cx, 0x2c, [2, 8, 0x10][port])?;
            }
        }
        Ok(packet)
    }
    pub fn host_in_token(&mut self, cx: &mut Cx<'_>, endpoint: u8) -> Result<(), String> {
        let port = self
            .options
            .in_endpoints
            .iter()
            .find(|r| r.0 == endpoint)
            .ok_or("Unknown USB IN endpoint")?
            .1;
        if port != 1 || !self.tx[port].packets.is_empty() {
            return Err("USB IN token requires empty bulk FIFO".into());
        }
        self.flag(cx, 0x30, 4)
    }
    fn try_dma(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.active.is_some() || self.reg(0x110c) & 1 == 0 {
            return Ok(());
        }
        let enabled = [self.dma[0][1] & 1 != 0, self.dma[1][1] & 1 != 0];
        if enabled[0] == enabled[1] {
            return Err("USB DMA requires exactly one enabled channel".into());
        }
        let channel = usize::from(enabled[1]);
        let input = self.reg(0x70) & 2 != 0 && self.fresh[0];
        let output = self.reg(0x70) & 1 != 0 && self.fresh[1];
        if !input && !output {
            return Ok(());
        }
        if input && output {
            return Err("USB DMA direction is ambiguous".into());
        }
        let direction = if input { Direction::In } else { Direction::Out };
        let exact = self.reg(if input { 0x1124 } else { 0x1120 }) as usize;
        let count = self.dma[channel][0x28 / 4] as usize;
        let address = self.dma[channel][0x24 / 4] as u64;
        if exact == 0
            || exact > count
            || count > self.options.dma_limit
            || address + count as u64 > self.wiring.ram_bytes
            || self.dma[channel][0x20 / 4] & 0x80703000 != 0x80401000
        {
            return Err("USB DMA extent or unit/burst grammar unsupported".into());
        }
        if input {
            cx.ram_read(address, &mut self.scratch[..exact])?;
        }
        self.fresh[if input { 0 } else { 1 }] = false;
        self.dma[channel][0x10 / 4] = 1;
        self.active = Some(Transfer {
            channel,
            direction,
            address,
            exact,
            received: 0,
            due: if input {
                Some(
                    cx.now()
                        .checked_add(self.options.transfer_ticks)
                        .ok_or("USB deadline overflow")?,
                )
            } else {
                None
            },
        });
        if !input {
            self.receive(cx)?;
        }
        Ok(())
    }
    fn receive(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let Some(a) = self.active.as_mut() else {
            return Ok(());
        };
        if a.direction != Direction::Out {
            return Ok(());
        }
        let count = (self.rx_length[1] - self.rx_position[1]).min(a.exact - a.received);
        if count == 0 {
            return Ok(());
        }
        self.scratch[a.received..a.received + count]
            .copy_from_slice(&self.rx[1][self.rx_position[1]..self.rx_position[1] + count]);
        a.received += count;
        self.rx_position[1] += count;
        if a.received == a.exact {
            a.due = Some(
                cx.now()
                    .checked_add(self.options.transfer_ticks)
                    .ok_or("USB deadline overflow")?,
            );
        }
        self.put(0x194, (self.rx_length[1] - self.rx_position[1]) as u32);
        self.put(0x2c, self.reg(0x2c) & !4);
        self.signal(cx)
    }
    fn complete(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let Some(a) = self.active.as_ref() else {
            return Ok(());
        };
        if a.due.is_none_or(|due| due > cx.now()) {
            return Ok(());
        }
        if a.direction == Direction::In {
            let capacity = self.options.packet_bytes;
            let q = &self.tx[1];
            if q.bytes.len() + a.exact > q.limit
                || q.packets.len() + a.exact.div_ceil(capacity) > q.limit / 32 + 1
            {
                return Err("USB DMA completion exceeds IN queue capacity".into());
            }
            for packet in self.scratch[..a.exact].chunks(capacity) {
                self.tx[1].push(packet)?;
            }
            self.put(0x34, self.reg(0x34) | 0x40);
        } else {
            cx.ram_write(a.address, &self.scratch[..a.exact])?;
        }
        let a = self.active.take().unwrap();
        self.dma[a.channel][0x10 / 4] = 0;
        self.dma[a.channel][0x44 / 4] = 1;
        if a.direction == Direction::Out && self.rx_position[1] < self.rx_length[1] {
            self.put(0x2c, self.reg(0x2c) | 4);
        }
        self.signal(cx)?;
        if self.dma[a.channel][0x20 / 4] & 0x10000000 != 0 {
            self.wiring.dma_irq.drive(cx, true)?;
        }
        Ok(())
    }
    pub fn offset(address: u64, size: u8) -> Option<(bool, usize, usize)> {
        if let Some(relative) = address.checked_sub(PHY + 0x800)
            && relative < 0x100
            && size == 4
            && DMA.contains(&(relative as usize % 0x80))
        {
            return Some((true, relative as usize / 0x80, relative as usize % 0x80));
        }
        let offset = address.checked_sub(CORE)? as usize;
        if size == 2 && HALF.contains(&offset) || size == 4 && WORD.contains(&offset) {
            Some((false, 0, offset))
        } else {
            None
        }
    }
    fn load(
        &mut self,
        cx: &mut Cx<'_>,
        dma: bool,
        channel: usize,
        o: usize,
        size: u8,
    ) -> Result<u32, String> {
        if dma {
            return Ok(self.dma[channel][o / 4]);
        }
        if o == 0x188 && size == 4 {
            if self.setup_position >= self.setup_length {
                return Err("Empty USB setup FIFO".into());
            }
            let value = u32::from_le_bytes(
                self.setup[self.setup_position..self.setup_position + 4]
                    .try_into()
                    .unwrap(),
            );
            self.setup_position += 4;
            return Ok(value);
        }
        if size == 4 && matches!(o, 0x180 | 0x190) {
            let channel = (o - 0x180) / 16;
            let position = self.rx_position[channel];
            let count = (self.rx_length[channel] - position).min(4);
            if count == 0 {
                return Err("Empty USB OUT FIFO".into());
            }
            let mut bytes = [0; 4];
            bytes[..count].copy_from_slice(&self.rx[channel][position..position + count]);
            self.rx_position[channel] += count;
            self.put(
                0x184 + channel * 16,
                (self.rx_length[channel] - self.rx_position[channel]) as u32,
            );
            if self.rx_length[channel] == self.rx_position[channel] {
                self.put(0x2c, self.reg(0x2c) & !if channel == 0 { 1 } else { 4 });
                self.signal(cx)?;
            }
            return Ok(u32::from_le_bytes(bytes));
        }
        if matches!(o, 0x18c | 0x19c) || o == 0x194 && size == 4 {
            return Err("USB TX FIFO is write-only".into());
        }
        Ok(self.reg(o) & if size == 2 { 65535 } else { u32::MAX })
    }
    fn store(
        &mut self,
        cx: &mut Cx<'_>,
        dma: bool,
        channel: usize,
        o: usize,
        size: u8,
        value: u32,
    ) -> Result<(), String> {
        if dma {
            match o {
                12 | 0x44 => {
                    if value != 1 {
                        return Err("USB DMA reset/ACK requires bit0".into());
                    }
                    self.dma[channel][0x44 / 4] = 0;
                    self.wiring.dma_irq.drive(
                        cx,
                        self.dma
                            .iter()
                            .any(|r| r[0x44 / 4] != 0 && r[0x20 / 4] & 0x10000000 != 0),
                    )?;
                }
                4 => {
                    if value & !0x10001 != 0 {
                        return Err("USB DMA control bits unsupported".into());
                    }
                    if value & 0x10000 != 0 {
                        if self.active.as_ref().is_some_and(|a| a.channel == channel) {
                            self.active = None;
                        }
                        self.dma[channel][0x10 / 4] = 0;
                    }
                    self.dma[channel][1] = value & !0x10000;
                }
                16 => return Err("USB DMA status is read-only".into()),
                0x24 if value & 0xc0000003 != 0 => {
                    return Err("USB DMA address must be aligned physical DRAM".into());
                }
                0x28 if value & 0xf0000003 != 0 || value as usize > self.options.dma_limit => {
                    return Err("USB DMA count exceeds aligned bound".into());
                }
                _ => self.dma[channel][o / 4] = value,
            }
            return Ok(());
        }
        if matches!(o, 0x2c | 0x30 | 0x34 | 0x9c | 0x184 | 0x1128) || o == 0x194 && size == 2 {
            return Err("USB event/count/speed input is read-only".into());
        }
        match o {
            0x18c | 0x194 | 0x19c if size == 4 => {
                let port = (o - 0x18c) / 8;
                let capacity = match port {
                    0 => 64,
                    1 => self.options.packet_bytes,
                    _ => 32,
                };
                if self.staged[port] + self.partial > capacity {
                    return Err("USB TX FIFO packet exceeds capacity".into());
                }
                let used = self.staged[port];
                self.staging[port][used..used + self.partial]
                    .copy_from_slice(&value.to_le_bytes()[..self.partial]);
                self.staged[port] += self.partial;
            }
            0x180 | 0x188 | 0x190 if size == 4 => {
                return Err("USB receive/setup FIFO is read-only".into());
            }
            0x111c => {
                self.partial = match value {
                    0x80000 => 3,
                    0xc0000 => 2,
                    0xe0000 => 1,
                    0x10..=0x13 => 4,
                    _ => return Err("Unsupported USB partial-word control".into()),
                };
                self.put(o, value);
            }
            0x88 => {
                if ![1, 2, 4].contains(&value) {
                    return Err("USB FIFO commit requires one port".into());
                }
                let port = value.trailing_zeros() as usize;
                let committed = self.tx[port].push(&self.staging[port][..self.staged[port]]);
                match committed {
                    // Stock firmware posts a two-byte status on port 2 after
                    // every service command and never waits for the host to
                    // read it; cameras run long service sessions whose hosts
                    // never poll that endpoint. A commit to the full status
                    // FIFO is lost, as it must be on the camera; the control
                    // and bulk ports still refuse.
                    Err(_) if port == 2 => self.dropped_status += 1,
                    result => result?,
                }
                self.staged[port] = 0;
            }
            0x54 | 0x58 | 0x5c => {
                let status = o - 0x28;
                self.put(status, self.reg(status) & value);
                self.put(o, value);
                self.signal(cx)?;
            }
            0x80 if value & !0xfff != 0 => return Err("USB FIFO reset bits unsupported".into()),
            _ => {
                self.put(o, value);
                if matches!(o, 0x40 | 0x44 | 0x48) {
                    self.signal(cx)?;
                }
                if matches!(o, 0x1120 | 0x1124) {
                    self.fresh[if o == 0x1124 { 0 } else { 1 }] = true;
                }
                if matches!(o, 0x110c | 0x1120 | 0x1124) {
                    self.try_dma(cx)?;
                }
            }
        }
        Ok(())
    }
}
impl Device for Endpoint {
    fujimod_emulator_devices::checkpoint_fields!(regs:Box<[u32]>,dma:[[u32;18];2],#[serde(with="fujimod_emulator_devices::checkpoint::matrix")] rx:[[u8;512];2],rx_length:[usize;2],rx_position:[usize;2],setup:[u8;8],setup_length:usize,setup_position:usize,#[serde(with="fujimod_emulator_devices::checkpoint::matrix")] staging:[[u8;512];3],staged:[usize;3],tx:[Queue;3],dropped_status:u64,partial:usize,fresh:[bool;2],active:Option<Transfer>,scratch:Box<[u8]>);

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        // save includes options, wiring, all FIFO bytes/positions and pending DMA.
        Ok(self.save())
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({
            "options": self.options, "wiring": self.wiring, "registers": self.regs,
            "dma": self.dma, "rx": self.rx.iter().map(|row| row.as_slice()).collect::<Vec<_>>(),
            "rx_length": self.rx_length, "rx_position": self.rx_position,
            "setup": self.setup, "setup_length": self.setup_length, "setup_position": self.setup_position,
            "staging": self.staging.iter().map(|row| row.as_slice()).collect::<Vec<_>>(),
            "staged": self.staged, "tx": self.tx, "partial": self.partial,
            "fresh": self.fresh, "active": self.active, "scratch": self.scratch,
        })
    }
    fn name(&self) -> &'static str {
        "usb-endpoint"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some((dma, ch, o)) = Self::offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        self.advance(cx);
        if cx.refusal().is_some() {
            return 0;
        }
        match self.load(cx, dma, ch, o, size) {
            Ok(v) => v as u64,
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some((dma, ch, o)) = Self::offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        let v = value as u32 & if size == 2 { 65535 } else { u32::MAX };
        if let Err(e) = self.store(cx, dma, ch, o, size, v) {
            cx.fail(e);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.complete(cx) {
            cx.fail(e);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.active.as_ref().and_then(|a| a.due)
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::offset(address, size)
            .is_some_and(|(dma, _, o)| dma || !matches!(o, 0x180 | 0x188 | 0x190))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::ram::{Effect, Ram};
    fn model() -> Endpoint {
        Endpoint::new(
            EndpointOptions {
                transfer_ticks: 4,
                packet_bytes: 64,
                dma_limit: 1024,
                out_endpoints: vec![(0, 0), (1, 1)],
                in_endpoints: vec![(0x80, 0), (0x82, 1), (0x83, 2)],
            },
            Wiring {
                irq: InterruptLine::Shared(84),
                dma_irq: InterruptLine::Shared(85),
                ram_bytes: 0x10000,
            },
        )
        .unwrap()
    }
    fn write(m: &mut Endpoint, h: &mut Ram, a: u64, s: u8, v: u64) {
        let mut cx = h.cx();
        m.write(&mut cx, a, s, v);
        assert_eq!(cx.refusal(), None, "{a:x}");
    }
    fn read(m: &mut Endpoint, h: &mut Ram, a: u64, s: u8) -> u64 {
        let mut cx = h.cx();
        let v = m.read(&mut cx, a, s);
        assert_eq!(cx.refusal(), None, "{a:x}");
        v
    }
    fn arm(m: &mut Endpoint, h: &mut Ram, input: bool, count: usize, exact: usize) {
        for (o, v) in [
            (4, 1),
            (0x20, 0x90401000),
            (0x24, 0x1000),
            (0x28, count as u64),
        ] {
            write(m, h, PHY + 0x800 + o, 4, v);
        }
        write(m, h, CORE + 0x70, 2, if input { 2 } else { 1 });
        write(
            m,
            h,
            CORE + if input { 0x1124 } else { 0x1120 },
            4,
            exact as u64,
        );
        write(m, h, CORE + 0x110c, 4, 1);
    }
    #[test]
    fn fifo_setup_partial_words_and_directional_registers() {
        let mut m = model();
        let mut h = Ram::new(0x10000);
        m.host_setup(&mut h.cx(), &[0, 1, 2, 3, 4, 5, 6, 7])
            .unwrap();
        assert_eq!(read(&mut m, &mut h, CORE + 0x188, 4), 0x03020100);
        assert_eq!(read(&mut m, &mut h, CORE + 0x188, 4), 0x07060504);
        m.host_out(&mut h.cx(), 1, &[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(read(&mut m, &mut h, CORE + 0x194, 2), 5);
        assert_eq!(read(&mut m, &mut h, CORE + 0x190, 4), 0x04030201);
        assert_eq!(read(&mut m, &mut h, CORE + 0x190, 4), 5);
        write(&mut m, &mut h, CORE + 0x111c, 4, 0xc0000);
        write(&mut m, &mut h, CORE + 0x194, 4, 0xdeadbeef);
        write(&mut m, &mut h, CORE + 0x88, 2, 2);
        assert_eq!(m.host_in(&mut h.cx(), 0x82, 64).unwrap(), [0xef, 0xbe]);
        let mut cx = h.cx();
        m.write(&mut cx, CORE + 0x194, 2, 1);
        assert!(cx.refusal().is_some());
    }
    #[test]
    fn out_dma_waits_for_wire_bytes_then_deadline_and_preserves_tail() {
        let mut m = model();
        let mut h = Ram::new(0x10000);
        arm(&mut m, &mut h, false, 8, 6);
        assert_eq!(m.next_deadline(0), None);
        m.host_out(&mut h.cx(), 1, &[1, 2, 3, 4]).unwrap();
        assert_eq!(m.next_deadline(0), None);
        m.host_out(&mut h.cx(), 1, &[5, 6, 7, 8]).unwrap();
        assert_eq!(m.next_deadline(0), Some(4));
        h.now = 3;
        m.advance(&mut h.cx());
        assert_eq!(&h.bytes[0x1000..0x1006], &[0; 6]);
        h.now = 4;
        m.advance(&mut h.cx());
        assert_eq!(&h.bytes[0x1000..0x1006], &[1, 2, 3, 4, 5, 6]);
        let wrote = h
            .effects
            .iter()
            .position(|e| *e == Effect::Write(0x1000, 6))
            .unwrap();
        let irq = h
            .effects
            .iter()
            .position(|e| *e == Effect::Irq(85, true))
            .unwrap();
        assert!(wrote < irq);
        assert_eq!(read(&mut m, &mut h, CORE + 0x194, 2), 2);
        assert_eq!(read(&mut m, &mut h, CORE + 0x190, 4), 0x0807);
        write(&mut m, &mut h, PHY + 0x844, 4, 1);
        assert_eq!(h.effects.last(), Some(&Effect::Irq(85, false)));
    }
    #[test]
    fn in_dma_packets_reset_cancellation_and_fresh_lengths() {
        let mut m = model();
        let mut h = Ram::new(0x10000);
        h.bytes[0x1000..0x1080].fill(0x5a);
        arm(&mut m, &mut h, true, 128, 100);
        assert!(m.host_in(&mut h.cx(), 0x82, 64).is_err());
        h.now = 4;
        m.advance(&mut h.cx());
        assert_eq!(m.host_in(&mut h.cx(), 0x82, 64).unwrap(), [0x5a; 64]);
        assert_eq!(m.host_in(&mut h.cx(), 0x82, 64).unwrap(), [0x5a; 36]);
        write(&mut m, &mut h, CORE + 0x110c, 4, 1);
        assert!(m.active.is_none());
        write(&mut m, &mut h, CORE + 0x1124, 4, 100);
        assert!(m.active.is_some());
        write(&mut m, &mut h, PHY + 0x804, 4, 0x10000);
        assert_eq!(m.next_deadline(4), None);
        h.now = 100;
        m.advance(&mut h.cx());
        assert!(m.host_in(&mut h.cx(), 0x82, 64).is_err());
    }
    #[test]
    fn unread_status_packets_fill_port_two_then_drop_while_control_refuses() {
        let mut device = model();
        let mut host = Ram::new(0x10000);
        for _ in 0..80 {
            write(&mut device, &mut host, CORE + 0x111c, 4, 0xc0000);
            write(&mut device, &mut host, CORE + 0x18c + 16, 4, 0x0201);
            write(&mut device, &mut host, CORE + 0x111c, 4, 0x12);
            write(&mut device, &mut host, CORE + 0x88, 2, 4);
        }
        assert_eq!(device.tx[2].packets.len(), 65);
        assert_eq!(device.dropped_status, 15);
        assert_eq!(device.host_in(&mut host.cx(), 0x83, 64).unwrap(), vec![1, 2]);
        device.tx[0].bytes.extend(vec![0; 4096]);
        device.tx[0].packets.push_back(4096);
        write(&mut device, &mut host, CORE + 0x111c, 4, 0xe0000);
        write(&mut device, &mut host, CORE + 0x18c, 4, 7);
        write(&mut device, &mut host, CORE + 0x111c, 4, 0x11);
        let mut cx = host.cx();
        device.write(&mut cx, CORE + 0x88, 2, 1);
        assert!(cx.refusal().is_some());
    }

    #[test]
    fn checkpoint_pending_in_dma_restores_packet_queue_capacity_and_wire_bytes() {
        let mut device = model();
        let mut host = Ram::new(0x10000);
        host.bytes[0x1000..0x1006].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        arm(&mut device, &mut host, true, 8, 6);
        let saved = device.checkpoint().unwrap();
        let mut state = Vec::new();
        ciborium::into_writer(&saved, &mut state).unwrap();
        host.now = 4;
        device.advance(&mut host.cx());
        let expected = device.host_in(&mut host.cx(), 0x82, 64).unwrap();
        device.restore_checkpoint(&saved).unwrap();
        let mut restored = Vec::new();
        ciborium::into_writer(&device.checkpoint().unwrap(), &mut restored).unwrap();
        assert_eq!(restored, state);
        host.now = 3;
        device.advance(&mut host.cx());
        assert!(device.host_in(&mut host.cx(), 0x82, 64).is_err());
        host.now = 4;
        device.advance(&mut host.cx());
        assert_eq!(device.host_in(&mut host.cx(), 0x82, 64).unwrap(), expected);
    }
}
