//! XP4 serial-flash controller and request-paced DMA. Controller, attached
//! flash bytes, DMA and IRQ routing are separate inputs.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
mod checkpoint;

pub const CONTROLLER: u64 = 0xfffd8000;
pub const DMA: u64 = 0xfffe0000;
pub const FLASH: u64 = 0xf0000000;
pub const FLASH_SIZE: u64 = 0x08000000;

#[derive(Clone, Copy)]
pub struct Geometry {
    pub erase_bytes: usize,
    pub page_bytes: usize,
    pub erase_ticks: u64,
    pub page_ticks: u64,
}
enum Mutation {
    Erase { address: u64, size: usize },
    Program { address: u64, bytes: Vec<u8> },
}

/// External NOR contents and identity, supplied by the board. Uninitialized
/// bytes are erased. No firmware structures or executable signatures enter it.
pub struct Nor {
    bytes: Vec<u8>,
    pub jedec: u32,
    pub write_enable: bool,
    geometry: Geometry,
    overlay: BTreeMap<usize, Vec<u8>>,
    pending: Option<(u64, Mutation)>,
}
impl Nor {
    pub fn new(bytes: Vec<u8>, jedec: u32, geometry: Geometry) -> Result<Self, String> {
        if bytes.len() as u64 > FLASH_SIZE
            || jedec > 0xffffff
            || !geometry.erase_bytes.is_power_of_two()
            || !geometry.page_bytes.is_power_of_two()
            || geometry.page_bytes > geometry.erase_bytes
            || geometry.page_bytes > 4096
            || geometry.erase_bytes as u64 > FLASH_SIZE
            || geometry.erase_ticks == 0
            || geometry.page_ticks == 0
        {
            return Err("Invalid XP4 NOR input".into());
        }
        Ok(Self {
            bytes,
            jedec,
            write_enable: false,
            geometry,
            overlay: BTreeMap::new(),
            pending: None,
        })
    }
    fn read(&self, address: u64, size: usize) -> Result<Vec<u8>, String> {
        let offset = address
            .checked_sub(FLASH)
            .ok_or("NOR address below aperture")?;
        if offset
            .checked_add(size as u64)
            .is_none_or(|end| end > FLASH_SIZE)
        {
            return Err("NOR extent outside aperture".into());
        }
        let mut out = vec![0xff; size];
        let start = offset as usize;
        if start < self.bytes.len() {
            let n = size.min(self.bytes.len() - start);
            out[..n].copy_from_slice(&self.bytes[start..start + n]);
        }
        let page = self.geometry.page_bytes;
        for (&index, bytes) in self
            .overlay
            .range(start / page..=(start + size.saturating_sub(1)) / page)
        {
            let lo = start.max(index * page);
            let hi = (start + size).min((index + 1) * page);
            if lo < hi {
                out[lo - start..hi - start]
                    .copy_from_slice(&bytes[lo - index * page..hi - index * page]);
            }
        }
        Ok(out)
    }
    fn status(&self) -> u32 {
        u32::from(self.pending.is_some()) | (u32::from(self.write_enable) << 1)
    }
    fn begin(&mut self, now: u64, mutation: Mutation) -> Result<(), String> {
        if self.pending.is_some() || !self.write_enable {
            return Err("NOR mutation while busy or write-disabled".into());
        }
        let (address, size, ticks) = match &mutation {
            Mutation::Erase { address, size } => (*address, *size, self.geometry.erase_ticks),
            Mutation::Program { address, bytes } => {
                if (address.wrapping_sub(FLASH) as usize % self.geometry.page_bytes) + bytes.len()
                    > self.geometry.page_bytes
                {
                    return Err("NOR program crosses a declared page boundary".into());
                }
                (*address, bytes.len(), self.geometry.page_ticks)
            }
        };
        self.read(address, size)?;
        if size == 0 {
            return Err("Empty NOR mutation".into());
        }
        self.write_enable = false;
        self.pending = Some((now + ticks, mutation));
        Ok(())
    }
    fn complete(&mut self, cx: &mut Cx<'_>) {
        if !self.pending.as_ref().is_some_and(|p| p.0 <= cx.now()) {
            return;
        }
        let (at, op) = self.pending.take().unwrap();
        let (address, size, name) = match op {
            Mutation::Erase { address, size } => {
                let start = (address - FLASH) as usize;
                let page = self.geometry.page_bytes;
                for index in start / page..(start + size) / page {
                    self.overlay.remove(&index);
                    if self
                        .bytes
                        .get(index * page..((index + 1) * page).min(self.bytes.len()))
                        .is_some_and(|b| b.iter().any(|&v| v != 255))
                    {
                        self.overlay.insert(index, vec![255; page]);
                    }
                }
                (address, size, "XP4 NOR erase complete")
            }
            Mutation::Program { address, bytes } => {
                let mut page = self
                    .read(
                        FLASH
                            + ((address - FLASH) / self.geometry.page_bytes as u64)
                                * self.geometry.page_bytes as u64,
                        self.geometry.page_bytes,
                    )
                    .unwrap();
                let start = (address - FLASH) as usize % self.geometry.page_bytes;
                for (old, new) in page[start..start + bytes.len()].iter_mut().zip(&bytes) {
                    *old &= *new;
                }
                self.overlay
                    .insert((address - FLASH) as usize / self.geometry.page_bytes, page);
                (address, bytes.len(), "XP4 NOR program complete")
            }
        };
        cx.record(Record::new(
            name,
            [
                ("address", Field::int(address)),
                ("size", Field::int(size as u64)),
                ("tick", Field::int(at)),
            ],
        ));
    }
    fn snapshot(&self) -> Value {
        let pending = self.pending.as_ref().map(|(at, m)| match m {
            Mutation::Erase { address, size } => {
                json!({"at":at,"erase_address":address,"size":size})
            }
            Mutation::Program { address, bytes } => {
                json!({"at":at,"program_address":address,"bytes":bytes})
            }
        });
        json!({"jedec":self.jedec,"write_enable":self.write_enable,"pending":pending,
            "erase_bytes":self.geometry.erase_bytes,"page_bytes":self.geometry.page_bytes,
            "erase_ticks":self.geometry.erase_ticks,"page_ticks":self.geometry.page_ticks,
            "overlay":self.overlay})
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    address: u64,
    size: usize,
    accepted: bool,
    accepted_at: u64,
    write: bool,
}
pub struct Port {
    nor: Nor,
    request: Option<Request>,
    program_data: Option<(u64, Vec<u8>)>,
}
pub type Shared = Arc<Mutex<Port>>;
pub fn port(bytes: Vec<u8>, jedec: u32, geometry: Geometry) -> Result<Shared, String> {
    Ok(Arc::new(Mutex::new(Port {
        nor: Nor::new(bytes, jedec, geometry)?,
        request: None,
        program_data: None,
    })))
}

pub struct SerialFlashController {
    port: Shared,
    regs: [u32; 64],
    due: Option<(u64, u64)>,
    operations: u64,
    irq: u32,
    irq_level: bool,
}
impl SerialFlashController {
    pub fn new(port: Shared, irq: u32) -> Self {
        Self {
            port,
            regs: [0; 64],
            due: None,
            operations: 0,
            irq,
            irq_level: false,
        }
    }
    fn drive_irq(&mut self, cx: &mut Cx<'_>, level: bool) {
        if level != self.irq_level {
            self.irq_level = level;
            cx.set_irq(self.irq, level);
        }
    }
    fn known(o: usize) -> bool {
        matches!(
            o,
            0 | 4
                | 8
                | 0xc
                | 0x10
                | 0x14
                | 0x18
                | 0x1c
                | 0x20
                | 0x38
                | 0x40
                | 0x44
                | 0x60
                | 0x64
                | 0x68
                | 0x6c
                | 0x70
                | 0x74
                | 0x78
                | 0x7c
                | 0x80
                | 0x90
                | 0x94
                | 0x98
                | 0x9c
                | 0xa0
                | 0xa4
                | 0xa8
                | 0xb4
                | 0xb8
                | 0xbc
                | 0xe0
                | 0xf0
        )
    }
    fn access(
        &mut self,
        cx: &mut Cx<'_>,
        address: u64,
        size: u8,
        write: bool,
        value: u32,
    ) -> Result<u64, String> {
        if (FLASH..FLASH + FLASH_SIZE).contains(&address) {
            if write {
                return Err("CPU store to NOR aperture without a program command".into());
            }
            let bytes = self.port.lock().unwrap().nor.read(address, size as usize)?;
            return Ok(bytes
                .iter()
                .enumerate()
                .fold(0, |v, (n, b)| v | u64::from(*b) << (8 * n)));
        }
        let o = address.wrapping_sub(CONTROLLER) as usize;
        if size != 4 || o & 3 != 0 || !Self::known(o) {
            cx.unmodeled();
            return Ok(0);
        }
        if !write {
            return Ok(self.regs[o / 4] as u64);
        }
        match o {
            0 => self.regs[0] = (value & !0x80000000) | (self.regs[0] & 0x80000000),
            0x40 | 0xa0 => return Err("Write to serial-flash receive/status register".into()),
            0x90 if value & 1 != 0 => {
                if self.due.is_some() {
                    return Err("Serial-flash command while busy".into());
                }
                let command = (value >> 24) as u8;
                if !matches!(
                    command,
                    0x9f | 0x05 | 0x06 | 0x04 | 0x03 | 0x13 | 0x0b | 0x0c | 0xdc | 0x21
                ) {
                    return Err(format!("Unsupported XP4 flash command {command:#x}"));
                }
                if command != 0x05 && self.port.lock().unwrap().nor.pending.is_some() {
                    return Err("NOR command while array is busy".into());
                }
                self.regs[o / 4] = value | 2;
                self.regs[0] &= !0x80000000;
                self.regs[0x40 / 4] = 0;
                self.due = Some((cx.now() + 8, 1));
                cx.schedule(cx.now() + 8, 1);
            }
            0x60 if value & 0x20 != 0 => {
                self.regs[o / 4] &= !0x20;
            }
            0x60 if value & 1 != 0 => {
                if self.due.is_some() || self.port.lock().unwrap().request.is_some() {
                    return Err("Serial-flash read while busy".into());
                }
                let command = self.regs[0x20 / 4];
                if !matches!(command, 0x606 | 0x707) {
                    return Err(format!("Unsupported XP4 mapped-read command {command:#x}"));
                }
                let address = u64::from(self.regs[0x68 / 4]);
                let size = self.regs[0x6c / 4] as usize;
                if self.port.lock().unwrap().nor.pending.is_some() {
                    return Err("Mapped NOR read while busy".into());
                }
                self.port.lock().unwrap().nor.read(address, size)?;
                self.port.lock().unwrap().request = Some(Request {
                    address,
                    size,
                    accepted: false,
                    accepted_at: 0,
                    write: false,
                });
                self.regs[o / 4] = value & !0x20;
                self.due = Some((cx.now() + 8, 2));
                cx.schedule(cx.now() + 8, 2);
            }
            0x70 if value & 0x20 != 0 => self.regs[o / 4] &= !0x20,
            0x70 if value & 1 != 0 => {
                if self.due.is_some() || self.regs[0x20 / 4] != 0x202 {
                    return Err("Unsupported or busy XP4 program request".into());
                }
                let address = u64::from(self.regs[0x78 / 4]);
                let size = self.regs[0x7c / 4] as usize;
                let mut port = self.port.lock().unwrap();
                if port.nor.pending.is_some() || !port.nor.write_enable || port.request.is_some() {
                    return Err("NOR stream program while busy or write-disabled".into());
                }
                port.nor.read(address, size)?;
                port.request = Some(Request {
                    address,
                    size,
                    accepted: false,
                    accepted_at: 0,
                    write: true,
                });
                self.regs[o / 4] = value & !0x20;
                self.due = Some((cx.now() + 8, 4));
                cx.schedule(cx.now() + 8, 4);
            }
            _ => self.regs[o / 4] = value,
        }
        // +44 rearms future completions; enabling it must not replay an old
        // completed command. Disabling it acknowledges the live IRQ latch.
        if self.regs[0x44 / 4] & 0x4000 == 0 || self.regs[0x40 / 4] & 0x4000 == 0 {
            self.drive_irq(cx, false);
        }
        Ok(0)
    }
}
impl Device for SerialFlashController {
    fn name(&self) -> &'static str {
        "xp4-serial-flash"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.access(cx, address, size, false, 0) {
            Ok(v) => v,
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if let Err(e) = self.access(cx, address, size, true, value as u32) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let port = self.port.lock().unwrap();
        self.due
            .map(|x| x.0)
            .into_iter()
            .chain(port.nor.pending.as_ref().map(|p| p.0))
            .chain(port.program_data.as_ref().map(|_| now))
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut port = self.port.lock().unwrap();
        port.nor.complete(cx);
        if let Some((arrived, bytes)) = port.program_data.take() {
            let Some(request) = port
                .request
                .take()
                .filter(|r| r.write && r.accepted && r.size == bytes.len())
            else {
                cx.fail("Program FIFO has no matching controller request");
                return;
            };
            if let Err(e) = port.nor.begin(
                arrived,
                Mutation::Program {
                    address: request.address,
                    bytes,
                },
            ) {
                cx.fail(e);
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if !self.due.is_some_and(|(at, t)| at <= cx.now() && t == token) {
            return;
        }
        let accepted_at = self.due.unwrap().0;
        self.due = None;
        self.operations += 1;
        if token == 1 {
            let mut port = self.port.lock().unwrap();
            self.regs[0xa0 / 4] = match (self.regs[0x90 / 4] >> 24) as u8 {
                0x9f => {
                    let count = ((self.regs[0x90 / 4] >> 20) & 7) + 1;
                    if count > 3 {
                        cx.fail("XP4 JEDEC read exceeds three identity bytes");
                        return;
                    }
                    port.nor.jedec & (u32::MAX >> (32 - count * 8))
                }
                0x05 => port.nor.status(),
                0x06 => {
                    port.nor.write_enable = true;
                    0
                }
                0x04 => {
                    port.nor.write_enable = false;
                    0
                }
                command @ (0xdc | 0x21) => {
                    let control = self.regs[0x90 / 4];
                    if control & 0xb0000 != 0xb0000 {
                        cx.fail("NOR erase requires the four-byte address phase");
                        return;
                    }
                    let size = if command == 0xdc {
                        port.nor.geometry.erase_bytes
                    } else {
                        4096
                    };
                    let address = FLASH
                        + u64::from(self.regs[0x94 / 4]).wrapping_sub(FLASH) / size as u64
                            * size as u64;
                    if let Err(e) = port
                        .nor
                        .begin(accepted_at, Mutation::Erase { address, size })
                    {
                        cx.fail(e);
                        return;
                    }
                    0
                }
                _ => {
                    cx.fail("XP4 PIO flash data-address protocol is not modeled");
                    return;
                }
            };
            self.regs[0x90 / 4] &= !3;
            self.regs[0] |= 0x80000000;
            self.regs[0x40 / 4] |= 0x4000;
        } else if let Some(request) = &mut self.port.lock().unwrap().request {
            request.accepted = true;
            request.accepted_at = accepted_at;
            let o = if token == 4 { 0x70 } else { 0x60 };
            self.regs[o / 4] = (self.regs[o / 4] & !1) | 0x20;
        }
        if token == 1 {
            self.drive_irq(cx, self.regs[0x44 / 4] & 0x4000 != 0);
        }
    }
    fn save(&self) -> Value {
        let port = self.port.lock().unwrap();
        json!({"registers":self.regs.to_vec(),"next":self.due,"operations":self.operations,
            "jedec":port.nor.jedec,"nor":port.nor.snapshot(),"stream":port.request,
            "program_fifo":port.program_data,"irq":self.irq,"irq_level":self.irq_level})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.restore_state(value)
    }
}

#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Channel {
    #[serde(rename = "registers")]
    regs: [u32; 14],
    #[serde(rename = "next")]
    due: Option<u64>,
    submitted: bool,
    completed: u64,
    submitted_at: u64,
}
pub struct DmaController {
    port: Shared,
    channels: [Channel; 16],
    routes: [u32; 16],
    mux: BTreeMap<u64, u32>,
    mux_changed: [u64; 16],
}
impl DmaController {
    pub fn new(port: Shared, routes: [u32; 16]) -> Self {
        Self {
            port,
            channels: Default::default(),
            routes,
            mux: BTreeMap::new(),
            mux_changed: [0; 16],
        }
    }
    fn ready_time(&self, n: usize) -> Option<u64> {
        let c = &self.channels[n];
        if !c.submitted || c.due.is_some() {
            return None;
        }
        let src = u64::from(c.regs[0x14 / 4]);
        let dst = u64::from(c.regs[0x18 / 4]);
        let write = (FLASH..FLASH + FLASH_SIZE).contains(&dst);
        if !(FLASH..FLASH + FLASH_SIZE).contains(&src) && !write {
            return Some(c.submitted_at);
        }
        let mux = *self
            .mux
            .get(&(0xfff4a500 + (n / 2) as u64 * 4))
            .unwrap_or(&0);
        if (mux >> ((n % 2) * 8)) & 63 != if write { 43 } else { 42 } {
            return None;
        }
        self.port
            .lock()
            .unwrap()
            .request
            .as_ref()
            .filter(|r| {
                r.accepted
                    && r.write == write
                    && r.address == if write { dst } else { src }
                    && r.size == c.regs[0x10 / 4] as usize + 1
            })
            .map(|r| c.submitted_at.max(r.accepted_at).max(self.mux_changed[n]))
    }
    fn access(
        &mut self,
        cx: &mut Cx<'_>,
        address: u64,
        size: u8,
        write: bool,
        value: u32,
    ) -> Result<u64, String> {
        if size != 4 || address & 3 != 0 {
            cx.unmodeled();
            return Ok(0);
        }
        if (0xfff4a500..0xfff4a520).contains(&address) {
            if write {
                if value & !0x01013f3f != 0 {
                    cx.unmodeled();
                    return Ok(0);
                }
                let word = self.mux.entry(address).or_default();
                // 107bb3c writes both selector fields but strobes only the
                // selected channel's lane. The unstrobed neighbor stays live.
                let first = ((address - 0xfff4a500) / 4 * 2) as usize;
                for (lane, (strobe, shift)) in
                    [(0x10000, 0), (0x1000000, 8)].into_iter().enumerate()
                {
                    if value & strobe != 0 {
                        if (*word ^ value) & (63 << shift) != 0 {
                            self.mux_changed[first + lane] = cx.now();
                        }
                        *word = (*word & !(63 << shift)) | (value & (63 << shift));
                    }
                }
            }
            return Ok(u64::from(*self.mux.get(&address).unwrap_or(&0)));
        }
        let rel = address.wrapping_sub(DMA);
        let n = (rel / 0x1000) as usize;
        let o = (rel % 0x1000) as usize;
        if n >= 16
            || !matches!(
                o,
                0 | 0x10 | 0x14 | 0x18 | 0x1c | 0x20 | 0x24 | 0x28 | 0x2c | 0x30 | 0x34
            )
        {
            cx.unmodeled();
            return Ok(0);
        }
        let c = &mut self.channels[n];
        if !write {
            return Ok(c.regs[o / 4] as u64);
        }
        match o {
            0x28 => {
                c.regs[o / 4] = value;
                if value & 0x10000001 == 0x10000001 {
                    if c.submitted {
                        return Err("XP4 DMA channel start while busy".into());
                    }
                    let count = u64::from(c.regs[0x10 / 4]) + 1;
                    if count > FLASH_SIZE || c.regs[0x24 / 4] != 0 {
                        return Err("Unsupported XP4 DMA extent/repeat".into());
                    }
                    c.regs[0x30 / 4] = 0x10000;
                    c.submitted = true;
                    c.submitted_at = cx.now();
                } else if value & 0x10000000 == 0 {
                    c.submitted = false;
                    c.due = None;
                    c.regs[0x30 / 4] &= !0x10000;
                }
            }
            0x30 => {
                c.regs[o / 4] = 0;
                cx.set_irq(self.routes[n], false);
            }
            _ => {
                if c.submitted {
                    return Err("XP4 DMA reconfiguration while active".into());
                }
                c.regs[o / 4] = value;
            }
        }
        Ok(0)
    }
    fn complete(&mut self, cx: &mut Cx<'_>, n: usize) -> Result<(), String> {
        let c = &mut self.channels[n];
        let src = u64::from(c.regs[0x14 / 4]);
        let dst = u64::from(c.regs[0x18 / 4]);
        let size = c.regs[0x10 / 4] as usize + 1;
        let completed_at = c.due.unwrap();
        let program = (FLASH..FLASH + FLASH_SIZE).contains(&dst);
        if c.regs[0x20 / 4] & 4 != 0 && !program {
            return Err("XP4 fixed-destination DMA is not modeled".into());
        }
        let data = if (FLASH..FLASH + FLASH_SIZE).contains(&src) {
            let port = self.port.lock().unwrap();
            let r = port
                .request
                .as_ref()
                .ok_or("DMA without serial-flash request")?;
            if !r.accepted || r.write || r.address != src || r.size != size {
                return Err("DMA differs from serial-flash request".into());
            }
            port.nor.read(src, size)?
        } else if c.regs[0x1c / 4] & 4 != 0 {
            let width = 1usize << ((c.regs[0x1c / 4] >> 16) & 3);
            if !size.is_multiple_of(width) {
                return Err("XP4 DMA count is not a multiple of transfer width".into());
            }
            cx.ram_bytes(src, width)?
                .iter()
                .copied()
                .cycle()
                .take(size)
                .collect()
        } else {
            cx.ram_bytes(src, size)?
        };
        if program {
            let mut port = self.port.lock().unwrap();
            if c.regs[0x20 / 4] & 4 == 0
                || port.program_data.is_some()
                || !port
                    .request
                    .as_ref()
                    .is_some_and(|r| r.write && r.accepted && r.address == dst && r.size == size)
            {
                return Err("DMA differs from the program FIFO request".into());
            }
            port.program_data = Some((completed_at, data));
        } else {
            cx.ram_write(dst, &data)?;
        }
        if (FLASH..FLASH + FLASH_SIZE).contains(&src) {
            self.port.lock().unwrap().request = None;
        }
        if c.regs[0x1c / 4] & 4 == 0 {
            c.regs[0x14 / 4] = c.regs[0x14 / 4].wrapping_add(size as u32);
        }
        if c.regs[0x20 / 4] & 4 == 0 {
            c.regs[0x18 / 4] = c.regs[0x18 / 4].wrapping_add(size as u32);
        }
        c.regs[0x30 / 4] = 1;
        c.regs[0x28 / 4] &= !0x10000000;
        c.submitted = false;
        c.due = None;
        c.completed += 1;
        // 107bb3c selects 0x01100001 for software request 44 and
        // 0x00201011/0x00200011 for peripheral requests. Bit4 is absent
        // from software fills, whose callbacks use the same physical IRQ.
        // Completion status remains asserted until the guest ACKs +30.
        cx.set_irq(self.routes[n], true);
        cx.record(Record::new(
            "XP4 DMA complete",
            [
                ("channel", Field::int(n as u64)),
                ("source", Field::int(src)),
                ("destination", Field::int(dst)),
                ("size", Field::int(size as u64)),
                ("irq", Field::int(self.routes[n])),
                ("tick", Field::int(completed_at)),
            ],
        ));
        Ok(())
    }
}
impl Device for DmaController {
    fn name(&self) -> &'static str {
        "xp4-dma"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.access(cx, address, size, false, 0) {
            Ok(v) => v,
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if let Err(e) = self.access(cx, address, size, true, value as u32) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        (0..16)
            .filter_map(|n| {
                self.channels[n]
                    .due
                    .or_else(|| self.ready_time(n).map(|_| now))
            })
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        for n in 0..16 {
            if let Some(started) = self.ready_time(n) {
                let at =
                    started + 16 + (u64::from(self.channels[n].regs[0x10 / 4]) + 1).div_ceil(16);
                self.channels[n].due = Some(at);
                cx.schedule(at, n as u64);
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        let n = token as usize;
        if n < 16
            && self.channels[n].submitted
            && self.channels[n].due.is_some_and(|at| at <= cx.now())
        {
            if let Err(e) = self.complete(cx, n) {
                cx.fail(e);
            }
        }
    }
    fn save(&self) -> Value {
        json!({"channels":self.channels.iter().map(|c|json!({"registers":c.regs.to_vec(),"next":c.due,"submitted":c.submitted,"submitted_at":c.submitted_at,"completed":c.completed})).collect::<Vec<_>>(),"irq_routes":self.routes,"request_mux":self.mux,"request_mux_changed":self.mux_changed})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.restore_state(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    struct TestHost {
        now: u64,
        ram: Vec<u8>,
        irqs: Vec<(u32, bool)>,
        events: Vec<(usize, u64, u64)>,
        records: Vec<Record>,
    }
    impl Host for TestHost {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, d: usize, t: u64, k: u64) {
            self.events.push((d, t, k));
        }
        fn set_irq(&mut self, l: u32, v: bool) {
            self.irqs.push((l, v));
        }
        fn raise_irq(&mut self, _c: usize, _l: u32) -> Result<(), String> {
            panic!("GIC uses physical levels")
        }
        fn ram_read(&self, a: u64, b: &mut [u8]) -> Result<(), String> {
            b.copy_from_slice(
                self.ram
                    .get(a as usize..a as usize + b.len())
                    .ok_or("DMA read bounds")?,
            );
            Ok(())
        }
        fn ram_write(&mut self, a: u64, b: &[u8]) -> Result<(), String> {
            self.ram
                .get_mut(a as usize..a as usize + b.len())
                .ok_or("DMA write bounds")?
                .copy_from_slice(b);
            Ok(())
        }
        fn record(&mut self, r: Record) {
            self.records.push(r);
        }
    }
    fn write(d: &mut dyn Device, h: &mut TestHost, id: usize, a: u64, v: u64, pc: u64) {
        let mut cx = Cx::new(h, 3, pc, id);
        d.write(&mut cx, a, 4, v);
        assert_eq!(cx.refusal(), None);
    }
    fn event(d: &mut dyn Device, h: &mut TestHost, id: usize, token: u64) {
        let mut cx = Cx::new(h, 0, 0, id);
        d.event(&mut cx, token);
        assert_eq!(cx.refusal(), None);
    }
    #[test]
    fn route_strobes_preserve_peer_and_late_selection_starts_a_full_transfer() {
        let image: Vec<u8> = (0..32).collect();
        let p = port(
            image.clone(),
            0,
            Geometry {
                erase_bytes: 4096,
                page_bytes: 256,
                erase_ticks: 20,
                page_ticks: 12,
            },
        )
        .unwrap();
        let mut flash = SerialFlashController::new(p.clone(), 37);
        let mut dma = DmaController::new(p, std::array::from_fn(|n| 207 + n as u32));
        let mut h = TestHost {
            now: 100,
            ram: vec![0; 0x2000],
            irqs: vec![],
            events: vec![],
            records: vec![],
        };
        for (o, v) in [
            (0x14, FLASH),
            (0x18, 0x1000),
            (0x10, 31),
            (0x1c, 0x30f04),
            (0x20, 0x30f00),
            (0x24, 0),
            (0x28, 0x10201011),
        ] {
            write(&mut dma, &mut h, 1, DMA + o, v, 0);
        }
        for (o, v) in [(0x20, 0x707), (0x68, FLASH), (0x6c, 32), (0x60, 1)] {
            write(&mut flash, &mut h, 0, CONTROLLER + o, v, 0);
        }
        h.now = 108;
        event(&mut flash, &mut h, 0, 2);
        assert_eq!(dma.next_deadline(h.now), None); // No matching request lane yet.
        h.now = 200;
        write(&mut dma, &mut h, 1, 0xfff4a500, 0x12c2a, 0);
        assert_eq!(dma.next_deadline(h.now), Some(200));
        h.now = 201;
        write(&mut dma, &mut h, 1, 0xfff4a500, 0x1002c2c, 0xdeadbeef);
        h.now = 205;
        write(&mut dma, &mut h, 1, 0xfff4a500, 0x12c2a, 0); // Same lane value changes no condition.
        assert_eq!(dma.mux[&0xfff4a500], 0x2c2a);
        assert_eq!(&dma.mux_changed[..2], &[200, 201]);
        h.now = 206;
        dma.advance(&mut Cx::new(&mut h, 0, 0, 1));
        assert_eq!(dma.next_deadline(h.now), Some(218));
        let resumed_port = port(
            image.clone(),
            0,
            Geometry {
                erase_bytes: 4096,
                page_bytes: 256,
                erase_ticks: 20,
                page_ticks: 12,
            },
        )
        .unwrap();
        let mut resumed_flash = SerialFlashController::new(resumed_port.clone(), 37);
        let mut resumed_dma =
            DmaController::new(resumed_port, std::array::from_fn(|n| 207 + n as u32));
        resumed_flash.restore(&flash.save()).unwrap();
        resumed_dma.restore(&dma.save()).unwrap();
        assert_eq!(resumed_flash.save(), flash.save());
        assert_eq!(resumed_dma.save(), dma.save());
        let mut resumed_host = TestHost {
            now: 217,
            ram: h.ram.clone(),
            irqs: vec![],
            events: vec![],
            records: vec![],
        };
        let pending = resumed_dma.save();
        event(&mut resumed_dma, &mut resumed_host, 1, 0);
        assert_eq!(resumed_dma.save(), pending);
        resumed_host.now = 218;
        event(&mut resumed_dma, &mut resumed_host, 1, 0);
        h.now = 217;
        let before = dma.save();
        event(&mut dma, &mut h, 1, 0);
        assert_eq!(dma.save(), before);
        assert_eq!(h.ram[0x1000..0x1020], [0; 32]);
        h.now = 218;
        event(&mut dma, &mut h, 1, 0);
        assert_eq!(&h.ram[0x1000..0x1020], image);
        assert_eq!(h.irqs, [(207, true)]);
        assert_eq!(dma.next_deadline(h.now), None);
        assert_eq!(resumed_dma.save(), dma.save());
        assert_eq!(resumed_flash.save(), flash.save());
        assert_eq!(resumed_host.ram, h.ram);
        assert_eq!(resumed_host.irqs, h.irqs);
    }
    #[test]
    fn four_kib_erase_checkpoint_preserves_a_larger_physical_block_geometry() {
        let geometry = Geometry {
            erase_bytes: 131072,
            page_bytes: 256,
            erase_ticks: 20,
            page_ticks: 7,
        };
        let image = vec![0; geometry.erase_bytes];
        let source = port(image.clone(), 0x123456, geometry).unwrap();
        let mut flash = SerialFlashController::new(source.clone(), 37);
        let target = port(image, 0x123456, geometry).unwrap();
        let mut resumed = SerialFlashController::new(target.clone(), 37);
        let mut host = TestHost {
            now: 100,
            ram: vec![],
            irqs: vec![],
            events: vec![],
            records: vec![],
        };
        write(&mut flash, &mut host, 0, CONTROLLER + 0x90, 0x06000001, 0);
        host.now = 108;
        event(&mut flash, &mut host, 0, 1);
        write(
            &mut flash,
            &mut host,
            0,
            CONTROLLER + 0x94,
            FLASH + 0x1234,
            0,
        );
        write(&mut flash, &mut host, 0, CONTROLLER + 0x90, 0x210b0001, 0);
        host.now = 116;
        event(&mut flash, &mut host, 0, 1);
        assert_eq!(flash.next_deadline(host.now), Some(136));
        resumed.restore(&flash.save()).unwrap();
        assert_eq!(resumed.save(), flash.save());
        let before = resumed.save();
        host.now = 135;
        resumed.advance(&mut Cx::new(&mut host, 3, 0xdead0000, 0));
        assert_eq!(resumed.save(), before);
        assert_eq!(
            target
                .lock()
                .unwrap()
                .nor
                .read(FLASH + 0x1000, 4096)
                .unwrap(),
            vec![0; 4096]
        );
        host.now = 136;
        for model in [&mut flash, &mut resumed] {
            model.advance(&mut Cx::new(&mut host, 0, 0, 0));
        }
        assert_eq!(resumed.save(), flash.save());
        let bytes = target
            .lock()
            .unwrap()
            .nor
            .read(FLASH, geometry.erase_bytes)
            .unwrap();
        assert_eq!(&bytes[..0x1000], vec![0; 0x1000]);
        assert_eq!(&bytes[0x1000..0x2000], vec![255; 0x1000]);
        assert_eq!(&bytes[0x2000..], vec![0; geometry.erase_bytes - 0x2000]);

        let mut invalid = before;
        invalid["nor"]["pending"]["size"] = json!(8192);
        assert!(resumed.restore(&invalid).is_err());
    }
    #[test]
    fn nor_mutations_and_dirty_pages_survive_a_fresh_physical_port() {
        let geometry = Geometry {
            erase_bytes: 256,
            page_bytes: 16,
            erase_ticks: 20,
            page_ticks: 7,
        };
        let source = port(vec![255; 256], 0x123456, geometry).unwrap();
        let mut flash = SerialFlashController::new(source.clone(), 37);
        let target = port(vec![255; 256], 0x123456, geometry).unwrap();
        let mut resumed = SerialFlashController::new(target.clone(), 37);
        let mut host = TestHost {
            now: 106,
            ram: vec![],
            irqs: vec![],
            events: vec![],
            records: vec![],
        };
        source.lock().unwrap().nor.write_enable = true;
        source
            .lock()
            .unwrap()
            .nor
            .begin(
                100,
                Mutation::Program {
                    address: FLASH + 4,
                    bytes: vec![0x12, 0x34],
                },
            )
            .unwrap();
        resumed.restore(&flash.save()).unwrap();
        let before = resumed.save();
        resumed.advance(&mut Cx::new(&mut host, 3, 0xdead0000, 0));
        assert_eq!(resumed.save(), before);
        host.now = 107;
        for model in [&mut flash, &mut resumed] {
            model.advance(&mut Cx::new(&mut host, 0, 0, 0));
        }
        assert_eq!(flash.save(), resumed.save());
        assert_eq!(
            target.lock().unwrap().nor.read(FLASH + 4, 2).unwrap(),
            [0x12, 0x34]
        );
        source.lock().unwrap().nor.write_enable = true;
        source
            .lock()
            .unwrap()
            .nor
            .begin(
                110,
                Mutation::Erase {
                    address: FLASH,
                    size: 256,
                },
            )
            .unwrap();
        resumed.restore(&flash.save()).unwrap();
        host.now = 130;
        for model in [&mut flash, &mut resumed] {
            model.advance(&mut Cx::new(&mut host, 0, 0, 0));
        }
        assert_eq!(flash.save(), resumed.save());
        assert_eq!(
            target.lock().unwrap().nor.read(FLASH, 256).unwrap(),
            vec![255; 256]
        );
    }
    fn transfer(pc: u64) -> (Value, Value, Vec<u8>, Vec<(u32, bool)>, Vec<Record>) {
        let mut image = vec![0xff; 0x120];
        for (i, b) in image[0x100..].iter_mut().enumerate() {
            *b = i as u8;
        }
        let p = port(
            image,
            0x332211,
            Geometry {
                erase_bytes: 131072,
                page_bytes: 256,
                erase_ticks: 4096,
                page_ticks: 512,
            },
        )
        .unwrap();
        let mut flash = SerialFlashController::new(p.clone(), 37);
        let mut dma = DmaController::new(p, std::array::from_fn(|n| 207 + n as u32));
        let mut h = TestHost {
            now: 100,
            ram: vec![0; 0x2000],
            irqs: vec![],
            events: vec![],
            records: vec![],
        };
        write(&mut flash, &mut h, 0, CONTROLLER + 0x90, 0x9fa00001, pc);
        h.now = 107;
        let before = flash.save();
        event(&mut flash, &mut h, 0, 1);
        assert_eq!(flash.save(), before);
        h.now = 108;
        event(&mut flash, &mut h, 0, 1);
        assert_eq!(flash.regs[0xa0 / 4], 0x332211);
        assert_ne!(flash.regs[0] & 0x80000000, 0);
        for (o, v) in [
            (0x14, FLASH + 0x100),
            (0x18, 0x1000),
            (0x10, 31),
            (0x1c, 0x30f04),
            (0x20, 0x30f00),
            (0x24, 0),
            (0x28, 0x10201011),
        ] {
            write(&mut dma, &mut h, 1, DMA + o, v, pc);
        }
        write(&mut dma, &mut h, 1, 0xfff4a500, 0x12c2a, pc);
        assert_eq!(dma.next_deadline(h.now), None); // Guest-only condition, no polling deadline.
        for (o, v) in [(0x20, 0x707), (0x68, FLASH + 0x100), (0x6c, 32), (0x60, 1)] {
            write(&mut flash, &mut h, 0, CONTROLLER + o, v, pc);
        }
        h.now = 116;
        event(&mut flash, &mut h, 0, 2);
        assert_eq!(dma.next_deadline(h.now), Some(116));
        {
            let mut cx = Cx::new(&mut h, 0, 0, 1);
            dma.advance(&mut cx);
            assert_eq!(cx.refusal(), None);
        }
        assert_eq!(dma.next_deadline(h.now), Some(134));
        h.now = 133;
        let before = dma.save();
        event(&mut dma, &mut h, 1, 0);
        assert_eq!(dma.save(), before);
        assert_eq!(h.ram[0x1000], 0);
        h.now = 134;
        event(&mut dma, &mut h, 1, 0);
        assert_eq!(&h.ram[0x1000..0x1020], &(0u8..32).collect::<Vec<_>>());
        assert_eq!(h.irqs, vec![(207, true)]);
        assert_eq!(dma.next_deadline(h.now), None);
        let before = dma.save();
        assert_eq!(dma.save(), before);
        assert_eq!(h.irqs.len(), 1);
        write(&mut dma, &mut h, 1, DMA + 0x30, 0, pc);
        assert_eq!(h.irqs.last(), Some(&(207, false)));
        (flash.save(), dma.save(), h.ram, h.irqs, h.records)
    }
    #[test]
    fn real_buffers_deadlines_irq_and_observations_are_program_independent() {
        assert_eq!(transfer(0x1000), transfer(0xabcde0));
    }
    fn mutations(pc: u64) -> (Value, Vec<Record>) {
        let p = port(
            vec![0; 8192],
            0,
            Geometry {
                erase_bytes: 4096,
                page_bytes: 256,
                erase_ticks: 20,
                page_ticks: 12,
            },
        )
        .unwrap();
        let mut flash = SerialFlashController::new(p.clone(), 37);
        let mut dma = DmaController::new(p.clone(), std::array::from_fn(|n| 207 + n as u32));
        let mut h = TestHost {
            now: 100,
            ram: vec![0; 8192],
            irqs: vec![],
            events: vec![],
            records: vec![],
        };
        write(&mut flash, &mut h, 0, CONTROLLER + 0x44, 0x4000, pc);
        write(&mut flash, &mut h, 0, CONTROLLER + 0x90, 0x06000001, pc);
        h.now = 108;
        event(&mut flash, &mut h, 0, 1);
        assert_eq!(p.lock().unwrap().nor.status(), 2);
        assert_eq!(h.irqs, [(37, true)]);
        write(&mut flash, &mut h, 0, CONTROLLER + 0x44, 0, pc);
        assert_eq!(h.irqs, [(37, true), (37, false)]);
        write(&mut flash, &mut h, 0, CONTROLLER + 0x44, 0x4000, pc);
        assert_eq!(h.irqs, [(37, true), (37, false)]); // Rearm does not replay old completion.
        write(&mut flash, &mut h, 0, CONTROLLER + 0x44, 0, pc);
        write(&mut flash, &mut h, 0, CONTROLLER + 0x94, FLASH + 0x1011, pc);
        write(&mut flash, &mut h, 0, CONTROLLER + 0x90, 0xdc0b0001, pc);
        h.now = 116;
        event(&mut flash, &mut h, 0, 1);
        assert_eq!(p.lock().unwrap().nor.status(), 1);
        assert_eq!(flash.next_deadline(116), Some(136));
        let before = flash.save();
        h.now = 135;
        flash.advance(&mut Cx::new(&mut h, 0, pc, 0));
        assert_eq!(flash.save(), before);
        assert_eq!(
            p.lock().unwrap().nor.read(FLASH + 0x1000, 4).unwrap(),
            [0; 4]
        );
        h.now = 136;
        flash.advance(&mut Cx::new(&mut h, 0, pc, 0));
        assert_eq!(
            p.lock().unwrap().nor.read(FLASH + 0x1000, 4096).unwrap(),
            vec![255; 4096]
        );
        assert_eq!(p.lock().unwrap().nor.read(FLASH, 4).unwrap(), [0; 4]);
        for value in [0xf0, 0xff] {
            h.ram[0x1000..0x1020].fill(value);
            write(&mut flash, &mut h, 0, CONTROLLER + 0x90, 0x06000001, pc);
            h.now += 8;
            event(&mut flash, &mut h, 0, 1);
            for (o, v) in [
                (0x14, 0x1000),
                (0x18, FLASH + 0x1004),
                (0x10, 31),
                (0x1c, 0x30f00),
                (0x20, 0x30f04),
                (0x24, 0),
                (0x28, 0x10201011),
            ] {
                write(&mut dma, &mut h, 1, DMA + o, v, pc);
            }
            write(&mut dma, &mut h, 1, 0xfff4a500, 0x12c2b, pc);
            assert_eq!(dma.next_deadline(h.now), None);
            for (o, v) in [(0x20, 0x202), (0x78, FLASH + 0x1004), (0x7c, 32), (0x70, 1)] {
                write(&mut flash, &mut h, 0, CONTROLLER + o, v, pc);
            }
            h.now += 8;
            event(&mut flash, &mut h, 0, 4);
            dma.advance(&mut Cx::new(&mut h, 0, pc, 1));
            h.now += 18;
            event(&mut dma, &mut h, 1, 0);
            assert_eq!(flash.next_deadline(h.now), Some(h.now));
            flash.advance(&mut Cx::new(&mut h, 0, pc, 0));
            assert_eq!(p.lock().unwrap().nor.status(), 1);
            let before = flash.save();
            h.now += 11;
            flash.advance(&mut Cx::new(&mut h, 0, pc, 0));
            assert_eq!(flash.save(), before);
            h.now += 1;
            flash.advance(&mut Cx::new(&mut h, 0, pc, 0));
            assert_eq!(
                p.lock().unwrap().nor.read(FLASH + 0x1004, 32).unwrap(),
                vec![0xf0; 32]
            );
        }
        assert_eq!(p.lock().unwrap().nor.bytes, vec![0; 8192]); // Immutable supplied image.
        assert_eq!(p.lock().unwrap().nor.status(), 0);
        (flash.save(), h.records)
    }
    #[test]
    fn erase_and_dma_program_use_wel_wip_real_bytes_and_one_to_zero_bits() {
        assert_eq!(mutations(0x1234), mutations(0xabcdef));
    }
    fn program_timing(late: bool) -> (Value, Value, Vec<Record>) {
        let p = port(
            vec![255; 8192],
            0,
            Geometry {
                erase_bytes: 4096,
                page_bytes: 256,
                erase_ticks: 20,
                page_ticks: 12,
            },
        )
        .unwrap();
        let mut flash = SerialFlashController::new(p.clone(), 37);
        let mut dma = DmaController::new(p.clone(), std::array::from_fn(|n| 207 + n as u32));
        let mut h = TestHost {
            now: 100,
            ram: vec![0xa5; 8192],
            irqs: vec![],
            events: vec![],
            records: vec![],
        };
        write(&mut flash, &mut h, 0, CONTROLLER + 0x90, 0x06000001, 0);
        h.now = 108;
        event(&mut flash, &mut h, 0, 1);
        h.now = 120;
        for (o, v) in [
            (0x14, 0x1000),
            (0x18, FLASH + 0x1004),
            (0x10, 31),
            (0x1c, 0x30f00),
            (0x20, 0x30f04),
            (0x24, 0),
            (0x28, 0x10201011),
        ] {
            write(&mut dma, &mut h, 1, DMA + o, v, 0);
        }
        write(&mut dma, &mut h, 1, 0xfff4a500, 0x12c2b, 0);
        for (o, v) in [(0x20, 0x202), (0x78, FLASH + 0x1004), (0x7c, 32), (0x70, 1)] {
            write(&mut flash, &mut h, 0, CONTROLLER + o, v, 0);
        }
        h.now = if late { 140 } else { 128 };
        event(&mut flash, &mut h, 0, 4);
        dma.advance(&mut Cx::new(&mut h, 0, 0, 1));
        assert_eq!(dma.next_deadline(h.now), Some(146));
        h.now = if late { 160 } else { 146 };
        event(&mut dma, &mut h, 1, 0);
        flash.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(flash.next_deadline(h.now), Some(158));
        h.now = 160;
        flash.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(
            p.lock().unwrap().nor.read(FLASH + 0x1004, 32).unwrap(),
            vec![0xa5; 32]
        );
        (flash.save(), dma.save(), h.records)
    }
    #[test]
    fn peer_acceptance_dma_and_array_program_keep_their_deadlines_when_delivered_late() {
        assert_eq!(program_timing(false), program_timing(true));
    }
}
