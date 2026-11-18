//! A running camera's physical controls, built from the catalog.
//!
//! The firmware sees controls only as electrical inputs: GPIO levels, ADC
//! voltages and serial status bits. Two things let a host operate them:
//!
//! - The wiring: the electrical levels every control position presents,
//!   composed across controls that share a wire. The session holds it as
//!   its `SwitchPanel`.
//! - The input confirmation: for every position, the firmware key scanner
//!   records that must read ready before it changes and must report it
//!   afterwards. The session holds it as its `contact_driver`.
//!
//! Both come from the catalog and the session's device graph; ADC thresholds
//! are read from the firmware's loaded configuration.
#![forbid(unsafe_code)]

mod graph;
mod interfaces;
mod keys;
mod mask;
mod panel;

use std::collections::{BTreeMap, BTreeSet};

use fujimod_catalog::reference::ResolvedSelection;
use fujimod_catalog_controls::{AdcInterval, facts::ConfigurationLocation};
use fujimod_catalog_wiring::{Located, LocatedControl, ScannerRecord};
use fujimod_emulator_devices::parts::switch_panel;
use fujimod_emulator_session::{
    Session,
    contact_driver::{self, Check, ControlChoice, ControlInput, Pose},
};
use serde_json::Value;

pub use graph::Nodes;
pub use keys::{Keys, check_key, set_gpio, set_key};

/// Bytes of the firmware's loaded configuration block, by offset.
pub trait Calibration {
    /// `length` bytes at `offset`, or `None` outside the block.
    fn bytes(&self, offset: u32, length: usize) -> Option<Vec<u8>>;
}

/// A configuration image held in memory.
impl Calibration for Vec<u8> {
    fn bytes(&self, offset: u32, length: usize) -> Option<Vec<u8>> {
        let start = usize::try_from(offset).ok()?;
        self.get(start..start.checked_add(length)?)
            .map(<[u8]>::to_vec)
    }
}

/// The configuration block a running session's firmware has loaded.
pub struct LiveCalibration<'a> {
    session: &'a Session,
    base: u64,
}

impl<'a> LiveCalibration<'a> {
    /// Locate the loaded block; refuses before the firmware has loaded it.
    pub fn new(session: &'a Session, location: ConfigurationLocation) -> Result<Self, String> {
        let base = match location {
            ConfigurationLocation::Direct { address } => address,
            ConfigurationLocation::Pointer { address } => {
                let mut bytes = [0; 4];
                session.board.read_physical(address, &mut bytes)?;
                u64::from(u32::from_le_bytes(bytes))
            }
        };
        if base == 0 {
            return Err("the firmware has not loaded its configuration yet".into());
        }
        Ok(Self { session, base })
    }
}

impl Calibration for LiveCalibration<'_> {
    fn bytes(&self, offset: u32, length: usize) -> Option<Vec<u8>> {
        let mut bytes = vec![0; length];
        self.session
            .board
            .read_physical(self.base.checked_add(u64::from(offset))?, &mut bytes)
            .ok()?;
        Some(bytes)
    }
}

/// One control the compiled panel cannot offer, and why.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Problem {
    pub control: String,
    pub reason: String,
}

/// The wiring and input confirmation built for one session.
#[derive(Clone)]
pub struct Controls {
    /// Electrical levels of every control position.
    pub wiring: switch_panel::Spec,
    /// Scanner checks that confirm each position change.
    pub confirmation: contact_driver::Spec,
    /// Electronic controls that could not be compiled.
    pub problems: Vec<Problem>,
}

/// The ready and expected memory checks of each pose.
struct Checks<'a> {
    nodes: &'a Nodes<'a>,
    calibration: Option<&'a dyn Calibration>,
    location: Option<ConfigurationLocation>,
    /// Calibration offsets of every admitted band on each ADC channel.
    channel_tags: BTreeMap<u8, BTreeSet<u32>>,
    /// The located predicate behind each scanner record.
    sources: BTreeMap<u64, &'a fujimod_catalog_wiring::Wire>,
}

fn configuration_offsets(interval: &AdcInterval) -> Option<[u32; 2]> {
    match interval {
        AdcInterval::ConfigurationU16 { lower, upper, .. } => Some([*lower, *upper]),
        AdcInterval::Literal { .. } => None,
    }
}

