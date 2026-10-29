//! X-Processor Pro flash controller and its memory-mapped aperture.
//!
//! The command port supplies timed, single-byte register transactions to an
//! explicitly attached NOR register device. With declared array timing, the
//! 4 KiB and 64 KiB erase commands and the page program (aperture stores while
//! the command registers hold a page program) change the array; other array
//! commands refuse. +0x30 bit 0 ends an aperture access: the X-T20 2.11 driver
//! sets it after DMA reads (0xc4ff48) and page stores (0xc506d4), and only
//! collected page data starts a program. The aperture reads attached array
//! contents, erased (0xff) past the supplied image.
use fujimod_emulator_devices::parts::micron_nor;
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Controller register window size in bytes.
pub const REGISTERS: u64 = 0x40;
/// Control-word bits a driver may carry over from the previous command: the
/// X-T20 2.11 driver writes `(old & 0xff00403f) | phase` (0xc51b28), so
/// command shapes are decoded from the remaining bits and the phase count.
const CARRIED: u32 = 0xff00403f;
/// Rewritten aperture bytes are kept in granules of the smallest erase.
const GRANULE: u64 = 0x1000;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub transfer_ticks: u64,
    /// Interrupt line driven while an unmasked status bit is pending.
    #[serde(default)]
    pub irq: Option<u32>,
    /// Array timing of the attached NOR devices; without it array commands
    /// refuse.
    #[serde(default)]
    pub array: Option<Array>,
}

/// Declared array geometry and synthetic latencies of attached NOR devices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Array {
    pub page_bytes: u64,
    pub erase_ticks: u64,
    pub page_ticks: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    due: u64,
    chip_select: u8,
    opcode: u8,
    transmit: Vec<u8>,
    receive: usize,
    /// Medium offset and size of an erase command.
    #[serde(default)]
    erase: Option<(u64, u64)>,
}

/// An array change that lands when the NOR device stops reporting busy.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArrayWrite {
    due: u64,
    offset: u64,
    /// Erased size, or the programmed bytes.
    erase: Option<u64>,
    program: Vec<u8>,
}

/// Passive union of every byte read through the aperture, including DMA reads.
/// Ranges are medium offsets with exclusive ends, independent of guest code.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadAudit {
    pub bytes: u64,
    pub ranges: Vec<(u64, u64)>,
}
impl ReadAudit {
    fn record(&mut self, mut start: u64, mut end: u64) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(end - start)
            .ok_or("X-Processor Pro flash read counter overflow")?;
        if start == end {
            return Ok(());
        }
        let first = self.ranges.partition_point(|&(_, hi)| hi < start);
        let mut last = first;
        while last < self.ranges.len() && self.ranges[last].0 <= end {
            start = start.min(self.ranges[last].0);
            end = end.max(self.ranges[last].1);
            last += 1;
        }
        self.ranges.splice(first..last, [(start, end)]);
        Ok(())
    }
}

/// Aperture state changed by array writes: rewritten granules and the page
/// program being collected from aperture stores.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contents {
    overlay: BTreeMap<u64, Vec<u8>>,
    /// Medium window and page size accepting stores, while armed.
    armed: Option<(u64, u64, u64)>,
    /// First medium offset and bytes stored for the armed page program.
    page: Option<(u64, Vec<u8>)>,
    /// Stores not yet reported through the controller status.
    unreported: bool,
}

