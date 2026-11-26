//! A body's catalog ladders as the panel's ladder wiring.
//!
//! The bands, samples and evidence come from the body's physical-controls
//! record. The value each wire reads with no control held comes from the run:
//! an ADC ladder reads the ADC block input's sample on its channel, and a
//! register ladder reads the body's declared released value or, where the
//! body has none, the run configuration's ladder input. A ladder without a
//! key-free value, a control without an emulator key and a wire whose
//! resolution differs from the model's are refused, never approximated.
//!
//! The catalog's physical-controls schema has no field yet for contact sets
//! (two keys on one wire, or a key with a latched position), position-dial
//! bands, pad diagonals, command-dial wiring, push pads or switch bits. For a
//! body whose decode establishes them, [`extension`] adds them from the
//! board's own table below.

use crate::{keys::Key, ladders};
use fujimod_catalog_configuration::{
    RunInputs,
    application_inputs::{ApplicationBlockInput, ApplicationRunInputs},
};
use fujimod_catalog_controls::{ControlLadder, LadderWire, PhysicalControls};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_devices::parts::pins::Pin;

/// Catalog control identities, including pad directions, and the emulator key
/// that presses each. A ladder rung on any other identity is refused.
const CONTROL_KEYS: [(&str, Key); 24] = [
    ("menu_ok_button", Key::Menu),
    ("disp_back_bluetooth_button", Key::Back),
    ("disp_back_button", Key::Back),
    ("selector_up_button", Key::Up),
    ("selector_down_button", Key::Down),
    ("selector_left_button", Key::Left),
    ("selector_right_button", Key::Right),
    ("delete_button", Key::Delete),
    ("drive_delete_button", Key::Delete),
    ("view_mode_button", Key::ViewMode),
    ("fn1_button", Key::Fn1),
    ("fn2_button", Key::Fn2),
    ("quick_menu_button", Key::Quick),
    ("focus_stick.up", Key::StickUp),
    ("focus_stick.down", Key::StickDown),
    ("focus_stick.left", Key::StickLeft),
    ("focus_stick.right", Key::StickRight),
    ("focus_stick", Key::StickPush),
    ("playback_button", Key::Play),
    ("play_button", Key::Play),
    ("q_button", Key::Quick),
    ("ael_button", Key::AeLock),
    ("ael_afl_button", Key::AeLock),
    ("af_on_button", Key::AfOn),
];

/// The emulator key that presses a catalog control identity, if one exists.
pub fn key_for_control(control_id: &str) -> Option<Key> {
    CONTROL_KEYS
        .iter()
        .find(|(id, _)| *id == control_id)
        .map(|(_, key)| *key)
}

/// The resolved body's ladder wiring, or `None` when the body declares no
/// ladder.
pub fn ladders(resolved: &ResolvedConfiguration) -> Result<Option<ladders::Options>, String> {
    let body = &resolved.reference.body.id;
    let Some(document) = resolved.reference.physical_controls.as_ref() else {
        return Ok(None);
    };
    let controls = PhysicalControls::decode_document(document, body)?;
    controls.validate()?;
    if controls.ladders.is_empty() {
        return Ok(None);
    }
    let inputs = match resolved.configuration.inputs.as_ref() {
        Some(RunInputs::Xp5(inputs)) => inputs,
        _ => {
            return Err(format!(
                "body {body} declares ladders but the run has no XP5 inputs"
            ));
        }
    };
    let ladders = controls
        .ladders
        .iter()
        .map(|ladder| model_ladder(body, ladder, inputs))
        .collect::<Result<Vec<_>, _>>()?;
    let mut options = ladders::Options {
        ladders,
        dials: Vec::new(),
        switches: Vec::new(),
        rear_dial: None,
        front_dial: None,
        gpio_keys: Vec::new(),
    };
    extension(body, &mut options)?;
    options.validate()?;
    Ok(Some(options))
}

