//! Generated stock first-start menu facts for one firmware.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const FIRST_START_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/first-start.generated.schema.json";

/// Stock language rows and the calendar editor observation layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirstStartMenu {
    /// Schema identifying the generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// Firmware that offers these menus.
    pub firmware_id: String,
    /// Stock initial language node address.
    pub menu_node: u64,
    /// Current language row observation address.
    pub cursor: u64,
    /// Child table pointer offset in the menu descriptor.
    pub children_offset: u64,
    /// Child table row size in bytes.
    pub row_bytes: u64,
    /// Offered languages in native row order.
    pub languages: Vec<FirstStartLanguage>,
    /// Calendar menu kind, including its active field nodes.
    pub date_kind: u32,
    /// Calendar editor's year, month, day, hour, minute and selection observation.
    pub date_editor: u64,
}

/// One offered stock language and its stored preference value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirstStartLanguage {
    /// Native menu key; independent of the stored preference value.
    pub key: u32,
    /// Stored language preference value.
    pub value: u32,
    /// Exact stock BKUP_LANGUAGE name.
    pub name: String,
}

impl FirstStartMenu {
    /// Validate document identity, bounded rows and unambiguous language values.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        if self.schema_uri != FIRST_START_SCHEMA_URI
            || self.schema != 1
            || self.menu_node == 0
            || self.cursor == 0
            || self.date_editor == 0
            || self.row_bytes < 8
            || self.row_bytes > 256
            || self.languages.is_empty()
            || self.languages.len() > 256
        {
            return Err("invalid first-start document identity or menu layout".into());
        }
        let mut keys = BTreeSet::new();
        let mut values = BTreeSet::new();
        let mut names = BTreeSet::new();
        for language in &self.languages {
            if !keys.insert(language.key)
                || !values.insert(language.value)
                || !names.insert(&language.name)
                || !language.name.starts_with("BKUP_LANGUAGE_")
            {
                return Err("first-start languages have repeated or unknown identities".into());
            }
        }
        Ok(())
    }
}
