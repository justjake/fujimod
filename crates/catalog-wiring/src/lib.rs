//! Physical controls resolved against the firmware's located inputs and the
//! body's wiring, as plain data.
//!
//! Each control's declared actions and positions are bound to the located
//! firmware input they name, and each input's electrical predicate is
//! translated from body connections to concrete pins, ADC channels or serial
//! status bits. Nothing here runs a guest or reads memory: configuration
//! offsets stay offsets, and firmware addresses stay firmware addresses.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use fujimod_catalog::reference::{ConnectionSignal, ResolvedSelection};
use fujimod_catalog_controls::{
    AdcEncoderCalibration, AdcInterval, ControlBehavior, ControlMotion, ElectricalInput,
    FirmwareControlIds, InputBinding, InputTarget, LocatedInput, PhysicalControl, PhysicalControls,
    facts::{ConfigurationLocation, ControlInterface, DecodedTouch},
};
use serde::Serialize;

/// Every physical control of a body, resolved for one firmware.
#[derive(Clone, Debug, Serialize)]
pub struct Located {
    /// Controls in canonical identity order.
    pub controls: Vec<LocatedControl>,
    /// Stable-state field offset within a scanner record, when the firmware's
    /// scanner reader is located.
    pub scanner_stable_offset: Option<u32>,
    /// Where the running firmware keeps its loaded configuration block.
    pub calibration: Option<ConfigurationLocation>,
    /// Number of distinct values the body's ADC reports.
    pub analog_levels: u32,
}

/// One physical control and the inputs its actions and positions name.
#[derive(Clone, Debug, Serialize)]
pub struct LocatedControl {
    pub id: String,
    /// Manual name.
    pub name: String,
    /// Mechanism kind: button, shutter, joystick, rocker, dial, ring, switch,
    /// lever, touch, mechanical or unknown.
    pub kind: String,
    /// Whether the control has no electronic action at all.
    pub mechanical: bool,
    /// Why the control's inputs could not be located.
    pub unavailable: Option<String>,
    /// Input names in the order they were bound.
    pub keys: Vec<String>,
    /// Electrical predicates of each input name that has one.
    pub input: BTreeMap<String, Vec<Wire>>,
    /// Actions in declaration order and the input names each asserts.
    pub actions: Vec<(String, Vec<String>)>,
    /// Positions in declaration order and the input each selects.
    pub positions: Vec<LocatedPosition>,
    /// A protocol outside the key scanner carried by one of its connections.
    pub interface: Option<Interface>,
}

impl LocatedControl {
    pub fn action(&self, name: &str) -> Option<&[String]> {
        self.actions
            .iter()
            .find(|(action, _)| action == name)
            .map(|(_, keys)| keys.as_slice())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LocatedPosition {
    pub position: String,
    pub keys: Vec<String>,
    /// Whether this position asserts its input, when declared.
    pub active: Option<bool>,
}

/// One located electrical predicate and the scanner record that reports it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Wire {
    pub kind: WireKind,
    pub scanner_record: Option<ScannerRecord>,
}

/// An electrical predicate on concrete body signals.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireKind {
    Gpio {
        port: u8,
        pin: u8,
        active_level: u8,
    },
    GpioContacts {
        pins: Vec<(u8, u8)>,
        levels: Vec<u8>,
    },
    Adc {
        channel: u8,
        intervals: Vec<AdcInterval>,
        /// Firmware callback result treated as active: 1 without a callback,
        /// absent when a callback's result is not located.
        expected_result: Option<u32>,
    },
    AdcConjunction {
        terms: Vec<(u8, AdcInterval)>,
        expected_result: u32,
    },
    SerialStatus {
        /// Base address of the serial controller block.
        base: u64,
        register: u8,
        mask: u8,
        active_value: u32,
    },
    GpioQuadrature {
        connection_id: String,
        pins: Vec<(u8, u8)>,
        phase_map: [u8; 4],
        transitions: [u8; 16],
        reset: [u32; 4],
        event: u32,
        guard: u64,
        threshold: u32,
    },
    GpioFixedCycle {
        connection_id: String,
        pins: Vec<(u8, u8)>,
        cycle_sign: i8,
    },
    GpioTransition {
        connection_id: String,
        pins: Vec<(u8, u8)>,
        phase_map: [u8; 4],
        transitions: [u8; 16],
        event: u32,
        guard: u64,
        threshold: u32,
    },
    AdcTransition {
        connection_id: String,
        channels: [u8; 2],
        phase_map: [u8; 4],
        transitions: [u8; 16],
        event: u32,
        calibration_offsets: AdcEncoderCalibration,
    },
}

