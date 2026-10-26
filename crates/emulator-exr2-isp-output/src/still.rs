//! FFF40000 still processor and its addressed calibration banks. Register
//! geometry drives RAW reads and IRQ61 output. Nearest six-by-six CFA color
//! uses programmed black, WB/exposure, radial shading and the measured film
//! law. Spatial filtering remains unmodeled.
use fujimod_emulator_imaging::exr2_isp::dma::{Raster, Transfer, physical};
use fujimod_emulator_imaging::exr2_isp::word;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_imaging::film::{Film, still_offset};
use fujimod_emulator_image_processing::raw::{Cfa, Packing, RawView};
use fujimod_emulator_devices::{Cx, Device};
use std::sync::Arc;
use fujimod_emulator_imaging::exr2_isp::options::still::Options;

pub struct StillProcessor {
    r: Vec<u32>,
    cfa: Cfa,
    film: Film,
    source: Arc<[u8]>,
    dma: Transfer,
    raw_dma: Transfer,
    delay: u64,
    irq: Irq,
}
impl StillProcessor {
    pub fn new(options: Options, irq: Irq) -> Result<Self, String> {
        if options.completion_ticks == 0 {
            return Err("still processor delay must be positive".into());
        }
        let mut r = vec![0; 0x16100 / 4];
        r[0x10000 / 4] = 0x10001;
        Ok(Self {
            r,
            cfa: Cfa::new(options.cfa)?,
            film: Film::default(),
            source: Arc::from([]),
            dma: Transfer::default(),
            raw_dma: Transfer::default(),
            delay: options.completion_ticks,
            irq,
        })
    }
    fn valid(o: usize) -> bool {
        matches!(o,0|0xb4..=0xf8|0x1400..=0x4bfc|0x8000..=0x805c|0x10000..=0x100fc|0x11000..=0x11ffc|0x14000..=0x160fc)
    }
    fn start(&mut self, cx: &Cx<'_>, control: u32) -> Result<(), String> {
        let r = |o: usize| self.r[(0x10000 + o) / 4];
        let packing = if r(8) & 0x100 != 0 {
            Packing::Packed12
        } else {
            Packing::U16
        };
        if r(0x44) > 16384 || r(0x48) > 16384 {
            return Err("still processor row pitch is unsupported".into());
        }
        let src = Raster {
            address: r(0x50) as u64,
            width: (r(0x64) >> 16) as usize + 1,
            height: (r(0x64) & 65535) as usize + 1,
            pitch: packing.row_bytes(r(0x44) as usize),
        };
        let dst = Raster {
            address: r(0x58) as u64,
            width: (r(0x38) >> 16) as usize,
            height: (r(0x38) & 65535) as usize,
            pitch: 2 * r(0x48) as usize,
        };
        src.validate(packing.row_bytes(src.width))?;
        dst.validate(dst.width * 2)?;
        if !src.width.is_multiple_of(2)
            || !dst.width.is_multiple_of(2)
            || src.pitch > 32768
            || dst.pitch > 32768
            || src.overlaps(dst)?
        {
            return Err("still processor RAW/output geometry overlaps or has an odd width".into());
        }
        // RAW writeback uses the third window and its own address/pitch. The
        // declared spatial approximation preserves these latched U16 samples;
        // RAW denoising and defect correction are not modeled. Backward,
        // sample-aligned overlap supports the register-programmed sliding window.
        let raw_output = if control == 3 {
            let raster = Raster {
                address: r(0x60) as u64,
                width: (r(0x40) >> 16) as usize,
                height: (r(0x40) & 65535) as usize,
                pitch: 2 * r(0x4c) as usize,
            };
            raster.validate(raster.width * 2)?;
            if !matches!(packing, Packing::U16)
                || raster.width != src.width
                || raster.height != src.height
                || raster.pitch > 32768
                || !raster.address.is_multiple_of(2)
                || !src.address.is_multiple_of(2)
                || dst.overlaps(raster)?
            {
                return Err("still RAW writeback requires matching U16 dimensions and separate color output".into());
            }
            if src.overlaps(raster)? {
                let source = physical(src.address, src.pitch * src.height)?;
                let destination = physical(raster.address, raster.pitch * raster.height)?;
                if raster.pitch != src.pitch || destination > source {
                    return Err(
                        "still RAW writeback overlap requires a backward equal-pitch window".into(),
                    );
                }
            }
            Some(raster)
        } else {
            None
        };
        let mut black = None;
        for o in (0x8004..0x8014).step_by(4) {
            for v in [self.r[o / 4] & 8191, self.r[o / 4] >> 16 & 8191] {
                if black.is_some_and(|b| b != v) {
                    return Err("unequal CFA black slots are unmodeled".into());
                }
                black = Some(v);
            }
        }
        let gains = [
            (self.r[0x8054 / 4] >> 16 & 32767) as u16,
            (self.r[0x8054 / 4] & 32767) as u16,
            (self.r[0x8058 / 4] >> 16 & 32767) as u16,
        ];
        let exposure = (self.r[0x8058 / 4] & 4095) as u16;
        let shading = crate::shading::Shading::from_registers(|o| self.r[(0x8000 + o) / 4])?;
        self.film
            .update(|o| self.r[(0x14000 + still_offset(o).unwrap()) / 4])?;
        // The still filter consumes its surrounding RAW context. For
        // 4944x3296 -> 4896x3264, output (0,0) maps to RAW (24,16). These are
        // cropped pixel windows, rather than full-field resampling coordinates.
        let crop = match (
            src.width.checked_sub(dst.width),
            src.height.checked_sub(dst.height),
        ) {
            (Some(x), Some(y)) if x % 2 == 0 && y % 2 == 0 => [x / 2, y / 2, dst.width, dst.height],
            _ => return Err("still output must fit a centered RAW filter window".into()),
        };
        self.source = vec![0; src.pitch * src.height].into();
        cx.ram_read(src.address, Arc::get_mut(&mut self.source).unwrap())?;
        self.dma
            .prepare_deferred(cx.now(), self.delay, dst, dst.width * 2)?;
        if let Some(raster) = raw_output {
            self.raw_dma
                .prepare_deferred(cx.now(), self.delay, raster, raster.width * 2)?;
        }
        let raw = RawView {
            bytes: &self.source,
            width: src.width,
            height: src.height,
            pitch: src.pitch,
            packing,
        };
        if let Err(error) = Cfa::validate_render(
            &raw,
            gains,
            exposure,
            crop,
            dst.width,
            dst.height,
            dst.width * dst.height * 2,
        ) {
            self.dma.cancel();
            self.raw_dma.cancel();
            return Err(error);
        }
        if let Some(raster) = raw_output {
            let source = self.source.clone();
            self.raw_dma.pixels.defer(move |mut out| {
                out.resize(raster.width * raster.height * 2, 0);
                for y in 0..raster.height {
                    out[y * raster.width * 2..(y + 1) * raster.width * 2]
                        .copy_from_slice(&source[y * src.pitch..y * src.pitch + raster.width * 2]);
                }
                out
            });
        }
        let source = self.source.clone();
        let cfa = self.cfa.clone();
        let film = self.film.clone();
        self.dma
            .pixels
            .defer_rows(dst.width * 2, dst.height, move |rows, out| {
                let raw = RawView {
                    bytes: &source,
                    width: src.width,
                    height: src.height,
                    pitch: src.pitch,
                    packing,
                };
                cfa.render_rows(
                    &raw,
                    &film,
                    black.unwrap() as u16,
                    gains,
                    exposure,
                    |x, y| shading.as_ref().map_or(2048, |s| s.gain(x, y)),
                    crop,
                    dst.width,
                    dst.height,
                    rows,
                    out,
                );
            });
        Ok(())
    }
}
impl Device for StillProcessor {
    fujimod_emulator_devices::checkpoint_fields!(
        r: Vec<u32>,
        cfa: Cfa,
        film: Film,
        dma: Transfer,
        raw_dma: Transfer,
        ; blobs: [source]
    );
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"r":self.r,"cfa":self.cfa,"film":self.film,
            "source":fujimod_emulator_devices::fingerprint::bytes(&self.source),"dma":self.dma.fingerprint(),
            "raw_dma":self.raw_dma.fingerprint()}),
        )
    }

    fn name(&self) -> &'static str {
        "exr2-isp-still-processor"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        let Some(o) = word(cx, a, s, 0xfff30000, 0x16100) else {
            return 0;
        };
        if !Self::valid(o) {
            cx.unmodeled();
            return 0;
        }
        self.r[o / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        let Some(o) = word(cx, a, s, 0xfff30000, 0x16100) else {
            return;
        };
        if !Self::valid(o) || v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if o == 0x10084 {
            self.r[o / 4] &= !(v as u32);
            return;
        }
        if self.dma.deadline().is_some() || self.raw_dma.deadline().is_some() {
            cx.fail("still processor reconfiguration during DMA");
            return;
        }
        if o == 0x10000 {
            self.r[o / 4] = v as u32 | 0x10001;
            return;
        }
        if o == 0x10004 {
            if !matches!(v, 1 | 3) {
                cx.fail(format!("unknown still processor START {v:#x}"));
                return;
            }
            if let Err(e) = self.start(cx, v as u32) {
                cx.fail(e);
                return;
            }
        }
        self.r[o / 4] = v as u32;
    }
    fn diagnostic_buffer(&self, name: &str) -> Result<&[u8], String> {
        match name {
            "raw-input" => Ok(&self.source),
            _ => Err(format!("unknown still-processor diagnostic buffer: {name}")),
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.dma
            .deadline()
            .into_iter()
            .chain(self.raw_dma.deadline())
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        match self.dma.write(cx) {
            Ok(true) => {
                if let Err(e) = self.raw_dma.write(cx) {
                    cx.fail(e);
                    return;
                }
                self.r[0x10084 / 4] |= 1;
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
        serde_json::json!((&self.r, &self.dma, &self.raw_dma))
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let (r, d, raw): (Vec<u32>, Transfer, Transfer) =
            serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if r.len() != self.r.len() {
            return Err("still register bank size".into());
        }
        self.r = r;
        self.dma = d;
        self.raw_dma = raw;
        self.film = Film::default();
        Ok(())
    }
}
