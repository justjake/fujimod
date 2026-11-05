//! Stock scene identities returned by a firmware's scene-name routine.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const UI_SCENE_NAMES_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/ui-scene-names.generated.schema.json";

/// Exact stock scene names and their firmware-specific numeric IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct UiSceneNames {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub id: String,
    pub firmware_id: String,
    /// SHA-256 of the update from which the names were located and executed.
    pub firmware_sha256: String,
    /// Every name returned by the stock routine, without renaming or aliases.
    pub ui_scene_names: BTreeMap<String, u32>,
}

impl UiSceneNames {
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let mut ids = BTreeSet::new();
        if self.schema_uri != UI_SCENE_NAMES_SCHEMA_URI
            || self.schema != 1
            || self.id != format!("{}-ui-scene-names", self.firmware_id)
            || self.firmware_sha256.len() != 64
            || !self.firmware_sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || self.ui_scene_names.is_empty()
            || self.ui_scene_names.iter().any(|(name, id)| {
                name.is_empty()
                    || !name
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
                    || *id >= 65536
                    || !ids.insert(*id)
            })
        {
            return Err("invalid stock scene-name identities".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> UiSceneNames {
        UiSceneNames {
            schema_uri: UI_SCENE_NAMES_SCHEMA_URI.into(),
            schema: 1,
            id: "example-100-ui-scene-names".into(),
            firmware_id: "example-100".into(),
            firmware_sha256: "a".repeat(64),
            ui_scene_names: BTreeMap::from([
                ("UILIB_SUB_SCR_EXAMPLE".into(), 1),
                ("UI_LIB_SUB_SCR_OTHER".into(), 3),
            ]),
        }
    }

    #[test]
    fn accepts_exact_stock_spellings_and_sparse_ids() {
        let document = names();
        document.validate().unwrap();
        assert_eq!(
            serde_json::from_slice::<UiSceneNames>(&serde_json::to_vec(&document).unwrap())
                .unwrap(),
            document
        );
    }

    #[test]
    fn rejects_duplicate_ids_and_mismatched_identity() {
        let mut document = names();
        document
            .ui_scene_names
            .insert("UILIB_SUB_SCR_THIRD".into(), 1);
        assert!(document.validate().is_err());
        let mut document = names();
        document.firmware_id = "other-100".into();
        assert!(document.validate().is_err());
    }
}
