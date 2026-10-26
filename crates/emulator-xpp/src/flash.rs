//! X-Processor Pro flash controller and its memory-mapped read aperture.
//!
//! The command port supplies timed, single-byte register transactions to an
//! explicitly attached NOR register device. Array commands refuse. The aperture
//! reads attached array contents, erased (0xff) past the supplied image.
use fujimod_emulator_devices::parts::micron_nor;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Controller register window size in bytes.
pub const REGISTERS: u64 = 0x40;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub transfer_ticks: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    due: u64,
    chip_select: u8,
    opcode: u8,
    transmit: Vec<u8>,
    receive: usize,
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

/// Read-only flash contents behind a physical window.
pub struct Aperture {
    base: u64,
    size: u64,
    image: Vec<u8>,
    reads: Mutex<ReadAudit>,
}
impl Aperture {
    pub fn new(base: u64, size: u64, image: Vec<u8>) -> Result<Self, String> {
        if base & 0xfff != 0
            || size == 0
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
    pub fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, String> {
        if !self.contains(address, len) {
            return Err("Flash read outside the aperture".into());
        }
        let start = (address - self.base) as usize;
        let mut out = vec![0xff; len];
        if start < self.image.len() {
            let n = len.min(self.image.len() - start);
            out[..n].copy_from_slice(&self.image[start..start + n]);
        }
        self.reads
            .lock()
            .unwrap()
            .record(start as u64, start as u64 + len as u64)?;
        Ok(out)
    }
    pub fn read_audit(&self) -> ReadAudit {
        self.reads.lock().unwrap().clone()
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
}

pub struct FlashController {
    options: Options,
    aperture: Arc<Aperture>,
    registers: [u32; (REGISTERS / 4) as usize],
    nor: BTreeMap<u8, micron_nor::Registers>,
    pending: Option<Transaction>,
    received: Option<u8>,
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
        })
    }
    fn start(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.pending.is_some() || self.received.is_some() {
            return Err("Serial-flash command port has an unfinished transaction".into());
        }
        let chip_select = match (self.registers[0x10 / 4] >> 6) & 3 {
            1 => 0,
            2 => 1,
            _ => return Err("Serial-flash command requires one chip select".into()),
        };
        if !self.nor.contains_key(&chip_select) {
            return Err(format!(
                "Serial-flash chip select {chip_select} has no attached register device"
            ));
        }
        let (transmit, receive) = match (self.registers[0x10 / 4], self.registers[0x20 / 4] & 0x7f)
        {
            (0x18040 | 0x18080, 1) => (vec![], 0),
            (0x38040 | 0x38080 | 0x39041 | 0x39081, 0x21) => (vec![], 1),
            (0x19041 | 0x19081, 0x41) => {
                let value = u8::try_from(self.registers[0x14 / 4])
                    .map_err(|_| "Serial-flash transmit byte overflow")?;
                (vec![value], 0)
            }
            _ => return Err("Unsupported serial-flash command phases".into()),
        };
        let opcode = u8::try_from(self.registers[0]).map_err(|_| "Serial-flash opcode overflow")?;
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
        });
        self.registers[0x1c / 4] = 1;
        cx.schedule(due, 0);
        Ok(())
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
            cx.fail("X-Processor Pro flash aperture writes are not modeled");
        } else if size != 4 || address & 3 != 0 {
            cx.unmodeled();
        } else if value > u32::MAX.into() {
            cx.fail("Serial-flash register value overflow");
        } else {
            let offset = address - self.options.base;
            match offset {
                0x18 | 0x24 => cx.fail("Write to read-only serial-flash register"),
                0x28 => {
                    if value & !0x3f != 0 {
                        cx.fail("Unsupported serial-flash status acknowledgement");
                    } else {
                        self.registers[0x24 / 4] &= !(value as u32);
                    }
                }
                0x1c => {
                    if value != 1 {
                        cx.fail("Unsupported serial-flash command trigger");
                    } else if let Err(error) = self.start(cx) {
                        cx.fail(error);
                    }
                }
                _ => self.registers[(offset / 4) as usize] = value as u32,
            }
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.pending.as_ref().is_some_and(|p| p.due <= cx.now()) {
            let pending = self.pending.take().unwrap();
            let nor = self.nor.get_mut(&pending.chip_select).unwrap();
            match nor.command(
                pending.due,
                pending.opcode,
                &pending.transmit,
                pending.receive,
            ) {
                Ok(bytes) => {
                    self.received = bytes.first().copied();
                    self.registers[0x24 / 4] |= 1 | if bytes.is_empty() { 0 } else { 8 };
                    self.registers[0x1c / 4] = 0;
                }
                Err(error) => cx.fail(error),
            }
        }
        for nor in self.nor.values_mut() {
            nor.advance(cx.now());
            if let Some(due) = nor.next_deadline() {
                cx.schedule(due, 0);
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.pending
            .as_ref()
            .map(|p| p.due)
            .into_iter()
            .chain(self.nor.values().filter_map(|n| n.next_deadline()))
            .min()
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        !self.aperture.contains(address, size as usize)
    }
    fn save(&self) -> Value {
        json!({ "options": self.options,"registers": self.registers, "aperture_reads": self.aperture.read_audit(),
            "nor":self.nor,"pending":self.pending,"received":self.received })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let options: Options =
            serde_json::from_value(value["options"].clone()).map_err(|e| e.to_string())?;
        if self.options != options {
            return Err("Serial-flash controller checkpoint wiring differs".into());
        }
        let registers =
            serde_json::from_value(value["registers"].clone()).map_err(|e| e.to_string())?;
        let audit =
            serde_json::from_value(value["aperture_reads"].clone()).map_err(|e| e.to_string())?;
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
        }) || (received.is_some() && nor.is_empty())
        {
            return Err("Invalid serial-flash pending transaction".into());
        }
        self.aperture.restore_audit(audit)?;
        self.registers = registers;
        self.nor = nor;
        self.pending = pending;
        self.received = received;
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
}