/// Flash contents behind a physical window.
pub struct Aperture {
    base: u64,
    size: u64,
    image: Vec<u8>,
    reads: Mutex<ReadAudit>,
    contents: Mutex<Contents>,
}
impl Aperture {
    pub fn new(base: u64, size: u64, image: Vec<u8>) -> Result<Self, String> {
        if base & 0xfff != 0
            || size == 0
            || size % GRANULE != 0
            || image.len() as u64 > size
            || base.checked_add(size).is_none_or(|end| end > 1 << 32)
        {
            return Err("Invalid X-Processor Pro flash aperture".into());
        }
        Ok(Self {
            base,
            size,
            image,
            reads: Mutex::new(ReadAudit::default()),
            contents: Mutex::new(Contents::default()),
        })
    }
    pub fn base(&self) -> u64 {
        self.base
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn contains(&self, address: u64, len: usize) -> bool {
        address >= self.base
            && address
                .checked_add(len as u64)
                .is_some_and(|end| end <= self.base + self.size)
    }
    /// Current bytes at a medium offset, without auditing the read.
    fn bytes(&self, contents: &Contents, start: u64, len: u64) -> Vec<u8> {
        let mut out = vec![0xff; len as usize];
        if start < self.image.len() as u64 {
            let n = len.min(self.image.len() as u64 - start) as usize;
            out[..n].copy_from_slice(&self.image[start as usize..start as usize + n]);
        }
        if len > 0 {
            for (&index, bytes) in contents
                .overlay
                .range(start / GRANULE..=(start + len - 1) / GRANULE)
            {
                let lo = start.max(index * GRANULE);
                let hi = (start + len).min((index + 1) * GRANULE);
                out[(lo - start) as usize..(hi - start) as usize].copy_from_slice(
                    &bytes[(lo - index * GRANULE) as usize..(hi - index * GRANULE) as usize],
                );
            }
        }
        out
    }
    pub fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, String> {
        if !self.contains(address, len) {
            return Err("Flash read outside the aperture".into());
        }
        let start = address - self.base;
        let out = self.bytes(&self.contents.lock().unwrap(), start, len as u64);
        self.reads
            .lock()
            .unwrap()
            .record(start, start + len as u64)?;
        Ok(out)
    }
    /// Collects a store for the armed page program; refuses otherwise.
    pub fn store(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let mut contents = self.contents.lock().unwrap();
        let Some((lo, hi, page_bytes)) = contents.armed else {
            return Err("X-Processor Pro flash aperture store without a page program".into());
        };
        let start = address.wrapping_sub(self.base);
        let first = contents.page.as_ref().map_or(start, |&(first, _)| first);
        let collected = contents.page.as_ref().map_or(0, |(_, b)| b.len() as u64);
        if !self.contains(address, bytes.len())
            || start < lo
            || start + bytes.len() as u64 > hi
            || start != first + collected
            || first % page_bytes + collected + bytes.len() as u64 > page_bytes
        {
            return Err("Flash page program store outside one contiguous page".into());
        }
        contents
            .page
            .get_or_insert_with(|| (start, vec![]))
            .1
            .extend_from_slice(bytes);
        contents.unreported = true;
        Ok(())
    }
    /// Arms page-program stores for the medium window `[lo, hi)`, or disarms;
    /// collected bytes must have been taken first.
    fn arm(&self, armed: Option<(u64, u64, u64)>) -> Result<(), String> {
        let mut contents = self.contents.lock().unwrap();
        if contents.armed != armed && contents.page.is_some() {
            return Err("Serial-flash page program abandoned with collected data".into());
        }
        contents.armed = armed;
        Ok(())
    }
    fn take_page(&self) -> Option<(u64, Vec<u8>)> {
        self.contents.lock().unwrap().page.take()
    }
    fn take_unreported(&self) -> bool {
        std::mem::take(&mut self.contents.lock().unwrap().unreported)
    }
    /// Erases (sets) or programs (clears bits of) the medium at `offset`.
    fn apply(&self, offset: u64, erase: Option<u64>, program: &[u8]) {
        let mut contents = self.contents.lock().unwrap();
        let len = erase.unwrap_or(program.len() as u64);
        for index in offset / GRANULE..(offset + len).div_ceil(GRANULE) {
            let mut granule = self.bytes(&contents, index * GRANULE, GRANULE);
            let lo = offset.max(index * GRANULE);
            let hi = (offset + len).min((index + 1) * GRANULE);
            for at in lo..hi {
                let byte = &mut granule[(at - index * GRANULE) as usize];
                *byte = match erase {
                    Some(_) => 0xff,
                    None => *byte & program[(at - offset) as usize],
                };
            }
            let image = self.bytes(&Contents::default(), index * GRANULE, GRANULE);
            if granule == image {
                contents.overlay.remove(&index);
            } else {
                contents.overlay.insert(index, granule);
            }
        }
    }
    pub fn read_audit(&self) -> ReadAudit {
        self.reads.lock().unwrap().clone()
    }
    pub fn contents(&self) -> Contents {
        self.contents.lock().unwrap().clone()
    }
    fn restore_audit(&self, audit: ReadAudit) -> Result<(), String> {
        let mut previous = None;
        let mut unique = 0u64;
        for &(start, end) in &audit.ranges {
            if start >= end || end > self.size || previous.is_some_and(|p| p >= start) {
                return Err("Invalid X-Processor Pro flash read ranges".into());
            }
            unique += end - start;
            previous = Some(end);
        }
        if unique > audit.bytes || (audit.ranges.is_empty() && audit.bytes != 0) {
            return Err("Invalid X-Processor Pro flash read count".into());
        }
        *self.reads.lock().unwrap() = audit;
        Ok(())
    }
    fn restore_contents(&self, contents: Contents) -> Result<(), String> {
        let armed_ok = contents.armed.is_none_or(|(lo, hi, page)| {
            lo < hi && hi <= self.size && page.is_power_of_two() && page <= GRANULE
        });
        let page_ok = contents.page.as_ref().is_none_or(|(first, bytes)| {
            contents.armed.is_some_and(|(lo, hi, page)| {
                !bytes.is_empty()
                    && *first >= lo
                    && first + bytes.len() as u64 <= hi
                    && first % page + bytes.len() as u64 <= page
            })
        });
        if !armed_ok
            || !page_ok
            || (contents.unreported && contents.page.is_none())
            || contents.overlay.iter().any(|(&index, bytes)| {
                (index + 1) * GRANULE > self.size || bytes.len() as u64 != GRANULE
            })
        {
            return Err("Invalid X-Processor Pro flash contents".into());
        }
        *self.contents.lock().unwrap() = contents;
        Ok(())
    }
}

/// Erase opcodes of the Micron 4-byte-address command set and their sizes.
fn erase_bytes(opcode: u8) -> Option<u64> {
    match opcode {
        0x21 => Some(0x1000),
        0xdc => Some(0x10000),
        _ => None,
    }
}

pub struct FlashController {
    options: Options,
    aperture: Arc<Aperture>,
    registers: [u32; (REGISTERS / 4) as usize],
    nor: BTreeMap<u8, micron_nor::Registers>,
    pending: Option<Transaction>,
    received: Option<u8>,
    array_write: Option<(u8, ArrayWrite)>,
    asserted: bool,
}
impl FlashController {
    pub fn new(
        options: Options,
        aperture: Arc<Aperture>,
        nor: BTreeMap<u8, micron_nor::Registers>,
    ) -> Result<Self, String> {
        if options.base & 0xfff != 0
            || aperture.contains(options.base, REGISTERS as usize)
            || options.transfer_ticks == 0
            || nor.keys().any(|&chip_select| chip_select > 1)
            || options.irq.is_some_and(|line| !(32..1020).contains(&line))
            || options.array.as_ref().is_some_and(|a| {
                !a.page_bytes.is_power_of_two()
                    || a.page_bytes > GRANULE
                    || a.erase_ticks == 0
                    || a.page_ticks == 0
            })
        {
            return Err("Invalid X-Processor Pro flash controller base".into());
        }
        Ok(Self {
            options,
            aperture,
            registers: [0; (REGISTERS / 4) as usize],
            nor,
            pending: None,
            received: None,
            array_write: None,
            asserted: false,
        })
    }
    /// Medium window of a chip select: each takes half the aperture.
    fn window(&self, chip_select: u8) -> (u64, u64) {
        let half = self.aperture.size() / 2;
        (
            u64::from(chip_select) * half,
            u64::from(chip_select + 1) * half,
        )
    }
    fn chip_select(&self) -> Result<u8, String> {
        match (self.registers[0x10 / 4] >> 6) & 3 {
            1 => Ok(0),
            2 => Ok(1),
            _ => Err("Serial-flash command requires one chip select".into()),
        }
    }
    /// Shape of the command in the control word and phase count, by its
    /// opcode, address, receive and transmit phases.
    fn shape(&self) -> Option<(bool, usize, bool)> {
        match (
            self.registers[0x10 / 4] & !CARRIED & !0xc0,
            self.registers[0x20 / 4] & 0x7f,
        ) {
            (0x18000, 0x01) => Some((false, 0, false)),
            (0x18000, 0x03) => Some((true, 0, false)),
            (0x38000 | 0x39000, 0x21) => Some((false, 1, false)),
            (0x19000, 0x41) => Some((false, 0, true)),
            (0xd8000, 0x43) => Some((true, 0, true)),
            _ => None,
        }
    }
    /// Arms aperture stores while the command registers hold a page program
    /// for an attached array.
    fn arm(&self) -> Result<(), String> {
        let armed = match (self.shape(), self.chip_select(), &self.options.array) {
            (Some((true, 0, true)), Ok(chip_select), Some(array))
                if self.registers[0] == 0x12 && self.nor.contains_key(&chip_select) =>
            {
                let (lo, hi) = self.window(chip_select);
                Some((lo, hi, array.page_bytes))
            }
            _ => None,
        };
        self.aperture.arm(armed)
    }
    fn start(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.pending.is_some() || self.received.is_some() {
            return Err("Serial-flash command port has an unfinished transaction".into());
        }
        let chip_select = self.chip_select()?;
        if !self.nor.contains_key(&chip_select) {
            return Err(format!(
                "Serial-flash chip select {chip_select} has no attached register device"
            ));
        }
        let opcode = u8::try_from(self.registers[0]).map_err(|_| "Serial-flash opcode overflow")?;
        let (transmit, receive, erase) = match self.shape() {
            Some((false, receive, false)) => (vec![], receive, None),
            Some((false, 0, true)) => {
                let value = u8::try_from(self.registers[0x14 / 4])
                    .map_err(|_| "Serial-flash transmit byte overflow")?;
                (vec![value], 0, None)
            }
            Some((true, 0, false)) => {
                let size = erase_bytes(opcode)
                    .filter(|_| self.options.array.is_some())
                    .ok_or_else(|| {
                        format!("Unsupported serial-flash address command {opcode:#04x}")
                    })?;
                let (lo, hi) = self.window(chip_select);
                let offset = u64::from(self.registers[0x04 / 4])
                    .checked_sub(self.aperture.base())
                    .filter(|offset| (lo..hi).contains(offset))
                    .ok_or("Serial-flash erase address outside its chip select")?;
                (vec![], 0, Some((offset & !(size - 1), size)))
            }
            Some((true, 0, true)) => {
                return Err("Serial-flash page programs start with aperture stores".into());
            }
            _ => return Err("Unsupported serial-flash command phases".into()),
        };
        let due = cx
            .now()
            .checked_add(self.options.transfer_ticks)
            .ok_or("Serial-flash transfer deadline overflow")?;
        self.pending = Some(Transaction {
            due,
            chip_select,
            opcode,
            transmit,
            receive,
            erase,
        });
        self.registers[0x1c / 4] = 1;
        cx.schedule(due, 0);
        Ok(())
    }
    /// Ends an aperture access; a collected page program is then programmed
    /// by the device, which must be idle and write-enabled.
    fn end_access(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let Some((offset, program)) = self.aperture.take_page() else {
            return Ok(());
        };
        let chip_select = self.chip_select()?;
        let ticks = self.options.array.as_ref().map(|a| a.page_ticks).unwrap();
        self.begin_array_write(cx, chip_select, offset, None, program, ticks)
    }
    fn begin_array_write(
        &mut self,
        cx: &mut Cx<'_>,
        chip_select: u8,
        offset: u64,
        erase: Option<u64>,
        program: Vec<u8>,
        ticks: u64,
    ) -> Result<(), String> {
        if self.array_write.is_some() {
            return Err("Serial-flash array write while another is running".into());
        }
        let nor = self
            .nor
            .get_mut(&chip_select)
            .ok_or("Serial-flash array write without a register device")?;
        let due = nor.begin_array_write(cx.now(), ticks)?;
        self.array_write = Some((
            chip_select,
            ArrayWrite {
                due,
                offset,
                erase,
                program,
            },
        ));
        cx.schedule(due, 0);
        Ok(())
    }
    fn update_irq(&mut self, cx: &mut Cx<'_>) {
        let Some(line) = self.options.irq else {
            return;
        };
        let level = self.registers[0x24 / 4] & !self.registers[0x28 / 4] & 0x3f != 0;
        if level != self.asserted {
            self.asserted = level;
            cx.set_irq(line, level);
        }
    }
}
impl Device for FlashController {
    fn name(&self) -> &'static str {
        "xpp-flash"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        if self.aperture.contains(address, size as usize) {
            return match self.aperture.read(address, size as usize) {
                Ok(b) => b.iter().rev().fold(0, |v, &x| v << 8 | u64::from(x)),
                Err(e) => {
                    cx.fail(e);
                    0
                }
            };
        }
        if size != 4 || address & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        let offset = address - self.options.base;
        if offset == 0x18 {
            return match self.received.take() {
                Some(value) => value.into(),
                None => {
                    cx.fail("Serial-flash receive FIFO is empty");
                    0
                }
            };
        }
        u64::from(self.registers[(offset / 4) as usize])
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        if self.aperture.contains(address, size as usize) {
            let bytes = &value.to_le_bytes()[..size as usize];
            if let Err(error) = self.aperture.store(address, bytes) {
                cx.fail(error);
            }
            self.advance(cx);
        } else if size != 4 || address & 3 != 0 {
            cx.unmodeled();
        } else if value > u32::MAX.into() {
            cx.fail("Serial-flash register value overflow");
        } else {
            let offset = address - self.options.base;
            match offset {
                0x18 | 0x24 => cx.fail("Write to read-only serial-flash register"),
                // Interrupt mask; each set bit also acknowledges its status.
                0x28 => {
                    if value & !0x3f != 0 {
                        cx.fail("Unsupported serial-flash status acknowledgement");
                    } else {
                        self.registers[0x24 / 4] &= !(value as u32);
                        self.registers[0x28 / 4] = value as u32;
                    }
                }
                0x1c => {
                    if value != 1 {
                        cx.fail("Unsupported serial-flash command trigger");
                    } else if let Err(error) = self.start(cx) {
                        cx.fail(error);
                    }
                }
                // Bit 0 ends an aperture access; it does not read back.
                0x30 => {
                    self.registers[0x30 / 4] = value as u32 & !1;
                    if value & 1 != 0
                        && let Err(error) = self.end_access(cx)
                    {
                        cx.fail(error);
                    }
                }
                _ => {
                    self.registers[(offset / 4) as usize] = value as u32;
                    if matches!(offset, 0x00 | 0x10 | 0x20)
                        && let Err(error) = self.arm()
                    {
                        cx.fail(error);
                    }
                }
            }
            self.update_irq(cx);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.pending.as_ref().is_some_and(|p| p.due <= cx.now()) {
            let pending = self.pending.take().unwrap();
            let result = match pending.erase {
                Some((offset, size)) => {
                    let ticks = self.options.array.as_ref().map(|a| a.erase_ticks).unwrap();
                    self.begin_array_write(
                        cx,
                        pending.chip_select,
                        offset,
                        Some(size),
                        vec![],
                        ticks,
                    )
                    .map(|()| vec![])
                }
                None => self.nor.get_mut(&pending.chip_select).unwrap().command(
                    pending.due,
                    pending.opcode,
                    &pending.transmit,
                    pending.receive,
                ),
            };
            match result {
                Ok(bytes) => {
                    self.received = bytes.first().copied();
                    self.registers[0x24 / 4] |= 1 | if bytes.is_empty() { 0 } else { 8 };
                    self.registers[0x1c / 4] = 0;
                }
                Err(error) => cx.fail(error),
            }
        }
        if self
            .array_write
            .as_ref()
            .is_some_and(|(_, w)| w.due <= cx.now())
        {
            let (chip_select, write) = self.array_write.take().unwrap();
            self.aperture
                .apply(write.offset, write.erase, &write.program);
            let size = write.erase.unwrap_or(write.program.len() as u64);
            cx.record(Record::new(
                if write.erase.is_some() {
                    "X-Processor Pro flash erase complete"
                } else {
                    "X-Processor Pro flash program complete"
                },
                [
                    ("chip_select", Field::int(chip_select)),
                    ("offset", Field::int(write.offset)),
                    ("size", Field::int(size)),
                    ("tick", Field::int(write.due)),
                ],
            ));
        }
        // Collected page stores are shifted out at once: done and written.
        if self.aperture.take_unreported() {
            self.registers[0x24 / 4] |= 0x21;
        }
        for nor in self.nor.values_mut() {
            nor.advance(cx.now());
            if let Some(due) = nor.next_deadline() {
                cx.schedule(due, 0);
            }
        }
        self.update_irq(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.pending
            .as_ref()
            .map(|p| p.due)
            .into_iter()
            .chain(self.array_write.as_ref().map(|(_, w)| w.due))
            .chain(self.nor.values().filter_map(|n| n.next_deadline()))
            .min()
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        !self.aperture.contains(address, size as usize)
    }
    fn save(&self) -> Value {
        json!({ "options": self.options,"registers": self.registers, "aperture_reads": self.aperture.read_audit(),
            "aperture_contents": self.aperture.contents(), "array_write": self.array_write,
            "nor":self.nor,"pending":self.pending,"received":self.received })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let options: Options =
            serde_json::from_value(value["options"].clone()).map_err(|e| e.to_string())?;
        if self.options != options {
            return Err("Serial-flash controller checkpoint wiring differs".into());
        }
        let registers: [u32; (REGISTERS / 4) as usize] =
            serde_json::from_value(value["registers"].clone()).map_err(|e| e.to_string())?;
        let audit =
            serde_json::from_value(value["aperture_reads"].clone()).map_err(|e| e.to_string())?;
        let contents: Contents = match value.get("aperture_contents") {
            Some(contents) => {
                serde_json::from_value(contents.clone()).map_err(|e| e.to_string())?
            }
            None => Contents::default(),
        };
        let array_write: Option<(u8, ArrayWrite)> = match value.get("array_write") {
            Some(write) => serde_json::from_value(write.clone()).map_err(|e| e.to_string())?,
            None => None,
        };
        let nor: BTreeMap<u8, micron_nor::Registers> =
            serde_json::from_value(value["nor"].clone()).map_err(|e| e.to_string())?;
        if self.nor.keys().ne(nor.keys()) {
            return Err("Serial-flash checkpoint register devices differ".into());
        }
        for (chip_select, saved) in &nor {
            self.nor[chip_select].validate_restore(saved)?;
        }
        let pending: Option<Transaction> =
            serde_json::from_value(value["pending"].clone()).map_err(|e| e.to_string())?;
        let received: Option<u8> =
            serde_json::from_value(value["received"].clone()).map_err(|e| e.to_string())?;
        if pending.as_ref().is_some_and(|p| {
            p.transmit.len() > 1
                || p.receive > 1
                || !nor.contains_key(&p.chip_select)
                || received.is_some()
                || p.erase.is_some_and(|(offset, size)| {
                    self.options.array.is_none()
                        || !size.is_power_of_two()
                        || offset % size != 0
                        || offset + size > self.aperture.size()
                })
        }) || (received.is_some() && nor.is_empty())
            || array_write.as_ref().is_some_and(|(chip_select, w)| {
                let (lo, hi) = self.window(*chip_select);
                let len = w.erase.unwrap_or(w.program.len() as u64);
                !nor.contains_key(chip_select)
                    || self.options.array.is_none()
                    || (w.erase.is_some() == !w.program.is_empty())
                    || len == 0
                    || w.offset < lo
                    || w.offset + len > hi
            })
        {
            return Err("Invalid serial-flash pending transaction".into());
        }
        self.aperture.restore_audit(audit)?;
        self.aperture.restore_contents(contents)?;
        self.registers = registers;
        self.asserted =
            self.options.irq.is_some() && registers[0x24 / 4] & !registers[0x28 / 4] & 0x3f != 0;
        self.nor = nor;
        self.pending = pending;
        self.received = received;
        self.array_write = array_write;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    #[test]
    fn command_chip_select_requires_its_own_attached_register_device() {
        use fujimod_emulator_devices::test_host::advance;
        let base = 0x80000;
        let aperture = Arc::new(Aperture::new(0xf0000000, 0x1000, vec![]).unwrap());
        let nor = micron_nor::Registers::new(micron_nor::Options {
            enhanced_volatile_reset: 0xff,
            write_ticks: 7,
        })
        .unwrap();
        let mut d = FlashController::new(
            Options {
                base,
                transfer_ticks: 2,
                irq: None,
                array: None,
            },
            aperture,
            BTreeMap::from([(1, nor)]),
        )
        .unwrap();
        let mut h = TestHost::new();
        write(&mut d, &mut h, base, 6);
        write(&mut d, &mut h, base + 0x10, 0x18040);
        write(&mut d, &mut h, base + 0x20, 1);
        {
            let mut cx = h.cx();
            d.write(&mut cx, base + 0x1c, 4, 1);
            assert!(
                cx.refusal()
                    .unwrap()
                    .contains("chip select 0 has no attached")
            );
        }
        write(&mut d, &mut h, base + 0x10, 0x18080);
        write(&mut d, &mut h, base + 0x1c, 1);
        advance(&mut d, &mut h, 2);
        write(&mut d, &mut h, base + 0x28, 1);
        write(&mut d, &mut h, base, 5);
        write(&mut d, &mut h, base + 0x10, 0x38080);
        write(&mut d, &mut h, base + 0x20, 0x21);
        write(&mut d, &mut h, base + 0x1c, 1);
        advance(&mut d, &mut h, 4);
        assert_eq!(read(&mut d, &mut h, base + 0x18), 2);
    }

    #[test]
    fn register_commands_publish_timed_fifo_flags_and_restore_an_armed_transfer() {
        use fujimod_emulator_devices::test_host::advance;
        let base = 0x80000;
        let aperture = Arc::new(Aperture::new(0xf0000000, 0x1000, vec![0; 16]).unwrap());
        let nor = micron_nor::Registers::new(micron_nor::Options {
            enhanced_volatile_reset: 0xff,
            write_ticks: 16,
        })
        .unwrap();
        let mut d = FlashController::new(
            Options {
                base,
                transfer_ticks: 2,
                irq: None,
                array: None,
            },
            aperture.clone(),
            BTreeMap::from([(0, nor)]),
        )
        .unwrap();
        let mut h = TestHost::new();
        let start = |d: &mut FlashController, h: &mut TestHost, opcode, control, mode| {
            write(d, h, base, opcode);
            write(d, h, base + 0x10, control);
            write(d, h, base + 0x20, mode);
            write(d, h, base + 0x1c, 1);
        };
        start(&mut d, &mut h, 0x06, 0x18040, 1);
        assert_eq!(read(&mut d, &mut h, base + 0x24), 0);
        advance(&mut d, &mut h, 2);
        assert_eq!(read(&mut d, &mut h, base + 0x24), 1);
        write(&mut d, &mut h, base + 0x28, 1);
        write(&mut d, &mut h, base + 0x14, 0xf5);
        start(&mut d, &mut h, 0x61, 0x19041, 0x41);
        advance(&mut d, &mut h, 4);
        write(&mut d, &mut h, base + 0x28, 1);
        start(&mut d, &mut h, 0x05, 0x38040, 0x21);
        let saved = d.save();
        for pass in 0..2 {
            advance(&mut d, &mut h, 6);
            assert_eq!(read(&mut d, &mut h, base + 0x24), 9);
            write(&mut d, &mut h, base + 0x28, 8);
            assert_eq!(read(&mut d, &mut h, base + 0x24), 1);
            assert_eq!(read(&mut d, &mut h, base + 0x18), 1);
            write(&mut d, &mut h, base + 0x28, 1);
            assert_eq!(read(&mut d, &mut h, base + 0x24), 0);
            if pass == 0 {
                d.restore(&saved).unwrap();
                h.now = 4;
            }
        }
        advance(&mut d, &mut h, 20);
        start(&mut d, &mut h, 0x65, 0x39041, 0x21);
        advance(&mut d, &mut h, 22);
        assert_eq!(read(&mut d, &mut h, base + 0x18), 0xf5);
        assert_eq!(aperture.read_audit().bytes, 0);
        start(&mut d, &mut h, 0x03, 0x38040, 0x21);
        h.now = 24;
        let mut cx = h.cx();
        d.advance(&mut cx);
        assert!(
            cx.refusal()
                .unwrap()
                .contains("Unsupported serial NOR command 0x03")
        );
    }

    #[test]
    fn aperture_reads_the_image_then_erased_bytes_and_registers_retain_writes() {
        let aperture =
            Arc::new(Aperture::new(0xf0000000, 0x08000000, vec![1, 2, 3, 4, 5]).unwrap());
        let mut flash = FlashController::new(
            Options {
                base: 0xfff14000,
                transfer_ticks: 2,
                irq: None,
                array: None,
            },
            aperture.clone(),
            BTreeMap::new(),
        )
        .unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut flash, &mut h, 0xf0000000), 0x04030201);
        assert_eq!(read(&mut flash, &mut h, 0xf0000004), 0xffffff05);
        write(&mut flash, &mut h, 0xfff14008, 0x700);
        assert_eq!(read(&mut flash, &mut h, 0xfff14008), 0x700);
        let saved = flash.save();
        let mut resumed = FlashController::new(
            Options {
                base: 0xfff14000,
                transfer_ticks: 2,
                irq: None,
                array: None,
            },
            aperture,
            BTreeMap::new(),
        )
        .unwrap();
        resumed.restore(&saved).unwrap();
        assert_eq!(resumed.save(), saved);
        let mut cx = h.cx();
        flash.write(&mut cx, 0xf0000000, 4, 0);
        assert!(cx.refusal().is_some());
        assert!(Aperture::new(0xf0000000, 4, vec![0; 5]).is_err());
    }