impl WireKind {
    /// The ADC channels and intervals this predicate tests.
    pub fn analog_terms(&self) -> Vec<(u8, &AdcInterval)> {
        match self {
            Self::Adc {
                channel, intervals, ..
            } => intervals
                .iter()
                .map(|interval| (*channel, interval))
                .collect(),
            Self::AdcConjunction { terms, .. } => terms
                .iter()
                .map(|(channel, interval)| (*channel, interval))
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// One firmware scanner record's address and the operands stored in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ScannerRecord {
    /// Firmware address of the record.
    pub address: u64,
    pub stride: u32,
    pub physical: u32,
    pub kind: u8,
    pub source: u32,
    pub lower: u32,
    pub upper: u32,
}

/// A protocol outside the key scanner.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Interface {
    PowerSwitch {
        /// Base address of the GPIO block.
        gpio_base: u64,
        port: u8,
        pin: u8,
        on_level: u8,
        saved_inputs: u64,
        endpoint: u32,
        register: u32,
        mask: u32,
        on_value: u32,
        off_value: u32,
        /// The body's microcontroller serial controller, when declared.
        controller_id: Option<String>,
    },
    PrefixedTouch {
        mode: u32,
        register: u32,
        report_bytes: u32,
        contacts: u32,
        coordinate_bits: u32,
        width: u32,
        height: u32,
        offset_tags: [u32; 2],
        irq: u32,
        priority: u32,
        irq_mode: u32,
        decoded: DecodedTouch,
        /// Control-bus device address and channel.
        address: u8,
        channel: u8,
    },
}

fn decode<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    what: &str,
) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|error| format!("{what}: {error}"))
}

/// Resolve every physical control of the selected body for the selected firmware.
pub fn locate(selection: &ResolvedSelection) -> Result<Located, String> {
    let physical: PhysicalControls = decode(
        selection
            .physical_controls
            .as_ref()
            .ok_or("selected body has no physical controls")?,
        "physical controls",
    )?;
    let ids: FirmwareControlIds = decode(
        selection
            .control_ids
            .as_ref()
            .ok_or("selected firmware has no control IDs")?,
        "firmware control IDs",
    )?;
    let calibration = selection
        .firmware
        .generated_facts
        .as_ref()
        .and_then(|facts| facts.cfgdata_location.as_ref())
        .map(|location| decode::<ConfigurationLocation>(location, "configuration location"))
        .transpose()?;
    let resolver = Resolver {
        selection,
        ids: &ids,
    };
    let controls = physical
        .controls
        .iter()
        .map(|(id, control)| resolver.control(id, control))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Located {
        controls,
        scanner_stable_offset: ids
            .input_readers
            .scanner
            .as_ref()
            .map(|reader| reader.stable_offset),
        calibration,
        analog_levels: physical.analog_levels.map_or(65536, u32::from),
    })
}

struct Resolver<'a> {
    selection: &'a ResolvedSelection,
    ids: &'a FirmwareControlIds,
}

/// A binding's name, or why it cannot be located.
fn binding_name(binding: &InputBinding) -> Option<&str> {
    match binding {
        InputBinding::DebugKey { name } | InputBinding::NativeInput { name } => Some(name),
        InputBinding::ScannerRow { .. } => None,
    }
}

/// The manual's kind for a control.
fn manual_kind(control: &PhysicalControl) -> String {
    let Some(behavior) = &control.behavior else {
        return if control.mechanical {
            "mechanical"
        } else {
            "unknown"
        }
        .into();
    };
    match behavior {
        ControlBehavior::Button { .. } => "button",
        ControlBehavior::Shutter { .. } => "shutter",
        ControlBehavior::Pad { .. } => "joystick",
        ControlBehavior::Rocker { .. } => "rocker",
        ControlBehavior::Dial { .. } => "dial",
        ControlBehavior::Ring { .. } => "ring",
        ControlBehavior::Switch { .. } => "switch",
        ControlBehavior::Lever { .. } => "lever",
        ControlBehavior::Touch => "touch",
    }
    .into()
}

type Group = Vec<(String, Option<InputBinding>)>;

