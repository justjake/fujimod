//! A body's ladder controls as switch-panel circuits, so hosts press, hold,
//! set and turn them through the session like every other body.
//!
//! Each wire becomes one circuit over every control that reads it. A case
//! lists the level the wire presents for one combination of held keys and
//! latched positions; a combination without a declared band has no case and
//! is refused. Rungs, contact sets and position bands come from the body's
//! [`crate::ladders::Options`]; the level a wire reads with nothing held comes
//! from the run.
use std::collections::BTreeMap;

use crate::{
    keys::Key,
    ladders::{CommandDial, Ladder, Options, PositionDial, Wire},
};
use fujimod_emulator_devices::parts::{
    pins::Pin,
    switch_panel::{Case, Circuit, Control, Signal, Spec},
};
use fujimod_emulator_session::contact_driver::{
    self, Actuator, ControlChoice, ControlInput, Interface, Pose,
};

/// Highest value a power-controller ladder register reads.
const REGISTER_MASK: u16 = 0x3ff;

/// The session nodes the panel drives.
pub struct Nodes {
    pub adc: String,
    pub gpio: String,
    /// The SPI controller node and attachment index of the power controller.
    pub power: (String, usize),
    /// The rear and front command dials' encoder nodes, for quadrature dials.
    pub rear_dial: Option<String>,
    pub front_dial: Option<String>,
}

/// Catalog identities and labels of the body's controls.
pub struct Names {
    pub keys: BTreeMap<Key, (String, String)>,
    /// Latched dials and register switches by model name.
    pub positions: BTreeMap<String, (String, String)>,
    /// The shutter button and its half- and full-press pads.
    pub shutter: Option<((String, String), [Pin; 2])>,
    pub rear_dial: Option<(String, String)>,
    pub front_dial: Option<(String, String)>,
}

/// What a running session reads to start the panel.
pub struct Idle<'a> {
    /// The ADC's current samples.
    pub adc: &'a [u16],
    /// The power controller's current value of a register.
    pub register: &'a dyn Fn(u8) -> Result<u16, String>,
}

pub use fujimod_emulator_session::inputs::Panel;

const RELEASED: &str = "released";
const PRESSED: &str = "pressed";

fn named<'a>(
    names: &'a BTreeMap<Key, (String, String)>,
    key: Key,
) -> Result<&'a (String, String), String> {
    names
        .get(&key)
        .ok_or_else(|| format!("{key:?} has no catalog control"))
}

fn button(id: &str, label: &str) -> Control {
    Control {
        id: id.into(),
        label: label.into(),
        kind: "button".into(),
        positions: vec![RELEASED.into(), PRESSED.into()],
        initial: RELEASED.into(),
        unavailable: None,
    }
}

/// The value a ladder reads for the held keys while its dial is at `position`.
fn level(ladder: &Ladder, held: &[Key], dial: Option<&DialPosition>, idle: u16) -> Option<u16> {
    if held.is_empty() {
        return Some(dial.map_or(idle, |p| p.sample));
    }
    let contact = dial.filter(|p| p.contact).map(|p| p.label.as_str());
    if contact.is_none() && held.len() == 1 {
        return ladder
            .rungs
            .iter()
            .find(|r| r.key == held[0])
            .map(|r| r.sample);
    }
    ladder
        .sets
        .iter()
        .find(|set| {
            set.position.as_deref() == contact
                && set.keys.len() == held.len()
                && set.keys.iter().all(|k| held.contains(k))
        })
        .map(|set| set.sample)
}

struct DialPosition {
    label: String,
    sample: u16,
    contact: bool,
}

fn dial_positions(dial: &PositionDial) -> Vec<DialPosition> {
    dial.positions
        .iter()
        .map(|p| DialPosition {
            label: p.label.clone(),
            sample: p.sample,
            contact: p.contact,
        })
        .collect()
}

fn dial_control(dial: &PositionDial, names: &Names, idle: u16) -> Result<Control, String> {
    let (id, label) = names
        .positions
        .get(&dial.name)
        .ok_or_else(|| format!("dial {} has no catalog control", dial.name))?;
    let initial = dial
        .positions
        .iter()
        .find(|p| (p.band_low..=p.band_high).contains(&idle))
        .ok_or_else(|| format!("{id} reads {idle}, outside every position"))?;
    Ok(Control {
        id: id.clone(),
        label: label.clone(),
        kind: "dial".into(),
        positions: dial.positions.iter().map(|p| p.label.clone()).collect(),
        initial: initial.label.clone(),
        unavailable: None,
    })
}

