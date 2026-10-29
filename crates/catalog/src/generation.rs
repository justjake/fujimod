//! Generation outcomes, including firmware without an execution profile.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Schema identity for generation outcomes.
pub const SCHEMA_URI: &str = "https://fujimod.com/schemas/generation.generated.schema.json";

/// One retained failure from a locator or table publisher.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
#[serde(deny_unknown_fields)]
pub struct GenerationError {
    /// Failed operation.
    pub locator: String,
    /// Specific failure, without host file locations.
    pub reason: String,
}

/// Whether a table was generated successfully.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
#[serde(rename_all = "kebab-case")]
pub enum GenerationStatus {
    /// Published successfully.
    Available,
    /// Generation or publication failed; errors explain why.
    Unavailable,
}

/// Outcome for one generated table.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
#[serde(deny_unknown_fields)]
pub struct TableGeneration {
    /// Publication status.
    pub status: GenerationStatus,
    /// Failures retained by the generator.
    pub errors: Vec<GenerationError>,
}

/// Independent table outcomes for a firmware release.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
#[serde(deny_unknown_fields)]
pub struct FirmwareGeneration {
    /// Firmware identity.
    pub firmware_id: String,
    /// Reported camera product.
    pub product: String,
    /// Public firmware release.
    pub version: String,
    /// Located internal version, when available.
    pub internal_version: Option<String>,
    /// Table name to its generation outcome.
    pub tables: BTreeMap<String, TableGeneration>,
}

/// Generation report for all attempted firmware releases.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GenerationResults {
    /// Schema identity.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// All attempted firmware, including unavailable tables.
    pub firmwares: Vec<FirmwareGeneration>,
}

impl GenerationResults {
    /// Decode and reject ambiguous or contradictory outcomes.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let report: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if report.schema_uri != SCHEMA_URI || report.schema != 1 {
            return Err("generation report schema differs".into());
        }
        let mut ids = BTreeSet::new();
        for firmware in &report.firmwares {
            fujimod_catalog_identity::validate_id(&firmware.firmware_id)?;
            if !ids.insert(&firmware.firmware_id) || firmware.product.trim().is_empty() {
                return Err("generation report has a repeated or empty identity".into());
            }
            for table in firmware.tables.values() {
                if (table.status == GenerationStatus::Available) != table.errors.is_empty()
                    || table
                        .errors
                        .iter()
                        .any(|e| e.locator.is_empty() || e.reason.is_empty())
                {
                    return Err("generation status disagrees with its errors".into());
                }
            }
        }
        Ok(report)
    }

    /// Report compiled into the catalog, independent of execution support.
    pub fn bundled() -> Result<Self, String> {
        Self::decode(include_bytes!("../../../catalog/generation.generated.json"))
    }
}

/// Explain an absent table using its retained generation outcome.
pub fn missing_table_reason(firmware_id: &str, table: &str) -> String {
    let report = match GenerationResults::bundled() {
        Ok(report) => report,
        Err(error) => return format!("Generation report could not be read: {error}"),
    };
    let outcome = report
        .firmwares
        .iter()
        .find(|f| f.firmware_id == firmware_id)
        .and_then(|f| f.tables.get(table));
    match outcome {
        Some(outcome) if !outcome.errors.is_empty() => outcome
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.locator, e.reason))
            .collect::<Vec<_>>()
            .join("; "),
        Some(_) => format!("The generated {table} artifact is missing for {firmware_id}"),
        None => format!("No {table} generation result was published for {firmware_id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_survive_without_a_resolved_camera() {
        let report = GenerationResults::bundled().unwrap();
        let firmware = report
            .firmwares
            .iter()
            .find(|f| f.firmware_id == "xe4-201")
            .unwrap();
        assert!(firmware.tables.values().any(|t| !t.errors.is_empty()));
        assert!(missing_table_reason("xe4-201", "settings").contains("No settings-bank locator"));
        assert!(
            missing_table_reason("unreported-100", "bulk").contains("No bulk generation result")
        );
        let mut value = serde_json::to_value(report).unwrap();
        value["firmwares"][0]["tables"] =
            serde_json::json!({"bulk":{"status":"unavailable","errors":[]}});
        assert!(GenerationResults::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
