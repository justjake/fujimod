//! FFF47000 packed422 scaler with centered phase sampling. Register fields define
//! addresses, pitches, centers, phases and 16.16 steps. Bilinear sampling is
//! a declared numerical approximation.
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::exr2_isp::dma::{Raster, Transfer};
use fujimod_emulator_imaging::exr2_isp::options::scaler::Options;
use fujimod_emulator_imaging::exr2_isp::word;
pub struct Scaler {
    r: Vec<u32>,
    irq: Irq,
    delay: u64,
    dma: Transfer,
    source: Vec<u8>,
}

fn has_zero_chroma(source: &[u8], raster: &Raster) -> bool {
    source
        .chunks_exact(raster.pitch)
        .take(raster.height)
        .any(|row| {
            row[..raster.width * 2]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pair| pair[2] == 0 || pair[3] == 0)
        })
}
// Centers and the destination rectangle are integer sample coordinates;
// phases are signed Q16 offsets. Samples use packed Y0 Y1 Cb Cr.
fn centered_packed422(
    source: &[u8],
    src: Raster,
    dst: Raster,
    transform: [i32; 4],
    phase: [u32; 2],
    input_center: u32,
    output_center: u32,
    origin: [u32; 2],
    reject_tags: bool,
    out: &mut [u8],
) -> Result<(), String> {
    // The vertical affine pass feeds the horizontal shear. Both retain Q16
    // precision here; bilinear sampling remains a declared approximation.
    let fixed = |x: usize, y: usize| -> [i64; 2] {
        let center = |word: u32, axis: usize| i64::from(word >> (axis * 16) & 8191);
        let signed_phase = |axis: usize| i64::from((phase[axis] << 2) as i32 >> 2);
        let dx = i64::from(origin[0]) + x as i64 - center(output_center, 0);
        let dy = i64::from(origin[1]) + y as i64 - center(output_center, 1);
        let sy = i64::from(transform[2]) * dx + i64::from(transform[3]) * dy + signed_phase(1);
        let sx =
            i64::from(transform[0]) * dx + i64::from(transform[1]) * sy / 65536 + signed_phase(0);
        [
            (center(input_center, 0) << 16) + sx,
            (center(input_center, 1) << 16) + sy,
        ]
    };
    let corners = [
        fixed(0, 0),
        fixed(dst.width - 1, 0),
        fixed(0, dst.height - 1),
        fixed(dst.width - 1, dst.height - 1),
    ];
    for axis in 0..2 {
        let bound = if axis == 0 { src.width } else { src.height };
        if corners.iter().all(|p| p[axis] < 0)
            || corners.iter().all(|p| p[axis] > ((bound - 1) as i64) << 16)
        {
            return Err("scaler centered span misses source raster".into());
        }
    }
    // The common zero-shear transform separates into two axes. Cache those
    // coordinates once per column/row, retaining identical Q16 sampling.
    let axes = if transform[1] == 0 && transform[2] == 0 {
        Some((
            (0..dst.width).map(|x| fixed(x, 0)[0]).collect::<Vec<_>>(),
            (0..dst.height).map(|y| fixed(0, y)[1]).collect::<Vec<_>>(),
        ))
    } else {
        None
    };
    for y in 0..dst.height {
        for x in (0..dst.width).step_by(2) {
            let at = (y * dst.width + x) * 2;
            for lane in 0..4 {
                let channel = if lane < 2 { 0 } else { lane - 1 };
                let index = if channel == 0 { x + lane } else { x };
                let point = if let Some((xs, ys)) = &axes {
                    [xs[index], ys[y]]
                } else {
                    fixed(index, y)
                };
                let mut fx = point[0].clamp(0, ((src.width - 1) as i64) << 16) as u64;
                let fy = point[1].clamp(0, ((src.height - 1) as i64) << 16) as u64;
                if channel != 0 {
                    fx /= 2;
                }
                let width = if channel == 0 {
                    src.width
                } else {
                    src.width / 2
                };
                let (x0, y0) = ((fx >> 16) as usize, (fy >> 16) as usize);
                let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(src.height - 1));
                let (dx, dy) = (fx & 65535, fy & 65535);
                if reject_tags {
                    // Unused source pixels do not require a transformed-tag
                    // law. Refuse if a sampling tap or its following row has
                    // a tag; that support includes the identity validity law.
                    for row in [y0, y1, (y1 + 1).min(src.height - 1)] {
                        for column in [x0, x1] {
                            let pair = if channel == 0 { column / 2 } else { column };
                            let at = row * src.pitch + pair * 4;
                            if source[at + 2] == 0 || source[at + 3] == 0 {
                                return Err(
                                    "scaler zero-chroma validity tags are unsupported".into()
                                );
                            }
                        }
                    }
                }
                let sample = |x: usize, y: usize| {
                    let pixel = if channel == 0 { x } else { x * 2 };
                    let i = y * src.pitch
                        + pixel / 2 * 4
                        + if channel == 0 { pixel % 2 } else { channel + 1 };
                    u64::from(source[i])
                };
                let a = sample(x0, y0) * (65536 - dx) + sample(x1, y0) * dx;
                let b = sample(x0, y1) * (65536 - dx) + sample(x1, y1) * dx;
                out[at + lane] = ((a * (65536 - dy) + b * dy + (1 << 31)) >> 32).min(255) as u8;
            }
        }
    }
    Ok(())
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
    fn start(&mut self, cx: &Cx<'_>, mode: u32) -> Result<(), String> {
        let r = &self.r;
        let src = Raster {
            address: (r[0x10 / 4] & 0x3fffffff) as u64,
            width: (r[0x18 / 4] & 8191) as usize,
            height: (r[0x18 / 4] >> 16 & 8191) as usize,
            pitch: (r[0x14 / 4] & 32767) as usize * 2,
        };
        let mut dst = Raster {
            address: (r[0x20 / 4] & 0x3fffffff) as u64,
            width: (r[0x28 / 4] & 8191) as usize,
            height: (r[0x28 / 4] >> 16 & 8191) as usize,
            pitch: (r[0x24 / 4] & 32767) as usize * 2,
        };
        src.validate(src.width * 2)?;
        dst.validate(dst.width * 2)?;
        let centered = [0x44, 0x4c, 0x50, 0x5c, 0x60, 0x64]
            .iter()
            .any(|&o| r[o / 4] != 0);
        // The driver programs +0x40..+0x64 only when a request carries a
        // transform; otherwise the block keeps its prior contents. A block
        // never written (zero) with equal source and destination geometry is
        // a 1:1 copy. Zero steps with any other geometry stay refused below.
        let steps = match [r[0x40 / 4], r[0x54 / 4]] {
            [0, 0] if !centered && src.width == dst.width && src.height == dst.height => {
                [0x10000; 2]
            }
            steps => steps,
        };
        if centered
            && [0x2c, 0x60, 0x64]
                .iter()
                .any(|&o| r[o / 4] & !0x1fff1fff != 0)
        {
            return Err("scaler centered fields are unsupported".into());
        }
        if self.dma.deadline().is_some()
            || r[0x7c / 4] & 0x1000 == 0
            || r[2] > 2
            || [0x44, 0x50].iter().any(|&o| r[o / 4] & !0x3ffff != 0)
            || [0x48, 0x58, 0x70].iter().any(|&o| r[o / 4] != 0)
            || !centered && [0x2c, 0x4c, 0x5c].iter().any(|&o| r[o / 4] != 0)
            || centered && [0x4c, 0x5c].iter().any(|&o| r[o / 4] & !0x3fffffff != 0)
            || [src.width, dst.width, src.pitch / 2, dst.pitch / 2]
                .iter()
                .any(|v| v % 8 != 0)
            || src.pitch > 16384
            || dst.pitch > 16384
            || !steps.iter().all(|&v| v > 0 && v <= 0x1ffff)
        {
            return Err("scaler geometry/transform is unsupported".into());
        }
        self.source.resize(src.pitch * src.height, 0);
        cx.ram_read(src.address, &mut self.source)?;
        // Measured identity transfers invalidate a pair when its own or the
        // following source row has zero Cb/Cr. Transformed samples touching
        // tags and identity last-row tags still lack a measured contract.
        let tagged = mode == 1 && has_zero_chroma(&self.source, &src);
        let identity = src.width == dst.width
            && src.height == dst.height
            && steps == [65536; 2]
            && [0x2c, 0x44, 0x4c, 0x50, 0x5c]
                .iter()
                .all(|&o| r[o / 4] == 0)
            && r[0x60 / 4] == r[0x64 / 4];
        let last_row_tagged = tagged
            && self.source[(src.height - 1) * src.pitch..][..src.width * 2]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pair| pair[2] == 0 || pair[3] == 0);
        if tagged && ((!identity && !centered) || (identity && last_row_tagged)) {
            return Err("scaler zero-chroma validity tags are unsupported".into());
        }
        if centered {
            let origin = [r[0x2c / 4] & 8191, r[0x2c / 4] >> 16 & 8191];
            if (origin[0] as usize + dst.width) * 2 > dst.pitch {
                return Err("scaler destination rectangle exceeds row pitch".into());
            }
            dst.address += u64::from(origin[1]) * dst.pitch as u64 + u64::from(origin[0]) * 2;
            self.dma.prepare(cx.now(), self.delay, dst, dst.width * 2)?;
            if let Err(error) = centered_packed422(
                &self.source,
                src,
                dst,
                [0x40, 0x44, 0x50, 0x54].map(|o| (r[o / 4] << 14) as i32 >> 14),
                [r[0x4c / 4], r[0x5c / 4]],
                r[0x60 / 4],
                r[0x64 / 4],
                origin,
                tagged && !identity,
                &mut self.dma.pixels,
            ) {
                self.dma.cancel();
                return Err(error);
            }
        } else {
            self.dma.prepare(cx.now(), self.delay, dst, dst.width * 2)?;
            if let Err(e) = fujimod_emulator_image_processing::resample::bilinear(
                &self.source,
                src.width,
                src.height,
                src.pitch,
                dst.width,
                dst.height,
                steps,
                &mut self.dma.pixels,
            ) {
                self.dma.cancel();
                return Err(e);
            }
        }
        if tagged && identity {
            for y in 0..src.height - 1 {
                for x in (0..src.width * 2).step_by(4) {
                    let current = y * src.pitch + x;
                    let next = current + src.pitch;
                    if [current + 2, current + 3, next + 2, next + 3]
                        .iter()
                        .any(|&at| self.source[at] == 0)
                    {
                        self.dma.pixels[y * dst.width * 2 + x..][..4].fill(0);
                    }
                }
            }
        }
        Ok(())
    }
}
impl Device for Scaler {
    fujimod_emulator_devices::checkpoint_fields!(r: Vec<u32>, dma: Transfer, #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")] source: Vec<u8>);
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"r":self.r,"dma":self.dma.fingerprint(),"source":fujimod_emulator_devices::state::bytes(&self.source)}),
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
                && let Err(e) = self.start(cx, v as u32)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validity_tags_exclude_luma_and_stride_padding() {
        let raster = Raster {
            address: 0,
            width: 4,
            height: 1,
            pitch: 12,
        };
        let mut source = [0, 255, 80, 96, 12, 13, 90, 110, 0, 0, 0, 0];
        assert!(!has_zero_chroma(&source, &raster));
        for index in [2, 3, 6, 7] {
            let saved = source[index];
            source[index] = 0;
            assert!(has_zero_chroma(&source, &raster));
            source[index] = saved;
        }
    }
}
