//! A body's catalog ladders as options of the ladder-key model.
//!
//! The bands, samples and evidence come from the body's physical-controls
//! record. The value each wire reads with no control held comes from the run:
//! an ADC ladder reads the ADC block input's sample on its channel, and a
//! register ladder reads the body's declared released value or, where the
//! body has none, the run configuration's ladder input. A ladder without a
//! key-free value, a control without an emulator key and a wire whose
//! resolution differs from the model's are refused, never approximated.

use fujimod_catalog_configuration::{
    RunInputs,
    application_inputs::{ApplicationBlockInput, ApplicationRunInputs},
};
use fujimod_catalog_controls::{ControlLadder, LadderWire, PhysicalControls};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_parts_controls::{key_input::Key, ladder_keys};

/// Catalog control identities, including pad directions, and the emulator key
/// that presses each. A ladder rung on any other identity is refused.
const CONTROL_KEYS: [(&str, Key); 16] = [
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
    let options = ladder_keys::Options { ladders };
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
            ]
        );
        assert_eq!(options.ladders.iter().map(|ladder| ladder.rungs.len()).sum::<usize>(), 16);
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

        let keys = ControlsOptions::LadderKeys(options)
            .open(fujimod_emulator_devices::xp5::pins::Pins::shared())
            .unwrap();
        assert_eq!(keys.register_sample(0x34), Some(1023));
        keys.enqueue(Duration::from_nanos(1), Key::Menu, true).unwrap();
        keys.enqueue(Duration::from_nanos(1), Key::Quick, true).unwrap();
        keys.advance(1);
        assert_eq!(keys.register_sample(0x34), Some(554));
        assert_eq!(keys.sample(2, 4095), 2213);
        let error = keys.enqueue(Duration::from_nanos(2), Key::ShutterHalf, true).unwrap_err();
        assert!(error.contains("no declared ladder rung"), "{error}");
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
        document["ladders"][0]["rungs"][0]["control_id"] = serde_json::json!("playback_button");
        resolved.reference.physical_controls = Some(document.clone());
        let error = ladder_keys(&resolved).unwrap_err();
        assert!(error.contains("playback_button have no emulator key"), "{error}");

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