/// Actions in declaration order, each a list of (control identity, binding),
/// and positions with a nameable binding.
fn declarations(
    id: &str,
    behavior: Option<&ControlBehavior>,
) -> (
    Vec<(String, Group)>,
    Vec<fujimod_catalog_controls::BoundPosition>,
) {
    let mut actions: Vec<(String, Group)> = Vec::new();
    let mut positions = Vec::new();
    let own = |binding: &Option<InputBinding>| vec![(id.to_owned(), binding.clone())];
    let set = |actions: &mut Vec<(String, Group)>, name: &str, group: Group| match actions
        .iter_mut()
        .find(|(action, _)| action == name)
    {
        Some(existing) => existing.1 = group,
        None => actions.push((name.to_owned(), group)),
    };
    let Some(behavior) = behavior else {
        return (actions, positions);
    };
    let mut press = None;
    match behavior {
        ControlBehavior::Button { press: binding } => {
            set(&mut actions, "press", own(binding));
            press = binding.clone();
        }
        ControlBehavior::Shutter { stages } => {
            for (stage, bindings) in stages {
                let group = match bindings {
                    Some(bindings) => bindings
                        .iter()
                        .map(|binding| (id.to_owned(), Some(binding.clone())))
                        .collect(),
                    None => vec![(id.to_owned(), None)],
                };
                set(&mut actions, stage, group);
            }
        }
        ControlBehavior::Pad {
            directions,
            press: binding,
        } => {
            for (direction, binding) in directions {
                set(
                    &mut actions,
                    direction.id(),
                    vec![(format!("{id}.{}", direction.id()), binding.clone())],
                );
            }
            press = binding.clone();
        }
        ControlBehavior::Rocker {
            decrease,
            increase,
            press: binding,
        } => {
            set(&mut actions, "decrease", own(decrease));
            set(&mut actions, "increase", own(increase));
            press = binding.clone();
        }
        ControlBehavior::Dial {
            motion,
            press: binding,
            ..
        }
        | ControlBehavior::Ring {
            motion,
            press: binding,
            ..
        } => {
            match motion {
                ControlMotion::Steps { decrease, increase } => {
                    set(&mut actions, "decrease", own(decrease));
                    set(&mut actions, "increase", own(increase));
                }
                ControlMotion::Positions { positions: bound } => positions = bound.clone(),
            }
            press = binding.clone();
        }
        ControlBehavior::Lever {
            positions: bound,
            momentary,
        } => {
            if *momentary == Some(true) {
                for position in bound {
                    set(&mut actions, &position.id, own(&position.key));
                }
            } else {
                positions = bound.clone();
            }
        }
        ControlBehavior::Switch { positions: bound } => positions = bound.clone(),
        ControlBehavior::Touch => {}
    }
    if press.is_some() {
        set(&mut actions, "press", own(&press));
    }
    // A scanner-row identity names a generated scanner row, not a firmware
    // key this resolution can locate; such positions are left out.
    positions.retain(|position| {
        position
            .key
            .as_ref()
            .is_none_or(|key| binding_name(key).is_some())
    });
    (actions, positions)
}