fn model_ladder(
    body: &str,
    ladder: &ControlLadder,
    inputs: &ApplicationRunInputs,
) -> Result<ladders::Ladder, String> {
    let wire = ladder.wire;
    let unmapped: Vec<&str> = ladder
        .rungs
        .iter()
        .map(|rung| rung.control_id.as_str())
        .filter(|id| key_for_control(id).is_none())
        .collect();
    if !unmapped.is_empty() {
        return Err(format!(
            "{body} {wire:?} rungs {} have no emulator key",
            unmapped.join(", ")
        ));
    }
    let wire = match wire {
        LadderWire::Adc { channel } => {
            let sample = adc_sample(inputs, channel)?;
            if ladder.released.is_some_and(|released| released != sample) {
                return Err(format!(
                    "{body} ADC ladder {channel} declares a released value other than the run's ADC sample {sample}"
                ));
            }
            if let Some(rung) = ladder.rungs.iter().find(|rung| rung.contains(sample)) {
                return Err(format!(
                    "{body} ADC channel {channel} sample {sample} holds {} from power-on",
                    rung.control_id
                ));
            }
            ladders::Wire::Adc { channel }
        }
        LadderWire::PowerControllerRegister { register } => {
            let supplied = inputs
                .ladders
                .iter()
                .find(|input| input.wire == wire)
                .map(|input| input.released);
            let released = match (ladder.released, supplied) {
                (Some(_), Some(_)) => {
                    return Err(format!(
                        "{body} register {register:#x} ladder has a released value from both the body and the run"
                    ));
                }
                (Some(released), None) | (None, Some(released)) => released,
                (None, None) => {
                    return Err(format!(
                        "{body} register {register:#x} ladder has no released value: the body declares none and the run supplies none"
                    ));
                }
            };
            ladders::Wire::PowerControllerRegister { register, released }
        }
    };
    Ok(ladders::Ladder {
        wire,
        levels: ladder.levels,
        rungs: ladder
            .rungs
            .iter()
            .map(|rung| ladders::Rung {
                key: key_for_control(&rung.control_id).expect("checked above"),
                band_low: rung.band_low,
                band_high: rung.band_high,
                sample: rung.sample,
                basis: rung.basis.clone(),
            })
            .collect(),
        sets: Vec::new(),
    })
}

/// Basis of every band the X-T5 table below declares.
const XT5_BANDS: &str = "X-T5 4.31 factory key-band table: configuration store halfwords read through \
0x14951b0 (factory page of the stock update, XOR 0xff), for the scanner rows of table 0x1c09ca8 and the \
ladder callbacks that accept combined presses. The sample is the band midpoint.";

fn midpoint(low: u16, high: u16) -> u16 {
    low + (high - low) / 2
}

fn positions(entries: &[(&str, &[&str], u16, u16, bool)]) -> Vec<ladders::Position> {
    entries
        .iter()
        .map(|&(label, aliases, low, high, contact)| ladders::Position {
            label: label.into(),
            aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
            band_low: low,
            band_high: high,
            sample: midpoint(low, high),
            contact,
        })
        .collect()
}

/// The board's table of decoded controls the catalog cannot yet express, by
/// body. Bodies without an entry are unchanged.
fn extension(body: &str, options: &mut ladders::Options) -> Result<(), String> {
    match body {
        "xt5" => xt5_extension(options),
        "x100vi" => x100vi_extension(options),
        _ => Ok(()),
    }
}

const X100VI_BANDS: &str = "Band of the X100VI 1.31 default configuration page, read in the emulator from \
configuration RAM 0x7508000 after cfg_initialize; research/x100vi-131/ui-path.md, physical controls.";

