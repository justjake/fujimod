//! Generated bring-up progress for one firmware.
//!
//! Each stage holds the latest recorded result of that stage for the firmware.
//! A result whose inputs changed since it was recorded is `stale` and does not
//! count as progress.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROGRESS_SCHEMA_URI: &str = "https://fujimod.com/schemas/progress.generated.schema.json";

/// Bring-up progress toward FujiMod on one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FirmwareProgress {
    /// Schema identifying the generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Firmware the progress belongs to.
    pub firmware_id: String,
    /// Camera model name, as the camera reports it.
    pub model: String,
    /// Firmware version.
    pub version: String,
    /// Located firmware facts, when the firmware has a facts document.
    pub facts: Option<FactCount>,
    /// Camera model whose boot ROM dump this firmware boots with, if any.
    pub boot_rom: Option<String>,
    /// Bring-up stages, in order.
    pub stages: ProgressStages,
    /// Stock camera features the emulator is graded on, by feature; only features the body has.
    pub stock: BTreeMap<String, StockProgress>,
    /// Walkthrough result of every shipped mod, by mod name.
    pub mods: BTreeMap<String, ModProgress>,
}

/// Bring-up stages, grouped as research, emulator, camera, mods and SD card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ProgressStages {
    /// The update file was registered, its overlays unpacked and every code unit's corpus exported.
    pub research: StageProgress,
    /// The function index was built and the firmware's facts located.
    pub analysis: StageProgress,
    /// The emulator boots to the shooting screen.
    pub rec: StageProgress,
    /// The emulator's menus respond to remote control.
    pub menu: StageProgress,
    /// A camera was identified over USB.
    pub camera_identity: StageProgress,
    /// A camera's memory was captured over USB.
    pub camera_snapshot: StageProgress,
    /// A camera's menus were driven over USB.
    pub camera_menu: StageProgress,
    /// The mod target was generated from the firmware's facts.
    pub mod_target: StageProgress,
    /// Every shipped mod was built for the firmware.
    pub mod_build: StageProgress,
    /// The dual-boot SD card image was built.
    pub sd_image: StageProgress,
    /// A camera booted the SD card image.
    pub sd_boot: StageProgress,
}

/// How many of a firmware's facts were located.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FactCount {
    pub located: u32,
    pub total: u32,
}

/// Latest grade of one stock-feature walkthrough.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StockProgress {
    pub result: StockResult,
    /// Graded parts that pass, for features graded in parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<u32>,
    /// Graded parts in total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
}

/// Grade of one stock feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum StockResult {
    Pass,
    /// Some graded parts pass.
    Partial,
    Fail,
    /// Recorded, but its inputs changed since.
    Stale,
    NotRun,
}

/// Latest result of one stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum StageProgress {
    Pass,
    Fail,
    /// Recorded, but its inputs changed since.
    Stale,
    NotRun,
}

/// Latest walkthrough result of one mod.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ModProgress {
    Pass,
    Fail,
    /// The mod cannot be built for this firmware.
    Unavailable,
    /// Recorded, but its inputs changed since.
    Stale,
    NotRun,
}