impl Resolver<'_> {
    fn control(&self, id: &str, control: &PhysicalControl) -> Result<LocatedControl, String> {
        let mut row = LocatedControl {
            id: id.into(),
            name: control.name.clone(),
            kind: manual_kind(control),
            mechanical: control.mechanical,
            unavailable: None,
            keys: Vec::new(),
            input: BTreeMap::new(),
            actions: Vec::new(),
            positions: Vec::new(),
            interface: None,
        };
        if control.mechanical {
            row.unavailable =
                Some("No electronic key established for this mechanical control".into());
            return Ok(row);
        }
        let (actions, positions) = declarations(id, control.behavior.as_ref());
        let mut located: Vec<(String, &LocatedInput)> = Vec::new();
        let mut bind = |control_id: &str, binding: &Option<InputBinding>| -> Result<(), String> {
            let binding = binding
                .as_ref()
                .ok_or("Physical action has no located firmware binding")?;
            let name =
                binding_name(binding).ok_or("Physical action has no located firmware binding")?;
            if !located.iter().any(|(known, _)| known == name) {
                located.push((name.to_owned(), self.input(control_id, binding)?));
            }
            Ok(())
        };
        let mut failure = None;
        for (_, group) in &actions {
            for (control_id, binding) in group {
                if let Err(error) = bind(control_id, binding) {
                    failure.get_or_insert(error);
                }
            }
        }
        for position in &positions {
            let result = match &position.key {
                Some(_) => bind(id, &position.key),
                None => Err("Physical position has no located firmware binding".into()),
            };
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        if let Some(error) = failure {
            row.unavailable = Some(error);
            return Ok(row);
        }
        for (name, input) in &located {
            if let Some(electrical) = &input.electrical {
                row.input
                    .insert(name.clone(), vec![self.wire(electrical, input)?]);
            }
        }
        row.keys = located.into_iter().map(|(name, _)| name).collect();
        row.actions = actions
            .into_iter()
            .map(|(action, group)| {
                let keys = group
                    .iter()
                    .filter_map(|(_, binding)| binding.as_ref().and_then(binding_name))
                    .map(str::to_owned)
                    .collect();
                (action, keys)
            })
            .collect();
        row.positions = positions
            .iter()
            .map(|position| LocatedPosition {
                position: position.id.clone(),
                keys: position
                    .key
                    .as_ref()
                    .and_then(binding_name)
                    .map(str::to_owned)
                    .into_iter()
                    .collect(),
                active: position.asserted,
            })
            .collect();
        for connection in &control.connection_ids {
            if let Some(interface) = self.ids.interfaces.get(connection) {
                row.interface = Some(self.interface(interface)?);
            }
        }
        Ok(row)
    }

    /// The one located input a binding names for a control, preferring
    /// inputs with a located electrical predicate.
    fn input(&self, control_id: &str, binding: &InputBinding) -> Result<&LocatedInput, String> {
        let (event, name) = match binding {
            InputBinding::DebugKey { name } => (
                self.ids
                    .debug_keys
                    .iter()
                    .find(|key| key.name == *name)
                    .map(|key| key.event),
                name,
            ),
            InputBinding::NativeInput { name } => (
                self.ids
                    .native_key_names
                    .iter()
                    .flat_map(|names| &names.rows)
                    .find(|key| key.name == *name)
                    .map(|key| key.event),
                name,
            ),
            InputBinding::ScannerRow { .. } => {
                return Err("Physical action has no located firmware binding".into());
            }
        };
        let event = event.ok_or_else(|| format!("Firmware has no input named {name}"))?;
        let candidates: Vec<&LocatedInput> = self
            .ids
            .inputs
            .iter()
            .filter(|input| {
                input.event == event
                    && matches!(&input.target, InputTarget::Control { control_id: owner } if owner == control_id)
                    && (!matches!(binding, InputBinding::DebugKey { .. })
                        || input.debug_key.as_deref() == Some(name.as_str()))
            })
            .collect();
        let electrical: Vec<&LocatedInput> = candidates
            .iter()
            .copied()
            .filter(|input| input.electrical.is_some())
            .collect();
        let chosen = if electrical.is_empty() {
            candidates
        } else {
            electrical
        };
        match chosen.as_slice() {
            [] => Err(format!("Firmware input for {name} is not located")),
            [one] => Ok(one),
            _ => Err(format!("Firmware input for {name} is not unique")),
        }
    }

    /// The body connection's signals, all of one kind.
    fn connection(&self, id: &str) -> Result<&[ConnectionSignal], String> {
        let mut matches = self
            .selection
            .body
            .connections
            .iter()
            .filter(|row| row.id == id);
        let connection = matches
            .next()
            .ok_or_else(|| format!("Control input requires one body connection: {id}"))?;
        if matches.next().is_some() || connection.signals.is_empty() {
            return Err(format!("Control input requires one body connection: {id}"));
        }
        Ok(&connection.signals)
    }

    fn gpio_pins(&self, id: &str) -> Result<Vec<(u8, u8)>, String> {
        self.connection(id)?
            .iter()
            .map(|signal| match signal {
                ConnectionSignal::Gpio { port, pin, .. } => Ok((*port, *pin)),
                _ => Err(format!("Control input has incompatible body wiring: {id}")),
            })
            .collect()
    }

    fn adc_channels(&self, id: &str) -> Result<Vec<u8>, String> {
        self.connection(id)?
            .iter()
            .map(|signal| match signal {
                ConnectionSignal::Adc { channel, .. } => Ok(*channel),
                _ => Err(format!("Control input has incompatible body wiring: {id}")),
            })
            .collect()
    }

    fn one_adc_channel(&self, id: &str) -> Result<u8, String> {
        match self.adc_channels(id)?.as_slice() {
            [channel] => Ok(*channel),
            _ => Err(format!("Control input has incompatible body wiring: {id}")),
        }
    }

    fn wire(&self, electrical: &ElectricalInput, input: &LocatedInput) -> Result<Wire, String> {
        let kind = match electrical {
            ElectricalInput::AdcConjunction {
                terms,
                expected_result,
                ..
            } => WireKind::AdcConjunction {
                terms: terms
                    .iter()
                    .map(|term| {
                        Ok((
                            self.one_adc_channel(&term.connection_id)?,
                            term.interval.clone(),
                        ))
                    })
                    .collect::<Result<_, String>>()?,
                expected_result: *expected_result,
            },
            ElectricalInput::AdcTransition {
                connection_id,
                phase_map,
                transitions,
                decoder_event,
                calibration_offsets,
            } => {
                let channels = self.adc_channels(connection_id)?;
                let [first, second] = channels.as_slice() else {
                    return Err("Analog encoder requires two distinct ordered channels".into());
                };
                if first == second {
                    return Err("Analog encoder requires two distinct ordered channels".into());
                }
                WireKind::AdcTransition {
                    connection_id: connection_id.clone(),
                    channels: [*first, *second],
                    phase_map: *phase_map,
                    transitions: *transitions,
                    event: *decoder_event,
                    calibration_offsets: calibration_offsets.clone(),
                }
            }
            ElectricalInput::GpioTransition {
                connection_id,
                phase_map,
                transitions,
                decoder_event,
                guard,
                threshold,
            } => {
                let pins = self.gpio_pins(connection_id)?;
                if pins.len() != 2 {
                    return Err("Transition encoder requires two ordered GPIO contacts".into());
                }
                WireKind::GpioTransition {
                    connection_id: connection_id.clone(),
                    pins,
                    phase_map: *phase_map,
                    transitions: *transitions,
                    event: *decoder_event,
                    guard: *guard,
                    threshold: *threshold,
                }
            }
            ElectricalInput::GpioQuadrature {
                connection_id,
                phase_map,
                transitions,
                reset,
                decoder_event,
                guard,
                threshold,
            } => WireKind::GpioQuadrature {
                connection_id: connection_id.clone(),
                pins: self.gpio_pins(connection_id)?,
                phase_map: *phase_map,
                transitions: *transitions,
                reset: *reset,
                event: *decoder_event,
                guard: *guard,
                threshold: *threshold,
            },
            ElectricalInput::GpioFixedCycle {
                connection_id,
                cycle_sign,
            } => WireKind::GpioFixedCycle {
                connection_id: connection_id.clone(),
                pins: self.gpio_pins(connection_id)?,
                cycle_sign: *cycle_sign,
            },
            ElectricalInput::Gpio {
                connection_id,
                expected_level,
                ..
            } => {
                let pins = self.gpio_pins(connection_id)?;
                let [(port, pin)] = pins.as_slice() else {
                    return Err(format!(
                        "Control input has incompatible body wiring: {connection_id}"
                    ));
                };
                WireKind::Gpio {
                    port: *port,
                    pin: *pin,
                    active_level: u8::from(*expected_level),
                }
            }
            ElectricalInput::GpioContacts {
                connection_id,
                expected_levels,
                ..
            } => {
                let pins = self.gpio_pins(connection_id)?;
                if pins.len() != expected_levels.len() {
                    return Err(format!(
                        "GPIO predicate differs from body contact count: {connection_id}"
                    ));
                }
                WireKind::GpioContacts {
                    pins,
                    levels: expected_levels
                        .iter()
                        .map(|level| u8::from(*level))
                        .collect(),
                }
            }
            ElectricalInput::Adc {
                connection_id,
                intervals,
                callback,
                expected_result,
            } => {
                if callback.is_none() && intervals.len() != 1 {
                    return Err(format!(
                        "Direct ADC input requires one interval: {connection_id}"
                    ));
                }
                WireKind::Adc {
                    channel: self.one_adc_channel(connection_id)?,
                    intervals: intervals.clone(),
                    expected_result: if callback.is_some() {
                        *expected_result
                    } else {
                        Some(1)
                    },
                }
            }
            ElectricalInput::SerialStatus {
                connection_id,
                register,
                mask,
                active_value,
                ..
            } => {
                let [
                    ConnectionSignal::SerialStatus {
                        block_id,
                        register: wired,
                        mask: wired_mask,
                    },
                ] = self.connection(connection_id)?
                else {
                    return Err(format!(
                        "Control input has incompatible body wiring: {connection_id}"
                    ));
                };
                if (u32::from(*wired), u32::from(*wired_mask)) != (*register, *mask) {
                    return Err(format!(
                        "Located status bits differ from body wiring: {connection_id}"
                    ));
                }
                let block = self
                    .selection
                    .soc
                    .blocks
                    .iter()
                    .find(|block| block.id == *block_id)
                    .ok_or_else(|| format!("Status bits name an absent SoC block: {block_id}"))?;
                WireKind::SerialStatus {
                    base: block.base,
                    register: *wired,
                    mask: *wired_mask,
                    active_value: *active_value,
                }
            }
        };
        let scanner_record = input.scanner.as_ref().map(|scanner| {
            let table = &self.ids.scanner;
            ScannerRecord {
                address: table.base + u64::from(table.stride) * u64::from(scanner.index),
                stride: table.stride,
                physical: scanner.witness.physical_key,
                kind: scanner.witness.input_kind.raw(),
                source: scanner.witness.source,
                lower: scanner.witness.lower,
                upper: scanner.witness.upper,
            }
        });
        Ok(Wire {
            kind,
            scanner_record,
        })
    }

    fn interface(&self, interface: &ControlInterface) -> Result<Interface, String> {
        Ok(match interface {
            ControlInterface::PowerSwitch {
                connection_id,
                saved_inputs,
                endpoint,
                register,
                mask,
                on_value,
                off_value,
            } => {
                let [
                    ConnectionSignal::Gpio {
                        port,
                        pin,
                        active_level,
                    },
                ] = self.connection(connection_id)?
                else {
                    return Err(format!(
                        "Control input has incompatible body wiring: {connection_id}"
                    ));
                };
                let mut gpio = self.selection.soc.blocks.iter().filter(|block| {
                    block.id == "gpio"
                        && matches!(block.kind, fujimod_catalog::reference::SocBlockKind::Gpio)
                });
                let block = gpio.next().ok_or("Power switch requires one GPIO block")?;
                if gpio.next().is_some() {
                    return Err("Power switch requires one GPIO block".into());
                }
                Interface::PowerSwitch {
                    gpio_base: block.base,
                    port: *port,
                    pin: *pin,
                    on_level: u8::from(*active_level),
                    saved_inputs: *saved_inputs,
                    endpoint: *endpoint,
                    register: *register,
                    mask: *mask,
                    on_value: *on_value,
                    off_value: *off_value,
                    controller_id: self
                        .selection
                        .body
                        .microcontroller
                        .as_ref()
                        .map(|controller| controller.controller_id.clone()),
                }
            }
            ControlInterface::PrefixedTouch {
                connection_id,
                mode,
                register,
                report_bytes,
                contacts,
                coordinate_bits,
                width,
                height,
                offset_tags,
                irq,
                priority,
                irq_mode,
                decoded,
            } => {
                let [
                    ConnectionSignal::Serial {
                        address, channel, ..
                    },
                ] = self.connection(connection_id)?
                else {
                    return Err(format!(
                        "Control input has incompatible body wiring: {connection_id}"
                    ));
                };
                Interface::PrefixedTouch {
                    mode: *mode,
                    register: *register,
                    report_bytes: *report_bytes,
                    contacts: *contacts,
                    coordinate_bits: *coordinate_bits,
                    width: *width,
                    height: *height,
                    offset_tags: *offset_tags,
                    irq: *irq,
                    priority: *priority,
                    irq_mode: *irq_mode,
                    decoded: decoded.clone(),
                    address: *address,
                    channel: *channel,
                }
            }
        })
    }
}

