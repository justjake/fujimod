//! Controls the firmware reads outside the key scanner: rotary encoders, the
//! touch panel and the power switch.
use fujimod_catalog_wiring::{Interface, LocatedControl, Wire, WireKind};
use fujimod_emulator_devices::parts::{
    pins::Pin,
    switch_panel::{Case, Circuit, Control, Signal},
};
use fujimod_emulator_session::contact_driver::{self, Pose};
use serde_json::Value;

use crate::{
    Calibration,
    graph::{self, Nodes},
};

const GRAY: [usize; 4] = [0, 1, 3, 2];

fn pin_pairs(value: &Value) -> Option<Vec<(u8, u8)>> {
    value
        .as_array()?
        .iter()
        .map(|item| Some((item["port"].as_u64()? as u8, item["pin"].as_u64()? as u8)))
        .collect()
}

fn index_of(values: &[u8; 4], wanted: u8) -> Result<usize, String> {
    values
        .iter()
        .position(|value| *value == wanted)
        .ok_or_else(|| "Decoder phase map is not a permutation".into())
}

/// The smallest Gray-edge detent producing one distinct event per direction.
fn edge_events(values: &[u8; 4], transitions: &[u8; 16]) -> Result<(i32, [u8; 2]), String> {
    let mut sorted = *values;
    sorted.sort_unstable();
    if sorted != [0, 1, 2, 3] || transitions.iter().any(|event| *event > 2) {
        return Err("Transition input is not a four-state two-event decoder".into());
    }
    for edges in [2usize, 4] {
        let mut polarity = Vec::new();
        for direction in [-1i32, 1] {
            let mut emitted = Vec::new();
            let mut complete = true;
            for (origin, phase) in GRAY.iter().enumerate() {
                let mut previous = index_of(values, *phase as u8)?;
                let mut events = Vec::new();
                for step in 1..=edges as i32 {
                    let phase = (origin as i32 + direction * step).rem_euclid(4) as usize;
                    let current = index_of(values, GRAY[phase] as u8)?;
                    let event = transitions[4 * current + previous];
                    if event != 0 {
                        events.push(event);
                    }
                    previous = current;
                }
                if events.len() != 1 {
                    complete = false;
                    break;
                }
                emitted.extend(events);
            }
            if !complete || emitted.len() != 4 || emitted.iter().any(|event| *event != emitted[0]) {
                break;
            }
            polarity.push((direction, emitted[0]));
        }
        if let [(_, negative), (_, positive)] = polarity.as_slice()
            && {
                let mut events = [*negative, *positive];
                events.sort_unstable();
                events == [1, 2]
            }
        {
            return Ok((edges as i32, [*negative, *positive]));
        }
    }
    Err("Gray edges do not produce one distinct native event per detent".into())
}

fn signed(bytes: &[u8]) -> i64 {
    match bytes.len() {
        1 => i64::from(bytes[0] as i8),
        _ => i64::from(i16::from_le_bytes([bytes[0], bytes[1]])),
    }
}

fn unsigned(bytes: &[u8]) -> i64 {
    bytes
        .iter()
        .rev()
        .fold(0i64, |value, byte| (value << 8) | i64::from(*byte))
}

