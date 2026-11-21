//! Keys read as ladders: an ADC channel's resistor-ladder sample, or a
//! power-controller register's 10-bit ladder value. The same model also holds
//! a body's latched position dials, its counting and quadrature command dials,
//! its GPIO push contacts and its register-bit switches.
//!
//! Every rung is declared with the band the firmware accepts, the sample the
//! model returns inside it, and the basis of both. A wire returns the sample
//! of the whole set of contacts held on it: a single key's rung, or a declared
//! contact set (two keys together, or keys held while the position dial that
//! shares the wire closes its contact). A key without a rung, and a set of
//! contacts without a declared band, has no electrical description, so its
//! edges are refused rather than approximated. No firmware memory or execution
//! state is available at this boundary.
use crate::resistor_keys::{Key, parse_script};
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::parts::controls::Controls;
use fujimod_emulator_devices::parts::pins::{DriveInput, Pin};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

const ADC_CHANNELS: u8 = 15;
const ADC_MAX: u16 = 4095;
const REGISTERS: u8 = 128;
const REGISTER_MAX: u16 = 1023;
/// Bound of the accumulated counting-dial steps the host may queue ahead of
/// the guest's reads.
const COUNT_LIMIT: i32 = 1 << 16;

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
    fn channel(self) -> Option<u8> {
        match self {
            Self::Adc { channel } => Some(channel),
            Self::PowerControllerRegister { .. } => None,
        }
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

/// A decoded set of contacts closed together on one wire: two or more keys,
/// or keys held while the position dial sharing the wire is at a contact
/// position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactSet {
    /// The held keys, each a rung of the same ladder.
    pub keys: Vec<Key>,
    /// The contact position of the dial that shares this wire, if it is part
    /// of the set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    pub band_low: u16,
    pub band_high: u16,
    pub sample: u16,
    pub basis: String,
}

/// A group of keys sharing one wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ladder {
    pub wire: Wire,
    /// Distinct values the body's firmware reads on the wire; must equal the
    /// model's resolution for that wire kind.
    pub levels: u16,
    pub rungs: Vec<Rung>,
    /// Decoded contact sets. A set of held contacts that is neither one rung
    /// nor one of these has no declared band and is refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sets: Vec<ContactSet>,
}

/// One position of a latched dial or switch read on an ADC channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    /// The dial's own label for the position.
    pub label: String,
    /// Other accepted spellings of the label.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub band_low: u16,
    pub band_high: u16,
    /// Value returned at this position; inside the band.
    pub sample: u16,
    /// Whether the position closes a contact on a wire shared with keys: a
    /// key held at a contact position reads the contact set naming it.
    #[serde(default)]
    pub contact: bool,
}

/// A latched dial or switch read as a band of one ADC channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PositionDial {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub channel: u8,
    pub positions: Vec<Position>,
    pub basis: String,
}

/// A two-position switch read as one bit of a power-controller register.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterSwitch {
    pub name: String,
    pub register: u8,
    pub bit: u8,
    /// Position while the bit is set.
    pub set: String,
    /// Position while the bit is clear.
    pub clear: String,
    pub basis: String,
}

/// A command dial counted by a power-controller register: a signed 4-bit
/// step count the controller clears when the guest reads it. `REAR+` adds a
/// step and `REAR-` removes one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountingDial {
    pub register: u8,
    pub basis: String,
}

/// A command dial read as a two-pad quadrature phase. `pins` are phase bit 1
/// then phase bit 0. `FRONT+` steps 3, 2, 0, 1, 3 (from rest, bit 0 falls
/// first) and `FRONT-` steps 3, 1, 0, 2, 3; each edge is one detent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadratureDial {
    pub pins: [Pin; 2],
    pub rest_phase: u8,
    pub basis: String,
}

/// A push contact on one GPIO pad.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpioKey {
    pub key: Key,
    pub pin: Pin,
    /// Pad level while pressed.
    pub active_high: bool,
    pub basis: String,
}

