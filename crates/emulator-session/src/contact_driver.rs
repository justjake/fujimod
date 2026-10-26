//! Host input automation over physical contacts and passive RAM observations.
//! This is not a peripheral: firmware checks are generated host postconditions.
//! Only the ordinary switch panel drives pins/voltages. No guest memory is written.
use super::Session;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub address: u64,
    pub bytes: Vec<u8>,
    /// Optional physical RAM address of a little-endian 32-bit base pointer.
    pub pointer: Option<u64>,
}
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pose {
    pub position: String,
    pub ready: Vec<Check>,
    pub expected: Vec<Check>,
}
/// One host-visible control in the session's input catalog.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct ControlInput {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub supported: bool,
    /// Why an unsupported control is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    pub unavailable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    pub positions: Option<Vec<ControlChoice>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    pub initial: Option<String>,
    /// Rotary controls accept relative steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    pub relative: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    pub width: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    pub height: Option<u16>,
    /// Shutter stages after the released pose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    pub stages: Option<Vec<ControlChoice>>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct ControlChoice {
    pub id: String,
    pub label: String,
}
/// Host input queue progress.
#[derive(Clone, Debug, serde::Serialize)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct QueueState {
    pub pending: usize,
    pub active: Option<ActiveTransition>,
    #[tsify(type = "Record<string, string>")]
    pub requested: BTreeMap<String, String>,
    pub completed: u64,
}
#[derive(Clone, Debug, serde::Serialize)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct ActiveTransition {
    pub id: String,
    pub position: String,
    /// Guest tick as a decimal string.
    pub started: String,
}
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Actuator {
    pub id: String,
    pub momentary: bool,
    /// Ordered contact stages; the first is the released pose for a button.
    pub states: Vec<Pose>,
}
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub poll_ticks: u64,
    pub max_phase_ticks: u64,
    pub catalog: Vec<ControlInput>,
    pub actuators: Vec<Actuator>,
    #[serde(default)]
    pub interfaces: Vec<Interface>,
}
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Interface {
    Rotary {
        id: String,
        node: String,
        cycles_per_step: i32,
    },
    Touch {
        id: String,
        node: String,
        width: u16,
        height: u16,
        offsets: [i16; 2],
    },
}
impl Interface {
    pub fn id(&self) -> &str {
        match self {
            Self::Rotary { id, .. } | Self::Touch { id, .. } => id,
        }
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Transition {
    id: String,
    position: String,
    started: u64,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Driver {
    pub spec: Spec,
    pending: VecDeque<Transition>,
    active: Option<Transition>,
    requested: BTreeMap<String, String>,
    completed: u64,
}
impl Driver {
    pub(crate) fn validate_checkpoint(&self, saved: &Self, now: u64) -> Result<(), String> {
        if self.spec != saved.spec
            || saved.pending.len() + usize::from(saved.active.is_some()) + saved.requested.len()
                > 64
        {
            return Err("host input snapshot policy or queue differs".into());
        }
        for transition in saved.active.iter().chain(&saved.pending) {
            if transition.started > now {
                return Err("host input snapshot starts after its board clock".into());
            }
            saved.pose(transition)?;
        }
        for (id, position) in &saved.requested {
            if !saved
                .actuator(id)?
                .states
                .iter()
                .any(|s| &s.position == position)
            {
                return Err("host input snapshot requested pose is absent".into());
            }
        }
        Ok(())
    }
    pub fn new(session: &Session, spec: Spec) -> Result<Self, String> {
        let panel = session
            .controls
            .as_ref()
            .ok_or("physical contacts are not connected")?;
        if spec.poll_ticks == 0
            || spec.max_phase_ticks < spec.poll_ticks
            || spec.catalog.len() > 256
        {
            return Err("invalid host contact sampling policy".into());
        }
        let mut names = BTreeSet::new();
        for actuator in &spec.actuators {
            let control = panel
                .spec()
                .controls
                .iter()
                .find(|c| c.id == actuator.id)
                .ok_or("contact observer names an absent control")?;
            if !names.insert(actuator.id.as_str())
                || actuator.states.len() != control.positions.len()
                || actuator
                    .states
                    .iter()
                    .zip(&control.positions)
                    .any(|(s, p)| s.position != *p)
                || actuator.momentary && actuator.states.len() < 2
            {
                return Err("contact observer poses differ from physical contacts".into());
            }
            for state in &actuator.states {
                if actuator.momentary && (state.ready.is_empty() || state.expected.is_empty()) {
                    return Err(
                        "momentary contact requires checked acknowledgment observations".into(),
                    );
                }
                if state.ready.len() + state.expected.len() > 128 {
                    return Err("contact observation exceeds its bound".into());
                }
                for check in state.ready.iter().chain(&state.expected) {
                    if check.bytes.is_empty() || check.bytes.len() > 64 {
                        return Err("contact observation requires 1..64 RAM bytes".into());
                    }
                    session.board.read_physical(
                        check.pointer.unwrap_or(check.address),
                        &mut vec![
                            0;
                            if check.pointer.is_some() {
                                4
                            } else {
                                check.bytes.len()
                            }
                        ],
                    )?;
                }
            }
        }
        if names.len() != panel.spec().controls.len() {
            return Err("contact observer omits a physical control".into());
        }
        let mut interface_names = BTreeSet::new();
        for interface in &spec.interfaces {
            if !interface_names.insert(interface.id()) {
                return Err("duplicate physical interface".into());
            }
            if names.contains(interface.id())
                && (!matches!(interface, Interface::Rotary { .. })
                    || !spec
                        .actuators
                        .iter()
                        .any(|a| a.id == interface.id() && a.momentary && a.states.len() == 2))
            {
                return Err("only a rotary interface may share a momentary push contact".into());
            }
            names.insert(interface.id());
            match interface {
                Interface::Rotary {
                    node,
                    cycles_per_step,
                    ..
                } => {
                    if !matches!(cycles_per_step, -1 | 1) {
                        return Err("invalid rotary cycle direction".into());
                    }
                    session.resources.encoder(node)?;
                }
                Interface::Touch {
                    node,
                    width,
                    height,
                    ..
                } => {
                    if *width == 0 || *height == 0 || *width > 4096 || *height > 4096 {
                        return Err("invalid touch coordinate domain".into());
                    }
                    session.admit_touch(node, &[])?;
                }
            }
        }
        let mut ids = BTreeSet::new();
        for input in &spec.catalog {
            let id = input.id.as_str();
            if !ids.insert(id) || input.supported && !names.contains(id) {
                return Err("catalog identities differ from physical contacts".into());
            }
        }
        if names.iter().any(|id| !ids.contains(*id)) {
            return Err("catalog omits a physical control".into());
        }
        Ok(Self {
            spec,
            pending: VecDeque::new(),
            active: None,
            requested: BTreeMap::new(),
            completed: 0,
        })
    }
    fn actuator(&self, id: &str) -> Result<&Actuator, String> {
        self.spec
            .actuators
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("physical control is unavailable: {id}"))
    }
    pub fn persistent_positions(&self, session: &Session) -> BTreeMap<String, String> {
        session
            .controls
            .as_ref()
            .map(|panel| {
                panel
                    .positions()
                    .iter()
                    .filter(|(id, _)| {
                        self.spec
                            .actuators
                            .iter()
                            .any(|a| a.id == **id && !a.momentary)
                    })
                    .map(|(id, p)| (id.clone(), p.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn boot_position(&self, id: &str, stage: Option<&str>) -> Result<String, String> {
        let actuator = self.actuator(id)?;
        if !actuator.momentary {
            return Err("boot hold requires a momentary control or explicit position".into());
        }
        let state = if let Some(stage) = stage {
            actuator
                .states
                .iter()
                .skip(1)
                .find(|s| s.position == stage)
                .ok_or("unknown boot press stage")?
        } else {
            &actuator.states[1]
        };
        Ok(state.position.clone())
    }
    /// Neutral physical pose for a power-on hold; no running scanner is required.
    pub fn boot_release_position(&self, id: &str) -> Result<String, String> {
        let actuator = self.actuator(id)?;
        if !actuator.momentary {
            return Err("boot release requires a momentary control".into());
        }
        Ok(actuator.states[0].position.clone())
    }
    pub fn busy(&self) -> bool {
        self.active.is_some() || !self.pending.is_empty()
    }
    pub fn owns(&self, id: &str) -> bool {
        self.active.as_ref().is_some_and(|t| t.id == id) || self.pending.iter().any(|t| t.id == id)
    }
    fn enqueue(
        &mut self,
        session: &Session,
        id: &str,
        stage: Option<&str>,
        release: bool,
    ) -> Result<(), String> {
        let actuator = self.actuator(id)?;
        if !actuator.momentary {
            return Err("use set_position for a persistent control".into());
        }
        let target = if release {
            0
        } else if let Some(stage) = stage {
            actuator
                .states
                .iter()
                .position(|s| s.position == stage)
                .filter(|n| *n != 0)
                .ok_or("unknown physical press stage")?
        } else {
            1
        };
        let desired = actuator.states[target].position.clone();
        let mut projected = session
            .controls
            .clone()
            .ok_or("physical contacts are not connected")?;
        for transition in &self.pending {
            projected = projected.transition(&transition.id, &transition.position)?;
        }
        let current = actuator
            .states
            .iter()
            .position(|s| projected.positions().get(id) == Some(&s.position))
            .ok_or("physical contact pose is unknown")?;
        if current == target {
            return Ok(());
        }
        let positions: Vec<_> = if target > current {
            actuator.states[current + 1..=target]
                .iter()
                .map(|s| s.position.clone())
                .collect()
        } else {
            vec![desired.clone()]
        };
        let mut requested = self.requested.clone();
        if release {
            requested.remove(id);
        } else {
            requested.insert(id.into(), desired);
        }
        // Each accepted held control retains a release slot.
        if self.pending.len()
            + usize::from(self.active.is_some())
            + positions.len()
            + requested.len()
            > 64
        {
            return Err("physical contact queue is full; wait for accepted inputs".into());
        }
        for position in &positions {
            projected = projected.transition(id, position)?;
        }
        let now = session.board.now();
        for position in positions {
            self.pending.push_back(Transition {
                id: id.into(),
                position,
                started: now,
            });
        }
        self.requested = requested;
        Ok(())
    }
    fn matches(session: &Session, checks: &[Check]) -> Result<bool, String> {
        for check in checks {
            let mut bytes = vec![0; check.bytes.len()];
            let address = if let Some(pointer) = check.pointer {
                let mut base = [0; 4];
                session.board.read_physical(pointer, &mut base)?;
                let Some(address) = u64::from(u32::from_le_bytes(base)).checked_add(check.address)
                else {
                    return Ok(false);
                };
                address
            } else {
                check.address
            };
            if let Err(error) = session.board.read_physical(address, &mut bytes) {
                if check.pointer.is_some() {
                    return Ok(false);
                }
                return Err(error);
            }
            if bytes != check.bytes {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn pose(&self, transition: &Transition) -> Result<&Pose, String> {
        self.actuator(&transition.id)?
            .states
            .iter()
            .find(|s| s.position == transition.position)
            .ok_or_else(|| "queued contact pose is absent".to_string())
    }
    fn pump(&mut self, session: &mut Session) -> Result<(), String> {
        let now = session.board.now();
        if let Some(active) = &self.active {
            let pose = self.pose(active)?;
            if !Self::matches(session, &pose.ready)? || !Self::matches(session, &pose.expected)? {
                if now.saturating_sub(active.started) > self.spec.max_phase_ticks {
                    return Err(format!(
                        "physical contact acknowledgment timed out: {} {}",
                        active.id, active.position
                    ));
                }
                return Ok(());
            }
            self.completed += 1;
            self.active = None;
        }
        if let Some(next) = self.pending.front() {
            if !Self::matches(session, &self.pose(next)?.ready)? {
                if now.saturating_sub(next.started) > self.spec.max_phase_ticks {
                    return Err(format!(
                        "physical contact scanner is not ready: {}",
                        next.id
                    ));
                }
                return Ok(());
            }
            session.apply_contact_position(&next.id, &next.position)?;
            let mut next = self.pending.pop_front().unwrap();
            next.started = now;
            self.active = Some(next);
        }
        Ok(())
    }
    pub fn state(&self) -> QueueState {
        QueueState {
            pending: self.pending.len(),
            active: self.active.as_ref().map(|a| ActiveTransition {
                id: a.id.clone(),
                position: a.position.clone(),
                started: a.started.to_string(),
            }),
            requested: self.requested.clone(),
            completed: self.completed,
        }
    }
    pub fn fingerprint(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|e| e.to_string())
    }
}
impl Session {
    pub fn admit_contact_input(
        &self,
        id: &str,
        stage: Option<&str>,
        release: bool,
    ) -> Result<(), String> {
        self.contact_driver
            .as_ref()
            .ok_or("physical input automation is not configured")?
            .clone()
            .enqueue(self, id, stage, release)
    }
    pub fn configure_contact_driver(&mut self, spec: Spec) -> Result<(), String> {
        if self.contact_driver.is_some() {
            return Err("contact driver is already configured".into());
        }
        self.contact_driver = Some(Driver::new(self, spec)?);
        Ok(())
    }
    pub fn contact_input(
        &mut self,
        id: &str,
        stage: Option<&str>,
        release: bool,
    ) -> Result<(), String> {
        let mut driver = self
            .contact_driver
            .take()
            .ok_or("physical input automation is not configured")?;
        let result = driver
            .enqueue(self, id, stage, release)
            .and_then(|()| driver.pump(self));
        self.contact_driver = Some(driver);
        result
    }
    pub fn pump_contacts(&mut self) -> Result<(), String> {
        if let Some(mut driver) = self.contact_driver.take() {
            let result = driver.pump(self);
            self.contact_driver = Some(driver);
            result?;
        }
        Ok(())
    }
    /// Reuse the runtime scheduler, sampling host acknowledgment conditions at
    /// explicit guest-time boundaries. An explicit pause_after retains its
    /// existing single-call scheduling contract.
    pub fn run_with_limits(
        &mut self,
        count: u64,
        quantum: usize,
        pause_after: Option<u64>,
        until: Option<u64>,
    ) -> Result<(), String> {
        self.pump_contacts()?;
        if count == 0
            || pause_after.is_some()
            || !self.contact_driver.as_ref().is_some_and(Driver::busy)
        {
            self.board
                .run_with_limits(count, quantum, pause_after, until)?;
            return self.pump_contacts();
        }
        let total = |s: &Session| {
            s.board
                .instruction_counts()
                .into_iter()
                .try_fold(0u64, |a, b| a.checked_add(b))
                .ok_or("instruction count overflow")
        };
        let end = total(self)?
            .checked_add(count)
            .ok_or("instruction budget overflow")?;
        loop {
            let current = total(self)?;
            if current >= end || until.is_some_and(|t| self.board.now() >= t) {
                return Ok(());
            }
            let pending = self.contact_driver.as_ref().filter(|d| d.busy());
            let horizon = match pending {
                Some(driver) => Some(
                    self.board
                        .now()
                        .saturating_add(driver.spec.poll_ticks)
                        .min(until.unwrap_or(u64::MAX)),
                ),
                None => until,
            };
            let before = (self.board.now(), current);
            self.board
                .run_with_limits(end - current, quantum, None, horizon)?;
            self.pump_contacts()?;
            if (self.board.now(), total(self)?) == before
                || horizon.is_none_or(|h| self.board.now() < h)
            {
                return Ok(());
            }
        }
    }
}
