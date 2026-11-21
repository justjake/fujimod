//! A body's catalog ladders as options of the ladder-key model.
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

use fujimod_catalog_configuration::{
    RunInputs,
    application_inputs::{ApplicationBlockInput, ApplicationRunInputs},
};
use fujimod_catalog_controls::{ControlLadder, LadderWire, PhysicalControls};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_devices::parts::pins::Pin;
use fujimod_emulator_parts_controls::{key_input::Key, ladder_keys};

/// Catalog control identities, including pad directions, and the emulator key
/// that presses each. A ladder rung on any other identity is refused.
const CONTROL_KEYS: [(&str, Key); 19] = [
    ("menu_ok_button", Key::Menu),
    ("disp_back_bluetooth_button", Key::Back),
    ("selector_up_button", Key::Up),
    ("selector_down_button", Key::Down),
    ("selector_left_button", Key::Left),
    ("selector_right_button", Key::Right),
    ("delete_button", Key::Delete),
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
    ("ael_button", Key::AeLock),
    ("af_on_button", Key::AfOn),
];

/// The emulator key that presses a catalog control identity, if one exists.
pub fn key_for_control(control_id: &str) -> Option<Key> {
    CONTROL_KEYS
        .iter()
        .find(|(id, _)| *id == control_id)
        .map(|(_, key)| *key)
}

