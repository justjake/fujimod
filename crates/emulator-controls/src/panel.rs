//! The control wiring: which electrical levels each control position
//! presents, including controls that share a wire.
//!
//! Each input's predicate becomes a set of allowed levels per electrical target.
//! A control pose is the intersection of its inputs' sets (asserted inputs
//! inside their bands, others outside), and controls sharing a target are
//! composed so every combination of their poses has one concrete realization.
use std::collections::{BTreeMap, BTreeSet};

use fujimod_catalog_controls::AdcInterval;
use fujimod_catalog_wiring::{LocatedControl, ScannerRecord, WireKind};
use fujimod_emulator_devices::parts::{
    pins::Pin,
    switch_panel::{Case, Circuit, Control, Signal},
};

use crate::{Calibration, graph::Nodes, mask::Mask};

/// Why one control cannot be offered; the rest of the panel still compiles.
pub struct Unresolved(pub String);

impl From<&str> for Unresolved {
    fn from(message: &str) -> Self {
        Self(message.into())
    }
}

/// One electrical contact the panel drives.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    Analog {
        node: String,
        channel: u8,
    },
    Gpio {
        node: String,
        port: u8,
        pin: u8,
    },
    GpioContacts {
        node: String,
        pins: Vec<(u8, u8)>,
    },
    SerialStatus {
        node: String,
        route: usize,
        register: u8,
        mask: u8,
    },
}

impl Target {
    /// Number of distinct levels the target can present.
    fn domain(&self, analog_levels: u32) -> u32 {
        match self {
            Self::GpioContacts { pins, .. } => 1 << pins.len(),
            Self::Analog { .. } => analog_levels,
            Self::Gpio { .. } | Self::SerialStatus { .. } => 2,
        }
    }

    fn full(&self, analog_levels: u32) -> Mask {
        Mask::full(self.domain(analog_levels))
    }

    fn signals(&self, value: u64) -> Vec<Signal> {
        match self {
            Self::GpioContacts { node, pins } => pins
                .iter()
                .enumerate()
                .map(|(index, (port, pin))| Signal::Gpio {
                    node: node.clone(),
                    pin: Pin {
                        port: *port,
                        pin: *pin,
                    },
                    high: value & (1 << index) != 0,
                })
                .collect(),
            Self::Gpio { node, port, pin } => vec![Signal::Gpio {
                node: node.clone(),
                pin: Pin {
                    port: *port,
                    pin: *pin,
                },
                high: value != 0,
            }],
            Self::SerialStatus {
                node,
                route,
                register,
                mask,
            } => vec![Signal::SerialStatus {
                node: node.clone(),
                route: *route,
                register: *register,
                mask: u16::from(*mask),
                value: if value != 0 { u16::from(*mask) } else { 0 },
            }],
            Self::Analog { node, channel } => vec![Signal::Analog {
                node: node.clone(),
                channel: usize::from(*channel),
                value: value as u16,
            }],
        }
    }
}

type Term = (Target, Mask);
/// Allowed levels of every target a predicate constrains.
pub type Clause = BTreeMap<Target, Mask>;

fn calibrated(calibration: Option<&dyn Calibration>, offset: u32) -> Result<u32, Unresolved> {
    let calibration = calibration.ok_or("ADC calibration bytes are unavailable")?;
    let bytes = calibration
        .bytes(offset, 2)
        .ok_or("ADC calibration offset escapes its input")?;
    Ok(u32::from(u16::from_le_bytes([bytes[0], bytes[1]])))
}

/// The levels an interval accepts, without the intervals it excludes.
fn interval_mask(
    interval: &AdcInterval,
    calibration: Option<&dyn Calibration>,
    analog_levels: u32,
) -> Result<Mask, Unresolved> {
    let (low, high) = match interval {
        AdcInterval::Literal { lower, upper, .. } => (u32::from(*lower), u32::from(*upper)),
        AdcInterval::ConfigurationU16 { lower, upper, .. } => {
            let low = calibrated(calibration, *lower)?;
            let high = calibrated(calibration, *upper)?;
            if low == 0 && high == 0 {
                return Err("ADC calibration band is zero-filled".into());
            }
            (low, high)
        }
    };
    if !(low <= high && high < analog_levels) {
        return Err("ADC calibration is outside the converter range".into());
    }
    let mut mask = Mask::range(low, high);
    for excluded in interval.exclusions() {
        mask = mask.difference(&interval_mask(excluded, calibration, analog_levels)?);
    }
    Ok(mask)
}

