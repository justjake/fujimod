//! FFF42000 window analysis. Y16 minimum-axis gradients, local 3x3 SAD
//! tracking, eight-byte result records, and latency are declared, unmeasured
//! numerical laws. Other pyramid/tuning words are retained without inventing
//! their silicon effects.
use fujimod_emulator_imaging::exr2_isp::{dma::{self, Raster, Transfer}, due};
use fujimod_emulator_devices::exr2::irq_routes::{Irq, IrqRoutes};
use fujimod_emulator_image_processing::analysis::{self, WindowGrid};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::exr2_isp::options::window_analysis::Options;
const BASE: u64 = 0xfff42000;
const IMAGE_BYTES: usize = 8 << 20;
pub struct Wiring {
    pub routes: IrqRoutes,
}
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Completion {
    route: Irq,
}
pub struct WindowAnalysis {
    options: Options,
    wiring: Wiring,
    registers: [u32; 528],
    pending: Option<Completion>,
    first: Vec<u8>,
    second: Vec<u8>,
    features: Vec<u8>,
    dma: Transfer,
}
impl WindowAnalysis {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if options.ticks_per_window == 0 {
            return Err("window clock must be positive".into());
        }
        let mut dma = Transfer::default();
        dma.pixels.resize(2048 * 8, 0);
        Ok(Self {
            options,
            wiring,
            registers: [0; 528],
            pending: None,
            first: vec![0; IMAGE_BYTES],
            second: vec![0; IMAGE_BYTES],
            features: vec![0; 2048 * 8],
            dma,
        })
    }
    fn offset(address: u64, size: u8) -> Result<usize, String> {
        let o = address.wrapping_sub(BASE);
        if size != 4
            || o & 3 != 0
            || !matches!(o,0..=0x54|0x60..=0x78|0x80..=0x9c|0x104|0x10c|0x120..=0x144|0x184|0x18c|0x1a0..=0x1c4|0x1f4|0x1f8|0x800..=0x83c)
        {
            return Err("unknown image-window word register".into());
        }
        Ok(o as usize)
    }
    fn memory(cx: &Cx<'_>, address: u64, out: &mut [u8]) -> Result<(), String> {
        if address & 7 != 0 || out.len() > IMAGE_BYTES {
            return Err("image-window DMA exceeds aligned bounded domain".into());
        }
        dma::physical(address, out.len())?;
        cx.ram_read(address, out)
    }
    fn start(&mut self, cx: &Cx<'_>) -> Result<(), String> {
        let r = &self.registers;
        let kind = r[1] & 3;
        let mode = (r[1] >> 8) & 3;
        if r[1] & !0x303 != 0
            || !matches!(kind, 1 | 2)
            || mode != 2
            || r[0x1f8 / 4] != 0x10000000
            || r[0x104 / 4] != 1
            || r[0x184 / 4] != 0x10001
        {
            return Err("image-window start needs declared mode and two armed DMA channels".into());
        }
        let width = ((r[0x50 / 4] >> 16) & 0x1fff) as usize + 1;
        let height = (r[0x50 / 4] & 0x1fff) as usize + 1;
        let count = (r[0x9c / 4] & 0x7ff) as usize + 1;
        let bytes = ((r[0x1a8 / 4] >> 2) & 0x3ffffff) as usize;
        if !(3..=2048).contains(&width) || !(3..=2048).contains(&height) || bytes != count * 8 {
            return Err("unsupported image-window dimensions or output count".into());
        }
        let image_bytes = width * height * 2;
        let destination = if kind == 1 { r[0x48 / 4] } else { r[0x4c / 4] } as u64;
        if destination & 7 != 0 {
            return Err("unaligned image-window result destination".into());
        }
        dma::physical(destination, bytes)?;
        let route = self.wiring.routes.latch(0x46)?;
        let delay = (count as u64)
            .checked_mul(self.options.ticks_per_window)
            .ok_or("window latency overflows")?;
        due(cx.now(), delay)?;
        self.dma.pixels.resize(2048 * 8, 0);
        Self::memory(cx, r[0x10 / 4] as u64, &mut self.first[..image_bytes])?;
        if kind == 1 {
            let columns = ((r[0x60 / 4] >> 16) & 0x3f) as usize + 1;
            let rows = (r[0x60 / 4] & 0x3f) as usize + 1;
            let origin = [
                ((r[0x64 / 4] >> 16) & 0x1fff) as usize,
                (r[0x64 / 4] & 0x1fff) as usize,
            ];
            let window = [
                ((r[0x68 / 4] >> 16) & 0xff) as usize + 1,
                (r[0x68 / 4] & 0xff) as usize + 1,
            ];
            let step = [
                ((r[0x6c / 4] >> 16) & 0xfff) as usize + 1,
                (r[0x6c / 4] & 0xfff) as usize + 1,
            ];
            if columns * rows != count
                || origin[0] + (columns - 1) * step[0] + window[0] > width
                || origin[1] + (rows - 1) * step[1] + window[1] > height
            {
                return Err("image-window grid exceeds its source raster".into());
            }
            analysis::window_features(
                &self.first[..image_bytes],
                width,
                height,
                WindowGrid {
                    columns,
                    rows,
                    origin,
                    window,
                    step,
                },
                &mut self.dma.pixels[..bytes],
            )?;
        } else {
            Self::memory(cx, r[0x2c / 4] as u64, &mut self.second[..image_bytes])?;
            Self::memory(cx, r[0x48 / 4] as u64, &mut self.features[..bytes])?;
            analysis::track_features(
                &self.first[..image_bytes],
                &self.second[..image_bytes],
                &self.features[..bytes],
                width,
                height,
                &mut self.dma.pixels[..bytes],
            )?;
        }
        self.dma.prepare(
            cx.now(),
            delay,
            Raster {
                address: destination,
                width: count,
                height: 1,
                pitch: bytes,
            },
            bytes,
        )?;
        self.pending = Some(Completion { route });
        self.registers[0] = 1;
        Ok(())
    }
    fn run(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if let Some(p) = self.pending.filter(|_| self.dma.ready(cx.now())) {
            self.wiring.routes.validate(p.route)?;
            self.dma.write(cx)?;
            self.registers[0] = 0x10000;
            self.pending = None;
            p.route.raise(cx)?;
        }
        Ok(())
    }
}
impl Device for WindowAnalysis {
    fujimod_emulator_devices::checkpoint_fields!(#[serde(with="fujimod_emulator_devices::checkpoint::array")] registers: [u32;528], pending: Option<Completion>, #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")] first: Vec<u8>, #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")] second: Vec<u8>, #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")] features: Vec<u8>, dma: Transfer);
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"registers":self.registers.as_slice(),"pending":self.pending,"dma":self.dma.fingerprint(),
                "first":fujimod_emulator_devices::fingerprint::bytes(&self.first),"second":fujimod_emulator_devices::fingerprint::bytes(&self.second),
                "features":fujimod_emulator_devices::fingerprint::bytes(&self.features)}),
        )
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"registers":self.registers.as_slice(),"dma_due":self.dma.deadline()})
    }
    fn name(&self) -> &'static str {
        "image-window-analysis"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match Self::offset(address, size) {
            Ok(0) => self.registers[0] as u64,
            _ => {
                cx.fail("image-window configuration getter is unmodeled");
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let result: Result<(), String> = (|| {
            let o = Self::offset(address, size)?;
            let v = u32::try_from(value).map_err(|_| "window word exceeds 32 bits")?;
            if o == 0 && matches!(v, 0 | 0x10000) {
                self.pending = None;
                self.dma.cancel();
                self.registers[0] = v;
                return Ok(());
            }
            if self.pending.is_some() {
                return Err("image-window request changed while active".into());
            }
            if o == 0 {
                if v != 1 {
                    return Err("unknown image-window control".into());
                }
                self.start(cx)
            } else {
                self.registers[o / 4] = v;
                Ok(())
            }
        })();
        if let Err(e) = result {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.dma.deadline()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.run(cx) {
            cx.fail(e);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        address == BASE && size == 4
    }
}