fn analog_encoder(
    id: &str,
    sources: [&WireKind; 2],
    nodes: &Nodes,
    calibration: Option<&dyn Calibration>,
) -> Result<contact_driver::Interface, String> {
    let [
        WireKind::AdcTransition {
            connection_id,
            channels,
            phase_map,
            transitions,
            event: first_event,
            calibration_offsets,
        },
        WireKind::AdcTransition {
            connection_id: second_connection,
            channels: second_channels,
            phase_map: second_map,
            transitions: second_transitions,
            event: second_event,
            calibration_offsets: second_offsets,
        },
    ] = sources
    else {
        return Err("Analog rotary directions use different decoder kinds".into());
    };
    if (
        channels,
        connection_id,
        phase_map,
        transitions,
        calibration_offsets,
    ) != (
        second_channels,
        second_connection,
        second_map,
        second_transitions,
        second_offsets,
    ) {
        return Err("Analog rotary directions use different physical decoders".into());
    }
    let mut events = [*first_event, *second_event];
    events.sort_unstable();
    if events != [1, 2] || *phase_map != [0, 1, 2, 3] {
        return Err("Analog rotary direction events or raw phase map differ".into());
    }
    let calibration = calibration.ok_or("Analog rotary calibration bytes are unavailable")?;
    let value = |offset: u16, width: usize, is_signed: bool| -> Result<i64, String> {
        let bytes = calibration
            .bytes(u32::from(offset), width)
            .ok_or("Analog rotary calibration offset escapes its declared image")?;
        Ok(if is_signed {
            signed(&bytes)
        } else {
            unsigned(&bytes)
        })
    };
    let tags = calibration_offsets;
    let adjustment = value(tags.offset, 2, true)?.clamp(-1023, 1023);
    let hysteresis = value(tags.hysteresis, 1, false)?;
    let centers = tags
        .centers
        .iter()
        .map(|offset| Ok((value(*offset, 2, false)? & 1023) + adjustment))
        .collect::<Result<Vec<_>, String>>()?;
    if centers.iter().any(|center| {
        !(0 <= center - hysteresis
            && center - hysteresis < center + hysteresis
            && center + hysteresis <= 1023)
    }) {
        return Err("Analog rotary Schmitt bands are outside the converter range".into());
    }
    let edges = value(tags.edges_per_step, 1, false)?;
    if !(1..=64).contains(&edges) {
        return Err("Analog rotary edge accumulator exceeds its signed counter bound".into());
    }
    let direction = if value(tags.direction, 1, false)? == 1 {
        -1
    } else {
        1
    };
    let matching: Vec<&Value> = nodes
        .all
        .iter()
        .copied()
        .filter(|node| {
            let outputs = &node["model"]["options"]["outputs"];
            graph::kind(node) == "encoder"
                && outputs["kind"].as_str() == Some("adc")
                && outputs["channels"].as_array().is_some_and(|wired| {
                    wired.iter().map(Value::as_u64).collect::<Vec<_>>()
                        == channels
                            .iter()
                            .map(|c| Some(u64::from(*c)))
                            .collect::<Vec<_>>()
                })
        })
        .collect();
    let [node] = matching.as_slice() else {
        return Err("Analog rotary channels do not select one native encoder".into());
    };
    let levels = node["model"]["options"]["outputs"]["levels"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let margin = levels.len() == 2
        && centers.iter().zip(&levels).all(|(center, level)| {
            let low = level["low"].as_i64().unwrap_or(-1);
            let high = level["high"].as_i64().unwrap_or(-1);
            0 <= low && low < center - hysteresis && center + hysteresis < high && high <= 1023
        });
    if !margin {
        return Err(
            "Analog rotary output codes do not have margin across both Schmitt bands".into(),
        );
    }
    let mut polarity = Vec::new();
    for sign in [-1i32, 1] {
        let mut emitted: Vec<u8> = (0..4)
            .map(|index: i32| {
                let next = GRAY[(index + sign).rem_euclid(4) as usize];
                transitions[4 * next + GRAY[index as usize]]
            })
            .collect();
        emitted.sort_unstable();
        emitted.dedup();
        let [event] = emitted.as_slice() else {
            return Err(
                "Analog rotary phase edges do not have one consistent native direction".into(),
            );
        };
        if !(1..=2).contains(event) {
            return Err(
                "Analog rotary phase edges do not have one consistent native direction".into(),
            );
        }
        polarity.push((sign, u32::from(*event)));
    }
    let desired = if direction == 1 {
        *second_event
    } else {
        3 - *second_event
    };
    let signs: Vec<i32> = polarity
        .iter()
        .filter(|(_, event)| *event == desired)
        .map(|(sign, _)| *sign)
        .collect();
    let [sign] = signs.as_slice() else {
        return Err("Analog rotary events do not distinguish physical direction".into());
    };
    Ok(contact_driver::Interface::Rotary {
        id: id.into(),
        node: graph::name(node),
        edges_per_step: sign * edges as i32,
    })
}

fn pins_of(wire: &WireKind) -> Option<&Vec<(u8, u8)>> {
    match wire {
        WireKind::GpioQuadrature { pins, .. }
        | WireKind::GpioFixedCycle { pins, .. }
        | WireKind::GpioTransition { pins, .. } => Some(pins),
        _ => None,
    }
}

/// The rotary encoder a control's decrease and increase actions name, when
/// both are decoder inputs.
pub fn encoder(
    row: &LocatedControl,
    nodes: &Nodes,
    calibration: Option<&dyn Calibration>,
) -> Result<Option<contact_driver::Interface>, String> {
    let mut sources: Vec<&Wire> = Vec::new();
    for action in ["decrease", "increase"] {
        let Some(keys) = row.action(action) else {
            return Ok(None);
        };
        if keys.is_empty() {
            return Ok(None);
        }
        let wires: Vec<&Wire> = keys
            .iter()
            .flat_map(|key| row.input.get(key).into_iter().flatten())
            .collect();
        let Some(first) = wires.first() else {
            return Ok(None);
        };
        if wires.iter().any(|wire| *wire != *first)
            || !matches!(
                first.kind,
                WireKind::GpioQuadrature { .. }
                    | WireKind::GpioFixedCycle { .. }
                    | WireKind::GpioTransition { .. }
                    | WireKind::AdcTransition { .. }
            )
        {
            return Ok(None);
        }
        sources.push(first);
    }
    let [first, second] = [&sources[0].kind, &sources[1].kind];
    if matches!(first, WireKind::AdcTransition { .. })
        || matches!(second, WireKind::AdcTransition { .. })
    {
        return analog_encoder(&row.id, [first, second], nodes, calibration).map(Some);
    }
    let first_pins = pins_of(first).cloned();
    let matching: Vec<&Value> = nodes
        .all
        .iter()
        .copied()
        .filter(|node| {
            graph::kind(node) == "encoder"
                && pin_pairs(&node["model"]["options"]["pins"]) == first_pins
        })
        .collect();
    let [node] = matching.as_slice() else {
        return Err("Located rotary pins do not select one native encoder".into());
    };
    let rotary = |edges_per_step: i32| {
        Some(contact_driver::Interface::Rotary {
            id: row.id.clone(),
            node: graph::name(node),
            edges_per_step,
        })
    };
    match (first, second) {
        (
            WireKind::GpioTransition {
                pins,
                connection_id,
                phase_map,
                transitions,
                event,
                guard,
                threshold,
            },
            WireKind::GpioTransition {
                pins: second_pins,
                connection_id: second_connection,
                phase_map: second_map,
                transitions: second_transitions,
                event: second_event,
                guard: second_guard,
                threshold: second_threshold,
            },
        ) => {
            if (
                pins,
                connection_id,
                phase_map,
                transitions,
                guard,
                threshold,
            ) != (
                second_pins,
                second_connection,
                second_map,
                second_transitions,
                second_guard,
                second_threshold,
            ) {
                return Err("Rotary directions use different physical transition decoders".into());
            }
            let mut events = [*event, *second_event];
            events.sort_unstable();
            if *threshold != 1 || events != [1, 2] {
                return Err("Unsupported transition debounce or direction events".into());
            }
            let (edges, [negative, positive]) = edge_events(phase_map, transitions)?;
            let sign = if u32::from(negative) == *second_event {
                -1
            } else if u32::from(positive) == *second_event {
                1
            } else {
                return Err("Unsupported transition debounce or direction events".into());
            };
            Ok(rotary(sign * edges))
        }
        (WireKind::GpioTransition { .. }, _) | (_, WireKind::GpioTransition { .. }) => {
            Err("Rotary directions use different physical decoder kinds".into())
        }
        (
            WireKind::GpioFixedCycle {
                pins,
                connection_id,
                cycle_sign,
            },
            WireKind::GpioFixedCycle {
                pins: second_pins,
                connection_id: second_connection,
                cycle_sign: second_sign,
            },
        ) => {
            if pins != second_pins
                || connection_id != second_connection
                || ![-1, 1].contains(cycle_sign)
                || ![-1, 1].contains(second_sign)
                || *cycle_sign != -*second_sign
            {
                return Err("Rotary directions require opposite checked electrical cycles".into());
            }
            Ok(rotary(i32::from(*second_sign) * 4))
        }
        (WireKind::GpioFixedCycle { .. }, _) | (_, WireKind::GpioFixedCycle { .. }) => {
            Err("Rotary directions require opposite checked electrical cycles".into())
        }
        (
            WireKind::GpioQuadrature {
                pins,
                phase_map,
                transitions,
                reset,
                event,
                guard,
                threshold,
                ..
            },
            WireKind::GpioQuadrature {
                pins: second_pins,
                phase_map: second_map,
                transitions: second_transitions,
                reset: second_reset,
                event: second_event,
                guard: second_guard,
                threshold: second_threshold,
                ..
            },
        ) => {
            if (pins, phase_map, transitions, reset, guard, threshold)
                != (
                    second_pins,
                    second_map,
                    second_transitions,
                    second_reset,
                    second_guard,
                    second_threshold,
                )
            {
                return Err("Rotary directions use different physical decoders".into());
            }
            if *threshold != 1 || event == second_event {
                return Err("Unsupported rotary debounce or direction events".into());
            }
            // The native encoder's positive cycle is 00,01,11,10,00. Count the
            // events the firmware's permutation, transition and reset tables
            // dispatch; never assume that a positive cycle means clockwise.
            let emitted = |phases: [u8; 4]| -> Result<Vec<u32>, String> {
                let mut old = index_of(phase_map, 0)?;
                let mut latched = false;
                let mut events = Vec::new();
                for phase in phases {
                    let sample = index_of(phase_map, phase)?;
                    if reset[sample] == 1 {
                        latched = false;
                    }
                    let event = u32::from(transitions[sample * 4 + old]);
                    if event != 0 && !latched {
                        events.push(event);
                        latched = true;
                    }
                    old = sample;
                }
                Ok(events)
            };
            let positive = emitted([1, 3, 2, 0])?;
            let negative = emitted([2, 3, 1, 0])?;
            let sign = if positive == [*second_event] && negative == [*event] {
                1
            } else if negative == [*second_event] && positive == [*event] {
                -1
            } else {
                return Err(
                    "A rotary cycle does not produce one distinct event in each direction".into(),
                );
            };
            Ok(rotary(sign * 4))
        }
        _ => Err("Rotary directions use different physical decoder kinds".into()),
    }
}

/// The touch panel endpoint a control's interface names.
pub fn touch(
    row: &LocatedControl,
    nodes: &Nodes,
    calibration: Option<&dyn Calibration>,
) -> Result<Option<contact_driver::Interface>, String> {
    let Some(Interface::PrefixedTouch {
        register,
        width,
        height,
        offset_tags,
        irq,
        address,
        ..
    }) = &row.interface
    else {
        return Ok(None);
    };
    let matching: Vec<&Value> = nodes
        .all
        .iter()
        .copied()
        .filter(|node| {
            let parts = &node["model"]["options"]["parts"];
            let touch = &parts["touch"];
            graph::kind(node) == "i2c"
                && parts["kind"].as_str() == Some("primary")
                && parts["prefix_address"].as_u64() == Some(u64::from(*address))
                && !touch.is_null()
                && touch["register"].as_u64() == Some(u64::from(*register))
                && touch["irq"].as_u64() == Some(u64::from(*irq))
        })
        .collect();
    let [node] = matching.as_slice() else {
        return Err("Located touch endpoint and IRQ do not select one native part".into());
    };
    let calibration = calibration.ok_or("Touch coordinate calibration is unavailable")?;
    let mut offsets = [0i16; 2];
    for (slot, tag) in offsets.iter_mut().zip(offset_tags) {
        let bytes = calibration
            .bytes(*tag, 1)
            .ok_or("Touch coordinate calibration is unavailable")?;
        *slot = i16::from(bytes[0] as i8);
    }
    Ok(Some(contact_driver::Interface::Touch {
        id: row.id.clone(),
        node: graph::name(node),
        width: u16::try_from(*width).map_err(|_| "Touch width exceeds 16 bits")?,
        height: u16::try_from(*height).map_err(|_| "Touch height exceeds 16 bits")?,
        offsets,
    }))
}

/// The power switch as one positional control driving its GPIO and its
/// microcontroller status bits together.
pub struct Power {
    pub control: Control,
    pub circuit: Circuit,
    pub actuator: contact_driver::Actuator,
}

pub fn power(row: &LocatedControl, nodes: &Nodes) -> Result<Option<Power>, String> {
    let Some(Interface::PowerSwitch {
        gpio_base,
        port,
        pin,
        on_level,
        register,
        mask,
        on_value,
        off_value,
        controller_id,
        ..
    }) = &row.interface
    else {
        return Ok(None);
    };
    let pads: Vec<&Value> = nodes
        .all
        .iter()
        .copied()
        .filter(|node| {
            graph::kind(node) == "gpio"
                && node["windows"].as_array().is_some_and(|windows| {
                    windows
                        .iter()
                        .any(|window| window["base"].as_u64() == Some(*gpio_base))
                })
        })
        .collect();
    let candidates: Vec<(&Value, graph::SerialRoute)> =
        graph::serial_routes(&nodes.all, &["registers", "wake-registers"])
            .into_iter()
            .filter(|(node, route)| {
                controller_id
                    .as_ref()
                    .is_none_or(|id| graph::name(node).replace('_', "-") == *id)
                    && route.readable
                    && route
                        .status_registers
                        .iter()
                        .any(|s| u32::from(*s) == *register)
            })
            .collect();
    let ([pad], [(node, route)]) = (pads.as_slice(), candidates.as_slice()) else {
        return Err("Power switch has no unique GPIO and serial status input".into());
    };
    let levels: Vec<u64> = pad["model"]["options"]["input_levels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect();
    let level = levels
        .get(usize::from(*port))
        .map(|word| (word >> pin) & 1)
        .ok_or("Power switch GPIO is outside the body")?;
    let status = route
        .registers
        .iter()
        .find(|(known, _)| u32::from(*known) == *register)
        .map(|(_, value)| value & u64::from(*mask))
        .ok_or("Power switch status register has no initial value")?;
    if (level == u64::from(*on_level)) != (status == u64::from(*on_value)) {
        return Err("Power switch initial GPIO and serial status disagree".into());
    }
    let register = u8::try_from(*register).map_err(|_| "Power switch register exceeds 8 bits")?;
    let mask8 = u8::try_from(*mask).map_err(|_| "Power switch mask exceeds 8 bits")?;
    let cases = ["ON", "OFF"]
        .into_iter()
        .map(|position| {
            let on = position == "ON";
            Ok(Case {
                positions: vec![position.into()],
                signals: vec![
                    Signal::Gpio {
                        node: graph::name(pad),
                        pin: Pin {
                            port: *port,
                            pin: *pin,
                        },
                        high: if on { *on_level != 0 } else { *on_level == 0 },
                    },
                    Signal::SerialStatus {
                        node: graph::name(node),
                        route: route.index,
                        register,
                        mask: u16::from(mask8),
                        value: u16::try_from(if on { *on_value } else { *off_value })
                            .map_err(|_| "Power switch status value exceeds 16 bits")?,
                    },
                ],
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Some(Power {
        control: Control {
            id: row.id.clone(),
            label: row.name.clone(),
            kind: row.kind.clone(),
            positions: vec!["ON".into(), "OFF".into()],
            initial: if level == u64::from(*on_level) {
                "ON"
            } else {
                "OFF"
            }
            .into(),
            unavailable: None,
        },
        circuit: Circuit {
            controls: vec![row.id.clone()],
            cases,
        },
        actuator: contact_driver::Actuator {
            id: row.id.clone(),
            momentary: false,
            states: ["ON", "OFF"]
                .into_iter()
                .map(|position| Pose {
                    position: position.into(),
                    ready: Vec::new(),
                    expected: Vec::new(),
                    hold_ticks: 0,
                })
                .collect(),
        },
    }))
}