impl Checks<'_> {
    fn check(&self, address: u64, bytes: Vec<u8>) -> Result<Check, String> {
        Ok(Check {
            address: self.nodes.physical(address, bytes.len() as u64)?,
            bytes,
            pointer: None,
        })
    }

    fn pose(&self, observation: &panel::Observation) -> Result<Pose, String> {
        let mut ready = Vec::new();
        let mut expected = Vec::new();
        let mut tags = BTreeSet::new();
        for observed in &observation.records {
            let ScannerRecord {
                address: base,
                stride,
                physical,
                kind,
                source,
                lower,
                upper,
            } = observed.record;
            let stable = observed
                .stable_offset
                .ok_or("Scanner observation has no stable state offset")?;
            let bounds: Vec<u8> = match stride {
                28 => [(lower as u16).to_le_bytes(), (upper as u16).to_le_bytes()].concat(),
                _ => [lower.to_le_bytes(), upper.to_le_bytes()].concat(),
            };
            ready.push(self.check(base, physical.to_le_bytes().to_vec())?);
            ready.push(self.check(base + 9, vec![kind])?);
            ready.push(self.check(base + 12, [source.to_le_bytes().to_vec(), bounds].concat())?);
            expected.push(self.check(base + u64::from(stable), vec![observed.expected])?);
            // A button on a shared ladder must preserve its neighboring dial
            // pose. Check every admitted calibration band on that channel.
            if let Some(wire) = self.sources.get(&base) {
                for (channel, _) in wire.kind.analog_terms() {
                    tags.extend(
                        self.channel_tags
                            .get(&channel)
                            .into_iter()
                            .flatten()
                            .copied(),
                    );
                }
            }
        }
        if !tags.is_empty() && (self.calibration.is_none() || self.location.is_none()) {
            return Err("Calibrated input has no checked loaded-calibration location".into());
        }
        for tag in tags {
            let raw = self
                .calibration
                .and_then(|calibration| calibration.bytes(tag, 2))
                .ok_or("Calibration observation exceeds its input")?;
            match self.location {
                Some(ConfigurationLocation::Pointer { address }) => ready.push(Check {
                    address: u64::from(tag),
                    bytes: raw,
                    pointer: Some(self.nodes.physical(address, 4)?),
                }),
                Some(ConfigurationLocation::Direct { address }) => {
                    ready.push(self.check(address + u64::from(tag), raw)?)
                }
                None => {
                    return Err(
                        "Calibrated input has no checked loaded-calibration location".into(),
                    );
                }
            }
        }
        Ok(Pose {
            position: observation.position.clone(),
            ready,
            expected,
        })
    }
}

fn choices(positions: &[String]) -> Vec<ControlChoice> {
    positions
        .iter()
        .map(|id| ControlChoice {
            id: id.clone(),
            label: id.clone(),
        })
        .collect()
}

fn supported(id: &str, label: &str, kind: &str) -> ControlInput {
    ControlInput {
        id: id.into(),
        label: label.into(),
        kind: kind.into(),
        supported: true,
        unavailable: None,
        positions: None,
        initial: None,
        relative: None,
        width: None,
        height: None,
        stages: None,
    }
}

