//! Stock event identities returned by a firmware's event-name routine.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const UI_EVENT_CODES_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/ui-event-codes.generated.schema.json";

/// Exact stock event names and their firmware-specific numeric codes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct UiEventCodes {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub id: String,
    pub firmware_id: String,
    /// SHA-256 of the update from which the codes were located.
    pub firmware_sha256: String,
    /// Located stock event names, without renaming or aliases.
    pub ui_event_codes: BTreeMap<String, u32>,
}

impl UiEventCodes {
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let mut ids = BTreeSet::new();
        if self.schema_uri != UI_EVENT_CODES_SCHEMA_URI
            || self.schema != 1
            || self.id != format!("{}-ui-event-codes", self.firmware_id)
            || self.firmware_sha256.len() != 64
            || !self.firmware_sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || self.ui_event_codes.is_empty()
            || self.ui_event_codes.iter().any(|(name, id)| {
                name.is_empty()
                    || !name
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
                    || *id == 0
                    || !ids.insert(*id)
            })
        {
            return Err("invalid stock event-name identities".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes() -> UiEventCodes {
        UiEventCodes {
            schema_uri: UI_EVENT_CODES_SCHEMA_URI.into(),
            schema: 1,
            id: "example-100-ui-event-codes".into(),
            firmware_id: "example-100".into(),
            firmware_sha256: "a".repeat(64),
            ui_event_codes: BTreeMap::from([("UI_EVTID_EXAMPLE".into(), 0x020d0000)]),
        }
    }

    #[test]
    fn accepts_full_width_stock_codes() {
        let value = codes();
        value.validate().unwrap();
        assert_eq!(
            serde_json::from_slice::<UiEventCodes>(&serde_json::to_vec(&value).unwrap()).unwrap(),
            value
        );
    }

    #[test]
    fn rejects_duplicate_codes_and_wrong_identity() {
        let mut value = codes();
        value
            .ui_event_codes
            .insert("UI_EVTID_OTHER".into(), 0x020d0000);
        assert!(value.validate().is_err());
        let mut value = codes();
        value.firmware_id = "other-100".into();
        assert!(value.validate().is_err());
    }
}