fn wire_term(
    wire: &WireKind,
    calibration: Option<&dyn Calibration>,
    nodes: &Nodes,
    analog_levels: u32,
) -> Result<Term, Unresolved> {
    match wire {
        WireKind::GpioContacts { pins, levels } => {
            let distinct: BTreeSet<_> = pins.iter().collect();
            if !(2..=8).contains(&pins.len())
                || distinct.len() != pins.len()
                || levels.len() != pins.len()
                || levels.iter().any(|level| *level > 1)
            {
                return Err("GPIO conjunction requires distinct contacts and proved levels".into());
            }
            let sample: u32 = levels
                .iter()
                .enumerate()
                .map(|(index, level)| u32::from(*level) << index)
                .sum();
            Ok((
                Target::GpioContacts {
                    node: nodes.gpio.clone(),
                    pins: pins.clone(),
                },
                Mask::single(sample),
            ))
        }
        WireKind::Gpio {
            port,
            pin,
            active_level,
        } => {
            if *active_level > 1 {
                return Err("GPIO active level is unproved".into());
            }
            Ok((
                Target::Gpio {
                    node: nodes.gpio.clone(),
                    port: *port,
                    pin: *pin,
                },
                Mask::single(u32::from(*active_level)),
            ))
        }
        WireKind::SerialStatus {
            base,
            register,
            mask,
            active_value,
        } => {
            let routes: Vec<_> = nodes
                .serial
                .iter()
                .filter(|route| {
                    route.base == *base
                        && route.readable
                        && route.status_registers.contains(register)
                })
                .collect();
            let [route] = routes.as_slice() else {
                return Err("Status bits have no unique serial status input".into());
            };
            if *active_value != 0 && *active_value != u32::from(*mask) {
                return Err("Status bits are not one contact level".into());
            }
            Ok((
                Target::SerialStatus {
                    node: route.node.clone(),
                    route: route.index,
                    register: *register,
                    mask: *mask,
                },
                Mask::single(u32::from(*active_value != 0)),
            ))
        }
        WireKind::Adc {
            channel,
            intervals,
            expected_result,
        } => {
            let mut mask = Mask::default();
            for interval in intervals {
                mask = mask.union(&interval_mask(interval, calibration, analog_levels)?);
            }
            match expected_result {
                Some(1) => {}
                Some(0) => mask = mask.xor(&Mask::full(analog_levels)),
                _ => return Err("ADC predicate result is not boolean".into()),
            }
            Ok((
                Target::Analog {
                    node: nodes.adc.clone(),
                    channel: *channel,
                },
                mask,
            ))
        }
        WireKind::GpioQuadrature { .. } | WireKind::GpioFixedCycle { .. } => Err(Unresolved(
            "This contact requires a different electrical interface: gpio_quadrature".into(),
        )),
        WireKind::GpioTransition { .. } => Err(Unresolved(
            "This contact requires a different electrical interface: gpio_transition".into(),
        )),
        WireKind::AdcTransition { .. } => Err(Unresolved(
            "This contact requires a different electrical interface: adc_transition".into(),
        )),
        WireKind::AdcConjunction { .. } => {
            Err("This contact requires a different electrical interface: adc_conjunction".into())
        }
    }
}

/// A predicate's terms and whether its active result is their negation.
fn terms(
    wire: &WireKind,
    calibration: Option<&dyn Calibration>,
    nodes: &Nodes,
    analog_levels: u32,
) -> Result<(Vec<Term>, bool), Unresolved> {
    let WireKind::AdcConjunction {
        terms,
        expected_result,
    } = wire
    else {
        return Ok((
            vec![wire_term(wire, calibration, nodes, analog_levels)?],
            false,
        ));
    };
    if !(2..=12).contains(&terms.len()) || *expected_result > 1 {
        return Err("ADC conjunction is not a checked Boolean predicate".into());
    }
    let mut result = Vec::new();
    for (channel, interval) in terms {
        if *channel >= 12 {
            return Err("ADC conjunction has an invalid term".into());
        }
        let single = WireKind::Adc {
            channel: *channel,
            intervals: vec![interval.clone()],
            expected_result: Some(1),
        };
        result.push(wire_term(&single, calibration, nodes, analog_levels)?);
    }
    let targets: BTreeSet<_> = result.iter().map(|(target, _)| target).collect();
    if targets.len() != result.len() {
        return Err("ADC conjunction repeats a channel".into());
    }
    Ok((result, *expected_result == 0))
}

