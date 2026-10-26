//! Caller-supplied evidence notes and qualified input identities.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Schema identity for supplemental catalog provenance.
pub const ANNOTATIONS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/catalog-annotations.generated.schema.json";

/// Evidence that accompanies catalog records without changing their public identities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CatalogAnnotations {
    /// Schema identity.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Source labels and their qualified evidence.
    pub records: BTreeMap<String, SourceAnnotations>,
}

/// Text evidence and explicitly qualified inputs for one source record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceAnnotations {
    /// Logical record and optional execution profile qualified by these notes.
    pub subject: AnnotationSubject,
    /// Hash-qualified source documents retained as evidence, never execution inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_documents: Option<BTreeMap<String, SourceDocument>>,
    /// Explicit synthetic byte fields in additional persistent records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nondefault_records: Option<BTreeMap<String, Vec<StorageSignature>>>,
    /// Hash-qualified manifests for the named firmware execution units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_unit_manifests: Option<BTreeMap<String, InputManifest>>,
    /// Explicit synthetic configuration signature bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_signature: Option<StorageSignature>,
    /// Manifest identity and its explicit caller-selected location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_manifest: Option<InputManifest>,
    /// Source-field paths and their recorded qualification or citations.
    pub notes: BTreeMap<String, TextNote>,
    /// Checked secondary-core release words, when supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_checks: Option<Vec<crate::profiles::ResetCheck>>,
    /// Hash-identified auxiliary firmware images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_blobs: Option<BTreeMap<String, FirmwareBlob>>,
    /// Persistent input contents and placement recipes, without file locations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistent_inputs: Option<fujimod_catalog_settings::persistent_inputs::PersistentInputs>,
    /// Geometry of the persistent medium used by these inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistent_geometry: Option<crate::firmware::PersistentInputsGeometry>,
    /// Explicit caller file locations for the named input assets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_assets: Option<BTreeMap<String, InputAssetLocation>>,
}

/// A complete source inventory retained independently of its execution projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceDocument {
    /// Caller-selected source location.
    pub path: String,
    /// SHA-256 of the complete source document.
    pub sha256: String,
    /// Source format whose fields the evidence preserves.
    pub format: SourceDocumentFormat,
}

/// Formats of retained source evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SourceDocumentFormat {
    /// Located observations and their proof spans.
    LocatedFactsV2,
}

/// Recorded prose or a list of prose citations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum TextNote {
    /// One recorded qualification.
    Text(String),
    /// Ordered qualifications or source citations.
    Lines(Vec<String>),
}

/// A firmware slice identified without embedding executable contents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FirmwareBlob {
    /// Byte offset in the installed flash image.
    pub native_flash_offset: u64,
    /// Image length.
    pub size: u64,
    /// SHA-256 of the image.
    pub sha256: String,
}

/// An explicit input-file slice supplied by the caller.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InputAssetLocation {
    /// Input-file location.
    pub path: String,
    /// Byte offset of the selected input within that file.
    pub file_offset: u64,
}

/// A caller-selected firmware manifest, optionally hash-qualified.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InputManifest {
    /// Manifest location supplied by the caller.
    pub path: String,
    /// Manifest content identity when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// A synthetic signature placed in a configuration block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StorageSignature {
    /// Destination offset within the configuration block.
    pub offset: u64,
    /// Explicit signature bytes.
    pub bytes: Vec<u8>,
}

/// Logical record selected independently of a document location.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AnnotationSubject {
    /// Reference record category.
    pub kind: AnnotationKind,
    /// Stable identity within that category.
    pub id: String,
    /// Named hardware or firmware layout profile when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Reference categories that accept supplemental evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    /// System on chip.
    Soc,
    /// Installed component.
    Part,
    /// Camera body.
    Body,
    /// Firmware installation.
    Firmware,
    /// Firmware table layout.
    FirmwareView,
    /// Located firmware observations and their source evidence.
    FirmwareFacts,
}
