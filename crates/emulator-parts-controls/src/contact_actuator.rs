//! A declared two-line actuator moves electrical contacts after settling.
//! Only driven pads, contact levels and guest virtual time affect the part.
use fujimod_emulator_devices::parts::pins::{Pin, Pins};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub drive: u8,
    pub contacts: [bool; 2],
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub drive: [Pin; 2],
    pub contacts: [Pin; 2],
    pub positions: Vec<Position>,
    pub initial_position: usize,
    pub settle_ticks: u64,
}
pub struct Wiring {
    pub pins: Box<dyn Pins>,
}
#[derive(Serialize, Deserialize)]
struct Motion {
    due: u64,
    position: usize,
}
pub struct ContactActuator {
    options: Options,
    pins: Box<dyn Pins>,
    observed: Option<u8>,
    position: usize,
    pending: Option<Motion>,
    transitions: u64,
}
type Saved = (Options, Option<u8>, usize, Option<Motion>, u64);
impl ContactActuator {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        for pin in options.drive.into_iter().chain(options.contacts) {
            pin.validate()?;
        }
        if options.drive[0] == options.drive[1]
            || options.contacts[0] == options.contacts[1]
            || options
                .drive
                .iter()
                .any(|pin| options.contacts.contains(pin))
            || options.positions.is_empty()
            || options.positions.len() > 4
            || options.initial_position >= options.positions.len()
            || options.settle_ticks == 0
            || options.positions.iter().enumerate().any(|(i, p)| {
                p.drive > 3
                    || options.positions[..i]
                        .iter()
                        .any(|other| p.drive == other.drive)
            })
        {
            return Err("contact actuator requires distinct pads, unique driven states and positive settling time".into());
        }
        Ok(Self {
            position: options.initial_position,
            options,
            pins: wiring.pins,
            observed: None,
            pending: None,
            transitions: 0,
        })
    }
    fn decoded_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<Saved, String> {
        let state: Saved = saved.decode(self.name())?;
        let (options, observed, position, pending, _) = &state;
        if options != &self.options
            || *position >= options.positions.len()
            || observed.is_some_and(|code| !options.positions.iter().any(|p| p.drive == code))
            || pending.as_ref().is_some_and(|p| {
                p.position >= options.positions.len()
                    || p.due == 0
                    || Some(options.positions[p.position].drive) != *observed
                    || options.positions[p.position].contacts
                        == options.positions[*position].contacts
            })
        {
            return Err(
                "contact actuator snapshot differs from its declared positions or wiring".into(),
            );
        }
        Ok(state)
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
    fn tick(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let drive = self.drive()?;
        if drive != self.observed {
            let position = drive
                .map(|code| {
                    self.options
                        .positions
                        .iter()
                        .position(|p| p.drive == code)
                        .ok_or_else(|| {
                            format!("contact actuator has no declared motion for drive {code:#x}")
                        })
                })
                .transpose()?;
            let pending = match position {
                Some(position)
                    if self.options.positions[position].contacts
                        != self.options.positions[self.position].contacts =>
                {
                    Some(Motion {
                        due: cx
                            .now()
                            .checked_add(self.options.settle_ticks)
                            .ok_or("contact actuator deadline overflow")?,
                        position,
                    })
                }
                _ => None,
            };
            // A new drive with the same contact levels has no future edge.
            if pending.is_none()
                && let Some(position) = position
            {
                self.position = position;
            }
            self.observed = drive;
            self.pending = pending;
        }
        if self.pending.as_ref().is_some_and(|p| p.due <= cx.now()) {
            let motion = self.pending.take().unwrap();
            for (pin, level) in self
                .options
                .contacts
                .into_iter()
                .zip(self.options.positions[motion.position].contacts)
            {
                self.pins.input(cx, pin, level)?;
            }
            self.position = motion.position;
            self.transitions += 1;
        }
        Ok(())
    }
}
impl Device for ContactActuator {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                &self.options,
                self.observed,
                self.position,
                &self.pending,
                self.transitions,
            ),
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.decoded_checkpoint(saved).map(|_| ())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (_, observed, position, pending, transitions) = self.decoded_checkpoint(saved)?;
        self.observed = observed;
        self.position = position;
        self.pending = pending;
        self.transitions = transitions;
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
        match self.drive() {
            Ok(drive) if drive == self.observed => self.pending.as_ref().map(|p| p.due),
            _ => Some(now),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.tick(cx) {
            cx.fail(e);
        }
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"options":self.options,"observed":self.observed,"position":self.position,
            "pending":self.pending,"transitions":self.transitions})
    }
}

#[cfg(test)]
mod tests;
