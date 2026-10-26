//! FFF12000/FFF13000 output banks. Packed Y0,Y1,Cb,Cr, nearest-pair
//! resampling, and completion latency are declared input laws; silicon
//! filtering is unresolved. Each instance owns one bank.
use fujimod_emulator_imaging::exr2_isp::memory_outputs::MemoryOutputHandle;
use fujimod_emulator_imaging::exr2_isp::options::output::{GeometryPolicy, Options};
use fujimod_emulator_imaging::exr2_isp::output_channels::{FrameTransfer, OutputChannels, Source};
use fujimod_emulator_imaging::exr2_isp::{dma::{self, Raster, Transfer}, due};
use fujimod_emulator_devices::exr2::irq_routes::IrqRoutes;
use fujimod_emulator_image_processing::analysis::nearest_pairs;
use fujimod_emulator_imaging::analysis::ImageLayout;
use fujimod_emulator_imaging::scene::ImageFrames;
use fujimod_emulator_devices::{Cx, Device};

#[cfg(test)]
#[path = "tests/output_work.rs"]
mod work_tests;

pub struct Wiring {
    pub base: u64,
    pub irq: u32,
    pub channel: usize,
    pub channels: OutputChannels,
    pub routes: IrqRoutes,
    pub frames: ImageFrames,
    pub memory_outputs: Option<MemoryOutputHandle>,
}
pub struct OutputChannel {
    options: Options,
    wiring: Wiring,
    registers: [u32; 64],
    stop_due: Option<u64>,
    seen_frame: u64,
}
impl OutputChannel {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if options.stop_ticks == 0
            || options.clock.delay(1).is_err()
            || wiring.irq >= 128
            || wiring.base & 0xfff != 0
            || wiring.channel > 1
        {
            return Err("invalid output bank wiring or clock".into());
        }
        {
            let mut channels = wiring.channels.0.lock().unwrap();
            if channels.clock.is_some_and(|clock| clock != options.clock) {
                return Err("paired output banks need one declared data clock".into());
            }
            channels.clock = Some(options.clock);
            channels.irqs[wiring.channel] = Some(wiring.irq);
        }
        Ok(Self {
            options,
            wiring,
            registers: [0; 64],
            stop_due: None,
            seen_frame: 0,
        })
    }
    fn offset(&self, address: u64, size: u8) -> Result<usize, String> {
        let o = address.wrapping_sub(self.wiring.base);
        if size != 4 || o & 3 != 0 || o >= 0x100 {
            return Err("output bank requires an aligned word".into());
        }
        Ok(o as usize)
    }
    fn mask(o: usize) -> Option<u32> {
        Some(match o {
            0x04 => 0x0fff0fff,
            0x08 => 0x8000ffff,
            0x0c => 0xffff,
            0x10 | 0x14 | 0x18 => 0x1fff1fff,
            0x1c => 0x7fff7fff,
            0x20 => 0x003f000f,
            0x30..=0xa8 => 0x1ffff,
            0xb0 | 0xb4 => 0x07ff07ff,
            0xb8 => 0x01ff0037,
            0xc0 => 0x3ffffff0,
            0xc4 => 0x1ff8,
            0xf4 => 0xf,
            _ => return None,
        })
    }
    fn valid(o: usize, v: u32) -> bool {
        Self::mask(o).is_some_and(|mask| v & !mask == 0)
            && match o {
                0xb0 => v & 7 == 7,
                0xb4 => v & 7 == 7 && (v >> 16) & 7 == 0,
                0xb8 => v & 7 <= 4,
                0x20 => v & 15 <= 8 && (v >> 16) & 63 <= 32,
                _ => true,
            }
    }
    fn layout(&self) -> Result<ImageLayout, String> {
        let r = &self.registers;
        if !Self::valid(0xb0, r[0xb0 / 4]) || !Self::valid(0xb4, r[0xb4 / 4]) {
            return Err("output dimensions are incomplete".into());
        }
        let layout = ImageLayout {
            width: (r[0xb0 / 4] as usize & 0x7ff) + 1,
            height: ((r[0xb0 / 4] >> 16) as usize & 0x7ff) + 1,
            stride: 2 * r[0xc4 / 4] as usize,
        };
        dma::physical(r[0xc0 / 4] as u64, layout.bytes()?)?;
        Ok(layout)
    }
    fn resize_buffer(&mut self) {
        if let Ok(layout) = self.layout() {
            let mut channels = self.wiring.channels.0.lock().unwrap();
            if channels.transfers[self.wiring.channel].is_none_or(|p| p.completed) {
                channels.dma[self.wiring.channel]
                    .pixels
                    .resize(layout.bytes().unwrap(), 0);
            }
        }
    }
    fn publish_readback(&self) -> Result<(), String> {
        let Some(wire) = &self.wiring.memory_outputs else {
            return Ok(());
        };
        let busy = self.wiring.channels.0.lock().unwrap().transfers[self.wiring.channel].is_some();
        let raster = if self.registers[0] == 0x10001 && !busy {
            self.layout().ok().map(|layout| Raster {
                address: self.registers[0xc0 / 4] as u64,
                width: layout.width,
                height: layout.height,
                pitch: layout.stride,
            })
        } else {
            None
        };
        wire.lock().unwrap().configure(self.wiring.channel, raster)
    }
    fn readback(&mut self, cx: &mut Cx<'_>) -> Result<bool, String> {
        let Some(wire) = &self.wiring.memory_outputs else {
            return Ok(false);
        };
        let mut outputs = wire.lock().unwrap();
        let mut channels = self.wiring.channels.0.lock().unwrap();
        if !outputs.active {
            channels.readback_routes = None;
            channels.readback_deadline = None;
            return Ok(false);
        }
        // The memory input owns this exposure. Sensor edges during it cannot
        // be replayed later as a fresh preview frame.
        if let Some(edge) = self
            .wiring
            .frames
            .0
            .lock()
            .unwrap()
            .edge
            .filter(|e| e.tick <= cx.now())
        {
            self.seen_frame = edge.index;
        }
        if outputs.completed() {
            return Ok(true);
        }
        let deadline = outputs
            .transfers
            .iter()
            .filter_map(Transfer::deadline)
            .min();
        if channels.readback_deadline != deadline {
            channels.readback_routes = None;
            channels.readback_deadline = deadline;
        }
        if channels.readback_routes.is_none() {
            let mut routes = [None; 2];
            for i in 0..2 {
                if outputs.active_destinations[i].is_some() {
                    if channels.transfers[i].is_some() || !channels.armed[i] {
                        return Err(
                            "RAW readback requires idle armed participating output banks".into(),
                        );
                    }
                    routes[i] = Some(
                        self.wiring
                            .routes
                            .latch(channels.irqs[i].ok_or("missing output IRQ")?)?,
                    );
                }
            }
            channels.readback_routes = Some(routes);
        }
        if outputs.transfers.iter().any(|t| t.ready(cx.now())) {
            if outputs
                .transfers
                .iter()
                .any(|t| t.deadline().is_some_and(|at| at > cx.now()))
            {
                return Err("RAW readback output clocks differ".into());
            }
            let routes = channels.readback_routes.unwrap();
            for route in routes.into_iter().flatten() {
                self.wiring.routes.validate(route)?;
            }
            let targets = outputs.active_destinations;
            for transfer in &mut outputs.transfers {
                transfer.write(cx)?;
            }
            for i in 0..2 {
                let Some(raster) = targets[i] else {
                    continue;
                };
                let span = dma::span(raster.address, raster.pitch * raster.height)?;
                channels.transfers[i] = Some(FrameTransfer {
                    source: Source::Memory,
                    span: (span.start, span.end),
                    completed: true,
                    route: routes[i].ok_or("RAW readback output lost its IRQ")?,
                });
                outputs.destinations[i] = None;
            }
            for route in routes.into_iter().flatten() {
                route.raise(cx)?;
            }
        }
        Ok(true)
    }
    fn latch(&mut self, now: u64) -> Result<(), String> {
        let frames = self.wiring.frames.0.lock().unwrap();
        let Some(edge) = frames.edge else {
            return Ok(());
        };
        if edge.index == self.seen_frame || edge.tick > now {
            return Ok(());
        }
        self.seen_frame = edge.index;
        // RAW sensor capture and preview output are concurrent. Only an
        // active memory-readback transfer owns these banks exclusively; run()
        // handles that ownership before consuming a sensor frame here.
        if self.registers[0] != 0x10001 {
            return Ok(());
        }
        let layout = match self.layout() {
            Ok(v) => v,
            Err(e) => {
                return match self.options.geometry {
                    GeometryPolicy::SkipIncomplete => Ok(()),
                    GeometryPolicy::RefuseIncomplete => Err(e),
                };
            }
        };
        let Ok(route) = self.wiring.routes.latch(self.wiring.irq) else {
            return Ok(());
        };
        let bounds = dma::span(self.registers[0xc0 / 4] as u64, layout.bytes()?)?;
        let (lo, hi) = (bounds.start, bounds.end);
        let mut channels = self.wiring.channels.0.lock().unwrap();
        if channels.transfers[self.wiring.channel].is_some()
            || channels
                .transfers
                .iter()
                .flatten()
                .any(|p| p.source != Source::Frame(edge.index))
        {
            return Ok(());
        }
        if channels.transfers[1 - self.wiring.channel]
            .is_some_and(|p| lo < p.span.1 && p.span.0 < hi)
        {
            return Err("armed output-bank DMA destinations overlap".into());
        }
        let transfer = &mut channels.dma[self.wiring.channel];
        let raster = Raster {
            address: self.registers[0xc0 / 4] as u64,
            width: layout.width,
            height: layout.height,
            pitch: layout.stride,
        };
        transfer.prepare_deferred(
            edge.tick,
            self.options.clock.delay(layout.bytes()?)?,
            raster,
            layout.stride,
        )?;
        let result = match &frames.pixels {
            fujimod_emulator_imaging::scene::frames::Pixels::Sensor(image) => (|| {
                let frozen =
                    fujimod_emulator_image_formation::radiometric::Image::of(image)?.freeze(layout)?;
                transfer.pixels.defer_fallible(move |mut staging| {
                    staging.resize(layout.stride * layout.height, 0);
                    let result = frozen
                        .render(0)
                        .map(|pixels| staging.copy_from_slice(&pixels));
                    (staging, result)
                });
                // Exact execution reports eager validation/computation errors
                // at the same access as before; workers report at completion.
                if fujimod_emulator_devices::work::worker_count() == 0 {
                    transfer.check_result()?;
                }
                Ok(())
            })(),
            fujimod_emulator_imaging::scene::frames::Pixels::Packed(pixels) => (|| {
                let input = frames.layout;
                if pixels.len() != input.bytes()? {
                    return Err("packed image extent differs from its raster".into());
                }
                let pixels = pixels.clone();
                transfer.pixels.defer_fallible(move |mut staging| {
                    staging.resize(layout.stride * layout.height, 0);
                    let result = nearest_pairs(&pixels, input, &mut staging, layout);
                    (staging, result)
                });
                if fujimod_emulator_devices::work::worker_count() == 0 {
                    transfer.check_result()?;
                }
                Ok(())
            })(),
        };
        if let Err(e) = result {
            transfer.cancel();
            return Err(e);
        }
        channels.transfers[self.wiring.channel] = Some(FrameTransfer {
            source: Source::Frame(edge.index),
            span: (lo, hi),
            completed: false,
            route,
        });
        drop(channels);
        drop(frames);
        self.publish_readback()?;
        Ok(())
    }
    fn run(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.stop_due.is_some_and(|t| cx.now() >= t) {
            self.registers[0] = 1;
            self.stop_due = None;
            let mut channels = self.wiring.channels.0.lock().unwrap();
            channels.armed[self.wiring.channel] = false;
            channels.transfers[self.wiring.channel] = None;
        }
        {
            let mut channels = self.wiring.channels.0.lock().unwrap();
            if channels.dma.iter().any(|t| t.ready(cx.now()))
                && channels
                    .dma
                    .iter()
                    .all(|t| t.deadline().is_none_or(|at| at <= cx.now()))
            {
                if channels
                    .transfers
                    .iter()
                    .flatten()
                    .any(|p| !p.completed && self.wiring.routes.validate(p.route).is_err())
                {
                    for i in 0..2 {
                        if channels.transfers[i].is_some_and(|p| !p.completed) {
                            channels.transfers[i] = None;
                            channels.dma[i].cancel();
                        }
                    }
                } else {
                    // A failed bank must not allow its peer to publish first.
                    for transfer in &channels.dma {
                        transfer.check_result()?;
                    }
                    // All participating payloads reach RAM before either IRQ.
                    for transfer in &mut channels.dma {
                        transfer.write(cx)?;
                    }
                    let mut routes = [None; 2];
                    for (i, transfer) in channels.transfers.iter_mut().enumerate() {
                        if let Some(p) = transfer.as_mut().filter(|p| !p.completed) {
                            p.completed = true;
                            routes[i] = Some(p.route);
                        }
                    }
                    for route in routes.into_iter().flatten() {
                        route.raise(cx)?;
                    }
                }
            }
        }
        // A memory readback can have no participating output banks while a
        // previously latched sensor frame is still draining. Complete that
        // DMA before the memory input suppresses new sensor-frame latching.
        if self.readback(cx)? {
            return Ok(());
        }
        self.latch(cx.now())
    }
}
impl Device for OutputChannel {
    fujimod_emulator_devices::checkpoint_fields!(#[serde(with="fujimod_emulator_devices::checkpoint::array")] registers: [u32;64], stop_due: Option<u64>, seen_frame: u64);
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"registers":self.registers.as_slice(),"stop_due":self.stop_due,"seen_frame":self.seen_frame,
            "channels":self.wiring.channels.fingerprint()}),
        )
    }
    fn save(&self) -> serde_json::Value {
        let channels = self.wiring.channels.0.lock().unwrap();
        let transfer = channels.transfers[self.wiring.channel];
        serde_json::json!({"registers":self.registers.as_slice(),"seen_frame":self.seen_frame,
        "stop_due":self.stop_due,"dma_due":channels.dma[self.wiring.channel].deadline(),
        "completed":transfer.map(|t|t.completed),"span":transfer.map(|t|t.span),
        "source_frame":transfer.and_then(|t| match t.source {
            Source::Frame(index) => Some(index), Source::Memory => None,
        })})
    }
    fn name(&self) -> &'static str {
        "image-output-channel"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let o = match self.offset(address, size) {
            Ok(v) => v,
            Err(e) => {
                cx.fail(e);
                return 0;
            }
        };
        if o == 0
            || Self::mask(o).is_some()
            || self.options.diagnostic_reads
                && matches!(o, 0xd0 | 0xd4 | 0xd8 | 0xdc | 0xe0 | 0xe4 | 0xf0)
        {
            self.registers[o / 4] as u64
        } else {
            cx.unmodeled();
            0
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let result: Result<(), String> = (|| {
            let o = self.offset(address, size)?;
            let v = u32::try_from(value).map_err(|_| "output word exceeds 32 bits")?;
            if o == 0
                && v != 0
                && v != self.registers[o / 4]
                && self.wiring.memory_outputs.as_ref().is_some_and(|w| {
                    let output = w.lock().unwrap();
                    output.active && !output.completed()
                })
            {
                return Err("output configuration changed during RAW readback".into());
            }
            match o {
                0 => match v {
                    0 => {
                        // A stop withdraws future destinations immediately, but
                        // the memory input owns the latched RAW transfer until
                        // both payloads reach RAM. Acknowledge only after that
                        // drain; otherwise the caller could reuse live DMA RAM.
                        let drain = self
                            .wiring
                            .memory_outputs
                            .as_ref()
                            .and_then(|wire| {
                                let outputs = wire.lock().unwrap();
                                outputs
                                    .active
                                    .then(|| {
                                        outputs
                                            .transfers
                                            .iter()
                                            .filter_map(Transfer::deadline)
                                            .max()
                                    })
                                    .flatten()
                            })
                            .unwrap_or(cx.now())
                            .max(cx.now());
                        self.stop_due = Some(due(drain, self.options.stop_ticks)?);
                        let mut channels = self.wiring.channels.0.lock().unwrap();
                        for i in 0..2 {
                            if channels.transfers[i].is_some_and(|p| !p.completed) {
                                channels.transfers[i] = None;
                                channels.dma[i].cancel();
                            }
                        }
                        channels.transfers[self.wiring.channel] = None;
                    }
                    0x10001 => {
                        self.stop_due = None;
                        self.resize_buffer();
                    }
                    _ => return Err("output control requires stop or arm".into()),
                },
                0xd4 => match v {
                    1 => {
                        let mut channels = self.wiring.channels.0.lock().unwrap();
                        if channels.transfers[self.wiring.channel].is_some_and(|p| p.completed) {
                            channels.transfers[self.wiring.channel] = None;
                        }
                    }
                    0x33ff1 => {}
                    _ => return Err("unknown output acknowledgment".into()),
                },
                0xd8 if v == 1 => {}
                _ if Self::valid(o, v) => {}
                _ => return Err("unknown output register or unsupported bits".into()),
            }
            self.registers[o / 4] = v;
            if o == 0 && v == 0x10001 {
                self.wiring.channels.0.lock().unwrap().armed[self.wiring.channel] = true;
            }
            // Config changes during DMA affect the next frame, never its bytes.
            if matches!(o, 0xb0 | 0xb4 | 0xc0 | 0xc4 | 0xd4) {
                self.resize_buffer();
                self.publish_readback()?;
            }
            if o == 0 {
                self.publish_readback()?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let edge = self.wiring.frames.0.lock().unwrap().edge;
        let input = edge
            .filter(|e| e.index != self.seen_frame)
            .map(|e| e.tick.max(now));
        let readback = self.wiring.memory_outputs.as_ref().and_then(|w| {
            let outputs = w.lock().unwrap();
            if !outputs.active || outputs.completed() {
                return None;
            }
            let channels = self.wiring.channels.0.lock().unwrap();
            if channels.readback_routes.is_none()
                || channels.readback_deadline
                    != outputs
                        .transfers
                        .iter()
                        .filter_map(Transfer::deadline)
                        .min()
            {
                Some(now)
            } else {
                outputs.transfers[self.wiring.channel].deadline()
            }
        });
        [
            self.stop_due,
            {
                // A frame publishes both payloads before either completion.
                // Unequal rasters finish at the later DMA deadline.
                let channels = self.wiring.channels.0.lock().unwrap();
                if channels.dma[self.wiring.channel].deadline().is_some() {
                    channels.dma.iter().filter_map(Transfer::deadline).max()
                } else {
                    None
                }
            },
            input,
            readback,
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
        self.offset(address, size)
            .is_ok_and(|o| o == 0 || Self::mask(o).is_some())
    }
}
