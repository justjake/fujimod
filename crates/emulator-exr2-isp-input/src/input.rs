//! FFF10000 image input with delayed reset/phase acknowledgments, packed
//! sensor-command transport, declared physical phase clocks, and RAW channels
//! 2/4. No startup ledger, program signature, or instruction inspection.
mod registers;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_image_processing::raw::{Cfa, Packing, RawView};
use fujimod_emulator_imaging::exr2_isp::memory_outputs::MemoryOutputHandle;
use fujimod_emulator_imaging::exr2_isp::options::input::{
    ClockMode, ClockSource, Command, Options, PhaseAck,
};
use fujimod_emulator_imaging::exr2_isp::{
    dma::{Raster, Transfer, physical},
    due,
    parameters::ParameterHandle,
    word,
};
use fujimod_emulator_imaging::film::Film;
use fujimod_emulator_imaging::scene::FrameEdge;
use fujimod_emulator_parts_sensor::sensor::Sensor;
use serde::{Deserialize, Serialize};

pub struct Wiring {
    pub frame_period: Option<fujimod_emulator_imaging::timing::FramePeriod>,
    pub statistics_grid: Option<fujimod_emulator_imaging::analysis::GridHandle>,
    pub phase_irqs: [Irq; 2],
    pub raw_irq: Irq,
    pub readback_irq: Irq,
    pub shutter_output: Option<usize>,
    pub sensor: Option<Sensor>,
    pub parameters: Option<ParameterHandle>,
    pub memory_outputs: Option<MemoryOutputHandle>,
    pub edges: fujimod_emulator_imaging::scene::FramePhases,
    pub start_tick: u64,
}
#[derive(Serialize, Deserialize)]
struct State {
    r: Vec<u32>,
    reset: Option<u64>,
    command_due: Option<u64>,
    raw_exposure: Option<serde_json::Value>,
    command_armed: bool,
    command_done: bool,
    command_bytes: Vec<u8>,
    phases: [Option<u64>; 6],
    phase_due: [Option<u64>; 2],
    phase_pending: [u32; 2],
    phase_period: u64,
    sensor_due: Option<u64>,
    capture_ack: bool,
    capture_irq_sent: bool,
    raw3_ack: bool,
    raw3_irq_sent: bool,
    readback_due: Option<u64>,
    stop_due: Option<u64>,
    dma: Transfer,
    raw3_dma: Transfer,
    raw_origins: std::collections::BTreeMap<u64, [usize; 2]>,
}
pub struct ImageInput {
    state: State,
    options: Options,
    wiring: Wiring,
    cfa: Option<Cfa>,
    film: Film,
    source: Vec<u8>,
}
impl ImageInput {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if wiring.shutter_output.is_some_and(|n| n >= 6) {
            return Err("shutter output index exceeds comparator bank".into());
        }
        if options.reset_ticks < 2
            || options.command_ticks < 2
            || options.phase_ticks < 2
            || options.raw_dma_ticks == 0
            || options.readback_ticks == 0
            || options.sensor_period_ticks == Some(0)
            || options.clock.as_ref().is_some_and(|c| {
                matches!(
                    c.source,
                    ClockSource::Periodic {
                        period_ticks: 0,
                        ..
                    } | ClockSource::Periodic {
                        first_delay_ticks: Some(0),
                        ..
                    }
                )
            })
            || options.command_controls.is_empty()
            || options.command_controls.len() > 8
            || options
                .command_controls
                .iter()
                .any(|v| ![0, 2, 0x1000000, 0x1030000].contains(v))
        {
            return Err("image input timing/command options are invalid".into());
        }
        if options.sensor_period_ticks.is_some() && wiring.sensor.is_none() {
            return Err("sensor frame clock has no attached sensor".into());
        }
        if options
            .clock
            .as_ref()
            .is_some_and(|c| c.source == ClockSource::Sensor {})
            && (wiring.sensor.is_none() || options.sensor_period_ticks.is_none())
        {
            return Err("input sensor clock requires an attached source and frame period".into());
        }
        let sensor_due = options
            .sensor_period_ticks
            .map(|t| due(wiring.start_tick, t))
            .transpose()?;
        let cfa = options.cfa.clone().map(Cfa::new).transpose()?;
        if wiring.memory_outputs.is_some() && cfa.is_none() {
            return Err("RAW readback requires a declared CFA layout".into());
        }
        Ok(Self {
            state: State {
                r: vec![0; 0x400 / 4],
                reset: None,
                command_due: None,
                raw_exposure: None,
                command_armed: false,
                command_done: false,
                command_bytes: Vec::with_capacity(4096),
                phases: [None; 6],
                phase_due: [None; 2],
                phase_pending: [0; 2],
                phase_period: 0,
                sensor_due,
                capture_ack: false,
                capture_irq_sent: false,
                raw3_ack: false,
                raw3_irq_sent: false,
                readback_due: None,
                stop_due: None,
                dma: Transfer::default(),
                raw3_dma: Transfer::default(),
                raw_origins: Default::default(),
            },
            options,
            wiring,
            cfa,
            film: Film::default(),
            source: Vec::new(),
        })
    }
    fn phase(o: usize) -> Option<usize> {
        if (0xc4..=0x128).contains(&o) && (o - 0xc4).is_multiple_of(0x14) {
            Some((o - 0xc4) / 0x14)
        } else {
            None
        }
    }
    fn valid(&self, o: usize) -> bool {
        registers::mask(o).is_some()
            || o == 0
            || o == 0x384
            || self.options.command.is_some() && [0x154, 0x160].contains(&o)
    }
    fn clock_gates(&self) -> [bool; 2] {
        let s = &self.state;
        let r = &s.r;
        let Some(c) = &self.options.clock else {
            return [false; 2];
        };
        if s.reset.is_some() {
            return [false; 2];
        }
        let mask = r[0x38c / 4];
        match c.mode {
            ClockMode::InputEnable => {
                if r[0x48 / 4] & 1 == 0 || r[0x70 / 4] & 1 != 0 || mask & 0x10000 == 0 {
                    return [false; 2];
                }
                [
                    mask & 1 != 0,
                    c.secondary
                        && (mask & 1 == 0 && mask & 2 != 0
                            || c.companion_secondary && mask & 2 != 0),
                ]
            }
            ClockMode::RegisterPeriods | ClockMode::SourceMask => [
                r[0] & 1 != 0 && mask & 1 != 0,
                c.secondary
                    && r[0] & 1 != 0
                    && mask & 0x202 != 0
                    && (c.mode == ClockMode::SourceMask || r[0x24 / 4] != 0),
            ],
        }
    }
    fn sync_clocks(&mut self, now: u64) -> Result<(), String> {
        let gates = self.clock_gates();
        let period = self.phase_period()?;
        if period != self.state.phase_period {
            self.state.phase_due = [None; 2];
            self.state.phase_period = period;
            if self.wiring.frame_period.is_some()
                && self.options.sensor_period_ticks.is_some()
                && period > 0
            {
                self.state.sensor_due = Some(due(now, period)?);
            }
        }
        if self.uses_sensor_clock() {
            self.state.phase_due = [None; 2];
            return Ok(());
        }
        if let Some(c) = &self.options.clock {
            for (i, enabled) in gates.iter().enumerate() {
                if !enabled {
                    self.state.phase_due[i] = None;
                }
            }
            // Source masks gate delivery from one input scan clock. Enabling
            // another source joins its next edge; command transport and
            // comparator acknowledgments do not restart that physical clock.
            let input_edge = if c.mode == ClockMode::InputEnable {
                self.state.phase_due.iter().flatten().copied().min()
            } else {
                None
            };
            for i in 0..2 {
                if !gates[i] {
                    self.state.phase_due[i] = None;
                } else if self.state.phase_due[i].is_none() {
                    let delay = if i == 1 && c.mode == ClockMode::RegisterPeriods {
                        self.state.r[0x24 / 4] as u64
                    } else {
                        if self.wiring.frame_period.is_some() {
                            period
                        } else {
                            match c.source {
                                ClockSource::Periodic {
                                    first_delay_ticks, ..
                                } => first_delay_ticks.unwrap_or(period),
                                ClockSource::Sensor {} => unreachable!(),
                            }
                        }
                    };
                    self.state.phase_due[i] = Some(input_edge.unwrap_or(due(now, delay)?));
                }
            }
        }
        Ok(())
    }
    fn uses_sensor_clock(&self) -> bool {
        self.options
            .clock
            .as_ref()
            .is_some_and(|c| c.source == ClockSource::Sensor {})
    }
    fn phase_period(&self) -> Result<u64, String> {
        let fixed = self.options.clock.as_ref().map_or(0, |c| match c.source {
            ClockSource::Periodic { period_ticks, .. } => period_ticks,
            ClockSource::Sensor {} => self.options.sensor_period_ticks.unwrap(),
        });
        self.selected_period(fixed)
    }
    fn publish_phase(&mut self, i: usize, edge: FrameEdge, cx: &mut Cx<'_>) -> Result<(), String> {
        self.wiring
            .edges
            .lock()
            .map_err(|_| "phase edges poisoned")?[i] = Some(edge);
        if self.state.phase_pending[i] != 0 {
            return Ok(());
        }
        let clock = self.options.clock.as_ref().unwrap();
        let flags = if i == 0 {
            1
        } else if clock.mode != ClockMode::InputEnable {
            (u32::from(self.state.r[0x38c / 4] & 0x200 != 0) * 2)
                | (u32::from(self.state.r[0x38c / 4] & 2 != 0) * 0x200)
        } else {
            2
        };
        self.state.phase_pending[i] = flags;
        if clock.mode != ClockMode::InputEnable {
            self.state.r[0x384 / 4] |= flags;
        }
        self.wiring.phase_irqs[i].raise(cx)
    }
    fn selected_period(&self, fixed: u64) -> Result<u64, String> {
        let period = self
            .wiring
            .frame_period
            .as_ref()
            .map(|p| {
                p.lock()
                    .map(|v| *v)
                    .map_err(|_| "input frame period poisoned")
            })
            .transpose()?
            .unwrap_or(0);
        Ok(if period == 0 { fixed } else { period })
    }
    fn command_arm(&mut self, cx: &Cx<'_>, v: u32) -> Result<(), String> {
        let s = &mut self.state;
        let r = &s.r;
        if v == 0 {
            if s.command_due.is_some() {
                return Err("stopping an active input command is unsupported".into());
            }
            s.command_armed = false;
            return Ok(());
        }
        if self.options.command.is_none()
            || v != 0x10001
            || s.command_armed
            || s.command_due.is_some()
            || !self.options.command_controls.contains(&r[0x98 / 4])
            || r[0x140 / 4] != 0
            || r[0x170 / 4] != 0x101000
            || r[0x178 / 4] != 0
            || r[0x15c / 4] != 7
        {
            return Err("unsupported input command channel arm".into());
        }
        let records = (r[0x9c / 4] >> 16) as usize + 1;
        if r[0x9c / 4] & 65535 != 2 || records > 512 {
            return Err("unsupported packed command count/unit".into());
        }
        physical(r[0x174 / 4] as u64, records * 8)?;
        s.command_bytes.resize(records * 8, 0);
        cx.ram_read(r[0x174 / 4] as u64, &mut s.command_bytes)?;
        if self.options.command == Some(Command::PackedWrites)
            && s.command_bytes
                .as_chunks::<8>()
                .0
                .iter()
                .any(|b| b[0] != 0x81 || b[3..].iter().any(|&v| v != 0))
        {
            return Err("input command opcode/padding is unsupported".into());
        }
        s.command_armed = true;
        Ok(())
    }
    fn readback_start(&mut self, cx: &Cx<'_>, v: u32) -> Result<(), String> {
        let s = &mut self.state;
        let r = &s.r;
        if v != 1
            || s.readback_due.is_some()
            || s.stop_due.is_some()
            || r[0x398 / 4] & 65535 != 0
            || r[0x3b8 / 4] != 0xfb000000
        {
            return Err("RAW channel4 requires an idle full-field transport".into());
        }
        // Separate packed-sample transport from the burst selector. The
        // packed path programs its line pitch in samples; U16 uses bytes.
        // Other source controls and partial-field modes remain unsupported.
        let (packing, pitch) = match (r[0x394 / 4], r[0x3ac / 4], r[0x39c / 4]) {
            (0x1000000, 0xc003, 0x40000) => (Packing::U16, r[0x3a4 / 4] as usize),
            (0x1000100, 0xc004, 0) => (
                Packing::Packed12,
                Packing::Packed12.row_bytes(r[0x3a4 / 4] as usize),
            ),
            _ => return Err("RAW channel4 packing/burst/field controls are unsupported".into()),
        };
        let src = Raster {
            address: r[0x3a8 / 4] as u64,
            width: (r[0x398 / 4] >> 16) as usize,
            height: r[0x3a0 / 4] as usize,
            pitch,
        };
        src.validate(packing.row_bytes(src.width))?;
        if src.width < 6
            || src.height < 6
            || src.width % 2 != 0
            || src.address & 1 != 0
            || r[0x3a4 / 4] & 1 != 0
        {
            return Err("RAW channel4 geometry is unsupported".into());
        }
        let mut outputs = self
            .wiring
            .memory_outputs
            .as_ref()
            .ok_or("RAW channel4 has no connected output channels")?
            .lock()
            .unwrap();
        if outputs.active {
            return Err("RAW channel4 requires idle programmed outputs".into());
        }
        let targets = outputs.destinations;
        if match targets {
            [Some(a), Some(b)] => a.overlaps(b)?,
            _ => false,
        } || targets.iter().flatten().any(|r| r.width % 2 != 0)
        {
            return Err("RAW readback outputs overlap or have odd widths".into());
        }
        let bank = self
            .wiring
            .parameters
            .as_ref()
            .ok_or("RAW readback has no parameter bank")?
            .lock()
            .unwrap();
        let gains = [
            (bank.words[0x200 / 4] & 32767) as u16,
            (bank.words[0x200 / 4] >> 16 & 32767) as u16,
            (bank.words[0x204 / 4] & 32767) as u16,
        ];
        let words = &bank.words;
        self.film.update(|o| words[o / 4])?;
        let mut cfa = self.cfa.as_ref().ok_or("RAW readback has no CFA layout")?.clone();
        if self.wiring.sensor.is_some() && r[1] & 1 != 0 {
            let [left, top] = *s.raw_origins.get(&physical(src.address, 1)?)
                .ok_or("RAW readback source has no captured sensor window")?;
            cfa = cfa.window(left, top);
        } else if r[1] & 1 != 0 && r[0x35c / 4] & 1 != 0 {
            if physical(src.address, 1)? != physical(r[0x358 / 4] as u64, 1)? {
                return Err("RAW readback source differs from the declared RAW window".into());
            }
            cfa = cfa.window((r[0x14 / 4] >> 16 & 0x3fff) as usize,
                             (r[0x18 / 4] >> 16 & 0x3fff) as usize);
        }
        self.source.resize(src.pitch * src.height, 0);
        cx.ram_read(src.address, &mut self.source)?;
        let raw = RawView {
            bytes: &self.source,
            width: src.width,
            height: src.height,
            pitch: src.pitch,
            packing,
        };
        let result = (|| {
            for (i, target) in targets.iter().enumerate() {
                let Some(target) = target else {
                    outputs.transfers[i].cancel();
                    continue;
                };
                outputs.transfers[i].prepare(
                    cx.now(),
                    self.options.readback_ticks,
                    *target,
                    target.width * 2,
                )?;
                cfa.render(
                        &raw,
                        &self.film,
                        0,
                        gains,
                        256,
                        |_, _| 2048,
                        [0, 0, raw.width, raw.height],
                        target.width,
                        target.height,
                        &mut outputs.transfers[i].pixels,
                    )?;
            }
            Ok::<(), String>(())
        })();
        if let Err(e) = result {
            for t in &mut outputs.transfers {
                t.cancel();
            }
            return Err(e);
        }
        outputs.active_destinations = outputs.destinations;
        outputs.active = true;
        s.readback_due = Some(due(
            cx.now(),
            self.options
                .readback_ticks
                .checked_add(1)
                .ok_or("RAW readback delay overflow")?,
        )?);
        s.r[0] &= !2;
        Ok(())
    }
    fn store(&mut self, cx: &Cx<'_>, o: usize, v: u32) -> Result<(), String> {
        // RAW mode uses bit 0. Writes may leave the remaining local-word bits
        // uninitialized; those bits do not select
        // additional transport modes. Keep the implemented register bit only.
        let v = if matches!(o, 0x35c | 0x378) { v & 1 } else { v };
        let old = self.state.r[o / 4];
        if o == 0x94 && v == 0 && self.state.command_due.is_some() {
            // Clearing START stops the transmitter. The receiver gets no
            // completed command, and the channel can be disarmed and reused.
            self.state.command_due = None;
            self.state.command_armed = false;
            self.state.command_done = true;
        }
        if self.state.command_due.is_some() && o != 0x388 {
            return Err("input reconfiguration during command transfer".into());
        }
        if self.state.command_armed && ![0x94, 0x154, 0x15c, 0x388].contains(&o) {
            return Err("input reconfiguration with armed command".into());
        }
        if self.state.phases.iter().any(Option::is_some) && Self::phase(o).is_none() && o != 0x388 {
            return Err("input reconfiguration during phase acknowledgment".into());
        }
        if self.state.readback_due.is_some() && o == 0x3ac && v != old {
            return Err("RAW readback interrupt gate changed during DMA".into());
        }
        if let Some(i) = Self::phase(o) {
            if v & !registers::mask(o).unwrap() != 0 || self.state.phases[i].is_some() {
                return Err("unknown or pending input phase fields".into());
            }
            if v & 3 == 2 {
                let preserve = self.options.phase_ack == PhaseAck::PreserveFields;
                if self.options.phase_ack == PhaseAck::Disabled
                    || old & 3 != 3
                    || (!preserve && (old != 3 || v != 2))
                    || preserve && (old & !3) != (v & !3)
                    || self.state.r[0x13c / 4] != 1
                    || self.state.r[0x70 / 4] & 1 != 0
                    || self.options.phase_ack == PhaseAck::Inactive
                        && self.state.r[0x48 / 4] & 1 != 0
                {
                    return Err(
                        "phase ACK requires staged mode3, mode2 request and enable gates".into(),
                    );
                }
                self.state.phases[i] = Some(due(cx.now(), self.options.phase_ticks)?);
            }
        } else {
            match o {
                0 => {
                    if v & !old != 0 || (old ^ v) & !0x40004003 != 0 {
                        return Err("unknown global input command".into());
                    }
                    if v & 1 == 0 {
                        self.state.reset = Some(due(cx.now(), self.options.reset_ticks)?);
                    }
                    if old & 2 != 0 && v & 2 == 0 {
                        if self.state.readback_due.is_some() {
                            return Err("stopping active RAW memory input is unsupported".into());
                        }
                        self.state.stop_due = Some(due(cx.now(), 16)?);
                    }
                }
                0x390 => self.readback_start(cx, v)?,
                0x4c | 0x74 => {
                    if v != 1 {
                        return Err("unknown input enable strobe".into());
                    }
                }
                0x154 => self.command_arm(cx, v)?,
                0x94 if v != 0 => {
                    if v != 1 || !self.state.command_armed || self.state.r[0x154 / 4] != 0x10001 {
                        return Err("input command START requires an armed channel".into());
                    }
                    self.state.command_due = Some(due(cx.now(), self.options.command_ticks)?);
                }
                0x15c | 0x350 | 0x36c => {
                    if v & !7 != 0 {
                        return Err("unknown RAW/command ACK".into());
                    }
                    if o == 0x350 && v & 1 != 0 {
                        self.state.capture_ack = false;
                        self.state.capture_irq_sent = false;
                    } else if o == 0x36c && v & 1 != 0 {
                        self.state.raw3_ack = false;
                        self.state.raw3_irq_sent = false;
                    }
                }
                0x3b4 => {
                    if v & !0x10000003 != 0 {
                        return Err("unknown RAW channel4 ACK".into());
                    }
                }
                0x160 | 0x384 => return Err("input status is read-only".into()),
                0x388 => {
                    if v > 1 {
                        return Err("unknown phase selector ACK".into());
                    }
                    let source = usize::from(v == 0);
                    self.state.r[0x384 / 4] &= !self.state.phase_pending[source];
                    self.state.phase_pending[source] = 0;
                }
                _ => {
                    if !registers::valid(o, v) {
                        return Err("input word exceeds its mask/domain".into());
                    }
                }
            }
        }
        self.state.r[o / 4] = if [0x350, 0x36c, 0x3b4].contains(&o) {
            0
        } else {
            v
        };
        if matches!(o, 0x28 | 0x2c) {
            self.publish_grid()?;
        }
        self.sync_clocks(cx.now())
    }
    fn publish_grid(&self) -> Result<(), String> {
        if let Some(grid) = &self.wiring.statistics_grid {
            *grid.lock().map_err(|_| "input image grid poisoned")? = [
                (self.state.r[0x28 / 4] & 0xfff) as usize + 1,
                (self.state.r[0x2c / 4] & 0x3fff) as usize + 1,
            ];
        }
        Ok(())
    }
    fn sensor_timing(&mut self) -> Result<(), String> {
        let Some(sensor) = &mut self.wiring.sensor else {
            return Ok(());
        };
        let horizontal = (self.state.r[0x54 / 4] & 0x3fff) + 1;
        // The graph connects a programmable comparator output to the external
        // shutter drive. A point packs horizontal clocks above vertical lines.
        let shutter_drive = self.wiring.shutter_output.and_then(|output| {
            let offset = 0xc4 + output * 0x14;
            (self.state.r[offset / 4] & 0x10000 != 0).then(|| {
                let point = self.state.r[(offset + 4) / 4];
                u64::from(point & 0x3fff) * u64::from(horizontal) + u64::from(point >> 16 & 0x3fff)
            })
        });
        sensor.timing(
            horizontal,
            (self.state.r[0x58 / 4] & 0x3fff) + 1,
            shutter_drive,
        )?;
        Ok(())
    }
    /// Latch one sensor frame and start its RAW transfer. Returns whether the
    /// sensor delivered a valid frame; an invalid readout has no edge or DMA.
    fn frame(&mut self, cx: &Cx<'_>) -> Result<bool, String> {
        self.sensor_timing()?;
        let Some(sensor) = &mut self.wiring.sensor else {
            return Ok(true);
        };
        sensor.preview_view([
            (self.state.r[0x28 / 4] >> 16 & 0x3fff) as usize,
            (self.state.r[0x28 / 4] & 0xfff) as usize + 1,
        ])?;
        if sensor.latch(cx.now())?.is_none() {
            return Ok(false);
        }
        let State {
            r,
            dma,
            raw3_dma,
            raw_exposure,
            raw_origins,
            capture_ack,
            raw3_ack,
            ..
        } = &mut self.state;
        if r[1] & 1 != 0 && r[0x35c / 4] & 1 != 0 && !*capture_ack && dma.deadline().is_none() {
            *dma = Self::prepare_raw_channel(
                cx.now(),
                self.options.raw_dma_ticks,
                &r,
                sensor,
                0x354,
                0x358,
                0x360,
                "RAW channel2",
            )?;
            *raw_exposure = sensor.physical_exposure();
            raw_origins.insert(physical(r[0x358 / 4] as u64, 1)?,
                [(r[0x14 / 4] >> 16 & 0x3fff) as usize, (r[0x18 / 4] >> 16 & 0x3fff) as usize]);
        }
        if r[1] & 1 != 0 && r[0x378 / 4] & 1 != 0 && !*raw3_ack && raw3_dma.deadline().is_none() {
            *raw3_dma = Self::prepare_raw_channel(
                cx.now(),
                self.options.raw_dma_ticks,
                &r,
                sensor,
                0x370,
                0x374,
                0x37c,
                "RAW channel3",
            )?;
            raw_origins.insert(physical(r[0x374 / 4] as u64, 1)?,
                [(r[0x14 / 4] >> 16 & 0x3fff) as usize, (r[0x18 / 4] >> 16 & 0x3fff) as usize]);
        }
        Ok(true)
    }
    fn prepare_raw_channel(
        now: u64,
        ticks: u64,
        r: &[u32],
        sensor: &mut Sensor,
        format_offset: usize,
        destination_offset: usize,
        stride_offset: usize,
        channel_name: &str,
    ) -> Result<Transfer, String> {
        let format = r[format_offset / 4] >> 20 & 7;
        // The low byte is independent of the burst-format field. Bits 28..30
        // gate completion/error interrupts, not the DMA itself.
        if ![3, 4].contains(&format) || r[1] & 2 != 0 {
            return Err(format!("{channel_name} transport is unsupported"));
        }
        let packing = if r[1] & 0x10000 != 0 {
            Packing::Packed12
        } else {
            Packing::U16
        };
        let width = (r[0x24 / 4] & 0x3fff) as usize;
        let height = (r[0x18 / 4] & 0x3fff) as usize + 1;
        let left = (r[0x14 / 4] >> 16 & 0x3fff) as usize;
        let top = (r[0x18 / 4] >> 16 & 0x3fff) as usize;
        if r[0x14 / 4] & !0x3fff1fff != 0
            || r[0x18 / 4] & !0x3fff3fff != 0
            || width == 0
            || width % 2 != 0
            || width > (r[0x14 / 4] & 0x1fff) as usize + 1
            || left + width > sensor.width
            || top + height > sensor.height
        {
            return Err(format!("{channel_name} requires a bounded input window"));
        }
        let row = packing.row_bytes(width);
        let alignment = (if packing == Packing::Packed12 { 4 } else { 1 }) << (format + 3);
        let destination = Raster {
            address: r[destination_offset / 4] as u64,
            width,
            height,
            pitch: packing
                .row_bytes(width.div_ceil(alignment) * alignment)
                .checked_add(r[stride_offset / 4] as usize)
                .ok_or_else(|| format!("{channel_name} pitch overflow"))?,
        };
        let mut transfer = Transfer::default();
        transfer.prepare_deferred(now, ticks, destination, row)?;
        let readout = sensor.freeze_readout(packing, [left, top, width, height])?;
        transfer
            .pixels
            .defer_rows(row, height, move |rows, out| readout.write_rows(rows, out));
        Ok(transfer)
    }
    fn tick(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let now = cx.now();
        if self.state.reset.is_some_and(|t| now >= t) {
            self.state.reset = None;
            self.state.r[0] |= 1;
        }
        if self.state.command_due.is_some_and(|t| now >= t) {
            self.sensor_timing()?;
            if let Some(sensor) = &mut self.wiring.sensor {
                sensor.command(&self.state.command_bytes)?;
            }
            self.state.command_due = None;
            self.state.command_armed = false;
            self.state.command_done = true;
            self.state.r[0x94 / 4] &= !1;
        }
        for i in 0..6 {
            if self.state.phases[i].is_some_and(|t| now >= t) {
                self.state.phases[i] = None;
                let o = 0xc4 + i * 0x14;
                self.state.r[o / 4] |= 3;
            }
        }
        self.sync_clocks(now)?;
        for i in 0..2 {
            if self.state.phase_due[i].is_some_and(|t| now >= t) {
                let clock = self.options.clock.as_ref().unwrap();
                let period = if i == 1 && clock.mode == ClockMode::RegisterPeriods {
                    self.state.r[0x24 / 4] as u64
                } else {
                    self.state.phase_period
                };
                let elapsed = (now - self.state.phase_due[i].unwrap()) / period + 1;
                let tick = self.state.phase_due[i].unwrap() + (elapsed - 1) * period;
                self.state.phase_due[i] = Some(due(
                    self.state.phase_due[i].unwrap(),
                    elapsed.checked_mul(period).ok_or("phase clock overflow")?,
                )?);
                // Physical frame edges continue while an interrupt is pending.
                // Only interrupt delivery coalesces until the guest acknowledges it.
                {
                    let mut edges = self.wiring.edges.lock().unwrap();
                    let index = edges[i]
                        .map_or(Some(elapsed), |e| e.index.checked_add(elapsed))
                        .ok_or("phase frame index overflow")?;
                    edges[i] = Some(FrameEdge { index, tick });
                }
                if self.state.phase_pending[i] == 0 {
                    let edge = self.wiring.edges.lock().unwrap()[i].unwrap();
                    self.publish_phase(i, edge, cx)?;
                }
            }
        }
        if self.state.sensor_due.is_some_and(|t| now >= t) {
            let period = self.selected_period(self.options.sensor_period_ticks.unwrap())?;
            let old = self.state.sensor_due.unwrap();
            self.state.sensor_due = Some(due(
                old,
                ((now - old) / period + 1)
                    .checked_mul(period)
                    .ok_or("sensor clock overflow")?,
            )?);
            if self.frame(cx)? && self.uses_sensor_clock() {
                let edge = self
                    .wiring
                    .sensor
                    .as_ref()
                    .unwrap()
                    .frames
                    .lock()
                    .map_err(|_| "sensor frame clock poisoned")?
                    .edge
                    .unwrap();
                for (i, enabled) in self.clock_gates().into_iter().enumerate() {
                    if enabled {
                        self.publish_phase(i, edge, cx)?;
                    }
                }
            }
        }
        if self.state.dma.write(cx)? {
            self.state.capture_ack = true;
        }
        if self.state.capture_ack
            && !self.state.capture_irq_sent
            && self.state.r[0x354 / 4] & 0x10000000 != 0
        {
            self.wiring.raw_irq.raise(cx)?;
            self.state.capture_irq_sent = true;
        }
        if self.state.raw3_dma.write(cx)? {
            self.state.raw3_ack = true;
        }
        if self.state.raw3_ack
            && !self.state.raw3_irq_sent
            && self.state.r[0x370 / 4] & 0x10000000 != 0
        {
            Irq {
                core: self.wiring.raw_irq.core,
                line: self
                    .wiring
                    .raw_irq
                    .line
                    .checked_add(1)
                    .ok_or("RAW channel3 IRQ overflow")?,
            }
            .raise(cx)?;
            self.state.raw3_irq_sent = true;
        }
        if self.state.readback_due.is_some_and(|t| now >= t) {
            let mut outputs = self.wiring.memory_outputs.as_ref().unwrap().lock().unwrap();
            if !outputs.completed() {
                return Err("RAW readback EOF precedes output DMA".into());
            }
            outputs.active = false;
            outputs.active_destinations = [None; 2];
            self.state.readback_due = None;
            self.state.r[0] |= 2;
            self.wiring.readback_irq.raise(cx)?;
        }
        if self.state.stop_due.is_some_and(|t| now >= t) {
            self.state.stop_due = None;
            self.state.r[0] |= 2;
        }
        Ok(())
    }
}
impl Device for ImageInput {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        let children = self
            .wiring
            .sensor
            .as_ref()
            .map(|s| s.checkpoint())
            .transpose()?
            .into_iter()
            .collect();
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                &self.state,
                &self.cfa,
                &self.film,
                self.wiring.frame_period.is_some(),
                self.wiring.statistics_grid.is_some(),
            ),
        )?
        .with_children(children)
        .with_blobs(vec![std::sync::Arc::from(self.source.as_slice())]))
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (state, _, _, frame_period, statistics_grid): (State, Option<Cfa>, Film, bool, bool) =
            saved.decode(self.name())?;
        if frame_period != self.wiring.frame_period.is_some()
            || statistics_grid != self.wiring.statistics_grid.is_some()
            || state.r.len() != 256
            || saved.children().len() != usize::from(self.wiring.sensor.is_some())
            || saved.blobs().len() != 1
        {
            return Err("image input checkpoint wiring or register extent differs".into());
        }
        if let Some(sensor) = &self.wiring.sensor {
            sensor.validate_checkpoint(&saved.children()[0])?;
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (state, cfa, film, _, _): (State, Option<Cfa>, Film, bool, bool) =
            saved.decode(self.name())?;
        if let Some(sensor) = &mut self.wiring.sensor {
            sensor.restore_checkpoint(&saved.children()[0])?;
        }
        self.state = state;
        self.publish_grid()?;
        self.cfa = cfa;
        self.film = film;
        self.source = saved.blobs()[0].to_vec();
        Ok(())
    }
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        let State {
            r,
            reset,
            command_due,
            raw_exposure,
            command_armed,
            command_done,
            command_bytes,
            phases,
            phase_due,
            phase_pending,
            phase_period,
            sensor_due,
            capture_ack,
            capture_irq_sent,
            raw3_ack,
            raw3_irq_sent,
            readback_due,
            stop_due,
            dma,
            raw3_dma,
            raw_origins,
        } = &self.state;
        let state = serde_json::json!({"r":r,"reset":reset,"command_due":command_due,"command_armed":command_armed,
            "raw_exposure":raw_exposure,"command_done":command_done,"command_bytes":fujimod_emulator_devices::fingerprint::bytes(command_bytes),"phases":phases,
            "phase_due":phase_due,"phase_pending":phase_pending,"phase_period":phase_period,"sensor_due":sensor_due,"capture_ack":capture_ack,
            "capture_irq_sent":capture_irq_sent,"raw3_ack":raw3_ack,"raw3_irq_sent":raw3_irq_sent,
            "readback_due":readback_due,"stop_due":stop_due,"dma":dma.fingerprint(),"raw3_dma":raw3_dma.fingerprint(),"raw_origins":raw_origins});
        let sensor = self.wiring.sensor.as_ref().map(|s| s.fingerprint());
        let outputs = self
            .wiring
            .memory_outputs
            .as_ref()
            .map(|s| s.lock().unwrap().fingerprint());
        Ok(
            serde_json::json!({"state":state,"edges":*self.wiring.edges.lock().unwrap(),"sensor":sensor,
            "frame_period":self.wiring.frame_period.is_some(),"statistics_grid":self.wiring.statistics_grid.is_some(),"outputs":outputs,"cfa":self.cfa,"film":self.film,"source":fujimod_emulator_devices::fingerprint::bytes(&self.source)}),
        )
    }

    fn name(&self) -> &'static str {
        "exr2-isp-image-input"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        let Some(o) = word(cx, a, s, 0xfff10000, 0x400) else {
            return 0;
        };
        if !self.valid(o)
            || [
                0x150, 0x250, 0x4c, 0x74, 0x390, 0x388, 0x15c, 0x25c, 0x350, 0x36c, 0x3b4,
            ]
            .contains(&o)
        {
            cx.unmodeled();
            return 0;
        }
        if o == 0x160
            && (self.state.command_due.is_some()
                || self.state.r[0x154 / 4] != 0
                || !self.state.command_done)
        {
            cx.fail("active command DMA status is unmodeled");
            return 0;
        }
        self.state.r[o / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        let Some(o) = word(cx, a, s, 0xfff10000, 0x400) else {
            return;
        };
        if !self.valid(o) || v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if let Err(e) = self.store(cx, o, v as u32) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        if self.state.capture_ack
            && !self.state.capture_irq_sent
            && self.state.r[0x354 / 4] & 0x10000000 != 0
        {
            return Some(now);
        }
        if self.state.raw3_ack
            && !self.state.raw3_irq_sent
            && self.state.r[0x370 / 4] & 0x10000000 != 0
        {
            return Some(now);
        }
        if self.wiring.frame_period.is_some()
            && self
                .phase_period()
                .is_ok_and(|p| p != self.state.phase_period)
        {
            return Some(now);
        }
        let s = &self.state;
        s.reset
            .into_iter()
            .chain(s.command_due)
            .chain(s.phases.into_iter().flatten())
            .chain(s.phase_due.into_iter().flatten())
            .chain(s.sensor_due)
            .chain(s.dma.deadline())
            .chain(s.raw3_dma.deadline())
            .chain(s.readback_due)
            .chain(s.stop_due)
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.tick(cx) {
            cx.fail(e);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx)
    }
    fn save(&self) -> serde_json::Value {
        let sensor = self
            .wiring
            .sensor
            .as_ref()
            .map(|s| serde_json::to_value(&*s.frames.lock().unwrap()).unwrap());
        let outputs = self
            .wiring
            .memory_outputs
            .as_ref()
            .map(|o| serde_json::to_value(&*o.lock().unwrap()).unwrap());
        serde_json::json!((
            &self.state,
            &*self.wiring.edges.lock().unwrap(),
            sensor,
            outputs,
            self.wiring.sensor.as_ref().map(|s| s.controls()),
        ))
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let (s, e, sensor, outputs, controls): (
            State,
            [Option<FrameEdge>; 2],
            Option<fujimod_emulator_imaging::scene::SensorFrames>,
            Option<fujimod_emulator_imaging::exr2_isp::memory_outputs::MemoryOutputs>,
            Option<serde_json::Value>,
        ) = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if s.r.len() != 0x400 / 4
            || sensor.is_some() != self.wiring.sensor.is_some()
            || outputs.is_some() != self.wiring.memory_outputs.is_some()
        {
            return Err("image input checkpoint bank or wiring mismatch".into());
        }
        self.state = s;
        self.publish_grid()?;
        *self.wiring.edges.lock().unwrap() = e;
        if let Some(frames) = sensor {
            *self.wiring.sensor.as_ref().unwrap().frames.lock().unwrap() = frames;
            self.wiring
                .sensor
                .as_mut()
                .unwrap()
                .restore_controls(&controls.ok_or("missing sensor controls")?)?;
        }
        if let Some(outputs) = outputs {
            *self.wiring.memory_outputs.as_ref().unwrap().lock().unwrap() = outputs;
        }
        Ok(())
    }
}
