//! Emulation choices for one processor, kept outside the reference catalog.
//!
//! A processor document records what the emulator chooses rather than what the
//! hardware is: the CPU model standing in for an unavailable one, the runtime
//! ABI, available execution engines, instruction and board-tick rates, the
//! modeled MMU and declared model limits. Hardware facts stay in the catalog
//! records.

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
    /// PMU counter cycles per board tick, when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pmu_cycles_per_tick: Option<u64>,
    /// Board ticks per second, when the processor declares its own time base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_ticks_per_second: Option<u64>,
    /// Memory-management model the emulator implements, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmu: Option<String>,
    /// Declared limits of the emulator's processor models.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<String>,
    /// Discovery probe apertures the emulator reserves, when it declares any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovery_mmio: Vec<DiscoveryAperture>,
    /// Processor blocks the emulator backs privately for each CPU.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub private_mmio: Vec<String>,
    /// Blocks whose unmodeled aligned words read back their last write instead of stopping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_word_blocks: Vec<String>,
    /// Functional completion latencies the emulator gives blocks; none is measured.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completion_latencies: Vec<BlockLatency>,
}

/// A functional completion latency for one block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BlockLatency {
    /// Processor block the latency applies to.
    pub block_id: String,
    /// Board ticks from a start to its completion.
    pub ticks: u64,
}

/// One discovery probe aperture, reserved below the high vectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct DiscoveryAperture {
    /// First address of the aperture.
    pub address: u64,
    /// Aperture size in bytes.
    pub size: u64,
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
        if self.pmu_cycles_per_tick == Some(0) {
            return Err("processor emulation PMU cycles per tick must be positive".into());
        }
        if self.board_ticks_per_second == Some(0) {
            return Err("processor emulation board tick rate must be positive".into());
        }
        for aperture in &self.discovery_mmio {
            if aperture.size == 0
                || aperture.address % 0x1000 != 0
                || aperture.size % 0x1000 != 0
                || aperture.address + aperture.size > 0xffff_0000
            {
                return Err("processor emulation discovery aperture is not a bounded peripheral page range".into());
            }
        }
        for id in &self.private_mmio {
            fujimod_catalog_identity::validate_id(id)?;
        }
        Ok(())
    }
}
