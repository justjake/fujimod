//! Recorded bring-up progress and separate publishing overrides for one firmware.
//!
//! Each stage holds the latest recorded result of that stage for the firmware.
//! A result whose inputs changed since it was recorded is `stale` and does not
//! count as progress.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROGRESS_SCHEMA_URI: &str = "https://fujimod.com/schemas/progress.generated.schema.json";
pub const PROGRESS_OVERRIDE_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/progress-override.generated.schema.json";

/// A maintainer's publishing decision, separate from recorded progress.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProgressOverride {
    /// Schema identifying the hand-authored document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Firmware whose published progress changes.
    pub firmware_id: String,
    /// Show every phase and step as complete.
    pub complete: bool,
}

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
    /// Bring-up tracks and their stages, in the bring-up graph's order.
    pub tracks: Vec<TrackProgress>,
    /// Stock camera features the emulator is graded on, by feature; only features the body has.
    pub stock: BTreeMap<String, StockProgress>,
    /// Walkthrough result of every shipped mod, by mod name.
    pub mods: BTreeMap<String, ModProgress>,
}

/// One bring-up track: static analysis, emulator, camera, FujiMod, or SD card and install.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TrackProgress {
    /// Stable track identifier.
    pub id: String,
    /// Track title as the bring-up runner prints it.
    pub title: String,
    /// CSS phase identifier used by the website's progress bars.
    pub phase: String,
    /// Share of the progress bar assigned to this track.
    pub weight: u32,
    /// Draw each step as its own cell instead of a single fill.
    pub cells: bool,
    /// The track's stages, in order.
    pub stages: Vec<StageEntry>,
    /// The ordered steps the website draws, resolved from the graph.
    pub steps: Vec<StepProgress>,
}

/// One bring-up stage and its latest result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StageEntry {
    /// Stable stage identifier.
    pub id: String,
    /// Stage title as the bring-up runner prints it.
    pub title: String,
    pub result: StageProgress,
}

/// One rendered step of a track.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StepProgress {
    /// Stable step identifier, including a stock feature or mod suffix.
    pub id: String,
    /// Step label shown in the progress bar.
    pub label: String,
    /// Graph stage that owns this step.
    pub stage: String,
    pub result: StepResult,
    /// Graded parts that pass.
    pub passed: u32,
    /// Graded parts in total.
    pub total: u32,
    /// Human-readable detail for a partial step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
    /// Some of the stage's graded parts pass (stock features, mod walkthroughs).
    Partial,
    Fail,
    /// Recorded, but its inputs changed since.
    Stale,
    NotRun,
}

/// Latest result of one rendered step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum StepResult {
    Pass,
    /// Some graded parts pass.
    Partial,
    Fail,
    Stale,
    NotRun,
    /// The mod cannot be built for this firmware.
    Unavailable,
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