/// Disjoint rectangular clauses for the conjunction of `terms`, or its
/// complete complement when inactive.
fn clauses(terms: &[Term], active: bool, analog_levels: u32) -> Vec<Clause> {
    if active {
        return vec![terms.iter().cloned().collect()];
    }
    let domains: Clause = terms
        .iter()
        .map(|(target, _)| (target.clone(), target.full(analog_levels)))
        .collect();
    let mut result = Vec::new();
    let mut prefix = Clause::new();
    for (target, mask) in terms {
        let mut clause = domains.clone();
        clause.extend(prefix.clone());
        clause.insert(target.clone(), domains[target].xor(mask));
        if clause.values().all(|mask| !mask.is_empty()) {
            result.push(clause);
        }
        prefix.insert(target.clone(), mask.clone());
    }
    result
}

/// Every nonempty pairwise intersection of two clause lists.
pub fn intersections(
    left: &[Clause],
    right: &[Clause],
    analog_levels: u32,
) -> Result<Vec<Clause>, Unresolved> {
    let mut result = Vec::new();
    for a in left {
        for b in right {
            let mut value = a.clone();
            for (target, mask) in b {
                let current = value
                    .get(target)
                    .cloned()
                    .unwrap_or_else(|| target.full(analog_levels));
                value.insert(target.clone(), current.intersection(mask));
            }
            if value.values().all(|mask| !mask.is_empty()) {
                result.push(value);
                if result.len() > 4096 {
                    return Err("Contact predicate exceeds 4096 logical alternatives".into());
                }
            }
        }
    }
    Ok(result)
}

/// One scanner record a pose must report, and the stable state it expects.
#[derive(Clone, Debug)]
pub struct Observed {
    pub record: ScannerRecord,
    pub stable_offset: Option<u32>,
    pub expected: u8,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub position: String,
    pub records: Vec<Observed>,
}

/// A control the panel offers, before composition with its wire neighbors.
struct Prepared {
    id: String,
    label: String,
    kind: String,
    positions: Vec<String>,
    masks: Vec<Vec<Clause>>,
    observations: Vec<Observation>,
    parent: String,
    momentary: bool,
}

pub struct PlanActuator {
    pub momentary: bool,
    pub states: Vec<Observation>,
}

pub struct UnavailableControl {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub reason: String,
}

pub struct Problem {
    pub control: String,
    pub reason: String,
}

pub struct Plan {
    pub controls: Vec<Control>,
    pub circuits: Vec<Circuit>,
    pub actuators: Vec<PlanActuator>,
    pub unavailable: Vec<UnavailableControl>,
    pub problems: Vec<Problem>,
}

type Widget = (String, String, String, Vec<(String, Vec<String>)>);

