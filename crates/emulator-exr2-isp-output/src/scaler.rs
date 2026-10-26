//! FFF47000 packed422 scaler. Register fields define address, pitch, dimensions
//! and positive 16.16 steps; bilinear sampling is declared, not measured.
use fujimod_emulator_imaging::exr2_isp::dma::{Raster, Transfer};
use fujimod_emulator_imaging::exr2_isp::word;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::exr2_isp::options::scaler::Options;
pub struct Scaler {
    r: Vec<u32>,
    irq: Irq,
    delay: u64,
    dma: Transfer,
    source: Vec<u8>,
}
impl Scaler {
    pub fn new(options: Options, irq: Irq) -> Result<Self, String> {
        if !(1..=1 << 24).contains(&options.completion_ticks) {
            return Err("scaler delay is unbounded".into());
        }
        Ok(Self {
            r: vec![0; 0x84 / 4],
            irq,
            delay: options.completion_ticks,
            dma: Transfer::default(),
            source: Vec::new(),
        })
    }
    fn valid(o: usize) -> bool {
        matches!(o,0|4|8|0x10|0x14|0x18|0x20..=0x2c|0x40..=0x64|0x70..=0x80)
    }
    fn start(&mut self, cx: &Cx<'_>) -> Result<(), String> {
        let r = &self.r;
        let src = Raster {
            address: (r[0x10 / 4] & 0x3fffffff) as u64,
            width: (r[0x18 / 4] & 8191) as usize,
            height: (r[0x18 / 4] >> 16 & 8191) as usize,
            pitch: (r[0x14 / 4] & 32767) as usize * 2,
        };
        let dst = Raster {
            address: (r[0x20 / 4] & 0x3fffffff) as u64,
            width: (r[0x28 / 4] & 8191) as usize,
            height: (r[0x28 / 4] >> 16 & 8191) as usize,
            pitch: (r[0x24 / 4] & 32767) as usize * 2,
        };
        src.validate(src.width * 2)?;
        dst.validate(dst.width * 2)?;
        if self.dma.deadline().is_some()
            || r[0x7c / 4] & 0x1000 == 0
            || r[2] > 2
            || [0x2c, 0x44, 0x48, 0x4c, 0x50, 0x58, 0x5c, 0x70]
                .iter()
                .any(|&o| r[o / 4] != 0)
            || [src.width, dst.width, src.pitch / 2, dst.pitch / 2]
                .iter()
                .any(|v| v % 8 != 0)
            || src.pitch > 16384
            || dst.pitch > 16384
            || ![r[0x40 / 4], r[0x54 / 4]]
                .iter()
                .all(|&v| v > 0 && v <= 0x1ffff)
        {
            return Err("scaler geometry/transform is unsupported".into());
        }
        self.source.resize(src.pitch * src.height, 0);
        cx.ram_read(src.address, &mut self.source)?;
        self.dma.prepare(cx.now(), self.delay, dst, dst.width * 2)?;
        if let Err(e) = fujimod_emulator_image_processing::resample::bilinear(
            &self.source,
            src.width,
            src.height,
            src.pitch,
            dst.width,
            dst.height,
            [r[0x40 / 4], r[0x54 / 4]],
            &mut self.dma.pixels,
        ) {
            self.dma.cancel();
            return Err(e);
        }
        Ok(())
    }
}
impl Device for Scaler {
    fujimod_emulator_devices::checkpoint_fields!(r: Vec<u32>, dma: Transfer, #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")] source: Vec<u8>);
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"r":self.r,"dma":self.dma.fingerprint(),"source":fujimod_emulator_devices::fingerprint::bytes(&self.source)}),
        )
    }

    fn name(&self) -> &'static str {
        "exr2-isp-scaler"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        let Some(o) = word(cx, a, s, 0xfff47000, 0x84) else {
            return 0;
        };
        if !Self::valid(o) {
            cx.unmodeled();
            return 0;
        }
        self.r[o / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        let Some(o) = word(cx, a, s, 0xfff47000, 0x84) else {
            return;
        };
        if !Self::valid(o) || v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if o == 0 {
            if v != 0 {
                cx.fail("unknown scaler reset");
                return;
            }
            self.dma.cancel();
            self.r[0x74 / 4] = 0;
        } else if o == 0x78 {
            self.r[0x74 / 4] &= !(v as u32);
        } else if o == 4 {
            if v & !3 != 0 {
                cx.fail("unknown scaler START bits");
                return;
            }
            if v & 1 != 0
                && let Err(e) = self.start(cx)
            {
                cx.fail(e);
                return;
            }
        } else if self.dma.deadline().is_some() {
            cx.fail("scaler reconfiguration during DMA");
            return;
        }
        self.r[o / 4] = v as u32;
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.dma.deadline()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        match self.dma.write(cx) {
            Ok(true) => {
                self.r[0x74 / 4] = 1;
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
        serde_json::json!((&self.r, &self.dma))
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let (r, d): (Vec<u32>, Transfer) =
            serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if r.len() != self.r.len() {
            return Err("scaler bank size".into());
        }
        self.r = r;
        self.dma = d;
        Ok(())
    }
}