    #[test]
    fn cpu_and_shared_dma_reads_share_a_checkpointed_passive_audit() {
        let aperture = Arc::new(Aperture::new(0xf0000000, 0x1000, vec![0; 16]).unwrap());
        let mut flash = FlashController::new(
            Options {
                base: 0xfff14000,
                transfer_ticks: 2,
                irq: None,
                array: None,
            },
            aperture.clone(),
            BTreeMap::new(),
        )
        .unwrap();
        let mut h = TestHost::new();
        read(&mut flash, &mut h, 0xf0000000);
        aperture.read(0xf0000008, 8).unwrap();
        aperture.read(0xf0000004, 8).unwrap();
        aperture.read(0xf0000010, 4).unwrap(); // erased backing still counts
        aperture.read(0xf0000100, 4).unwrap();
        let audit = aperture.read_audit();
        assert_eq!(audit.bytes, 28);
        assert_eq!(audit.ranges, [(0, 20), (0x100, 0x104)]);
        let saved = flash.save();
        aperture.read(0xf0000000, 4).unwrap();
        flash.restore(&saved).unwrap();
        assert_eq!(aperture.read_audit().bytes, 28);
        let mut bad = saved.clone();
        bad["aperture_reads"]["ranges"] = json!([[0, 0x1001]]);
        assert!(flash.restore(&bad).is_err());
        assert_eq!(flash.save(), saved);
        assert!(aperture.read(0xefffffff, 1).is_err());
        assert_eq!(flash.save(), saved);
    }

    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const BASE: u64 = 0xfff14000;
    const WINDOW: u64 = 0xf0000000;

