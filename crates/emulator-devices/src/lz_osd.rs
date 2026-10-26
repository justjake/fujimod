//! Method-4 streaming decompressor with separately selected OSD transport.
//! Processor wiring supplies the register base, interrupt and DMA high bits.
//! OSD output has no default codec; unsupported transports refuse the start.

/// A selected output transport. This never chooses command timing or reads
/// CPU state; the controller owns the source/destination DMA and interrupts.
pub trait OsdCodec: Send {
    fn pixel_bytes(&self, width: u32, height: u32) -> Result<usize, String>;
    fn secondary_bytes(&self) -> usize;
    fn encode(
        &self,
        primary: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<Vec<u8>, String>;
}
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default, Serialize, Deserialize)]
struct Lz {
    header: Option<[u32; 5]>,
    consumed: u32,
    produced: u32,
    page: u32,
    literal: u8,
    match_first: Option<u8>,
    history: Vec<u8>,
}
impl Lz {
    fn emit(&mut self, byte: u8, out: &mut Vec<u8>) -> Result<(), String> {
        if self.produced >= self.header.unwrap()[0] {
            return Err("LZ output exceeds declared size".into());
        }
        if self.history.len() < 2048 {
            self.history.resize(2048, 0);
        }
        self.history[self.produced as usize & 2047] = byte;
        self.produced += 1;
        out.push(byte);
        Ok(())
    }
    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        for &byte in bytes {
            if self.literal != 0 {
                self.emit(byte, &mut out)?;
                self.literal -= 1;
            } else if let Some(first) = self.match_first.take() {
                let distance = (((first & 127) as u32) << 4) | (byte as u32 >> 4);
                let length = byte & 15;
                if distance == 0 || distance > self.produced || length == 0 {
                    return Err("invalid method-4 back reference".into());
                }
                for _ in 0..length {
                    let b = self.history[(self.produced - distance) as usize & 2047];
                    self.emit(b, &mut out)?;
                }
            } else if byte == 0 {
                return Err("zero method-4 literal run".into());
            } else if byte < 128 {
                self.literal = byte;
            } else {
                self.match_first = Some(byte);
            }
            self.consumed += 1;
        }
        if self.consumed == self.header.unwrap()[1]
            && (self.literal != 0
                || self.match_first.is_some()
                || self.produced != self.header.unwrap()[0])
        {
            return Err("truncated LZ token or wrong decoded size".into());
        }
        Ok(out)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    regs: [u32; 27],
    lz: Lz,
    configured: bool,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    pending: Option<Conversion>,
}
#[derive(Serialize, Deserialize)]
struct Conversion {
    primary: u64,
    secondary: u64,
    pixels: Vec<u8>,
    header: Vec<u8>,
    due: u64,
}
pub struct Decompressor {
    base: u64,
    irq: u32,
    codec: Option<Box<dyn OsdCodec>>,
    state: State,
    high: Arc<Mutex<[u32; 2]>>,
}
impl Decompressor {
    pub fn new(
        base: u64,
        irq: u32,
        high: Arc<Mutex<[u32; 2]>>,
        codec: Option<Box<dyn OsdCodec>>,
    ) -> Self {
        Self {
            base,
            irq,
            codec,
            state: State::default(),
            high,
        }
    }
    /// Base address of the register window the decompressor was wired at.
    pub fn base(&self) -> u64 {
        self.base
    }
    fn convert(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let s = &mut self.state;
        if s.pending.is_some() || s.regs[26] & 1 != 0 {
            return Err("OSD start before completion/ACK".into());
        }
        if s.regs[16] != 1
            || s.regs[22] != 0x400
            || s.regs[23] != 0x04000400
            || s.regs[24] != 0x1010001
        {
            return Err("unsupported OSD converter configuration".into());
        }
        let dims = s.regs[18];
        if dims & !0x0fff0fff != 0 {
            return Err("reserved OSD dimension bits".into());
        }
        let (width, height) = (dims & 0xfff, (dims >> 16) & 0xfff);
        if width == 0 || height == 0 {
            return Err("zero OSD dimensions".into());
        }
        let codec = self
            .codec
            .as_ref()
            .ok_or("OSD output transport is not selected")?;
        let pixel_bytes = codec.pixel_bytes(width, height)?;
        let secondary_bytes = codec.secondary_bytes();
        let high = *self.high.lock().unwrap();
        let source = s.regs[19] as u64 | (high[0] as u64) << 32;
        let primary = s.regs[20] as u64 | (high[1] as u64) << 32;
        let secondary = s.regs[21] as u64;
        if primary == 0 || secondary == 0 || source == 0 {
            return Err("null OSD DMA address".into());
        }
        let primary_end = primary
            .checked_add(pixel_bytes as u64)
            .ok_or("OSD primary extent overflow")?;
        let secondary_end = secondary
            .checked_add(secondary_bytes as u64)
            .ok_or("OSD secondary extent overflow")?;
        if primary < secondary_end && secondary < primary_end {
            return Err("OSD output planes overlap".into());
        }
        let pixels = cx.ram_bytes(source, pixel_bytes)?;
        // Check both destinations before scheduling, without touching bytes.
        cx.ram_bytes(primary, pixel_bytes)?;
        cx.ram_bytes(secondary, secondary_bytes)?;
        let header = codec.encode(primary, width, height, &pixels)?;
        if header.len() != secondary_bytes {
            return Err("OSD transport output extent mismatch".into());
        }
        let due = cx
            .now()
            .checked_add(
                (pixel_bytes as u64)
                    .checked_mul(5)
                    .and_then(|n| n.checked_add(1000))
                    .ok_or("OSD latency overflow")?,
            )
            .ok_or("OSD deadline overflow")?;
        s.pending = Some(Conversion {
            primary,
            secondary,
            pixels,
            header,
            due,
        });
        s.generation += 1;
        // Hardware latency is unmeasured; preserve asynchronous completion.
        cx.schedule(due, s.generation);
        Ok(())
    }
    fn dma(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let s = &mut self.state;
        if !s.configured || s.regs[16] != 0 {
            return Err("decompression without mode-0 stream setup".into());
        }
        if s.regs[10] & 1 != 0 {
            return Err("decompression submitted before completion ACK".into());
        }
        let high = *self.high.lock().unwrap();
        let source = s.regs[2] as u64 | ((high[0] as u64) << 32);
        let dest = s.regs[4] as u64 | ((high[1] as u64) << 32);
        if s.lz.header.is_none() {
            let h = cx.ram_bytes(
                source
                    .checked_sub(20)
                    .ok_or("LZ header address underflow")?,
                20,
            )?;
            let h: [u32; 5] = std::array::from_fn(|n| {
                u32::from_le_bytes(h[n * 4..n * 4 + 4].try_into().unwrap())
            });
            if h[4] != 4
                || h[3] != 0x4000
                || h[1] < 20
                || h[0] == 0
                || h[2] != h[1].div_ceil(h[3])
                || s.regs[5] != h[0]
                || s.regs[6] != 20
                || s.regs[7] != h[2]
            {
                return Err(format!("unsupported/inconsistent method-4 header {h:x?}"));
            }
            s.lz.header = Some(h);
            s.lz.consumed = 20;
        }
        let h = s.lz.header.unwrap();
        if s.lz.page >= h[2] {
            return Err("DMA submitted after final compressed page".into());
        }
        // The first page starts at +20 but uses the programmed full-page
        // length; +18 credits those header bytes to that first page.
        let credit = if s.lz.page == 0 { 20 } else { 0 };
        let expected = (h[1] - s.lz.page * h[3]).min(h[3]);
        if s.regs[3] != expected {
            return Err(format!(
                "LZ page {} length {:#x}, expected {expected:#x}",
                s.lz.page, s.regs[3]
            ));
        }
        let input = cx.ram_bytes(source, (expected - credit) as usize)?;
        let offset = s.lz.produced;
        let out = s.lz.feed(&input)?;
        cx.ram_write(dest + offset as u64, &out)?;
        s.lz.page += 1;
        s.regs[10] |= 1;
        cx.set_irq(self.irq, true);
        Ok(())
    }
}
impl Device for Decompressor {
    fn name(&self) -> &'static str {
        "lz-osd-engine"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || !matches!(offset, 0..=0x28 | 0x40..=0x68) {
            cx.unmodeled();
            return 0;
        }
        match offset {
            0 => 1,
            0x24 => 0x10001,
            0x64 => {
                if self.state.pending.is_some() {
                    0
                } else {
                    0x1010001
                }
            }
            _ => self.state.regs[offset as usize / 4] as u64,
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || !matches!(offset, 0..=0x28 | 0x40..=0x68) {
            cx.unmodeled();
            return;
        }
        let v = value as u32;
        if self.state.pending.is_some() && !matches!(offset, 0 | 0x28 | 0x68) {
            cx.fail("engine register changed during OSD conversion");
            return;
        }
        match offset {
            0 if v == 0 => {
                let generation = self.state.generation + 1;
                self.state = State::default();
                self.state.generation = generation;
                cx.set_irq(self.irq, false);
            }
            0 => cx.fail("unsupported engine reset value"),
            0x24 | 0x64 => cx.unmodeled(),
            0x28 | 0x68 => {
                self.state.regs[offset as usize / 4] &= !v;
                cx.set_irq(
                    self.irq,
                    (self.state.regs[10] | self.state.regs[26]) & 1 != 0,
                );
            }
            0x20 => {
                self.state.regs[8] = v;
                match v {
                    0x84000400 => {
                        self.state.lz = Lz::default();
                        self.state.configured = false;
                    }
                    0x94001400 => {
                        self.state.configured = true;
                    }
                    _ => cx.fail(format!("unsupported decompression config {v:#x}")),
                }
            }
            4 if v == 1 => {
                if let Err(e) = self.dma(cx) {
                    cx.fail(e);
                }
            }
            0x44 if v == 1 => {
                if let Err(e) = self.convert(cx) {
                    cx.fail(e);
                }
            }
            4 | 0x44 if v != 0 => cx.fail("unsupported engine start value"),
            _ => self.state.regs[offset as usize / 4] = v,
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.pending.as_ref().map(|job| job.due)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.event(cx, self.state.generation);
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token != self.state.generation
            || self
                .state
                .pending
                .as_ref()
                .is_some_and(|job| cx.now() < job.due)
        {
            return;
        }
        if let Some(job) = self.state.pending.take() {
            if let Err(e) = cx
                .ram_write(job.primary, &job.pixels)
                .and_then(|()| cx.ram_write(job.secondary, &job.header))
            {
                cx.fail(e);
                return;
            }
            self.state.regs[26] |= 1;
            cx.log("OSD converter DMA complete");
            cx.set_irq(self.irq, true);
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(state.clone())
            .map_err(|e| format!("invalid decompressor checkpoint: {e}"))?;
        Ok(())
    }
}