fn x100vi_extension(options: &mut ladders::Options) -> Result<(), String> {
    let menu = options
        .ladders
        .iter_mut()
        .find(|ladder| ladder.wire == ladders::Wire::Adc { channel: 11 })
        .ok_or("the X100VI catalog ladders lack ADC 11")?;
    // MENU/OK callback 0x14dcce0 and DISP/BACK callback 0x14dcbb8 both accept
    // [cfg3925, cfg3923] = 1887..2111, the two keys held together.
    menu.sets.push(ladders::ContactSet {
        keys: vec![Key::Menu, Key::Back],
        position: None,
        band_low: 1887,
        band_high: 2111,
        sample: midpoint(1887, 2111),
        basis: X100VI_BANDS.into(),
    });
    let dial = |pins: [Pin; 2], basis: &str| ladders::QuadratureDial {
        pins,
        rest_phase: 0,
        basis: basis.into(),
    };
    options.rear_dial = Some(ladders::CommandDial::Quadrature(dial(
        [Pin { port: 5, pin: 7 }, Pin { port: 5, pin: 9 }],
        "Scanner codes 0x7f/0x80 (UI 0x26/0x27) read GPIO 5.7 and 5.9; cfg3805 bit 3 sets the \
direction. Both pads idle low.",
    )));
    options.front_dial = Some(dial(
        [Pin { port: 5, pin: 8 }, Pin { port: 13, pin: 5 }],
        "Scanner codes 0x82/0x83 (UI 0x46/0x47) read GPIO 5.8 and 13.5; cfg3805 bit 2 sets the \
direction. Both pads idle low.",
    ));
    Ok(())
}

