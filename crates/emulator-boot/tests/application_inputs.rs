//! Application run inputs are declared per body in the run configuration, one
//! entry per device the body has, and checked against the selected hardware.
use fujimod_catalog_configuration::{
    RunConfiguration, RunInputs, application_inputs::ApplicationBlockInput,
};
use fujimod_catalog_resolution::{ResolvedConfiguration, validate_selection};

fn xp5_inputs(configuration: &mut RunConfiguration) -> &mut fujimod_catalog_configuration::application_inputs::ApplicationRunInputs {
    match configuration.inputs.as_mut() {
        Some(RunInputs::Xp5(inputs)) => inputs,
        _ => panic!("{} has no XP5 run inputs", configuration.id),
    }
}

#[test]
fn xp5_bodies_declare_only_the_devices_they_have() {
    for (id, blocks) in [
        ("x100vi-131", ["adc", "gpio", "flash-aperture"].as_slice()),
        ("xt5-431", ["adc", "flash-aperture"].as_slice()),
    ] {
        let mut configuration = ResolvedConfiguration::bundled(id).unwrap().configuration;
        let declared: Vec<_> = xp5_inputs(&mut configuration)
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
        assert!(resolved.hardware_inputs().unwrap().is_none(), "{id}");
    }
}

#[test]
fn inputs_for_a_device_the_selected_hardware_lacks_are_refused() {
    let resolved = ResolvedConfiguration::bundled("xt5-431").unwrap();
    let selection = &resolved.reference;
    validate_selection(&resolved.configuration, selection).unwrap();

    let refused = |edit: &dyn Fn(&mut RunConfiguration), needle: &str| {
        let mut configuration = resolved.configuration.clone();
        edit(&mut configuration);
        let error = validate_selection(&configuration, selection).unwrap_err();
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