/// One wire's circuit: every key on it and the dial sharing it.
fn ladder_circuit(
    ladder: &Ladder,
    options: &Options,
    names: &Names,
    nodes: &Nodes,
    idle: &Idle<'_>,
    controls: &mut Vec<Control>,
) -> Result<Circuit, String> {
    let keys: Vec<Key> = ladder.rungs.iter().map(|r| r.key).collect();
    let (dial, released, signal): (Option<&PositionDial>, u16, Box<dyn Fn(u16) -> Signal>) =
        match ladder.wire {
            Wire::Adc { channel } => {
                let idle = *idle
                    .adc
                    .get(usize::from(channel))
                    .ok_or_else(|| format!("the ADC has no channel {channel}"))?;
                let node = nodes.adc.clone();
                (
                    options.dials.iter().find(|d| d.channel == channel),
                    idle,
                    Box::new(move |value| Signal::Analog {
                        node: node.clone(),
                        channel: usize::from(channel),
                        value,
                    }),
                )
            }
            Wire::PowerControllerRegister { register, released } => {
                let (node, route) = nodes.power.clone();
                (
                    None,
                    released,
                    Box::new(move |value| Signal::SerialStatus {
                        node: node.clone(),
                        route,
                        register,
                        mask: REGISTER_MASK,
                        value,
                    }),
                )
            }
        };
    let mut ids = Vec::new();
    for &key in &keys {
        let (id, label) = named(&names.keys, key)?;
        controls.push(button(id, label));
        ids.push(id.clone());
    }
    let positions = match dial {
        Some(dial) => {
            controls.push(dial_control(dial, names, released)?);
            ids.push(controls.last().unwrap().id.clone());
            dial_positions(dial).into_iter().map(Some).collect()
        }
        None => vec![None],
    };
    if keys.len() > 12 {
        return Err("a ladder with more than 12 keys has too many contact combinations".into());
    }
    let mut cases = Vec::new();
    for position in &positions {
        for held_mask in 0u32..1 << keys.len() {
            let held: Vec<Key> = (0..keys.len())
                .filter(|i| held_mask & (1 << i) != 0)
                .map(|i| keys[i])
                .collect();
            let Some(value) = level(ladder, &held, position.as_ref(), released) else {
                continue;
            };
            let mut pose: Vec<String> = (0..keys.len())
                .map(|i| {
                    if held_mask & (1 << i) != 0 {
                        PRESSED
                    } else {
                        RELEASED
                    }
                    .into()
                })
                .collect();
            if let Some(position) = position {
                pose.push(position.label.clone());
            }
            cases.push(Case {
                positions: pose,
                signals: vec![signal(value)],
            });
        }
    }
    Ok(Circuit {
        controls: ids,
        cases,
    })
}

