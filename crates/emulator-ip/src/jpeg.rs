//! JPEG encoder and decoder register blocks of the Fujifilm X-Processor Pro
//! and X-Processor 4 (one register layout, see each body's catalog entry).
//!
//! Encoder. The firmware writes the quantization tables word by word to
//! +0x8c (32 words: luma then chroma, big-endian bytes in zigzag order), the
//! restart interval in MCUs to +0x80, height << 16 | width to +0x44, the
//! packed 4:2:2 source (Y0 Y1 Cb Cr) to +0x100, its line pitch in pixels to
//! +0x130 (a multiple of 8) and the window origin y << 16 | x to +0x12c (x a
//! multiple of 8; a run reads height lines of width pixels from there), the
//! destination to +0x10c and its size limit to +0x110, and starts a run with
//! bit 0 of +0x00 (bit 8 resets; both read clear). +0x24 bit 16 reads set
//! (ready); a finished run sets bit 4, which writing 1 clears, and the level
//! interrupt is high while it is set and not masked. The stream size is read
//! at +0x94. The stream is SOI, DHT, DQT, DRI, SOF0 (4:2:2), SOS, scan and
//! EOI; its SOF dimensions are the +0x44 size. A run whose stream is longer
//! than the limit completes, writes only the limit's bytes and reports the
//! full stream length (a declared approximation: what the hardware writes
//! beyond the limit is not observed).
//!
//! Two register contracts select how the other words behave:
//! [`Registers::LastWrite`] lets every word of the window read back its last
//! write and takes the last 32 table words written before a start (the
//! X-Processor Pro contract); [`Registers::Declared`] accepts only the
//! declared words and bits, loads the tables only while a gate bit is set,
//! masks the interrupt by a declared word and refuses everything else.
//!
//! Decoder (X-Processor Pro contract). Initialization writes 1 to +0x118 and
//! waits for status bit 16; status +0x04 is write-1-to-clear. A run reads the
//! stream at +0x10c (size +0x110) and writes packed 4:2:2 (Y0 Y1 Cb Cr, lines
//! padded to 16 pixels) to +0x100 (limit +0x104), scaled by
//! 1 / 2^(+0xa0 bits 1..0); with +0x120 bit 0 it first crops to x +0x124 and
//! y +0x128 (start | end << 16, inclusive). A finished run sets status bit 4
//! and the end of the output at +0x108; a failed one sets bit 17. The level
//! interrupt is high while either is set. The stream refill on status bit 24
//! (+0x118 = 4) is not modeled and fails. 4:2:2 output for a 4:2:0 stream is
//! not checked against a camera.
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_image_codecs::jpeg::{Codec, decode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
pub const CONTROL: u64 = 0x00;
pub const START: u32 = 1;
pub const RESET: u32 = 1 << 8;
pub const STATUS: u64 = 0x24;
pub const READY: u32 = 1 << 16;
pub const DONE: u32 = 1 << 4;
pub const SIZE: u64 = 0x44;
pub const RESTART: u64 = 0x80;
pub const TABLES: u64 = 0x8c;
pub const STREAM: u64 = 0x94;
pub const WINDOW_ORIGIN: u64 = 0x12c;
pub const PITCH: u64 = 0x130;
pub const SOURCE: u64 = 0x100;
pub const DESTINATION: u64 = 0x10c;
pub const LIMIT: u64 = 0x110;
const TABLE_WORDS: usize = 32;
const MAX_STREAM: usize = 64 * 1024 * 1024;

/// How the encoder treats words other than its run registers.
pub enum Registers {
    /// Every word of the window reads back its last write; a start takes
    /// the last 32 table words written.
    LastWrite,
    /// Only the declared words and bits; everything else refuses.
    Declared(Contract),
}

/// The words a processor's driver uses.
pub struct Contract {
    /// Words the driver writes and the bits it may set in each. Writing
    /// the status word clears the written bits.
    pub writable: Vec<(u64, u32)>,
    /// Words the driver reads.
    pub readable: Vec<u64>,
    /// Word and bit that open table loading: table words are accepted only
    /// while the bit is set, and clearing it latches exactly 32 of them.
    pub table_gate: (u64, u32),
    /// Word whose [`DONE`] bit masks the completion interrupt.
    pub interrupt_mask: u64,
    /// Physical address of the encoder's memory type word outside the
    /// window. Only type 0 (the flat physical map) is modeled.
    pub memory_type: Option<u64>,
}

impl Contract {
    fn check(&self, base: u64) -> Result<(), String> {
        let offsets: Vec<u64> = self.writable.iter().map(|&(o, _)| o).collect();
        let mut sorted = offsets.clone();
        sorted.sort_unstable();
        sorted.dedup();
        let mask = |offset| self.writable.iter().find(|&&(o, _)| o == offset).map(|&(_, m)| m);
        let valid = sorted.len() == offsets.len()
            && offsets
                .iter()
                .chain(&self.readable)
                .all(|o| o & 3 == 0 && *o < WINDOW)
            && [CONTROL, STATUS, SIZE, TABLES, SOURCE, DESTINATION]
                .iter()
                .all(|o| offsets.contains(o))
            && mask(self.table_gate.0).is_some_and(|m| m & self.table_gate.1 != 0)
            && self.table_gate.1.count_ones() == 1
            && mask(self.interrupt_mask).is_some_and(|m| m & DONE != 0)
            && self.memory_type.is_none_or(|a| {
                a & 3 == 0 && a <= u64::from(u32::MAX) - 3 && !(base..base + WINDOW).contains(&a)
            });
        if valid {
            Ok(())
        } else {
            Err("Invalid JPEG encoder register contract".into())
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    fifo: Vec<u32>,
    tables: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    status: u32,
    stream: u32,
    runs: u64,
}
pub struct Encoder {
    name: &'static str,
    base: u64,
    irq: u32,
    ticks: u64,
    registers: Registers,
    state: State,
}
impl Encoder {
    pub fn new(
        name: &'static str,
        base: u64,
        irq: u32,
        ticks: u64,
        registers: Registers,
    ) -> Result<Self, String> {
        if base & 0xfff != 0 || !(32..512).contains(&irq) || ticks == 0 {
            return Err("Invalid JPEG encoder".into());
        }
        if let Registers::Declared(contract) = &registers {
            contract.check(base)?;
        }
        Ok(Self {
            name,
            base,
            irq,
            ticks,
            registers,
            state: State::default(),
        })
    }
    /// The physical windows the encoder owns.
    pub fn windows(&self) -> Vec<(u64, u64)> {
        let mut windows = vec![(self.base, self.base + WINDOW)];
        if let Registers::Declared(Contract {
            memory_type: Some(address),
            ..
        }) = self.registers
        {
            windows.push((address, address + 4));
        }
        windows
    }
    fn word(&self, offset: u64) -> u32 {
        self.state.words.get(&offset).copied().unwrap_or(0)
    }
    fn contract(&self) -> Option<&Contract> {
        match &self.registers {
            Registers::LastWrite => None,
            Registers::Declared(contract) => Some(contract),
        }
    }
    fn interrupt(&self) -> bool {
        self.state.status & DONE != 0
            && self
                .contract()
                .is_none_or(|c| self.word(c.interrupt_mask) & DONE == 0)
    }
    /// The packed source window: `height` lines of `width` pixels from
    /// (x, y) of a picture whose lines are `pitch` pixels apart (0 reads as
    /// `width`).
    fn source(&self, cx: &mut Cx<'_>, width: usize, height: usize) -> Result<Vec<u8>, String> {
        let origin = self.word(WINDOW_ORIGIN);
        let (x, y) = ((origin & 0xffff) as usize, (origin >> 16) as usize);
        let pitch = match (self.word(PITCH) & 0xffff) as usize {
            0 => width,
            n => n,
        };
        let base = u64::from(self.word(SOURCE));
        if pitch == width && x == 0 {
            return cx.ram_bytes(base + (y * pitch * 2) as u64, width * height * 2);
        }
        if x + width > pitch {
            return Err(format!(
                "JPEG encoder window x {x} + width {width} exceeds the pitch {pitch}"
            ));
        }
        let mut source = Vec::with_capacity(width * height * 2);
        for line in 0..height {
            let start = base + (((y + line) * pitch + x) * 2) as u64;
            source.extend_from_slice(&cx.ram_bytes(start, width * 2)?);
        }
        Ok(source)
    }
    fn encode(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let mut codec = Codec::default();
        codec.tables(&self.state.tables)?;
        let size = self.word(SIZE);
        let (width, height) = ((size & 0xffff) as usize, (size >> 16) as usize);
        let restart = self.word(RESTART) as u16;
        let limit = match self.word(LIMIT) as usize {
            0 => MAX_STREAM,
            n => n.min(MAX_STREAM),
        };
        let source = self.source(cx, width, height)?;
        let mut body = Vec::new();
        codec.encode(&source, width, height, restart, &mut body, MAX_STREAM - 2)?;
        let mut stream = vec![0xff, 0xd8];
        stream.extend_from_slice(&body);
        // Only the programmed limit is written; the reported length tells the
        // firmware the stream overflowed.
        cx.ram_write(
            self.word(DESTINATION).into(),
            &stream[..stream.len().min(limit)],
        )?;
        self.state.stream = stream.len() as u32;
        Ok(())
    }
    /// A write under the declared contract: checks the word and its bits and
    /// applies table loading; `false` when it refused.
    fn declared_write(&mut self, cx: &mut Cx<'_>, offset: u64, value: u32) -> bool {
        let Some(contract) = self.contract() else {
            return true;
        };
        let Some(&(_, mask)) = contract.writable.iter().find(|&&(o, _)| o == offset) else {
            cx.unmodeled();
            return false;
        };
        if value & !mask != 0 {
            cx.fail(format!(
                "JPEG encoder +{offset:#x} write {value:#x} sets bits outside {mask:#x}"
            ));
            return false;
        }
        let (gate, bit) = contract.table_gate;
        let open = self.word(gate) & bit != 0;
        if offset == TABLES && !open {
            cx.fail("JPEG encoder table word written while table loading is closed");
            return false;
        }
        if offset == TABLES && self.state.fifo.len() == TABLE_WORDS {
            cx.fail("JPEG encoder table loading exceeds 32 words");
            return false;
        }
        if offset == gate && !open && value & bit != 0 {
            self.state.fifo.clear();
        }
        if offset == gate && open && value & bit == 0 {
            if self.state.fifo.len() != TABLE_WORDS {
                cx.fail("JPEG encoder table loading closed before 32 words");
                return false;
            }
            self.state.tables = std::mem::take(&mut self.state.fifo)
                .iter()
                .flat_map(|w| w.to_be_bytes())
                .collect();
        }
        true
    }
}
impl Device for Encoder {
    fn name(&self) -> &'static str {
        self.name
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4
            || offset & 3 != 0
            || offset >= WINDOW
            || self.contract().is_some_and(|c| !c.readable.contains(&offset))
        {
            cx.unmodeled();
            return 0;
        }
        u64::from(match offset {
            STATUS => self.state.status | READY,
            STREAM => self.state.stream,
            CONTROL => self.word(CONTROL) & !(START | RESET),
            o => self.word(o),
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 {
            return cx.unmodeled();
        }
        if let Some(memory_type) = self.contract().and_then(|c| c.memory_type)
            && address == memory_type
        {
            if value != 0 {
                cx.fail(format!("JPEG encoder memory type {value} is not modeled"));
            }
            return;
        }
        let offset = address.wrapping_sub(self.base);
        if offset & 3 != 0 || offset >= WINDOW {
            return cx.unmodeled();
        }
        if !self.declared_write(cx, offset, value) {
            return;
        }
        let declared = self.contract().is_some();
        match offset {
            STATUS => {
                self.state.status &= !value;
                let level = self.interrupt();
                cx.set_irq(self.irq, level);
                return;
            }
            TABLES => {
                self.state.fifo.push(value);
                if self.state.fifo.len() > TABLE_WORDS {
                    self.state.fifo.remove(0);
                }
                return;
            }
            CONTROL if value & RESET != 0 && !declared => {
                self.state.fifo.clear();
            }
            CONTROL if value & START != 0 && self.state.due.is_none() => {
                if !declared && self.state.fifo.len() == TABLE_WORDS {
                    self.state.tables = self
                        .state
                        .fifo
                        .iter()
                        .flat_map(|w| w.to_be_bytes())
                        .collect();
                }
                if !declared {
                    self.state.fifo.clear();
                }
                self.state.due = cx.now().checked_add(self.ticks);
                if self.state.due.is_none() {
                    return cx.fail("JPEG encoder deadline overflow");
                }
            }
            _ => {}
        }
        if value == 0 {
            self.state.words.remove(&offset);
        } else {
            self.state.words.insert(offset, value);
        }
        if self
            .contract()
            .is_some_and(|c| c.interrupt_mask == offset)
        {
            let level = self.interrupt();
            cx.set_irq(self.irq, level);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.state.due.is_some_and(|at| at <= cx.now()) {
            return;
        }
        self.state.due = None;
        if let Err(e) = self.encode(cx) {
            return cx.fail(format!("JPEG encoder run failed: {e}"));
        }
        self.state.runs += 1;
        self.state.status |= DONE;
        let level = self.interrupt();
        cx.set_irq(self.irq, level);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        Ok(())
    }
    fujimod_emulator_devices::checkpoint_state!(state: State, Self::check);
}
impl Encoder {
    fn check(&self, state: &State) -> Result<(), String> {
        let undeclared = |o: &u64, v: &u32| {
            self.contract().is_some_and(|c| {
                !c.writable
                    .iter()
                    .any(|&(w, m)| w == *o && v & !m == 0 && w != STATUS && w != TABLES)
            })
        };
        if state
            .words
            .iter()
            .any(|(o, v)| o & 3 != 0 || *o >= WINDOW || *v == 0 || undeclared(o, v))
            || state.fifo.len() > TABLE_WORDS
            || !(state.tables.is_empty() || state.tables.len() == 4 * TABLE_WORDS)
            || state.status & !DONE != 0
        {
            return Err("Invalid JPEG encoder checkpoint".into());
        }
        Ok(())
    }
}

const D_STATUS: u64 = 0x04;
const D_INITIALISED: u32 = 1 << 16;
const D_FAILED: u32 = 1 << 17;
const D_SCALE: u64 = 0xa0;
const D_OUTPUT: u64 = 0x100;
const D_LIMIT: u64 = 0x104;
const D_END: u64 = 0x108;
const D_STREAM: u64 = 0x10c;
const D_STREAM_SIZE: u64 = 0x110;
const D_COMMAND: u64 = 0x118;
const D_INITIALISE: u32 = 1;
const D_CROP: u64 = 0x120;
const D_CROP_X: u64 = 0x124;
const D_CROP_Y: u64 = 0x128;
const MAX_SIDE: usize = 8192;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecoderState {
    words: BTreeMap<u64, u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    status: u32,
    end: u32,
    runs: u64,
}
pub struct Decoder {
    name: &'static str,
    base: u64,
    irq: u32,
    ticks: u64,
    state: DecoderState,
}
impl Decoder {
    pub fn new(name: &'static str, base: u64, irq: u32, ticks: u64) -> Result<Self, String> {
        if base & 0xfff != 0 || !(32..512).contains(&irq) || ticks == 0 {
            return Err("Invalid JPEG decoder".into());
        }
        Ok(Self {
            name,
            base,
            irq,
            ticks,
            state: DecoderState::default(),
        })
    }
    fn word(&self, offset: u64) -> u32 {
        self.state.words.get(&offset).copied().unwrap_or(0)
    }
    /// Decodes, crops, scales and packs; `Ok(None)` when the picture does
    /// not fit the output limit.
    fn decode(&self, cx: &mut Cx<'_>) -> Result<Option<u32>, String> {
        let stream = cx.ram_bytes(
            self.word(D_STREAM).into(),
            self.word(D_STREAM_SIZE) as usize,
        )?;
        let (ycc, width, height) = decode(&stream, MAX_SIDE, MAX_SIDE)?;
        let (mut x0, mut x1, mut y0, mut y1) = (0, width - 1, 0, height - 1);
        if self.word(D_CROP) & 1 != 0 {
            let (x, y) = (self.word(D_CROP_X) as usize, self.word(D_CROP_Y) as usize);
            (x0, x1, y0, y1) = (x & 0xffff, x >> 16, y & 0xffff, y >> 16);
            if x0 > x1 || y0 > y1 || x1 >= width || y1 >= height {
                return Err("JPEG decoder crop outside the picture".into());
            }
        }
        let shift = self.word(D_SCALE) & 3;
        let step = 1usize << shift;
        let (w, h) = ((x1 + 1 - x0) >> shift, (y1 + 1 - y0) >> shift);
        let stride = w.div_ceil(16) * 16 * 2;
        if stride * h > self.word(D_LIMIT) as usize {
            return Ok(None);
        }
        let mut out = vec![0u8; stride * h];
        let mut line = vec![[0u32; 3]; w];
        for y in 0..h {
            line.iter_mut().for_each(|p| *p = [0; 3]);
            for sy in 0..step {
                let row = (y0 + y * step + sy) * width;
                for (x, sum) in line.iter_mut().enumerate() {
                    for sx in 0..step {
                        let at = (row + x0 + x * step + sx) * 3;
                        for c in 0..3 {
                            sum[c] += u32::from(ycc[at + c]);
                        }
                    }
                }
            }
            let n = (step * step) as u32;
            let pixels: Vec<[u8; 3]> = line
                .iter()
                .map(|s| s.map(|v| ((v + n / 2) / n) as u8))
                .collect();
            for (x, p) in pixels.iter().enumerate() {
                let at = y * stride + x / 2 * 4;
                out[at + x % 2] = p[0];
            }
            for (pair, p) in pixels.chunks(2).enumerate() {
                let at = y * stride + pair * 4;
                for c in 1..3 {
                    let sum: u32 = p.iter().map(|q| u32::from(q[c])).sum();
                    out[at + c + 1] = ((sum + p.len() as u32 / 2) / p.len() as u32) as u8;
                }
            }
        }
        let output = self.word(D_OUTPUT);
        cx.ram_write(output.into(), &out)?;
        Ok(Some(output + out.len() as u32))
    }
}
impl Device for Decoder {
    fn name(&self) -> &'static str {
        self.name
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || offset >= WINDOW {
            cx.unmodeled();
            return 0;
        }
        u64::from(match offset {
            D_STATUS => self.state.status,
            D_END => self.state.end,
            CONTROL => self.word(CONTROL) & !(START | RESET),
            D_COMMAND => 0,
            o => self.word(o),
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 || offset & 3 != 0 || offset >= WINDOW {
            return cx.unmodeled();
        }
        match offset {
            D_STATUS => {
                self.state.status &= !value;
                if self.state.status & (DONE | D_FAILED) == 0 {
                    cx.set_irq(self.irq, false);
                }
                return;
            }
            D_COMMAND if value == D_INITIALISE => {
                self.state.status |= D_INITIALISED;
                return;
            }
            D_COMMAND => return cx.fail("JPEG decoder stream refill is not modeled"),
            CONTROL if value & START != 0 && self.state.due.is_none() => {
                self.state.due = cx.now().checked_add(self.ticks);
                if self.state.due.is_none() {
                    return cx.fail("JPEG decoder deadline overflow");
                }
            }
            _ => {}
        }
        if value == 0 {
            self.state.words.remove(&offset);
        } else {
            self.state.words.insert(offset, value);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.state.due.is_some_and(|at| at <= cx.now()) {
            return;
        }
        self.state.due = None;
        self.state.runs += 1;
        match self.decode(cx) {
            Ok(Some(end)) => {
                self.state.end = end;
                self.state.status |= DONE;
            }
            Ok(None) => self.state.status |= D_FAILED,
            Err(e) => return cx.fail(format!("JPEG decoder run failed: {e}")),
        }
        cx.set_irq(self.irq, true);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: DecoderState =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        Ok(())
    }
    fujimod_emulator_devices::checkpoint_state!(state: DecoderState, Self::check);
}
impl Decoder {
    fn check(&self, state: &DecoderState) -> Result<(), String> {
        if state
            .words
            .iter()
            .any(|(o, v)| o & 3 != 0 || *o >= WINDOW || *v == 0)
            || state.status & !(DONE | D_FAILED | D_INITIALISED) != 0
        {
            return Err("Invalid JPEG decoder checkpoint".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfffc8000;

    #[test]
    fn a_run_encodes_the_source_with_the_uploaded_tables() {
        let mut encoder = Encoder::new("encoder", BASE, 134, 10, Registers::LastWrite).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut encoder, &mut h, BASE + STATUS), u64::from(READY));
        write(&mut encoder, &mut h, BASE, RESET);
        assert_eq!(read(&mut encoder, &mut h, BASE), 0);
        for _ in 0..TABLE_WORDS {
            write(&mut encoder, &mut h, BASE + TABLES, 0x0303_0303);
        }
        h.ram_write(0x1000, &[128; 32 * 16 * 2]).unwrap();
        write(&mut encoder, &mut h, BASE + SIZE, 16 << 16 | 32);
        write(&mut encoder, &mut h, BASE + SOURCE, 0x1000);
        write(&mut encoder, &mut h, BASE + DESTINATION, 0x4000);
        write(&mut encoder, &mut h, BASE + LIMIT, 0x2000);
        write(&mut encoder, &mut h, BASE, START);
        assert_eq!(read(&mut encoder, &mut h, BASE), 0);
        assert_eq!(encoder.next_deadline(0), Some(10));
        advance(&mut encoder, &mut h, 10);
        assert_eq!(
            read(&mut encoder, &mut h, BASE + STATUS),
            u64::from(READY | DONE)
        );
        let size = read(&mut encoder, &mut h, BASE + STREAM) as usize;
        let mut stream = vec![0; size];
        h.ram_read(0x4000, &mut stream).unwrap();
        assert_eq!(&stream[..4], &[0xff, 0xd8, 0xff, 0xc4]);
        assert_eq!(&stream[size - 2..], &[0xff, 0xd9]);
        let saved = encoder.save();
        write(&mut encoder, &mut h, BASE + STATUS, DONE);
        assert_eq!(read(&mut encoder, &mut h, BASE + STATUS), u64::from(READY));
        let mut other = Encoder::new("encoder", BASE, 134, 10, Registers::LastWrite).unwrap();
        other.restore(&saved).unwrap();
        assert_eq!(read(&mut other, &mut h, BASE + STREAM) as usize, size);
    }

    #[test]
    fn a_stream_longer_than_the_limit_completes_and_reports_its_length() {
        let mut encoder = Encoder::new("encoder", BASE, 134, 10, Registers::LastWrite).unwrap();
        let mut h = TestHost::new();
        for _ in 0..TABLE_WORDS {
            write(&mut encoder, &mut h, BASE + TABLES, 0x0101_0101);
        }
        // A noisy source does not fit in 64 bytes.
        let source: Vec<u8> = (0..32 * 16 * 2).map(|i| (i * 97 % 251) as u8).collect();
        h.ram_write(0x1000, &source).unwrap();
        write(&mut encoder, &mut h, BASE + SIZE, 16 << 16 | 32);
        write(&mut encoder, &mut h, BASE + SOURCE, 0x1000);
        write(&mut encoder, &mut h, BASE + DESTINATION, 0x4000);
        write(&mut encoder, &mut h, BASE + LIMIT, 64);
        write(&mut encoder, &mut h, BASE, START);
        advance(&mut encoder, &mut h, 10);
        assert_eq!(
            read(&mut encoder, &mut h, BASE + STATUS),
            u64::from(READY | DONE)
        );
        let size = read(&mut encoder, &mut h, BASE + STREAM) as usize;
        assert!(size > 64, "{size}");
        let mut written = vec![0; 80];
        h.ram_read(0x4000, &mut written).unwrap();
        assert_eq!(&written[..2], &[0xff, 0xd8]);
        assert!(written[64..].iter().all(|&b| b == 0));
    }

    fn encode_stream(source: &[u8], words: &[(u64, u32)]) -> Vec<u8> {
        let mut encoder = Encoder::new("encoder", BASE, 134, 10, Registers::LastWrite).unwrap();
        let mut h = TestHost::new();
        for _ in 0..TABLE_WORDS {
            write(&mut encoder, &mut h, BASE + TABLES, 0x0303_0303);
        }
        h.ram_write(0x1000, source).unwrap();
        for &(offset, value) in words {
            write(&mut encoder, &mut h, BASE + offset, value);
        }
        write(&mut encoder, &mut h, BASE + SOURCE, 0x1000);
        write(&mut encoder, &mut h, BASE + DESTINATION, 0x20000);
        write(&mut encoder, &mut h, BASE, START);
        advance(&mut encoder, &mut h, 10);
        let size = read(&mut encoder, &mut h, BASE + STREAM) as usize;
        let mut stream = vec![0; size];
        h.ram_read(0x20000, &mut stream).unwrap();
        stream
    }

    #[test]
    fn a_run_reads_the_window_at_the_origin_through_the_pitch() {
        // 48x24 picture; the run encodes its 32x16 window at (8, 4).
        let (pitch, x, y) = (48usize, 8usize, 4usize);
        let picture: Vec<u8> = (0..pitch * 24 * 2).map(|i| (i * 31 % 241) as u8).collect();
        let mut window = Vec::new();
        for line in 0..16 {
            let start = ((y + line) * pitch + x) * 2;
            window.extend_from_slice(&picture[start..start + 32 * 2]);
        }
        let cropped = encode_stream(
            &picture,
            &[
                (SIZE, 16 << 16 | 32),
                (PITCH, pitch as u32),
                (WINDOW_ORIGIN, (y << 16 | x) as u32),
            ],
        );
        let packed = encode_stream(&window, &[(SIZE, 16 << 16 | 32), (PITCH, 32)]);
        assert_eq!(cropped, packed);
        assert_ne!(
            cropped,
            encode_stream(&picture, &[(SIZE, 16 << 16 | 32), (PITCH, pitch as u32)])
        );
    }

    #[test]
    fn a_run_without_tables_fails() {
        let mut encoder = Encoder::new("encoder", BASE, 134, 10, Registers::LastWrite).unwrap();
        let mut h = TestHost::new();
        write(&mut encoder, &mut h, BASE + SIZE, 16 << 16 | 32);
        write(&mut encoder, &mut h, BASE, START);
        h.now = 10;
        let mut cx = h.cx();
        encoder.advance(&mut cx);
        assert!(cx.refusal().is_some());
    }

    #[test]
    fn the_decoder_scales_an_encoded_picture() {
        let mut encoder = Encoder::new("encoder", BASE, 134, 10, Registers::LastWrite).unwrap();
        let mut h = TestHost::new();
        for _ in 0..TABLE_WORDS {
            write(&mut encoder, &mut h, BASE + TABLES, 0x0101_0101);
        }
        // Y rises by 8 per pixel across the 32x16 picture.
        let source: Vec<u8> = (0..32 * 16 / 2)
            .flat_map(|i| {
                let x = (i % 16 * 2) as u8;
                [x * 8, (x + 1) * 8, 100, 160]
            })
            .collect();
        h.ram_write(0x1000, &source).unwrap();
        write(&mut encoder, &mut h, BASE + SIZE, 16 << 16 | 32);
        write(&mut encoder, &mut h, BASE + SOURCE, 0x1000);
        write(&mut encoder, &mut h, BASE + DESTINATION, 0x4000);
        write(&mut encoder, &mut h, BASE, START);
        advance(&mut encoder, &mut h, 10);
        let size = read(&mut encoder, &mut h, BASE + STREAM);

        const D: u64 = 0xfffc9000;
        let mut decoder = Decoder::new("decoder", D, 135, 5).unwrap();
        write(&mut decoder, &mut h, D + D_COMMAND, D_INITIALISE);
        assert_eq!(
            read(&mut decoder, &mut h, D + D_STATUS),
            u64::from(D_INITIALISED)
        );
        write(&mut decoder, &mut h, D + D_STATUS, D_INITIALISED);
        write(&mut decoder, &mut h, D + D_STREAM, 0x4000);
        write(&mut decoder, &mut h, D + D_STREAM_SIZE, size as u32);
        write(&mut decoder, &mut h, D + D_OUTPUT, 0x8000);
        write(&mut decoder, &mut h, D + D_LIMIT, 0x1000);
        write(&mut decoder, &mut h, D + D_SCALE, 1);
        write(&mut decoder, &mut h, D, START);
        advance(&mut decoder, &mut h, 15);
        assert_eq!(read(&mut decoder, &mut h, D + D_STATUS), u64::from(DONE));
        // 16x8 pixels, lines padded to 16 pixels of 2 bytes.
        assert_eq!(read(&mut decoder, &mut h, D + D_END), 0x8000 + 16 * 2 * 8);
        let mut out = vec![0; 16 * 2 * 8];
        h.ram_read(0x8000, &mut out).unwrap();
        for (i, q) in out.chunks(4).enumerate() {
            let x = i % 8 * 2;
            let y = (x * 16 + 4) as i32;
            assert!((i32::from(q[0]) - y).abs() <= 3, "{i} {q:?}");
            assert!((i32::from(q[2]) - 100).abs() <= 3 && (i32::from(q[3]) - 160).abs() <= 3);
        }
        write(&mut decoder, &mut h, D + D_LIMIT, 16);
        write(&mut decoder, &mut h, D, START);
        advance(&mut decoder, &mut h, 20);
        assert_eq!(
            read(&mut decoder, &mut h, D + D_STATUS),
            u64::from(DONE | D_FAILED)
        );
        let saved = decoder.save();
        Decoder::new("decoder", D, 135, 5).unwrap().restore(&saved).unwrap();
    }
}
