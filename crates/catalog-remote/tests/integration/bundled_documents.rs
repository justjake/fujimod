use fujimod_catalog::ReferenceCatalog;

fn remote_selection<'a>(
    selection: &'a fujimod_catalog::ResolvedSelection,
) -> fujimod_catalog_remote::Selection<'a> {
    let generated = selection.firmware.generated_facts.as_ref();
    fujimod_catalog_remote::Selection {
        firmware_id: &selection.firmware.id,
        body_id: &selection.body.id,
        firmware_sha256: selection
            .firmware
            .update
            .as_ref()
            .map(|content| content.sha256.as_str()),
        remote_facts: generated.and_then(|facts| facts.remote.as_ref()),
        physical_controls: selection.physical_controls.as_ref(),
        control_ids: selection.control_ids.as_ref(),
        connections: selection
            .body
            .connections
            .iter()
            .map(|connection| connection.id.as_str())
            .collect(),
    }
}

#[test]
fn every_bundled_remote_document_decodes_and_validates() {
    let catalog = ReferenceCatalog::bundled().unwrap();
    for firmware_id in catalog.firmware_ids() {
        let selection = catalog.resolve_firmware(firmware_id).unwrap();
        let Some(generated) = selection.firmware.generated_facts.as_ref() else {
            continue;
        };
        if let Some(value) = &generated.remote {
            let input = remote_selection(&selection);
            let facts = fujimod_catalog_remote::Facts::from_selection(&input, None)
                .unwrap_or_else(|error| panic!("firmware {firmware_id}: {error}"));
            assert!(value.is_object());
            assert_eq!(facts.firmware.firmware_id, firmware_id);
        }
        if let Some(value) = &generated.observation {
            serde_json::from_value::<fujimod_catalog_remote::FirmwareObservation>(value.clone())
                .unwrap_or_else(|error| panic!("firmware {firmware_id} observation: {error}"));
        }
        for (name, value) in [
            (
                "configuration location",
                generated.cfgdata_location.as_ref(),
            ),
            (
                "additional UI modes",
                generated.additional_ui_modes.as_ref(),
            ),
            ("menu keys", generated.menu_keys.as_ref()),
        ] {
            if let Some(value) = value {
                match name {
                    "configuration location" => {
                        serde_json::from_value::<
                            fujimod_catalog_controls::facts::ConfigurationLocation,
                        >(value.clone())
                        .unwrap_or_else(|error| panic!("firmware {firmware_id} {name}: {error}"));
                    }
                    "additional UI modes" => {
                        serde_json::from_value::<fujimod_catalog_controls::facts::AdditionalUiModes>(value.clone())
                            .unwrap_or_else(|error| panic!("firmware {firmware_id} {name}: {error}"));
                    }
                    "menu keys" => {
                        serde_json::from_value::<fujimod_catalog_controls::facts::MenuKeys>(
                            value.clone(),
                        )
                        .unwrap_or_else(|error| panic!("firmware {firmware_id} {name}: {error}"));
                    }
                    _ => unreachable!(),
                }
            }
        }
        for identity in &generated.identity_checks {
            let identity =
                serde_json::from_value::<fujimod_catalog_controls::facts::AddressIdentity>(
                    identity.clone(),
                )
                .unwrap_or_else(|error| panic!("firmware {firmware_id} address identity: {error}"));
            identity
                .validate()
                .unwrap_or_else(|error| panic!("firmware {firmware_id} address identity: {error}"));
        }
    }
}