/// Compile the panel and driver for `located` controls on a session whose
/// definition is `definition`, reading thresholds through `calibration`.
pub fn compile(
    located: &Located,
    definition: &Value,
    calibration: Option<&dyn Calibration>,
    ticks_per_second: u64,
) -> Result<Controls, String> {
    let nodes = Nodes::new(definition)?;
    // Rotary steps go to the encoder, never to scanner buttons.
    let mut controls: Vec<LocatedControl> = located.controls.clone();
    for row in &mut controls {
        if matches!(interfaces::encoder(row, &nodes, calibration), Ok(Some(_))) {
            row.actions
                .retain(|(action, _)| action != "decrease" && action != "increase");
        }
    }
    let mut plan = panel::compile(
        &controls,
        calibration,
        &nodes,
        located.analog_levels,
        located.scanner_stable_offset,
    )?;
    let sources: BTreeMap<u64, &fujimod_catalog_wiring::Wire> = located
        .controls
        .iter()
        .flat_map(|row| row.input.values().flatten())
        .filter_map(|wire| wire.scanner_record.map(|record| (record.address, wire)))
        .collect();
    let admitted: BTreeSet<u64> = plan
        .actuators
        .iter()
        .flat_map(|actuator| &actuator.states)
        .flat_map(|state| &state.records)
        .map(|observed| observed.record.address)
        .collect();
    let mut channel_tags: BTreeMap<u8, BTreeSet<u32>> = BTreeMap::new();
    for address in &admitted {
        if let Some(wire) = sources.get(address) {
            for (channel, interval) in wire.kind.analog_terms() {
                let tags = channel_tags.entry(channel).or_default();
                if let Some(offsets) = configuration_offsets(interval) {
                    tags.extend(offsets);
                }
            }
        }
    }
    let checks = Checks {
        nodes: &nodes,
        calibration,
        location: located.calibration,
        channel_tags,
        sources,
    };
    let mut inputs = Vec::new();
    let mut actuators = Vec::new();
    for (control, actuator) in plan.controls.iter().zip(&plan.actuators) {
        let mut item = supported(&control.id, &control.label, &control.kind);
        if !actuator.momentary {
            item.initial = Some(control.initial.clone());
            item.positions = Some(choices(&control.positions));
        } else if control.kind == "shutter" {
            item.stages = Some(choices(&control.positions[1..]));
        }
        inputs.push(item);
        let checked = actuator
            .states
            .iter()
            .flat_map(|state| &state.records)
            .all(|observed| {
                matches!(
                    (observed.record.stride, observed.stable_offset),
                    (28, Some(21)) | (32, Some(25))
                )
            });
        let states = actuator
            .states
            .iter()
            .map(|state| {
                if checked {
                    checks.pose(state)
                } else {
                    Ok(Pose {
                        position: state.position.clone(),
                        ready: Vec::new(),
                        expected: Vec::new(),
                    })
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        actuators.push(contact_driver::Actuator {
            id: control.id.clone(),
            momentary: actuator.momentary,
            states,
        });
    }
    let mut accepted = BTreeSet::new();
    let mut interface_errors = BTreeMap::new();
    let mut driver_interfaces = Vec::new();
    for row in &located.controls {
        let binding =
            interfaces::encoder(row, &nodes, calibration).and_then(|binding| match binding {
                Some(binding) => Ok(Some(binding)),
                None => interfaces::touch(row, &nodes, calibration),
            });
        let power = interfaces::power(row, &nodes);
        let (binding, power) = match (binding, power) {
            (Ok(binding), Ok(power)) => (binding, power),
            (Err(error), _) | (_, Err(error)) => {
                interface_errors.insert(row.id.clone(), error);
                continue;
            }
        };
        if let Some(binding) = binding {
            accepted.insert(row.id.clone());
            let existing = inputs
                .iter()
                .filter(|input: &&ControlInput| input.id == row.id)
                .count();
            if existing > 1 {
                return Err("Duplicate physical control identity".into());
            }
            let index = match inputs.iter().position(|input| input.id == row.id) {
                Some(index) => index,
                None => {
                    let kind = match &binding {
                        contact_driver::Interface::Rotary { .. } => "rotary",
                        contact_driver::Interface::Touch { .. } => "touch",
                    };
                    inputs.push(supported(&row.id, &row.name, kind));
                    inputs.len() - 1
                }
            };
            match &binding {
                contact_driver::Interface::Rotary { .. } => inputs[index].relative = Some(true),
                contact_driver::Interface::Touch { width, height, .. } => {
                    inputs[index].width = Some(*width);
                    inputs[index].height = Some(*height);
                }
            }
            driver_interfaces.push(binding);
        }
        if let Some(power) = power {
            accepted.insert(row.id.clone());
            let mut item = supported(&power.control.id, &power.control.label, &power.control.kind);
            item.initial = Some(power.control.initial.clone());
            item.positions = Some(choices(&power.control.positions));
            inputs.push(item);
            plan.controls.push(power.control);
            plan.circuits.push(power.circuit);
            actuators.push(power.actuator);
        }
    }
    for unavailable in &mut plan.unavailable {
        if let Some(error) = interface_errors.get(&unavailable.id) {
            unavailable.reason = error.clone();
        }
    }
    let mut problems = Vec::new();
    for problem in plan.problems {
        if accepted.contains(&problem.control) {
            continue;
        }
        let reason = interface_errors
            .get(&problem.control)
            .cloned()
            .unwrap_or(problem.reason);
        problems.push(Problem {
            control: problem.control,
            reason,
        });
    }
    for unavailable in &plan.unavailable {
        if !accepted.contains(&unavailable.id) {
            inputs.push(ControlInput {
                supported: false,
                unavailable: Some(unavailable.reason.clone()),
                ..supported(&unavailable.id, &unavailable.label, &unavailable.kind)
            });
        }
    }
    let poll_ticks = (ticks_per_second / 1000).max(1);
    Ok(Controls {
        wiring: switch_panel::Spec {
            controls: plan.controls,
            circuits: plan.circuits,
        },
        confirmation: contact_driver::Spec {
            poll_ticks,
            max_phase_ticks: ticks_per_second * 3,
            catalog: inputs,
            actuators,
            interfaces: driver_interfaces,
        },
        problems,
    })
}

/// Build the selected body's control wiring and input confirmation for a
/// running session, reading thresholds from the firmware's loaded
/// configuration, and install them.
/// Does nothing when the session's controls are already connected.
pub fn connect(
    session: &mut Session,
    selection: &ResolvedSelection,
    ticks_per_second: u64,
) -> Result<Vec<Problem>, String> {
    if session.controls.is_some() {
        return Ok(Vec::new());
    }
    let located = fujimod_catalog_wiring::locate(selection)?;
    let controls = {
        let calibration = located
            .calibration
            .map(|location| LiveCalibration::new(session, location))
            .transpose()?;
        compile(
            &located,
            &session.definition,
            calibration
                .as_ref()
                .map(|calibration| calibration as &dyn Calibration),
            ticks_per_second,
        )?
    };
    session.configure_controls(controls.wiring)?;
    session.configure_contact_driver(controls.confirmation)?;
    Ok(controls.problems)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use fujimod_catalog_controls::AdcInterval;
    use fujimod_catalog_wiring::{Located, LocatedControl, ScannerRecord, Wire, WireKind};
    use fujimod_emulator_devices::parts::switch_panel::Signal;
    use serde_json::json;

    fn record(index: u64) -> ScannerRecord {
        ScannerRecord {
            address: 0x1000 + 28 * index,
            stride: 28,
            physical: index as u32,
            kind: 1,
            source: 8,
            lower: 0,
            upper: 0,
        }
    }

    fn button(id: &str, key: &str, kind: WireKind, index: u64) -> LocatedControl {
        LocatedControl {
            id: id.into(),
            name: id.to_uppercase(),
            kind: "button".into(),
            mechanical: false,
            unavailable: None,
            keys: vec![key.into()],
            input: BTreeMap::from([(
                key.into(),
                vec![Wire {
                    kind,
                    scanner_record: Some(record(index)),
                }],
            )]),
            actions: vec![("press".into(), vec![key.into()])],
            positions: Vec::new(),
            interface: None,
        }
    }

    fn band(lower: u16, upper: u16) -> WireKind {
        WireKind::Adc {
            channel: 2,
            intervals: vec![AdcInterval::Literal {
                lower,
                upper,
                when: None,
                unless: Vec::new(),
            }],
            expected_result: Some(1),
        }
    }

    #[test]
    fn buttons_sharing_a_ladder_compose_into_one_circuit() {
        let located = Located {
            controls: vec![
                button("up", "UP", band(100, 199), 0),
                button("down", "DOWN", band(300, 399), 1),
                button(
                    "shoot",
                    "S",
                    WireKind::Gpio {
                        port: 1,
                        pin: 3,
                        active_level: 0,
                    },
                    2,
                ),
            ],
            scanner_stable_offset: Some(21),
            calibration: None,
            analog_levels: 1024,
        };
        let definition = json!({
            "devices": {"nodes": [
                {"name": "gpio", "model": {"kind": "gpio", "options": {"input_levels": [0, 0xffff]}}},
                {"name": "adc", "model": {"kind": "adc", "options": {"samples": [0, 0, 1023, 0]}}},
            ]},
            "execution": {"mappings": [], "ram": [{"base": 0, "size": 0x10000}]},
        });
        let controls = super::compile(&located, &definition, None, 12_000_000).unwrap();
        assert!(controls.problems.is_empty());
        let ladder = controls
            .wiring
            .circuits
            .iter()
            .find(|c| c.controls == ["down", "up"])
            .unwrap();
        let level = |pose: [&str; 2]| {
            let case = ladder
                .cases
                .iter()
                .find(|case| case.positions == pose)
                .unwrap();
            match case.signals.as_slice() {
                [
                    Signal::Analog {
                        channel: 2, value, ..
                    },
                ] => *value,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(level(["released", "released"]), 1023);
        assert_eq!(level(["released", "pressed"]), 149);
        assert_eq!(level(["pressed", "released"]), 349);
        assert!(
            !ladder
                .cases
                .iter()
                .any(|case| case.positions == ["pressed", "pressed"])
        );
        let shoot = controls
            .wiring
            .circuits
            .iter()
            .find(|c| c.controls == ["shoot"])
            .unwrap();
        assert_eq!(shoot.cases.len(), 2);
        assert!(
            controls
                .wiring
                .controls
                .iter()
                .all(|control| control.initial == "released")
        );
        let up = controls
            .confirmation
            .actuators
            .iter()
            .find(|a| a.id == "up")
            .unwrap();
        let pressed = &up.states[1];
        assert_eq!(pressed.expected.len(), 1);
        assert_eq!(
            (
                pressed.expected[0].address,
                pressed.expected[0].bytes.as_slice()
            ),
            (0x1000 + 21, &[1u8][..])
        );
        assert_eq!(pressed.ready.len(), 3);
    }
}
