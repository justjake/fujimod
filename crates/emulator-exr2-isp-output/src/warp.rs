//! FFF65000 warp registers. Endpoint nearest
//! packed422 sampling and timing are declared approximations. Memory DMA
//! completes with IRQ74; the explicit JPEG stream has joint START ownership.
use fujimod_emulator_imaging::exr2_isp::dma::{Raster, Transfer};
use fujimod_emulator_imaging::exr2_isp::stream::{State, StreamHandle};
use fujimod_emulator_imaging::exr2_isp::{due, word};
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device};
use std::sync::Arc;
use fujimod_emulator_imaging::exr2_isp::options::warp::Options;

pub struct Warp {
    r: Vec<u32>,
    options: Options,
    irq: Irq,
    stream: StreamHandle,
    source: Arc<[u8]>,
    dma: Transfer,
    stop: Option<u64>,
}
impl Warp {
    pub fn new(options: Options, wiring: (Irq, StreamHandle)) -> Result<Self, String> {
        if options.completion_ticks == 0 || options.stop_ticks == 0 {
            return Err("warp timing must be positive".into());
        }
        let mut r = vec![0; 0x14c / 4];
        r[0] = 0x10000;
        Ok(Self {
            r,
            options,
            irq: wiring.0,
            stream: wiring.1,
            source: Arc::from([]),
            dma: Transfer::default(),
            stop: None,
        })
    }
    fn valid(o: usize) -> bool {
        matches!(o, 0 | 4 | 0x10..=0xd8 | 0x140 | 0x144 | 0x148)
    }
    fn start(&mut self, cx: &Cx<'_>) -> Result<(), String> {
        let r = &self.r;
        if r[0x148 / 4] != 0x21 || ![0, 0x40].contains(&r[1]) {
            return Err("warp requires DMA/stream enables".into());
        }
        if r[0x14 / 4] > 32768 || r[0x20 / 4] > 32768 {
            return Err("warp row pitch is unsupported".into());
        }
        let src = Raster {
            address: r[0x10 / 4] as u64,
            width: (r[0x18 / 4] & 65535) as usize,
            height: (r[0x18 / 4] >> 16) as usize,
            pitch: r[0x14 / 4] as usize * 2,
        };
        let dst = Raster {
            address: r[0x1c / 4] as u64,
            width: (r[0x28 / 4] & 65535) as usize,
            height: (r[0x28 / 4] >> 16) as usize,
            pitch: r[0x20 / 4] as usize * 2,
        };
        let (w, h) = (
            (r[0x54 / 4] & 65535) as usize + 1,
            (r[0x54 / 4] >> 16) as usize + 1,
        );
        let last = [(r[0x50 / 4] & 65535) as usize, (r[0x50 / 4] >> 16) as usize];
        let mode = r[0x34 / 4];
        src.validate(src.width * 2)?;
        if mode > 2
            || [0x24, 0x30, 0x48].iter().any(|&o| r[o / 4] != 0)
            || r[0x40 / 4] != 4096
            || r[0x44 / 4] != 4096
            || r[0x4c / 4] != 0x10001
            || !src.width.is_multiple_of(2)
            || !dst.width.is_multiple_of(2)
            || w % 2 != 0
            || w > dst.width
            || h > dst.height
            || dst.pitch < dst.width * 2
            || !src.pitch.is_multiple_of(4)
            || dst.height > 8192
            || dst.pitch > 65536
        {
            return Err("warp geometry exceeds packed422 endpoint law".into());
        }
        if self.stream.lock().unwrap().state != State::Idle {
            return Err("warp stream is outstanding".into());
        }
        self.source = vec![0; src.pitch * src.height].into();
        cx.ram_read(src.address, Arc::get_mut(&mut self.source).unwrap())?;
        let source = self.source.clone();
        if r[1] == 0x40 {
            let mut stream = self.stream.lock().unwrap();
            let plan = fujimod_emulator_image_processing::resample::Endpoint::new(
                self.source.len(),
                src.width,
                src.height,
                src.pitch,
                w,
                h,
                last,
                mode,
                w * h * 2,
            )?;
            stream.pixels.defer_rows(w * 2, h, move |rows, out| {
                plan.render_rows(&source, rows, out)
            });
            stream.width = w;
            stream.height = h;
            stream.state = State::Ready;
        } else {
            dst.validate(dst.width * 2)?;
            let active = Raster {
                width: w,
                height: h,
                ..dst
            };
            if src.overlaps(active)? {
                return Err("warp source and output overlap".into());
            }
            self.dma
                .prepare_deferred(cx.now(), self.options.completion_ticks, active, w * 2)?;
            let plan = match fujimod_emulator_image_processing::resample::Endpoint::new(
                self.source.len(),
                src.width,
                src.height,
                src.pitch,
                w,
                h,
                last,
                mode,
                w * h * 2,
            ) {
                Ok(plan) => plan,
                Err(error) => {
                    self.dma.cancel();
                    return Err(error);
                }
            };
            self.dma.pixels.defer_rows(w * 2, h, move |rows, out| {
                plan.render_rows(&source, rows, out)
            });
        }
        Ok(())
    }
}
impl Device for Warp {
    fujimod_emulator_devices::checkpoint_fields!(
        r: Vec<u32>,
        dma: Transfer,
        stop: Option<u64>,
        ; blobs: [source]
    );
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"r":self.r,"dma":self.dma.fingerprint(),"stop":self.stop,
            "source":fujimod_emulator_devices::fingerprint::bytes(&self.source),"stream":*self.stream.lock().unwrap()}),
        )
    }

    fn name(&self) -> &'static str {
        "exr2-isp-warp"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        self.advance(cx);
        let Some(o) = word(cx, a, s, 0xfff65000, 0x14c) else {
            return 0;
        };
        if !Self::valid(o) {
            cx.unmodeled();
            return 0;
        }
        self.r[o / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        self.advance(cx);
        let Some(o) = word(cx, a, s, 0xfff65000, 0x14c) else {
            return;
        };
        if !Self::valid(o) || v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if o == 0x144 {
            if v & !0x3f != 0 {
                cx.fail("unknown warp ACK bits");
                return;
            }
            self.r[o / 4] &= !(v as u32);
            return;
        }
        if self.dma.deadline().is_some() || self.stop.is_some() {
            cx.fail("warp reconfiguration while busy");
            return;
        }
        if self.stream.lock().unwrap().state == State::Claimed {
            cx.fail("warp stream is claimed by JPEG");
            return;
        }
        if o == 0 {
            match v {
                0 => match due(cx.now(), self.options.stop_ticks) {
                    Ok(t) => {
                        self.stop = Some(t);
                        self.stream.lock().unwrap().state = State::Idle;
                    }
                    Err(e) => {
                        cx.fail(e);
                        return;
                    }
                },
                1 | 0x10001 => {
                    if let Err(e) = self.start(cx) {
                        cx.fail(e);
                        return;
                    }
                }
                _ => {
                    cx.fail("unknown warp command");
                    return;
                }
            }
        }
        self.r[o / 4] = v as u32;
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        if self.stream.lock().unwrap().state == State::Finished {
            return Some(now);
        }
        self.dma.deadline().into_iter().chain(self.stop).min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        {
            let mut s = self.stream.lock().unwrap();
            if s.state == State::Finished {
                s.state = State::Idle;
                self.r[0] = 0x10000;
            }
        }
        if self.stop.is_some_and(|t| cx.now() >= t) {
            self.stop = None;
            self.r[0] = 0x10000;
        }
        match self.dma.write(cx) {
            Ok(true) => {
                self.r[0] = 0x10000;
                self.r[0x144 / 4] |= 0x20;
                Transfer::interrupt(cx, self.irq)
            }
            Err(e) => cx.fail(e),
            _ => {}
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!((&self.r, &self.dma, self.stop, &*self.stream.lock().unwrap()))
    }
    fn diagnostic_state(&self) -> serde_json::Value {
        let stream = self.stream.lock().unwrap();
        let summary = serde_json::json!({"state":stream.state,"width":stream.width,
            "height":stream.height,
            "pixels":fujimod_emulator_devices::fingerprint::bytes(&stream.pixels)});
        serde_json::json!((&self.r, self.dma.fingerprint(), self.stop, summary))
    }
    fn diagnostic_buffer(&self, name: &str) -> Result<&[u8], String> {
        if name != "dma-output" {
            return Err(format!("warp has no diagnostic buffer {name}"));
        }
        self.dma.pixels.check()?;
        Ok(&self.dma.pixels)
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let (r, d, t, s): (
            Vec<u32>,
            Transfer,
            Option<u64>,
            fujimod_emulator_imaging::exr2_isp::stream::Stream,
        ) = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if r.len() != self.r.len() {
            return Err("warp bank size".into());
        }
        self.r = r;
        self.dma = d;
        self.stop = t;
        *self.stream.lock().unwrap() = s;
        Ok(())
    }
}