/// Compile the body's ladder controls for a session with `nodes`, starting
/// from the levels the run reads in `idle`.
pub fn compile(
    options: &Options,
    names: &Names,
    nodes: &Nodes,
    idle: &Idle<'_>,
    ticks_per_second: u64,
) -> Result<Panel, String> {
    let mut controls = Vec::new();
    let mut circuits = Vec::new();
    for ladder in &options.ladders {
        circuits.push(ladder_circuit(
            ladder,
            options,
            names,
            nodes,
            idle,
            &mut controls,
        )?);
    }
    for dial in &options.dials {
        if options.ladders.iter().any(|l| {
            l.wire
                == Wire::Adc {
                    channel: dial.channel,
                }
        }) {
            continue;
        }
        let sample = *idle
            .adc
            .get(usize::from(dial.channel))
            .ok_or_else(|| format!("the ADC has no channel {}", dial.channel))?;
        let control = dial_control(dial, names, sample)?;
        circuits.push(Circuit {
            controls: vec![control.id.clone()],
            cases: dial
                .positions
                .iter()
                .map(|p| Case {
                    positions: vec![p.label.clone()],
                    signals: vec![Signal::Analog {
                        node: nodes.adc.clone(),
                        channel: usize::from(dial.channel),
                        value: p.sample,
                    }],
                })
                .collect(),
        });
        controls.push(control);
    }
    for switch in &options.switches {
        let (id, label) = names
            .positions
            .get(&switch.name)
            .ok_or_else(|| format!("switch {} has no catalog control", switch.name))?;
        let bit = 1u16 << switch.bit;
        let set = (idle.register)(switch.register)? & bit != 0;
        let (node, route) = nodes.power.clone();
        controls.push(Control {
            id: id.clone(),
            label: label.clone(),
            kind: "switch".into(),
            positions: vec![switch.clear.clone(), switch.set.clone()],
            initial: if set {
                switch.set.clone()
            } else {
                switch.clear.clone()
            },
            unavailable: None,
        });
        circuits.push(Circuit {
            controls: vec![id.clone()],
            cases: [(&switch.clear, 0), (&switch.set, bit)]
                .into_iter()
                .map(|(position, value)| Case {
                    positions: vec![position.clone()],
                    signals: vec![Signal::SerialStatus {
                        node: node.clone(),
                        route,
                        register: switch.register,
                        mask: bit,
                        value,
                    }],
                })
                .collect(),
        });
    }
    let gpio = |pin: Pin, high: bool| Signal::Gpio {
        node: nodes.gpio.clone(),
        pin,
        high,
    };
    for key in &options.gpio_keys {
        let (id, label) = named(&names.keys, key.key)?;
        controls.push(button(id, label));
        circuits.push(Circuit {
            controls: vec![id.clone()],
            cases: vec![
                Case {
                    positions: vec![RELEASED.into()],
                    signals: vec![gpio(key.pin, !key.active_high)],
                },
                Case {
                    positions: vec![PRESSED.into()],
                    signals: vec![gpio(key.pin, key.active_high)],
                },
            ],
        });
    }
    if let Some(((id, label), [half, full])) = &names.shutter {
        controls.push(Control {
            id: id.clone(),
            label: label.clone(),
            kind: "shutter".into(),
            positions: vec![RELEASED.into(), "half".into(), "full".into()],
            initial: RELEASED.into(),
            unavailable: None,
        });
        circuits.push(Circuit {
            controls: vec![id.clone()],
            cases: [
                (RELEASED, true, true),
                ("half", false, true),
                ("full", false, false),
            ]
            .into_iter()
            .map(|(position, half_high, full_high)| Case {
                positions: vec![position.into()],
                signals: vec![gpio(*half, half_high), gpio(*full, full_high)],
            })
            .collect(),
        });
    }
    // Firmware scanner records are not located for these controls, so a
    // press holds for a few key scans instead of waiting for an acknowledgment.
    let hold_ticks = (ticks_per_second / 10).max(1);
    let mut catalog = Vec::new();
    let mut actuators = Vec::new();
    for control in &controls {
        let momentary = matches!(control.kind.as_str(), "button" | "shutter");
        catalog.push(ControlInput {
            id: control.id.clone(),
            label: control.label.clone(),
            kind: control.kind.clone(),
            supported: true,
            unavailable: None,
            positions: (!momentary).then(|| {
                control
                    .positions
                    .iter()
                    .map(|p| ControlChoice {
                        id: p.clone(),
                        label: p.clone(),
                    })
                    .collect()
            }),
            initial: (!momentary).then(|| control.initial.clone()),
            relative: None,
            width: None,
            height: None,
            stages: (control.kind == "shutter").then(|| {
                control.positions[1..]
                    .iter()
                    .map(|p| ControlChoice {
                        id: p.clone(),
                        label: p.clone(),
                    })
                    .collect()
            }),
        });
        actuators.push(Actuator {
            id: control.id.clone(),
            momentary,
            states: control
                .positions
                .iter()
                .map(|p| Pose {
                    position: p.clone(),
                    ready: Vec::new(),
                    expected: Vec::new(),
                    hold_ticks: if momentary { hold_ticks } else { 0 },
                })
                .collect(),
        });
    }
    let mut interfaces = Vec::new();
    let mut rotary =
        |names: &Option<(String, String)>, node: Option<&String>| -> Result<(), String> {
            let (Some((id, label)), Some(node)) = (names, node) else {
                return Ok(());
            };
            catalog.push(ControlInput {
                id: id.clone(),
                label: label.clone(),
                kind: "rotary".into(),
                supported: true,
                unavailable: None,
                positions: None,
                initial: None,
                relative: Some(true),
                width: None,
                height: None,
                stages: None,
            });
            interfaces.push(Interface::Rotary {
                id: id.clone(),
                node: node.clone(),
                edges_per_step: 1,
            });
            Ok(())
        };
    match &options.rear_dial {
        Some(CommandDial::Counting(_)) => rotary(&names.rear_dial, Some(&nodes.power.0))?,
        Some(CommandDial::Quadrature(_)) => rotary(&names.rear_dial, nodes.rear_dial.as_ref())?,
        None => {}
    }
    if options.front_dial.is_some() {
        rotary(&names.front_dial, nodes.front_dial.as_ref())?;
    }
    Ok(Panel {
        wiring: Spec { controls, circuits },
        confirmation: contact_driver::Spec {
            poll_ticks: (ticks_per_second / 1000).max(1),
            max_phase_ticks: ticks_per_second * 3,
            catalog,
            actuators,
            interfaces,
        },
    })
}

