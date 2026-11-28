//! Application startup inputs are declared per body in its record, one entry
//! per device the body has, and checked against the selected hardware.
use fujimod_catalog_configuration::{RunInputs, application::ApplicationBlockInput};
use fujimod_catalog_resolution::ResolvedConfiguration;

fn xp5_inputs(
    resolved: &mut ResolvedConfiguration,
) -> &mut fujimod_catalog_run_inputs::application::ApplicationRunInputs {
    match resolved.inputs.as_mut() {
        Some(RunInputs::Xp5(inputs)) => inputs,
        _ => panic!("{} has no XP5 startup inputs", resolved.configuration.id),
    }
}

#[test]
fn xp5_bodies_declare_only_the_devices_they_have() {
    for (id, blocks) in [
        ("x100vi-131", ["adc", "gpio", "flash-aperture"].as_slice()),
        ("xt5-431", ["adc", "flash-aperture", "gpio"].as_slice()),
    ] {
        let mut resolved = ResolvedConfiguration::bundled(id).unwrap();
        let declared: Vec<_> = xp5_inputs(&mut resolved)
            .blocks
            .iter()
            .map(|input| input.block_id().to_owned())
            .collect();
        assert_eq!(declared, blocks, "{id}");
    }
}

#[test]
fn no_xp5_body_names_a_reset_inputs_document_with_another_bodys_values() {
    for id in ["x100vi-131", "xt5-431"] {
        let resolved = ResolvedConfiguration::bundled(id).unwrap();
        assert!(resolved.hardware_inputs().is_none(), "{id}");
    }
}

#[test]
fn inputs_for_a_device_the_selected_hardware_lacks_are_refused() {
    let resolved = ResolvedConfiguration::bundled("xt5-431").unwrap();
    resolved.validate().unwrap();

    let refused = |edit: &dyn Fn(&mut ResolvedConfiguration), needle: &str| {
        let mut invalid = resolved.clone();
        edit(&mut invalid);
        let error = invalid.validate().unwrap_err();
        assert!(error.contains(needle), "{error}");
    };
    // A block the SoC does not have.
    refused(
        &|configuration| {
            for input in &mut xp5_inputs(configuration).blocks {
                if let ApplicationBlockInput::Adc { block_id, .. } = input {
                    *block_id = "no-such-block".into();
                }
            }
        },
        "missing SoC block",
    );
    // A block of the wrong hardware category.
    refused(
        &|configuration| {
            for input in &mut xp5_inputs(configuration).blocks {
                if let ApplicationBlockInput::Adc { block_id, .. } = input {
                    *block_id = "receiver0".into();
                }
            }
        },
        "wrong hardware category",
    );
    // A part the body does not carry.
    refused(
        &|configuration| {
            for input in &mut xp5_inputs(configuration).blocks {
                if let ApplicationBlockInput::Flash { part_id, .. } = input {
                    *part_id = "x100vi-serial-nor".into();
                }
            }
        },
        "uninstalled part",
    );
}