/// Body-declared ladders and the body's other declared controls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub ladders: Vec<Ladder>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dials: Vec<PositionDial>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switches: Vec<RegisterSwitch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rear_dial: Option<CountingDial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front_dial: Option<QuadratureDial>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpio_keys: Vec<GpioKey>,
}

fn band(low: u16, high: u16, sample: u16, maximum: u16) -> bool {
    low <= high && high <= maximum && (low..=high).contains(&sample)
}

fn overlaps(a: (u16, u16), b: (u16, u16)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

impl Position {
    fn named(&self, name: &str) -> bool {
        let name = name.trim();
        if std::iter::once(&self.label)
            .chain(&self.aliases)
            .any(|label| label.eq_ignore_ascii_case(name))
        {
            return true;
        }
        matches!(
            (self.label.parse::<f64>(), name.parse::<f64>()),
            (Ok(a), Ok(b)) if (a - b).abs() < 1e-9
        )
    }
    fn holds(&self, sample: u16) -> bool {
        (self.band_low..=self.band_high).contains(&sample)
    }
}

impl PositionDial {
    fn named(&self, name: &str) -> bool {
        std::iter::once(&self.name)
            .chain(&self.aliases)
            .any(|known| known.eq_ignore_ascii_case(name.trim()))
    }
    fn position(&self, name: &str) -> Option<&Position> {
        self.positions.iter().find(|position| position.named(name))
    }
    fn at(&self, sample: u16) -> Option<&Position> {
        self.positions.iter().find(|position| position.holds(sample))
    }
    fn labels(&self) -> String {
        self.positions
            .iter()
            .map(|position| position.label.as_str())
            .collect::<Vec<_>>()
            .join("|")
    }
}

impl RegisterSwitch {
    fn position(&self, name: &str) -> Option<&str> {
        [&self.set, &self.clear]
            .into_iter()
            .find(|label| label.eq_ignore_ascii_case(name.trim()))
            .map(String::as_str)
    }
}

impl Options {
    /// Check every declaration against the model: wire range and resolution,
    /// disjoint bands with their samples, released values outside every band,
    /// one rung per key, decoded sets of known contacts, dials with disjoint
    /// positions, distinct pads and registers, and a basis for each.
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
            let maximum = ladder.wire.maximum();
            let mut bands: Vec<(u16, u16)> = Vec::new();
            for rung in &ladder.rungs {
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
                if !band(rung.band_low, rung.band_high, rung.sample, maximum) {
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
                if bands.iter().any(|other| overlaps(*other, (rung.band_low, rung.band_high))) {
                    return Err(format!("{:?} band overlaps another rung", rung.key));
                }
                bands.push((rung.band_low, rung.band_high));
            }
            let dial = ladder.wire.channel().and_then(|channel| self.dial_on(channel));
            let mut decoded = BTreeSet::new();
            for set in &ladder.sets {
                let name = format!("{:?} set {:?}+{:?}", ladder.wire, set.keys, set.position);
                let members: BTreeSet<Key> = set.keys.iter().copied().collect();
                if members.is_empty() || members.len() != set.keys.len() {
                    return Err(format!("{name} needs distinct keys"));
                }
                if let Some(key) = set
                    .keys
                    .iter()
                    .find(|key| !ladder.rungs.iter().any(|rung| rung.key == **key))
                {
                    return Err(format!("{name} names {key:?}, which has no rung on this ladder"));
                }
                match &set.position {
                    None if members.len() < 2 => {
                        return Err(format!("{name} is one key: declare it as a rung"));
                    }
                    None => {}
                    Some(label) => {
                        let contact = dial
                            .and_then(|dial| dial.positions.iter().find(|p| p.label == *label))
                            .is_some_and(|position| position.contact);
                        if !contact {
                            return Err(format!(
                                "{name} names a position that is no contact position of a dial on this wire"
                            ));
                        }
                    }
                }
                if set.basis.trim().is_empty() {
                    return Err(format!("{name} has no basis"));
                }
                if !band(set.band_low, set.band_high, set.sample, maximum) {
                    return Err(format!("{name} needs low <= sample <= high <= {maximum}"));
                }
                if let Wire::PowerControllerRegister { released, .. } = ladder.wire
                    && (set.band_low..=set.band_high).contains(&released)
                {
                    return Err(format!("{name} band holds the released value"));
                }
                if bands.iter().any(|other| overlaps(*other, (set.band_low, set.band_high))) {
                    return Err(format!("{name} band overlaps another band"));
                }
                bands.push((set.band_low, set.band_high));
                if !decoded.insert((set.keys.iter().copied().collect::<BTreeSet<_>>(), set.position.clone())) {
                    return Err(format!("{name} is declared twice"));
                }
            }
            if let Some(dial) = dial
                && let Some(position) = dial.positions.iter().find(|position| {
                    bands.iter().any(|other| overlaps(*other, (position.band_low, position.band_high)))
                })
            {
                return Err(format!(
                    "{} position {} overlaps a key band on channel {}",
                    dial.name, position.label, dial.channel
                ));
            }
        }
        let mut names = BTreeSet::new();
        let mut channels = BTreeSet::new();
        for dial in &self.dials {
            for name in std::iter::once(&dial.name).chain(&dial.aliases) {
                if name.trim().is_empty() || !names.insert(name.to_ascii_lowercase()) {
                    return Err(format!("dial name {name:?} is empty or repeats"));
                }
            }
            if dial.channel >= ADC_CHANNELS {
                return Err(format!("{} dial channel {} does not exist", dial.name, dial.channel));
            }
            if !channels.insert(dial.channel) {
                return Err(format!("ADC channel {} carries two dials", dial.channel));
            }
            if dial.positions.is_empty() || dial.basis.trim().is_empty() {
                return Err(format!("{} dial needs positions and a basis", dial.name));
            }
            let shared = self
                .ladders
                .iter()
                .any(|ladder| ladder.wire.channel() == Some(dial.channel));
            for (index, position) in dial.positions.iter().enumerate() {
                if !band(position.band_low, position.band_high, position.sample, ADC_MAX) {
                    return Err(format!(
                        "{} position {} needs low <= sample <= high <= {ADC_MAX}",
                        dial.name, position.label
                    ));
                }
                if position.contact && !shared {
                    return Err(format!(
                        "{} position {} is a contact, but no ladder shares channel {}",
                        dial.name, position.label, dial.channel
                    ));
                }
                if dial.positions[..index].iter().any(|other| {
                    overlaps(
                        (other.band_low, other.band_high),
                        (position.band_low, position.band_high),
                    ) || other.named(&position.label)
                }) {
                    return Err(format!(
                        "{} position {} overlaps or repeats another position",
                        dial.name, position.label
                    ));
                }
            }
        }
        let ladder_registers: BTreeSet<u8> = self.register_ladders().into_iter().collect();
        let mut registers = BTreeSet::new();
        for switch in &self.switches {
            if switch.name.trim().is_empty() || !names.insert(switch.name.to_ascii_lowercase()) {
                return Err(format!("switch name {:?} is empty or repeats", switch.name));
            }
            if switch.register >= REGISTERS || switch.bit >= 16 {
                return Err(format!("{} switch register or bit does not exist", switch.name));
            }
            if ladder_registers.contains(&switch.register) || !registers.insert((switch.register, switch.bit)) {
                return Err(format!("{} switch bit is driven twice", switch.name));
            }
            if switch.set.eq_ignore_ascii_case(&switch.clear) || switch.basis.trim().is_empty() {
                return Err(format!("{} switch needs two positions and a basis", switch.name));
            }
        }
        if let Some(dial) = &self.rear_dial {
            if dial.register >= REGISTERS
                || ladder_registers.contains(&dial.register)
                || self.switches.iter().any(|switch| switch.register == dial.register)
            {
                return Err(format!("counting dial register {:#x} is unusable", dial.register));
            }
            if dial.basis.trim().is_empty() {
                return Err("counting dial has no basis".into());
            }
        }
        let mut pads = BTreeSet::new();
        if let Some(dial) = &self.front_dial {
            for pin in dial.pins {
                pin.validate()?;
                if !pads.insert((pin.port, pin.pin)) {
                    return Err("quadrature dial pads repeat".into());
                }
            }
            if dial.rest_phase > 3 || dial.basis.trim().is_empty() {
                return Err("quadrature dial needs a two-bit rest phase and a basis".into());
            }
        }
        for gpio in &self.gpio_keys {
            gpio.pin.validate()?;
            if !pads.insert((gpio.pin.port, gpio.pin.pin)) {
                return Err(format!("{:?} reuses a GPIO pad", gpio.key));
            }
            if gpio.key.dial() || !keys.insert(gpio.key) {
                return Err(format!("{:?} is declared twice or is a dial detent", gpio.key));
            }
            if gpio.basis.trim().is_empty() {
                return Err(format!("{:?} GPIO key has no basis", gpio.key));
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

    fn gpio_key(&self, key: Key) -> Option<&GpioKey> {
        self.gpio_keys.iter().find(|gpio| gpio.key == key)
    }

    fn dial_on(&self, channel: u8) -> Option<&PositionDial> {
        self.dials.iter().find(|dial| dial.channel == channel)
    }

    fn dial(&self, name: &str) -> Option<&PositionDial> {
        self.dials.iter().find(|dial| dial.named(name))
    }

    fn switch(&self, name: &str) -> Option<&RegisterSwitch> {
        self.switches
            .iter()
            .find(|switch| switch.name.eq_ignore_ascii_case(name.trim()))
    }

    fn register_ladders(&self) -> Vec<u8> {
        let mut registers: Vec<u8> = self
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

    /// Whether the body declares this key in any form.
    fn knows(&self, key: Key) -> bool {
        match key {
            Key::RearPlus | Key::RearMinus => self.rear_dial.is_some(),
            Key::FrontPlus | Key::FrontMinus => self.front_dial.is_some(),
            key => self.ladder_of(key).is_some() || self.gpio_key(key).is_some(),
        }
    }

    /// The sample of a nonempty set of held keys on `ladder` while the dial
    /// sharing the wire stands at `position` (a contact position or none).
    fn resolve(&self, ladder: &Ladder, held: &BTreeSet<Key>, position: Option<&str>) -> Option<u16> {
        if position.is_none() && held.len() == 1 {
            let key = held.first()?;
            return ladder.rungs.iter().find(|rung| rung.key == *key).map(|rung| rung.sample);
        }
        ladder
            .sets
            .iter()
            .find(|set| {
                set.position.as_deref() == position
                    && set.keys.len() == held.len()
                    && set.keys.iter().all(|key| held.contains(key))
            })
            .map(|set| set.sample)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Edge {
    at: Duration,
    key: Key,
    pressed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct PositionEdge {
    at: Duration,
    dial: String,
    position: String,
}

// New fields carry `serde(default)`: checkpoints are CBOR maps, so a snapshot
// written before they existed still decodes, with no position set, no queued
// position change, no counted step and the quadrature dial at rest.
#[derive(Default, Serialize, Deserialize)]
struct State {
    held: BTreeSet<Key>,
    pending: Vec<Edge>,
    now: Duration,
    started: bool,
    used: bool,
    /// Applied dial and switch positions by dial name. A dial without an
    /// entry reads the ADC window's own sample.
    #[serde(default)]
    positions: BTreeMap<String, String>,
    #[serde(default)]
    pending_positions: Vec<PositionEdge>,
    /// Counting-dial steps not yet read by the guest.
    #[serde(default)]
    rear_count: i32,
    /// Quadrature phase; `None` is the declared rest phase.
    #[serde(default)]
    front_phase: Option<u8>,
}

/// One change on a wire's timeline, for checking that every intermediate set
/// of held contacts has a declared band.
#[derive(Clone)]
enum Change {
    Key(Key, bool),
    Position(String),
}

impl Change {
    fn order(&self) -> u8 {
        match self {
            Self::Key(_, false) => 0,
            Self::Position(_) => 1,
            Self::Key(_, true) => 2,
        }
    }
}

/// Ladder-read keys. The CLI and a frontend enqueue the same physical edges;
/// only a due guest event applies them.
#[derive(Clone)]
pub struct LadderKeys {
    state: Arc<Mutex<State>>,
    options: Arc<Options>,
    pins: Option<Arc<dyn DriveInput>>,
    /// The idle sample each ADC channel last read before any key or position,
    /// so a press queued before the first guest read can still be checked
    /// against the dial position the run started with. Host observation only.
    observed: Arc<Mutex<[Option<u16>; ADC_CHANNELS as usize]>>,
}

impl LadderKeys {
    /// Ladders and register controls only; declarations that drive GPIO pads
    /// need [`LadderKeys::with_pins`].
    pub fn new(options: Options) -> Result<Self, String> {
        Self::open(options, None)
    }

    /// Ladders plus controls that drive the board's GPIO pads.
    pub fn with_pins(options: Options, pins: Arc<dyn DriveInput>) -> Result<Self, String> {
        Self::open(options, Some(pins))
    }

    fn open(options: Options, pins: Option<Arc<dyn DriveInput>>) -> Result<Self, String> {
        options.validate()?;
        if pins.is_none() && (options.front_dial.is_some() || !options.gpio_keys.is_empty()) {
            return Err("ladder keys with GPIO controls need the board's pins".into());
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State::default())),
            options: Arc::new(options),
            pins,
            observed: Arc::new(Mutex::new([None; ADC_CHANNELS as usize])),
        })
    }

    /// Begin guest time at `now`; edges queued earlier become relative to it.
    pub fn start(&self, now: Duration) {
        let mut s = self.state.lock().unwrap();
        if !s.started {
            for edge in &mut s.pending {
                edge.at += now;
            }
            for edge in &mut s.pending_positions {
                edge.at += now;
            }
            s.now = now;
            s.started = true;
        }
    }

    /// The position a dial sharing `ladder`'s wire stands at, before any
    /// queued change: the applied position, else the position of the idle
    /// sample the channel last read.
    fn current_position(&self, s: &State, dial: &PositionDial) -> Option<String> {
        s.positions.get(&dial.name).cloned().or_else(|| {
            self.observed.lock().unwrap()[usize::from(dial.channel)]
                .and_then(|sample| dial.at(sample))
                .map(|position| position.label.clone())
        })
    }

    /// Replay `ladder`'s queued changes plus `extra` from the current state and
    /// refuse the first set of held contacts without a declared band.
    fn check_wire(&self, s: &State, ladder: &Ladder, extra: (Duration, Change)) -> Result<(), String> {
        let on_wire = |key: Key| ladder.rungs.iter().any(|rung| rung.key == key);
        let dial = ladder.wire.channel().and_then(|channel| self.options.dial_on(channel));
        let mut timeline: Vec<(Duration, Change)> = s
            .pending
            .iter()
            .filter(|edge| on_wire(edge.key))
            .map(|edge| (edge.at, Change::Key(edge.key, edge.pressed)))
            .collect();
        if let Some(dial) = dial {
            timeline.extend(
                s.pending_positions
                    .iter()
                    .filter(|edge| edge.dial == dial.name)
                    .map(|edge| (edge.at, Change::Position(edge.position.clone()))),
            );
        }
        timeline.push(extra);
        timeline.sort_by_key(|(at, change)| (*at, change.order()));
        let mut held: BTreeSet<Key> = s.held.iter().copied().filter(|key| on_wire(*key)).collect();
        let mut position = dial.and_then(|dial| self.current_position(s, dial));
        for (_, change) in timeline {
            match &change {
                Change::Key(key, true) => {
                    held.insert(*key);
                }
                Change::Key(key, false) => {
                    held.remove(key);
                }
                Change::Position(label) => position = Some(label.clone()),
            }
            if held.is_empty() {
                continue;
            }
            let contact = dial
                .zip(position.as_deref())
                .and_then(|(dial, label)| dial.positions.iter().find(|p| p.label == label))
                .filter(|position| position.contact)
                .map(|position| position.label.as_str());
            if self.options.resolve(ladder, &held, contact).is_none() {
                let at = contact.map_or(String::new(), |label| {
                    format!(" at {} {label}", dial.map_or("", |dial| dial.name.as_str()))
                });
                return Err(match change {
                    Change::Key(key, _) => {
                        let others: Vec<Key> = held.iter().copied().filter(|other| *other != key).collect();
                        format!(
                            "{key:?} pressed while {others:?} holds the same ladder{at}: no band is declared for that set"
                        )
                    }
                    Change::Position(_) => format!(
                        "position change{at} while {held:?} holds the same ladder: no band is declared for that set"
                    ),
                });
            }
        }
        Ok(())
    }

    /// Queue a physical edge. A key with no declared wiring is refused, and so
    /// is a press that would hold a set of contacts on one wire whose band is
    /// not declared. Dial detents apply on the press; their release is a no-op.
    pub fn enqueue(&self, at: Duration, key: Key, pressed: bool) -> Result<(), String> {
        if !self.options.knows(key) {
            return Err(format!(
                "{key:?} has no declared ladder rung or wiring; the body's wiring for it is unestablished"
            ));
        }
        let mut s = self.state.lock().unwrap();
        let at = if at == Duration::ZERO { s.now } else { at };
        if key.dial() && !pressed {
            return Ok(());
        }
        if let Some((ladder, _)) = self.options.ladder_of(key) {
            self.check_wire(&s, ladder, (at, Change::Key(key, pressed)))?;
        }
        s.used = true;
        s.pending.push(Edge { at, key, pressed });
        s.pending.sort_by_key(|edge| (edge.at, edge.pressed));
        Ok(())
    }

    /// Queue a dial or switch position change. Unknown dials and positions are
    /// refused, and so is a change that would leave held keys on a shared wire
    /// in a set without a declared band.
    pub fn set_position(&self, at: Duration, dial: &str, position: &str) -> Result<(), String> {
        let (name, label) = if let Some(found) = self.options.dial(dial) {
            let label = found.position(position).ok_or_else(|| {
                format!("{} has no position {position:?}; positions: {}", found.name, found.labels())
            })?;
            (found.name.clone(), label.label.clone())
        } else if let Some(switch) = self.options.switch(dial) {
            let label = switch.position(position).ok_or_else(|| {
                format!("{} has no position {position:?}; positions: {}|{}", switch.name, switch.clear, switch.set)
            })?;
            (switch.name.clone(), label.to_owned())
        } else {
            let mut known: Vec<&str> = self.options.dials.iter().map(|dial| dial.name.as_str()).collect();
            known.extend(self.options.switches.iter().map(|switch| switch.name.as_str()));
            return Err(format!("this body declares no dial {dial:?}; dials: {}", known.join(", ")));
        };
        let mut s = self.state.lock().unwrap();
        let at = if at == Duration::ZERO { s.now } else { at };
        if let Some(found) = self.options.dial(&name)
            && let Some(ladder) = self
                .options
                .ladders
                .iter()
                .find(|ladder| ladder.wire.channel() == Some(found.channel))
        {
            self.check_wire(&s, ladder, (at, Change::Position(label.clone())))?;
        }
        s.used = true;
        s.pending_positions.push(PositionEdge { at, dial: name, position: label });
        s.pending_positions.sort_by_key(|edge| edge.at);
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

    /// The idle sample of an ADC channel: the applied position of its dial,
    /// else the window's own sample.
    fn idle(&self, s: &State, channel: u8, fallback: u16) -> u16 {
        self.options
            .dial_on(channel)
            .and_then(|dial| s.positions.get(&dial.name).and_then(|label| dial.position(label)))
            .map_or(fallback, |position| position.sample)
    }

    /// The value of a ladder's wire for the held set; an undeclared set (only
    /// reachable through a position the guest's idle sample implies) reads idle.
    fn ladder_value(&self, s: &State, ladder: &Ladder, idle: u16) -> u16 {
        let held: BTreeSet<Key> = ladder
            .rungs
            .iter()
            .map(|rung| rung.key)
            .filter(|key| s.held.contains(key))
            .collect();
        if held.is_empty() {
            return idle;
        }
        let contact = ladder
            .wire
            .channel()
            .and_then(|channel| self.options.dial_on(channel))
            .and_then(|dial| dial.at(idle))
            .filter(|position| position.contact)
            .map(|position| position.label.as_str());
        self.options.resolve(ladder, &held, contact).unwrap_or(idle)
    }

    fn drive_front(&self, phase: u8) {
        if let (Some(dial), Some(pins)) = (&self.options.front_dial, &self.pins) {
            pins.set_input(dial.pins[0], phase & 2 != 0);
            pins.set_input(dial.pins[1], phase & 1 != 0);
        }
    }

    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state
            .held
            .iter()
            .chain(state.pending.iter().map(|edge| &edge.key))
            .any(|key| !self.options.knows(*key))
        {
            return Err("ladder controls checkpoint names a key this body has no rung for".into());
        }
        let known = |dial: &str, position: &str| {
            self.options.dial(dial).is_some_and(|found| found.name == dial && found.position(position).is_some())
                || self.options.switch(dial).is_some_and(|switch| switch.position(position).is_some())
        };
        if state.positions.iter().any(|(dial, position)| !known(dial, position))
            || state.pending_positions.iter().any(|edge| !known(&edge.dial, &edge.position))
        {
            return Err("ladder controls checkpoint names a position this body does not declare".into());
        }
        if state.front_phase.is_some_and(|phase| phase > 3) || state.rear_count.abs() > COUNT_LIMIT {
            return Err("ladder controls checkpoint has an invalid dial phase or count".into());
        }
        Ok(())
    }
}

/// Next quadrature phase for one detent: direction 1 is 3, 2, 0, 1, 3.
fn step(phase: u8, forward: bool) -> u8 {
    const FORWARD: [u8; 4] = [1, 3, 0, 2];
    const BACKWARD: [u8; 4] = [2, 0, 3, 1];
    if forward { FORWARD[usize::from(phase & 3)] } else { BACKWARD[usize::from(phase & 3)] }
}

impl Controls for LadderKeys {
    fn next_deadline(&self) -> Option<u64> {
        let s = self.state.lock().unwrap();
        [
            s.pending.first().map(|edge| edge.at),
            s.pending_positions.first().map(|edge| edge.at),
        ]
        .into_iter()
        .flatten()
        .min()
        .map(|at| at.as_nanos().min(u64::MAX as u128) as u64)
    }
    fn advance(&self, now: u64) -> Vec<String> {
        let now = Duration::from_nanos(now);
        let mut lines = Vec::new();
        let mut s = self.state.lock().unwrap();
        s.now = now;
        loop {
            let key_at = s.pending.first().map(|edge| edge.at).filter(|at| *at <= now);
            let position_at = s.pending_positions.first().map(|edge| edge.at).filter(|at| *at <= now);
            match (key_at, position_at) {
                (None, None) => break,
                (Some(key), Some(position)) if position < key => {}
                (Some(_), _) => {
                    let edge = s.pending.remove(0);
                    let what = match edge.key {
                        Key::RearPlus | Key::RearMinus => {
                            let delta = if edge.key == Key::RearPlus { 1 } else { -1 };
                            s.rear_count = (s.rear_count + delta).clamp(-COUNT_LIMIT, COUNT_LIMIT);
                            format!("detent (count {})", s.rear_count)
                        }
                        Key::FrontPlus | Key::FrontMinus => {
                            let rest = self.options.front_dial.as_ref().map_or(3, |dial| dial.rest_phase);
                            let phase = step(s.front_phase.unwrap_or(rest), edge.key == Key::FrontPlus);
                            s.front_phase = Some(phase);
                            self.drive_front(phase);
                            format!("detent (phase {phase})")
                        }
                        key => {
                            if edge.pressed {
                                s.held.insert(key);
                            } else {
                                s.held.remove(&key);
                            }
                            if let (Some(gpio), Some(pins)) = (self.options.gpio_key(key), &self.pins) {
                                pins.set_input(gpio.pin, edge.pressed == gpio.active_high);
                            }
                            (if edge.pressed { "press" } else { "release" }).to_owned()
                        }
                    };
                    lines.push(format!(
                        "physical input {:?} {what} at {:.3}s",
                        edge.key,
                        edge.at.as_secs_f64()
                    ));
                    continue;
                }
                (None, Some(_)) => {}
            }
            let edge = s.pending_positions.remove(0);
            lines.push(format!(
                "physical input {} {} at {:.3}s",
                edge.dial,
                edge.position,
                edge.at.as_secs_f64()
            ));
            s.positions.insert(edge.dial, edge.position);
        }
        lines
    }
    fn sample(&self, channel: usize, fallback: u16) -> u16 {
        let Ok(channel) = u8::try_from(channel) else {
            return fallback;
        };
        if let Some(observed) = self.observed.lock().unwrap().get_mut(usize::from(channel)) {
            *observed = Some(fallback);
        }
        let s = self.state.lock().unwrap();
        let idle = self.idle(&s, channel, fallback);
        self.options
            .ladders
            .iter()
            .find(|ladder| ladder.wire.channel() == Some(channel))
            .map_or(idle, |ladder| self.ladder_value(&s, ladder, idle))
    }
    fn register_ladders(&self) -> Vec<u8> {
        self.options.register_ladders()
    }
    fn register_sample(&self, register: u8) -> Option<u16> {
        let s = self.state.lock().unwrap();
        self.options.ladders.iter().find_map(|ladder| match ladder.wire {
            Wire::PowerControllerRegister { register: r, released } if r == register => {
                Some(self.ladder_value(&s, ladder, released))
            }
            _ => None,
        })
    }
    fn driven_registers(&self) -> Vec<u8> {
        let mut registers = self.options.register_ladders();
        registers.extend(self.options.switches.iter().map(|switch| switch.register));
        registers.sort_unstable();
        registers.dedup();
        registers
    }
    fn drive_register(&self, register: u8, current: u16) -> Option<u16> {
        if let Some(value) = self.register_sample(register) {
            return Some(value);
        }
        let s = self.state.lock().unwrap();
        let mut value = current;
        let mut driven = false;
        for switch in self.options.switches.iter().filter(|switch| switch.register == register) {
            if let Some(position) = s.positions.get(&switch.name) {
                let bit = 1u16 << switch.bit;
                value = if position.eq_ignore_ascii_case(&switch.set) { value | bit } else { value & !bit };
                driven = true;
            }
        }
        driven.then_some(value)
    }
    fn take_register(&self, register: u8) -> Option<u16> {
        if self.options.rear_dial.as_ref()?.register != register {
            return None;
        }
        let mut s = self.state.lock().unwrap();
        let steps = s.rear_count.clamp(-8, 7);
        s.rear_count -= steps;
        Some((steps as u16) & 0xf)
    }
    fn save(&self) -> Value {
        serde_json::to_value(&*self.state.lock().unwrap()).unwrap()
    }
    fn restore(&self, v: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(v.clone())
            .map_err(|e| format!("ladder controls checkpoint: {e}"))?;
        self.validate_state(&state)?;
        *self.state.lock().unwrap() = state;
        Ok(())
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode("physical-controls", &*self.state.lock().unwrap())
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_state(&saved.decode("physical-controls")?)
    }
    fn restore_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: State = saved.decode("physical-controls")?;
        self.validate_state(&state)?;
        *self.state.lock().unwrap() = state;
        Ok(())
    }
}