fn xt5_extension(options: &mut ladders::Options) -> Result<(), String> {
    let adc = |channel| move |wire: ladders::Wire| wire == ladders::Wire::Adc { channel };
    let register = |number| move |wire: ladders::Wire| matches!(wire, ladders::Wire::PowerControllerRegister { register, .. } if register == number);
    fn ladder<'a>(
        options: &'a mut ladders::Options,
        is: &dyn Fn(ladders::Wire) -> bool,
    ) -> Result<&'a mut ladders::Ladder, String> {
        options
            .ladders
            .iter_mut()
            .find(|ladder| is(ladder.wire))
            .ok_or_else(|| "the catalog ladders lack a wire the board table extends".to_owned())
    }
    let rung = |key, low, high| ladders::Rung {
        key,
        band_low: low,
        band_high: high,
        sample: midpoint(low, high),
        basis: XT5_BANDS.into(),
    };
    let set = |keys: &[Key], position: Option<&str>, low, high| ladders::ContactSet {
        keys: keys.to_vec(),
        position: position.map(str::to_owned),
        band_low: low,
        band_high: high,
        sample: midpoint(low, high),
        basis: XT5_BANDS.into(),
    };
    // Focus-lever diagonals (scanner rows 88..95, reader 0x14073cc).
    ladder(options, &adc(14))?.rungs.extend([
        rung(Key::StickDownLeft, 2326, 2617),
        rung(Key::StickDownRight, 1767, 2101),
        rung(Key::StickUpLeft, 1253, 1442),
        rung(Key::StickUpRight, 1109, 1252),
    ]);
    // ADC 1: DELETE (0..788) is exclusive, so no set names it.
    ladder(options, &adc(1))?.sets.extend([
        set(&[Key::Fn2, Key::AfOn], None, 2326, 2617),
        set(&[Key::Fn2, Key::ViewMode], None, 1880, 2101),
        set(&[Key::AfOn, Key::ViewMode], None, 1701, 1879),
    ]);
    // ADC 2: Fn1 and Q share the wire with the STILL/MOVIE lever (row 7).
    ladder(options, &adc(2))?.sets.extend([
        set(&[Key::Fn1], Some("MOVIE"), 2326, 2617),
        set(&[Key::Quick], Some("MOVIE"), 1880, 2101),
        set(&[Key::Fn1, Key::Quick], None, 1701, 1879),
        set(&[Key::Fn1, Key::Quick], Some("MOVIE"), 100, 1700),
    ]);
    // ADC 11: PLAY shares the wire with the focus-mode selector; its catalog
    // rung is PLAY with the selector at C.
    ladder(options, &adc(11))?.sets.extend([
        set(&[Key::Play], Some("S"), 1880, 2101),
        set(&[Key::Play], Some("M"), 1701, 1879),
    ]);
    ladder(options, &register(0x34))?.sets.extend([
        set(&[Key::Back, Key::AeLock], None, 583, 655),
        set(&[Key::Back, Key::Menu], None, 472, 526),
        set(&[Key::AeLock, Key::Menu], None, 427, 471),
    ]);
    ladder(options, &register(0x35))?.sets.extend([
        set(&[Key::Down, Key::Left], None, 583, 655),
        set(&[Key::Down, Key::Right], None, 472, 526),
        set(&[Key::Left, Key::Up], None, 316, 348),
        set(&[Key::Right, Key::Up], None, 283, 315),
    ]);
    let dial =
        |name: &str, aliases: &[&str], channel, entries: &[(&str, &[&str], u16, u16, bool)]| {
            ladders::PositionDial {
                name: name.into(),
                aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
                channel,
                positions: positions(entries),
                basis: XT5_BANDS.into(),
            }
        };
    options.dials = vec![
        dial(
            "drive",
            &[],
            3,
            &[
                ("PANO", &["PANORAMA"], 100, 842, false),
                ("ADV", &[], 843, 1424, false),
                ("BKT", &[], 1425, 2006, false),
                ("CH", &[], 2007, 2594, false),
                ("CL", &[], 2595, 3194, false),
                ("S", &[], 3195, 3812, false),
                ("HDR", &[], 3813, 4095, false),
            ],
        ),
        dial(
            "ev",
            &["exposure"],
            7,
            &[
                ("-3", &[], 3992, 4095, false),
                ("-2.7", &["-2 2/3"], 3783, 3991, false),
                ("-2.3", &["-2 1/3"], 3576, 3782, false),
                ("-2", &[], 3369, 3575, false),
                ("-1.7", &["-1 2/3"], 3163, 3368, false),
                ("-1.3", &["-1 1/3"], 2957, 3162, false),
                ("-1", &[], 2751, 2956, false),
                ("-0.7", &["-2/3"], 2547, 2750, false),
                ("-0.3", &["-1/3"], 2342, 2546, false),
                ("0", &[], 2138, 2341, false),
                ("+0.3", &["+1/3"], 1934, 2137, false),
                ("+0.7", &["+2/3"], 1730, 1933, false),
                ("+1", &[], 1526, 1729, false),
                ("+1.3", &["+1 1/3"], 1323, 1525, false),
                ("+1.7", &["+1 2/3"], 1119, 1322, false),
                ("+2", &[], 915, 1118, false),
                ("+2.3", &["+2 1/3"], 712, 914, false),
                ("+2.7", &["+2 2/3"], 507, 711, false),
                ("+3", &[], 303, 506, false),
                ("C", &[], 100, 302, false),
            ],
        ),
        dial(
            "mode",
            &["lever"],
            2,
            &[
                ("MOVIE", &[], 3067, 3750, true),
                ("STILL", &[], 3751, 4095, false),
            ],
        ),
        dial(
            "focus",
            &["af"],
            11,
            &[
                ("M", &["MF"], 2618, 3066, true),
                ("S", &["AF-S"], 3067, 3750, true),
                ("C", &["AF-C"], 3751, 4095, false),
            ],
        ),
        // Labels 125..160 and C are marked as guesses in the decode.
        dial(
            "iso",
            &[],
            12,
            &[
                ("A", &["AUTO"], 4005, 4095, false),
                ("C", &["CMD", "COMMAND"], 3823, 4004, false),
                ("12800", &[], 3642, 3822, false),
                ("10000", &[], 3462, 3641, false),
                ("8000", &[], 3282, 3461, false),
                ("6400", &[], 3102, 3281, false),
                ("5000", &[], 2924, 3101, false),
                ("4000", &[], 2745, 2923, false),
                ("3200", &[], 2567, 2744, false),
                ("2500", &[], 2389, 2566, false),
                ("2000", &[], 2212, 2388, false),
                ("1600", &[], 2034, 2211, false),
                ("1250", &[], 1857, 2033, false),
                ("1000", &[], 1680, 1856, false),
                ("800", &[], 1503, 1679, false),
                ("640", &[], 1326, 1502, false),
                ("500", &[], 1149, 1325, false),
                ("400", &[], 972, 1148, false),
                ("320", &[], 795, 971, false),
                ("250", &[], 618, 794, false),
                ("200", &[], 441, 617, false),
                ("160", &[], 263, 440, false),
                ("125", &[], 100, 262, false),
            ],
        ),
        // Shutter speeds: a number N below 1 s is 1/N; "1" is one second.
        dial(
            "ss",
            &["shutter"],
            13,
            &[
                ("A", &["AUTO"], 3957, 4095, false),
                ("8000", &["1/8000"], 3683, 3956, false),
                ("4000", &["1/4000"], 3418, 3682, false),
                ("2000", &["1/2000"], 3160, 3417, false),
                ("1000", &["1/1000"], 2909, 3159, false),
                ("500", &["1/500"], 2664, 2908, false),
                ("250", &["1/250"], 2423, 2663, false),
                ("125", &["1/125"], 2187, 2422, false),
                ("60", &["1/60"], 1953, 2186, false),
                ("30", &["1/30"], 1721, 1952, false),
                ("15", &["1/15"], 1492, 1720, false),
                ("8", &["1/8"], 1262, 1491, false),
                ("4", &["1/4"], 1033, 1261, false),
                ("2", &["1/2"], 803, 1032, false),
                ("1", &["1S"], 571, 802, false),
                ("T", &[], 336, 570, false),
                ("B", &[], 100, 335, false),
            ],
        ),
    ];
    // Row 0 reads POWER held while register 0x00 bit 10 is clear; that set
    // means OFF is a guess of the decode.
    options.switches = vec![ladders::RegisterSwitch {
        name: "power".into(),
        register: 0x00,
        bit: 10,
        set: "OFF".into(),
        clear: "ON".into(),
        basis:
            "Scanner row 0 (physical 0x0c, UI POWER) reads ((byte [0x1c09c99] ^ 4) >> 2) & 1, the \
copy of power-controller register 0x00 that 0x1407d2c refreshes each scan."
                .into(),
    }];
    options.rear_dial = Some(ladders::CommandDial::Counting(ladders::CountingDial {
        register: 0x36,
        basis:
            "0x1407d2c reads power-controller register 0x36 each scan through 0x13d5834: bits 2..0 \
count, bit 3 sign. The firmware never writes it, so the controller clears the count on read."
                .into(),
    }));
    options.front_dial = Some(ladders::QuadratureDial {
        pins: [Pin { port: 0, pin: 9 }, Pin { port: 0, pin: 10 }],
        rest_phase: 3,
        basis:
            "Sampler 0x140a5bc: phase = level(0.9) << 1 | level(0.10), idle 3; transition table \
0x1c0af58 steps 3, 2, 0, 1, 3 for CMD2_R (physical 0x8e) and 3, 1, 0, 2, 3 for CMD2_L (0x8f)."
                .into(),
    });
    options.gpio_keys = vec![
        ladders::GpioKey {
            key: Key::RearPush,
            pin: Pin { port: 0, pin: 13 },
            active_high: false,
            basis: "Scanner row 97 (physical 0x1e, UI 0x25) reads GPIO 0.13 low.".into(),
        },
        ladders::GpioKey {
            key: Key::FrontPush,
            pin: Pin { port: 0, pin: 14 },
            active_high: false,
            basis: "Scanner row 98 (physical 0x1f, UI 0x44) reads GPIO 0.14 low.".into(),
        },
    ];
    Ok(())
}