/// A key wired to one GPIO contact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DigitalKey {
    pub port: u8,
    pub pin: u8,
    /// Level of the contact while the key is pressed.
    pub active: bool,
}

/// The GPIO contact a debug key name or body connection id drives, when it
/// is one direct GPIO contact. Interrupt lines the connection also declares
/// (the line the contact's edges raise) do not change the contact.
pub fn digital_key(
    selection: &ResolvedSelection,
    name: &str,
) -> Result<Option<DigitalKey>, String> {
    let ids = selection
        .control_ids
        .as_ref()
        .map(|ids| decode::<FirmwareControlIds>(ids, "firmware control IDs"))
        .transpose()?;
    let found = ids.iter().flat_map(|ids| &ids.inputs).find(|input| {
        input
            .debug_key
            .as_ref()
            .is_some_and(|key| key.eq_ignore_ascii_case(name))
            && matches!(input.target, InputTarget::Control { .. })
    });
    let (connection_id, expected) = match found.and_then(|input| input.electrical.as_ref()) {
        Some(ElectricalInput::Gpio {
            connection_id,
            expected_level,
            ..
        }) => (connection_id.as_str(), Some(*expected_level)),
        Some(_) => return Ok(None),
        None => (name, None),
    };
    let Some(connection) = selection
        .body
        .connections
        .iter()
        .find(|wire| wire.id == connection_id)
    else {
        return Ok(None);
    };
    let mut contacts = connection
        .signals
        .iter()
        .filter(|signal| !matches!(signal, ConnectionSignal::Interrupt { .. }));
    let (
        Some(ConnectionSignal::Gpio {
            port,
            pin,
            active_level,
        }),
        None,
    ) = (contacts.next(), contacts.next())
    else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| expected != *active_level) {
        return Err(format!(
            "key {name} disagrees with its declared GPIO polarity"
        ));
    }
    Ok(Some(DigitalKey {
        port: *port,
        pin: *pin,
        active: *active_level,
    }))
}

