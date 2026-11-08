//! Session-owned cold reconstruction after a modeled supply-off alarm.
use crate::{Session, durable};
use fujimod_emulator_devices::exr2::wake_alarm::Supply;

/// Rebuild the same checked board with fresh processors and devices. The
/// supplied packet belongs to the external power controller and the epoch
/// counters belong to the host timeline, not to the fresh processors.
pub trait ColdBoot {
    fn build(
        &mut self,
        tick: u64,
        instructions: Vec<u64>,
        packet: [u8; 16],
    ) -> Result<Session, String>;
}

pub struct PowerCycle {
    clock: String,
    media: durable::Spec,
    logic_inputs: Vec<String>,
    builder: Box<dyn ColdBoot>,
    retained: Option<durable::Media>,
}
impl PowerCycle {
    pub fn new(clock: String, media: durable::Spec, builder: Box<dyn ColdBoot>) -> Self {
        Self {
            clock,
            media,
            logic_inputs: Vec::new(),
            builder,
            retained: None,
        }
    }
    /// Preserve the declared external input levels across loss of power.
    pub fn with_logic_inputs(mut self, inputs: Vec<String>) -> Self {
        self.logic_inputs = inputs;
        self
    }
    pub(crate) fn clear_retained(&mut self) {
        self.retained = None;
    }
}
impl Session {
    /// Install the board-specific cold constructor before the first instruction.
    pub fn configure_power_cycle(&mut self, cycle: PowerCycle) -> Result<(), String> {
        if self.power_cycle.is_some() || !self.board.epoch_is_unexecuted() {
            return Err("power cycle must be configured before execution".into());
        }
        for name in &cycle.logic_inputs {
            self.resources.logic_level(name)?;
            self.resources.wire_users(name)?;
        }
        let power = self
            .resources
            .clock(&cycle.clock)?
            .power_alarm()?
            .ok_or("clock has no power controller")?;
        if power
            .lock()
            .map_err(|_| "power alarm poisoned")?
            .packet()
            .is_none()
        {
            return Err("power controller has no persistent packet".into());
        }
        self.power_cycle = Some(cycle);
        Ok(())
    }

    /// Advance the powered-off interval, returning true when a new epoch was
    /// installed. A time limit may stop the interval before the wake deadline.
    pub(crate) fn advance_power_cycle(&mut self, until: Option<u64>) -> Result<bool, String> {
        let mut cycle = self
            .power_cycle
            .take()
            .ok_or("supply off has no cold boot constructor")?;
        let result = (|| {
            if cycle.retained.is_none() {
                cycle.retained = Some(durable::Media::capture(self, &cycle.media)?);
            }
            let power = self
                .resources
                .clock(&cycle.clock)?
                .power_alarm()?
                .ok_or("clock has no power controller")?;
            let (supply, due) = {
                let state = power.lock().map_err(|_| "power alarm poisoned")?;
                (state.supply(), state.next_deadline())
            };
            if supply != Supply::Off && supply != Supply::Wake {
                return Err("board and power supply state disagree".into());
            }
            let target = if supply == Supply::Wake {
                self.board.now()
            } else {
                match (due, until) {
                    (Some(a), Some(b)) => a.min(b),
                    (Some(a), None) => a,
                    (None, Some(b)) => b,
                    (None, None) => return Ok(false),
                }
            };
            if target < self.board.now() {
                return Err("wake boundary moved backward".into());
            }
            self.board.advance_unpowered(target)?;
            let packet = {
                let mut state = power.lock().map_err(|_| "power alarm poisoned")?;
                state.advance(target)?;
                if state.supply() != Supply::Wake {
                    return Ok(false);
                }
                state.packet().ok_or("power packet is unavailable")?
            };
            let counts = self.board.instruction_counts();
            let inputs = cycle.logic_inputs.iter().map(|name| {
                Ok((name.clone(), self.resources.logic_level(name)?.high()))
            }).collect::<Result<Vec<_>, String>>()?;
            let mut next = cycle.builder.build(target, counts.clone(), packet)?;
            if next.power_cycle.is_some()
                || next.board.now() != target
                || next.board.instruction_counts() != counts
                || !next.board.epoch_is_unexecuted()
            {
                return Err(
                    "cold constructor did not preserve the timeline and fresh processor state"
                        .into(),
                );
            }
            let next_power = next
                .resources
                .clock(&cycle.clock)?
                .power_alarm()?
                .ok_or("cold clock has no power controller")?;
            {
                let next_power = next_power.lock().map_err(|_| "power alarm poisoned")?;
                if next_power.supply() != Supply::On || next_power.packet() != Some(packet) {
                    return Err("cold constructor did not retain power-controller state".into());
                }
            }
            cycle
                .retained
                .as_ref()
                .unwrap()
                .apply(&mut next, &cycle.media)?;
            for (name, high) in inputs {
                next.logic_input(&name, high)?;
            }
            cycle.retained = None;
            *self = next;
            Ok(true)
        })();
        self.power_cycle = Some(cycle);
        result
    }
}
