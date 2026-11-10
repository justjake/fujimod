//! Explicit power/reset gate around the bounded X-mount protocol part.
//!
//! Both declared supply enables and released reset must be driven high.
//! The startup delay is a board input, not measured lens timing. The ready
//! output is high while off/starting and low after startup. With a declared
//! busy duration it returns high that long after startup; without one it
//! remains low. With a declared operation duration, each operation the link
//! starts pulls a released ready output low for that long. Motion and
//! additional connector signals are unmodeled.
use fujimod_emulator_parts_io::serial::{Frame, Wire};
use super::x_mount_link::{Configuration, Link};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pins {
    pub supplies: [Option<bool>; 2],
    pub reset_released: Option<bool>,
}
impl Pins {
    fn active(self) -> bool {
        self.supplies == [Some(true); 2] && self.reset_released == Some(true)
    }
}
/// Declared durations; none is measured lens timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    /// From power and released reset until the lens serves frames and pulls ready low.
    pub startup_ticks: u64,
    /// From then until ready returns high; `None` keeps it low.
    #[serde(default)]
    pub busy_ticks: Option<u64>,
    /// How long each started operation holds a released ready output low; `None` reports none.
    #[serde(default)]
    pub operation_ticks: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Phase {
    Off,
    Starting { at: u64 },
    /// Serving frames with the ready output low.
    Ready,
    /// Serving frames with the ready output low until `until` (startup busy or an operation).
    Busy { until: u64 },
    /// Serving frames with the ready output released high.
    Released,
}
impl Phase {
    fn serving(self) -> bool {
        matches!(self, Self::Ready | Self::Busy { .. } | Self::Released)
    }
    fn ready_high(self) -> bool {
        matches!(self, Self::Off | Self::Starting { .. } | Self::Released)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    pins: Pins,
    phase: Phase,
    epoch: u32,
    event_tick: Option<u64>,
}
struct Unit {
    configuration: Configuration,
    timing: Timing,
    state: State,
    link: Link,
}
impl Unit {
    fn new(configuration: Configuration, timing: Timing) -> Result<Self, String> {
        if timing.startup_ticks == 0
            || timing.busy_ticks == Some(0)
            || timing.operation_ticks == Some(0)
        {
            return Err(
                "X-mount lens startup, busy and operation durations must be positive".into(),
            );
        }
        if timing.operation_ticks.is_some() && timing.busy_ticks.is_none() {
            return Err(
                "X-mount lens operation pulses need a ready output that returns high".into(),
            );
        }
        Ok(Self {
            link: Link::new(configuration.clone())?,
            configuration,
            timing,
            state: State {
                pins: Pins::default(),
                phase: Phase::Off,
                epoch: 0,
                event_tick: None,
            },
        })
    }
    fn next(&self, now: u64, pins: Pins) -> Option<u64> {
        if pins != self.state.pins {
            return Some(now);
        }
        match self.state.phase {
            Phase::Starting { at } => Some(at),
            Phase::Busy { until } => Some(until),
            _ => None,
        }
    }
    fn tick(&mut self, now: u64, pins: Pins) -> Result<(), String> {
        if self.next(now, pins).is_none_or(|at| at > now) {
            return Ok(());
        }
        if self.state.event_tick.is_some_and(|at| at > now) {
            return Err("X-mount lens input time moved backward".into());
        }
        if pins != self.state.pins {
            let phase = if pins.active() && !self.state.pins.active() {
                Phase::Starting {
                    at: now
                        .checked_add(self.timing.startup_ticks)
                        .ok_or("X-mount lens startup clock overflow")?,
                }
            } else if !pins.active() {
                Phase::Off
            } else {
                self.state.phase
            };
            if pins.active() != self.state.pins.active() {
                self.state.epoch = self
                    .state
                    .epoch
                    .checked_add(1)
                    .ok_or("X-mount lens reset epoch overflow")?;
                self.link.reset();
            }
            self.state.pins = pins;
            self.state.phase = phase;
            self.state.event_tick = Some(now);
        }
        if let Phase::Starting { at } = self.state.phase
            && at <= now
        {
            self.state.phase = match self.timing.busy_ticks {
                Some(busy) => Phase::Busy {
                    until: at
                        .checked_add(busy)
                        .ok_or("X-mount lens busy clock overflow")?,
                },
                None => Phase::Ready,
            };
            self.state.event_tick = Some(at);
        }
        if let Phase::Busy { until } = self.state.phase
            && until <= now
        {
            self.state.phase = Phase::Released;
            self.state.event_tick = Some(until);
        }
        Ok(())
    }
    fn validate_state(&self, state: &State) -> Result<(), String> {
        if (state.epoch == 0 && state.pins.active())
            || (state.epoch != 0 && state.event_tick.is_none())
        {
            return Err("X-mount lens power checkpoint differs from its declared inputs".into());
        }
        match state.phase {
            Phase::Off if !state.pins.active() => (),
            Phase::Starting { at }
                if state.pins.active()
                    && state
                        .event_tick
                        .and_then(|start| start.checked_add(self.timing.startup_ticks))
                        == Some(at) => {}
            Phase::Ready
                if self.timing.busy_ticks.is_none()
                    && state.pins.active()
                    && state.event_tick.is_some() => {}
            Phase::Busy { until }
                if self.timing.busy_ticks.is_some()
                    && state.pins.active()
                    && state.event_tick.is_some_and(|start| start < until) => {}
            Phase::Released
                if self.timing.busy_ticks.is_some()
                    && state.pins.active()
                    && state.event_tick.is_some() => {}
            _ => return Err("invalid X-mount lens power phase/deadline".into()),
        }
        Ok(())
    }
    fn save(&self) -> Value {
        json!({"kind":"powered-x-mount-lens","configuration":self.configuration,
            "timing":self.timing,"state":self.state,"link":self.link.save()})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State =
            serde_json::from_value(value["state"].clone()).map_err(|e| e.to_string())?;
        if value["kind"] != "powered-x-mount-lens"
            || value["configuration"] != json!(self.configuration)
            || value["timing"] != json!(self.timing)
        {
            return Err("X-mount lens power checkpoint differs from its declared inputs".into());
        }
        self.validate_state(&state)?;
        let mut link = Link::new(self.configuration.clone())?;
        link.restore(&value["link"])?;
        if !state.phase.serving() && link.frame_count() != 0 {
            return Err("inactive X-mount lens checkpoint contains serial transfers".into());
        }
        self.state = state;
        self.link = link;
        Ok(())
    }
}

/// The board's GPIO boundary and serial controller share physical part state.
#[derive(Clone)]
pub struct Port(Arc<Mutex<Unit>>);
impl Port {
    pub fn new(configuration: Configuration, timing: Timing) -> Result<Self, String> {
        Ok(Self(Arc::new(Mutex::new(Unit::new(configuration, timing)?))))
    }
    pub fn next_event_tick(&self, now: u64, pins: Pins) -> Option<u64> {
        self.0.lock().unwrap().next(now, pins)
    }
    pub fn tick(&self, now: u64, pins: Pins) -> Result<(), String> {
        self.0.lock().unwrap().tick(now, pins)
    }
    /// Powered, started and exchanging serial frames, whatever its ready output shows.
    pub fn serving(&self) -> bool {
        self.0.lock().unwrap().state.phase.serving()
    }
    pub fn ready_high(&self) -> bool {
        self.0.lock().unwrap().state.phase.ready_high()
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPower {
    configuration: Configuration,
    timing: Timing,
    state: State,
}
impl Wire for Port {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        let unit = self.0.lock().unwrap();
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            "powered-x-mount-lens",
            &SavedPower {
                configuration: unit.configuration.clone(),
                timing: unit.timing,
                state: unit.state.clone(),
            },
        )?
        .with_children(vec![unit.link.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let candidate: SavedPower = saved.decode("powered-x-mount-lens")?;
        let unit = self.0.lock().unwrap();
        if candidate.configuration != unit.configuration
            || candidate.timing != unit.timing
            || saved.children().len() != 1
            || !saved.blobs().is_empty()
        {
            return Err("X-mount lens power checkpoint differs from its declared inputs".into());
        }
        unit.validate_state(&candidate.state)?;
        unit.link.validate_checkpoint(&saved.children()[0])?;
        let mut link = Link::new(unit.configuration.clone())?;
        link.restore_checkpoint(&saved.children()[0])?;
        if !candidate.state.phase.serving() && link.frame_count() != 0 {
            return Err("inactive X-mount lens checkpoint contains serial transfers".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let candidate: SavedPower = saved.decode("powered-x-mount-lens")?;
        let mut unit = self.0.lock().unwrap();
        unit.link.restore_checkpoint(&saved.children()[0])?;
        unit.state = candidate.state;
        Ok(())
    }
    fn validate_pending(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
        token: u64,
        frame: Frame,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let candidate: SavedPower = saved.decode("powered-x-mount-lens")?;
        let unit = self.0.lock().unwrap();
        if !candidate.state.phase.serving() || token >> 32 != candidate.state.epoch as u64 {
            return Err("X-mount lens lost power/reset during a serial frame".into());
        }
        unit.link
            .validate_pending(&saved.children()[0], token & u32::MAX as u64, frame)
    }

    fn begin(&mut self, frame: Frame) -> Result<u64, String> {
        let mut unit = self.0.lock().unwrap();
        if !unit.state.phase.serving() {
            return Err("X-mount lens is not powered and ready".into());
        }
        let token = unit.link.begin(frame)?;
        if token > u32::MAX as u64 {
            return Err("X-mount lens serial token exceeds its bounded epoch".into());
        }
        Ok((unit.state.epoch as u64) << 32 | token)
    }
    fn finish(&mut self, token: u64, frame: Frame, now: u64) -> Result<u16, String> {
        let mut unit = self.0.lock().unwrap();
        if !unit.state.phase.serving() || token >> 32 != unit.state.epoch as u64 {
            return Err("X-mount lens lost power/reset during a serial frame".into());
        }
        let operations = unit.link.operations();
        let value = unit.link.finish(token & u32::MAX as u64, frame, now)?;
        if unit.link.operations() != operations
            && let Some(ticks) = unit.timing.operation_ticks
            && unit.state.phase != Phase::Ready
        {
            // The operation holds ready low from the sync frame that started it. During the
            // startup busy period the output is already low and stays so until both end.
            let end = now
                .checked_add(ticks)
                .ok_or("X-mount lens operation clock overflow")?;
            let until = match unit.state.phase {
                Phase::Busy { until } => until.max(end),
                _ => end,
            };
            unit.state.phase = Phase::Busy { until };
            unit.state.event_tick = Some(now);
        }
        Ok(value)
    }
    fn save(&self) -> Value {
        self.0.lock().unwrap().save()
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        self.0.lock().unwrap().restore(state)
    }
}

#[cfg(test)]
mod tests;