    fn array_controller(image: Vec<u8>) -> (FlashController, Arc<Aperture>) {
        let aperture = Arc::new(Aperture::new(WINDOW, 0x08000000, image).unwrap());
        let nor = micron_nor::Registers::new(micron_nor::Options {
            enhanced_volatile_reset: 0xff,
            write_ticks: 7,
        })
        .unwrap();
        let controller = FlashController::new(
            Options {
                base: BASE,
                transfer_ticks: 1,
                irq: Some(35),
                array: Some(Array {
                    page_bytes: 0x100,
                    erase_ticks: 10,
                    page_ticks: 5,
                }),
            },
            aperture.clone(),
            BTreeMap::from([(0, nor)]),
        )
        .unwrap();
        (controller, aperture)
    }
    fn put(d: &mut FlashController, h: &mut RecordingHost, offset: u64, value: u32) {
        let mut cx = Cx::new(h, 0, 0, 0);
        d.write(&mut cx, BASE + offset, 4, value.into());
        assert_eq!(cx.refusal(), None, "{offset:#x}={value:#x}");
    }
    fn get(d: &mut FlashController, h: &mut RecordingHost, offset: u64) -> u32 {
        let mut cx = Cx::new(h, 0, 0, 0);
        let value = d.read(&mut cx, BASE + offset, 4) as u32;
        assert_eq!(cx.refusal(), None);
        value
    }
    fn at(d: &mut FlashController, h: &mut RecordingHost, now: u64) {
        h.now = now;
        let mut cx = Cx::new(h, 0, 0, 0);
        d.event(&mut cx, 0);
        assert_eq!(cx.refusal(), None);
    }
    /// Issues one command as the X-T20 driver does, keeping the read-mode
    /// control bits 0x1f, then acknowledges it.
    fn command(
        d: &mut FlashController,
        h: &mut RecordingHost,
        opcode: u32,
        phase: u32,
        count: u32,
    ) {
        put(d, h, 0x10, phase | 0x1f);
        put(d, h, 0x00, opcode);
        put(d, h, 0x20, count);
        put(d, h, 0x28, 0x3e);
        put(d, h, 0x1c, 1);
        let now = h.now + 1;
        at(d, h, now);
        assert_eq!(get(d, h, 0x24) & 1, 1);
    }
    fn status(d: &mut FlashController, h: &mut RecordingHost) -> u32 {
        command(d, h, 0x05, 0x38040, 0x21);
        put(d, h, 0x28, 0x3f);
        get(d, h, 0x18)
    }

