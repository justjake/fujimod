//! A body's keys read as ladders: an ADC channel's resistor-ladder sample, or
//! a power-controller register's 10-bit ladder value, plus its latched
//! position dials, command dials, GPIO push contacts and register-bit
//! switches. The panel compiles these into switch-panel circuits.
//!
//! Every rung is declared with the band the firmware accepts, the sample the
//! wire reads inside it, and the basis of both. A wire reads the sample of the
//! whole set of contacts held on it: a single key's rung, or a declared
//! contact set (two keys together, or keys held while the position dial that
//! shares the wire closes its contact).
use crate::keys::Key;
use fujimod_emulator_devices::parts::pins::Pin;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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

/// Where an encoder's two Gray-code phase bits reach the processor, bit 1 first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuadratureOutputs {
    Gpio { pins: [Pin; 2] },
    Adc { channels: [u8; 2] },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadratureDial {
    pub outputs: QuadratureOutputs,
    pub rest_phase: u8,
    pub edge_period_ms: u32,
    /// Signed Gray-code transitions for a positive detent.
    pub edges_per_step: i32,
    pub basis: String,
}

/// How a command dial reaches the processor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandDial {
    Counting(CountingDial),
    Quadrature(QuadratureDial),
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
    pub rear_dial: Option<CommandDial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front_dial: Option<QuadratureDial>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpio_keys: Vec<GpioKey>,
    /// Holding the first key also closes the second key's contact on another wire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coupled_contacts: Vec<(Key, Key)>,
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
                        return Err(format!(
                            "register {register:#x} released value exceeds 10 bits"
                        ));
                    }
                }
                _ => {}
            }
            if self.ladders[..index]
                .iter()
                .any(|other| other.wire.same_line(ladder.wire))
            {
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
                if bands
                    .iter()
                    .any(|other| overlaps(*other, (rung.band_low, rung.band_high)))
                {
                    return Err(format!("{:?} band overlaps another rung", rung.key));
                }
                bands.push((rung.band_low, rung.band_high));
            }
            let dial = ladder
                .wire
                .channel()
                .and_then(|channel| self.dial_on(channel));
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
                    return Err(format!(
                        "{name} names {key:?}, which has no rung on this ladder"
                    ));
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
                if bands
                    .iter()
                    .any(|other| overlaps(*other, (set.band_low, set.band_high)))
                {
                    return Err(format!("{name} band overlaps another band"));
                }
                bands.push((set.band_low, set.band_high));
                if !decoded.insert((
                    set.keys.iter().copied().collect::<BTreeSet<_>>(),
                    set.position.clone(),
                )) {
                    return Err(format!("{name} is declared twice"));
                }
            }
            if let Some(dial) = dial
                && let Some(position) = dial.positions.iter().find(|position| {
                    bands
                        .iter()
                        .any(|other| overlaps(*other, (position.band_low, position.band_high)))
                })
            {
                return Err(format!(
                    "{} position {} overlaps a key band on channel {}",
                    dial.name, position.label, dial.channel
                ));
            }
        }
        let mut links = BTreeSet::new();
        for &(key, contact) in &self.coupled_contacts {
            let wire = |key| {
                self.ladders
                    .iter()
                    .find(|ladder| ladder.rungs.iter().any(|rung| rung.key == key))
                    .map(|ladder| ladder.wire)
            };
            if !matches!((wire(key), wire(contact)), (Some(a), Some(b)) if a != b)
                || !links.insert((key, contact))
                || self
                    .coupled_contacts
                    .iter()
                    .any(|&(source, _)| source == contact)
            {
                return Err(
                    "coupled contacts must link distinct ladder wires without chaining".into(),
                );
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
                return Err(format!(
                    "{} dial channel {} does not exist",
                    dial.name, dial.channel
                ));
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
                if !band(
                    position.band_low,
                    position.band_high,
                    position.sample,
                    ADC_MAX,
                ) {
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
                return Err(format!(
                    "{} switch register or bit does not exist",
                    switch.name
                ));
            }
            if ladder_registers.contains(&switch.register)
                || !registers.insert((switch.register, switch.bit))
            {
                return Err(format!("{} switch bit is driven twice", switch.name));
            }
            if switch.set.eq_ignore_ascii_case(&switch.clear) || switch.basis.trim().is_empty() {
                return Err(format!(
                    "{} switch needs two positions and a basis",
                    switch.name
                ));
            }
        }
        if let Some(CommandDial::Counting(dial)) = &self.rear_dial {
            if dial.register >= REGISTERS
                || ladder_registers.contains(&dial.register)
                || self
                    .switches
                    .iter()
                    .any(|switch| switch.register == dial.register)
            {
                return Err(format!(
                    "counting dial register {:#x} is unusable",
                    dial.register
                ));
            }
            if dial.basis.trim().is_empty() {
                return Err("counting dial has no basis".into());
            }
        }
        let mut pads = BTreeSet::new();
        let quadrature = [
            match &self.rear_dial {
                Some(CommandDial::Quadrature(dial)) => Some(dial),
                _ => None,
            },
            self.front_dial.as_ref(),
        ];
        channels.extend(
            self.ladders
                .iter()
                .filter_map(|ladder| ladder.wire.channel()),
        );
        for dial in quadrature.into_iter().flatten() {
            match dial.outputs {
                QuadratureOutputs::Gpio { pins } => {
                    for pin in pins {
                        pin.validate()?;
                        if !pads.insert((pin.port, pin.pin)) {
                            return Err("quadrature dial pads repeat".into());
                        }
                    }
                }
                QuadratureOutputs::Adc { channels: outputs } => {
                    for channel in outputs {
                        if channel >= ADC_CHANNELS || !channels.insert(channel) {
                            return Err(
                                "quadrature ADC channel is invalid or already driven".into()
                            );
                        }
                    }
                }
            }
            if dial.rest_phase > 3
                || dial.edge_period_ms == 0
                || dial.edges_per_step == 0
                || dial.edges_per_step.unsigned_abs() > 64
                || dial.basis.trim().is_empty()
            {
                return Err("quadrature dial needs valid phase timing, detents and a basis".into());
            }
        }
        for gpio in &self.gpio_keys {
            gpio.pin.validate()?;
            if !pads.insert((gpio.pin.port, gpio.pin.pin)) {
                return Err(format!("{:?} reuses a GPIO pad", gpio.key));
            }
            if gpio.key.dial() || !keys.insert(gpio.key) {
                return Err(format!(
                    "{:?} is declared twice or is a dial detent",
                    gpio.key
                ));
            }
            if gpio.basis.trim().is_empty() {
                return Err(format!("{:?} GPIO key has no basis", gpio.key));
            }
        }
        Ok(())
    }

    fn dial_on(&self, channel: u8) -> Option<&PositionDial> {
        self.dials.iter().find(|dial| dial.channel == channel)
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
}
