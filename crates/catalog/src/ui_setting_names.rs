//! Stock setting identities returned by a firmware's setting-name routine.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const UI_SETTING_NAMES_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/ui-setting-names.generated.schema.json";

/// Exact stock setting names and their firmware-specific numeric IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct UiSettingNames {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub id: String,
    pub firmware_id: String,
    /// SHA-256 of the update from which the names were located.
    pub firmware_sha256: String,
    /// Every name in the located stock setting table, without renaming or aliases.
    pub ui_setting_names: BTreeMap<String, u32>,
}

impl UiSettingNames {
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let mut ids = BTreeSet::new();
        if self.schema_uri != UI_SETTING_NAMES_SCHEMA_URI
            || self.schema != 1
            || self.id != format!("{}-ui-setting-names", self.firmware_id)
            || self.firmware_sha256.len() != 64
            || !self.firmware_sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || self.ui_setting_names.is_empty()
            || self.ui_setting_names.get("UI_SETP_NONE") != Some(&0)
            || self.ui_setting_names.iter().any(|(name, id)| {
                name.is_empty()
                    || !name
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
                    || *id >= 65536
                    || !ids.insert(*id)
            })
        {
            return Err("invalid stock setting-name identities".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> UiSettingNames {
        UiSettingNames {
            schema_uri: UI_SETTING_NAMES_SCHEMA_URI.into(),
            schema: 1,
            id: "example-100-ui-setting-names".into(),
            firmware_id: "example-100".into(),
            firmware_sha256: "a".repeat(64),
            ui_setting_names: BTreeMap::from([
                ("UI_SETP_NONE".into(), 0),
                ("UI_SETP_COLOR_SPACE".into(), 41),
                ("UI_SET_LAST_READ_FULLPATH".into(), 836),
            ]),
        }
    }

    #[test]
    fn accepts_stock_spellings_and_sparse_ids() {
        let document = names();
        document.validate().unwrap();
        assert_eq!(
            serde_json::from_slice::<UiSettingNames>(&serde_json::to_vec(&document).unwrap())
                .unwrap(),
            document
        );
    }

    #[test]
    fn rejects_duplicate_ids_missing_zero_and_mismatched_identity() {
        let mut document = names();
        document
            .ui_setting_names
            .insert("UI_SETP_OTHER".into(), 41);
        assert!(document.validate().is_err());
        let mut document = names();
        document.ui_setting_names.remove("UI_SETP_NONE");
        assert!(document.validate().is_err());
        let mut document = names();
        document.firmware_id = "other-100".into();
        assert!(document.validate().is_err());
    }
}
