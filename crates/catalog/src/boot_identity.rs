//! Section-table identity of every registered EXR II body.
//!
//! One universal SD-boot stage 1 loads stock stage 1 from NAND and runs one
//! `FM_EXR2.BIN` for every EXR II body, so stage 2 must identify its body
//! itself. Stock stage 1 is not unique (X-M1 1.01, 1.20 and 1.50 and X-A1 1.50
//! share one image), so stage 2 hashes the 1 KiB NAND section table at the
//! offset the loaded stage-1 image names and matches it here. The document
//! holds offsets and hashes, never firmware bytes.
use serde::{Deserialize, Serialize};

pub const EXR2_BOOT_IDENTITY_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/exr2-boot-identity.generated.schema.json";

/// Section-table identity table for the registered EXR II bodies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct Exr2BootIdentity {
    /// Schema identifying this generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// Bytes hashed from each named NAND offset.
    pub section_table_bytes: u64,
    /// One row per registered EXR II body.
    pub firmwares: Vec<Exr2BootIdentityEntry>,
}

/// One body's section-table hash and where the table sits in NAND.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct Exr2BootIdentityEntry {
    /// Release identity the identified body reports.
    pub firmware_id: String,
    /// Native-flash offset of the section table.
    pub section_table_native_offset: u64,
    /// SHA-256 of the section table.
    pub section_table_sha256: String,
}

impl Exr2BootIdentity {
    /// Validate identity, the shared table size and the one-row-per-body rule.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        if self.schema_uri != EXR2_BOOT_IDENTITY_SCHEMA_URI || self.schema != 1 {
            return Err(format!("boot identity {}: invalid schema", self.id));
        }
        if self.section_table_bytes != 1024 {
            return Err("boot identity: invalid section-table size".into());
        }
        if self.firmwares.is_empty() {
            return Err("boot identity: no firmware rows".into());
        }
        let mut seen_ids = std::collections::BTreeSet::new();
        let mut seen_tables = std::collections::BTreeSet::new();
        for row in &self.firmwares {
            fujimod_catalog_identity::validate_id(&row.firmware_id)?;
            fujimod_catalog_identity::validate_sha256(&row.section_table_sha256)?;
            if row.section_table_native_offset % 4 != 0 {
                return Err(format!(
                    "boot identity: {} has an unaligned section table",
                    row.firmware_id
                ));
            }
            if !seen_ids.insert(row.firmware_id.as_str()) {
                return Err(format!(
                    "boot identity: {} is listed twice",
                    row.firmware_id
                ));
            }
            if !seen_tables.insert(row.section_table_sha256.as_str()) {
                return Err(format!(
                    "boot identity: {} repeats a section table",
                    row.firmware_id
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn table() -> Exr2BootIdentity {
        serde_json::from_str(include_str!(
            "../../../catalog/exr2-boot-identity.generated.json"
        ))
        .unwrap()
    }
    #[test]
    fn all_section_tables_are_distinct() {
        let value = table();
        value.validate().unwrap();
        assert_eq!(value.firmwares.len(), 16);
    }
    #[test]
    fn ambiguous_or_malformed_identities_are_refused() {
        let mut value = table();
        value.firmwares[1].section_table_sha256 = value.firmwares[0].section_table_sha256.clone();
        assert!(value.validate().is_err());
        let mut value = table();
        value.section_table_bytes = 512;
        assert!(value.validate().is_err());
        let mut value = table();
        value.firmwares[1].firmware_id = value.firmwares[0].firmware_id.clone();
        assert!(value.validate().is_err());
    }
}