/// Catalog identities of the board table's controls that are not a catalog
/// rung: pad diagonals, dial pushes, latched dials and switches, by body.
fn extension_ids(
    body: &str,
) -> (
    &'static [(Key, &'static str, &'static str)],
    &'static [(&'static str, &'static str)],
) {
    match body {
        "xt5" => (
            &[
                (
                    Key::StickUpLeft,
                    "focus_stick.up_left",
                    "Focus stick up-left",
                ),
                (
                    Key::StickUpRight,
                    "focus_stick.up_right",
                    "Focus stick up-right",
                ),
                (
                    Key::StickDownLeft,
                    "focus_stick.down_left",
                    "Focus stick down-left",
                ),
                (
                    Key::StickDownRight,
                    "focus_stick.down_right",
                    "Focus stick down-right",
                ),
                (
                    Key::RearPush,
                    "rear_command_dial.push",
                    "Rear command dial press",
                ),
                (
                    Key::FrontPush,
                    "front_command_dial.push",
                    "Front command dial press",
                ),
            ],
            &[
                ("drive", "drive_dial"),
                ("ev", "exposure_compensation_dial"),
                ("mode", "still_movie_mode_dial"),
                ("focus", "focus_mode_selector"),
                ("iso", "sensitivity_dial"),
                ("ss", "shutter_speed_dial"),
                ("power", "power_switch"),
            ],
        ),
        _ => (&[], &[]),
    }
}

