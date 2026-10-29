//! The reference catalog compiled into this library.
use crate::{ErrorKind, fail};
use fujimod_catalog::{ReferenceCatalog, ResolvedSelection};
use std::sync::OnceLock;
use wasm_bindgen::prelude::*;

pub(crate) fn bundled() -> Result<&'static ReferenceCatalog, JsValue> {
    static CATALOG: OnceLock<Result<ReferenceCatalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(ReferenceCatalog::bundled)
        .as_ref()
        .map_err(|message| fail(ErrorKind::Definition, message))
}

pub(crate) fn resolve(firmware_id: &str) -> Result<ResolvedSelection, JsValue> {
    bundled()?
        .resolve_firmware(firmware_id)
        .map_err(|message| fail(ErrorKind::Definition, &message))
}

/// Every catalog firmware with its body, SoC, parts and generated facts.
#[wasm_bindgen(js_name=catalogSelections, unchecked_return_type="ResolvedSelection[]")]
pub fn catalog_selections() -> Result<JsValue, JsValue> {
    let catalog = bundled()?;
    let selections = catalog
        .firmware_ids()
        .map(|id| catalog.resolve_firmware(id))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|message| fail(ErrorKind::Definition, &message))?;
    let json = serde_json::to_string(&selections)
        .map_err(|error| fail(ErrorKind::Definition, &error.to_string()))?;
    js_sys::JSON::parse(&json)
}

/// Public/internal version mappings for every known update, independent of emulator support.
#[wasm_bindgen(js_name=catalogFirmwareVersions, unchecked_return_type="FirmwareVersion[]")]
pub fn catalog_firmware_versions() -> Result<JsValue, JsValue> {
    let json = serde_json::to_string(bundled()?.firmware_versions())
        .map_err(|error| fail(ErrorKind::Definition, &error.to_string()))?;
    js_sys::JSON::parse(&json)
}

/// Bulk profiles for known releases, including releases without execution profiles.
#[wasm_bindgen(js_name=catalogBulkProfiles, unchecked_return_type="FirmwareBulk[]")]
pub fn catalog_bulk_profiles() -> Result<JsValue, JsValue> {
    let profiles: Vec<_> = bundled()?.bulk_profiles().collect();
    let json = serde_json::to_string(&profiles)
        .map_err(|error| fail(ErrorKind::Definition, &error.to_string()))?;
    js_sys::JSON::parse(&json)
}

/// Per-firmware table outcomes, including failures for cameras without execution profiles.
#[wasm_bindgen(js_name=catalogGenerationResults, unchecked_return_type="FirmwareGeneration[]")]
pub fn catalog_generation_results() -> Result<JsValue, JsValue> {
    let report = fujimod_catalog::generation::GenerationResults::bundled()
        .map_err(|message| fail(ErrorKind::Definition, &message))?;
    let json = serde_json::to_string(&report.firmwares)
        .map_err(|error| fail(ErrorKind::Definition, &error.to_string()))?;
    js_sys::JSON::parse(&json)
}
