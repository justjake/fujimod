//! Emulation choices for one processor, kept outside the reference catalog.
//!
//! A processor document records what the emulator chooses rather than what the
//! hardware is: the CPU model standing in for an unavailable one, the runtime
//! ABI, available execution engines, instruction and board-tick rates, the
//! modeled MMU, declared model limits and the power-on RAM fill the emulator
//! applies. Hardware facts stay in the catalog records.

use crate::{BUNDLED_PROCESSOR_EMULATIONS, RunInstructionRate};
use serde::{Deserialize, Serialize};

pub const PROCESSOR_EMULATION_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-processor.schema.json";

/// Emulation choices for one processor identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ProcessorEmulation {
    /// Stable schema URI for this processor-emulation document.
    #[serde(rename = "$schema")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "\"https://fujimod.com/schemas/emulator-processor.schema.json\"")
    )]
    pub schema_uri: String,
    /// Processor-emulation document version.
    pub schema: u32,
    /// Processor identity this document describes.
    pub id: String,
    /// CPU model the emulator executes, standing in for an unavailable one.
    pub cpu_model: String,
    /// Runtime ABI the CPU model presents to firmware and mods.
    pub runtime_abi: String,
    /// Execution engines implemented for this processor, when restricted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub engines: Vec<String>,
    /// Instruction entries consumed per board tick, when the processor declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_per_tick: Option<RunInstructionRate>,
    /// Board ticks per second, when the processor declares its own time base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_ticks_per_second: Option<u64>,
    /// Memory-management model the emulator implements, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmu: Option<String>,
    /// Declared limits of the emulator's processor models.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<String>,
    /// The byte every RAM cell holds at power-on, as read on a camera. Absent
    /// when unmeasured; the emulator then starts RAM zeroed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_on_ram_fill: Option<u8>,
}

impl ProcessorEmulation {
    /// Resolve a bundled processor-emulation document by its processor identity.
    pub fn bundled(id: &str) -> Result<Self, String> {
        fujimod_catalog_identity::validate_id(id)?;
        let document = BUNDLED_PROCESSOR_EMULATIONS
            .iter()
            .find_map(|(candidate, document)| (*candidate == id).then_some(*document))
            .ok_or_else(|| format!("unsupported processor emulation: {id}"))?;
        let emulation: Self = serde_json::from_str(document)
            .map_err(|error| format!("processor emulation {id}: {error}"))?;
        emulation.validate(id)?;
        Ok(emulation)
    }

    /// Identities of every bundled processor-emulation document.
    pub fn bundled_ids() -> impl Iterator<Item = &'static str> {
        BUNDLED_PROCESSOR_EMULATIONS.iter().map(|(id, _)| *id)
    }

    /// Check document identity and declared choices.
    pub fn validate(&self, selected_id: &str) -> Result<(), String> {
        if self.schema != 1
            || self.schema_uri != PROCESSOR_EMULATION_SCHEMA_URI
            || self.id != selected_id
        {
            return Err("processor emulation schema or identity differs".into());
        }
        fujimod_catalog_identity::validate_id(&self.id)?;
        if self.cpu_model.trim().is_empty() || self.runtime_abi.trim().is_empty() {
            return Err("processor emulation declares no CPU model or runtime ABI".into());
        }
        if let Some(rate) = &self.instructions_per_tick {
            rate.validate()?;
        }
        if self.board_ticks_per_second == Some(0) {
            return Err("processor emulation board tick rate must be positive".into());
        }
        Ok(())
    }
}
