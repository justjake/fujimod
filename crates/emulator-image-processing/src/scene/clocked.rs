//! Explicit external post-ISP frame clock. Pixels are supplied by the host or
//! a declared moving integer pattern; statistics retain their selected input.
//! No guest memory, software mode or instruction address drives the source.
use fujimod_emulator_devices::capability::StatisticsCadence;
use fujimod_emulator_devices::parts::pins::{InputPins, Selection};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::scene::clocked::Options;
use fujimod_emulator_image_formation::{exposure::ExposedFrame, radiometric::Image};
use fujimod_emulator_imaging::scene::frames::{ImageSource, Pixels};
use fujimod_emulator_imaging::scene::{
    FrameCadence, FrameEdge, FramePhases, ImageFrames, SensorFramesHandle, StatisticsFrames,
};
use std::sync::Arc;

pub struct ClockedFrames {
    clock: FrameCadence,
    phase: Option<(FramePhases, usize)>,
    images: ImageFrames,
    statistics: StatisticsFrames,
    frames: u64,
    cadence: StatisticsCadence,
    activity: Option<(Box<dyn InputPins>, Selection)>,
    optics: Option<super::optics::Optics>,
    renderer: Option<fujimod_emulator_image_formation::radiometric::Renderer>,
    frame_period: Option<fujimod_emulator_imaging::timing::FramePeriod>,
    external_period: u64,
    sensor_clock: Option<SensorFramesHandle>,
    seen_sensor_frame: Option<FrameEdge>,
    seen_external_frame: Option<FrameEdge>,
    last_physical: bool,
}
pub struct Wiring {
    pub sensor_clock: Option<SensorFramesHandle>,
    pub frame_period: Option<fujimod_emulator_imaging::timing::FramePeriod>,
    pub renderer: Option<fujimod_emulator_image_formation::radiometric::Renderer>,
    pub images: ImageFrames,
    pub statistics: StatisticsFrames,
    pub activity: Option<(Box<dyn InputPins>, Selection)>,
    pub start_tick: u64,
    pub position: Option<fujimod_emulator_devices::parts::optics::Position>,
    pub phase_edges: Option<FramePhases>,
}
impl ClockedFrames {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        let phase = match (options.phase_source, wiring.phase_edges) {
            (Some(index @ 0..=1), Some(edges)) => Some((edges, index)),
            (None, None) => None,
            _ => {
                return Err(
                    "external frames require a valid phase index and matching edge wire".into(),
                );
            }
        };
        let optics = match (options.optics, wiring.position) {
            (Some(options), Some(position)) => Some(super::optics::Optics::new(options, position)?),
            (None, None) => None,
            _ => return Err("optical frames require both a declared law and focus wire".into()),
        };
        if wiring.sensor_clock.is_some() && wiring.renderer.is_none() {
            return Err("physical frame clock requires its sensor renderer".into());
        }
        if (phase.is_some() && wiring.sensor_clock.is_some())
            || (optics.is_some() && wiring.renderer.is_some())
        {
            return Err(
                "external image source has competing physical clock or optical transfer laws"
                    .into(),
            );
        }
        if wiring.renderer.is_some() {
            wiring.images.use_sensor();
        }
        if let Some((pins, selection)) = &wiring.activity {
            pins.level(selection.pin)?;
        }
        let first = wiring
            .start_tick
            .checked_add(options.period_ticks)
            .ok_or("external frame deadline overflow")?;
        let last_physical = wiring.sensor_clock.is_some();
        Ok(Self {
            clock: FrameCadence::new(options.period_ticks, first)?,
            phase,
            images: wiring.images,
            statistics: wiring.statistics,
            frames: 0,
            cadence: StatisticsCadence::EveryNth { frames: 1 },
            activity: wiring.activity,
            optics,
            renderer: wiring.renderer,
            frame_period: wiring.frame_period,
            external_period: options.period_ticks,
            sensor_clock: wiring.sensor_clock,
            seen_sensor_frame: None,
            seen_external_frame: None,
            last_physical,
        })
    }
    pub fn configure(
        &mut self,
        period: u64,
        cadence: StatisticsCadence,
        now: u64,
    ) -> Result<(), String> {
        let cadence = cadence.validate()?;
        if matches!(cadence, StatisticsCadence::Activity { .. }) && self.activity.is_none() {
            return Err("activity statistics cadence requires a physical input wire".into());
        }
        if self.phase.is_some() {
            return Err("external frames use the wired input phase clock".into());
        }
        let mut clock = self.clock.clone();
        if period == 0 {
            return Err("external frame period must be positive".into());
        }
        clock.set_period(self.selected_period_or(period)?, now)?;
        self.clock = clock;
        self.cadence = cadence;
        self.external_period = period;
        Ok(())
    }
    fn selected_period(&self) -> Result<u64, String> {
        self.selected_period_or(self.external_period)
    }
    fn physical_edge(&self) -> Option<FrameEdge> {
        let clock = self.sensor_clock.as_ref()?;
        let image = self.images.0.lock().ok()?;
        if !matches!(image.source, ImageSource::Sensor) {
            return None;
        }
        drop(image);
        clock.lock().ok()?.edge
    }
    fn uses_sensor_clock(&self) -> bool {
        self.sensor_clock.is_some()
            && self
                .images
                .0
                .lock()
                .is_ok_and(|image| matches!(image.source, ImageSource::Sensor))
    }
    fn selected_period_or(&self, external_period: u64) -> Result<u64, String> {
        if matches!(
            self.images
                .0
                .lock()
                .map_err(|_| "frame source poisoned")?
                .source,
            ImageSource::Sensor
        ) {
            if let Some(period) = &self.frame_period {
                let period = *period.lock().map_err(|_| "frame period poisoned")?;
                if period > 0 {
                    return Ok(period);
                }
            }
        }
        Ok(external_period)
    }
}
impl fujimod_emulator_devices::capability::FrameSource for ClockedFrames {
    fn configure(&mut self, period: u64, cadence: StatisticsCadence, now: u64) -> Result<(), String> {
        ClockedFrames::configure(self, period, cadence, now)
    }
    fn frame_state(&self) -> serde_json::Value {
        self.save()
    }
}
impl Device for ClockedFrames {
    fn frame_source(&self) -> Option<&(dyn fujimod_emulator_devices::capability::FrameSource + 'static)> {
        Some(self)
    }
    fn frame_source_mut(
        &mut self,
    ) -> Option<&mut (dyn fujimod_emulator_devices::capability::FrameSource + 'static)> {
        Some(self)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        let mut children: Vec<_> = self
            .optics
            .as_ref()
            .map(|o| o.checkpoint())
            .transpose()?
            .into_iter()
            .collect();
        if let Some(renderer) = &self.renderer {
            children.push(renderer.checkpoint()?);
        }
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                &self.clock,
                self.frames,
                self.cadence,
                self.optics.is_some(),
                self.renderer.is_some(),
                self.phase.as_ref().map(|(_, index)| *index),
                self.external_period,
                self.frame_period.is_some(),
                self.sensor_clock.is_some(),
                self.seen_sensor_frame,
                self.seen_external_frame,
                self.last_physical,
            ),
        )?
        .with_children(children))
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (
            clock,
            _,
            cadence,
            optics,
            renderer,
            phase,
            external_period,
            frame_period,
            sensor_clock,
            seen_sensor,
            seen_external,
            last_physical,
        ): (
            FrameCadence,
            u64,
            StatisticsCadence,
            bool,
            bool,
            Option<usize>,
            u64,
            bool,
            bool,
            Option<FrameEdge>,
            Option<FrameEdge>,
            bool,
        ) = saved.decode(self.name())?;
        cadence.validate()?;
        if sensor_clock != self.sensor_clock.is_some()
            || (!sensor_clock && last_physical)
            || seen_sensor.is_some_and(|e| e.index == 0)
            || seen_external.is_some_and(|e| e.index == 0)
            || external_period == 0
            || clock.period() == 0
            || frame_period != self.frame_period.is_some()
            || phase != self.phase.as_ref().map(|(_, index)| *index)
            || optics != self.optics.is_some()
            || renderer != self.renderer.is_some()
            || saved.children().len() != usize::from(optics) + usize::from(renderer)
            || !saved.blobs().is_empty()
            || matches!(cadence, StatisticsCadence::Activity { .. }) && self.activity.is_none()
        {
            return Err("external image clock checkpoint wiring differs".into());
        }
        if let Some(renderer) = &self.renderer {
            renderer.validate_checkpoint(&saved.children()[usize::from(optics)])?;
        }
        if let Some(optics) = &self.optics {
            let bytes = self
                .images
                .0
                .lock()
                .map_err(|_| "image-frame poisoned")?
                .layout
                .bytes()?;
            optics.validate_checkpoint(&saved.children()[0], bytes)?;
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (
            clock,
            frames,
            cadence,
            optics,
            _,
            _,
            external_period,
            _,
            _,
            seen_sensor_frame,
            seen_external_frame,
            last_physical,
        ): (
            FrameCadence,
            u64,
            StatisticsCadence,
            bool,
            bool,
            Option<usize>,
            u64,
            bool,
            bool,
            Option<FrameEdge>,
            Option<FrameEdge>,
            bool,
        ) = saved.decode(self.name())?;
        if let Some(optics) = &mut self.optics {
            let bytes = self
                .images
                .0
                .lock()
                .map_err(|_| "image-frame poisoned")?
                .layout
                .bytes()?;
            optics.restore_checkpoint(&saved.children()[0], bytes)?;
        }
        if let Some(renderer) = &mut self.renderer {
            renderer.restore_checkpoint(&saved.children()[usize::from(optics)])?;
        }
        self.clock = clock;
        self.frames = frames;
        self.cadence = cadence;
        self.external_period = external_period;
        self.seen_sensor_frame = seen_sensor_frame;
        self.seen_external_frame = seen_external_frame;
        self.last_physical = last_physical;
        Ok(())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"clock":self.clock,"phase_source":self.phase.as_ref().map(|(_,i)|i),"frames":self.frames,"cadence":self.cadence,"external_period":self.external_period,"seen_sensor_frame":self.seen_sensor_frame,"seen_external_frame":self.seen_external_frame,"sensor_clock":self.sensor_clock.is_some(),"last_physical":self.last_physical,"frame_period":self.frame_period.is_some(),
            "images":self.images.fingerprint(),"statistics":self.statistics.fingerprint(),
            "optics":self.optics.as_ref().map(|o|o.fingerprint()),"renderer":self.renderer.as_ref().map(|r|r.fingerprint())}),
        )
    }
    fn name(&self) -> &'static str {
        "external-image-frames"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        if self.last_physical != self.uses_sensor_clock() {
            return Some(now);
        }
        if self.uses_sensor_clock() {
            return self
                .physical_edge()
                .filter(|edge| Some(*edge) != self.seen_sensor_frame)
                .map(|edge| edge.tick.max(now));
        }
        if self.phase.is_none()
            && self
                .selected_period()
                .is_ok_and(|p| p != self.clock.period())
        {
            return Some(now);
        }
        match &self.phase {
            Some((edges, index)) => edges.lock().unwrap()[*index]
                .filter(|edge| edge.index > self.frames)
                .map(|edge| edge.tick.max(now)),
            None => self.clock.next_deadline(),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let physical = self.uses_sensor_clock();
        if self.last_physical != physical {
            self.last_physical = physical;
            if physical {
                if let Some(edge) = self.physical_edge().filter(|e| e.tick <= cx.now()) {
                    self.seen_sensor_frame = Some(edge);
                }
            } else {
                self.clock.take_due(cx.now());
            }
            return;
        }
        if self.phase.is_none() {
            if let Err(error) = self
                .selected_period()
                .and_then(|p| self.clock.set_period(p, cx.now()))
            {
                cx.fail(error);
                return;
            }
        }
        let physical = self.uses_sensor_clock();
        let edge = if physical {
            self.physical_edge()
                .filter(|edge| Some(*edge) != self.seen_sensor_frame && edge.tick <= cx.now())
        } else {
            match &self.phase {
                Some((edges, index)) => edges.lock().unwrap()[*index].filter(|edge| {
                    Some(*edge) != self.seen_external_frame && edge.tick <= cx.now()
                }),
                None => self.clock.take_due(cx.now()),
            }
        };
        let Some(edge) = edge else {
            return;
        };
        let previous = if physical {
            self.seen_sensor_frame
        } else {
            self.seen_external_frame
        };
        let Some(delta) = edge
            .index
            .checked_sub(previous.map_or(0, |e| e.index))
            .filter(|&n| n > 0)
        else {
            cx.fail("source frame index did not advance");
            return;
        };
        let Some(index) = self.frames.checked_add(delta) else {
            cx.fail("image frame index overflow");
            return;
        };
        if physical {
            self.seen_sensor_frame = Some(edge);
        } else {
            self.seen_external_frame = Some(edge);
        }
        let source_index = edge.index;
        let edge = FrameEdge {
            index,
            tick: edge.tick,
        };
        let mut image = self.images.0.lock().unwrap();
        let changed = image.prepared.is_some()
            || !matches!(image.source, ImageSource::External);
        if let Some(pixels) = image.prepared.take() {
            image.pixels = Pixels::Packed(pixels);
        }
        if matches!(image.source, ImageSource::Sensor) {
            let Some(renderer) = &mut self.renderer else {
                cx.fail("image source has no sensor renderer");
                return;
            };
            match renderer.capture() {
                Ok(Some(frame)) => {
                    image.radiance = Some(Arc::new(frame.exposure.clone()));
                    image.pixels = Pixels::Sensor(frame);
                }
                Ok(None) => image.radiance = None,
                Err(e) => {
                    cx.fail(e);
                    return;
                }
            }
        } else if matches!(image.source, ImageSource::Analytic) {
            image.radiance = None;
            let layout = image.layout;
            let bytes = match image.pixels.packed_mut(layout, cx.compute_workers()) {
                Ok(pixels) => std::sync::Arc::make_mut(pixels),
                Err(e) => {
                    cx.fail(e);
                    return;
                }
            };
            for y in 0..layout.height {
                for x in (0..layout.width).step_by(2) {
                    let i = y * layout.stride + x * 2;
                    let phase = (edge.index % 512) as usize;
                    bytes[i..i + 4].copy_from_slice(&[
                        ((x + y + phase * 3) % 192 + 32) as u8,
                        ((x + y + phase * 3 + 1) % 192 + 32) as u8,
                        ((x / 8 + phase) % 96 + 80) as u8,
                        ((y / 8 + phase) % 96 + 80) as u8,
                    ]);
                }
            }
        } else {
            image.radiance = None;
        }
        if let Some(optics) = &mut self.optics {
            if let Err(e) = optics.render(&mut image, changed) {
                cx.fail(e);
                return;
            }
        }
        image.edge = Some(edge);
        drop(image);
        let active = match &self.activity {
            Some((pins, selection)) => match pins.level(selection.pin) {
                Ok(high) => high == selection.high,
                Err(error) => {
                    cx.fail(error);
                    return;
                }
            },
            None => false,
        };
        if let Err(error) = self
            .cadence
            .admits(source_index, active, false)
            .and_then(|admit| {
                if admit {
                    self.statistics.publish(edge)
                } else {
                    Ok(())
                }
            })
        {
            cx.fail(error);
            return;
        }
        self.frames = edge.index;
    }
    fn save(&self) -> serde_json::Value {
        let image = self.images.0.lock().unwrap();
        serde_json::json!({"frames":self.frames,"phase_source":self.phase.as_ref().map(|(_,i)|i),"clock":self.clock,"statistics_cadence":self.cadence,"external_period":self.external_period,"seen_sensor_frame":self.seen_sensor_frame,"seen_external_frame":self.seen_external_frame,"sensor_clock":self.sensor_clock.is_some(),"last_physical":self.last_physical,"frame_period":self.frame_period.is_some(),
            "physical_exposure":image.radiance.as_deref().and_then(|e|ExposedFrame::of(e).ok()).map(|e|e.physical_state()),
            "readouts":match &image.pixels {Pixels::Sensor(p)=>Image::of(p).ok().map(|p|p.readouts()), _=>None},
            "image_edge":image.edge,
            "focus_position":self.optics.as_ref().and_then(|o|o.last_position())})
    }
}