/// Connect the body's ladder controls to an opened XP5 session: compile the
/// panel from the run's current levels, install it and its input driver, and
/// send `inputs`' key edges and positions to it.
pub fn connect(
    session: &mut fujimod_emulator_session::Session,
    inputs: &mut crate::session::Inputs,
    options: Options,
    names: Names,
    gpio: String,
    ticks_per_second: u64,
) -> Result<(), String> {
    use fujimod_emulator_xp5_io::spi::Spi;
    let adc = format!("{:#x}", fujimod_emulator_xp5_io::adc::BASE);
    let mut power: Option<(usize, String, usize)> = None;
    for (name, &id) in &session.nodes {
        if let Ok(Some(route)) = session
            .board
            .with_device::<Spi, _>(id, |spi| spi.part_route("power-controller"))
        {
            match &power {
                Some((other, ..)) if *other != id => {
                    return Err("more than one XP5 power controller is attached".into());
                }
                Some(_) => {}
                None => power = Some((id, name.clone(), route)),
            }
        }
    }
    let (_, node, route) =
        power.ok_or("the XP5 board has no power controller for its ladder controls")?;
    let power = (node, route);
    let nodes = Nodes {
        adc: adc.clone(),
        gpio,
        power,
        rear_dial: matches!(options.rear_dial, Some(CommandDial::Quadrature(_)))
            .then(|| crate::boot::REAR_DIAL_NODE.to_owned()),
        front_dial: options
            .front_dial
            .is_some()
            .then(|| crate::boot::FRONT_DIAL_NODE.to_owned()),
    };
    let samples = session.adc_samples(&adc)?;
    let spi = session.node(&nodes.power.0)?;
    let route = nodes.power.1;
    let board = &session.board;
    let register = |register: u8| -> Result<u16, String> {
        board.inspect_capability::<dyn fujimod_emulator_devices::capability::SerialStatus, _>(
            spi,
            |serial| serial.status_value(route, register),
        )
    };
    let panel = compile(
        &options,
        &names,
        &nodes,
        &Idle {
            adc: &samples,
            register: &register,
        },
        ticks_per_second,
    )?;
    session.install_panel(panel)?;
    inputs.use_panel(crate::session::PanelInputs {
        options,
        names,
        ticks_per_second,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_catalog_resolution::ResolvedConfiguration;
    use fujimod_emulator_devices::parts::switch_panel::SwitchPanel;

    fn xt5() -> Panel {
        let resolved = ResolvedConfiguration::bundled("xt5-431").unwrap();
        let options = crate::ladder_controls::ladders(&resolved).unwrap().unwrap();
        let names = crate::ladder_controls::panel_names(&resolved).unwrap();
        // The run's idle ADC levels: every dial at its stock position, every ladder released.
        let mut adc = vec![4095u16; 15];
        adc[3] = 3500;
        adc[7] = 2200;
        adc[12] = 4050;
        adc[13] = 4000;
        let nodes = Nodes {
            adc: "adc".into(),
            gpio: "gpio".into(),
            power: ("spi".into(), 2),
            rear_dial: None,
            front_dial: Some("front-dial".into()),
        };
        compile(
            &options,
            &names,
            &nodes,
            &Idle {
                adc: &adc,
                register: &|_| Ok(0),
            },
            1_000_000_000,
        )
        .unwrap()
    }

    fn xh2s(adc3: u16) -> Panel {
        let resolved = ResolvedConfiguration::bundled("xh2s-730").unwrap();
        let options = crate::ladder_controls::ladders(&resolved).unwrap().unwrap();
        let names = crate::ladder_controls::panel_names(&resolved).unwrap();
        let mut adc = [4095; 15];
        adc[3] = adc3;
        let nodes = Nodes {
            adc: "adc".into(),
            gpio: "gpio".into(),
            power: ("spi".into(), 0),
            rear_dial: Some("rear-dial".into()),
            front_dial: Some("front-dial".into()),
        };
        compile(
            &options,
            &names,
            &nodes,
            &Idle {
                adc: &adc,
                register: &|_| Ok(0),
            },
            1_000_000_000,
        )
        .unwrap()
    }

    fn analog(channel: usize, value: u16) -> Signal {
        Signal::Analog {
            node: "adc".into(),
            channel,
            value,
        }
    }

    #[test]
    fn xh2s_buttons_use_catalog_ladders_and_the_record_pad() {
        let switches = SwitchPanel::new(xh2s(3774).wiring).unwrap();
        for (id, channel, sample) in [
            ("menu_ok_button", 6, 391),
            ("disp_back_button", 6, 2869),
            ("quick_menu_button", 6, 3439),
            ("ael_button", 6, 2230),
            ("playback_button", 5, 3439),
            ("af_on_button", 11, 2869),
            ("fn1_button", 12, 2869),
            ("fn2_button", 12, 2230),
            ("fn3_button", 11, 2230),
        ] {
            let pressed = switches.transition(id, "pressed").unwrap();
            assert!(
                pressed
                    .signals()
                    .unwrap()
                    .contains(&analog(channel, sample)),
                "{id}"
            );
            let released = pressed.transition(id, "released").unwrap();
            assert!(
                released.signals().unwrap().contains(&analog(channel, 4095)),
                "{id}"
            );
        }
        let record_pad = |high| Signal::Gpio {
            node: "gpio".into(),
            pin: Pin { port: 0, pin: 13 },
            high,
        };
        let pressed = switches
            .transition("movie_recording_button", "pressed")
            .unwrap();
        assert!(pressed.signals().unwrap().contains(&record_pad(false)));
        assert!(switches.signals().unwrap().contains(&record_pad(true)));
    }

    #[test]
    fn xt5_controls_compile_to_one_panel() {
        let panel = xt5();
        let switches = SwitchPanel::new(panel.wiring.clone()).unwrap();
        let menu = switches.transition("menu_ok_button", "pressed").unwrap();
        assert!(menu.signals().unwrap().contains(&Signal::SerialStatus {
            node: "spi".into(),
            route: 2,
            register: 0x34,
            mask: 0x3ff,
            value: 554,
        }));
        // MENU with BACK is a declared contact set; MENU with PLAY is on another wire.
        let both = menu
            .transition("disp_back_bluetooth_button", "pressed")
            .unwrap();
        assert!(both.signals().unwrap().iter().any(|s| matches!(
            s,
            Signal::SerialStatus {
                register: 0x34,
                value: 499,
                ..
            }
        )));
        let ev = switches
            .transition("exposure_compensation_dial", "+1")
            .unwrap();
        assert!(ev.signals().unwrap().contains(&Signal::Analog {
            node: "adc".into(),
            channel: 7,
            value: 1627
        }));
        assert_eq!(switches.positions()["drive_dial"], "S");
        assert_eq!(switches.positions()["power_switch"], "ON");
        let full = switches.transition("shutter_button", "full").unwrap();
        assert!(full.signals().unwrap().contains(&Signal::Gpio {
            node: "gpio".into(),
            pin: Pin { port: 0, pin: 1 },
            high: false
        }));
        // Two selector directions without a declared set are refused.
        let up = switches
            .transition("selector_up_button", "pressed")
            .unwrap();
        assert!(up.transition("selector_down_button", "pressed").is_err());
        let ids: Vec<_> = panel
            .confirmation
            .interfaces
            .iter()
            .map(|i| i.id().to_owned())
            .collect();
        assert_eq!(ids, ["rear_command_dial", "front_command_dial"]);
    }

    #[test]
    fn xh2_controls_compile_to_one_panel() {
        let resolved = ResolvedConfiguration::bundled("xh2-530").unwrap();
        let options = crate::ladder_controls::ladders(&resolved).unwrap().unwrap();
        let names = crate::ladder_controls::panel_names(&resolved).unwrap();
        let nodes = Nodes {
            adc: "adc".into(),
            gpio: "gpio".into(),
            power: ("spi".into(), 0),
            rear_dial: None,
            front_dial: None,
        };
        let panel = compile(
            &options,
            &names,
            &nodes,
            &Idle {
                adc: &[4095; 15],
                register: &|_| Ok(0),
            },
            1_000_000_000,
        )
        .unwrap();
        let switches = SwitchPanel::new(panel.wiring.clone()).unwrap();
        let analog = |channel, value| Signal::Analog {
            node: "adc".into(),
            channel,
            value,
        };
        let menu = switches.transition("menu_ok_button", "pressed").unwrap();
        assert!(menu.signals().unwrap().contains(&analog(6, 391)));
        let back = switches.transition("disp_back_button", "pressed").unwrap();
        assert!(back.signals().unwrap().contains(&analog(6, 2869)));
        // MENU/OK with DISP/BACK has no declared band on ADC 6.
        assert!(menu.transition("disp_back_button", "pressed").is_err());
        for (id, value) in [
            ("selector_up_button", 1568),
            ("selector_right_button", 2074),
            ("selector_left_button", 2823),
            ("selector_down_button", 3380),
        ] {
            let pressed = switches.transition(id, "pressed").unwrap();
            assert!(
                pressed.signals().unwrap().contains(&analog(13, value)),
                "{id}"
            );
        }
        // S1 on GPIO 0.0 and S2 on 0.1, both active low; a full press holds S1 too.
        let pad = |pin, high| Signal::Gpio {
            node: "gpio".into(),
            pin: Pin { port: 0, pin },
            high,
        };
        let half = switches.transition("shutter_button", "half").unwrap();
        let signals = half.signals().unwrap();
        assert!(signals.contains(&pad(0, false)) && signals.contains(&pad(1, true)));
        let full = half.transition("shutter_button", "full").unwrap();
        let signals = full.signals().unwrap();
        assert!(signals.contains(&pad(0, false)) && signals.contains(&pad(1, false)));
        assert!(panel.confirmation.interfaces.is_empty());
    }

    #[test]
    fn x100vi_controls_compile_to_one_panel() {
        let resolved = ResolvedConfiguration::bundled("x100vi-131").unwrap();
        let options = crate::ladder_controls::ladders(&resolved).unwrap().unwrap();
        let names = crate::ladder_controls::panel_names(&resolved).unwrap();
        let nodes = Nodes {
            adc: "adc".into(),
            gpio: "gpio".into(),
            power: ("spi".into(), 0),
            rear_dial: Some("rear-dial".into()),
            front_dial: Some("front-dial".into()),
        };
        let panel = compile(
            &options,
            &names,
            &nodes,
            &Idle {
                adc: &[4095; 15],
                register: &|_| Ok(0),
            },
            1_000_000_000,
        )
        .unwrap();
        let switches = SwitchPanel::new(panel.wiring.clone()).unwrap();
        let menu = switches.transition("menu_ok_button", "pressed").unwrap();
        assert!(menu.signals().unwrap().contains(&Signal::Analog {
            node: "adc".into(),
            channel: 11,
            value: 3472
        }));
        let both = menu.transition("disp_back_button", "pressed").unwrap();
        assert!(both.signals().unwrap().contains(&Signal::Analog {
            node: "adc".into(),
            channel: 11,
            value: 1999
        }));
        let up = switches.transition("focus_stick.up", "pressed").unwrap();
        assert!(up.signals().unwrap().contains(&Signal::Analog {
            node: "adc".into(),
            channel: 14,
            value: 1558
        }));
        let half = switches.transition("shutter_button", "half").unwrap();
        assert!(half.signals().unwrap().contains(&Signal::Gpio {
            node: "gpio".into(),
            pin: Pin { port: 0, pin: 0 },
            high: false
        }));
        let dials: Vec<_> = panel
            .confirmation
            .interfaces
            .iter()
            .map(|interface| match interface {
                Interface::Rotary { id, node, .. } => (id.as_str(), node.as_str()),
                _ => panic!("only command dials are interfaces"),
            })
            .collect();
        assert_eq!(
            dials,
            [
                ("rear_command_dial", "rear-dial"),
                ("front_command_dial", "front-dial")
            ]
        );
    }
}