/// Catalog identities and labels of the resolved body's ladder controls.
pub fn panel_names(resolved: &ResolvedConfiguration) -> Result<crate::panel::Names, String> {
    let body = &resolved.reference.body.id;
    let document = resolved
        .reference
        .physical_controls
        .as_ref()
        .ok_or_else(|| format!("body {body} has no physical controls"))?;
    let controls = PhysicalControls::decode_document(document, body)?;
    let label = |id: &str| -> Result<String, String> {
        let (control, direction) = id.split_once('.').map_or((id, None), |(c, d)| (c, Some(d)));
        let name = &controls
            .controls
            .get(control)
            .ok_or_else(|| format!("{body} catalog has no control {control}"))?
            .name;
        Ok(match direction {
            Some(direction) => format!("{name} {direction}"),
            None => name.clone(),
        })
    };
    let mut keys = std::collections::BTreeMap::new();
    for rung in controls.ladders.iter().flat_map(|ladder| &ladder.rungs) {
        if let Some(key) = key_for_control(&rung.control_id) {
            keys.insert(key, (rung.control_id.clone(), label(&rung.control_id)?));
        }
    }
    let (extra_keys, positions) = extension_ids(body);
    for &(key, id, name) in extra_keys {
        keys.insert(key, (id.to_owned(), name.to_owned()));
    }
    let positions = positions
        .iter()
        .map(|&(model, id)| Ok((model.to_owned(), (id.to_owned(), label(id)?))))
        .collect::<Result<_, String>>()?;
    let shutter = match controls.controls.get("shutter_button") {
        Some(control) if matches!(body.as_str(), "xt5" | "x100vi" | "xh2") => Some((
            ("shutter_button".to_owned(), control.name.clone()),
            // S1 (half press) on GPIO 0.0 and S2 on 0.1, both active low.
            [Pin { port: 0, pin: 0 }, Pin { port: 0, pin: 1 }],
        )),
        _ => None,
    };
    let dial = |id: &str| -> Result<Option<(String, String)>, String> {
        Ok(match controls.controls.contains_key(id) {
            true => Some((id.to_owned(), label(id)?)),
            false => None,
        })
    };
    Ok(crate::panel::Names {
        keys,
        positions,
        shutter,
        rear_dial: dial("rear_command_dial")?,
        front_dial: dial("front_command_dial")?,
    })
}

