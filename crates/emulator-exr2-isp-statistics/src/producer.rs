//! FFF18000 statistics DMA and FFF28000 parameter/LUT bank. External banks or
//! selected gradient arithmetic consume the connected packed-luma raster; DMA
//! uses an explicit transfer clock. No configuration hashes or finite lifetimes.
use fujimod_emulator_imaging::exr2_isp::{dma, due};
use fujimod_emulator_devices::exr2::irq_routes::{IrqRoutes, Irq};
use fujimod_emulator_imaging::scene::{ImageFrames, StatisticsFrames, StatisticsSpan};
use fujimod_emulator_image_formation::radiometric::FrozenReadout;
use fujimod_emulator_imaging::analysis::ImageLayout;
use fujimod_emulator_devices::work::Job;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use fujimod_emulator_imaging::exr2_isp::options::producer::Options;

const BASE: u64 = 0xfff18000;
const GLOBAL: u64 = 0xfff28000;
const IRQ: u32 = 0x2e;
pub struct Wiring {
    pub grid: Option<fujimod_emulator_imaging::analysis::GridHandle>,
    pub routes: IrqRoutes,
    pub frames: StatisticsFrames,
    pub images: ImageFrames,
}
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum AckStep {
    Clear,
    Request,
    ReadClear,
    ReadFlag,
}
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Transfer {
    due: u64,
    route: Irq,
    destinations: [u32; 10],
    histogram: bool,
    computed_histogram: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct RasterBanks {
    contrast: Option<(Vec<u8>, Vec<u8>)>,
    metering: Option<Vec<u8>>,
}
enum RasterCompletion {
    Running(Job<Result<RasterBanks, String>>),
    Ready(Result<RasterBanks, String>),
}
impl RasterCompletion {
    fn snapshot(&self) -> Result<RasterBanks, String> {
        match self {
            Self::Running(job) => job.get().clone(),
            Self::Ready(result) => result.clone(),
        }
    }
    fn take(self) -> Result<RasterBanks, String> {
        match self {
            Self::Running(job) => job.into_value(),
            Self::Ready(result) => result,
        }
    }
}
enum RasterInput {
    Sensor(FrozenReadout),
    Packed(Arc<[u8]>),
}
struct RasterJobInput {
    source: RasterInput,
    layout: ImageLayout,
    registers: [u32; 1024],
    contrast: Option<fujimod_emulator_imaging::exr2_isp::contrast::Arithmetic>,
    metering: bool,
}
impl RasterJobInput {
    fn compute(self) -> Result<RasterBanks, String> {
        let pixels = match self.source {
            RasterInput::Sensor(readout) => readout.render(0)?,
            RasterInput::Packed(pixels) => pixels,
        };
        let contrast = self
            .contrast
            .as_ref()
            .map(|arithmetic| {
                fujimod_emulator_imaging::exr2_isp::contrast::banks(
                    &self.registers,
                    &pixels,
                    self.layout,
                    arithmetic,
                )
            })
            .transpose()?;
        let metering = self
            .metering
            .then(|| crate::metering::banks(&self.registers, &pixels, self.layout))
            .transpose()?;
        Ok(RasterBanks { contrast, metering })
    }
}
enum ImagePixelsSnapshot {
    Sensor {
        image: Arc<fujimod_emulator_image_formation::radiometric::Image>,
        readout: FrozenReadout,
    },
    Packed(Arc<[u8]>),
}
impl ImagePixelsSnapshot {
    fn histogram_image(
        &self,
    ) -> Option<&Arc<fujimod_emulator_image_formation::radiometric::Image>> {
        match self {
            Self::Sensor { image, .. } => Some(image),
            Self::Packed(_) => None,
        }
    }
    fn raster_input(&self) -> RasterInput {
        match self {
            Self::Sensor { readout, .. } => RasterInput::Sensor(readout.clone()),
            Self::Packed(pixels) => RasterInput::Packed(pixels.clone()),
        }
    }
}
struct ImageSnapshot {
    edge: Option<fujimod_emulator_imaging::scene::FrameEdge>,
    layout: ImageLayout,
    pixels: ImagePixelsSnapshot,
    radiance: Option<fujimod_emulator_image_formation::exposure::ExposedFrame>,
}

fn reuse_spans(target: &mut Vec<StatisticsSpan>, source: &[StatisticsSpan]) {
    target.resize_with(source.len(), || StatisticsSpan {
        destination: 0,
        offset: 0,
        bytes: Vec::new(),
    });
    target.truncate(source.len());
    for (target, source) in target.iter_mut().zip(source) {
        target.destination = source.destination;
        target.offset = source.offset;
        target.bytes.resize(source.bytes.len(), 0);
        target.bytes.copy_from_slice(&source.bytes);
    }
}

pub struct Block18000 {
    options: Options,
    wiring: Wiring,
    registers: Box<[u32; 1024]>,
    global: Box<[u32; 3328]>,
    stop_due: Option<u64>,
    ack_due: [Option<u64>; 2],
    ack_step: AckStep,
    ack_channel: Option<usize>,
    pending: Option<Transfer>,
    raster_completion: Option<RasterCompletion>,
    seen_frame: u64,
    input_spans: Vec<StatisticsSpan>,
    spans: Vec<StatisticsSpan>,
    histogram_bytes: [u8; 1024],
    pixel_cache: crate::pixel_statistics::Cache,
}
impl Block18000 {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if options.clock.delay(1).is_err() || options.ack_ticks == 0 || options.stop_ticks == 0 {
            return Err("statistics clocks must be positive".into());
        }
        if options.pixels_per_tick == Some(0) {
            return Err("statistics pixel clock must be positive".into());
        }
        if let Some(arithmetic) = &options.contrast {
            arithmetic.validate()?;
        }
        let input_spans = wiring.frames.0.lock().unwrap().spans.clone();
        let mut spans = input_spans.clone();
        if !options.external_statistics && spans.iter().any(|s| s.destination == 9) {
            return Err("histogram bins are computed from the connected image raster".into());
        }
        if options.contrast.is_some() && !options.external_statistics {
            if spans.iter().any(|s| s.destination < 2) {
                return Err(
                    "computed contrast banks cannot have external replacement bytes".into(),
                );
            }
            spans.push(StatisticsSpan {
                destination: 0,
                offset: 0,
                bytes: vec![0; 2048],
            });
            spans.push(StatisticsSpan {
                destination: 1,
                offset: 0,
                bytes: vec![0; 80],
            });
        }
        if options.metering.is_some() && !options.external_statistics {
            if spans.iter().any(|s| s.destination == 2) {
                return Err("computed metering cannot have external replacement bytes".into());
            }
            spans.push(StatisticsSpan {
                destination: 2,
                offset: 0,
                bytes: vec![0; 3072],
            });
        }
        if spans.is_empty() && !options.histogram && !options.external_statistics {
            return Err("statistics requires a declared source of bank data".into());
        }
        Ok(Self {
            options,
            wiring,
            registers: Box::new([0; 1024]),
            global: Box::new([0; 3328]),
            stop_due: None,
            ack_due: [None; 2],
            ack_step: AckStep::Clear,
            ack_channel: None,
            pending: None,
            raster_completion: None,
            seen_frame: 0,
            input_spans,
            spans,
            histogram_bytes: [0; 1024],
            pixel_cache: Default::default(),
        })
    }
    fn offset(address: u64, size: u8) -> Result<(bool, usize), String> {
        if size != 4 || address & 3 != 0 {
            return Err("statistics registers require aligned words".into());
        }
        if (BASE..BASE + 0x1000).contains(&address) {
            Ok((false, (address - BASE) as usize))
        } else if (GLOBAL..GLOBAL + 0x3400).contains(&address) {
            Ok((true, (address - GLOBAL) as usize))
        } else {
            Err("unknown statistics register".into())
        }
    }
    fn mask(global: bool, o: usize) -> Option<u32> {
        Some(if global {
            match o {
                4 | 0x10 | 0x14 | 0x40 | 0x44 | 0x50 | 0x54 | 0x64 | 0x78 | 0x7c => 0x1fff1fff,
                8 => 0x1fff0000,
                0xc => 0x00ff007f,
                0x1c | 0x60 => 1,
                0x18 => 0x3f,
                0x20..=0x3c => 0xdfffdfff,
                0x48 => 0xfffff,
                0x4c | 0x70 => 0x3fff,
                0x68 => 0x003f003f,
                0x6c | 0x80 => 0x07ff07ff,
                0x74 => 0x00070001,
                0x84 => 0x3f3f1fff,
                0x88..=0xf4 => 0x3fff3fff,
                0x100..=0x10fc => 0x9fffdfff,
                0x1100..=0x19fc => u32::MAX,
                0x1c00..=0x2ffc => 0x3fffffff,
                0x3000..=0x33fc => 0xfffff,
                _ => return None,
            }
        } else {
            match o {
                0xc => 0x333f,
                0x100 | 0x104 | 0x200 | 0x204 | 0x300 | 0x304 | 0x414 => 0x1ffff1ff,
                0x108 | 0x258 | 0x30c => 0x01ff01ff,
                0x15c => 0x1f07073f,
                0x10c..=0x158 => 0x1fff1fff,
                0x208..=0x254 => {
                    if (o - 0x208).is_multiple_of(8) {
                        0x3fff1fff
                    } else {
                        0x1fff1fff
                    }
                }
                0x160 | 0x178 | 0x184 => 0x013f0fff,
                0x16c => 0xfff,
                0x164 | 0x168 | 0x170 | 0x174 | 0x17c | 0x180 | 0x188 | 0x18c => 0x0fff0fff,
                0x190 => 0x003f3f3f,
                0x308 => 7,
                0x310 => 0x3ff,
                0x400 | 0x410 => 0x3fff1fff,
                0x404 | 0x40c | 0x604 | 0x608 => 0x1fff1fff,
                0x408 => 0x3ffff1ff,
                0x418 => 1,
                0x500..=0x520 => u32::MAX,
                0x524 => 0x3ff,
                0x600 => 3,
                _ => return None,
            }
        })
    }
    fn config_valid(global: bool, o: usize, v: u32) -> bool {
        Self::mask(global, o).is_some_and(|m| v & !m == 0)
            && match (global, o) {
                (true, 0xc) => v & 7 <= 5 && (v >> 3) & 15 <= 11,
                (true, 0x18) => v & 15 != 0,
                (false, 0x600) => v <= 2,
                (false, 0x308) => v <= 5,
                (false, 0x524) => v & 0x200 != 0 && (v & 0x100 != 0) == (v & 0xff != 0),
                _ => true,
            }
    }
    fn live(&self) -> bool {
        let r = &self.registers;
        r[1] == 0x33173317
            && r[0x884 / 4] == 1
            && r[0x8f8 / 4] == 0x10000000
            && r[0x500 / 4..0x524 / 4].iter().all(|v| *v != 0)
    }
    fn histogram(&self) -> bool {
        let r = &self.registers;
        r[1] == 0x33173317 && r[2] & 1 != 0 && r[0x984 / 4] == 1 && r[0x9f8 / 4] == 0x10000000
    }
    fn latch(&mut self, now: u64) -> Result<(), String> {
        let frame = self.wiring.frames.0.lock().unwrap();
        let Some(edge) = frame.edge else {
            return Ok(());
        };
        if edge.index == self.seen_frame || edge.tick > now {
            return Ok(());
        }
        self.seen_frame = edge.index;
        if self.pending.is_some()
            || self.ack_channel.is_some()
            || self.ack_due.iter().any(Option::is_some)
        {
            return Ok(());
        }
        let live = self.live();
        let histogram = self.histogram();
        let computed_histogram = histogram && !self.options.external_statistics;
        if !live && !histogram {
            return Ok(());
        }
        let Ok(route) = self.wiring.routes.latch(IRQ) else {
            return Ok(());
        };
        reuse_spans(&mut self.input_spans, &frame.spans);
        drop(frame);
        let image = {
            let image = self.wiring.images.0.lock().unwrap();
            let pixels = match &image.pixels {
                fujimod_emulator_imaging::scene::frames::Pixels::Sensor(physical) => {
                    let physical =
                        fujimod_emulator_image_formation::radiometric::Image::of(physical)?;
                    ImagePixelsSnapshot::Sensor {
                        readout: physical.freeze(image.layout)?,
                        image: physical,
                    }
                }
                fujimod_emulator_imaging::scene::frames::Pixels::Packed(pixels) => {
                    ImagePixelsSnapshot::Packed(pixels.clone())
                }
            };
            ImageSnapshot {
                edge: image.edge,
                layout: image.layout,
                pixels,
                radiance: image
                    .radiance
                    .as_deref()
                    .map(fujimod_emulator_image_formation::exposure::ExposedFrame::of)
                    .transpose()?
                    .cloned(),
            }
        };
        let grid = if image.radiance.is_some() {
            self.wiring
                .grid
                .as_ref()
                .map(|g| {
                    g.lock()
                        .map(|v| *v)
                        .map_err(|_| "statistics image grid poisoned")
                })
                .transpose()?
                .unwrap_or([image.layout.width, image.layout.height])
        } else {
            [image.layout.width, image.layout.height]
        };
        let mut destinations = [0; 10];
        destinations[..9].copy_from_slice(&self.registers[0x500 / 4..0x524 / 4]);
        destinations[9] = self.registers[0x700 / 4];
        if histogram {
            if (!(self.options.histogram || self.options.external_statistics))
                || self.registers[0x704 / 4] != 0x200
            {
                return Err("unsupported histogram count mode; 0x200 selects 256 u32 bins".into());
            }
        }
        if computed_histogram {
            if image.edge != Some(edge) {
                return Err("histogram image and statistics frame edges differ".into());
            }
            let x = self.registers[0x604 / 4];
            let y = self.registers[0x608 / 4];
            let window = [
                (x >> 16) as usize,
                (y >> 16).saturating_sub(1) as usize,
                (x as usize & 0x1fff) + 1,
                (y as usize & 0x1fff) + 1,
            ];
            match &image.pixels {
                ImagePixelsSnapshot::Sensor {
                    image: physical, ..
                } => {
                    if live && image.radiance.is_some() {
                        let mut windows =
                            crate::pixel_statistics::windows(grid, self.registers.as_slice());
                        windows.push([
                            window[0].min(grid[0]),
                            window[1].min(grid[1]),
                            (window[0] + window[2])
                                .min(grid[0])
                                .saturating_sub(window[0]),
                            (window[1] + window[3])
                                .min(grid[1])
                                .saturating_sub(window[1]),
                        ]);
                        // Both banks sample one physical input grid. Prepare their
                        // union once before either requests its own measurements.
                        physical.measurement_windows(grid, &windows)?;
                    }
                    physical.histogram(grid, window, &mut self.histogram_bytes)?
                }
                ImagePixelsSnapshot::Packed(pixels) => {
                    fujimod_emulator_image_processing::analysis::histogram_luma(
                        pixels,
                        image.layout,
                        grid,
                        window,
                        &mut self.histogram_bytes,
                    )?
                }
            }
            dma::span(destinations[9] as u64, self.histogram_bytes.len())?;
        }
        let mut radiometric_spans = None;
        if live && !self.options.external_statistics {
            if (image.radiance.is_some()
                || self.options.contrast.is_some()
                || self.options.metering.is_some())
                && image.edge != Some(edge)
            {
                return Err("image/statistics frame edges differ".into());
            }
            if let Some(exposed) = &image.radiance {
                let measured;
                let exposed = if let Some(physical) = image.pixels.histogram_image() {
                    measured = physical.measurement_windows(
                        grid,
                        &crate::pixel_statistics::windows(grid, self.registers.as_slice()),
                    )?;
                    &measured
                } else {
                    exposed
                };
                if self.options.metering.is_some() {
                    return Err("radiometric and packed-pixel metering cannot replace the same statistics bank".into());
                }
                let mut spans: Vec<_> = self
                    .input_spans
                    .iter()
                    .filter(|s| ![0, 1, 2, 3, 6].contains(&s.destination))
                    .cloned()
                    .collect();
                spans.extend_from_slice(self.pixel_cache.measure(
                    exposed,
                    grid,
                    self.registers.as_slice(),
                )?);
                radiometric_spans = Some(spans);
            }
        }
        let base = radiometric_spans.as_deref().unwrap_or(&self.input_spans);
        let contrast = self.options.contrast.is_some() && !self.options.external_statistics;
        let metering = self.options.metering.is_some() && !self.options.external_statistics;
        let computed = [(0, 2048), (1, 80), (2, 3072)];
        let selected = || {
            computed.iter().copied().filter(
                |(destination, _)| {
                    if *destination < 2 {
                        contrast
                    } else {
                        metering
                    }
                },
            )
        };
        let extras = selected().count();
        let same = self.spans.len() == base.len() + extras
            && self.spans.iter().zip(base).all(|(a, b)| {
                a.destination == b.destination
                    && a.offset == b.offset
                    && a.bytes.len() == b.bytes.len()
            })
            && self.spans[base.len()..]
                .iter()
                .zip(selected())
                .all(|(s, (destination, bytes))| {
                    s.destination == destination && s.offset == 0 && s.bytes.len() == bytes
                });
        if same {
            for (to, from) in self.spans.iter_mut().zip(base) {
                to.bytes.copy_from_slice(&from.bytes);
            }
        } else {
            self.spans = base.to_vec();
            for (destination, bytes) in selected() {
                self.spans.push(StatisticsSpan {
                    destination,
                    offset: 0,
                    bytes: vec![0; bytes],
                });
            }
        }
        let raster_completion = if contrast || metering {
            let input = RasterJobInput {
                source: image.pixels.raster_input(),
                layout: image.layout,
                registers: *self.registers,
                contrast: self.options.contrast.clone().filter(|_| contrast),
                metering,
            };
            let job = Job::submit(move || input.compute());
            if fujimod_emulator_devices::work::worker_count() == 0 {
                let result = job.into_value();
                if let Err(error) = &result {
                    return Err(error.clone());
                }
                Some(RasterCompletion::Ready(result))
            } else {
                Some(RasterCompletion::Running(job))
            }
        } else {
            None
        };
        if live {
            for (i, span) in self.spans.iter().enumerate() {
                let address = destinations[span.destination] as u64 + span.offset as u64;
                if address & 3 != 0 {
                    return Err("unaligned statistics DMA destination".into());
                }
                let bounds = dma::span(address, span.bytes.len())?;
                if computed_histogram {
                    let hist = dma::span(destinations[9] as u64, self.histogram_bytes.len())?;
                    if bounds.start < hist.end && hist.start < bounds.end {
                        return Err("statistics and histogram DMA destinations overlap".into());
                    }
                }
                for previous in &self.spans[..i] {
                    let prior = dma::span(
                        destinations[previous.destination] as u64 + previous.offset as u64,
                        previous.bytes.len(),
                    )?;
                    if bounds.start < prior.end && prior.start < bounds.end {
                        return Err("statistics DMA destinations overlap".into());
                    }
                }
            }
        }
        if !live {
            destinations[..9].fill(0);
        }
        if !histogram {
            destinations[9] = 0;
        }
        let processing_ticks = if let Some(rate) = self.options.pixels_per_tick {
            if image.edge != Some(edge) {
                return Err("statistics processing image and frame edges differ".into());
            }
            u64::try_from(
                image
                    .layout
                    .width
                    .checked_mul(image.layout.height)
                    .ok_or("statistics raster size overflow")?,
            )
            .map_err(|_| "statistics raster size exceeds clock")?
            .div_ceil(rate)
        } else {
            0
        };
        self.pending = Some(Transfer {
            due: due(
                edge.tick
                    .checked_add(processing_ticks)
                    .ok_or("statistics processing deadline overflow")?,
                self.options.clock.delay(
                    self.spans
                        .iter()
                        .filter(|s| destinations[s.destination] != 0)
                        .map(|s| s.bytes.len())
                        .sum::<usize>()
                        + if computed_histogram {
                            self.histogram_bytes.len()
                        } else {
                            0
                        },
                )?,
            )?,
            route,
            destinations,
            histogram,
            computed_histogram,
        });
        self.raster_completion = raster_completion;
        Ok(())
    }
    fn raster_snapshot(&self) -> Option<Result<RasterBanks, String>> {
        self.raster_completion
            .as_ref()
            .map(RasterCompletion::snapshot)
    }
    fn replace_raster_bank(
        spans: &mut [StatisticsSpan],
        destination: usize,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        let span = spans
            .iter_mut()
            .find(|span| span.destination == destination && span.offset == 0)
            .ok_or("missing computed statistics bank")?;
        if span.bytes.len() != bytes.len() {
            return Err("computed statistics bank extent changed".into());
        }
        span.bytes.copy_from_slice(&bytes);
        Ok(())
    }
    fn finish_raster(&mut self) -> Result<(), String> {
        let Some(completion) = self.raster_completion.take() else {
            return Ok(());
        };
        let banks = completion.take()?;
        if let Some((contrast, bands)) = banks.contrast {
            Self::replace_raster_bank(&mut self.spans, 0, contrast)?;
            Self::replace_raster_bank(&mut self.spans, 1, bands)?;
        }
        if let Some(metering) = banks.metering {
            Self::replace_raster_bank(&mut self.spans, 2, metering)?;
        }
        Ok(())
    }
    fn run(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.stop_due.is_some_and(|t| cx.now() >= t) {
            self.global[0] = 0x10000000;
            self.stop_due = None;
        }
        for ch in 0..2 {
            if self.ack_due[ch].is_some_and(|t| cx.now() >= t) {
                self.registers[(0x8f4 + ch * 0x100) / 4] = 0;
                self.ack_due[ch] = None;
            }
        }
        if let Some(p) = self.pending.filter(|p| cx.now() >= p.due) {
            if self.wiring.routes.validate(p.route).is_err()
                || p.destinations[0] != 0 && !self.live()
                || p.histogram && !self.histogram()
            {
                self.pending = None;
                self.raster_completion = None;
            } else {
                self.finish_raster()?;
                for s in &self.spans {
                    if p.destinations[s.destination] != 0 {
                        dma::write(
                            cx,
                            p.destinations[s.destination] as u64 + s.offset as u64,
                            &s.bytes,
                        )?;
                    }
                }
                if p.computed_histogram {
                    dma::write(cx, p.destinations[9] as u64, &self.histogram_bytes)?;
                }
                self.pending = None;
                self.ack_channel = Some(0);
                self.ack_step = AckStep::Clear;
                p.route.raise(cx)?;
            }
        }
        self.latch(cx.now())
    }
    fn ack_read(&mut self, o: usize) -> Result<(), String> {
        let Some(ch) = self.ack_channel else {
            return Ok(());
        };
        let clear = 0x88c + ch * 0x100;
        let flag = 0x8f4 + ch * 0x100;
        match (o, self.ack_step) {
            (x, AckStep::ReadClear) if x == clear => self.ack_step = AckStep::ReadFlag,
            (x, AckStep::ReadFlag) if x == flag => {
                if self.registers[flag / 4] == 0 {
                    self.ack_channel = if ch == 0 { Some(1) } else { None };
                    self.ack_step = AckStep::Clear;
                } else {
                    self.ack_step = AckStep::ReadClear;
                }
            }
            _ => return Err("statistics require the next paired ACK read".into()),
        }
        Ok(())
    }
    fn ack_write(&mut self, cx: &Cx<'_>, o: usize, v: u32) -> Result<(), String> {
        let ch = usize::from(o >= 0x900);
        let clear = 0x88c + ch * 0x100;
        let flag = 0x8f4 + ch * 0x100;
        if v > 1 {
            return Err("statistics ACK word must be 0 or 1".into());
        }
        if let Some(current) = self.ack_channel {
            if ch != current {
                return Err("statistics ACK channel is out of order".into());
            }
            match (o, v, self.ack_step) {
                (x, 0, AckStep::Clear) if x == clear => self.ack_step = AckStep::Request,
                (x, 1, AckStep::Request) if x == flag => self.ack_step = AckStep::ReadClear,
                _ => return Err("statistics require clear then ACK request".into()),
            }
        }
        if o == flag && v == 1 {
            if self.registers[clear / 4] != 0 {
                return Err("statistics ACK requires cleared channel flag".into());
            }
            self.ack_due[ch] = Some(due(cx.now(), self.options.ack_ticks)?);
        }
        Ok(())
    }
}
impl Device for Block18000 {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                self.registers.as_slice(),
                self.global.as_slice(),
                self.stop_due,
                self.ack_due,
                self.ack_step,
                self.ack_channel,
                self.pending,
                self.seen_frame,
                &self.spans,
                self.histogram_bytes.as_slice(),
                self.wiring.grid.is_some(),
                self.raster_snapshot(),
            ),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let s: (
            Vec<u32>,
            Vec<u32>,
            Option<u64>,
            [Option<u64>; 2],
            AckStep,
            Option<usize>,
            Option<Transfer>,
            u64,
            Vec<StatisticsSpan>,
            Vec<u8>,
            bool,
            Option<Result<RasterBanks, String>>,
        ) = saved.decode(self.name())?;
        if s.10 != self.wiring.grid.is_some()
            || s.0.len() != 1024
            || s.1.len() != 3328
            || s.9.len() != 1024
            || s.11.is_some() && s.6.is_none()
        {
            return Err("statistics snapshot bank length differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (
            registers,
            global,
            stop_due,
            ack_due,
            ack_step,
            ack_channel,
            pending,
            seen_frame,
            spans,
            histogram,
            _,
            raster,
        ): (
            Vec<u32>,
            Vec<u32>,
            Option<u64>,
            [Option<u64>; 2],
            AckStep,
            Option<usize>,
            Option<Transfer>,
            u64,
            Vec<StatisticsSpan>,
            Vec<u8>,
            bool,
            Option<Result<RasterBanks, String>>,
        ) = saved.decode(self.name())?;
        self.registers.copy_from_slice(&registers);
        self.global.copy_from_slice(&global);
        self.stop_due = stop_due;
        self.ack_due = ack_due;
        self.ack_step = ack_step;
        self.ack_channel = ack_channel;
        self.pending = pending;
        self.raster_completion = raster.map(RasterCompletion::Ready);
        self.seen_frame = seen_frame;
        self.spans = spans;
        self.histogram_bytes.copy_from_slice(&histogram);
        self.pixel_cache.clear();
        Ok(())
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"options":self.options,"registers":self.registers.as_slice(),"global":self.global.as_slice(),
            "stop_due":self.stop_due,"ack_due":self.ack_due,"ack_step":self.ack_step,"ack_channel":self.ack_channel,
            "pending":self.pending,"seen_frame":self.seen_frame,"spans":self.spans,
            "raster_result":self.raster_snapshot(),"statistics_grid":self.wiring.grid.is_some(),
            "histogram":fujimod_emulator_devices::fingerprint::bytes(&self.histogram_bytes)}),
        )
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"registers":self.registers.as_slice(),"global":self.global.as_slice(),
            "seen_frame":self.seen_frame,"stop_due":self.stop_due,"pending_due":self.pending.map(|p|p.due),
            "ack_channel":self.ack_channel,"ack_due":self.ack_due,
            "computed_source":self.pixel_cache.source(),
            "histogram_destination":self.pending.filter(|p|p.histogram).map(|p|p.destinations[9])})
    }
    fn name(&self) -> &'static str {
        "isp-block18000"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let result: Result<u64, String> = (|| {
            let (g, o) = Self::offset(address, size)?;
            if g {
                let write_only = (0x1100..0x1a00).contains(&o) || (0x1c00..0x3400).contains(&o);
                if o == 0
                    || Self::mask(g, o).is_some() && !write_only
                    || self.options.diagnostic_reads
                {
                    return Ok(self.global[o / 4] as u64);
                }
            } else {
                if matches!(o, 0x88c | 0x8f4 | 0x98c | 0x9f4) {
                    self.ack_read(o)?;
                }
                if matches!(
                    o,
                    0 | 4
                        | 8
                        | 0x880
                        | 0x884
                        | 0x88c
                        | 0x890
                        | 0x8a0
                        | 0x8c4
                        | 0x8f4
                        | 0x8f8
                        | 0x700
                        | 0x704
                        | 0x980
                        | 0x984
                        | 0x98c
                        | 0x990
                        | 0x9a0
                        | 0x9c4
                        | 0x9f4
                        | 0x9f8
                ) || Self::mask(g, o).is_some()
                {
                    return Ok(self.registers[o / 4] as u64);
                }
            }
            Err("unknown statistics register read".into())
        })();
        match result {
            Ok(v) => v,
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let result: Result<(), String> = (|| {
            let (g, o) = Self::offset(address, size)?;
            let v = u32::try_from(value).map_err(|_| "statistics word exceeds 32 bits")?;
            if g && o == 0 {
                if v == 0 {
                    self.stop_due = Some(due(cx.now(), self.options.stop_ticks)?);
                    self.pending = None;
                    self.raster_completion = None;
                } else if v & !0x1007333f != 0
                    || [0, 2, 4, 8].into_iter().any(|shift| (v >> shift) & 3 > 2)
                {
                    return Err("unsupported statistics global mode".into());
                }
            } else if Self::mask(g, o).is_some() {
                if !Self::config_valid(g, o, v) {
                    return Err("unsupported statistics configuration bits or domain".into());
                }
            } else if g {
                return Err("unknown statistics global register".into());
            } else if matches!(o, 0x88c | 0x8f4 | 0x98c | 0x9f4) {
                self.ack_write(cx, o, v)?;
            } else {
                let valid = match o {
                    0 | 0x880 | 0x980 => v == 0,
                    4 => matches!(v, 0 | 0x33173317 | 0x33170000),
                    8 => matches!(v, 0 | 1 | 0x10000 | 0x10001),
                    0x884 | 0x984 => v <= 1,
                    0x8a0 => v == 0x80003000,
                    0x9a0 => matches!(v, 0x80003000 | 0x80403000),
                    0x8c4 | 0x9c4 => v == 2,
                    0x8f8 | 0x9f8 => matches!(v, 0 | 0x10000000),
                    0x704 => {
                        (self.options.histogram || self.options.external_statistics) && v == 0x200
                    }
                    0x700 => v & 3 == 0 && dma::physical(v as u64, 1024).is_ok(),
                    _ => false,
                };
                if !valid {
                    return Err("unsupported statistics control word".into());
                }
            }
            if g {
                self.global[o / 4] = v;
            } else {
                self.registers[o / 4] = v;
            }
            Ok(())
        })();
        if let Err(e) = result {
            cx.fail(format!("{e}: write {address:#010x} = {value:#010x}"));
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let input = self
            .wiring
            .frames
            .0
            .lock()
            .unwrap()
            .edge
            .filter(|e| e.index != self.seen_frame)
            .map(|e| e.tick.max(now));
        [
            self.stop_due,
            self.pending.map(|p| p.due),
            self.ack_due[0],
            self.ack_due[1],
            input,
        ]
        .into_iter()
        .flatten()
        .min()
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
        Self::offset(address, size)
            .is_ok_and(|(g, o)| g && o == 0 || !g && matches!(o, 0x890 | 0x990))
    }
}