fn widgets(row: &LocatedControl) -> Result<(Vec<Widget>, BTreeSet<String>), Unresolved> {
    let inactive: BTreeSet<&str> = row
        .positions
        .iter()
        .filter(|position| position.active == Some(false) && !position.keys.is_empty())
        .map(|position| position.position.as_str())
        .collect();
    let (widgets, keys): (Vec<Widget>, BTreeSet<String>) = if row.kind == "shutter" {
        let names: BTreeSet<&str> = row.actions.iter().map(|(name, _)| name.as_str()).collect();
        if names != BTreeSet::from(["half_press", "full_press"]) {
            return Err("Shutter stages are unresolved".into());
        }
        let half = row.action("half_press").unwrap_or_default().to_vec();
        let full = row.action("full_press").unwrap_or_default().to_vec();
        let both: Vec<String> = half.iter().chain(&full).cloned().collect();
        let keys: BTreeSet<String> = both.iter().cloned().collect();
        if keys.len() != 2 {
            return Err("Shutter stages require two distinct contacts".into());
        }
        (
            vec![(
                row.id.clone(),
                row.name.clone(),
                "shutter".into(),
                vec![
                    ("released".into(), Vec::new()),
                    ("half".into(), half),
                    ("full".into(), both),
                ],
            )],
            keys,
        )
    } else if !row.positions.is_empty() && row.kind != "joystick" {
        let states = row
            .positions
            .iter()
            .map(|position| {
                let selected = if position.active == Some(false) {
                    Vec::new()
                } else {
                    position.keys.clone()
                };
                (position.position.clone(), selected)
            })
            .collect();
        let keys = row
            .positions
            .iter()
            .flat_map(|position| position.keys.iter().cloned())
            .collect();
        (
            vec![(row.id.clone(), row.name.clone(), row.kind.clone(), states)],
            keys,
        )
    } else if !row.actions.is_empty() {
        let single = row.actions.len() == 1;
        let widgets = row
            .actions
            .iter()
            .map(|(action, keys)| {
                let (id, label) = if single {
                    (row.id.clone(), row.name.clone())
                } else {
                    (
                        format!("{}.{action}", row.id),
                        format!("{} / {action}", row.name),
                    )
                };
                (
                    id,
                    label,
                    "button".to_owned(),
                    vec![
                        ("released".to_owned(), Vec::new()),
                        ("pressed".to_owned(), keys.clone()),
                    ],
                )
            })
            .collect();
        let keys = row
            .actions
            .iter()
            .flat_map(|(_, keys)| keys.iter().cloned())
            .collect();
        (widgets, keys)
    } else {
        return Err("No electronic action or position is established".into());
    };
    let unbound = widgets.iter().any(|(_, _, _, states)| {
        states.iter().any(|(position, selected)| {
            position != "released" && !inactive.contains(position.as_str()) && selected.is_empty()
        })
    });
    if keys.is_empty() || unbound {
        return Err("A declared action or position has no physical key".into());
    }
    Ok((widgets, keys))
}

fn prepare(
    row: &LocatedControl,
    calibration: Option<&dyn Calibration>,
    nodes: &Nodes,
    analog_levels: u32,
    stable_offset: Option<u32>,
) -> Result<Vec<Prepared>, Unresolved> {
    let (widgets, keys) = widgets(row)?;
    let mut wires: BTreeMap<String, (Vec<Term>, bool)> = BTreeMap::new();
    let mut records: BTreeMap<String, Vec<ScannerRecord>> = BTreeMap::new();
    for key in &keys {
        let sources = row
            .input
            .get(key)
            .filter(|sources| !sources.is_empty())
            .ok_or_else(|| Unresolved(format!("Physical key has no electrical source: {key}")))?;
        let candidates = sources
            .iter()
            .map(|source| terms(&source.kind, calibration, nodes, analog_levels))
            .collect::<Result<Vec<_>, _>>()?;
        if candidates
            .iter()
            .any(|candidate| *candidate != candidates[0])
        {
            return Err(Unresolved(format!(
                "Physical key requires unresolved contact composition: {key}"
            )));
        }
        for (target, _) in &candidates[0].0 {
            nodes.current(target)?;
        }
        wires.insert(key.clone(), candidates[0].clone());
        records.insert(
            key.clone(),
            sources
                .iter()
                .filter_map(|source| source.scanner_record)
                .collect(),
        );
    }
    let mut prepared = Vec::new();
    for (id, label, kind, states) in widgets {
        let momentary = kind == "button" || kind == "shutter";
        let widget_keys: BTreeSet<&String> =
            states.iter().flat_map(|(_, selected)| selected).collect();
        if momentary
            && (stable_offset.is_none() || widget_keys.iter().any(|key| records[*key].is_empty()))
        {
            return Err("Momentary input has no checked scanner acknowledgment".into());
        }
        let mut state_masks = Vec::new();
        let mut observations = Vec::new();
        for (position, selected) in &states {
            let mut masks: Vec<Clause> = vec![Clause::new()];
            let mut observed: Vec<Observed> = Vec::new();
            for key in &widget_keys {
                let (key_terms, inverted) = &wires[*key];
                let active = selected.contains(key);
                masks = intersections(
                    &masks,
                    &clauses(key_terms, active != *inverted, analog_levels),
                    analog_levels,
                )?;
                for record in &records[*key] {
                    let entry = Observed {
                        record: *record,
                        stable_offset,
                        expected: u8::from(active),
                    };
                    match observed
                        .iter_mut()
                        .find(|known| known.record.address == record.address)
                    {
                        Some(known) => *known = entry,
                        None => observed.push(entry),
                    }
                }
            }
            if masks.is_empty() {
                return Err(Unresolved(format!(
                    "Declared position has no electrical realization: {position}"
                )));
            }
            state_masks.push(masks);
            observations.push(Observation {
                position: position.clone(),
                records: observed,
            });
        }
        prepared.push(Prepared {
            id,
            label,
            kind,
            positions: states.into_iter().map(|(position, _)| position).collect(),
            masks: state_masks,
            observations,
            parent: row.id.clone(),
            momentary,
        });
    }
    Ok(prepared)
}

