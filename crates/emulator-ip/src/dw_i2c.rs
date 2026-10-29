//! DesignWare APB I2C master (DW_apb_i2c): FIFOs, interrupts, aborts and
//! byte timing. Attached targets are a separate [`Bus`].
//!
//! Each DATA_CMD word takes effect on the bus when it is written: the address
//! phase, the target's byte and the stop bit. Its visible results (the TX FIFO
//! level, received bytes, START_DET/STOP_DET/TX_ABRT and the abort source)
//! appear when the byte completes on the wire. A byte takes nine SCL periods,
//! plus one for a start or repeated start with its address byte and one for a
//! stop; the period is the HCNT+LCNT pair of the speed selected in IC_CON,
//! converted from the declared `ic_clk` to device ticks. The FIFO depth is a
//! declared input. A NACKed address aborts with ABRT_7B_ADDR_NOACK: the TX
//! FIFO is flushed and later writes are dropped until TX_ABRT is cleared.
//! IC_ENABLE.ABORT aborts with ABRT_USER_ABRT. Configuration words read back
//! their last write; reset values, slave mode, 10-bit addressing, high speed,
//! DMA and data NACKs are not modeled. TX_EMPTY asserts only while enabled.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// Register window size in bytes.
pub const WINDOW: u64 = 0x100;

const CON: u64 = 0x00;
const TAR: u64 = 0x04;
const DATA_CMD: u64 = 0x10;
const SS_SCL: u64 = 0x14;
const FS_SCL: u64 = 0x1c;
const INTR_STAT: u64 = 0x2c;
const INTR_MASK: u64 = 0x30;
const RAW_INTR_STAT: u64 = 0x34;
const RX_TL: u64 = 0x38;
const TX_TL: u64 = 0x3c;
const CLR_INTR: u64 = 0x40;
const ENABLE: u64 = 0x6c;
const STATUS: u64 = 0x70;
const TXFLR: u64 = 0x74;
const RXFLR: u64 = 0x78;
const TX_ABRT_SOURCE: u64 = 0x80;
const ENABLE_STATUS: u64 = 0x9c;
/// Configuration words that read back their last write.
const WORDS: [u64; 21] = [
    CON, TAR, 0x08, 0x0c, 0x14, 0x18, 0x1c, 0x20, 0x24, 0x28, INTR_MASK, RX_TL, TX_TL, 0x7c, 0x88,
    0x8c, 0x90, 0x94, 0x98, 0xa0, 0xa4,
];

const RX_UNDER: u32 = 1;
const RX_OVER: u32 = 1 << 1;
const RX_FULL: u32 = 1 << 2;
const TX_OVER: u32 = 1 << 3;
const TX_EMPTY: u32 = 1 << 4;
const TX_ABRT: u32 = 1 << 6;
const ACTIVITY: u32 = 1 << 8;
const STOP_DET: u32 = 1 << 9;
const START_DET: u32 = 1 << 10;
/// CLR_RX_UNDER (0x44) through CLR_GEN_CALL (0x68), one latched bit each.
const CLEARS: [(u64, u32); 10] = [
    (0x44, RX_UNDER),
    (0x48, RX_OVER),
    (0x4c, TX_OVER),
    (0x50, 1 << 5),
    (0x54, TX_ABRT),
    (0x58, 1 << 7),
    (0x5c, ACTIVITY),
    (0x60, STOP_DET),
    (0x64, START_DET),
    (0x68, 1 << 11),
];

const CMD_READ: u32 = 1 << 8;
const CMD_STOP: u32 = 1 << 9;
const CMD_RESTART: u32 = 1 << 10;
const RESTART_EN: u32 = 1 << 5;
const ENABLED: u32 = 1;
const ABORT: u32 = 1 << 1;
const ABRT_7B_ADDR_NOACK: u32 = 1;
const ABRT_USER_ABRT: u32 = 1 << 16;

