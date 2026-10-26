//! Qualified emulator rehearsal facts supplied explicitly by callers.

use serde::{Deserialize, Serialize};

/// An explicitly empty device-reference set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalEmpty"))]
pub struct Empty {}

/// Declared capture facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalCapture"))]
pub struct Capture {
    /// Declared address.
    pub address: u64,
    /// Declared method.
    pub method: String,
    /// Declared name.
    pub name: String,
    /// Declared size.
    pub size: u64,
}

/// Declared configuration facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalConfiguration"))]
pub struct Configuration {
    /// Declared address.
    pub address: u64,
    /// Declared cached address.
    pub cached_address: u64,
    /// Declared max read.
    pub max_read: u64,
    /// Declared ram reply basis.
    pub ram_reply_basis: String,
    /// Declared ram reply bytes.
    pub ram_reply_bytes: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared callback table facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalCallbackTable"))]
pub struct CallbackTable {
    /// Declared address.
    pub address: u64,
    /// Declared evidence.
    pub evidence: Vec<String>,
    /// Declared hex.
    pub hex: String,
}

/// Declared code checks facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalCodeChecks"))]
pub struct CodeChecks {
    /// Declared address.
    pub address: u64,
    /// Declared name.
    pub name: String,
    /// Declared sha256.
    pub sha256: String,
    /// Declared size.
    pub size: u64,
}

/// Declared manifest facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalManifest"))]
pub struct Manifest {
    /// Declared path.
    pub path: String,
    /// Declared sha256.
    pub sha256: String,
}

/// Declared output state facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalOutputState"))]
pub struct OutputState {
    /// Declared address.
    pub address: u64,
    /// Declared evidence.
    pub evidence: Vec<String>,
    /// Declared size.
    pub size: u64,
}

/// Declared payloads facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalPayloads"))]
pub struct Payloads {
    /// Declared address.
    pub address: u64,
    /// Declared evidence.
    pub evidence: Vec<String>,
    /// Declared limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// Declared name.
    pub name: String,
    /// Declared size.
    pub size: u64,
}

/// Declared pointer context facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalPointerContext"))]
pub struct PointerContext {
    /// Declared address.
    pub address: u64,
    /// Declared evidence.
    pub evidence: Vec<String>,
    /// Declared hex.
    pub hex: String,
    /// Declared sha256.
    pub sha256: String,
}

/// Declared quiet facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalQuiet"))]
pub struct Quiet {
    /// Declared address.
    pub address: u64,
    /// Declared evidence.
    pub evidence: Vec<String>,
    /// Declared size.
    pub size: u64,
    /// Declared value.
    pub value: u64,
}

/// Declared region precondition facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalRegionPrecondition"))]
pub struct RegionPrecondition {
    /// Declared basis.
    pub basis: String,
    /// Declared evidence.
    pub evidence: Vec<String>,
    /// Declared kind.
    pub kind: String,
}

/// Declared state words facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalStateWords"))]
pub struct StateWords {
    /// Declared address.
    pub address: u64,
    /// Declared evidence.
    pub evidence: Vec<String>,
    /// Declared limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// Declared name.
    pub name: String,
    /// Declared value.
    pub value: u64,
}

/// Declared guard facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalGuard"))]
pub struct Guard {
    /// Declared address.
    pub address: u64,
    /// Declared callback table.
    pub callback_table: CallbackTable,
    /// Declared code checks.
    pub code_checks: Vec<CodeChecks>,
    /// Declared expected stock word.
    pub expected_stock_word: u64,
    /// Declared kind.
    pub kind: String,
    /// Declared manifest.
    pub manifest: Manifest,
    /// Declared output state.
    pub output_state: OutputState,
    /// Declared payloads.
    pub payloads: Vec<Payloads>,
    /// Declared pointer context.
    pub pointer_context: PointerContext,
    /// Declared quiet.
    pub quiet: Quiet,
    /// Declared region precondition.
    pub region_precondition: RegionPrecondition,
    /// Declared state words.
    pub state_words: Vec<StateWords>,
}

/// Declared identity facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalIdentity"))]
pub struct Identity {
    /// Declared internal versions.
    pub internal_versions: Vec<String>,
    /// Declared products.
    pub products: Vec<String>,
    /// Declared sections.
    pub sections: Vec<CodeChecks>,
}

/// Declared review facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "RehearsalReview"))]
pub struct Review {
    /// Declared evidence.
    pub evidence: Vec<String>,
    /// Declared scope.
    pub scope: String,
    /// Declared status.
    pub status: String,
}

/// Declared rehearsal facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Rehearsal {
    /// Document version.
    pub schema: u32,
    /// Stable schema identity.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Declared bulk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bulk: Option<String>,
    /// Declared camera operations authorized.
    pub camera_operations_authorized: bool,
    /// Declared capture.
    pub capture: Vec<Capture>,
    /// Declared configuration.
    pub configuration: Configuration,
    /// Declared execution scope.
    pub execution_scope: String,
    /// Declared firmware id.
    pub firmware_id: String,
    /// Declared firmware sha256.
    pub firmware_sha256: String,
    /// Declared guard.
    pub guard: Guard,
    /// Declared identity.
    pub identity: Identity,
    /// Declared limits.
    pub limits: Vec<String>,
    /// Declared manual exit steps.
    pub manual_exit_steps: Vec<String>,
    /// Declared ram alias.
    pub ram_alias: u64,
    /// Declared ready for hardware.
    pub ready_for_hardware: bool,
    /// Declared review.
    pub review: Review,
}

/// Schema identity for caller-supplied rehearsal records.
pub const REHEARSAL_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-rehearsal.generated.schema.json";
