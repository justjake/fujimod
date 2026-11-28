//! Where a firmware keeps the state that says its camera is at REC.
//!
//! An X-Processor 5 firmware enters REC before it draws the shooting screen,
//! so an open watches the located UI mode, system mode, pending mode and menu
//! flag, checks the stock code is intact, and waits until REC with a drawn
//! screen has held for the settle time. Every address is located in the
//! firmware; the host only reads them, never writes or substitutes them.
use serde::{Deserialize, Serialize};

pub const REC_READINESS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/rec-readiness.generated.schema.json";

/// One located state word: its address and width.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct StateWord {
    /// Physical address of the word.
    pub address: u64,
    /// Width in bytes, 1 to 4, little-endian.
    pub bytes: u8,
}

/// A stock code range whose digest must hold while REC is observed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct IdentityCheck {
    pub address: u64,
    pub size: u64,
    pub sha256: String,
}

/// The located REC state of one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RecReadiness {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub id: String,
    pub firmware_id: String,
    /// SHA-256 of the update the addresses were located in.
    pub firmware_sha256: String,
    /// The current UI mode.
    pub ui_mode: StateWord,
    /// The UI mode a mode change is about to commit; zero when none is pending.
    pub pending_mode: StateWord,
    /// The system mode the located getter returns.
    pub system_mode: StateWord,
    /// The menu flag; zero while no menu is open.
    pub menu_active: StateWord,
    /// The UI mode value of REC.
    pub rec_ui_mode: u32,
    /// The system mode value the firmware holds at REC.
    pub rec_system_mode: u32,
    /// How long REC with a drawn screen must hold before the camera is at REC.
    pub settle_milliseconds: u64,
    /// Stock code that must be unchanged while REC is observed.
    pub identity_checks: Vec<IdentityCheck>,
}

impl RecReadiness {
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let words = [
            self.ui_mode,
            self.pending_mode,
            self.system_mode,
            self.menu_active,
        ];
        if self.schema_uri != REC_READINESS_SCHEMA_URI
            || self.schema != 1
            || self.id != format!("{}-rec-readiness", self.firmware_id)
            || self.firmware_sha256.len() != 64
            || !self.firmware_sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || words.iter().any(|word| !(1..=4).contains(&word.bytes))
            || self.identity_checks.iter().any(|check| {
                check.size == 0
                    || check.sha256.len() != 64
                    || !check.sha256.bytes().all(|c| c.is_ascii_hexdigit())
            })
        {
            return Err(format!(
                "firmware {} REC readiness is invalid",
                self.firmware_id
            ));
        }
        Ok(())
    }
}
