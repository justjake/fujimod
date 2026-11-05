//! Keys read as ladders: an ADC channel's resistor-ladder sample, or a
//! power-controller register's 10-bit ladder value.
//!
//! Every rung is declared with the band the firmware accepts, the sample the
//! model returns inside it, and the basis of both. A key without a rung has no
//! electrical description, so its edges are refused rather than approximated.
//! No firmware memory or execution state is available at this boundary.
use crate::resistor_keys::{Key, parse_script};
use fujimod_emulator_devices::parts::controls::Controls;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

const ADC_CHANNELS: u8 = 15;
const ADC_MAX: u16 = 4095;
const REGISTERS: u8 = 128;
const REGISTER_MAX: u16 = 1023;

/// Where a ladder is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Wire {
    /// One 12-bit ADC channel. The level with no key held is that ADC
    /// window's own declared sample.
    Adc { channel: u8 },
    /// One 10-bit power-controller register. `released` is its value with no
    /// key held.
    PowerControllerRegister { register: u8, released: u16 },
}

impl Wire {
    fn same_line(self, other: Self) -> bool {
        match (self, other) {
            (Self::Adc { channel: a }, Self::Adc { channel: b }) => a == b,
            (
                Self::PowerControllerRegister { register: a, .. },
                Self::PowerControllerRegister { register: b, .. },
            ) => a == b,
            _ => false,
        }
    }
    fn maximum(self) -> u16 {
        match self {
            Self::Adc { .. } => ADC_MAX,
            Self::PowerControllerRegister { .. } => REGISTER_MAX,
        }
    }
    /// Number of distinct values this model reads on the wire.
    pub fn levels(self) -> u16 {
        self.maximum() + 1
    }
}

/// One key of a ladder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rung {
    pub key: Key,
    /// Lowest value the firmware accepts as this key.
    pub band_low: u16,
    /// Highest value the firmware accepts as this key.
    pub band_high: u16,
    /// Value returned while the key is held; inside the band.
    pub sample: u16,
    /// Where the band and the sample come from (addresses, hashes, limits).
    pub basis: String,
}

/// A group of keys sharing one wire; at most one is held at a time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ladder {
    pub wire: Wire,
    /// Distinct values the body's firmware reads on the wire; must equal the
    /// model's resolution for that wire kind.
    pub levels: u16,
    pub rungs: Vec<Rung>,
}

/// Body-declared ladders.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub ladders: Vec<Ladder>,
}

impl Options {
    /// Check every ladder against the model: wire range and resolution,
    /// disjoint bands with their samples, released values outside every band,
    /// one rung per key and a basis for each.
    pub fn validate(&self) -> Result<(), String> {
        if self.ladders.is_empty() {
            return Err("ladder keys declare no ladder".into());
        }
        let mut keys = BTreeSet::new();
        for (index, ladder) in self.ladders.iter().enumerate() {
            match ladder.wire {
                Wire::Adc { channel } if channel >= ADC_CHANNELS => {
                    return Err(format!("ADC ladder channel {channel} does not exist"));
                }
                Wire::PowerControllerRegister { register, released } => {
                    if register >= REGISTERS {
                        return Err(format!("ladder register {register:#x} does not exist"));
                    }
                    if released > REGISTER_MAX {
                        return Err(format!("register {register:#x} released value exceeds 10 bits"));
                    }
                }
                _ => {}
            }
            if self.ladders[..index].iter().any(|other| other.wire.same_line(ladder.wire)) {
                return Err(format!("{:?} is declared as two ladders", ladder.wire));
            }
            if ladder.rungs.is_empty() {
                return Err(format!("{:?} declares no rung", ladder.wire));
            }
            if ladder.levels != ladder.wire.levels() {
                return Err(format!(
                    "{:?} declares {} levels; the model reads {} on this wire",
                    ladder.wire,
                    ladder.levels,
                    ladder.wire.levels()
                ));
            }
            for (position, rung) in ladder.rungs.iter().enumerate() {
                if rung.key.dial() {
                    return Err(format!(
                        "{:?} cannot be a ladder rung: dial detents are quadrature pairs",
                        rung.key
                    ));
                }
                if !keys.insert(rung.key) {
                    return Err(format!("{:?} is declared on two rungs", rung.key));
                }
                if rung.basis.trim().is_empty() {
                    return Err(format!("{:?} rung has no basis", rung.key));
                }
                let maximum = ladder.wire.maximum();
                if rung.band_low > rung.band_high
                    || rung.band_high > maximum
                    || !(rung.band_low..=rung.band_high).contains(&rung.sample)
                {
                    return Err(format!(
                        "{:?} rung needs low <= sample <= high <= {maximum}",
                        rung.key
                    ));
                }
                if let Wire::PowerControllerRegister { register, released } = ladder.wire
                    && (rung.band_low..=rung.band_high).contains(&released)
                {
                    return Err(format!(
                        "register {register:#x} released value {released} lies in the {:?} band",
                        rung.key
                    ));
                }
                if ladder.rungs[..position].iter().any(|other| {
                    rung.band_low <= other.band_high && other.band_low <= rung.band_high
                }) {
                    return Err(format!("{:?} band overlaps another rung", rung.key));
                }
            }
        }
        Ok(())
    }

