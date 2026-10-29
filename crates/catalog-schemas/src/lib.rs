//! JSON Schema documents generated from the public catalog Rust types.
#![cfg(feature = "schema")]

use schemars::JsonSchema;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Build the committed schema documents from the serde source types.
pub fn generated_documents() -> BTreeMap<&'static str, Value> {
    let mut documents = BTreeMap::new();
    documents.insert(
        "generation.generated.schema.json",
        schema::<fujimod_catalog::generation::GenerationResults>(
            fujimod_catalog::generation::SCHEMA_URI,
        ),
    );
    documents.insert(
        "flash.generated.schema.json",
        schema::<fujimod_catalog::capture::FirmwareFlash>(
            fujimod_catalog::capture::FLASH_SCHEMA_URI,
        ),
    );
    documents.insert(
        "state.generated.schema.json",
        schema::<fujimod_catalog::capture::FirmwareState>(
            fujimod_catalog::capture::STATE_SCHEMA_URI,
        ),
    );
    documents.insert(
        "firmware-versions.generated.schema.json",
        schema::<fujimod_catalog::firmware_versions::FirmwareVersions>(
            fujimod_catalog::firmware_versions::SCHEMA_URI,
        ),
    );
    documents.insert(
        "emulator-factory-inputs.generated.schema.json",
        schema::<fujimod_catalog_run_inputs::RunFactoryInputs>(
            fujimod_catalog_run_inputs::RUN_FACTORY_INPUTS_SCHEMA_URI,
        ),
    );
    documents.insert(
        "emulator-rehearsal.generated.schema.json",
        schema::<fujimod_catalog::rehearsal::Rehearsal>(
            fujimod_catalog::rehearsal::REHEARSAL_SCHEMA_URI,
        ),
    );
    documents.insert(
        "catalog-annotations.generated.schema.json",
        schema::<fujimod_catalog::provenance::CatalogAnnotations>(
            fujimod_catalog::provenance::ANNOTATIONS_SCHEMA_URI,
        ),
    );
    documents.insert(
        "emulator-hardware-inputs.generated.schema.json",
        schema::<fujimod_catalog_run_inputs::HardwareInputs>(
            fujimod_catalog_run_inputs::HARDWARE_INPUTS_SCHEMA_URI,
        ),
    );
    documents.insert(
        "emulator-firmware-inputs.generated.schema.json",
        schema::<fujimod_catalog_run_inputs::RunFirmwareInputs>(
            fujimod_catalog_run_inputs::RUN_FIRMWARE_INPUTS_SCHEMA_URI,
        ),
    );
    documents.insert(
        "settings.generated.schema.json",
        schema::<fujimod_catalog_settings::FirmwareSettings>(
            fujimod_catalog_settings::SETTINGS_SCHEMA_URI,
        ),
    );
    documents.insert(
        "bulk.generated.schema.json",
        schema::<fujimod_catalog_bulk::FirmwareBulk>(fujimod_catalog_bulk::BULK_SCHEMA_URI),
    );
    documents.insert(
        "progress.generated.schema.json",
        schema::<fujimod_catalog::progress::FirmwareProgress>(
            fujimod_catalog::progress::PROGRESS_SCHEMA_URI,
        ),
    );
    documents.insert(
        "first-start.generated.schema.json",
        schema::<fujimod_catalog::first_start::FirstStartMenu>(
            fujimod_catalog::first_start::FIRST_START_SCHEMA_URI,
        ),
    );
    documents.insert(
        "soc.schema.json",
        schema::<fujimod_catalog::reference::Soc>("https://fujimod.com/schemas/soc.schema.json"),
    );
    documents.insert(
        "part.schema.json",
        schema::<fujimod_catalog::reference::Part>("https://fujimod.com/schemas/part.schema.json"),
    );
    documents.insert(
        "body.schema.json",
        schema::<fujimod_catalog::reference::Body>("https://fujimod.com/schemas/body.schema.json"),
    );
    documents.insert(
        "firmware.schema.json",
        schema::<fujimod_catalog::reference::Firmware>(
            "https://fujimod.com/schemas/firmware.schema.json",
        ),
    );
    documents.insert(
        "emulator-run-configuration.schema.json",
        schema::<fujimod_catalog_configuration::RunConfiguration>(
            fujimod_catalog_configuration::RUN_CONFIGURATION_SCHEMA_URI,
        ),
    );
    documents.insert(
        "emulator-run-overlay.schema.json",
        schema::<fujimod_catalog_configuration::RunOverlay>(
            fujimod_catalog_configuration::RUN_OVERLAY_SCHEMA_URI,
        ),
    );
    documents.insert(
        "resolved-selection.schema.json",
        schema::<fujimod_catalog::reference::ResolvedSelection>(
            "https://fujimod.com/schemas/resolved-selection.schema.json",
        ),
    );
    documents.insert(
        "resolved-configuration.schema.json",
        schema::<fujimod_catalog_resolution::ResolvedConfiguration>(
            "https://fujimod.com/schemas/resolved-configuration.schema.json",
        ),
    );
    documents.insert(
        "remote-firmware.generated.schema.json",
        schema::<fujimod_catalog_remote::FirmwareFacts>(
            fujimod_catalog_remote::FIRMWARE_FACTS_SCHEMA_URI,
        ),
    );
    documents.insert(
        "build-menu.generated.schema.json",
        schema::<fujimod_catalog_remote::BuildMenuManifest>(
            fujimod_catalog_remote::MENU_MANIFEST_SCHEMA_URI,
        ),
    );
    documents.insert(
        "remote-facts.generated.schema.json",
        schema::<fujimod_catalog_remote::Facts>(
            "https://fujimod.com/schemas/remote-facts.generated.schema.json",
        ),
    );
    documents.insert(
        "remote-menus.generated.schema.json",
        schema::<fujimod_catalog_remote::MenuTables>(
            "https://fujimod.com/schemas/remote-menus.generated.schema.json",
        ),
    );
    documents.insert(
        "remote-resources.generated.schema.json",
        schema::<fujimod_catalog_remote::Resources>(
            "https://fujimod.com/schemas/remote-resources.generated.schema.json",
        ),
    );
    documents.insert(
        "remote-scanout.generated.schema.json",
        schema::<fujimod_catalog_remote::Scanout>(
            "https://fujimod.com/schemas/remote-scanout.generated.schema.json",
        ),
    );
    documents.insert(
        "remote-state.generated.schema.json",
        schema::<fujimod_catalog_remote::FirmwareObservation>(
            "https://fujimod.com/schemas/remote-state.generated.schema.json",
        ),
    );
    documents.insert(
        "remote-debug-keys.generated.schema.json",
        schema::<Vec<fujimod_catalog_remote::DebugKey>>(
            "https://fujimod.com/schemas/remote-debug-keys.generated.schema.json",
        ),
    );
    documents.insert(
        "physical-controls.schema.json",
        schema::<fujimod_catalog_controls::PhysicalControls>(
            fujimod_catalog_controls::PHYSICAL_CONTROLS_SCHEMA_URI,
        ),
    );
    documents.insert(
        "control-ids.schema.json",
        schema::<fujimod_catalog_controls::FirmwareControlIds>(
            fujimod_catalog_controls::CONTROL_IDS_SCHEMA_URI,
        ),
    );
    documents
}

fn schema<T: JsonSchema>(id: &str) -> Value {
    let mut document =
        serde_json::to_value(schemars::schema_for!(T)).expect("schemars schema serializes to JSON");
    let object = document
        .as_object_mut()
        .expect("schemars root schema is a JSON object");
    object.insert("$id".into(), Value::String(id.into()));
    if let Some(Value::Object(properties)) = object.get_mut("properties") {
        if properties.contains_key("$schema") {
            properties.insert("$schema".into(), schema_uri_property(id));
        }
    }
    document
}

fn schema_uri_property(id: &str) -> Value {
    let mut property = Map::new();
    property.insert("type".into(), Value::String("string".into()));
    property.insert("const".into(), Value::String(id.into()));
    property.insert(
        "description".into(),
        Value::String("Stable schema identifier for this document.".into()),
    );
    Value::Object(property)
}
