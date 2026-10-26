//! FFF62000 register/FIFO and memory/warp-stream protocol. Full-MCU packed422
//! input, uploaded nonzero byte quantizers and U16 DRI are supported. EXR II
//! DCT arithmetic is modeled; only q<=3 is measured. Timing is declared.
//! DMA bytes/count/status precede IRQ77. Entropy output beyond 0x110 completes
//! with status 0x20 instead of READY (0x10), count 0 and no RAM write.
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_image_codecs::jpeg::{Codec, OUTPUT_OVERFLOW};
use fujimod_emulator_imaging::exr2_isp::{
    dma::{Transfer, physical},
    due,
    stream::{State, StreamHandle},
    word,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub completion_ticks: u64,
    pub reset_ticks: u64,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
enum Pending {
    Idle(u64),
    Reset(u64),
    Encode { stream: bool },
}
pub struct Encoder {
    r: Vec<u32>,
    fifo: Vec<u8>,
    cursor: usize,
    ready: bool,
    codec: Codec,
    options: Options,
    irq: Irq,
    stream: StreamHandle,
    source: Arc<[u8]>,
    dma: Transfer,
    pending: Option<Pending>,
}
impl Encoder {
    pub fn new(options: Options, wiring: (Irq, StreamHandle)) -> Result<Self, String> {
        if options.completion_ticks == 0 || options.reset_ticks == 0 {
            return Err("JPEG delays must be positive".into());
        }
        Ok(Self {
            r: vec![0; 0x12c / 4],
            fifo: vec![0; 128],
            cursor: 0,
            ready: false,
            codec: Codec::default(),
            options,
            irq: wiring.0,
            stream: wiring.1,
            source: Arc::from([]),
            dma: Transfer::default(),
            pending: None,
        })
    }
    fn valid(o: usize) -> bool {
        matches!(
            o,
            0 | 0x24
                | 0x28
                | 0x40
                | 0x44
                | 0x48
                | 0x4c
                | 0x50
                | 0x80
                | 0x84
                | 0x8c
                | 0x94
                | 0x100
                | 0x10c
                | 0x110
                | 0x118
                | 0x11c
                | 0x120
                | 0x124
                | 0x128
        )
    }
    fn start(&mut self, cx: &Cx<'_>) -> Result<(), String> {
        let r = &self.r;
        let (w, h) = ((r[0x44 / 4] & 65535) as usize, (r[0x44 / 4] >> 16) as usize);
        if let Some(offset) = [0x48, 0x4c, 0x50, 0x11c, 0x124, 0x128]
            .into_iter()
            .find(|&offset| r[offset / 4] != 0 && (offset != 0x48 || r[offset / 4] != 0x11))
        {
            return Err(format!(
                "JPEG register {:#x}={:#x} exceeds packed422 input law",
                0xfff62000 + offset,
                r[offset / 4]
            ));
        }
        if r[0] != 4
            || !self.ready
            || r[0x28 / 4] != 0xfd0000
            || r[0x40 / 4] != 1
            || r[0x80 / 4] > u16::MAX as u32
            || ![0, 0x11].contains(&r[0x48 / 4])
            || ![0, 2].contains(&r[0x120 / 4])
            || [0x4c, 0x50, 0x11c, 0x124, 0x128]
                .iter()
                .any(|&o| r[o / 4] != 0)
            || w < 16
            || w % 16 != 0
            || h < 8
            || h % 8 != 0
            || w * h > 32_000_000
        {
            return Err("JPEG configuration exceeds packed422 input law".into());
        }
        // Step encodings zero and 0x11 both use this declared packed422
        // transport.
        // The interpretation as implicit/explicit unity steps is unmeasured;
        // other values and source transports still refuse.
        let (dst, capacity) = (r[0x10c / 4] as u64, r[0x110 / 4] as usize);
        let dest = physical(dst, capacity)?;
        if dst & 7 != 0 || capacity < 8 {
            return Err("JPEG output requires aligned bounded allocation".into());
        }
        let streaming = r[0x120 / 4] == 2;
        let restart = r[0x80 / 4] as u16;
        self.codec
            .validate(w * h * 2, w, h, restart, capacity - 2)?;
        let input = if streaming {
            let mut s = self.stream.lock().unwrap();
            Some(s.claim(w, h)?)
        } else {
            let src = r[0x100 / 4] as u64;
            let count = w * h * 2;
            let source = physical(src, count)?;
            if src & 7 != 0 || source < dest + capacity as u64 && dest < source + count as u64 {
                return Err("JPEG source and output allocations overlap or are unaligned".into());
            }
            self.source = vec![0; count].into();
            cx.ram_read(src, Arc::get_mut(&mut self.source).unwrap())?;
            None
        };
        if let Err(e) =
            self.dma
                .prepare_variable(cx.now(), self.options.completion_ticks, dst, capacity)
        {
            if streaming {
                self.stream.lock().unwrap().state = State::Ready;
            }
            return Err(e);
        }
        let codec = self.codec.clone();
        let source = self.source.clone();
        self.dma.pixels.defer_fallible(move |mut out| {
            let result = (|| {
                let pixels = match &input {
                    Some(input) => input.bytes()?,
                    None => &source,
                };
                codec.encode(pixels, w, h, restart, &mut out, capacity - 2)
            })();
            if result.is_ok() {
                out.splice(0..0, [0xff, 0xd8]);
            }
            match result {
                // Overflow completes as a hardware error with no output;
                // a successful stream is never empty.
                Err(error) if error == OUTPUT_OVERFLOW => (Vec::new(), Ok(())),
                result => (out, result),
            }
        });
        if fujimod_emulator_devices::work::worker_count() == 0
            && let Err(error) = self.dma.check_result()
        {
            self.dma.cancel();
            if streaming {
                self.stream.lock().unwrap().state = State::Ready;
            }
            return Err(error);
        }
        self.pending = Some(Pending::Encode { stream: streaming });
        Ok(())
    }
    fn store(&mut self, cx: &Cx<'_>, o: usize, v: u32) -> Result<(), String> {
        if self.pending.is_some() && o != 0x24 {
            return Err("JPEG write while busy".into());
        }
        if o == 0x24 {
            if v & !0x10030 != 0 {
                return Err("unknown JPEG ACK bits".into());
            }
            self.r[o / 4] &= !v;
            return Ok(());
        }
        match o {
            0x118 => {
                if v != 1 {
                    return Err("unknown JPEG idle request".into());
                }
                self.pending = Some(Pending::Idle(due(cx.now(), self.options.reset_ticks)?));
            }
            0 => match v {
                0x100 => {
                    self.pending = Some(Pending::Reset(due(cx.now(), self.options.reset_ticks)?))
                }
                5 => self.start(cx)?,
                4 => {}
                _ => return Err("unknown JPEG command".into()),
            },
            0x84 => match v {
                // Table selection is independent of FIFO transport state.
                0 => self.ready = false,
                0x100 => {
                    self.cursor = 0;
                    self.ready = false;
                }
                1 if self.cursor == 128 => {
                    self.codec.tables(&self.fifo)?;
                    self.ready = true;
                }
                _ => return Err("incomplete JPEG quantization FIFO selection".into()),
            },
            0x8c => {
                if self.ready || self.r[0x84 / 4] != 0x100 || self.cursor >= 128 {
                    return Err("JPEG quantization upload outside FIFO".into());
                }
                self.fifo[self.cursor..self.cursor + 4].copy_from_slice(&v.to_be_bytes());
                self.cursor += 4;
            }
            0x94 => return Err("JPEG output count is read-only".into()),
            _ => {}
        }
        self.r[o / 4] = v;
        Ok(())
    }
}
impl Device for Encoder {
    fujimod_emulator_devices::checkpoint_fields!(
        r: Vec<u32>,
        #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")] fifo: Vec<u8>,
        cursor: usize,
        ready: bool,
        dma: Transfer,
        pending: Option<Pending>,
        ; blobs: [source]
    );
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-jpeg-encoder"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        let Some(o) = word(cx, a, s, 0xfff62000, 0x12c) else {
            return 0;
        };
        if ![0, 0x24, 0x94].contains(&o) {
            cx.unmodeled();
            return 0;
        }
        self.r[o / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        let Some(o) = word(cx, a, s, 0xfff62000, 0x12c) else {
            return;
        };
        if !Self::valid(o) || v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if let Err(e) = self.store(cx, o, v as u32) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        match self.pending {
            Some(Pending::Idle(t) | Pending::Reset(t)) => Some(t),
            Some(Pending::Encode { .. }) => self.dma.deadline(),
            None => None,
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.next_deadline(cx.now()).is_none_or(|d| cx.now() < d) {
            return;
        }
        match self.pending.unwrap() {
            Pending::Idle(_) => self.r[0x24 / 4] |= 0x10000,
            Pending::Reset(_) => {
                self.r.fill(0);
                self.fifo.fill(0);
                self.cursor = 0;
                self.ready = false;
            }
            Pending::Encode { stream } => {
                if let Err(error) = self.dma.check_result() {
                    self.dma.cancel();
                    self.pending = None;
                    if stream {
                        self.stream.lock().unwrap().state = State::Ready;
                    }
                    cx.fail(error);
                    return;
                }
                if self.dma.pixels.is_empty() {
                    // Output overflow: stock channel8_complete takes a
                    // completion without READY (0x10) as status 2 with no
                    // bytes, and metadata_retry_compression_result retries at
                    // a recomputed quality. No output reaches RAM.
                    self.dma.cancel();
                    self.r[0] &= !1;
                    self.r[0x24 / 4] |= 0x20;
                    self.r[0x94 / 4] = 0;
                    if stream && let Err(e) = self.stream.lock().unwrap().finish() {
                        cx.fail(e);
                        return;
                    }
                    Transfer::interrupt(cx, self.irq);
                    self.pending = None;
                    return;
                }
                let produced = self.dma.pixels.len() as u32;
                match self.dma.write(cx) {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(e) => {
                        cx.fail(e);
                        return;
                    }
                }
                self.r[0] &= !1;
                self.r[0x24 / 4] |= 0x10;
                self.r[0x94 / 4] = produced;
                if stream && let Err(e) = self.stream.lock().unwrap().finish() {
                    cx.fail(e);
                    return;
                }
                Transfer::interrupt(cx, self.irq);
            }
        }
        self.pending = None;
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!((
            &self.r,
            &self.fifo,
            self.cursor,
            self.ready,
            &self.dma,
            &self.pending
        ))
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let (r, f, c, b, d, p): (Vec<u32>, Vec<u8>, usize, bool, Transfer, Option<Pending>) =
            serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if r.len() != self.r.len() || f.len() != 128 || c > 128 || c % 4 != 0 {
            return Err("JPEG encoder state size".into());
        }
        if b {
            self.codec.tables(&f)?;
        }
        self.r = r;
        self.fifo = f;
        self.cursor = c;
        self.ready = b;
        self.dma = d;
        self.pending = p;
        Ok(())
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    #[test]
    fn fifo_source_and_pending_dma_restore_with_bounded_encoding() {
        let mut device = Encoder::new(
            Options {
                completion_ticks: 5,
                reset_ticks: 2,
            },
            (Irq { core: 0, line: 77 }, Default::default()),
        )
        .unwrap();
        device.source = vec![0xf9; 16384].into();
        device.fifo = (0..128).map(|i| (255 - i) as u8).collect();
        device.cursor = 16;
        device.ready = true;
        device.pending = Some(Pending::Encode { stream: false });
        let identity = device.fingerprint().unwrap();
        let saved = device.checkpoint().unwrap();
        assert!(saved.bytes() < device.source.len() + device.fifo.len() + 1024);
        device.source = Arc::from([]);
        device.fifo.fill(0);
        device.cursor = 0;
        device.ready = false;
        device.pending = None;
        device.restore_checkpoint(&saved).unwrap();
        assert_eq!(&*device.source, &vec![0xf9; 16384]);
        assert_eq!(
            device.fifo,
            (0..128).map(|i| (255 - i) as u8).collect::<Vec<_>>()
        );
        assert_eq!(device.cursor, 16);
        assert!(device.ready);
        assert!(matches!(
            device.pending,
            Some(Pending::Encode { stream: false })
        ));
        assert_eq!(device.fingerprint().unwrap(), identity);
    }
}