/// The resolved body's ladders as ladder-key options, or `None` when the body
/// declares no ladder.
pub fn ladder_keys(
    resolved: &ResolvedConfiguration,
) -> Result<Option<ladder_keys::Options>, String> {
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
        _ => return Err(format!("body {body} declares ladders but the run has no XP5 inputs")),
    };
    let ladders = controls
        .ladders
        .iter()
        .map(|ladder| model_ladder(body, ladder, inputs))
        .collect::<Result<Vec<_>, _>>()?;
    let mut options = ladder_keys::Options {
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
) -> Result<ladder_keys::Ladder, String> {
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
            ladder_keys::Wire::Adc { channel }
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
            ladder_keys::Wire::PowerControllerRegister { register, released }
        }
    };
    Ok(ladder_keys::Ladder {
        wire,
        levels: ladder.levels,
        rungs: ladder
            .rungs
            .iter()
            .map(|rung| ladder_keys::Rung {
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

fn positions(entries: &[(&str, &[&str], u16, u16, bool)]) -> Vec<ladder_keys::Position> {
    entries
        .iter()
        .map(|&(label, aliases, low, high, contact)| ladder_keys::Position {
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
fn extension(body: &str, options: &mut ladder_keys::Options) -> Result<(), String> {
    if body != "xt5" {
        return Ok(());
    }
    let adc = |channel| move |wire: ladder_keys::Wire| wire == ladder_keys::Wire::Adc { channel };
    let register = |number| {
        move |wire: ladder_keys::Wire| {
            matches!(wire, ladder_keys::Wire::PowerControllerRegister { register, .. } if register == number)
        }
    };
    fn ladder<'a>(
        options: &'a mut ladder_keys::Options,
        is: &dyn Fn(ladder_keys::Wire) -> bool,
    ) -> Result<&'a mut ladder_keys::Ladder, String> {
        options
            .ladders
            .iter_mut()
            .find(|ladder| is(ladder.wire))
            .ok_or_else(|| "the catalog ladders lack a wire the board table extends".to_owned())
    }
    let rung = |key, low, high| ladder_keys::Rung {
        key,
        band_low: low,
        band_high: high,
        sample: midpoint(low, high),
        basis: XT5_BANDS.into(),
    };
    let set = |keys: &[Key], position: Option<&str>, low, high| ladder_keys::ContactSet {
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
    let dial = |name: &str, aliases: &[&str], channel, entries: &[(&str, &[&str], u16, u16, bool)]| {
        ladder_keys::PositionDial {
            name: name.into(),
            aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
            channel,
            positions: positions(entries),
            basis: XT5_BANDS.into(),
        }
    };
    options.dials = vec![
        dial("drive", &[], 3, &[
            ("PANO", &["PANORAMA"], 100, 842, false),
            ("ADV", &[], 843, 1424, false),
            ("BKT", &[], 1425, 2006, false),
            ("CH", &[], 2007, 2594, false),
            ("CL", &[], 2595, 3194, false),
            ("S", &[], 3195, 3812, false),
            ("HDR", &[], 3813, 4095, false),
        ]),
        dial("ev", &["exposure"], 7, &[
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
        ]),
        dial("mode", &["lever"], 2, &[
            ("MOVIE", &[], 3067, 3750, true),
            ("STILL", &[], 3751, 4095, false),
        ]),
        dial("focus", &["af"], 11, &[
            ("M", &["MF"], 2618, 3066, true),
            ("S", &["AF-S"], 3067, 3750, true),
            ("C", &["AF-C"], 3751, 4095, false),
        ]),
        // Labels 125..160 and C are marked as guesses in the decode.
        dial("iso", &[], 12, &[
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
        ]),
        // Shutter speeds: a number N below 1 s is 1/N; "1" is one second.
        dial("ss", &["shutter"], 13, &[
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
        ]),
    ];
    // Row 0 reads POWER held while register 0x00 bit 10 is clear; that set
    // means OFF is a guess of the decode.
    options.switches = vec![ladder_keys::RegisterSwitch {
        name: "power".into(),
        register: 0x00,
        bit: 10,
        set: "OFF".into(),
        clear: "ON".into(),
        basis: "Scanner row 0 (physical 0x0c, UI POWER) reads ((byte [0x1c09c99] ^ 4) >> 2) & 1, the \
copy of power-controller register 0x00 that 0x1407d2c refreshes each scan."
            .into(),
    }];
    options.rear_dial = Some(ladder_keys::CountingDial {
        register: 0x36,
        basis: "0x1407d2c reads power-controller register 0x36 each scan through 0x13d5834: bits 2..0 \
count, bit 3 sign. The firmware never writes it, so the controller clears the count on read."
            .into(),
    });
    options.front_dial = Some(ladder_keys::QuadratureDial {
        pins: [Pin { port: 0, pin: 9 }, Pin { port: 0, pin: 10 }],
        rest_phase: 3,
        basis: "Sampler 0x140a5bc: phase = level(0.9) << 1 | level(0.10), idle 3; transition table \
0x1c0af58 steps 3, 2, 0, 1, 3 for CMD2_R (physical 0x8e) and 3, 1, 0, 2, 3 for CMD2_L (0x8f)."
            .into(),
    });
    options.gpio_keys = vec![
        ladder_keys::GpioKey {
            key: Key::RearPush,
            pin: Pin { port: 0, pin: 13 },
            active_high: false,
            basis: "Scanner row 97 (physical 0x1e, UI 0x25) reads GPIO 0.13 low.".into(),
        },
        ladder_keys::GpioKey {
            key: Key::FrontPush,
            pin: Pin { port: 0, pin: 14 },
            active_high: false,
            basis: "Scanner row 98 (physical 0x1f, UI 0x44) reads GPIO 0.14 low.".into(),
        },
    ];
    Ok(())
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
    use fujimod_emulator_parts_controls::key_input::ControlsOptions;
    use std::time::Duration;

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
    fn xt5_ladders_become_the_ladder_key_model() {
        let options = ladder_keys(&xt5()).unwrap().unwrap();
        let wires: Vec<_> = options.ladders.iter().map(|ladder| ladder.wire).collect();
        assert_eq!(
            wires,
            [
                ladder_keys::Wire::Adc { channel: 1 },
                ladder_keys::Wire::Adc { channel: 2 },
                ladder_keys::Wire::Adc { channel: 14 },
                ladder_keys::Wire::PowerControllerRegister { register: 0x34, released: 1023 },
                ladder_keys::Wire::PowerControllerRegister { register: 0x35, released: 1023 },
                ladder_keys::Wire::Adc { channel: 11 },
            ]
        );
        // 19 catalog rungs plus the four focus-lever diagonals of the board table.
        assert_eq!(options.ladders.iter().map(|ladder| ladder.rungs.len()).sum::<usize>(), 23);
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

        let keys = ControlsOptions::LadderKeys(options)
            .open(fujimod_emulator_devices::xp5::pins::Pins::shared())
            .unwrap();
        assert_eq!(keys.register_sample(0x34), Some(1023));
        keys.enqueue(Duration::from_nanos(1), Key::Menu, true).unwrap();
        keys.enqueue(Duration::from_nanos(1), Key::Quick, true).unwrap();
        keys.advance(1);
        assert_eq!(keys.register_sample(0x34), Some(554));
        assert_eq!(keys.sample(2, 4095), 2213);
        // PLAY pulls ADC11 (focus-mode selector at C) into the shared band.
        assert_eq!(keys.sample(11, 4095), 4095);
        keys.enqueue(Duration::from_nanos(1), Key::Play, true).unwrap();
        keys.advance(1);
        assert_eq!(keys.sample(11, 4095), 2213);
        let error = keys.enqueue(Duration::from_nanos(2), Key::ShutterHalf, true).unwrap_err();
        assert!(error.contains("no declared ladder rung"), "{error}");
    }

    fn xt5_keys() -> std::sync::Arc<dyn fujimod_emulator_parts_controls::key_input::KeyInput> {
        ControlsOptions::LadderKeys(ladder_keys(&xt5()).unwrap().unwrap())
            .open(fujimod_emulator_devices::xp5::pins::Pins::shared())
            .unwrap()
    }

    fn at(ns: u64) -> Duration {
        Duration::from_nanos(ns)
    }

    #[test]
    fn xt5_contact_sets_read_their_band_midpoints() {
        let keys = xt5_keys();
        for key in [Key::AeLock, Key::AfOn, Key::StickUpLeft] {
            keys.enqueue(at(1), key, true).unwrap();
        }
        keys.advance(1);
        assert_eq!(keys.register_sample(0x34), Some(711));
        assert_eq!(keys.sample(1, 4095), 2842);
        assert_eq!(keys.sample(14, 4095), 1347);
        // Pairs on one wire.
        keys.enqueue(at(2), Key::Menu, true).unwrap();
        keys.enqueue(at(2), Key::Fn2, true).unwrap();
        keys.enqueue(at(2), Key::Left, true).unwrap();
        keys.enqueue(at(2), Key::Up, true).unwrap();
        keys.advance(2);
        assert_eq!(keys.register_sample(0x34), Some(449), "AE-L + MENU 427..471");
        assert_eq!(keys.sample(1, 4095), 2471, "Fn2 + AF-ON 2326..2617");
        assert_eq!(keys.register_sample(0x35), Some(332), "LEFT + UP 316..348");
        // Undecoded sets: three keys on ADC 1, DELETE with another key, two
        // focus-lever directions, the lever push with MENU.
        let error = keys.enqueue(at(3), Key::ViewMode, true).unwrap_err();
        assert!(error.contains("no band is declared"), "{error}");
        let error = keys.enqueue(at(3), Key::StickUp, true).unwrap_err();
        assert!(error.contains("no band is declared"), "{error}");
        let error = keys.enqueue(at(3), Key::StickPush, true).unwrap_err();
        assert!(error.contains("no band is declared"), "{error}");
        let keys = xt5_keys();
        keys.enqueue(at(1), Key::Delete, true).unwrap();
        assert!(keys.enqueue(at(1), Key::Fn2, true).is_err());
    }

    #[test]
    fn xt5_shared_wires_follow_the_lever_and_focus_positions() {
        let keys = xt5_keys();
        keys.enqueue(at(1), Key::Quick, true).unwrap();
        keys.enqueue(at(1), Key::Play, true).unwrap();
        keys.advance(1);
        // Window samples as with XT5_ADC=2=3400,11=3400: lever MOVIE, focus S.
        assert_eq!(keys.sample(2, 3400), 1990, "MOVIE + Q 1880..2101");
        assert_eq!(keys.sample(11, 3400), 1990, "S + PLAY 1880..2101");
        assert_eq!(keys.sample(2, 4095), 2213, "STILL: Q alone");
        assert_eq!(keys.sample(11, 4095), 2213, "C: PLAY alone");
        keys.set_position(at(2), "focus", "M").unwrap();
        keys.set_position(at(2), "mode", "MOVIE").unwrap();
        keys.enqueue(at(2), Key::Fn1, true).unwrap();
        keys.advance(2);
        assert_eq!(keys.sample(11, 4095), 1790, "M + PLAY 1701..1879");
        assert_eq!(keys.sample(2, 4095), 900, "MOVIE + Fn1 + Q 100..1700");
    }

    #[test]
    fn xt5_dials_switch_and_command_dials_are_declared() {
        let keys = xt5_keys();
        for (dial, position, channel, sample) in [
            ("iso", "800", 12, 1591),
            ("ss", "1/250", 13, 2543),
            ("ss", "T", 13, 453),
            ("ev", "-2/3", 7, 2648),
            ("ev", "+1", 7, 1627),
            ("drive", "CH", 3, 2300),
            ("focus", "S", 11, 3408),
            ("mode", "STILL", 2, 3923),
        ] {
            keys.set_position(at(5), dial, position).unwrap();
            keys.advance(5);
            assert_eq!(keys.sample(channel, 4095), sample, "{dial}={position}");
        }
        assert!(keys.set_position(at(6), "iso", "100").is_err());
        assert!(keys.set_position(at(6), "drive", "VIDEO").is_err());
        keys.set_position(at(6), "power", "OFF").unwrap();
        keys.advance(6);
        assert_eq!(keys.driven_registers(), [0x00, 0x34, 0x35]);
        assert_eq!(keys.drive_register(0x00, 0x280), Some(0x680));
        for _ in 0..2 {
            keys.enqueue(at(7), Key::RearPlus, true).unwrap();
        }
        keys.enqueue(at(7), Key::FrontPlus, true).unwrap();
        keys.enqueue(at(7), Key::FrontPush, true).unwrap();
        keys.advance(7);
        assert_eq!(keys.take_register(0x36), Some(2));
        assert_eq!(keys.take_register(0x36), Some(0));
    }

    #[test]
    fn every_mapped_identity_is_a_catalog_control() {
        let document = xt5().reference.physical_controls.unwrap();
        let controls = PhysicalControls::decode_document(&document, "xt5").unwrap();
        for (id, _) in CONTROL_KEYS {
            controls.control(id).unwrap();
        }
    }

    #[test]
    fn a_register_ladder_needs_one_released_value() {
        let mut resolved = xt5();
        inputs(&mut resolved).ladders.retain(|input| {
            input.wire != LadderWire::PowerControllerRegister { register: 0x35 }
        });
        let error = ladder_keys(&resolved).unwrap_err();
        assert!(error.contains("register 0x35 ladder has no released value"), "{error}");

        let mut resolved = xt5();
        let mut document = resolved.reference.physical_controls.take().unwrap();
        document["ladders"][3]["released"] = serde_json::json!(1000);
        resolved.reference.physical_controls = Some(document);
        let error = ladder_keys(&resolved).unwrap_err();
        assert!(error.contains("from both the body and the run"), "{error}");
    }

    #[test]
    fn rungs_without_an_emulator_key_and_foreign_resolutions_are_refused() {
        let mut resolved = xt5();
        let mut document = resolved.reference.physical_controls.take().unwrap();
        document["ladders"][0]["rungs"][0]["control_id"] = serde_json::json!("rear_command_dial");
        resolved.reference.physical_controls = Some(document.clone());
        let error = ladder_keys(&resolved).unwrap_err();
        assert!(error.contains("rear_command_dial have no emulator key"), "{error}");

        // A 10-bit register ladder declared with 12-bit resolution is not the model's wire.
        let mut document = xt5().reference.physical_controls.unwrap();
        document["ladders"][3]["levels"] = serde_json::json!(4096);
        resolved.reference.physical_controls = Some(document);
        let error = ladder_keys(&resolved).unwrap_err();
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
        let error = ladder_keys(&resolved).unwrap_err();
        assert!(error.contains("channel 14 sample 1604 holds focus_stick.up"), "{error}");
    }

    #[test]
    fn bodies_without_ladders_have_no_ladder_model() {
        assert!(
            ladder_keys(&ResolvedConfiguration::bundled("x100vi-131").unwrap())
                .unwrap()
                .is_none()
        );
    }
}