/// A key read from one ADC channel as a band of samples, with the bands of
/// every other input on that channel.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AnalogKey {
    pub channel: u8,
    /// The band that reads as this key pressed.
    pub band: AdcInterval,
    /// Bands of the other inputs on the same channel.
    pub others: Vec<AdcInterval>,
    /// Where the running firmware keeps the configuration that
    /// configuration-offset bounds are read from.
    pub calibration: Option<ConfigurationLocation>,
    /// Number of distinct values the body's ADC reports.
    pub analog_levels: u32,
}

/// The ADC band a key name reads, when the key is one unconditional band on
/// one channel.
pub fn analog_key(selection: &ResolvedSelection, name: &str) -> Result<Option<AnalogKey>, String> {
    let located = locate(selection)?;
    let kinds = |control: &LocatedControl| {
        control
            .input
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .flat_map(|(_, wires)| wires)
            .map(|wire| wire.kind.clone())
            .collect::<Vec<_>>()
    };
    let Some(kinds) = located
        .controls
        .iter()
        .map(kinds)
        .find(|kinds| !kinds.is_empty())
    else {
        return Ok(None);
    };
    let [
        WireKind::Adc {
            channel, intervals, ..
        },
    ] = kinds.as_slice()
    else {
        return Ok(None);
    };
    let [band] = intervals.as_slice() else {
        return Ok(None);
    };
    if band.condition().is_some() || !band.exclusions().is_empty() {
        return Ok(None);
    }
    let others = located
        .controls
        .iter()
        .flat_map(|control| &control.input)
        .filter(|(key, _)| !key.eq_ignore_ascii_case(name))
        .flat_map(|(_, wires)| wires)
        .flat_map(|wire| wire.kind.analog_terms())
        .filter(|(other, _)| other == channel)
        .map(|(_, interval)| interval.clone())
        .collect();
    Ok(Some(AnalogKey {
        channel: *channel,
        band: band.clone(),
        others,
        calibration: located.calibration,
        analog_levels: located.analog_levels,
    }))
}