    fn ladder_of(&self, key: Key) -> Option<(&Ladder, &Rung)> {
        self.ladders.iter().find_map(|ladder| {
            ladder
                .rungs
                .iter()
                .find(|rung| rung.key == key)
                .map(|rung| (ladder, rung))
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Edge {
    at: Duration,
    key: Key,
    pressed: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    held: BTreeSet<Key>,
    pending: Vec<Edge>,
    now: Duration,
    started: bool,
    used: bool,
}

/// Ladder-read keys. The CLI and a frontend enqueue the same physical edges;
/// only a due guest event applies them.
#[derive(Clone)]
pub struct LadderKeys {
    state: Arc<Mutex<State>>,
    options: Arc<Options>,
}

impl LadderKeys {
    pub fn new(options: Options) -> Result<Self, String> {
        options.validate()?;
        Ok(Self {
            state: Arc::new(Mutex::new(State::default())),
            options: Arc::new(options),
        })
    }

    /// Begin guest time at `now`; edges queued earlier become relative to it.
    pub fn start(&self, now: Duration) {
        let mut s = self.state.lock().unwrap();
        if !s.started {
            for edge in &mut s.pending {
                edge.at += now;
            }
            s.now = now;
            s.started = true;
        }
    }

    /// Queue a physical edge. A key with no rung is refused, and so is a press
    /// that would hold two keys of one wire at once: the bands of a combined
    /// press are not declared.
    pub fn enqueue(&self, at: Duration, key: Key, pressed: bool) -> Result<(), String> {
        let (ladder, _) = self.options.ladder_of(key).ok_or_else(|| {
            format!("{key:?} has no declared ladder rung; the body's wiring for it is unestablished")
        })?;
        let mut s = self.state.lock().unwrap();
        let at = if at == Duration::ZERO { s.now } else { at };
        let on_wire = |k: Key| ladder.rungs.iter().any(|rung| rung.key == k);
        let mut timeline: Vec<(Duration, Key, bool)> = s
            .pending
            .iter()
            .filter(|edge| on_wire(edge.key))
            .map(|edge| (edge.at, edge.key, edge.pressed))
            .collect();
        timeline.push((at, key, pressed));
        // Releases sort before presses at the same instant.
        timeline.sort_by_key(|(at, _, pressed)| (*at, *pressed));
        let mut held: BTreeSet<Key> = s.held.iter().copied().filter(|k| on_wire(*k)).collect();
        for (_, edge_key, edge_pressed) in timeline {
            if edge_pressed {
                if let Some(other) = held.iter().find(|other| **other != edge_key) {
                    return Err(format!(
                        "{edge_key:?} pressed while {other:?} holds the same ladder: no band is declared for both"
                    ));
                }
                held.insert(edge_key);
            } else {
                held.remove(&edge_key);
            }
        }
        s.used = true;
        s.pending.push(Edge { at, key, pressed });
        s.pending.sort_by_key(|edge| (edge.at, edge.pressed));
        Ok(())
    }

    pub fn script(&self, script: &str) -> Result<(), String> {
        self.script_at(script, Duration::ZERO)
    }

    /// Append a `t=SECONDS:KEY;t=SECONDS:-KEY` script relative to `origin`.
    pub fn script_at(&self, script: &str, origin: Duration) -> Result<(), String> {
        for (at, key, pressed) in parse_script(script, origin)? {
            self.enqueue(at, key, pressed)?;
        }
        Ok(())
    }

    pub fn used(&self) -> bool {
        self.state.lock().unwrap().used
    }

    fn held_rung(&self, s: &State, ladder: &Ladder) -> Option<Rung> {
        ladder
            .rungs
            .iter()
            .find(|rung| s.held.contains(&rung.key))
            .cloned()
    }
}

impl Controls for LadderKeys {
    fn next_deadline(&self) -> Option<u64> {
        self.state
            .lock()
            .unwrap()
            .pending
            .first()
            .map(|edge| edge.at.as_nanos().min(u64::MAX as u128) as u64)
    }
    fn advance(&self, now: u64) -> Vec<String> {
        let now = Duration::from_nanos(now);
        let mut lines = Vec::new();
        let mut s = self.state.lock().unwrap();
        s.now = now;
        while s.pending.first().is_some_and(|edge| edge.at <= now) {
            let edge = s.pending.remove(0);
            if edge.pressed {
                s.held.insert(edge.key);
            } else {
                s.held.remove(&edge.key);
            }
            lines.push(format!(
                "physical input {:?} {} at {:.3}s",
                edge.key,
                if edge.pressed { "press" } else { "release" },
                edge.at.as_secs_f64()
            ));
        }
        lines
    }
    fn sample(&self, channel: usize, fallback: u16) -> u16 {
        let s = self.state.lock().unwrap();
        self.options
            .ladders
            .iter()
            .find(|ladder| matches!(ladder.wire, Wire::Adc { channel: c } if usize::from(c) == channel))
            .and_then(|ladder| self.held_rung(&s, ladder))
            .map_or(fallback, |rung| rung.sample)
    }
    fn register_ladders(&self) -> Vec<u8> {
        let mut registers: Vec<u8> = self
            .options
            .ladders
            .iter()
            .filter_map(|ladder| match ladder.wire {
                Wire::PowerControllerRegister { register, .. } => Some(register),
                Wire::Adc { .. } => None,
            })
            .collect();
        registers.sort_unstable();
        registers
    }
    fn register_sample(&self, register: u8) -> Option<u16> {
        let s = self.state.lock().unwrap();
        self.options.ladders.iter().find_map(|ladder| match ladder.wire {
            Wire::PowerControllerRegister { register: r, released } if r == register => {
                Some(self.held_rung(&s, ladder).map_or(released, |rung| rung.sample))
            }
            _ => None,
        })
    }
    fn save(&self) -> Value {
        serde_json::to_value(&*self.state.lock().unwrap()).unwrap()
    }
    fn restore(&self, v: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(v.clone())
            .map_err(|e| format!("ladder controls checkpoint: {e}"))?;
        if state.held.iter().chain(state.pending.iter().map(|edge| &edge.key)).any(|k| self.options.ladder_of(*k).is_none()) {
            return Err("ladder controls checkpoint names a key this body has no rung for".into());
        }
        *self.state.lock().unwrap() = state;
        Ok(())
    }
}
