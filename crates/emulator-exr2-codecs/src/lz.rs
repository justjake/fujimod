//! EXR II shared LZ/MOVRSZ engine at FFF43000, wired to physical IRQ 45.
//! No program address participates in this model.
//!
//! LZ throughput, encoded-zero 512 KiB count, malformed-stream status 101,
//! source latching and MOVRSZ box/filter arithmetic are declared unmeasured
//! laws. Supported descriptor forms and bounded scalar half-size FIR
//! descriptors are admitted. The
//! default resize mode awaits a result and has no manufactured completion.
use fujimod_emulator_devices::exr2::irq;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device, compression::method4::Stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const BASE: u64 = 0xfff43000;
pub const IRQ: u32 = 0x45;
const SOURCE_SAMPLES: usize = 1024 * 480;
const SOURCE_BYTES: usize = SOURCE_SAMPLES * 2;
const OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const ORDER: [usize; 12] = [
    8, 0x10, 0x14, 0x18, 0x20, 0x24, 0x28, 0x30, 0x34, 0x38, 0x3c, 0x80,
];
const MASKS: [u32; 12] = [
    0x11113,
    u32::MAX,
    0x7ff0,
    0x1fff1fff,
    u32::MAX,
    0x7ff0,
    0x1fff1fff,
    0x3ff,
    0x7f7f7f,
    u32::MAX,
    0x3ff03ff,
    0x31ff31ff,
];
const PYRAMID: [[u32; 6]; 4] = [
    [0x500, 0x01e00280, 0x280, 0x00f00140, 0x00a00140, 0x10001000],
    [0x280, 0x00f00140, 0x140, 0x007800a0, 0x00a00140, 0x10001000],
    [0x140, 0x007800a0, 0xa0, 0x003c0050, 0x005000a0, 0x1000],
    [0xa0, 0x003c0050, 0x50, 0x001e0028, 0x00280050, 0],
];
const FILTERED: [[u32; 6]; 4] = [
    [0x340, 0x015001c0, 0x340, 0x00a80100, 0x00800100, 0x20002000],
    [0x340, 0x00a80100, 0x340, 0x00540080, 0x00800100, 0x20002000],
    [0x340, 0x00540080, 0x340, 0x002a0040, 0x00400080, 0x10001000],
    [0x340, 0x002a0040, 0x340, 0x00150020, 0x00200040, 0],
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum RasterKind {
    BoxHalf,
    PackedFilter,
    FilteredRational,
    ScalarFilteredHalf,
    ScalarWeightedHalf,
    ScalarThresholdedHalf,
    LumaFilteredHalf,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResizeMode {
    #[default]
    AwaitResult,
    DeclaredPackedPyramid,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub bytes_per_tick: u64,
    pub resize: ResizeMode,
    pub resize_ticks: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            bytes_per_tick: 512,
            resize: ResizeMode::AwaitResult,
            resize_ticks: 400000,
        }
    }
}
pub struct Wiring {
    pub base: u64,
    pub controllers: Vec<(usize, irq::Inputs)>,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum Phase {
    Configuring,
    Configured,
    Awaiting,
    Active,
    Completed,
    Acknowledged,
    Released,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resize {
    owner: usize,
    step: usize,
    phase: Phase,
    enabled: bool,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
enum Job {
    Lz,
    Resize,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    due: u64,
    job: Job,
    source_len: usize,
    destination: u64,
    irq: Option<Irq>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    regs: Vec<u32>,
    stream: Stream,
    next_destination: Option<u64>,
    reset_due: Option<u64>,
    pending: Option<Pending>,
    resize: Option<Resize>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            regs: vec![0; 36],
            stream: Stream::default(),
            next_destination: None,
            reset_due: None,
            pending: None,
            resize: None,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    base: u64,
    options: Options,
    controllers: Vec<usize>,
    state: State,
    #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")]
    source: Vec<u8>,
}
pub struct Decompressor {
    options: Options,
    base: u64,
    controllers: Vec<(usize, irq::Inputs)>,
    state: State,
    source: Box<[u8]>,
    output: Box<[u8]>,
    a: Box<[u16]>,
    b: Box<[u16]>,
}
impl Decompressor {
    pub fn new(options: Options, w: Wiring) -> Result<Self, String> {
        if options.bytes_per_tick == 0
            || options.resize_ticks == 0
            || w.base & 255 != 0
            || w.base.checked_add(0x100).is_none()
            || w.controllers.iter().any(|(c, _)| *c > 1)
            || w.controllers.len() > 2
            || w.controllers.len() == 2 && w.controllers[0].0 == w.controllers[1].0
        {
            return Err("invalid EXR II decompressor options/wiring".into());
        }
        Ok(Self {
            options,
            base: w.base,
            controllers: w.controllers,
            state: State::default(),
            source: vec![0; SOURCE_BYTES].into_boxed_slice(),
            output: vec![0; OUTPUT_BYTES].into_boxed_slice(),
            a: vec![0; SOURCE_SAMPLES].into_boxed_slice(),
            b: vec![0; SOURCE_SAMPLES].into_boxed_slice(),
        })
    }
    fn offset(&self, address: u64, size: u8) -> Option<usize> {
        let o = usize::try_from(address.checked_sub(self.base)?).ok()?;
        (size == 4
            && (ORDER.contains(&o) || matches!(o, 0 | 4 | 0x40 | 0x44 | 0x48 | 0x4c | 0x88 | 0x8c)))
        .then_some(o)
    }
    fn r(&self, o: usize) -> u32 {
        self.state.regs[o / 4]
    }
    fn set(&mut self, o: usize, v: u32) {
        self.state.regs[o / 4] = v;
    }
    fn route(&self) -> Result<Option<Irq>, String> {
        let mut route = None;
        for (core, input) in &self.controllers {
            if input.enabled() >> IRQ & 1 != 0 {
                if route.is_some() {
                    return Err("ambiguous decompressor physical IRQ route".into());
                }
                route = Some(Irq {
                    core: *core,
                    line: IRQ,
                });
            }
        }
        Ok(route)
    }
    fn configure(&mut self, cpu: usize, o: usize, v: u32) -> Result<(), String> {
        if self.state.pending.is_some() || self.r(0) != 1 || cpu > 1 {
            return Err("MOVRSZ requires ready hardware and a real owner".into());
        }
        let step = if let Some(r) = &self.state.resize {
            if r.owner != cpu {
                return Err("MOVRSZ configuration is closed or owner changed".into());
            }
            match r.phase {
                Phase::Configuring => r.step,
                Phase::Configured if o == ORDER[0] => 0,
                _ => return Err("MOVRSZ configuration is closed or owner changed".into()),
            }
        } else {
            0
        };
        if ORDER.get(step) != Some(&o)
            || v & !MASKS[step] != 0
            || matches!(o, 0x10 | 0x20) && v & 15 != 0
            || matches!(o, 0x18 | 0x28) && v & 7 != 0
            || o == 0x34
                && self.r(8) & 0x1000 != 0
                && (v & 127) + 2 * ((v >> 8 & 127) + (v >> 16 & 127)) != 128
            || o == 0x38
                && (v & 255) + 2 * ((v >> 8 & 255) + (v >> 16 & 255) + (v >> 24 & 255)) != 256
            || o == 0x3c
                && [v & 1023, v >> 16 & 1023]
                    .iter()
                    .any(|v| *v > 512 || v & 7 != 0)
        {
            return Err("unsupported MOVRSZ configuration word/order/alignment".into());
        }
        self.set(o, v);
        self.state.resize = Some(Resize {
            owner: cpu,
            step: step + 1,
            phase: if step + 1 == ORDER.len() {
                Phase::Configured
            } else {
                Phase::Configuring
            },
            enabled: false,
        });
        Ok(())
    }
    fn start_lz(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.r(0) != 1
            || self.state.pending.is_some()
            || self.r(0x80) != 0x30003000
            || self.r(0x8c) != 1
            || self.r(0x44) & !0x7ffff != 0
        {
            return Err("unsupported decompressor start/setup or busy hardware".into());
        }
        let count = match self.r(0x44) {
            0 => 0x80000,
            n => n as usize,
        };
        let destination = self.r(0x48) as u64;
        if self
            .state
            .next_destination
            .is_some_and(|d| d != destination)
        {
            return Err("noncontiguous method-4 destination".into());
        }
        let irq = self.route()?;
        let due = cx
            .now()
            .checked_add((count as u64 / self.options.bytes_per_tick).max(2))
            .ok_or("LZ deadline overflow")?;
        cx.ram_read(self.r(0x40) as u64, &mut self.source[..count])?;
        self.set(4, 2);
        self.set(0x88, 0);
        self.set(0x4c, 0);
        self.state.pending = Some(Pending {
            due,
            job: Job::Lz,
            source_len: count,
            destination,
            irq,
        });
        Ok(())
    }
    fn raster_kind(&self) -> Result<RasterKind, String> {
        Self::raster_kind_for(&self.state.regs)
    }
    fn raster_kind_for(regs: &[u32]) -> Result<RasterKind, String> {
        let r = |o: usize| regs[o / 4];
        let stage = [r(0x14), r(0x18), r(0x24), r(0x28), r(0x3c), r(0x80)];
        if PYRAMID.contains(&stage)
            && r(8) == 0
            && r(0x30) == 0x40
            && r(0x34) == 0x2040
            && r(0x38) == 0x4080
        {
            return Ok(RasterKind::BoxHalf);
        }
        if stage == [0x280, 0x00f00140, 0x340, 0x00f00140, 0x01400140, 0x213c2000]
            && r(8) == 0x10000
            && r(0x30) == 0x40
            && r(0x34) == 0x082030
            && r(0x38) == 0x020a3e6c
        {
            return Ok(RasterKind::PackedFilter);
        }
        if FILTERED.contains(&stage)
            && r(8)
                == if stage[1] == 0x015001c0 {
                    0x1010
                } else {
                    0x1011
                }
            && r(0x30) == 0x40
            && r(0x34) == 0x141c20
            && r(0x38) == 0x00223c44
        {
            return Ok(RasterKind::FilteredRational);
        }
        let (sw, sh) = (r(0x18) & 8191, r(0x18) >> 16);
        let (dw, dh) = (r(0x28) & 8191, r(0x28) >> 16);
        // This bounded scalar descriptor supplies the saved threshold and
        // asymmetric kernels. Its pixel operation is not decoded; raster()
        // applies a small empirical luma curve after the declared FIR.
        if r(8) == 0x1100
            && r(0x30) == 0x4c
            && r(0x34) == 0x00191a1a
            && r(0x38) == 0x24242526
            && r(0x3c) == 0x00400080
            && r(0x80) == 0
            && sw == 720
            && sh == 480
            && dw == 368
            && dh == 240
            && r(0x14) == 1440
            && r(0x24) == 1440
        {
            return Ok(RasterKind::ScalarThresholdedHalf);
        }
        // The matching unthresholded kernel is used by the following
        // half-size stages. Keep its exact weights while allowing the same
        // bounded dimensions and row-padded layouts as the scalar FIR path.
        if r(8) == 0x1000
            && r(0x30) == 0x40
            && r(0x34) == 0x00191a1a
            && r(0x38) == 0x24242526
            && r(0x3c) == 0x00400080
            && r(0x80) == 0
            && (1..=720).contains(&sw)
            && (1..=480).contains(&sh)
            && dh > 0
            && dh * 2 == sh
            && dw >= sw.div_ceil(2)
            && dw <= sw.div_ceil(2) + 15
            && r(0x14) >= sw * 2
            && r(0x14) <= 1440
            && r(0x24) >= dw * 2
            && r(0x24) <= 1440
        {
            return Ok(RasterKind::ScalarWeightedHalf);
        }
        // Declared replicated-edge FIR with a 2:1 pixel-center step. The
        // destination may include up to 15 replicated columns for alignment.
        // Format/control and both normalized kernels remain explicit; an
        // arbitrary new descriptor does not acquire a completion law.
        if r(8) == 0x1000
            && r(0x30) == 0x40
            && r(0x34) == 0x082030
            && r(0x38) == 0x081e3844
            && r(0x3c) == 0x00400080
            && r(0x80) == 0
            && (1..=720).contains(&sw)
            && (1..=480).contains(&sh)
            && dh > 0
            && dh * 2 == sh
            && dw >= sw.div_ceil(2)
            && dw <= sw.div_ceil(2) + 15
            && r(0x14) >= sw * 2
            && r(0x14) <= 1440
            && r(0x24) >= dw * 2
            && r(0x24) <= 1440
        {
            return Ok(RasterKind::ScalarFilteredHalf);
        }
        // The packing controls select packed 422 input and byte-plane output.
        // Register 80 describes 128/64 transfer blocks, not sample formats.
        // This law shares the declared replicated-edge FIR and half-size
        // pixel-center sampling, retaining only the luma component.
        if r(8) == 0x10
            && r(0x30) == 0x40
            && r(0x34) == 0x082030
            && r(0x38) == 0x081e3844
            && r(0x3c) == 0x01000200
            && r(0x80) == 0x20003000
            && (16..=1024).contains(&sw)
            && (2..=480).contains(&sh)
            && dw * 2 == sw
            && dh * 2 == sh
            && r(0x14) >= sw * 2
            && r(0x14) <= 2048
            && r(0x24) >= dw
            && r(0x24) <= 2048
        {
            return Ok(RasterKind::LumaFilteredHalf);
        }
        let registers = [0x08, 0x14, 0x18, 0x24, 0x28, 0x30, 0x34, 0x38, 0x3c, 0x80]
            .map(|offset| (offset, r(offset)));
        Err(format!(
            "MOVRSZ descriptor has no selected numerical law: {registers:x?}"
        ))
    }
    fn resize_alias_conflicts(
        regs: &[u32],
        kind: RasterKind,
        source: std::ops::Range<u64>,
        dest: std::ops::Range<u64>,
    ) -> Result<bool, String> {
        if source.start >= dest.end
            || dest.start >= source.end
            || kind == RasterKind::FilteredRational
        {
            return Ok(false);
        }
        if !matches!(
            kind,
            RasterKind::ScalarFilteredHalf
                | RasterKind::ScalarWeightedHalf
                | RasterKind::ScalarThresholdedHalf
                | RasterKind::LumaFilteredHalf
        ) {
            return Ok(true);
        }
        let raster = |address, pitch, dimensions: u32| fujimod_emulator_imaging::exr2_isp::dma::Raster {
            address,
            pitch,
            width: (dimensions & 8191) as usize,
            height: (dimensions >> 16) as usize,
        };
        let a = raster(source.start, regs[0x14 / 4] as usize, regs[0x18 / 4]);
        let b = raster(dest.start, regs[0x24 / 4] as usize, regs[0x28 / 4]);
        a.active_rows_overlap(
            a.width * 2,
            b,
            b.width
                * if kind == RasterKind::LumaFilteredHalf {
                    1
                } else {
                    2
                },
        )
    }
    fn start_resize(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let r = self
            .state
            .resize
            .as_ref()
            .ok_or("MOVRSZ configuration absent")?;
        if r.owner != cx.cpu
            || r.phase != Phase::Configured
            || !r.enabled
            || self.r(0) != 1
            || self.state.pending.is_some()
        {
            return Err("MOVRSZ start requires complete same-owner configuration".into());
        }
        let irq = self
            .route()?
            .ok_or("MOVRSZ requires one enabled physical IRQ route")?;
        if self.options.resize == ResizeMode::AwaitResult {
            self.set(4, 1);
            self.state.resize.as_mut().unwrap().phase = Phase::Awaiting;
            return Ok(());
        }
        let kind = self.raster_kind()?;
        let count = self.r(0x14) as usize * (self.r(0x18) >> 16) as usize;
        let bytes = self.r(0x24) as usize * (self.r(0x28) >> 16) as usize;
        let source = fujimod_emulator_imaging::exr2_isp::dma::span(self.r(0x10) as u64, count)?;
        let dest = fujimod_emulator_imaging::exr2_isp::dma::span(self.r(0x20) as u64, bytes)?;
        if count > SOURCE_BYTES
            || Self::resize_alias_conflicts(&self.state.regs, kind, source, dest)?
        {
            return Err("unsupported MOVRSZ source/destination extent/overlap".into());
        }
        let due = cx
            .now()
            .checked_add(self.options.resize_ticks)
            .ok_or("MOVRSZ deadline overflow")?;
        cx.ram_read(self.r(0x10) as u64, &mut self.source[..count])?;
        self.set(4, 1);
        self.set(0x88, 0);
        self.state.pending = Some(Pending {
            due,
            job: Job::Resize,
            source_len: count,
            destination: self.r(0x20) as u64,
            irq: Some(irq),
        });
        self.state.resize.as_mut().unwrap().phase = Phase::Active;
        Ok(())
    }
    fn raster(&mut self) -> Result<(usize, usize, usize), String> {
        let kind = self.raster_kind()?;
        let (sw, sh) = (self.r(0x18) as usize & 8191, (self.r(0x18) >> 16) as usize);
        let (w, h) = (self.r(0x28) as usize & 8191, (self.r(0x28) >> 16) as usize);
        let (sp, pitch) = (self.r(0x14) as usize, self.r(0x24) as usize);
        let (sf, df) = (self.r(0x80) >> 12 & 3, self.r(0x80) >> 28 & 3);
        if kind != RasterKind::BoxHalf {
            let width = if kind == RasterKind::PackedFilter {
                w
            } else {
                sw.min(sp / 2)
            };
            let (horizontal, vertical, ratio) = match kind {
                RasterKind::PackedFilter => {
                    ([2, 10, 62, 108, 62, 10, 2], [8, 32, 48, 32, 8], [1, 1])
                }
                RasterKind::ScalarFilteredHalf | RasterKind::LumaFilteredHalf => {
                    ([8, 30, 56, 68, 56, 30, 8], [8, 32, 48, 32, 8], [2, 1])
                }
                RasterKind::ScalarWeightedHalf | RasterKind::ScalarThresholdedHalf => {
                    ([36, 36, 37, 38, 37, 36, 36], [25, 26, 26, 26, 25], [2, 1])
                }
                RasterKind::FilteredRational => (
                    [0, 34, 60, 68, 60, 34, 0],
                    [20, 28, 32, 28, 20],
                    [
                        self.r(0x3c) as usize & 1023,
                        (self.r(0x3c) >> 16) as usize & 1023,
                    ],
                ),
                RasterKind::BoxHalf => unreachable!(),
            };
            use fujimod_emulator_image_processing::separable::Samples;
            let input = if sf == 0 && df == 0 {
                Samples::Scalar16
            } else {
                Samples::YyCbCr422
            };
            let output = if kind == RasterKind::LumaFilteredHalf {
                Samples::Luma8
            } else {
                input
            };
            let row_bytes = w * output.bytes_per_pixel();
            fujimod_emulator_image_processing::separable::filter(
                &self.source[..sp * sh],
                fujimod_emulator_image_processing::separable::Geometry {
                    width,
                    height: sh,
                    pitch: sp,
                    output_width: w,
                    output_height: h,
                    input,
                    output,
                },
                &horizontal,
                &vertical,
                ratio,
                &mut self.output[..row_bytes * h],
                &mut self.a,
                &mut self.b,
            )?;
            if kind == RasterKind::ScalarThresholdedHalf {
                // The measured 8-bit sample points (16→19, 21→24, 240→255)
                // fit this empirical curve. The register's exact operation
                // and its ordering relative to filtering remain unknown.
                for sample in self.output[..row_bytes * h].chunks_exact_mut(2) {
                    let value = u16::from_le_bytes([sample[0], sample[1]]);
                    let adjusted = (u32::from(value) + u32::from(value) / 16 + 2).min(255);
                    sample.copy_from_slice(&(adjusted as u16).to_le_bytes());
                }
            }
            return Ok((row_bytes, h, pitch));
        }
        let get = |x: usize, y: usize| -> u32 {
            if sf == 1 {
                self.source[y * sp + x / 2 * 4 + x % 2] as u32
            } else {
                u16::from_le_bytes(
                    self.source[y * sp + x * 2..y * sp + x * 2 + 2]
                        .try_into()
                        .unwrap(),
                ) as u32
            }
        };
        for y in 0..h {
            for x in 0..w {
                let luma = (get(x * 2, y * 2)
                    + get(x * 2 + 1, y * 2)
                    + get(x * 2, y * 2 + 1)
                    + get(x * 2 + 1, y * 2 + 1)
                    + 2)
                    / 4;
                if df == 0 {
                    self.output[(y * w + x) * 2..(y * w + x) * 2 + 2]
                        .copy_from_slice(&(luma as u16).to_le_bytes());
                } else {
                    self.output[y * w * 2 + x / 2 * 4 + x % 2] = luma as u8;
                }
            }
        }
        if df == 1 {
            for y in 0..h {
                for x in 0..w / 2 {
                    for c in 2..4 {
                        let at = y * 2 * sp + x * 8 + c;
                        self.output[y * w * 2 + x * 4 + c] = ((self.source[at] as u16
                            + self.source[at + 4] as u16
                            + self.source[at + sp] as u16
                            + self.source[at + sp + 4] as u16
                            + 2)
                            / 4) as u8;
                    }
                }
            }
        }
        Ok((w * 2, h, pitch))
    }
    fn write_word(&mut self, cx: &mut Cx<'_>, o: usize, v: u32) -> Result<(), String> {
        if o == 0x4c {
            return Err("decompressor output count is read-only".into());
        }
        if o == 0x88 {
            if v != u32::MAX && v & !0xf01 != 0 {
                return Err("unsupported decompressor W1C bits".into());
            }
            if let Some(r) = &mut self.state.resize {
                if self.options.resize != ResizeMode::AwaitResult {
                    if r.owner != cx.cpu
                        || !matches!(
                            r.phase,
                            Phase::Completed | Phase::Acknowledged | Phase::Released
                        )
                        || !matches!(v, 1 | u32::MAX)
                    {
                        return Err("MOVRSZ ACK before completion or by different owner".into());
                    }
                    if self.state.regs[0x88 / 4] & v & 1 != 0 {
                        r.phase = Phase::Acknowledged;
                    }
                }
            }
            self.set(o, self.r(o) & !v);
            return Ok(());
        }
        if ORDER.contains(&o) && (o != 0x80 || self.state.resize.is_some()) {
            return self.configure(cx.cpu, o, v);
        }
        if let Some(r) = &mut self.state.resize {
            if cx.cpu != r.owner {
                return Err("MOVRSZ shared engine owner changed".into());
            }
            match (o, v, r.phase) {
                (0x8c, 1, Phase::Configured) => {
                    r.enabled = true;
                    self.set(o, v);
                    return Ok(());
                }
                (4, 1, Phase::Configured) => return self.start_resize(cx),
                (0x8c, 1, Phase::Completed | Phase::Acknowledged) => return Ok(()),
                (0x8c, u32::MAX, Phase::Acknowledged) => {
                    r.phase = Phase::Released;
                    self.set(o, v);
                    return Ok(());
                }
                // The loader masks the shared IRQ before resetting the engine.
                (0x8c, 0, Phase::Released) => {
                    self.set(o, v);
                    return Ok(());
                }
                (0, 0, Phase::Released) => self.state.resize = None,
                _ => {
                    return Err(
                        "MOVRSZ active/configured shared-engine mutation is unsupported".into(),
                    );
                }
            }
        }
        if o == 0 {
            if v != 0 {
                return Err("unsupported decompressor reset".into());
            }
            self.state.pending = None;
            self.state.resize = None;
            self.state.stream.reset();
            self.state.next_destination = None;
            self.set(0, 0);
            self.set(0x88, 0);
            self.set(0x4c, 0);
            self.state.reset_due = Some(
                cx.now()
                    .checked_add(2)
                    .ok_or("decompressor reset deadline overflow")?,
            );
            return Ok(());
        }
        if self.state.pending.is_some() {
            return Err("decompressor descriptor changed during DMA".into());
        }
        if o == 0x8c && !matches!(v, 0 | 1) {
            return Err("unsupported decompressor interrupt enable".into());
        }
        if o == 4 {
            if v != 2 {
                return Err("unsupported decompressor trigger".into());
            }
            return self.start_lz(cx);
        }
        self.set(o, v);
        Ok(())
    }
    fn complete(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.state.reset_due.is_some_and(|d| cx.now() >= d) {
            self.state.reset_due = None;
            self.set(0, 1);
        }
        let Some(p) = self.state.pending.as_ref() else {
            return Ok(());
        };
        if cx.now() < p.due {
            return Ok(());
        }
        let route = self.route()?;
        if let Some(irq) = p.irq {
            let masked_resize = matches!(p.job, Job::Resize) && route.is_none();
            if !masked_resize && route.map(|r| (r.core, r.line)) != Some((irq.core, irq.line)) {
                return Err("decompressor IRQ route changed during DMA".into());
            }
        }
        let p = self.state.pending.take().unwrap();
        match p.job {
            Job::Lz => match self
                .state
                .stream
                .feed(&self.source[..p.source_len], &mut self.output)
            {
                Ok(count) => {
                    if count != 0 {
                        cx.ram_write(p.destination, &self.output[..count])?;
                    }
                    self.state.next_destination = Some(
                        p.destination
                            .checked_add(count as u64)
                            .ok_or("LZ destination overflow")?,
                    );
                    self.set(0x4c, count as u32);
                    self.set(0x88, 1);
                    if let Some(irq) = p.irq {
                        irq.raise(cx)?;
                    }
                }
                Err(_) => {
                    self.set(0x88, 0x101);
                }
            },
            Job::Resize => {
                let (row, rows, pitch) = self.raster()?;
                for y in 0..rows {
                    cx.ram_write(
                        p.destination + (y * pitch) as u64,
                        &self.output[y * row..(y + 1) * row],
                    )?;
                }
                self.set(0x88, 1);
                self.state.resize.as_mut().unwrap().phase = Phase::Completed;
                p.irq.ok_or("MOVRSZ pending route absent")?.raise(cx)?;
            }
        }
        Ok(())
    }
    fn validate_state(&self, s: &State) -> Result<(), String> {
        s.stream.validate()?;
        if s.regs.len() != 36 {
            return Err("invalid decompressor register extent".into());
        }
        let r = |o: usize| s.regs[o / 4];
        if s.regs.iter().enumerate().any(|(n, &v)| {
            v != 0
                && !ORDER.contains(&(n * 4))
                && !matches!(n * 4, 0 | 4 | 0x40 | 0x44 | 0x48 | 0x4c | 0x88 | 0x8c)
        }) || r(0) > 1
            || r(0x88) & !0xf01 != 0
            || r(0x4c) as usize > OUTPUT_BYTES
            || s.reset_due.is_some() && (r(0) != 0 || s.pending.is_some() || s.resize.is_some())
        {
            return Err("invalid decompressor register/reset state".into());
        }
        if let Some(resize) = &s.resize {
            if resize.owner > 1
                || resize.step == 0
                || resize.step > ORDER.len()
                || r(0) != 1
                || (resize.phase == Phase::Configuring) != (resize.step < ORDER.len())
                || resize.enabled && resize.phase == Phase::Configuring
                || !matches!(resize.phase, Phase::Configuring | Phase::Configured)
                    && !resize.enabled
                || resize.phase == Phase::Active
                    && s.pending
                        .as_ref()
                        .is_none_or(|p| !matches!(p.job, Job::Resize))
                || resize.phase == Phase::Awaiting && self.options.resize != ResizeMode::AwaitResult
                || matches!(
                    resize.phase,
                    Phase::Active | Phase::Completed | Phase::Acknowledged | Phase::Released
                ) && self.options.resize != ResizeMode::DeclaredPackedPyramid
                || resize.phase == Phase::Released && !matches!(r(0x8c), 0 | u32::MAX)
                || resize.enabled && resize.phase != Phase::Released && r(0x8c) != 1
                || !matches!(resize.phase, Phase::Configuring | Phase::Configured) && r(4) != 1
                || resize.phase == Phase::Completed && r(0x88) != 1
                || matches!(resize.phase, Phase::Acknowledged | Phase::Released) && r(0x88) != 0
            {
                return Err("invalid MOVRSZ phase/owner checkpoint".into());
            }
            for step in 0..resize.step {
                let o = ORDER[step];
                let v = r(o);
                if v & !MASKS[step] != 0
                    || matches!(o, 0x10 | 0x20) && v & 15 != 0
                    || matches!(o, 0x18 | 0x28) && v & 7 != 0
                    || o == 0x34
                        && r(8) & 0x1000 != 0
                        && (v & 127) + 2 * ((v >> 8 & 127) + (v >> 16 & 127)) != 128
                    || o == 0x38
                        && (v & 255) + 2 * ((v >> 8 & 255) + (v >> 16 & 255) + (v >> 24 & 255))
                            != 256
                    || o == 0x3c
                        && [v & 1023, v >> 16 & 1023]
                            .iter()
                            .any(|v| *v > 512 || v & 7 != 0)
                {
                    return Err("invalid MOVRSZ configuration checkpoint".into());
                }
            }
        }
        if let Some(p) = &s.pending {
            if p.due == 0
                || r(0) != 1
                || r(0x88) != 0
                || p.source_len == 0
                || p.source_len > SOURCE_BYTES
                || p.irq.is_some_and(|i| {
                    i.core > 1
                        || i.line != IRQ
                        || !self.controllers.iter().any(|(c, _)| *c == i.core)
                })
            {
                return Err("invalid decompressor pending job".into());
            }
            match p.job {
                Job::Lz => {
                    let count = match r(0x44) {
                        0 => 0x80000,
                        n => n as usize,
                    };
                    if s.resize.is_some()
                        || r(4) != 2
                        || r(0x80) != 0x30003000
                        || r(0x8c) != 1
                        || r(0x44) & !0x7ffff != 0
                        || p.source_len != count
                        || p.destination != r(0x48) as u64
                        || s.next_destination.is_some_and(|d| d != p.destination)
                    {
                        return Err("inconsistent pending LZ descriptor".into());
                    }
                }
                Job::Resize => {
                    let kind = Self::raster_kind_for(&s.regs)?;
                    let source = fujimod_emulator_imaging::exr2_isp::dma::span(
                        r(0x10) as u64,
                        r(0x14) as usize * (r(0x18) >> 16) as usize,
                    )?;
                    let dest = fujimod_emulator_imaging::exr2_isp::dma::span(
                        r(0x20) as u64,
                        r(0x24) as usize * (r(0x28) >> 16) as usize,
                    )?;
                    if s.resize.as_ref().is_none_or(|r| r.phase != Phase::Active)
                        || p.irq.is_none()
                        || p.source_len != (source.end - source.start) as usize
                        || p.destination != r(0x20) as u64
                        || Self::resize_alias_conflicts(&s.regs, kind, source, dest)?
                    {
                        return Err("inconsistent pending MOVRSZ descriptor".into());
                    }
                }
            }
        }
        Ok(())
    }
}
impl Device for Decompressor {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &Saved {
                base: self.base,
                options: self.options.clone(),
                controllers: self.controllers.iter().map(|(c, _)| *c).collect(),
                state: self.state.clone(),
                source: self.source[..self.state.pending.as_ref().map_or(0, |p| p.source_len)]
                    .to_vec(),
            },
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_saved(&saved.decode(self.name())?)
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.apply_saved(saved.decode(self.name())?)
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
    fn name(&self) -> &'static str {
        "exr2-lz"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(o) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if ORDER.contains(&o) && o != 0x80 {
            cx.fail("MOVRSZ configuration getter is unsupported");
            return 0;
        }
        if o == 0x4c && self.state.resize.is_some() {
            cx.fail("MOVRSZ output-count semantics are unsupported");
            return 0;
        }
        self.r(o) as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(o) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if let Err(e) = self.write_word(cx, o, value as u32) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state
            .reset_due
            .into_iter()
            .chain(self.state.pending.as_ref().map(|p| p.due))
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.complete(cx) {
            cx.fail(e);
        }
    }
    fn save(&self) -> Value {
        serde_json::json!({"base":self.base,"options":self.options,"controllers":self.controllers.iter().map(|(c,_)|*c).collect::<Vec<_>>(),"state":self.state,"source":&self.source[..self.state.pending.as_ref().map_or(0,|p|p.source_len)]})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let saved: Saved = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.apply_saved(saved)
    }
}

impl Decompressor {
    fn validate_saved(&self, saved: &Saved) -> Result<(), String> {
        self.validate_state(&saved.state)?;
        if saved.base != self.base
            || saved.options != self.options
            || !saved
                .controllers
                .iter()
                .copied()
                .eq(self.controllers.iter().map(|(c, _)| *c))
            || saved.source.len() > SOURCE_BYTES
            || saved.source.len() != saved.state.pending.as_ref().map_or(0, |p| p.source_len)
        {
            return Err("invalid decompressor checkpoint bounds/state".into());
        }
        Ok(())
    }
    fn apply_saved(&mut self, saved: Saved) -> Result<(), String> {
        self.validate_saved(&saved)?;
        self.source[..saved.source.len()].copy_from_slice(&saved.source);
        self.state = saved.state;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
