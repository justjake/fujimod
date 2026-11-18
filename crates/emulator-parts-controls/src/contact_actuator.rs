//! A motor-driven contact cam: a shutter charge mechanism with two position
//! contacts. While the declared run drive is applied, the cam advances through
//! contact positions declared along motor run time and stops at the release
//! latch, its last position. A rising release input at the latch returns the
//! cam, after its travel time, to run time 0. Only driven pads, the release
//! input level and guest virtual time affect the part.
use fujimod_emulator_devices::parts::pins::{Pin, Pins};
use fujimod_emulator_devices::signals::Level;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    /// Motor run time from the released position at which these levels start.
    pub run_ticks: u64,
    pub contacts: [bool; 2],
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub drive: [Pin; 2],
    pub contacts: [Pin; 2],
    /// Drive code (`drive[0] | drive[1] << 1`) that runs the motor.
    pub run_drive: u8,
    /// Drive codes that hold the cam still. Any other code refuses.
    pub hold_drives: Vec<u8>,
    /// Hold drives under which a release input edge releases the latch; the
    /// other drives keep it latched.
    pub release_drives: Vec<u8>,
    /// Ascending from run time 0; the last position is the release latch.
    pub positions: Vec<Position>,
    pub initial_run_ticks: u64,
    /// Run time past the latch observed without harm before the drive stops.
    pub latch_overrun_ticks: u64,
    /// Release input edge to the return to run time 0.
    pub release_ticks: u64,
}
pub struct Wiring {
    pub pins: Box<dyn Pins>,
    pub release: Option<Level>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    drive: Option<u8>,
    release_level: bool,
    run: u64,
    /// Guest tick up to which `run` has integrated a running motor.
    since: u64,
    release_due: Option<u64>,
    transitions: u64,
}
pub struct ContactActuator {
    options: Options,
    pins: Box<dyn Pins>,
    release: Option<Level>,
    state: State,
}
impl ContactActuator {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        for pin in options.drive.into_iter().chain(options.contacts) {
            pin.validate()?;
        }
        let p = &options.positions;
        if options.drive[0] == options.drive[1]
            || options.contacts[0] == options.contacts[1]
            || options
                .drive
                .iter()
                .any(|pin| options.contacts.contains(pin))
            || options.run_drive > 3
            || options
                .hold_drives
                .iter()
                .any(|&d| d > 3 || d == options.run_drive)
            || options
                .release_drives
                .iter()
                .any(|d| !options.hold_drives.contains(d))
            || p.first().is_none_or(|first| first.run_ticks != 0)
            || p.windows(2).any(|w| w[0].run_ticks >= w[1].run_ticks)
            || options.initial_run_ticks > p.last().unwrap().run_ticks
            || options.release_ticks == 0
        {
            return Err("contact cam requires distinct pads, a run drive, ascending positions from 0 and a release time".into());
        }
        Ok(Self {
            state: State {
                drive: None,
                release_level: wiring.release.as_ref().is_some_and(Level::high),
                run: options.initial_run_ticks,
                since: 0,
                release_due: None,
                transitions: 0,
            },
            options,
            pins: wiring.pins,
            release: wiring.release,
        })
    }
    fn latch(&self) -> u64 {
        self.options.positions.last().unwrap().run_ticks
    }
    fn position(&self, run: u64) -> usize {
        self.options
            .positions
            .iter()
            .rposition(|p| p.run_ticks <= run)
            .unwrap()
    }
    fn running(&self) -> bool {
        self.state.drive == Some(self.options.run_drive)
    }
    fn drive(&self) -> Result<Option<u8>, String> {
        let [a, b] = self.options.drive;
        Ok(
            match (self.pins.output_level(a)?, self.pins.output_level(b)?) {
                (Some(a), Some(b)) => Some(u8::from(a) | u8::from(b) << 1),
                _ => None,
            },
        )
    }
    fn release_level(&self) -> bool {
        self.release.as_ref().is_some_and(Level::high)
    }
    fn set_run(&mut self, cx: &mut Cx<'_>, run: u64) -> Result<(), String> {
        let (old, new) = (self.position(self.state.run), self.position(run));
        self.state.run = run;
        if old != new {
            for (pin, level) in self
                .options
                .contacts
                .into_iter()
                .zip(self.options.positions[new].contacts)
            {
                self.pins.input(cx, pin, level)?;
            }
            self.state.transitions += 1;
        }
        Ok(())
    }
    fn tick(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let now = cx.now();
        if self.running() {
            let run = self.state.run + now.saturating_sub(self.state.since);
            self.state.since = now;
            if run > self.latch() + self.options.latch_overrun_ticks {
                return Err("contact cam motor driven past its release latch".into());
            }
            self.set_run(cx, run)?;
        }
        if self.state.release_due.is_some_and(|due| due <= now) {
            self.state.release_due = None;
            self.set_run(cx, 0)?;
        }
        let level = self.release_level();
        if level
            && !self.state.release_level
            && self
                .state
                .drive
                .is_some_and(|d| self.options.release_drives.contains(&d))
        {
            if self.state.run < self.latch() || self.state.release_due.is_some() {
                return Err("contact cam release input outside its latch".into());
            }
            self.state.release_due = Some(
                now.checked_add(self.options.release_ticks)
                    .ok_or("contact cam deadline overflow")?,
            );
        }
        self.state.release_level = level;
        let drive = self.drive()?;
        if drive != self.state.drive {
            if let Some(code) = drive
                && code != self.options.run_drive
                && !self.options.hold_drives.contains(&code)
            {
                return Err(format!(
                    "contact cam has no declared motion for drive {code:#x}"
                ));
            }
            self.state.drive = drive;
            self.state.since = now;
        }
        Ok(())
    }
    fn decoded_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<State, String> {
        let (options, state): (Options, State) = saved.decode(self.name())?;
        if options != self.options
            || state.run > self.latch() + options.latch_overrun_ticks
            || state.release_due.is_some_and(|due| due == 0)
        {
            return Err(
                "contact cam snapshot differs from its declared positions or wiring".into(),
            );
        }
        Ok(state)
    }
}
impl Device for ContactActuator {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(&self.options, &self.state),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.decoded_checkpoint(saved).map(|_| ())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.state = self.decoded_checkpoint(saved)?;
        Ok(())
    }
    fn name(&self) -> &'static str {
        "contact-actuator"
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(self.save())
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        if !self.drive().is_ok_and(|d| d == self.state.drive)
            || self.release_level() != self.state.release_level
        {
            return Some(now);
        }
        let motion = self.running().then(|| {
            let next = self
                .options
                .positions
                .get(self.position(self.state.run) + 1)
                .map_or(self.latch() + self.options.latch_overrun_ticks + 1, |p| {
                    p.run_ticks
                });
            self.state.since + next.saturating_sub(self.state.run).max(1)
        });
        motion.into_iter().chain(self.state.release_due).min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.tick(cx) {
            cx.fail(e);
        }
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"options":self.options,"state":self.state})
    }
}

#[cfg(test)]
mod tests;