fn targets(control: &Prepared) -> BTreeSet<&Target> {
    control
        .masks
        .iter()
        .flatten()
        .flat_map(|clause| clause.keys())
        .collect()
}

/// Compile every control into one physical panel. Controls that cannot be
/// offered are reported; inconsistent wiring refuses the whole panel.
pub fn compile(
    controls: &[LocatedControl],
    calibration: Option<&dyn Calibration>,
    nodes: &Nodes,
    analog_levels: u32,
    stable_offset: Option<u32>,
) -> Result<Plan, String> {
    if !(2..=65536).contains(&analog_levels) {
        return Err("Analog input domain must declare 2..65536 discrete levels".into());
    }
    let mut prepared = Vec::new();
    let mut unavailable = Vec::new();
    let mut problems = Vec::new();
    for row in controls {
        match prepare(row, calibration, nodes, analog_levels, stable_offset) {
            Ok(controls) => prepared.extend(controls),
            Err(Unresolved(reason)) => {
                if !row.mechanical {
                    problems.push(Problem {
                        control: row.id.clone(),
                        reason: reason.clone(),
                    });
                }
                unavailable.push(UnavailableControl {
                    id: row.id.clone(),
                    label: row.name.clone(),
                    kind: row.kind.clone(),
                    reason,
                });
            }
        }
    }
    // Composite predicates share a finite contact domain. Differently shaped
    // domains over the same pad need an explicit composition, never two writers.
    let all: BTreeSet<&Target> = prepared.iter().flat_map(targets).collect();
    let pads = |target: &Target| -> BTreeSet<(String, u8, u8)> {
        match target {
            Target::Gpio { node, port, pin } => BTreeSet::from([(node.clone(), *port, *pin)]),
            Target::GpioContacts { node, pins } => pins
                .iter()
                .map(|(port, pin)| (node.clone(), *port, *pin))
                .collect(),
            _ => BTreeSet::new(),
        }
    };
    let ordered: Vec<&Target> = all.into_iter().collect();
    for (index, a) in ordered.iter().enumerate() {
        for b in &ordered[index + 1..] {
            if !pads(a).is_disjoint(&pads(b)) {
                return Err(
                    "Overlapping GPIO predicate domains require explicit contact composition"
                        .into(),
                );
            }
        }
    }
    // Components join controls sharing a target and actions of one manual control.
    let mut groups: Vec<Vec<Prepared>> = Vec::new();
    for control in prepared {
        let own = targets(&control)
            .into_iter()
            .cloned()
            .collect::<BTreeSet<Target>>();
        let (merging, kept): (Vec<_>, Vec<_>) = groups.into_iter().partition(|group| {
            group.iter().any(|other| {
                other.parent == control.parent
                    || targets(other).iter().any(|target| own.contains(*target))
            })
        });
        let mut group = vec![control];
        group.extend(merging.into_iter().flatten());
        groups = kept;
        groups.push(group);
    }
    let mut admitted = Vec::new();
    let mut actuators = Vec::new();
    let mut circuits = Vec::new();
    for mut group in groups {
        group.sort_by(|a, b| a.id.cmp(&b.id));
        let count: usize = group
            .iter()
            .map(|control| control.positions.len())
            .product();
        if count > 4096 {
            return Err("Contact component exceeds 4096 declared combinations".into());
        }
        let mut cases = Vec::new();
        let mut initial: Vec<Vec<String>> = Vec::new();
        let mut choice = vec![0usize; group.len()];
        'choices: loop {
            let parents: Vec<&str> = group
                .iter()
                .zip(&choice)
                .filter(|(control, index)| control.momentary && **index != 0)
                .map(|(control, _)| control.parent.as_str())
                .collect();
            let distinct: BTreeSet<&str> = parents.iter().copied().collect();
            if distinct.len() == parents.len() {
                let mut alternatives = vec![Clause::new()];
                for (control, index) in group.iter().zip(&choice) {
                    alternatives =
                        intersections(&alternatives, &control.masks[*index], analog_levels)
                            .map_err(|Unresolved(reason)| reason)?;
                }
                if !alternatives.is_empty() {
                    let pose: Vec<String> = group
                        .iter()
                        .zip(&choice)
                        .map(|(control, index)| control.positions[*index].clone())
                        .collect();
                    let mut best: Option<(usize, Vec<(Target, Mask)>)> = None;
                    for alternative in alternatives {
                        let mut misses = 0;
                        for (target, mask) in &alternative {
                            if !mask.contains(
                                nodes.current(target).map_err(|Unresolved(reason)| reason)?,
                            ) {
                                misses += 1;
                            }
                        }
                        let key = (misses, alternative.into_iter().collect::<Vec<_>>());
                        if best.as_ref().is_none_or(|best| key < *best) {
                            best = Some(key);
                        }
                    }
                    let (misses, masks) = best.expect("alternatives are nonempty");
                    let mut signals = Vec::new();
                    for (target, mask) in &masks {
                        let current = nodes.current(target).map_err(|Unresolved(reason)| reason)?;
                        let level = mask
                            .level(current, u64::from(target.domain(analog_levels)))
                            .ok_or("Contact combination has no electrical level")?;
                        signals.extend(target.signals(level));
                    }
                    if misses == 0 {
                        initial.push(pose.clone());
                    }
                    cases.push(Case {
                        positions: pose,
                        signals,
                    });
                }
            }
            // Advance the odometer, last control fastest.
            let mut place = group.len();
            loop {
                if place == 0 {
                    break 'choices;
                }
                place -= 1;
                choice[place] += 1;
                if choice[place] < group[place].positions.len() {
                    break;
                }
                choice[place] = 0;
            }
        }
        let [initial] = initial.as_slice() else {
            return Err(format!(
                "Default body levels do not identify one contact pose: {:?}",
                group
                    .iter()
                    .map(|control| control.id.as_str())
                    .collect::<Vec<_>>()
            ));
        };
        for (index, control) in group.iter().enumerate() {
            if control.positions.iter().any(|position| {
                !cases
                    .iter()
                    .any(|case: &Case| case.positions[index] == *position)
            }) {
                return Err(format!(
                    "Contact composition omits a declared position: {}",
                    control.id
                ));
            }
        }
        circuits.push(Circuit {
            controls: group.iter().map(|control| control.id.clone()).collect(),
            cases,
        });
        for (index, control) in group.into_iter().enumerate() {
            admitted.push(Control {
                id: control.id.clone(),
                label: control.label,
                kind: control.kind,
                positions: control.positions,
                initial: initial[index].clone(),
                unavailable: None,
            });
            actuators.push(PlanActuator {
                momentary: control.momentary,
                states: control.observations,
            });
        }
    }
    Ok(Plan {
        controls: admitted,
        circuits,
        actuators,
        unavailable,
        problems,
    })
}

#[cfg(test)]
mod calibration_tests {
    use super::*;

    #[test]
    fn calibrated_band_requires_loaded_bounds() {
        let interval = AdcInterval::ConfigurationU16 {
            lower: 0,
            upper: 2,
            when: None,
            unless: Vec::new(),
        };
        assert!(interval_mask(&interval, Some(&vec![0; 4]), 1024).is_err());
        assert!(interval_mask(&interval, Some(&vec![0xff; 4]), 1024).is_err());
        assert!(interval_mask(&interval, Some(&vec![0, 0, 29, 0]), 1024).is_ok());
    }
}