/// Targets on the wires of one controller.
pub trait Bus: Send + 'static {
    /// The address byte of a start or repeated start; `false` is a NACK.
    fn start(&mut self, address: u16, read: bool) -> bool;
    fn write(&mut self, byte: u8);
    fn read(&mut self) -> Result<u8, String>;
    fn stop(&mut self);
    fn save(&self) -> Value;
    fn restore(&mut self, value: &Value) -> Result<(), String>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Options {
    pub base: u64,
    pub irq: u32,
    pub fifo_depth: usize,
    /// The `ic_clk` the SCL counts divide.
    pub clock_hz: u64,
    pub tick_hz: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    ticks: u64,
    start: bool,
    stop: bool,
    nack: bool,
    /// Written while an earlier NACK was pending; the abort flushes it.
    discarded: bool,
    byte: Option<u8>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    enable: u32,
    tx: VecDeque<Command>,
    rx: VecDeque<u8>,
    latched: u32,
    abort_source: u32,
    deadline: Option<u64>,
    /// A target acknowledged its address and has not seen a stop.
    open: bool,
    reading: bool,
    /// The bus is between a visible start and stop.
    wire: bool,
    /// An abort is pending or latched; DATA_CMD writes are dropped.
    flushing: bool,
    line: bool,
}

pub struct Controller<B: Bus> {
    options: Options,
    bus: B,
    state: State,
}

impl<B: Bus> Controller<B> {
    pub fn new(options: Options, bus: B) -> Result<Self, String> {
        if options.base & 0xff != 0
            || !(1..=256).contains(&options.fifo_depth)
            || options.clock_hz == 0
            || options.tick_hz == 0
        {
            return Err("Invalid DesignWare I2C base, FIFO depth or clock".into());
        }
        Ok(Self {
            options,
            bus,
            state: State::default(),
        })
    }
    pub fn bus(&self) -> &B {
        &self.bus
    }
    fn word(&self, offset: u64) -> u32 {
        self.state.words.get(&offset).copied().unwrap_or(0)
    }
    fn raw(&self) -> u32 {
        let s = &self.state;
        let mut raw = s.latched;
        if s.rx.len() > self.word(RX_TL) as usize {
            raw |= RX_FULL;
        }
        if s.enable & ENABLED != 0 && s.tx.len() <= self.word(TX_TL) as usize {
            raw |= TX_EMPTY;
        }
        raw
    }
    fn update_line(&mut self, cx: &mut Cx<'_>) {
        let level = self.raw() & self.word(INTR_MASK) != 0;
        if level != self.state.line {
            self.state.line = level;
            cx.set_irq(self.options.irq, level);
        }
    }
    fn status(&self) -> u32 {
        let s = &self.state;
        let busy = s.wire || !s.tx.is_empty();
        u32::from(busy)
            | u32::from(s.tx.len() < self.options.fifo_depth) << 1
            | u32::from(s.tx.is_empty()) << 2
            | u32::from(!s.rx.is_empty()) << 3
            | u32::from(s.rx.len() >= self.options.fifo_depth) << 4
            | u32::from(busy) << 5
    }
    /// Ticks of `bits` SCL periods at the selected speed.
    fn bit_ticks(&self, bits: u64) -> Result<u64, String> {
        let first = match self.word(CON) >> 1 & 3 {
            1 => SS_SCL,
            2 => FS_SCL,
            _ => return Err("DesignWare I2C high speed or invalid speed is not modeled".into()),
        };
        let counts = u64::from(self.word(first)) + u64::from(self.word(first + 4));
        let product = u128::from(bits * counts) * u128::from(self.options.tick_hz);
        let ticks = product.div_ceil(u128::from(self.options.clock_hz));
        u64::try_from(ticks)
            .ok()
            .filter(|t| *t > 0)
            .ok_or_else(|| "DesignWare I2C SCL counts give no byte time".into())
    }
    fn command(&mut self, cx: &mut Cx<'_>, value: u32) {
        let s = &self.state;
        if s.enable & ENABLED == 0 {
            cx.unsupported("DesignWare I2C DATA_CMD write while disabled");
            return;
        }
        if s.flushing && s.latched & TX_ABRT != 0 {
            return;
        }
        if s.tx.len() >= self.options.fifo_depth {
            self.state.latched |= TX_OVER;
            return;
        }
        let read = value & CMD_READ != 0;
        let restart = value & CMD_RESTART != 0 && self.word(CON) & RESTART_EN != 0;
        let mut command = Command {
            ticks: 0,
            start: false,
            stop: value & CMD_STOP != 0,
            nack: false,
            discarded: self.state.flushing,
            byte: None,
        };
        if !command.discarded {
            let target = (self.word(TAR) & 0x3ff) as u16;
            let s = &mut self.state;
            if !s.open || restart || read != s.reading {
                command.start = true;
                s.reading = read;
                s.open = self.bus.start(target, read);
                command.nack = !s.open;
                s.flushing = command.nack;
            }
            if !command.nack {
                if read {
                    command.byte = Some(match self.bus.read() {
                        Ok(byte) => byte,
                        Err(reason) => {
                            cx.unsupported(reason);
                            0
                        }
                    });
                } else {
                    self.bus.write(value as u8);
                }
            }
            if command.stop && self.state.open {
                self.bus.stop();
                self.state.open = false;
            }
        }
        let bits = 9 + 10 * u64::from(command.start) + u64::from(command.stop);
        command.ticks = match self.bit_ticks(bits) {
            Ok(ticks) => ticks,
            Err(reason) => {
                cx.unsupported(reason);
                return;
            }
        };
        if self.state.deadline.is_none() {
            self.state.deadline = cx.now().checked_add(command.ticks);
        }
        self.state.tx.push_back(command);
    }
    /// Ends the transfer now: TX FIFO flushed, stop sent, TX_ABRT latched.
    fn abort(&mut self, source: u32) {
        let s = &mut self.state;
        s.tx.clear();
        s.deadline = None;
        s.abort_source |= source;
        s.latched |= TX_ABRT | STOP_DET;
        s.wire = false;
        s.flushing = true;
        if s.open {
            s.open = false;
            self.bus.stop();
        }
    }
    fn clear(&mut self, bits: u32) {
        let s = &mut self.state;
        if bits & TX_ABRT != 0 && s.latched & TX_ABRT != 0 {
            s.flushing = false;
            s.abort_source = 0;
        }
        s.latched &= !bits;
    }
    fn set_enable(&mut self, value: u32) {
        let s = &mut self.state;
        if value & ENABLED == 0 {
            s.tx.clear();
            s.rx.clear();
            s.deadline = None;
            s.wire = false;
            s.flushing = false;
            if s.open {
                s.open = false;
                self.bus.stop();
            }
        }
        s.enable = value & ENABLED;
        if value & ABORT != 0 && value & ENABLED != 0 && (s.wire || !s.tx.is_empty()) {
            self.abort(ABRT_USER_ABRT);
        }
    }
}

