//! Independent focus/iris motors. Initial positions, travel limits, vanes,
//! encoder ratios, and clock scales are declared mechanical inputs.
use fujimod_emulator_devices::exr2::pulse::PulsePins;
use fujimod_emulator_devices::parts::pins::{Pin, Pins};
use fujimod_emulator_devices::{Cx, bus::InterruptLine};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Above,
    Below,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Vane {
    pub pin: Pin,
    pub threshold: i64,
    pub side: Side,
}
impl Vane {
    pub fn level(self, position: i64) -> bool {
        match self.side {
            Side::Above => position >= self.threshold,
            Side::Below => position <= self.threshold,
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CounterOutput {
    pub channel: u8,
    pub divider: u32,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TravelBoundary {
    #[default]
    Refuse,
    EndStop,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MotorOptions {
    pub initial_position: i64,
    pub travel_limits: [i64; 2],
    #[serde(default)]
    pub boundary: TravelBoundary,
    pub ticks_per_step: u64,
    pub settle_ticks: u64,
    pub home: Option<Vane>,
    pub counters: Vec<CounterOutput>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EncoderOptions {
    pub pins: [Pin; 2],
    pub steps_per_phase: u32,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) struct Ramp {
    pub periods: [u64; 5],
    pub stage_steps: u32,
    pub accelerate: bool,
    pub decelerate: bool,
}
impl Ramp {
    fn period(self, elapsed: u32, remaining: u32) -> u64 {
        let mut stage = 4;
        if self.accelerate {
            stage = stage.min(elapsed / self.stage_steps);
        }
        if self.decelerate {
            stage = stage.min(remaining.saturating_sub(1) / self.stage_steps);
        }
        if !self.accelerate && !self.decelerate {
            stage = 0;
        }
        self.periods[stage.min(4) as usize]
    }
}
/// Five register-selected microstep periods. The declared piecewise ramp
/// advances one stage per programmed microstep count, then keeps the last rate.
/// Encoder effects remain at integral 32-microstep positions, with no segment IRQ.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) struct StepClock {
    periods: [u64; 5],
    stage_microsteps: u32,
    elapsed_microsteps: u32,
}
impl StepClock {
    pub fn new(periods: [u64; 5], stage_microsteps: u32) -> Result<Self, String> {
        if periods.contains(&0) || stage_microsteps == 0 {
            return Err("banked lens ramp requires positive periods and stage length".into());
        }
        Ok(Self {
            periods,
            stage_microsteps,
            elapsed_microsteps: 0,
        })
    }
    fn period(&self) -> Result<u64, String> {
        let mut ticks = 0u64;
        for i in 0..32 {
            let index = (self
                .elapsed_microsteps
                .checked_add(i)
                .ok_or("lens microstep count overflows")?
                / self.stage_microsteps)
                .min(4) as usize;
            ticks = ticks
                .checked_add(self.periods[index])
                .ok_or("lens step period overflows")?;
        }
        Ok(ticks)
    }
    fn advance(&mut self) -> Result<u64, String> {
        self.elapsed_microsteps = self
            .elapsed_microsteps
            .checked_add(32)
            .ok_or("lens microstep count overflows")?;
        self.period()
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) struct Motion {
    remaining: u32,
    elapsed: u32,
    ramp: Option<Ramp>,
    sign: i64,
    period: u64,
    due: u64,
    clock: Option<StepClock>,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Motor {
    pub options: MotorOptions,
    pub position: i64,
    pub pending: Option<Motion>,
    queue: VecDeque<Motion>,
    remainders: Vec<u32>,
    home_level: Option<bool>,
}
pub(super) struct Mechanics {
    pub motors: [Motor; 2],
    pub pulses: PulsePins,
    pub gpio: Box<dyn Pins>,
    pub irqs: [InterruptLine; 2],
    encoder: Option<EncoderOptions>,
    encoder_phase: i64,
}
impl Mechanics {
    pub(super) fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            "lens-mechanics",
            &(&self.motors, self.encoder, self.encoder_phase),
        )
    }
    pub(super) fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (motors, _, _): ([Motor; 2], Option<EncoderOptions>, i64) =
            saved.decode("lens-mechanics")?;
        for motor in &motors {
            for motion in motor.pending.iter().chain(motor.queue.iter()) {
                if motion.clock.is_some_and(|clock| {
                    clock.stage_microsteps == 0
                        || clock.periods.contains(&0)
                        || clock.elapsed_microsteps % 32 != 0
                }) || (motion.clock.is_some() && motion.ramp.is_some())
                {
                    return Err("banked microstep snapshot has invalid stage clocks".into());
                }
                if motion.period == 0
                    || motion
                        .ramp
                        .is_some_and(|ramp| ramp.stage_steps == 0 || ramp.periods.contains(&0))
                {
                    return Err("lens ramp snapshot has a zero stage or period".into());
                }
            }
        }
        Ok(())
    }
    pub(super) fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (motors, encoder, encoder_phase) = saved.decode("lens-mechanics")?;
        self.motors = motors;
        self.encoder = encoder;
        self.encoder_phase = encoder_phase;
        Ok(())
    }

    pub(super) fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({"motors":self.motors,"encoder":self.encoder,"encoder_phase":self.encoder_phase,"irqs":self.irqs})
    }
    pub fn new(
        options: [MotorOptions; 2],
        encoder: Option<EncoderOptions>,
        pulses: PulsePins,
        gpio: Box<dyn Pins>,
        irqs: [InterruptLine; 2],
    ) -> Result<Self, String> {
        let mut channels = 0u16;
        for o in &options {
            if o.ticks_per_step == 0
                || o.settle_ticks == 0
                || o.travel_limits[0] >= o.travel_limits[1]
                || !(o.travel_limits[0]..=o.travel_limits[1]).contains(&o.initial_position)
            {
                return Err(
                    "lens motor requires finite travel, an initial position, and positive clocks"
                        .into(),
                );
            }
            if let Some(vane) = o.home {
                vane.pin.validate()?;
            }
            for counter in &o.counters {
                if !pulses.counter_wired(counter.channel)
                    || counter.divider == 0
                    || channels & (1 << counter.channel) != 0
                {
                    return Err("invalid or duplicate lens counter output".into());
                }
                channels |= 1 << counter.channel;
            }
        }
        if let Some(e) = encoder {
            for p in e.pins {
                p.validate()?;
            }
            if !pulses.quadrature_wired() || e.steps_per_phase == 0 || e.pins[0] == e.pins[1] {
                return Err("invalid lens quadrature encoder".into());
            }
        }
        let encoder_phase = encoder.map_or(0, |e| {
            options[0]
                .initial_position
                .div_euclid(i64::from(e.steps_per_phase))
        });
        if encoder.is_some() && pulses.phase() != encoder_phase.rem_euclid(4) as u8 {
            return Err("lens encoder phase contradicts the declared initial position".into());
        }
        let motors = options.map(|o| Motor {
            position: o.initial_position,
            pending: None,
            queue: VecDeque::with_capacity(16),
            remainders: vec![0; o.counters.len()],
            home_level: o.home.map(|v| v.level(o.initial_position)),
            options: o,
        });
        Ok(Self {
            motors,
            encoder,
            encoder_phase,
            pulses,
            gpio,
            irqs,
        })
    }
    pub fn initialize(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        for motor in &mut self.motors {
            if let Some(vane) = motor.options.home {
                let high = vane.level(motor.position);
                self.gpio.input(cx, vane.pin, high)?;
                motor.home_level = Some(high);
            }
        }
        if let Some(e) = self.encoder {
            let levels = [3, 2, 0, 1][self.encoder_phase.rem_euclid(4) as usize];
            for (i, pin) in e.pins.into_iter().enumerate() {
                self.gpio.input(cx, pin, levels & (1 << i) != 0)?;
            }
        }
        Ok(())
    }
    pub fn begin(
        &mut self,
        axis: usize,
        steps: u32,
        sign: i64,
        period: Option<u64>,
        now: u64,
        queue: bool,
    ) -> Result<(), String> {
        let m = &mut self.motors[axis];
        let period = period.unwrap_or(m.options.ticks_per_step);
        if period == 0 {
            return Err("lens step period must be positive".into());
        }
        let delay = if steps == 0 {
            m.options.settle_ticks
        } else {
            period
        };
        let motion = Motion {
            remaining: steps,
            elapsed: 0,
            ramp: None,
            sign,
            period,
            clock: None,
            due: now
                .checked_add(delay)
                .ok_or("lens motion deadline overflow")?,
        };
        if queue && m.pending.is_some() {
            if m.queue.len() >= 16 {
                return Err("lens segment queue is full".into());
            }
            m.queue.push_back(motion);
        } else {
            m.pending = Some(motion);
        }
        Ok(())
    }
    pub(super) fn complete_without_motion(
        &self,
        cx: &mut Cx<'_>,
        axis: usize,
    ) -> Result<(), String> {
        if self.motors[axis].pending.is_some() || !self.motors[axis].queue.is_empty() {
            return Err("zero-quantity lens request overlaps pending motion".into());
        }
        match self.irqs[axis] {
            InterruptLine::Core { core, line } => cx.raise_irq(core, line)?,
            InterruptLine::Shared(line) => cx.set_irq(line, true),
        }
        Ok(())
    }
    pub fn profile(&mut self, axis: usize, ramp: Ramp) -> Result<(), String> {
        let Some(p) = &mut self.motors[axis].pending else {
            return Ok(());
        };
        if p.remaining == 0 {
            return Ok(());
        }
        let period = ramp.period(0, p.remaining);
        p.due = (p.due - p.period)
            .checked_add(period)
            .ok_or("lens ramp deadline overflow")?;
        p.period = period;
        p.ramp = Some(ramp);
        Ok(())
    }
    pub fn begin_clocked(
        &mut self,
        axis: usize,
        steps: u32,
        sign: i64,
        clock: StepClock,
        now: u64,
    ) -> Result<(), String> {
        self.begin(axis, steps, sign, Some(clock.period()?), now, false)?;
        if let Some(pending) = &mut self.motors[axis].pending {
            pending.clock = Some(clock);
        }
        Ok(())
    }
    pub fn stop(&mut self, axis: usize) -> bool {
        let m = &mut self.motors[axis];
        m.queue.clear();
        m.pending.take().is_some()
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.motors
            .iter()
            .filter_map(|m| m.pending.map(|p| p.due))
            .min()
    }
    pub fn step(&mut self, cx: &mut Cx<'_>, at: u64) -> Result<(), String> {
        for axis in 0..2 {
            let m = &mut self.motors[axis];
            let Some(mut p) = m.pending else {
                continue;
            };
            if at < p.due {
                continue;
            }
            if p.remaining > 0 {
                let position = m
                    .position
                    .checked_add(p.sign)
                    .ok_or("lens position overflow")?;
                let position = if !(m.options.travel_limits[0]..=m.options.travel_limits[1])
                    .contains(&position)
                {
                    match m.options.boundary {
                        TravelBoundary::Refuse => {
                            return Err("lens motion exceeds declared mechanical travel".into());
                        }
                        TravelBoundary::EndStop => {
                            position.clamp(m.options.travel_limits[0], m.options.travel_limits[1])
                        }
                    }
                } else {
                    position
                };
                let moved = position != m.position;
                m.position = position;
                for (i, output) in m.options.counters.iter().enumerate().filter(|_| moved) {
                    m.remainders[i] += 1;
                    if m.remainders[i] == output.divider {
                        if self.pulses.accepts_edges(output.channel) {
                            self.pulses.counter_edges(output.channel, 1)?;
                        }
                        m.remainders[i] = 0;
                    }
                }
                if let Some(vane) = m.options.home {
                    let high = vane.level(m.position);
                    if m.home_level != Some(high) {
                        self.gpio.input(cx, vane.pin, high)?;
                        m.home_level = Some(high);
                    }
                }
                if axis == 0
                    && let Some(e) = self.encoder
                {
                    let phase = m.position.div_euclid(i64::from(e.steps_per_phase));
                    if phase != self.encoder_phase {
                        let levels = [3, 2, 0, 1][phase.rem_euclid(4) as usize];
                        for (i, pin) in e.pins.into_iter().enumerate() {
                            self.gpio.input(cx, pin, levels & (1 << i) != 0)?;
                        }
                        self.pulses
                            .quadrature_phase(cx, phase.rem_euclid(4) as u8)?;
                        self.encoder_phase = phase;
                    }
                }
                p.remaining -= 1;
                p.elapsed += 1;
            }
            if p.remaining == 0 {
                m.pending = None;
                // Encoder and vane levels are visible before end-of-motion.
                match self.irqs[axis] {
                    InterruptLine::Core { core, line } => cx.raise_irq(core, line)?,
                    InterruptLine::Shared(line) => cx.set_irq(line, true),
                }
                if let Some(mut next) = m.queue.pop_front() {
                    let delay = if next.remaining == 0 {
                        m.options.settle_ticks
                    } else {
                        next.period
                    };
                    next.due = p
                        .due
                        .checked_add(delay)
                        .ok_or("lens segment deadline overflow")?;
                    m.pending = Some(next);
                }
            } else {
                if let Some(ramp) = p.ramp {
                    p.period = ramp.period(p.elapsed, p.remaining);
                }
                if let Some(clock) = &mut p.clock {
                    p.period = clock.advance()?;
                }
                p.due = p
                    .due
                    .checked_add(p.period)
                    .ok_or("lens step deadline overflow")?;
                m.pending = Some(p);
            }
        }
        Ok(())
    }
}