/// One step of a command dial whose count input is a countdown-timer channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DialStep {
    /// The physical control id, which also names the dial on the board.
    pub control: String,
    /// The countdown-timer channel the dial's quadrature phases drive.
    pub channel: u8,
    /// 1 for the dial's increase step, -1 for its decrease step.
    pub direction: i8,
}

/// The dial step a debug key name stands for, when the key is the increase or
/// decrease step of a stepped dial wired to one countdown-timer channel.
pub fn dial_step(selection: &ResolvedSelection, name: &str) -> Result<Option<DialStep>, String> {
    let Some(physical) = selection.physical_controls.as_ref() else {
        return Ok(None);
    };
    let physical: PhysicalControls = decode(physical, "physical controls")?;
    let named = |binding: &Option<InputBinding>| match binding {
        Some(InputBinding::DebugKey { name: key }) => key.eq_ignore_ascii_case(name),
        _ => false,
    };
    for (id, control) in &physical.controls {
        let Some(ControlBehavior::Dial {
            motion: ControlMotion::Steps { decrease, increase },
            ..
        }) = &control.behavior
        else {
            continue;
        };
        let direction = if named(increase) {
            1
        } else if named(decrease) {
            -1
        } else {
            continue;
        };
        let mut channels = control
            .connection_ids
            .iter()
            .filter_map(|connection| {
                selection
                    .body
                    .connections
                    .iter()
                    .find(|wire| &wire.id == connection)
            })
            .flat_map(|wire| &wire.signals)
            .filter_map(|signal| match signal {
                ConnectionSignal::CountdownTimer { channel, .. } => Some(*channel),
                _ => None,
            });
        return match (channels.next(), channels.next()) {
            (Some(channel), None) => Ok(Some(DialStep {
                control: id.clone(),
                channel,
                direction,
            })),
            (None, _) => Err(format!(
                "{name} steps {id}, whose body connections declare no countdown-timer channel"
            )),
            _ => Err(format!(
                "{name} steps {id}, which drives several countdown-timer channels"
            )),
        };
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use fujimod_catalog::reference::ReferenceCatalog;

    #[test]
    fn xt3_command_dial_steps_resolve_to_their_countdown_channels() {
        let selection = ReferenceCatalog::bundled()
            .unwrap()
            .resolve_firmware("xt3-511")
            .unwrap();
        let step = |name| super::dial_step(&selection, name).unwrap();
        let expect = |control: &str, channel, direction| {
            Some(super::DialStep {
                control: control.into(),
                channel,
                direction,
            })
        };
        assert_eq!(step("CMD_RIGHT"), expect("rear_command_dial", 2, 1));
        assert_eq!(step("CMD_LEFT"), expect("rear_command_dial", 2, -1));
        assert_eq!(step("CMD2_R"), expect("front_command_dial", 4, 1));
        assert_eq!(step("CMD2_L"), expect("front_command_dial", 4, -1));
        // A dial's press, an ordinary key and an unknown name are not steps.
        assert_eq!(step("CMD_PUSH"), None);
        assert_eq!(step("DOWN"), None);
        assert_eq!(step("CMD_NOWHERE"), None);
    }

    #[test]
    fn a_dial_step_without_a_countdown_channel_is_refused() {
        let mut selection = ReferenceCatalog::bundled()
            .unwrap()
            .resolve_firmware("xt3-511")
            .unwrap();
        for connection in &mut selection.body.connections {
            if connection.id == "xt3-rear-dial-count" {
                connection.signals.clear();
            }
        }
        let error = super::dial_step(&selection, "CMD_RIGHT").unwrap_err();
        assert_eq!(
            error,
            "CMD_RIGHT steps rear_command_dial, whose body connections declare no countdown-timer channel"
        );
        // The other dial still resolves.
        assert!(super::dial_step(&selection, "CMD2_R").unwrap().is_some());
    }

    #[test]
    fn every_firmware_with_located_inputs_resolves_its_controls() {
        let catalog = ReferenceCatalog::bundled().unwrap();
        let mut resolved = 0;
        for id in catalog.firmware_ids() {
            let selection = catalog.resolve_firmware(id).unwrap();
            if selection.control_ids.is_none() || selection.physical_controls.is_none() {
                continue;
            }
            let located = super::locate(&selection).unwrap_or_else(|error| panic!("{id}: {error}"));
            assert!(!located.controls.is_empty(), "{id} has no controls");
            resolved += 1;
        }
        assert!(resolved >= 31, "only {resolved} firmwares resolved");
    }

    #[test]
    fn xt3_shutter_contacts_are_keys_beside_their_interrupt_lines() {
        let selection = ReferenceCatalog::bundled()
            .unwrap()
            .resolve_firmware("xt3-511")
            .unwrap();
        for (name, pin) in [("xt3-shutter-s1", 0), ("xt3-shutter-s2", 1)] {
            let wire = selection
                .body
                .connections
                .iter()
                .find(|wire| wire.id == name)
                .unwrap();
            assert!(wire.signals.iter().any(|signal| matches!(
                signal,
                fujimod_catalog::reference::ConnectionSignal::Interrupt { .. }
            )));
            assert_eq!(
                super::digital_key(&selection, name).unwrap(),
                Some(super::DigitalKey {
                    port: 0,
                    pin,
                    active: false
                })
            );
        }
        // The firmware's S1 and S2 rows are not claimed by a wired input yet.
        assert_eq!(super::digital_key(&selection, "S1").unwrap(), None);
    }

    #[test]
    fn x70_menu_ok_is_one_calibrated_adc_band_reported_by_one_scanner_record() {
        let selection = ReferenceCatalog::bundled()
            .unwrap()
            .resolve_firmware("x70-110")
            .unwrap();
        let located = super::locate(&selection).unwrap();
        let menu = located
            .controls
            .iter()
            .find(|row| row.id == "menu_ok_button")
            .unwrap();
        assert_eq!(menu.action("press"), Some(&["EXE_M".to_owned()][..]));
        let [wire] = menu.input["EXE_M"].as_slice() else {
            panic!("one wire")
        };
        let super::WireKind::Adc {
            channel: 8,
            intervals,
            expected_result: Some(1),
        } = &wire.kind
        else {
            panic!("{:?}", wire.kind)
        };
        assert!(matches!(
            intervals.as_slice(),
            [fujimod_catalog_controls::AdcInterval::ConfigurationU16 { .. }]
        ));
        assert!(wire.scanner_record.is_some());
        assert_eq!(located.analog_levels, 1024);
        assert!(located.calibration.is_some());
    }

    #[test]
    fn x70_menu_ok_resolves_to_one_configuration_band_on_its_adc_channel() {
        use fujimod_catalog_controls::AdcInterval;
        let selection = ReferenceCatalog::bundled()
            .unwrap()
            .resolve_firmware("x70-110")
            .unwrap();
        let menu = super::analog_key(&selection, "exe_m").unwrap().unwrap();
        assert_eq!(menu.channel, 8);
        assert!(matches!(menu.band, AdcInterval::ConfigurationU16 { .. }));
        assert!(!menu.others.is_empty());
        assert!(menu.others.iter().all(|band| *band != menu.band));
        assert_eq!(menu.analog_levels, 1024);
        assert!(menu.calibration.is_some());
        assert_eq!(super::analog_key(&selection, "NOT_A_KEY").unwrap(), None);
    }
}