impl<B: Bus> Device for Controller<B> {
    fn name(&self) -> &'static str {
        "designware-i2c"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.options.base);
        if !matches!(size, 2 | 4) || offset & 3 != 0 || offset >= WINDOW {
            cx.unmodeled();
            return 0;
        }
        let value = match offset {
            o if WORDS.contains(&o) => self.word(o),
            DATA_CMD => match self.state.rx.pop_front() {
                Some(byte) => byte.into(),
                None => {
                    self.state.latched |= RX_UNDER;
                    0
                }
            },
            INTR_STAT => self.raw() & self.word(INTR_MASK),
            RAW_INTR_STAT => self.raw(),
            CLR_INTR => {
                self.clear(!(RX_FULL | TX_EMPTY));
                0
            }
            ENABLE => self.state.enable,
            STATUS => self.status(),
            TXFLR => self.state.tx.len() as u32,
            RXFLR => self.state.rx.len() as u32,
            TX_ABRT_SOURCE => self.state.abort_source,
            ENABLE_STATUS => self.state.enable,
            o => match CLEARS.iter().find(|(at, _)| *at == o) {
                Some((_, bit)) => {
                    self.clear(*bit);
                    0
                }
                None => {
                    cx.unmodeled();
                    return 0;
                }
            },
        };
        self.update_line(cx);
        let mask = if size == 2 { 0xffff } else { u32::MAX };
        (value & mask).into()
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.options.base);
        if !matches!(size, 2 | 4) || offset & 3 != 0 || value > u32::MAX.into() {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        match offset {
            o if WORDS.contains(&o) => {
                self.state.words.insert(o, value);
            }
            DATA_CMD => self.command(cx, value),
            ENABLE => self.set_enable(value),
            _ => {
                cx.unmodeled();
                return;
            }
        }
        self.update_line(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        while let Some(at) = self.state.deadline.filter(|at| *at <= cx.now()) {
            let s = &mut self.state;
            let Some(command) = s.tx.pop_front() else {
                s.deadline = None;
                break;
            };
            if command.start {
                s.latched |= START_DET | ACTIVITY;
                s.wire = true;
            }
            if command.nack {
                s.tx.clear();
                s.deadline = None;
                s.abort_source |= ABRT_7B_ADDR_NOACK;
                s.latched |= TX_ABRT | STOP_DET;
                s.wire = false;
                break;
            }
            if let Some(byte) = command.byte {
                if s.rx.len() < self.options.fifo_depth {
                    s.rx.push_back(byte);
                } else {
                    s.latched |= RX_OVER;
                }
            }
            if command.stop {
                s.latched |= STOP_DET;
                s.wire = false;
            }
            s.deadline = s.tx.front().map(|next| at + next.ticks);
        }
        self.update_line(cx);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.deadline
    }
    fn save(&self) -> Value {
        serde_json::json!({ "controller": self.state, "bus": self.bus.save() })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["controller"].clone())
            .map_err(|e| format!("invalid DesignWare I2C checkpoint: {e}"))?;
        if state.deadline.is_some() == state.tx.is_empty()
            || state.tx.len() > self.options.fifo_depth
            || state.rx.len() > self.options.fifo_depth
            || state.words.keys().any(|o| !WORDS.contains(o))
        {
            return Err("DesignWare I2C checkpoint FIFOs or deadline are inconsistent".into());
        }
        self.bus.restore(&value["bus"])?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfff20000;

    /// One register-file target at 0x1c: first written byte selects.
    #[derive(Default)]
    struct File {
        registers: [u8; 4],
        pointer: Option<u8>,
        log: Vec<String>,
    }
    impl Bus for File {
        fn start(&mut self, address: u16, read: bool) -> bool {
            self.log.push(format!("start {address:#x} {read}"));
            if !read {
                self.pointer = None;
            }
            address == 0x1c
        }
        fn write(&mut self, byte: u8) {
            match self.pointer {
                None => self.pointer = Some(byte),
                Some(p) => {
                    self.registers[p as usize] = byte;
                    self.pointer = Some(p + 1);
                }
            }
        }
        fn read(&mut self) -> Result<u8, String> {
            let p = self.pointer.unwrap_or(0);
            self.pointer = Some(p + 1);
            self.registers
                .get(p as usize)
                .copied()
                .ok_or_else(|| "undeclared".into())
        }
        fn stop(&mut self) {
            self.log.push("stop".into());
        }
        fn save(&self) -> Value {
            serde_json::to_value(self.registers).unwrap()
        }
        fn restore(&mut self, value: &Value) -> Result<(), String> {
            self.registers = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
            Ok(())
        }
    }

    /// The X-T20 2.11 transfer setup 0xc52b30: fast mode, unmasked 0x5f.
    fn controller(h: &mut TestHost, target: u32) -> Controller<File> {
        let options = Options {
            base: BASE,
            irq: 69,
            fifo_depth: 8,
            clock_hz: 266_000_000,
            tick_hz: 48_000_000,
        };
        let mut c = Controller::new(options, File::default()).unwrap();
        for (o, v) in [(0x14, 0x533), (0x18, 0x545), (0x1c, 0x13f), (0x20, 0x151)] {
            write(&mut c, h, BASE + o, v);
        }
        write(&mut c, h, BASE + CON, 0x65);
        write(&mut c, h, BASE + TAR, target);
        write(&mut c, h, BASE + INTR_MASK, 0x5f);
        write(&mut c, h, BASE + ENABLE, 1);
        c
    }

    #[test]
    fn write_then_restarted_read_completes_after_the_byte_times() {
        let mut h = TestHost::new();
        let mut c = controller(&mut h, 0x1c);
        assert_eq!(read(&mut c, &mut h, BASE + INTR_STAT), u64::from(TX_EMPTY));
        // Register pointer 2, then two reads; restart on the first, stop on the last.
        for word in [0x02, 0x500, 0x300] {
            write(&mut c, &mut h, BASE + DATA_CMD, word);
        }
        assert_eq!(read(&mut c, &mut h, BASE + TXFLR), 3);
        assert_eq!(read(&mut c, &mut h, BASE + STATUS), 0x23);
        // 9 + 10 bits at 656 counts of 266 MHz, in 48 MHz ticks.
        let first = (19 * 656 * 48_000_000u64).div_ceil(266_000_000);
        assert_eq!(c.next_deadline(0), Some(first));
        advance(&mut c, &mut h, first - 1);
        assert_eq!(read(&mut c, &mut h, BASE + TXFLR), 3);
        advance(&mut c, &mut h, 1_000_000);
        assert_eq!(read(&mut c, &mut h, BASE + RXFLR), 2);
        assert_eq!(read(&mut c, &mut h, BASE + STATUS), 0x0e);
        assert_eq!(
            read(&mut c, &mut h, BASE + INTR_STAT),
            u64::from(RX_FULL | TX_EMPTY)
        );
        assert_eq!(read(&mut c, &mut h, BASE + DATA_CMD), 0);
        assert_eq!(read(&mut c, &mut h, BASE + DATA_CMD), 0);
        assert_eq!(
            read(&mut c, &mut h, BASE + RAW_INTR_STAT),
            u64::from(TX_EMPTY | STOP_DET | START_DET | ACTIVITY)
        );
        read(&mut c, &mut h, BASE + CLR_INTR);
        assert_eq!(
            read(&mut c, &mut h, BASE + RAW_INTR_STAT),
            u64::from(TX_EMPTY)
        );
        assert_eq!(c.bus().log, ["start 0x1c false", "start 0x1c true", "stop"]);
    }

    #[test]
    fn written_bytes_reach_the_target_and_checkpoints_restore() {
        let mut h = TestHost::new();
        let mut c = controller(&mut h, 0x1c);
        for word in [0x01, 0xab, 0x2cd] {
            write(&mut c, &mut h, BASE + DATA_CMD, word);
        }
        assert_eq!(c.bus().registers, [0, 0xab, 0xcd, 0]);
        let saved = c.save();
        advance(&mut c, &mut h, 1_000_000);
        assert_eq!(read(&mut c, &mut h, BASE + STATUS), 0x06);
        c.restore(&saved).unwrap();
        assert_eq!(read(&mut c, &mut h, BASE + TXFLR), 3);
        assert!(c.next_deadline(0).is_some());
    }

    #[test]
    fn a_missing_target_aborts_and_flushes_until_tx_abort_is_cleared() {
        let mut h = TestHost::new();
        let mut c = controller(&mut h, 0x39);
        for word in [0x05, 0x206] {
            write(&mut c, &mut h, BASE + DATA_CMD, word);
        }
        advance(&mut c, &mut h, 1_000_000);
        assert_eq!(read(&mut c, &mut h, BASE + TXFLR), 0);
        assert_eq!(read(&mut c, &mut h, BASE + STATUS), 0x06);
        assert_eq!(read(&mut c, &mut h, BASE + TX_ABRT_SOURCE), 1);
        assert_ne!(
            read(&mut c, &mut h, BASE + INTR_STAT) & u64::from(TX_ABRT),
            0
        );
        write(&mut c, &mut h, BASE + DATA_CMD, 0x07);
        assert_eq!(read(&mut c, &mut h, BASE + TXFLR), 0);
        read(&mut c, &mut h, BASE + 0x54);
        assert_eq!(read(&mut c, &mut h, BASE + TX_ABRT_SOURCE), 0);
        write(&mut c, &mut h, BASE + DATA_CMD, 0x07);
        assert_eq!(read(&mut c, &mut h, BASE + TXFLR), 1);
        // A user abort of the pending address ends it at once.
        write(&mut c, &mut h, BASE + ENABLE, 3);
        assert_eq!(read(&mut c, &mut h, BASE + ENABLE), 1);
        assert_eq!(read(&mut c, &mut h, BASE + TX_ABRT_SOURCE), 1 << 16);
        assert_eq!(c.next_deadline(0), None);
    }

    #[test]
    fn an_undeclared_target_register_is_unsupported_and_disabled_writes_are_refused() {
        let mut h = TestHost::new();
        let mut c = controller(&mut h, 0x1c);
        write(&mut c, &mut h, BASE + DATA_CMD, 0x09);
        let mut cx = h.cx();
        c.write(&mut cx, BASE + DATA_CMD, 4, 0x100);
        assert!(cx.is_unmodeled());
        write(&mut c, &mut h, BASE + ENABLE, 0);
        assert_eq!(read(&mut c, &mut h, BASE + TXFLR), 0);
        let mut cx = h.cx();
        c.write(&mut cx, BASE + DATA_CMD, 4, 0x1);
        assert!(cx.is_unmodeled());
    }
}
