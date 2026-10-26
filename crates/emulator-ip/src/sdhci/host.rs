//! SDHCI memory-card host with explicit register-base wiring. MMC, erase, tuning,
//! and UHS refuse. Latencies and card identity are declared inputs.
//! SDMA writes bytes before status and IRQ; boundaries require W1C/resume.
use super::{Clocks, registers::Registers};
use fujimod_emulator_devices::bus::FastReadWord;
use fujimod_emulator_devices::parts::card_contacts::Contacts;
use fujimod_emulator_devices::parts::read_fifo::ReadFifo;
use fujimod_emulator_devices::parts::sd_card::Card;
use fujimod_emulator_devices::parts::sd_card::faults::{DataDirection, DataFault, DataFaultEvent};
use fujimod_emulator_devices::storage_reads::StorageReads;
use fujimod_emulator_devices::{Cx, Device, bus::InterruptLine};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU16, Ordering},
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// Both register-width interfaces on the same physical host and card.
    Sdhc,
    Application,
    Rom,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializationFault {
    #[default]
    None,
    /// CMD8 completes with Command Timeout Error and no response.
    RejectCmd8,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub protocol: Protocol,
    pub transfer_ticks: u64,
    pub clock_ticks: u64,
    pub rca: u16,
    pub ram_bytes: u64,
    #[serde(default)]
    pub initialization_fault: InitializationFault,
}
const MAX_DATA_FAULTS: usize = 32;
const COMMAND_TIMEOUT_ERROR: u16 = 1;
const DATA_CRC_ERROR: u16 = 0x20;
const ERROR_INTERRUPT: u16 = 0x8000;
pub struct Wiring {
    pub card: Card,
    pub contacts: Contacts,
    pub irq: InterruptLine,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum State {
    Idle,
    Ready,
    Identification,
    Standby,
    Transfer,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
enum Payload {
    None,
    Metadata {
        #[serde(with = "fujimod_emulator_devices::checkpoint::array")]
        bytes: [u8; 64],
        len: usize,
    },
    Read(u32),
    Write(u32),
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Transfer {
    Dma,
    PioRead,
    PioWrite,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Command,
    Ready,
    Sector,
    Boundary,
}
#[derive(Serialize, Deserialize)]
struct Request {
    state: State,
    app: bool,
    block_length: usize,
    bus_width: u8,
    response: [u8; 16],
    payload: Payload,
    transfer: Transfer,
    quantity: usize,
    cursor: usize,
    address: u32,
    phase: Phase,
    due: Option<u64>,
    command: u8,
    fault: Option<usize>,
}
fn read_media(
    card: &Card,
    reads: &mut StorageReads,
    offset: u64,
    out: &mut [u8],
) -> Result<(), String> {
    card.read_bytes(offset, out)?;
    reads.record(offset, out.len() as u64, u64::from(card.sectors()) * 512)
}
pub struct SdHost {
    options: Options,
    // Guest media transfers since the last explicit card replacement. Host
    // inspection and contact removal do not affect this passive ledger.
    reads: StorageReads,
    card: Card,
    contacts: Contacts,
    irq: Option<InterruptLine>,
    regs: Registers,
    clocks: Clocks,
    state: State,
    app: bool,
    block_length: usize,
    bus_width: u8,
    response: [u8; 16],
    request: Option<Request>,
    fifo: ReadFifo,
    status_reads: [Arc<AtomicU16>; 2],
    scratch: Box<[u8]>,
    rom_fifo_ready: bool,
    faults: Vec<DataFault>,
    fault_history: Vec<DataFaultEvent>,
}
impl SdHost {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        Self::construct(options, wiring.card, wiring.contacts, Some(wiring.irq))
    }
    /// An explicitly unconnected interrupt output. Polling is supported; an
    /// enabled interrupt assertion refuses rather than inventing a route.
    pub fn new_polled(options: Options, card: Card, contacts: Contacts) -> Result<Self, String> {
        Self::construct(options, card, contacts, None)
    }
    fn construct(
        options: Options,
        card: Card,
        contacts: Contacts,
        irq: Option<InterruptLine>,
    ) -> Result<Self, String> {
        if options.base & 0xfff != 0
            || options.base > 0xffffe000
            || options.transfer_ticks == 0
            || options.rca == 0
            || options.ram_bytes == 0
            || options.ram_bytes > 0x1_0000_0000
        {
            return Err("Invalid SD latency, RCA or DRAM extent".into());
        }
        let clocks = Clocks::new(&[(options.base + 0x2c, options.clock_ticks)])?;
        Ok(Self {
            options,
            reads: StorageReads::default(),
            card,
            contacts,
            irq,
            regs: Registers::new(),
            clocks,
            state: State::Idle,
            app: false,
            block_length: 512,
            bus_width: 1,
            response: [0; 16],
            request: None,
            fifo: ReadFifo::new(512)?,
            status_reads: std::array::from_fn(|_| Arc::new(AtomicU16::new(0))),
            scratch: vec![0; 512 * 1024].into_boxed_slice(),
            rom_fifo_ready: false,
            faults: Vec::new(),
            fault_history: Vec::new(),
        })
    }
    /// Declare a data CRC failure for later multi-block transfers.
    pub fn inject_data_fault(&mut self, fault: DataFault) -> Result<(), String> {
        if self.busy() {
            return Err("configure SD data faults while the host is idle".into());
        }
        if fault.sector.is_some_and(|s| s >= self.card.sectors())
            || fault.after_sectors > 0xffff
            || !(1..=0x10000).contains(&fault.occurrences)
            || self.faults.len() >= MAX_DATA_FAULTS
        {
            return Err(
                "SD data fault sector, prefix, occurrences or rule count unsupported".into(),
            );
        }
        self.faults.push(fault);
        Ok(())
    }
    pub fn clear_data_faults(&mut self) -> Result<(), String> {
        if self.busy() {
            return Err("clear SD data faults while the host is idle".into());
        }
        self.faults.clear();
        Ok(())
    }
    pub fn data_faults(&self) -> &[DataFault] {
        &self.faults
    }
    pub fn fault_history(&self) -> &[DataFaultEvent] {
        &self.fault_history
    }
    fn select_fault(&mut self, r: &mut Request) -> Result<(), String> {
        let (direction, first) = match (r.command, r.payload) {
            (18, Payload::Read(first)) => (DataDirection::Read, first),
            (25, Payload::Write(first)) => (DataDirection::Write, first),
            _ => return Ok(()),
        };
        let Some((index, cursor)) = self.faults.iter().enumerate().find_map(|(index, f)| {
            let cursor = (i64::from(f.sector.unwrap_or(first)) - i64::from(first)
                + i64::from(f.after_sectors))
                * 512;
            (f.direction == direction && (0..r.quantity as i64).contains(&cursor))
                .then_some((index, cursor as usize))
        }) else {
            return Ok(());
        };
        if r.transfer != Transfer::Dma && cursor != 0 {
            return Err("partial PIO data CRC faults are not modeled".into());
        }
        r.fault = Some(cursor);
        let fault = &mut self.faults[index];
        fault.occurrences -= 1;
        if fault.occurrences == 0 {
            self.faults.remove(index);
        }
        Ok(())
    }
    /// Data CRC error status ends the transfer at the failing byte position.
    fn data_error(&mut self, cx: &mut Cx<'_>, r: Request) -> Result<(), String> {
        let sector = match r.payload {
            Payload::Read(first) | Payload::Write(first) => first,
            _ => unreachable!(),
        };
        if self.fault_history.len() == MAX_DATA_FAULTS {
            self.fault_history.remove(0);
        }
        self.fault_history.push(DataFaultEvent {
            command: r.command,
            sector,
            transferred: r.cursor,
        });
        self.regs.set(0x32, self.regs.get(0x32) | DATA_CRC_ERROR);
        self.finish(cx, r, ERROR_INTERRUPT)
    }
    pub fn card(&self) -> &Card {
        &self.card
    }
    /// Session construction assigns an external medium's initial identity.
    pub fn initialize_revision(&mut self, revision: u64) -> Result<(), String> {
        if self.busy() {
            return Err("SD medium is busy".into());
        }
        self.card.initialize_revision(revision);
        Ok(())
    }
    pub fn busy(&self) -> bool {
        self.request.is_some() || self.rom_fifo_ready
    }
    pub fn replace_card(&mut self, mut card: Card) -> Result<Card, String> {
        if self.busy() {
            return Err("SD medium is busy".into());
        }
        card.supersede(self.card.revision())?;
        self.reads = StorageReads::default();
        Ok(std::mem::replace(&mut self.card, card))
    }
    /// A new, unexecuted board reconnects its external medium without editing
    /// content or manufacturing a revision. The session checks cold ownership.
    pub fn retain_card(&mut self, card: Card) -> Result<(), String> {
        if self.busy() || card.sectors() != self.card.sectors() {
            return Err("retained SD medium differs or host is busy".into());
        }
        self.card = card;
        Ok(())
    }
    pub fn readable_card(&self, revision: Option<u64>) -> Result<&Card, String> {
        if !self.contacts.present() {
            return Err("No SD card inserted".into());
        }
        if self.busy() {
            return Err("SD medium is busy".into());
        }
        if revision.is_some_and(|r| r != self.card.revision()) {
            return Err("SD medium revision changed".into());
        }
        Ok(&self.card)
    }
    fn signal(&self, cx: &mut Cx<'_>) -> Result<(), String> {
        match self.irq {
            Some(irq) => irq.drive(cx, self.regs.asserted()),
            None if self.regs.asserted() => Err("SD interrupt output is unconnected".into()),
            None => Ok(()),
        }
    }
    fn r1(&self) -> u32 {
        let state = match self.state {
            State::Idle => 0,
            State::Ready => 1,
            State::Identification => 2,
            State::Standby => 3,
            State::Transfer => 4,
        };
        state << 9
            | if self.state == State::Transfer || self.options.protocol == Protocol::Rom {
                0x100
            } else {
                0
            }
    }
    fn physical(&self, address: u32, len: usize, rom: bool) -> Result<u64, String> {
        let a = address as u64;
        if rom && !(0x40000000..0x60000000).contains(&a) {
            return Err("ROM SDMA requires DRAM alias".into());
        }
        let physical = if (0x40000000..0x60000000).contains(&a) {
            a & 0x3fffffff
        } else {
            a
        };
        if address & 3 != 0
            || len == 0
            || len > 0x2000000
            || physical
                .checked_add(len as u64)
                .is_none_or(|end| end > self.options.ram_bytes)
        {
            return Err("SD DMA exceeds aligned DRAM".into());
        }
        // ROM must retain the alias while physical low RAM is shadowed by ROM.
        Ok(if rom { a } else { physical })
    }
    fn plan(&self, word: u32, argument: u32) -> Result<Request, String> {
        let command = (word >> 24 & 63) as u8;
        let rom = self.options.protocol == Protocol::Rom;
        let app = self.app && command != 55;
        let expected = if rom {
            (match command {
                0 => 0,
                8 | 55 | 3 | 16 | 6 => 0x1a,
                41 => 2,
                2 => 9,
                7 => 0x1b,
                17 => 0x3a,
                _ => 0xff,
            }) as u32
        } else if app {
            match command {
                6 => 0x061a0000,
                13 => 0x0d3a0011,
                41 => 0x29020000,
                42 => 0x2a1a0000,
                51 => 0x333a0011,
                _ => u32::MAX,
            }
        } else {
            match command {
                0 => 0,
                2 => 0x02090000,
                3 => 0x031a0000,
                7 => 0x071b0000,
                8 => 0x081a0000,
                9 => 0x09090000,
                12 => 0x0cdb0000,
                13 => 0x0d1a0000,
                16 => 0x101a0000,
                18 => 0x123a0037,
                25 => 0x193a0027,
                55 => 0x371a0000,
                6 => 0x063a0011,
                _ => u32::MAX,
            }
        };
        let single =
            self.options.protocol == Protocol::Sdhc && matches!(word, 0x113a0010 | 0x113a0011);
        let pio = !rom && (matches!(word, 0x193a0026 | 0x123a0036) || single && word & 1 == 0);
        if (rom && word >> 16 & 255 != expected) || (!rom && word != expected && !pio && !single) {
            return Err(format!("Unsupported SD command {command}/{word:#x}"));
        }
        let mut request = Request {
            state: self.state,
            app: false,
            block_length: self.block_length,
            bus_width: self.bus_width,
            response: [0; 16],
            payload: Payload::None,
            transfer: Transfer::Dma,
            quantity: 0,
            cursor: 0,
            address: self.regs.word(0),
            phase: Phase::Command,
            due: None,
            command,
            fault: None,
        };
        let mut response = self.r1();
        let rca = (self.options.rca as u32) << 16;
        let bad = match command {
            0 => {
                request.state = State::Idle;
                request.block_length = 512;
                request.bus_width = 1;
                response = 0;
                argument != 0
            }
            8 => {
                response = 0x1aa;
                self.state != State::Idle || argument != 0x1aa
            }
            55 => {
                request.app = true;
                response |= 0x20;
                if rom {
                    argument >> 16
                        != if matches!(self.state, State::Standby | State::Transfer) {
                            self.options.rca as u32
                        } else {
                            0
                        }
                } else {
                    !matches!(self.state, State::Idle | State::Standby | State::Transfer)
                        || ![0, rca].contains(&(argument & 0xffff0000))
                }
            }
            41 if app => {
                // Physical Layer 7.10 section 4.2.3.1: a zero voltage window
                // queries OCR without initializing; bits 31:24 are ignored.
                let voltage = if rom || self.options.protocol == Protocol::Sdhc {
                    0x00ff8000
                } else {
                    0x00100000
                };
                let inquiry = argument & 0x00ffffff == 0;
                response = voltage;
                if !inquiry {
                    request.state = State::Ready;
                    // SDHC, immediately ready, with no 1.8 V switch accepted.
                    response |= 0xc0000000;
                }
                self.state != State::Idle
                    || (!inquiry
                        && (argument & !0x51ffff00 != 0
                            || argument & 0x40000000 == 0
                            || argument & voltage == 0))
            }
            2 => {
                request.state = State::Identification;
                if !rom {
                    let cid = u128::from_be_bytes(*b"\xfeMCVIRT0\x10\x12\x34\x56\x78\x01\xaa\xff");
                    request.response = (cid >> 8).to_le_bytes();
                }
                self.state != State::Ready || argument != 0
            }
            3 => {
                request.state = State::Standby;
                response = rca | if rom { 0x500 } else { 0x600 };
                self.state != State::Identification || argument != 0
            }
            9 if !rom => {
                let csd = 1u128 << 126
                    | 0x0eu128 << 112
                    | 0x32u128 << 96
                    | 0x515u128 << 84
                    | 9u128 << 80
                    | (self.card.sectors() as u128 / 1024 - 1) << 48
                    | 1u128 << 46
                    | 0x7fu128 << 39
                    | 9u128 << 22
                    | 1;
                request.response = (csd >> 8).to_le_bytes();
                self.state != State::Standby || argument & 0xffff0000 != rca
            }
            7 => {
                request.state = if argument != 0 {
                    State::Transfer
                } else {
                    State::Standby
                };
                response = if argument != 0 { 0x900 } else { 0x600 };
                !matches!(self.state, State::Standby | State::Transfer)
                    || ![0, rca].contains(&(argument & 0xffff0000))
                    || rom && argument == 0
            }
            _ if self.state != State::Transfer => true,
            16 => {
                request.block_length = argument as usize;
                ![8, 64, 512].contains(&argument) || rom && argument != 512
            }
            42 if app && !rom => argument != 0,
            6 if app => {
                request.bus_width = 4;
                argument != 2
            }
            13 if app && !rom => {
                let mut bytes = [0; 64];
                bytes[0] = 0x80;
                bytes[10] = 0x70;
                request.payload = Payload::Metadata { bytes, len: 64 };
                argument != 0 || self.block_length != 64 || self.bus_width != 4
            }
            51 if app && !rom => {
                let mut bytes = [0; 64];
                bytes[..2].copy_from_slice(&[2, 5]);
                request.payload = Payload::Metadata { bytes, len: 8 };
                argument != 0 || self.block_length != 8
            }
            6 if !rom => {
                let mut bytes = [0; 64];
                bytes[13] = 1;
                request.payload = Payload::Metadata { bytes, len: 64 };
                argument != 1 || self.block_length != 64
            }
            17 | 18 | 25 => {
                let count = if command == 17 {
                    1
                } else {
                    let count = self.regs.get(6);
                    if count == 0 { 65536 } else { count as u32 }
                };
                self.card.range(argument, count)?;
                request.payload = if command == 25 {
                    Payload::Write(argument)
                } else {
                    Payload::Read(argument)
                };
                request.quantity = count as usize * 512;
                if pio || rom && word as u16 == 0x10 {
                    request.transfer = if command == 25 {
                        Transfer::PioWrite
                    } else {
                        Transfer::PioRead
                    };
                }
                self.block_length != 512
                    || command == 25 && self.contacts.protected()
                    || (rom || single)
                        && (self.regs.get(6) != 1 || ![0x10, 0x11].contains(&(word as u16)))
            }
            12 if !rom => argument != 0,
            13 if !rom => argument != rca,
            _ => true,
        };
        if bad {
            return Err(format!(
                "SD CMD{command} argument or card state unsupported"
            ));
        }
        if !matches!(command, 2 | 9) {
            request.response[..4].copy_from_slice(&response.to_le_bytes());
        }
        if let Payload::Metadata { len, .. } = request.payload {
            request.quantity = len;
        }
        Ok(request)
    }
    fn start(&mut self, cx: &mut Cx<'_>, word: u32) -> Result<(), String> {
        let rom = self.options.protocol == Protocol::Rom;
        if self.busy()
            || !self.contacts.present()
            || self.regs.get(0x30) != 0
            || self.regs.get(0x32) != 0
        {
            return Err("SD command requires present idle card and cleared status".into());
        }
        let clock = self.clocks.read(cx, self.options.base + 0x2c, 2);
        if clock & 7 != 7
            || self.regs.get(0x28) & 0x100 == 0 && !rom
            || rom && self.regs.get(0x28) >> 8 & 15 != 15
        {
            return Err("SD command before clock/power setup".into());
        }
        let argument = self.regs.word(8);
        let mut r = self.plan(word, argument)?;
        if rom && r.command != 17 && word as u16 != 0 {
            return Err("ROM non-data command has transfer mode bits".into());
        }
        if rom && self.regs.get(0x34) & 3 != 3 {
            return Err("ROM command requires status enabled".into());
        }
        if r.quantity != 0 {
            if self.regs.get(4) & 0xfff
                != if matches!(r.payload, Payload::Metadata { .. }) {
                    r.quantity as u16
                } else {
                    512
                }
                || self.regs.get(0x28) & 0x18 != 0
            {
                return Err("SD block-size or DMA mode unsupported".into());
            }
            if !rom && r.command != 17 && self.regs.get(0x34) & self.regs.get(0x38) & 2 == 0 {
                return Err("SD data-complete signal disabled".into());
            }
            if r.transfer == Transfer::Dma {
                self.physical(r.address, r.quantity, rom || r.command == 17)?;
                let boundary = 4096usize << (self.regs.get(4) >> 12 & 7);
                if !rom
                    && r.quantity > boundary - r.address as usize % boundary
                    && self.regs.get(0x34) & self.regs.get(0x38) & 8 == 0
                {
                    return Err("SDMA boundary interrupt disabled".into());
                }
                if matches!(r.payload, Payload::Write(_))
                    && r.address & if r.address & 0x40000000 != 0 { 7 } else { 31 } != 0
                {
                    return Err("SDMA write alignment unsupported".into());
                }
            } else if !rom && r.command != 17 {
                let signals = if r.transfer == Transfer::PioWrite {
                    0x13
                } else {
                    0x23
                };
                if self.regs.get(0x34) & self.regs.get(0x38) & signals != signals {
                    return Err("SD PIO directional signals disabled".into());
                }
            }
            if let Payload::Write(first) = r.payload {
                self.card.reserve_write(first, (r.quantity / 512) as u32)?;
            }
        }
        r.due = Some(
            cx.now()
                .checked_add(self.options.transfer_ticks)
                .ok_or("SD deadline overflow")?,
        );
        self.select_fault(&mut r)?;
        self.regs.put_word(12, word);
        self.request = Some(r);
        Ok(())
    }
    fn finish(&mut self, cx: &mut Cx<'_>, r: Request, status: u16) -> Result<(), String> {
        self.state = r.state;
        self.app = r.app;
        self.block_length = r.block_length;
        self.bus_width = r.bus_width;
        self.response = r.response;
        self.regs.set(0x30, self.regs.get(0x30) | status);
        self.signal(cx)
    }
    fn complete(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let Some(mut r) = self.request.take() else {
            return Ok(());
        };
        if r.due.is_none_or(|due| due > cx.now()) {
            self.request = Some(r);
            return Ok(());
        }
        let rom = self.options.protocol == Protocol::Rom;
        if !self.contacts.present()
            || matches!(r.payload, Payload::Write(_)) && self.contacts.protected()
        {
            self.request = Some(r);
            return Err("SD contacts changed during transfer".into());
        }
        if r.command == 8 && self.options.initialization_fault == InitializationFault::RejectCmd8 {
            self.regs
                .set(0x32, self.regs.get(0x32) | COMMAND_TIMEOUT_ERROR);
            self.regs.set(0x30, self.regs.get(0x30) | ERROR_INTERRUPT);
            return self.signal(cx);
        }
        r.due = None;
        self.response = r.response;
        if r.quantity == 0 {
            // A busy response (R1b) ends with transfer complete when enabled.
            let busy_end = if r.command == 7 && (rom || self.options.protocol == Protocol::Sdhc) {
                2 & self.regs.get(0x34)
            } else {
                0
            };
            let status = 1 | busy_end;
            return self.finish(
                cx,
                r,
                status & if rom { self.regs.get(0x34) } else { u16::MAX },
            );
        }
        if r.transfer != Transfer::Dma {
            if r.phase == Phase::Sector && r.transfer == Transfer::PioWrite {
                if let Payload::Write(first) = r.payload {
                    self.card.write_bytes(
                        first as u64 * 512 + r.cursor as u64,
                        &self.fifo.snapshot().0,
                    )?;
                }
                r.cursor += 512;
            }
            self.regs.set(6, ((r.quantity - r.cursor) / 512) as u16);
            if r.fault == Some(r.cursor) {
                return self.data_error(cx, r);
            }
            if r.cursor == r.quantity {
                return self.finish(cx, r, 2);
            }
            if r.transfer == Transfer::PioRead
                && let Payload::Read(first) = r.payload
            {
                let mut bytes = [0; 512];
                read_media(
                    &self.card,
                    &mut self.reads,
                    first as u64 * 512 + r.cursor as u64,
                    &mut bytes,
                )?;
                self.fifo.restore(&bytes, 0)?;
            }
            if rom {
                self.fifo.rewind();
                self.rom_fifo_ready = true;
                return self.finish(cx, r, 0x21 & self.regs.get(0x34));
            }
            let status = if r.transfer == Transfer::PioWrite {
                0x10
            } else {
                0x20
            } | if r.phase == Phase::Command { 1 } else { 0 };
            r.phase = Phase::Ready;
            self.fifo.rewind();
            self.request = Some(r);
            self.regs.set(0x30, self.regs.get(0x30) | status);
            return self.signal(cx);
        }
        if r.fault == Some(r.cursor) {
            return self.data_error(cx, r);
        }
        let boundary = 4096usize << (self.regs.get(4) >> 12 & 7);
        let address = r
            .address
            .checked_add(r.cursor as u32)
            .ok_or("SDMA address overflow")?;
        let end = r.fault.filter(|f| *f > r.cursor).unwrap_or(r.quantity);
        let length = (end - r.cursor).min(boundary - address as usize % boundary);
        let physical = self.physical(address, length, rom || r.command == 17)?;
        match r.payload {
            Payload::Write(first) => {
                cx.ram_read(physical, &mut self.scratch[..length])?;
                self.card.write_bytes(
                    first as u64 * 512 + r.cursor as u64,
                    &self.scratch[..length],
                )?;
            }
            Payload::Read(first) => {
                read_media(
                    &self.card,
                    &mut self.reads,
                    first as u64 * 512 + r.cursor as u64,
                    &mut self.scratch[..length],
                )?;
                cx.ram_write(physical, &self.scratch[..length])?;
            }
            Payload::Metadata { bytes, .. } => {
                cx.ram_write(physical, &bytes[r.cursor..r.cursor + length])?
            }
            Payload::None => unreachable!(),
        }
        r.cursor += length;
        self.regs.set(
            6,
            (r.quantity.div_ceil(self.block_length) - r.cursor / self.block_length) as u16,
        );
        self.regs.put_word(0, r.address + r.cursor as u32);
        if r.fault == Some(r.cursor) {
            return self.data_error(cx, r);
        }
        if r.cursor < r.quantity {
            r.phase = Phase::Boundary;
            self.request = Some(r);
            self.regs.set(0x30, self.regs.get(0x30) | 9);
            self.signal(cx)
        } else {
            self.finish(cx, r, 3 & if rom { self.regs.get(0x34) } else { u16::MAX })
        }
    }
    fn pio(&mut self, cx: &mut Cx<'_>, write: Option<u32>) -> Result<u32, String> {
        if self.options.protocol == Protocol::Rom {
            if write.is_some() || !self.rom_fifo_ready {
                return Err("ROM SD PIO read without ready sector".into());
            }
            let value = self.fifo.read().ok_or("ROM SD PIO FIFO is empty")?;
            if self.fifo.position() == 512 {
                self.rom_fifo_ready = false;
                self.regs
                    .set(0x30, self.regs.get(0x30) | (2 & self.regs.get(0x34)));
            }
            return Ok(value);
        }
        let r = self.request.as_mut().ok_or("SD PIO without request")?;
        let is_write = r.transfer == Transfer::PioWrite;
        // Present State's buffer enable controls access. Buffer Ready is a
        // latched interrupt notification, independent of FIFO availability
        // (Host Controller 4.20 section 2.2.9).
        if r.phase != Phase::Ready || is_write != write.is_some() {
            return Err("SD PIO requires a ready buffer in the transfer direction".into());
        }
        let value = if let Some(value) = write {
            self.fifo.write(value)?;
            0
        } else {
            self.fifo.read().ok_or("SD PIO FIFO is empty")?
        };
        if self.fifo.position() == 512 {
            if !is_write {
                r.cursor += 512;
            }
            r.phase = Phase::Sector;
            r.due = Some(
                cx.now()
                    .checked_add(self.options.transfer_ticks)
                    .ok_or("SD PIO deadline overflow")?,
            );
        }
        Ok(value)
    }
    fn read_register(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if address == self.options.base + 0x2c && matches!(size, 2 | 4) {
            let clock = self.clocks.read(cx, address, 2);
            return clock
                | if size == 4 {
                    u64::from(self.regs.get(0x2e)) << 16
                } else {
                    0
                };
        }
        let offset = address.wrapping_sub(self.options.base) as usize;
        if !self.valid(offset, size) {
            cx.unmodeled();
            return 0;
        }
        self.advance(cx);
        if cx.refusal().is_some() {
            return 0;
        }
        if offset == 0x20 {
            return match self.pio(cx, None) {
                Ok(v) => v as u64,
                Err(e) => {
                    cx.fail(e);
                    0
                }
            };
        }
        if offset == 0x24 {
            return 0x20000
                | if self.contacts.present() { 0x10000 } else { 0 }
                | self.request.as_ref().map_or(0, |r| {
                    if r.phase == Phase::Ready {
                        if r.transfer == Transfer::PioWrite {
                            0x403
                        } else {
                            0x803
                        }
                    } else if r.quantity != 0 || matches!(r.command, 7 | 12) {
                        3
                    } else {
                        1
                    }
                });
        }
        if (16..32).contains(&offset) {
            let count = size as usize;
            let mut bytes = [0; 4];
            bytes[..count].copy_from_slice(&self.response[offset - 16..offset - 16 + count]);
            return u32::from_le_bytes(bytes) as u64;
        }
        match size {
            4 => self.regs.word(offset) as u64,
            2 => self.regs.get(offset) as u64,
            _ => (self.regs.get(offset & !1) >> ((offset % 2) * 8) & 255) as u64,
        }
    }
    fn publish_reads(&self) {
        for (i, value) in self.status_reads.iter().enumerate() {
            value.store(self.regs.get(0x30 + i * 2), Ordering::Release);
        }
        let ready = if self.options.protocol == Protocol::Rom {
            self.rom_fifo_ready
        } else {
            self.request
                .as_ref()
                .is_some_and(|r| r.phase == Phase::Ready && r.transfer == Transfer::PioRead)
        };
        self.fifo.publish(ready);
    }
    fn valid(&self, offset: usize, size: u8) -> bool {
        if self.options.protocol == Protocol::Sdhc {
            Registers::valid(offset, size)
                || matches!(
                    (offset, size),
                    (4, 4)
                        | (8, 4)
                        | (16, 4)
                        | (20, 4)
                        | (24, 4)
                        | (28, 4)
                        | (0x28, 4)
                        | (0x30, 4)
                        | (0x34, 4)
                        | (0x38, 4)
                        | (0x28, 1)
                        | (0x29, 1)
                        | (0x2e, 1)
                        | (0x2f, 1)
                )
        } else if self.options.protocol == Protocol::Application {
            Registers::valid(offset, size)
        } else {
            matches!(
                (offset, size),
                (0, 4)
                    | (4, 2)
                    | (6, 2)
                    | (8, 4)
                    | (12, 2)
                    | (14, 2)
                    | (16, 4)
                    | (0x20, 4)
                    | (0x24, 4)
                    | (0x28, 1)
                    | (0x29, 1)
                    | (0x2e, 1)
                    | (0x2f, 1)
                    | (0x30, 2)
                    | (0x32, 2)
                    | (0x34, 2)
                    | (0x36, 2)
            )
        }
    }
    fn store(
        &mut self,
        cx: &mut Cx<'_>,
        offset: usize,
        size: u8,
        value: u32,
    ) -> Result<(), String> {
        let rom = self.options.protocol == Protocol::Rom;
        if offset == 0x24 || (16..32).contains(&offset) {
            return Err("SD response/present-state is read-only".into());
        }
        if offset == 0x20 {
            self.pio(cx, Some(value))?;
            return Ok(());
        }
        if offset == 0x30 && size == 4 {
            self.regs.set(0x30, self.regs.get(0x30) & !(value as u16));
            self.regs
                .set(0x32, self.regs.get(0x32) & !((value >> 16) as u16));
            return self.signal(cx);
        }
        if matches!(offset, 0x30 | 0x32) {
            self.regs
                .set(offset, self.regs.get(offset) & !(value as u16));
            return self.signal(cx);
        }
        if offset == 0x2e || offset == 0x2f {
            if value & if offset == 0x2f { !1 } else { !0x70f } != 0 {
                return Err("Unsupported SD reset/timeout bits".into());
            }
            if value & if offset == 0x2f { 1 } else { 0x700 } != 0 {
                self.request = None;
                self.rom_fifo_ready = false;
                self.regs.set(0x30, 0);
                self.regs.set(0x32, 0);
                if rom || offset == 0x2f || value & 0x100 != 0 {
                    self.regs = Registers::new();
                    self.response = [0; 16];
                    self.clocks.write(cx, self.options.base + 0x2c, 2, 0);
                }
                self.signal(cx)?;
            }
            if offset == 0x2e {
                self.regs.set(offset, value as u16 & 15);
            }
            return Ok(());
        }
        if offset == 14 || offset == 12 && size == 4 {
            return self.start(
                cx,
                if size == 4 {
                    value
                } else {
                    (value << 16) | self.regs.get(12) as u32
                },
            );
        }
        if offset == 0 && self.request.is_some() {
            let r = self.request.as_mut().unwrap();
            if r.phase != Phase::Boundary
                || self.regs.get(0x30) & 8 != 0
                || value != self.regs.word(0)
            {
                return Err("SDMA resume requires boundary ACK and current address".into());
            }
            r.phase = Phase::Command;
            r.due = Some(
                cx.now()
                    .checked_add(self.options.transfer_ticks)
                    .ok_or("SD deadline overflow")?,
            );
            return Ok(());
        }
        if self.request.is_some() {
            return Err("SD configuration changed during command".into());
        }
        if offset == 0x28 && !rom && value & !0xfff != 0
            || offset == 4 && value & 0x8000 != 0
            || offset == 0x3e && value != 0
            || offset == 0x100 && value & !7 != 0
            || offset == 0x104 && value != 0
        {
            return Err("Unsupported SD host/UHS/tuning configuration".into());
        }
        match size {
            4 => self.regs.put_word(offset, value),
            2 => self.regs.set(offset, value as u16),
            1 => {
                let shift = (offset % 2) * 8;
                let old = self.regs.get(offset & !1);
                self.regs.set(
                    offset & !1,
                    (old & !(255 << shift)) | ((value as u16) << shift),
                );
            }
            _ => unreachable!(),
        }
        self.signal(cx)
    }
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    card: Card,
    reads: StorageReads,
    contacts: u8,
    initialization_fault: InitializationFault,
    regs: Registers,
    clocks: Clocks,
    state: State,
    app: bool,
    block_length: usize,
    bus_width: u8,
    response: [u8; 16],
    request: Option<Request>,
    #[serde(with = "fujimod_emulator_devices::checkpoint::array")]
    fifo: [u8; 512],
    #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")]
    scratch: Box<[u8]>,
    fifo_position: usize,
    rom_fifo_ready: bool,
    faults: Vec<DataFault>,
    fault_history: Vec<DataFaultEvent>,
}
impl Device for SdHost {
    fn card_slot(&self) -> Option<&(dyn fujimod_emulator_devices::capability::CardSlot + 'static)> {
        Some(self)
    }
    fn card_slot_mut(
        &mut self,
    ) -> Option<&mut (dyn fujimod_emulator_devices::capability::CardSlot + 'static)> {
        Some(self)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        let (fifo, fifo_position) = self.fifo.snapshot();
        #[derive(Serialize)]
        struct View<'a> {
            card: &'a Card,
            reads: &'a StorageReads,
            contacts: u8,
            initialization_fault: InitializationFault,
            regs: &'a Registers,
            clocks: &'a Clocks,
            state: State,
            app: bool,
            block_length: usize,
            bus_width: u8,
            response: [u8; 16],
            request: &'a Option<Request>,
            fifo: &'a [u8],
            #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")]
            scratch: &'a [u8],
            fifo_position: usize,
            rom_fifo_ready: bool,
            faults: &'a [DataFault],
            fault_history: &'a [DataFaultEvent],
        }
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &View {
                card: &self.card,
                reads: &self.reads,
                contacts: self.contacts.checkpoint_level(),
                initialization_fault: self.options.initialization_fault,
                regs: &self.regs,
                clocks: &self.clocks,
                state: self.state,
                app: self.app,
                block_length: self.block_length,
                bus_width: self.bus_width,
                response: self.response,
                request: &self.request,
                fifo: &fifo,
                scratch: &self.scratch,
                fifo_position,
                rom_fifo_ready: self.rom_fifo_ready,
                faults: &self.faults,
                fault_history: &self.fault_history,
            },
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let state: Snapshot = saved.decode(self.name())?;
        if state.initialization_fault != self.options.initialization_fault {
            return Err("SD initialization fault policy differs".into());
        }
        state
            .reads
            .validate(u64::from(state.card.sectors()) * 512)?;
        let mut clocks = Clocks::new(&[(self.options.base + 0x2c, self.options.clock_ticks)])?;
        clocks.restore(&state.clocks.save())?;
        self.fifo.validate(&state.fifo, state.fifo_position)
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let s: Snapshot = saved.decode(self.name())?;
        self.fifo.restore(&s.fifo, s.fifo_position)?;
        self.card = s.card;
        self.reads = s.reads;
        self.contacts.restore_level(s.contacts);
        self.regs = s.regs;
        self.clocks = s.clocks;
        self.state = s.state;
        self.app = s.app;
        self.block_length = s.block_length;
        self.bus_width = s.bus_width;
        self.response = s.response;
        self.request = s.request;
        self.scratch = s.scratch;
        self.rom_fifo_ready = s.rom_fifo_ready;
        self.faults = s.faults;
        self.fault_history = s.fault_history;
        self.publish_reads();
        Ok(())
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        let (fifo, fifo_position) = self.fifo.snapshot();
        let request = self.request.as_ref().map(|r| {
            let payload = match r.payload {
                Payload::None => serde_json::json!({"kind":"none"}),
                Payload::Read(lba) => serde_json::json!({"kind":"read","lba":lba}),
                Payload::Write(lba) => serde_json::json!({"kind":"write","lba":lba}),
                Payload::Metadata { bytes, len } => serde_json::json!({"kind":"metadata","bytes":fujimod_emulator_devices::fingerprint::bytes(&bytes),"len":len}),
            };
            serde_json::json!({"state":r.state,"app":r.app,"block_length":r.block_length,"bus_width":r.bus_width,"response":r.response,
                "payload":payload,"transfer":r.transfer,"quantity":r.quantity,"cursor":r.cursor,"address":r.address,"phase":r.phase,
                "due":r.due,"command":r.command,"fault":r.fault})
        });
        Ok(
            serde_json::json!({"reads":self.reads,"card":self.card.fingerprint(),"contacts":[self.contacts.present(),self.contacts.protected()],"registers":self.regs.0.as_slice(),
            "clocks":self.clocks.save(),"state":self.state,"app":self.app,"block_length":self.block_length,"bus_width":self.bus_width,
            "response":self.response,"request":request,"fifo":fujimod_emulator_devices::fingerprint::bytes(&fifo),"scratch":fujimod_emulator_devices::fingerprint::bytes(&self.scratch),
            "fifo_position":fifo_position,"rom_fifo_ready":self.rom_fifo_ready,
            "initialization_fault":self.options.initialization_fault,"faults":self.faults,"fault_history":self.fault_history}),
        )
    }
    fn name(&self) -> &'static str {
        "sdhci-memory-card"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.fifo.publish(false);
        let value = self.read_register(cx, address, size);
        if cx.refusal().is_none() {
            self.publish_reads();
        } else {
            self.fifo.publish(false);
        }
        value
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.fifo.publish(false);
        if address == self.options.base + 0x2c && matches!(size, 2 | 4) {
            if value > u32::MAX as u64
                || size == 2 && value > u16::MAX as u64
                || size == 4 && (value >> 16) & !0x70f != 0
            {
                cx.unmodeled();
                return;
            }
            if self.request.is_some() {
                cx.fail("SD clock changed during command");
            } else {
                self.clocks.write(cx, address, 2, value & 0xffff);
                if size == 4 && cx.refusal().is_none() {
                    if let Err(e) = self.store(cx, 0x2e, 2, (value >> 16) as u32) {
                        cx.fail(e);
                    }
                }
            }
            if cx.refusal().is_none() {
                self.publish_reads();
            }
            return;
        }
        let offset = address.wrapping_sub(self.options.base) as usize;
        if !self.valid(offset, size)
            || value
                > if size == 4 {
                    u32::MAX as u64
                } else {
                    (1u64 << (size * 8)) - 1
                }
        {
            cx.unmodeled();
            return;
        }
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        if let Err(reason) = self.store(cx, offset, size, value as u32) {
            cx.fail(reason);
        }
        if cx.refusal().is_none() {
            self.publish_reads();
        } else {
            self.fifo.publish(false);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.fifo.publish(false);
        self.clocks.advance(cx);
        if let Err(reason) = self.complete(cx) {
            cx.fail(reason);
        }
        if cx.refusal().is_none() {
            self.publish_reads();
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.request
            .as_ref()
            .and_then(|r| r.due)
            .into_iter()
            .chain(self.clocks.next_deadline(now))
            .min()
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({
            "reads": self.reads,
            "card": {"present":self.contacts.present(), "protected":self.contacts.protected(),
                "sectors":self.card.sectors(), "revision":self.card.revision(),
                "dirty_sectors":self.card.dirty_sectors()},
            "busy":self.busy(), "response":self.response, "registers":self.regs.0.as_slice(),
            "request":self.request.as_ref().map(|r| serde_json::json!({"command":r.command,
                "cursor":r.cursor,"quantity":r.quantity,"deadline":r.due})),
            "clocks":self.clocks.save(), "initialization_fault":self.options.initialization_fault,
            "data_faults":self.faults, "fault_history":self.fault_history
        })
    }
    fn fast_reads(&self, window_base: u64, window_size: u64) -> Vec<FastReadWord> {
        let address = self.options.base + 0x20;
        let end = window_base.saturating_add(window_size);
        let mut reads = Vec::new();
        if address >= window_base && address + 4 <= end {
            let fifo = self.fifo.clone();
            reads.push(FastReadWord::consuming_u32(
                address - window_base,
                Arc::new(move || fifo.read_fast()),
            ));
        }
        for (i, value) in self.status_reads.iter().enumerate() {
            let address = self.options.base + 0x30 + i as u64 * 2;
            if address >= window_base && address + 2 <= end {
                reads.push(FastReadWord::atomic_u16(
                    address - window_base,
                    value.clone(),
                ));
            }
        }
        reads
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        address != self.options.base + 0x20
            && (self.valid(address.wrapping_sub(self.options.base) as usize, size)
                || address == self.options.base + 0x2c && matches!(size, 2 | 4))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: u64 = 0xfff50000;
    use fujimod_emulator_devices::parts::sd_card;
    use fujimod_emulator_devices::test_host::ram::{Effect, Ram};
    #[test]
    fn word_clock_timeout_status_and_enable_lanes_are_coherent() {
        let mut m = host(Protocol::Sdhc);
        let mut h = Ram::new(0x10000);
        write(&mut m, &mut h, 0x2c, 4, 0xe0001);
        h.now = 2;
        assert_eq!(read(&mut m, &mut h, 0x2c, 4), 0xe0003);
        assert!(m.pollable(BASE + 0x2c, 4));
        m.regs.set(0x30, 0x20);
        m.regs.set(0x32, 2);
        write(&mut m, &mut h, 0x30, 4, 0x20020);
        assert_eq!(read(&mut m, &mut h, 0x30, 4), 0);
        write(&mut m, &mut h, 0x34, 4, u32::MAX.into());
        assert_eq!(read(&mut m, &mut h, 0x34, 4), u64::from(u32::MAX));
        m.regs.set(0x30, 0x20);
        m.regs.set(0x32, 2);
        m.publish_reads();
        write(&mut m, &mut h, 0x2c, 4, 0x1000000);
        assert_eq!(m.status_reads[0].load(Ordering::Acquire), 0);
        assert_eq!(m.status_reads[1].load(Ordering::Acquire), 0);
        assert_eq!(read(&mut m, &mut h, 0x34, 4), 0);
        assert_eq!(read(&mut m, &mut h, 0x2c, 4), 0);
    }
    fn host(protocol: Protocol) -> SdHost {
        host_at(protocol, BASE)
    }
    fn host_at(protocol: Protocol, base: u64) -> SdHost {
        SdHost::new(
            Options {
                base,
                protocol,
                transfer_ticks: 10,
                clock_ticks: 2,
                rca: 0x1234,
                ram_bytes: 0x10000,
                initialization_fault: InitializationFault::None,
            },
            Wiring {
                card: Card::new(sd_card::Options {
                    sectors: 0x800000,
                    max_dirty_sectors: 65536,
                })
                .unwrap(),
                contacts: Contacts::new(true, false),
                irq: InterruptLine::Shared(78),
            },
        )
        .unwrap()
    }
    #[test]
    fn rca_commands_ignore_reserved_bits_but_keep_card_selection() {
        let mut m = host(Protocol::Sdhc);
        m.state = State::Standby;
        for word in [0x071b0000, 0x371a0000, 0x09090000] {
            assert!(m.plan(word, 0x12340600).is_ok());
            assert!(m.plan(word, 0x12350600).is_err());
        }
    }
    #[test]
    fn ocr_inquiry_keeps_idle_until_a_supported_voltage_is_selected() {
        let mut m = host(Protocol::Sdhc);
        let mut h = Ram::new(0x10000);
        setup(&mut m, &mut h);
        for argument in [0, 0xff000000] {
            command(&mut m, &mut h, 0x371a0000, 0);
            due(&mut m, &mut h);
            command(&mut m, &mut h, 0x29020000, argument);
            due(&mut m, &mut h);
            assert_eq!(m.state, State::Idle);
            assert_eq!(read(&mut m, &mut h, 0x10, 4), 0x00ff8000);
        }
        command(&mut m, &mut h, 0x371a0000, 0);
        due(&mut m, &mut h);
        for argument in [0x51000100, 0x11100000, 0x71ff8000] {
            assert!(m.plan(0x29020000, argument).is_err());
        }
        command(&mut m, &mut h, 0x29020000, 0x51ff8000);
        due(&mut m, &mut h);
        assert_eq!(m.state, State::Ready);
        assert_eq!(read(&mut m, &mut h, 0x10, 4), 0xc0ff8000);
        command(&mut m, &mut h, 0x02090000, 0);
        due(&mut m, &mut h);
        let mut cid = Vec::new();
        for offset in [0x10, 0x14, 0x18, 0x1c] {
            let value = read(&mut m, &mut h, offset, 4);
            let halves =
                read(&mut m, &mut h, offset, 2) | read(&mut m, &mut h, offset + 2, 2) << 16;
            assert_eq!(value, halves);
            cid.extend_from_slice(&(value as u32).to_le_bytes());
        }
        assert_eq!(
            cid,
            (u128::from_be_bytes(*b"\xfeMCVIRT0\x10\x12\x34\x56\x78\x01\xaa\xff") >> 8)
                .to_le_bytes()
        );
    }
    #[test]
    fn combined_single_read_uses_current_transfer_mode_not_previous_command() {
        let mut m = host(Protocol::Sdhc);
        m.state = State::Transfer;
        m.regs.set(6, 1);
        m.regs.set(12, 0);
        assert!(matches!(
            m.plan(0x113a0010, 0).unwrap().transfer,
            Transfer::PioRead
        ));
        assert!(matches!(
            m.plan(0x113a0011, 0).unwrap().transfer,
            Transfer::Dma
        ));
        m.regs.set(12, 0xffff);
        assert!(matches!(
            m.plan(0x113a0010, 0).unwrap().transfer,
            Transfer::PioRead
        ));
    }
    #[test]
    fn unconnected_output_allows_polling_and_refuses_an_asserted_interrupt() {
        let mut m = host(Protocol::Sdhc);
        m.irq = None;
        let mut h = Ram::new(0x10000);
        m.regs.set(0x30, 1);
        assert!(m.signal(&mut h.cx()).is_ok());
        m.regs.set(0x34, 1);
        m.regs.set(0x38, 1);
        assert_eq!(
            m.signal(&mut h.cx()).unwrap_err(),
            "SD interrupt output is unconnected"
        );
    }
    #[test]
    fn base_wiring_moves_clocks_fifo_status_and_checkpoint_identity_together() {
        for base in [0xfff44000, BASE, 0x12340000] {
            let mut m = host_at(Protocol::Sdhc, base);
            let mut h = Ram::new(0x10000);
            m.write(&mut h.cx(), base + 0x2c, 2, 1);
            assert_eq!(h.cx().refusal(), None);
            h.now = 2;
            assert_eq!(m.read(&mut h.cx(), base + 0x2c, 2), 3);
            assert!(m.pollable(base + 0x30, 2));
            assert!(!m.pollable(base + 0x20, 4));
            assert_eq!(m.fast_reads(base, 0x1000).len(), 3);
            let saved = m.checkpoint().unwrap();
            let mut other = host_at(Protocol::Sdhc, base + 0x1000);
            assert!(other.validate_checkpoint(&saved).is_err());
            assert!(other.restore_checkpoint(&saved).is_err());
            m.restore_checkpoint(&saved).unwrap();
        }
    }
    #[test]
    fn fingerprint_distinguishes_media_with_the_same_diagnostic_summary() {
        let mut a = host(Protocol::Sdhc);
        let mut b = host(Protocol::Sdhc);
        a.card.write_bytes(512, &[1; 512]).unwrap();
        b.card.write_bytes(512, &[2; 512]).unwrap();
        assert_eq!(a.save(), b.save());
        assert_ne!(a.fingerprint().unwrap(), b.fingerprint().unwrap());
        let stable = a.fingerprint().unwrap();
        assert_eq!(a.fingerprint().unwrap(), stable);
        let mut bytes = [0; 512];
        a.card.read_bytes(512, &mut bytes).unwrap();
        assert_eq!(bytes, [1; 512]);
    }
    #[test]
    fn media_read_ledger_tracks_pio_and_dma_without_counting_host_inspection() {
        let mut m = host(Protocol::Sdhc);
        let mut h = Ram::new(0x10000);
        setup(&mut m, &mut h);
        m.state = State::Transfer;
        m.card.write_bytes(512, &[0x42; 1024]).unwrap();
        write(&mut m, &mut h, 4, 2, 512);
        write(&mut m, &mut h, 6, 2, 1);
        command(&mut m, &mut h, 0x113a0010, 1);
        due(&mut m, &mut h);
        for _ in 0..128 {
            assert_eq!(read(&mut m, &mut h, 0x20, 4), 0x42424242);
        }
        due(&mut m, &mut h);
        assert_eq!(m.reads.bytes, 512);
        assert_eq!(m.reads.ranges, [(512, 1024)]);
        let saved = m.checkpoint().unwrap();
        let fingerprint = m.fingerprint().unwrap();
        assert_eq!(m.card().sector(2).unwrap(), [0x42; 512]);
        assert_eq!(m.fingerprint().unwrap(), fingerprint);
        write(&mut m, &mut h, 0, 4, 0x40001000);
        write(&mut m, &mut h, 6, 2, 1);
        command(&mut m, &mut h, 0x113a0011, 2);
        due(&mut m, &mut h);
        assert_eq!(&h.bytes[0x1000..0x1200], &[0x42; 512]);
        assert_eq!(m.reads.bytes, 1024);
        assert_eq!(m.reads.ranges, [(512, 1536)]);
        m.contacts.set(false, false);
        assert_eq!(m.save()["reads"]["bytes"], 1024);
        m.restore_checkpoint(&saved).unwrap();
        assert_eq!(m.reads.bytes, 512);
        assert_eq!(m.reads.ranges, [(512, 1024)]);
        write(&mut m, &mut h, 0x2f, 1, 1);
        assert_eq!(m.reads.bytes, 512);
        m.replace_card(
            Card::new(sd_card::Options {
                sectors: 0x800000,
                max_dirty_sectors: 65536,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(m.reads, StorageReads::default());
    }
    fn write(m: &mut SdHost, h: &mut Ram, o: u64, s: u8, v: u64) {
        let mut cx = h.cx();
        m.write(&mut cx, BASE + o, s, v);
        assert_eq!(cx.refusal(), None, "offset {o:x}");
    }
    fn read(m: &mut SdHost, h: &mut Ram, o: u64, s: u8) -> u64 {
        let mut cx = h.cx();
        let v = m.read(&mut cx, BASE + o, s);
        assert_eq!(cx.refusal(), None);
        v
    }
    fn setup(m: &mut SdHost, h: &mut Ram) {
        write(m, h, 0x2c, 2, 5);
        h.now = 2;
        m.advance(&mut h.cx());
        if m.options.protocol == Protocol::Rom {
            write(m, h, 0x29, 1, 15);
        } else {
            write(m, h, 0x28, 2, 0x100);
        }
        write(m, h, 0x34, 2, 0xffff);
        if m.options.protocol == Protocol::Application {
            write(m, h, 0x38, 2, 0xffff);
        }
    }
    fn command(m: &mut SdHost, h: &mut Ram, word: u32, arg: u32) {
        write(m, h, 0x30, 2, 0xffff);
        if m.options.protocol == Protocol::Rom {
            write(m, h, 8, 4, arg as u64);
            write(m, h, 14, 2, (word >> 16) as u64);
        } else {
            write(m, h, 8, 2, (arg & 65535) as u64);
            write(m, h, 10, 2, (arg >> 16) as u64);
            write(m, h, 12, 4, word as u64);
        }
    }
    fn due(m: &mut SdHost, h: &mut Ram) {
        h.now = m.next_deadline(h.now).unwrap();
        let mut cx = h.cx();
        m.advance(&mut cx);
        assert_eq!(cx.refusal(), None);
    }
    #[test]
    fn enumeration_deadlines_clock_and_held_irq() {
        let mut m = host(Protocol::Application);
        let mut h = Ram::new(0x10000);
        setup(&mut m, &mut h);
        for (word, arg) in [
            (0, 0),
            (0x081a0000, 0x1aa),
            (0x371a0000, 0),
            (0x29020000, 0x41100000),
            (0x02090000, 0),
            (0x031a0000, 0),
            (0x09090000, 0x12340000),
            (0x071b0000, 0x12340000),
        ] {
            command(&mut m, &mut h, word, arg);
            let before = m.response;
            let tick = m.next_deadline(h.now).unwrap();
            h.now = tick - 1;
            m.advance(&mut h.cx());
            assert_eq!(m.response, before);
            due(&mut m, &mut h);
            assert_eq!(read(&mut m, &mut h, 0x30, 2) & 1, 1);
            assert_eq!(h.effects.last(), Some(&Effect::Irq(78, true)));
            write(&mut m, &mut h, 0x30, 2, 0xffff);
            assert_eq!(h.effects.last(), Some(&Effect::Irq(78, false)));
        }
        assert_eq!(m.state, State::Transfer);
        let mut cx = h.cx();
        m.write(&mut cx, BASE + 12, 4, 0x26000000);
        assert!(cx.refusal().is_some());
        assert!(!m.busy());
    }
    #[test]
    fn dma_boundary_requires_ack_resume_and_writes_before_irq() {
        let mut m = host(Protocol::Application);
        let mut h = Ram::new(0x10000);
        setup(&mut m, &mut h);
        m.state = State::Transfer;
        m.card.write_bytes(0, &vec![0x5a; 1024]).unwrap();
        write(&mut m, &mut h, 0, 4, 0xf00);
        write(&mut m, &mut h, 4, 2, 512);
        write(&mut m, &mut h, 6, 2, 2);
        command(&mut m, &mut h, 0x123a0037, 0);
        due(&mut m, &mut h);
        assert_eq!(m.next_deadline(h.now), None);
        assert_eq!(read(&mut m, &mut h, 0, 4), 0x1000);
        assert_eq!(&h.bytes[0xf00..0x1000], &[0x5a; 256]);
        assert_eq!(read(&mut m, &mut h, 6, 2), 2);
        assert_eq!(
            &h.effects[h.effects.len() - 2..],
            &[Effect::Write(0xf00, 256), Effect::Irq(78, true)]
        );
        let mut cx = h.cx();
        m.write(&mut cx, BASE, 4, 0x1000);
        assert!(cx.refusal().is_some());
        write(&mut m, &mut h, 0x30, 2, 9);
        write(&mut m, &mut h, 0, 4, 0x1000);
        due(&mut m, &mut h);
        assert_eq!(&h.bytes[0xf00..0x1300], &[0x5a; 1024]);
        assert_eq!(read(&mut m, &mut h, 6, 2), 0);
        assert!(!m.busy());
    }
    fn fault(direction: DataDirection, sector: Option<u32>, occurrences: u32) -> DataFault {
        DataFault {
            direction,
            sector,
            after_sectors: 0,
            occurrences,
        }
    }
    #[test]
    fn sdma_read_crc_fault_delivers_prefix_then_error_status_and_irq() {
        let mut m = host(Protocol::Application);
        let mut h = Ram::new(0x10000);
        setup(&mut m, &mut h);
        write(&mut m, &mut h, 0x36, 2, 0xffff);
        write(&mut m, &mut h, 0x3a, 2, 0xffff);
        m.state = State::Transfer;
        m.card.write_bytes(512 * 8, &[0x5a; 1024]).unwrap();
        m.inject_data_fault(fault(DataDirection::Read, Some(9), 1))
            .unwrap();
        write(&mut m, &mut h, 0, 4, 0x1000);
        write(&mut m, &mut h, 4, 2, 512);
        write(&mut m, &mut h, 6, 2, 2);
        command(&mut m, &mut h, 0x123a0037, 8);
        assert!(m.data_faults().is_empty());
        let checkpoint = m.checkpoint().unwrap();
        due(&mut m, &mut h);
        assert!(!m.busy());
        assert_eq!(&h.bytes[0x1000..0x1200], &[0x5a; 512]);
        assert_eq!(&h.bytes[0x1200..0x1400], &[0; 512]);
        assert_eq!(read(&mut m, &mut h, 0x32, 2), 0x20);
        assert_eq!(read(&mut m, &mut h, 0x30, 2), 0x8000);
        assert_eq!(read(&mut m, &mut h, 6, 2), 1);
        assert_eq!(h.effects.last(), Some(&Effect::Irq(78, true)));
        assert_eq!(
            m.fault_history(),
            [DataFaultEvent {
                command: 18,
                sector: 8,
                transferred: 512
            }]
        );
        write(&mut m, &mut h, 0x32, 2, 0xffff);
        write(&mut m, &mut h, 0x30, 2, 0xffff);
        assert_eq!(h.effects.last(), Some(&Effect::Irq(78, false)));
        // A retry is a fresh transfer; the exhausted rule no longer applies.
        write(&mut m, &mut h, 0, 4, 0x1000);
        write(&mut m, &mut h, 6, 2, 2);
        command(&mut m, &mut h, 0x123a0037, 8);
        due(&mut m, &mut h);
        assert_eq!(&h.bytes[0x1000..0x1400], &[0x5a; 1024]);
        assert_eq!(read(&mut m, &mut h, 0x32, 2), 0);
        assert_eq!(read(&mut m, &mut h, 0x30, 2), 3);
        // The checkpoint retains the armed request and fault position.
        let mut restored = host(Protocol::Application);
        restored.restore_checkpoint(&checkpoint).unwrap();
        let mut g = Ram::new(0x10000);
        g.now = h.now;
        due(&mut restored, &mut g);
        assert_eq!(restored.regs.get(0x32), 0x20);
        assert_eq!(restored.fault_history().len(), 1);
    }
    #[test]
    fn write_crc_faults_apply_to_sdma_prefixes_and_pio_command_start() {
        let mut m = host(Protocol::Application);
        let mut h = Ram::new(0x10000);
        setup(&mut m, &mut h);
        m.state = State::Transfer;
        h.bytes[0x1000..0x1400].fill(0x77);
        m.inject_data_fault(DataFault {
            direction: DataDirection::Write,
            sector: None,
            after_sectors: 1,
            occurrences: 1,
        })
        .unwrap();
        m.inject_data_fault(fault(DataDirection::Write, None, 1))
            .unwrap();
        assert!(
            m.inject_data_fault(fault(DataDirection::Read, None, 0))
                .is_err()
        );
        write(&mut m, &mut h, 0, 4, 0x1000);
        write(&mut m, &mut h, 4, 2, 512);
        write(&mut m, &mut h, 6, 2, 2);
        command(&mut m, &mut h, 0x193a0027, 4);
        due(&mut m, &mut h);
        assert_eq!(m.card.sector(4).unwrap(), [0x77; 512]);
        assert_eq!(m.card.sector(5).unwrap(), [0; 512]);
        assert_eq!(read(&mut m, &mut h, 0x32, 2), 0x20);
        write(&mut m, &mut h, 0x32, 2, 0xffff);
        write(&mut m, &mut h, 6, 2, 1);
        command(&mut m, &mut h, 0x193a0026, 6);
        due(&mut m, &mut h);
        assert!(!m.busy());
        assert_eq!(read(&mut m, &mut h, 0x30, 2), 0x8000);
        assert_eq!(read(&mut m, &mut h, 0x32, 2), 0x20);
        assert_eq!(m.fault_history()[1].transferred, 0);
        assert!(m.data_faults().is_empty());
        // A PIO failure after a committed prefix is not modeled.
        write(&mut m, &mut h, 0x32, 2, 0xffff);
        write(&mut m, &mut h, 0x30, 2, 0xffff);
        m.inject_data_fault(fault(DataDirection::Write, Some(7), 1))
            .unwrap();
        write(&mut m, &mut h, 6, 2, 2);
        write(&mut m, &mut h, 8, 2, 6);
        write(&mut m, &mut h, 10, 2, 0);
        let mut cx = h.cx();
        m.write(&mut cx, BASE + 12, 4, 0x193a0026);
        assert!(cx.refusal().is_some());
    }
    #[test]
    fn published_pio_prefix_matches_callbacks_and_restores_its_shared_cursor() {
        for protocol in [Protocol::Rom, Protocol::Application, Protocol::Sdhc] {
            let mut normal = host(protocol);
            let mut fast = host(protocol);
            let mut a = Ram::new(0x10000);
            let mut b = Ram::new(0x10000);
            for (m, h) in [(&mut normal, &mut a), (&mut fast, &mut b)] {
                setup(m, h);
                if protocol != Protocol::Rom {
                    write(m, h, 0x38, 2, 0xffff);
                }
                m.state = State::Transfer;
                let bytes = (0..128u32).flat_map(u32::to_le_bytes).collect::<Vec<_>>();
                m.card.write_bytes(0, &bytes).unwrap();
                write(m, h, 4, 2, 512);
                write(m, h, 6, 2, 1);
                if protocol == Protocol::Rom {
                    write(m, h, 12, 2, 0x10);
                    command(m, h, 0x113a0000, 0);
                } else {
                    command(m, h, 0x123a0036, 0);
                }
                due(m, h);
            }
            let words = fast.fast_reads(BASE, 0x1000);
            let status = fast.fast_reads(BASE + 0x30, 4);
            assert_eq!(
                status
                    .iter()
                    .map(|r| (r.offset, r.value.width()))
                    .collect::<Vec<_>>(),
                [(0, 2), (2, 2)]
            );
            let fujimod_emulator_devices::bus::FastReadValue::AtomicU16(normal_status) =
                &status[0].value
            else {
                panic!("normal status is not a halfword snapshot");
            };
            let fujimod_emulator_devices::bus::FastReadValue::AtomicU16(error_status) =
                &status[1].value
            else {
                panic!("error status is not a halfword snapshot");
            };
            let fujimod_emulator_devices::bus::FastReadValue::ConsumingU32(consume) =
                &words[0].value
            else {
                panic!("PIO FIFO did not publish a consuming word");
            };
            // Neither callback nor published FIFO access requires clearing the
            // Buffer Read Ready interrupt before consuming available data.
            for word in 0..127u32 {
                assert_eq!(read(&mut normal, &mut a, 0x20, 4), u64::from(word));
                assert_eq!(consume(), Some(word));
                assert_eq!(normal.fingerprint().unwrap(), fast.fingerprint().unwrap());
                assert_eq!(normal.next_deadline(a.now), fast.next_deadline(b.now));
                assert_eq!(a.effects, b.effects);
                assert_eq!(normal_status.load(Ordering::Acquire), normal.regs.get(0x30));
                assert_eq!(error_status.load(Ordering::Acquire), normal.regs.get(0x32));
                if word == 63 {
                    let saved = fast.checkpoint().unwrap();
                    assert_eq!(consume(), Some(64));
                    fast.restore_checkpoint(&saved).unwrap();
                    // The descriptor captured before restore keeps the same cursor.
                    assert_eq!(normal.fingerprint().unwrap(), fast.fingerprint().unwrap());
                }
            }
            assert_eq!(
                consume(),
                None,
                "completion word must use the device callback"
            );
            assert_eq!(read(&mut normal, &mut a, 0x20, 4), 127);
            assert_eq!(read(&mut fast, &mut b, 0x20, 4), 127);
            assert_eq!(consume(), None);
            assert_eq!(normal.fingerprint().unwrap(), fast.fingerprint().unwrap());
            assert_eq!(a.effects, b.effects);
            if protocol != Protocol::Rom {
                due(&mut normal, &mut a);
                due(&mut fast, &mut b);
            }
            assert_eq!(normal.fingerprint().unwrap(), fast.fingerprint().unwrap());
            assert_eq!(a.effects, b.effects);
            assert!(!fast.busy());
            assert_eq!(normal_status.load(Ordering::Acquire), normal.regs.get(0x30));
            assert_eq!(error_status.load(Ordering::Acquire), normal.regs.get(0x32));
            if protocol == Protocol::Rom {
                write(&mut fast, &mut b, 0x2f, 1, 1);
            } else {
                write(&mut fast, &mut b, 0x2e, 2, 0x700);
            }
            assert_eq!(normal_status.load(Ordering::Acquire), 0);
            assert_eq!(error_status.load(Ordering::Acquire), 0);
        }
    }
    #[test]
    fn pio_sector_latency_reset_and_write_protection() {
        let mut m = host(Protocol::Application);
        let mut h = Ram::new(0x10000);
        setup(&mut m, &mut h);
        m.state = State::Transfer;
        write(&mut m, &mut h, 4, 2, 512);
        write(&mut m, &mut h, 6, 2, 1);
        command(&mut m, &mut h, 0x193a0026, 4);
        let mut cx = h.cx();
        m.write(&mut cx, BASE + 0x20, 4, 0xaa);
        assert!(cx.refusal().is_some());
        due(&mut m, &mut h);
        for _ in 0..128 {
            write(&mut m, &mut h, 0x20, 4, 0x12345678);
        }
        assert_eq!(read(&mut m, &mut h, 0x30, 2) & 0x10, 0x10);
        assert_eq!(m.card.sector(4).unwrap(), [0; 512]);
        due(&mut m, &mut h);
        assert_eq!(&m.card.sector(4).unwrap()[..4], &[0x78, 0x56, 0x34, 0x12]);
        write(&mut m, &mut h, 0x30, 2, 0xffff);
        write(&mut m, &mut h, 6, 2, 1);
        command(&mut m, &mut h, 0x123a0036, 4);
        due(&mut m, &mut h);
        write(&mut m, &mut h, 0x30, 2, 0x21);
        for _ in 0..128 {
            assert_eq!(read(&mut m, &mut h, 0x20, 4), 0x12345678);
        }
        assert!(m.busy());
        due(&mut m, &mut h);
        assert!(!m.busy());
        write(&mut m, &mut h, 0x30, 2, 0xffff);
        command(&mut m, &mut h, 0x193a0026, 4);
        write(&mut m, &mut h, 0x2e, 2, 0x700);
        assert_eq!(m.next_deadline(h.now), None);
        m.contacts.set(true, true);
        let mut cx = h.cx();
        m.write(&mut cx, BASE + 12, 4, 0x193a0026);
        assert!(cx.refusal().is_some());
    }
    #[test]
    fn combined_host_keeps_media_across_rom_and_application_register_accesses() {
        let mut m = host(Protocol::Sdhc);
        let mut h = Ram::new(0x10000);
        setup(&mut m, &mut h);
        write(&mut m, &mut h, 0x29, 1, 15);
        write(&mut m, &mut h, 0x38, 2, 0);
        for (command, argument) in [
            (0u16, 0u32),
            (0x081a, 0x1aa),
            (0x371a, 0),
            (0x2902, 0x40100000),
            (0x0209, 0),
            (0x031a, 0),
            (0x071b, 0x12340000),
        ] {
            write(&mut m, &mut h, 0x30, 2, 0xffff);
            write(&mut m, &mut h, 8, 4, argument as u64);
            write(&mut m, &mut h, 12, 2, 0);
            write(&mut m, &mut h, 14, 2, command as u64);
            due(&mut m, &mut h);
        }
        assert_eq!(m.state, State::Transfer);
        m.card.write_bytes(0, &[0x42; 512]).unwrap();
        write(&mut m, &mut h, 0x30, 2, 0xffff);
        write(&mut m, &mut h, 4, 2, 512);
        write(&mut m, &mut h, 6, 2, 1);
        write(&mut m, &mut h, 8, 4, 0);
        write(&mut m, &mut h, 12, 2, 0x10);
        write(&mut m, &mut h, 14, 2, 0x113a);
        due(&mut m, &mut h);
        for _ in 0..128 {
            assert_eq!(read(&mut m, &mut h, 0x20, 4), 0x42424242);
        }
        due(&mut m, &mut h);
        assert!(!m.busy());
        write(&mut m, &mut h, 0x38, 2, 0xffff);
        command(&mut m, &mut h, 0, 0);
        due(&mut m, &mut h);
        assert_eq!(m.state, State::Idle);
        assert_eq!(m.card.sector(0).unwrap(), [0x42; 512]);
    }
    #[test]
    fn busy_select_ends_with_enabled_transfer_complete() {
        for (enabled, status) in [(0xffff, 3), (0xfffd, 1)] {
            let mut m = host(Protocol::Sdhc);
            let mut h = Ram::new(0x10000);
            setup(&mut m, &mut h);
            write(&mut m, &mut h, 0x34, 2, enabled);
            m.state = State::Standby;
            command(&mut m, &mut h, 0x071b0000, 0x12340000);
            due(&mut m, &mut h);
            assert_eq!(read(&mut m, &mut h, 0x30, 2), status);
            assert_eq!(m.state, State::Transfer);
        }
    }
    #[test]
    fn rom_single_sector_pio_and_alias_dma() {
        let mut m = host(Protocol::Rom);
        let mut h = Ram::new(0x10000);
        setup(&mut m, &mut h);
        m.state = State::Transfer;
        m.card.write_bytes(0, &[0x7b; 512]).unwrap();
        write(&mut m, &mut h, 4, 2, 512);
        write(&mut m, &mut h, 6, 2, 1);
        write(&mut m, &mut h, 12, 2, 0x10);
        command(&mut m, &mut h, 0x113a0000, 0);
        due(&mut m, &mut h);
        assert_eq!(read(&mut m, &mut h, 0x30, 2), 0x21);
        for _ in 0..128 {
            assert_eq!(read(&mut m, &mut h, 0x20, 4), 0x7b7b7b7b);
        }
        assert_eq!(read(&mut m, &mut h, 0x30, 2), 0x23);
        write(&mut m, &mut h, 0, 4, 0x40001000);
        write(&mut m, &mut h, 12, 2, 0x11);
        command(&mut m, &mut h, 0x113a0000, 0);
        due(&mut m, &mut h);
        assert!(h.effects.contains(&Effect::Write(0x40001000, 512)));
        assert_eq!(&h.bytes[0x1000..0x1200], &[0x7b; 512]);
    }
}

impl fujimod_emulator_devices::capability::CardSlot for SdHost {
    fn card(&self) -> &fujimod_emulator_devices::parts::sd_card::Card {
        SdHost::card(self)
    }
    fn busy(&self) -> bool {
        SdHost::busy(self)
    }
    fn readable_card(
        &self,
        revision: Option<u64>,
    ) -> std::result::Result<&fujimod_emulator_devices::parts::sd_card::Card, String> {
        SdHost::readable_card(self, revision)
    }
    fn replace_card(
        &mut self,
        card: fujimod_emulator_devices::parts::sd_card::Card,
    ) -> std::result::Result<fujimod_emulator_devices::parts::sd_card::Card, String> {
        SdHost::replace_card(self, card)
    }
    fn retain_card(
        &mut self,
        card: fujimod_emulator_devices::parts::sd_card::Card,
    ) -> std::result::Result<(), String> {
        SdHost::retain_card(self, card)
    }
    fn initialize_revision(&mut self, revision: u64) -> std::result::Result<(), String> {
        SdHost::initialize_revision(self, revision)
    }
    fn inject_data_fault(&mut self, fault: DataFault) -> std::result::Result<(), String> {
        SdHost::inject_data_fault(self, fault)
    }
    fn clear_data_faults(&mut self) -> std::result::Result<(), String> {
        SdHost::clear_data_faults(self)
    }
    fn data_faults(&self) -> &[DataFault] {
        SdHost::data_faults(self)
    }
    fn fault_history(&self) -> &[DataFaultEvent] {
        SdHost::fault_history(self)
    }
}