/// The run's sample on one channel of its single ADC block input.
fn adc_sample(inputs: &ApplicationRunInputs, channel: u8) -> Result<u16, String> {
    let mut adc = inputs.blocks.iter().filter_map(|block| match block {
        ApplicationBlockInput::Adc { samples, .. } => Some(samples),
        _ => None,
    });
    let samples = match (adc.next(), adc.next()) {
        (Some(samples), None) => samples,
        (None, _) => return Err("ADC ladders need the run's ADC block input".into()),
        (Some(_), Some(_)) => {
            return Err("ADC ladders need exactly one ADC block input in the run".into());
        }
    };
    samples
        .get(usize::from(channel))
        .copied()
        .ok_or_else(|| format!("the run's ADC input has no sample for ladder channel {channel}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xt5() -> ResolvedConfiguration {
        ResolvedConfiguration::bundled("xt5-431").unwrap()
    }

    fn inputs(resolved: &mut ResolvedConfiguration) -> &mut ApplicationRunInputs {
        match resolved.configuration.inputs.as_mut() {
            Some(RunInputs::Xp5(inputs)) => inputs,
            _ => panic!("X-T5 run has XP5 inputs"),
        }
    }

    #[test]
    fn xt5_ladders_become_the_ladder_wiring() {
        let options = ladders(&xt5()).unwrap().unwrap();
        let wires: Vec<_> = options.ladders.iter().map(|ladder| ladder.wire).collect();
        assert_eq!(
            wires,
            [
                ladders::Wire::Adc { channel: 1 },
                ladders::Wire::Adc { channel: 2 },
                ladders::Wire::Adc { channel: 14 },
                ladders::Wire::PowerControllerRegister {
                    register: 0x34,
                    released: 1023
                },
                ladders::Wire::PowerControllerRegister {
                    register: 0x35,
                    released: 1023
                },
                ladders::Wire::Adc { channel: 11 },
            ]
        );
        // 19 catalog rungs plus the four focus-lever diagonals of the board table.
        assert_eq!(
            options
                .ladders
                .iter()
                .map(|ladder| ladder.rungs.len())
                .sum::<usize>(),
            23
        );
        let rung = |key: Key| {
            options
                .ladders
                .iter()
                .flat_map(|ladder| &ladder.rungs)
                .find(|rung| rung.key == key)
                .map(|rung| (rung.band_low, rung.band_high, rung.sample))
                .unwrap()
        };
        // MENU/OK alone is 527..582; 0..200 on the same register is the focus-stick press.
        assert_eq!(rung(Key::Menu), (527, 582, 554));
        assert_eq!(rung(Key::StickPush), (0, 200, 100));
        assert_eq!(rung(Key::Back), (768, 933, 850));
        assert_eq!(rung(Key::Up), (377, 420, 398));
        assert_eq!(rung(Key::Play), (2102, 2325, 2213));
    }

    #[test]
    fn x100vi_ladders_wire_its_buttons_stick_and_dials() {
        let options = ladders(&ResolvedConfiguration::bundled("x100vi-131").unwrap())
            .unwrap()
            .unwrap();
        let rung = |key: Key| {
            options
                .ladders
                .iter()
                .flat_map(|ladder| &ladder.rungs)
                .find(|rung| rung.key == key)
                .map(|rung| (rung.band_low, rung.band_high))
                .unwrap()
        };
        assert_eq!(rung(Key::Menu), (3111, 3834));
        assert_eq!(rung(Key::Back), (2112, 2342));
        assert_eq!(rung(Key::StickUp), (1438, 1679));
        assert_eq!(rung(Key::StickDown), (3053, 3765));
        assert!(matches!(
            options.rear_dial,
            Some(ladders::CommandDial::Quadrature(_))
        ));
        assert!(options.front_dial.is_some());
    }

    #[test]
    fn every_mapped_identity_is_a_catalog_control() {
        let bodies = [
            ("xt5-431", "xt5"),
            ("xh2-530", "xh2"),
            ("xh2s-730", "xh2s"),
            ("xe5-112", "xe5"),
        ]
        .map(|(configuration, body)| {
            let document = ResolvedConfiguration::bundled(configuration)
                .unwrap()
                .reference
                .physical_controls
                .unwrap();
            PhysicalControls::decode_document(&document, body).unwrap()
        });
        for (id, _) in CONTROL_KEYS {
            assert!(
                bodies.iter().any(|controls| controls.control(id).is_ok()),
                "{id} is no body's catalog control"
            );
        }
    }

    #[test]
    fn xe5_ladders_become_the_ladder_wiring() {
        let options = ladders(&ResolvedConfiguration::bundled("xe5-112").unwrap())
            .unwrap()
            .unwrap();
        let channels: Vec<u8> = options
            .ladders
            .iter()
            .filter_map(|ladder| match ladder.wire {
                ladders::Wire::Adc { channel } => Some(channel),
                _ => None,
            })
            .collect();
        assert_eq!(channels, [1, 2, 8, 11, 14]);
    }

    #[test]
    fn xh2_ladders_become_the_ladder_wiring() {
        let options = ladders(&ResolvedConfiguration::bundled("xh2-530").unwrap())
            .unwrap()
            .unwrap();
        let wires: Vec<_> = options.ladders.iter().map(|ladder| ladder.wire).collect();
        assert_eq!(
            wires,
            [
                ladders::Wire::Adc { channel: 6 },
                ladders::Wire::Adc { channel: 13 }
            ]
        );
        let rung = |key: Key| {
            options
                .ladders
                .iter()
                .flat_map(|ladder| &ladder.rungs)
                .find(|rung| rung.key == key)
                .map(|rung| (rung.band_low, rung.band_high, rung.sample))
                .unwrap()
        };
        assert_eq!(rung(Key::Menu), (0, 782, 391));
        assert_eq!(rung(Key::Back), (2637, 3102, 2869));
        assert_eq!(rung(Key::Up), (1448, 1688, 1568));
        assert_eq!(rung(Key::Right), (1930, 2219, 2074));
        assert_eq!(rung(Key::Left), (2606, 3041, 2823));
        assert_eq!(rung(Key::Down), (3042, 3719, 3380));
        assert!(options.gpio_keys.is_empty() && options.dials.is_empty());
    }

    #[test]
    fn xh2s_ladders_become_the_ladder_wiring() {
        let options = ladders(&ResolvedConfiguration::bundled("xh2s-730").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(options.ladders.len(), 5);
    }

    #[test]
    fn a_register_ladder_needs_one_released_value() {
        let mut resolved = xt5();
        inputs(&mut resolved)
            .ladders
            .retain(|input| input.wire != LadderWire::PowerControllerRegister { register: 0x35 });
        let error = ladders(&resolved).unwrap_err();
        assert!(
            error.contains("register 0x35 ladder has no released value"),
            "{error}"
        );

        let mut resolved = xt5();
        let mut document = resolved.reference.physical_controls.take().unwrap();
        document["ladders"][3]["released"] = serde_json::json!(1000);
        resolved.reference.physical_controls = Some(document);
        let error = ladders(&resolved).unwrap_err();
        assert!(error.contains("from both the body and the run"), "{error}");
    }

    #[test]
    fn rungs_without_an_emulator_key_and_foreign_resolutions_are_refused() {
        let mut resolved = xt5();
        let mut document = resolved.reference.physical_controls.take().unwrap();
        document["ladders"][0]["rungs"][0]["control_id"] = serde_json::json!("rear_command_dial");
        resolved.reference.physical_controls = Some(document.clone());
        let error = ladders(&resolved).unwrap_err();
        assert!(
            error.contains("rear_command_dial have no emulator key"),
            "{error}"
        );

        // A 10-bit register ladder declared with 12-bit resolution is not the model's wire.
        let mut document = xt5().reference.physical_controls.unwrap();
        document["ladders"][3]["levels"] = serde_json::json!(4096);
        resolved.reference.physical_controls = Some(document);
        let error = ladders(&resolved).unwrap_err();
        assert!(error.contains("the model reads 1024"), "{error}");
    }

    #[test]
    fn an_adc_sample_inside_a_band_holds_that_control() {
        let mut resolved = xt5();
        for block in &mut inputs(&mut resolved).blocks {
            if let ApplicationBlockInput::Adc { samples, .. } = block {
                samples[14] = 1604;
            }
        }
        let error = ladders(&resolved).unwrap_err();
        assert!(
            error.contains("channel 14 sample 1604 holds focus_stick.up"),
            "{error}"
        );
    }

    #[test]
    fn bodies_without_ladders_have_no_ladder_model() {
        let mut resolved = xt5();
        let mut document = resolved.reference.physical_controls.take().unwrap();
        document.as_object_mut().unwrap().remove("ladders");
        resolved.reference.physical_controls = Some(document);
        assert!(ladders(&resolved).unwrap().is_none());
    }
}