    #[test]
    fn driver_carried_control_bits_keep_the_command_shape() {
        let (mut d, _) = array_controller(vec![]);
        let mut h = RecordingHost::default();
        command(&mut d, &mut h, 0x06, 0x18040, 0x01);
        assert_eq!(status(&mut d, &mut h), 2);
        put(&mut d, &mut h, 0x10, 0x18042 | 0x1f);
        put(&mut d, &mut h, 0x20, 0x41);
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        d.write(&mut cx, BASE + 0x1c, 4, 1);
        assert!(
            cx.refusal()
                .unwrap()
                .contains("Unsupported serial-flash command phases")
        );
    }

    #[test]
    fn erase_and_aperture_page_program_follow_the_driver_sequence() {
        let (mut d, aperture) = array_controller(vec![0; 0x2000]);
        let mut h = RecordingHost::default();
        // Erase without write-enable refuses when the command completes.
        put(&mut d, &mut h, 0x04, 0xf0001234);
        command(&mut d, &mut h, 0x06, 0x18040, 0x01);
        command(&mut d, &mut h, 0x21, 0x18047, 0x03);
        assert_eq!(status(&mut d, &mut h), 1);
        assert_eq!(aperture.read(WINDOW + 0x1000, 4).unwrap(), [0; 4]);
        let now = h.now + 10;
        at(&mut d, &mut h, now);
        assert_eq!(status(&mut d, &mut h), 0);
        assert_eq!(
            aperture.read(WINDOW + 0xffe, 4).unwrap(),
            [0, 0, 0xff, 0xff]
        );
        assert_eq!(
            aperture.read(WINDOW + 0x1ffc, 8).unwrap(),
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );

        // Page program: the DMA stores into the aperture, the driver unmasks
        // status bit 5, acknowledges it and ends the program through +0x30.
        command(&mut d, &mut h, 0x06, 0x18040, 0x01);
        put(&mut d, &mut h, 0x28, 0x3f);
        put(&mut d, &mut h, 0x10, 0xd8047 | 0x19);
        put(&mut d, &mut h, 0x00, 0x12);
        put(&mut d, &mut h, 0x20, 0x43);
        put(&mut d, &mut h, 0x28, 0x3e);
        aperture.store(WINDOW + 0x10f0, &[0x5a; 8]).unwrap();
        assert!(aperture.store(WINDOW + 0x10f0, &[0x5a; 8]).is_err());
        aperture.store(WINDOW + 0x10f8, &[0x0f; 8]).unwrap();
        assert!(aperture.store(WINDOW + 0x1100, &[0; 4]).is_err());
        h.irqs.clear();
        put(&mut d, &mut h, 0x28, 0x1e);
        assert_eq!(get(&mut d, &mut h, 0x24), 0x21);
        assert_eq!(h.irqs, [(35, true)]);
        put(&mut d, &mut h, 0x28, 0x3f);
        assert_eq!(h.irqs, [(35, true), (35, false)]);
        put(&mut d, &mut h, 0x30, 1);
        assert_eq!(get(&mut d, &mut h, 0x30), 0);
        // Ending an access without collected data (after a read) changes nothing.
        put(&mut d, &mut h, 0x30, 1);
        let saved = d.save();

        let (mut resumed, copy) = array_controller(vec![0; 0x2000]);
        resumed.restore(&saved).unwrap();
        assert_eq!(resumed.save(), saved);
        for (d, aperture) in [(&mut d, &aperture), (&mut resumed, &copy)] {
            let mut h = RecordingHost {
                now: h.now,
                ..RecordingHost::default()
            };
            assert_eq!(status(d, &mut h), 1);
            let now = h.now + 5;
            at(d, &mut h, now);
            assert_eq!(status(d, &mut h), 0);
            let mut expected = vec![0xff; 0x20];
            expected[0x10..0x18].fill(0x5a);
            expected[0x18..].fill(0x0f);
            assert_eq!(aperture.read(WINDOW + 0x10e0, 0x20).unwrap(), expected);
        }

        // Ending a program without write-enable, or rewriting the command
        // with collected data, refuses.
        put(&mut d, &mut h, 0x10, 0xd8047 | 0x19);
        put(&mut d, &mut h, 0x00, 0x12);
        put(&mut d, &mut h, 0x20, 0x43);
        aperture.store(WINDOW + 0x1200, &[0; 4]).unwrap();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        d.write(&mut cx, BASE + 0x00, 4, 0x05);
        assert!(cx.refusal().unwrap().contains("abandoned"));
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        d.write(&mut cx, BASE + 0x30, 4, 1);
        assert!(cx.refusal().unwrap().contains("write-disabled"));
        let mut bad = saved;
        bad["array_write"][1]["offset"] = json!(0x04000000);
        assert!(resumed.restore(&bad).is_err());
    }
}
