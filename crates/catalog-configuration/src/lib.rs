//! Closed run-configuration documents kept outside the reference catalog.
//!
//! Configurations select body and firmware identities, execution policy,
//! session choices (settings, cards, cables, scene) and the device policies of
//! the host factory. They hold no hardware: the body record declares its board
//! and resting inputs, and the firmware record how it starts. They do not own
//! reference-document paths, firmware bytes, or caller input paths.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub mod processor;
pub use fujimod_catalog_run_inputs::*;
pub use processor::{PROCESSOR_EMULATION_SCHEMA_URI, ProcessorEmulation};

pub const RUN_CONFIGURATION_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-run-configuration.schema.json";
pub const RUN_OVERLAY_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-run-overlay.schema.json";

include!(concat!(env!("OUT_DIR"), "/bundled_configurations.rs"));

/// A run asset's bytes by name (`configurations/emulator/assets/<name>.bin`):
/// the statistics windows and lens registrations a scene presents.
pub fn bundled_asset(name: &str) -> Option<&'static [u8]> {
    BUNDLED_ASSETS
        .iter()
        .find_map(|(candidate, bytes)| (*candidate == name).then_some(*bytes))
}

/// A validated, named run configuration for one resolved body and firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunConfiguration {
    /// Stable schema URI for this run-configuration document.
    #[serde(rename = "$schema")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "\"https://fujimod.com/schemas/emulator-run-configuration.schema.json\"")
    )]
    pub schema_uri: String,
    /// Run-configuration document version.
    pub schema: u32,
    /// Stable run-configuration identity.
    pub id: String,
    /// Intended operation selected by this run configuration.
    pub purpose: RunPurpose,
    /// Selected body identity.
    pub body_id: String,
    /// Selected firmware identity.
    pub firmware_id: String,
    /// Firmware entry when this run starts other than its firmware's own start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "RunStart | null")
    )]
    pub start: Option<RunStart>,
    /// Execution policy for this run.
    #[serde(default)]
    pub execution: Option<RunExecution>,
    /// External scene inputs generated for this run.
    #[serde(default)]
    pub scene: Option<RunScene>,
    /// Test signals this run presents to audio serial receivers; a block without one models only its reset handshake.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_signals: Vec<RunAudioSignal>,
    /// Named overlays to apply in declaration order.
    pub overlays: Vec<String>,
    /// Browser camera attachments declared by a selected overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(default))]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "RunBrowserCamera | null")
    )]
    pub browser_camera: Option<RunBrowserCamera>,
    /// Explicit device policy and startup selections for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<RunPreset>,
}

impl RunConfiguration {
    /// Backend selected for Cortex-A7 processors in this run.
    pub fn cpu_backend(&self) -> RunCpuBackend {
        self.preset
            .as_ref()
            .and_then(|preset| preset.factory_options.cpu_backend)
            .unwrap_or_default()
    }

    /// Resolve a bundled run-configuration by its stable identity.
    pub fn bundled(id: &str) -> Result<Self, String> {
        fujimod_catalog_identity::validate_id(id)?;
        let document = BUNDLED_RUN_CONFIGURATIONS
            .iter()
            .find_map(|(candidate, document)| (*candidate == id).then_some(*document))
            .ok_or_else(|| format!("unsupported run configuration: {id}"))?;
        let configuration: Self = serde_json::from_str(document)
            .map_err(|error| format!("run configuration {id}: {error}"))?;
        configuration.validate(id)?;
        Ok(configuration)
    }

    /// Identities of every bundled run configuration.
    pub fn bundled_ids() -> impl Iterator<Item = &'static str> {
        BUNDLED_RUN_CONFIGURATIONS.iter().map(|(id, _)| *id)
    }

    /// Check document identity and scene asset references before use.
    pub fn validate(&self, selected_id: &str) -> Result<(), String> {
        if self.schema != 1
            || self.schema_uri != RUN_CONFIGURATION_SCHEMA_URI
            || self.id != selected_id
        {
            return Err("run configuration schema or identity differs".into());
        }
        if let Some(preset) = &self.preset {
            preset.validate()?;
        }
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.body_id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let mut overlays = BTreeSet::new();
        for overlay in &self.overlays {
            fujimod_catalog_identity::validate_id(overlay)?;
            if !overlays.insert(overlay) {
                return Err("run configuration repeats an overlay identity".into());
            }
        }
        if let Some(execution) = &self.execution {
            if execution.quantum == 0 {
                return Err("run execution quantum must be positive".into());
            }
            execution.instructions_per_tick.validate()?;
        }
        if let Some(scene) = &self.scene {
            scene.validate()?;
        }
        if let Some(browser) = &self.browser_camera {
            browser.validate()?;
        }
        Ok(())
    }
}

/// Intended operation selected by a run configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunPurpose {
    /// Start the selected firmware for normal operation.
    RomBoot,
    /// Prepare a service-session run.
    ServiceSession,
    /// Prepare a rehearsal of a declared service operation.
    Rehearsal,
    /// Prepare a boot-access probe.
    BootAccess,
    /// Prepare a controlled shutdown run.
    Shutdown,
}

/// Closed instruction-derived timebase ratio for a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunInstructionRate {
    /// Instruction entries consumed per board tick.
    pub numerator: u64,
    /// Board ticks advanced per instruction entry.
    pub denominator: u64,
}

impl RunInstructionRate {
    /// Check that both terms of the rate are positive and bounded.
    pub fn validate(&self) -> Result<(), String> {
        if self.numerator == 0
            || self.denominator == 0
            || self.numerator > 1 << 32
            || self.denominator > 1 << 32
        {
            return Err("instruction-per-tick ratio must be positive and bounded".into());
        }
        Ok(())
    }
}

/// Browser camera attachments: a virtual card, still-capture policy and LCD preview cadence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunBrowserCamera {
    /// Blank virtual card inserted into the declared card slot.
    pub virtual_card: RunVirtualCard,
    /// Still-capture policy identity.
    pub still_capture: RunStillCapture,
    /// LCD preview statistics cadence.
    pub preview: RunPreview,
    /// The image sensor's scene input.
    pub sensor: RunBrowserSensor,
}

impl RunBrowserCamera {
    /// Check every declared attachment before a run is prepared.
    pub fn validate(&self) -> Result<(), String> {
        self.virtual_card.validate()?;
        self.preview.validate()?;
        self.sensor.validate()
    }
}

/// The browser's external scene input for the image sensor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunBrowserSensor {
    /// Identity of the run's scene asset that supplies the sensor's exposure input.
    pub frames: String,
}

impl RunBrowserSensor {
    fn validate(&self) -> Result<(), String> {
        if self.frames.is_empty() {
            return Err("browser sensor scene input has no frames identity".into());
        }
        Ok(())
    }
}

/// A virtual FAT card geometry declared for one browser camera.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunVirtualCard {
    /// Card filesystem.
    pub filesystem: RunCardFilesystem,
    /// FAT volume label: one to eleven ASCII bytes.
    pub label: String,
    /// Sectors on the blank card.
    pub sectors: u32,
    /// First sector of the FAT partition.
    pub partition_start: u32,
    /// Sectors per FAT cluster.
    pub sectors_per_cluster: u8,
}

impl RunVirtualCard {
    fn validate(&self) -> Result<(), String> {
        if !(1..=11).contains(&self.label.len()) || !self.label.is_ascii() {
            return Err("virtual card label must be one to eleven ASCII bytes".into());
        }
        if self.sectors == 0 || self.partition_start == 0 || self.sectors_per_cluster == 0 {
            return Err("virtual card geometry must be positive".into());
        }
        Ok(())
    }
}

/// Card filesystems a virtual card can declare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunCardFilesystem {
    /// FAT32 with the declared partition start and cluster size.
    Fat32,
}

/// Still-capture policy identities a browser camera can select.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunStillCapture {
    /// Register-level capture, observed read-only on the card.
    RegisterCaptureReadonlyCardObservation,
}

/// LCD preview statistics cadence, in frames.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunPreview {
    /// Preview frames per second: one through sixty.
    pub frames_per_second: u8,
    /// Statistics period while idle: one through sixty frames.
    pub statistics_every_frames: u8,
    /// Statistics period while active: one through sixty frames.
    pub active_statistics_every_frames: u8,
    /// Statistics divider profile identity.
    pub statistics_profile: String,
    /// Declared basis for the cadence.
    pub basis: String,
}

impl RunPreview {
    fn validate(&self) -> Result<(), String> {
        let cadence = |value: u8| (1..=60).contains(&value);
        if !cadence(self.frames_per_second)
            || !cadence(self.statistics_every_frames)
            || !cadence(self.active_statistics_every_frames)
        {
            return Err("preview cadence values must be one through sixty".into());
        }
        if self.statistics_profile.is_empty() || self.basis.is_empty() {
            return Err("preview requires a statistics profile and basis".into());
        }
        Ok(())
    }
}

/// A named, typed change to one run configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunOverlay {
    /// Stable schema URI for this run-overlay document.
    #[serde(rename = "$schema")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "\"https://fujimod.com/schemas/emulator-run-overlay.schema.json\"")
    )]
    pub schema_uri: String,
    /// Run-overlay document version.
    pub schema: u32,
    /// Stable run-overlay identity.
    pub id: String,
    /// Run configuration this overlay applies to.
    pub configuration_id: String,
    /// One typed run input change.
    pub change: RunOverlayChange,
}

impl RunOverlay {
    /// Resolve a bundled run overlay by its stable identity.
    pub fn bundled(id: &str) -> Result<Self, String> {
        fujimod_catalog_identity::validate_id(id)?;
        let document = BUNDLED_RUN_OVERLAYS
            .iter()
            .find_map(|(candidate, document)| (*candidate == id).then_some(*document))
            .ok_or_else(|| format!("unsupported run overlay: {id}"))?;
        let overlay: Self =
            serde_json::from_str(document).map_err(|error| format!("run overlay {id}: {error}"))?;
        overlay.validate(id, &overlay.configuration_id)?;
        Ok(overlay)
    }

    /// Identities of every bundled run overlay.
    pub fn bundled_ids() -> impl Iterator<Item = &'static str> {
        BUNDLED_RUN_OVERLAYS.iter().map(|(id, _)| *id)
    }

    /// Check the overlay identity and its selected configuration.
    pub fn validate(&self, selected_id: &str, configuration_id: &str) -> Result<(), String> {
        if self.schema != 1
            || self.schema_uri != RUN_OVERLAY_SCHEMA_URI
            || self.id != selected_id
            || self.configuration_id != configuration_id
        {
            return Err("run overlay schema or identity differs".into());
        }
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.configuration_id)?;
        match &self.change {
            RunOverlayChange::Preset { preset } => preset.validate(),
            RunOverlayChange::Execution { execution } => {
                if execution.quantum == 0 {
                    return Err("run quantum must be positive".into());
                }
                execution.instructions_per_tick.validate()
            }
            RunOverlayChange::BrowserCamera { browser_camera } => browser_camera.validate(),
            _ => Ok(()),
        }
    }
}

/// The closed set of changes supported by run overlays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
// Catalog records are few and long-lived, so the variant size difference costs nothing.
#[allow(clippy::large_enum_variant)]
pub enum RunOverlayChange {
    /// Select a closed set of startup inputs and device policies.
    Preset { preset: RunPreset },
    /// Connect or disconnect the declared USB cable.
    UsbConnection { connected: bool },
    /// Request named camera settings on top of the configuration's own.
    Settings {
        requested: std::collections::BTreeMap<String, String>,
    },
    /// Select another named firmware entry.
    Start { start: RunStart },
    /// Select one named run operation.
    Purpose { purpose: RunPurpose },
    /// Declare the execution policy for a run whose base configuration has none.
    Execution { execution: RunExecution },
    /// Attach a virtual card, still-capture policy and preview cadence.
    BrowserCamera { browser_camera: RunBrowserCamera },
}

/// Startup policy and synthetic external inputs selected together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunPreset {
    /// Closed device policy selections; omitted fields retain factory defaults.
    pub factory_options: RunFactoryOptions,
    /// Caller-supplied factory input locations selected independently of policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory_inputs_id: Option<String>,
    /// Boot and device layout selected within the chosen firmware identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_profile_id: Option<String>,
    /// Provenance for caller-supplied hardware inputs, keyed by source field path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hardware_notes: BTreeMap<String, String>,
    /// Named scanout selected by firmware observation without firmware addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observer: Option<RunObserver>,
    /// Settings the camera starts with when the run declares no startup inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<RunPowerOnSettings>,
    /// Caller-supplied persistent scene inputs keyed by medium identity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nand_inputs: BTreeMap<String, RunNandSceneInput>,
    /// Card file paths and their external asset identities, grouped by medium.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, BTreeMap<String, String>>,
    /// The reset inputs this run powers on with in place of the body's resting
    /// ones: the controls held and cables connected for a service entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "Record<string, unknown> | null")
    )]
    pub power_on_inputs: Option<HardwareReset>,
    /// Caller-supplied, firmware-qualified input recipe selected by identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_inputs_id: Option<String>,
    /// Inherited policy inputs to clear before applying this preset.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clear_options: Vec<RunClearedOption>,
    /// Intended operation for this input selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<RunPurpose>,
    /// Boot entry when it differs from the base run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<RunStart>,
    /// Initial cable state, including for runs with no complete reset input set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_connected: Option<bool>,
    /// Virtual card declaration without a preview or capture pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_card: Option<RunVirtualCard>,
    /// Complete execution policy when selecting timing or batch options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<RunExecution>,
    /// Restriction on the execution this configuration represents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_scope: Option<RunExecutionScope>,
    /// Declared electrical startup levels, applied after the body reset inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initial_gpio: Vec<RunPadLevel>,
    /// Optional virtual card and preview inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_camera: Option<RunBrowserCamera>,
    /// Explicit limits on this synthetic input selection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<String>,
}

impl RunPreset {
    /// Check preset identities and the options it selects.
    pub fn validate(&self) -> Result<(), String> {
        self.factory_options.validate()?;
        if let Some(id) = &self.factory_inputs_id {
            fujimod_catalog_identity::validate_id(id)?;
        }
        if let Some(id) = &self.firmware_profile_id {
            fujimod_catalog_identity::validate_id(id)?;
        }
        if self
            .clear_options
            .iter()
            .enumerate()
            .any(|(index, option)| self.clear_options[..index].contains(option))
        {
            return Err("run preset repeats a cleared option".into());
        }
        for (id, medium) in &self.nand_inputs {
            fujimod_catalog_identity::validate_id(id)?;
            medium.validate()?;
        }
        if let Some(observer) = &self.observer {
            fujimod_catalog_identity::validate_id(&observer.scanout)?;
        }
        if let Some(id) = &self.firmware_inputs_id {
            fujimod_catalog_identity::validate_id(id)?;
        }
        if let Some(card) = &self.virtual_card {
            card.validate()?;
        }
        if self.start.is_some() && self.factory_options.entry.is_some() {
            return Err("run preset repeats its start entry".into());
        }
        if let Some(execution) = &self.execution {
            if execution.quantum == 0 {
                return Err("run quantum must be positive".into());
            }
            execution.instructions_per_tick.validate()?;
            if self
                .factory_options
                .quantum
                .is_some_and(|value| value != execution.quantum as u64)
                || self
                    .factory_options
                    .instructions_per_tick
                    .is_some_and(|value| {
                        u128::from(value) * u128::from(execution.instructions_per_tick.denominator)
                            != u128::from(execution.instructions_per_tick.numerator)
                    })
            {
                return Err("run factory timing differs from execution policy".into());
            }
        } else if self.factory_options.quantum.is_some()
            || self.factory_options.instructions_per_tick.is_some()
        {
            return Err("run factory timing requires an explicit execution policy".into());
        }
        if let Some(camera) = &self.browser_camera {
            camera.validate()?;
        }
        let mut pads = BTreeSet::new();
        for pad in &self.initial_gpio {
            if pad.port > 5 || pad.pin > 15 || !pads.insert((pad.port, pad.pin)) {
                return Err("run GPIO input is outside the EXR2 pad range or repeated".into());
            }
        }
        Ok(())
    }
}

/// Named device observed by a firmware readiness monitor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunObserver {
    pub scanout: String,
    pub instance: u8,
}

/// A declared electrical startup input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunPadLevel {
    pub port: u8,
    pub pin: u8,
    pub level: bool,
    pub basis: String,
}

/// Execution eligibility carried by a run selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum RunExecutionScope {
    #[serde(rename = "emulator-only")]
    EmulatorOnly,
    #[serde(rename = "isolated-driver")]
    IsolatedDriver,
    #[serde(rename = "offline-boot-development")]
    OfflineBootDevelopment,
}

/// A host test signal presented to an audio serial receiver.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunAudioSignal {
    /// Audio serial block receiving the signal.
    pub block_id: String,
    /// Declared sample rate; the serial clock dividers are not decoded.
    pub sample_rate_hz: u64,
    /// Sine tone frequency.
    pub tone_hz: u64,
    /// Sine tone amplitude in thousandths of full scale.
    pub tone_amplitude_thousandths: u16,
}

/// Execution policy selected by the run configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunExecution {
    /// Strictly model unsupported hardware and bus behavior.
    pub mode: RunExecutionMode,
    /// CPU engine selected for the configured processors.
    pub engine: RunEngine,
    /// Maximum instruction count scheduled in one batch.
    pub quantum: usize,
    /// Instruction-derived virtual-time ratio.
    pub instructions_per_tick: RunInstructionRate,
}

/// A supported run execution mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunExecutionMode {
    /// Refuse unsupported hardware or bus behavior.
    Strict,
}

/// A supported processor execution backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunEngine {
    /// Run the configured processors through the Unicorn backend.
    Unicorn,
    /// Run AArch64 processors through Hypervisor.framework; M-profile companions use Unicorn.
    Hypervisor,
}

impl std::str::FromStr for RunEngine {
    type Err = serde::de::value::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::deserialize(serde::de::value::StrDeserializer::new(value))
    }
}

/// Model-specific scene inputs and the devices that consume them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunScene {
    /// Generated scene assets for an EXR2-based body.
    Exr2(Exr2RunScene),
    /// External scene inputs for an XP4-based body.
    Xp4(fujimod_catalog_run_inputs::application::ApplicationRunScene),
    /// External scene inputs for an XP5-based body.
    Xp5(fujimod_catalog_run_inputs::application::ApplicationRunScene),
}

impl RunScene {
    /// Check scene bounds and generated-asset references.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Exr2(scene) => scene.validate(),
            Self::Xp4(scene) => scene.validate(),
            Self::Xp5(scene) => scene.validate(),
        }
    }
}

/// Generated scene assets and the devices that consume them on EXR2 bodies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct Exr2RunScene {
    /// Deterministic external byte assets generated from fixed patterns.
    pub generated_assets: Vec<GeneratedSceneAsset>,
    /// Image input presented to live-view hardware.
    pub images: RunImages,
    /// Statistics input presented to live-view hardware.
    pub statistics: RunStatistics,
    /// Initial frame clock period in virtual ticks.
    pub frame_period_ticks: u64,
}

impl Exr2RunScene {
    fn validate(&self) -> Result<(), String> {
        if self.generated_assets.is_empty() || self.generated_assets.len() > 256 {
            return Err("run scene generated-asset count is outside its bound".into());
        }
        let mut assets = BTreeMap::new();
        for asset in &self.generated_assets {
            asset.validate()?;
            if assets.insert(asset.id.as_str(), asset.size).is_some() {
                return Err("run scene repeats a generated asset identity".into());
            }
        }
        if self.frame_period_ticks == 0 {
            return Err("run scene frame period must be positive".into());
        }
        if self.images.live_image.width == 0
            || self.images.live_image.height == 0
            || self.images.live_image.stride < self.images.live_image.width
        {
            return Err("run scene image raster is invalid".into());
        }
        let image_bytes = self
            .images
            .live_image
            .stride
            .checked_mul(self.images.live_image.height)
            .ok_or("run scene image extent overflows")?;
        if assets.get(self.images.live_image.asset.as_str()) != Some(&image_bytes) {
            return Err("run scene image asset does not match its raster extent".into());
        }
        if self.statistics.live_statistics.len() > 256 {
            return Err("run scene has too many statistics spans".into());
        }
        for span in &self.statistics.live_statistics {
            fujimod_catalog_identity::validate_id(&span.asset)?;
            if !assets.contains_key(span.asset.as_str()) {
                return Err("run scene statistics refer to an undeclared asset".into());
            }
        }
        Ok(())
    }
}

/// A generated run-scene input asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct GeneratedSceneAsset {
    /// Stable asset identity scoped to the configuration.
    pub id: String,
    /// Repeated bytes used to generate the asset.
    pub pattern: Vec<u8>,
    /// Generated byte length.
    pub size: usize,
}

impl GeneratedSceneAsset {
    fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        if self.pattern.is_empty()
            || self.size == 0
            || self.size > 64 << 20
            || !self.size.is_multiple_of(self.pattern.len())
        {
            return Err("run scene generated asset has an invalid extent or pattern".into());
        }
        Ok(())
    }
}

/// The named image sources used by the run scene.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunImages {
    /// Live-view image and raster layout.
    pub live_image: RunImage,
}

/// A live-view image source and its raster layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunImage {
    /// Raster width in pixels.
    pub width: usize,
    /// Raster height in pixels.
    pub height: usize,
    /// Raster row stride in bytes.
    pub stride: usize,
    /// Generated scene asset identity.
    pub asset: String,
}

/// The named statistics sources used by the run scene.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunStatistics {
    /// Statistics spans presented to live-view hardware.
    pub live_statistics: Vec<RunStatisticsSpan>,
}

/// One statistics asset span.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunStatisticsSpan {
    /// Statistics destination selected by the device implementation.
    pub destination: usize,
    /// Byte offset within the destination.
    pub offset: u32,
    /// Generated scene asset identity.
    pub asset: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_run_document_validates_without_hardware_resolution() {
        for (id, _) in BUNDLED_RUN_CONFIGURATIONS {
            RunConfiguration::bundled(id).unwrap_or_else(|error| panic!("{id}: {error}"));
        }
        for (id, _) in BUNDLED_RUN_OVERLAYS {
            RunOverlay::bundled(id).unwrap_or_else(|error| panic!("{id}: {error}"));
        }
    }

    #[test]
    fn preset_options_are_closed_and_preserve_explicit_disconnection() {
        let overlay = RunOverlay::bundled("x70-110-browser-usb").unwrap();
        let RunOverlayChange::Preset { preset } = overlay.change else {
            panic!("expected preset");
        };
        assert_eq!(preset.usb_connected, Some(true));
        assert_eq!(
            preset.factory_options.secondary_bus_profile,
            Some(RunSecondaryBusProfile::Disabled)
        );
        assert!(
            serde_json::from_str::<RunFactoryOptions>(r#"{"storage_profile":"unknown"}"#).is_err()
        );
        assert!(serde_json::from_str::<RunFactoryOptions>(r#"{"unmodeled_input":true}"#).is_err());
        assert!(
            serde_json::from_str::<RunFactoryOptions>(r#"{"secondary_bus_profile":null}"#)
                .unwrap()
                .secondary_bus_profile
                .is_none()
        );
        let mut overlay = RunOverlay::bundled("x70-110-browser-camera").unwrap();
        let RunOverlayChange::Preset { preset } = &mut overlay.change else {
            panic!("expected preset");
        };
        preset.factory_options.quantum = Some(0);
        assert!(
            overlay
                .validate(&overlay.id, &overlay.configuration_id)
                .is_err()
        );
    }

    #[test]
    fn sd_boot_presets_keep_storage_and_cleared_journals() {
        let overlay = RunOverlay::bundled("xm1-150-sd-card-boot-rec").unwrap();
        let RunOverlayChange::Preset { preset } = overlay.change else {
            panic!("expected preset");
        };
        assert_eq!(preset.start, Some(RunStart::SdCard));
        assert_eq!(
            preset.factory_options.storage_profile,
            Some(RunStorageProfile::Erased)
        );
        assert_eq!(preset.factory_options.settings, Some(RunSettings::Default));
        assert!(
            preset
                .clear_options
                .contains(&RunClearedOption::UserJournals)
        );
        assert_eq!(preset.factory_options.sd_card_present, Some(true));
    }

    #[test]
    fn bundled_configuration_is_identity_checked_and_declares_no_hardware() {
        let config = RunConfiguration::bundled("x70-110-rec").unwrap();
        let Some(RunScene::Exr2(scene)) = config.scene.as_ref() else {
            panic!("X70 run scene must be EXR2 scene inputs");
        };
        assert_eq!(config.id, "x70-110-rec");
        assert_eq!(config.body_id, "x70");
        assert_eq!(config.firmware_id, "x70-110");
        assert_eq!(config.purpose, RunPurpose::RomBoot);
        assert_eq!(config.start, None, "the firmware declares how it starts");
        let execution = config.execution.as_ref().unwrap();
        assert_eq!(execution.mode, RunExecutionMode::Strict);
        assert_eq!(
            execution.instructions_per_tick,
            RunInstructionRate {
                numerator: 24,
                denominator: 1
            }
        );
        assert_eq!(scene.frame_period_ticks, 400000);
        assert!(config.overlays.is_empty());
        assert!(RunConfiguration::bundled("unknown").is_err());
        // Hardware lives in the body record: a run declaring its inputs is refused.
        let mut document: serde_json::Value = serde_json::from_str(
            BUNDLED_RUN_CONFIGURATIONS
                .iter()
                .find(|(id, _)| *id == "x70-110-rec")
                .unwrap()
                .1,
        )
        .unwrap();
        document["inputs"] = serde_json::json!(null);
        assert!(serde_json::from_value::<RunConfiguration>(document).is_err());
    }

    #[test]
    fn scene_asset_references_are_closed_and_bounded() {
        let mut config = RunConfiguration::bundled("x70-110-rec").unwrap();
        let Some(RunScene::Exr2(scene)) = config.scene.as_mut() else {
            panic!("X70 run scene must be EXR2 scene inputs");
        };
        scene.images.live_image.asset = "missing".into();
        assert!(config.validate("x70-110-rec").is_err());
        config = RunConfiguration::bundled("x70-110-rec").unwrap();
        let Some(RunScene::Exr2(scene)) = config.scene.as_mut() else {
            panic!("X70 run scene must be EXR2 scene inputs");
        };
        scene.generated_assets[0].pattern.clear();
        assert!(config.validate("x70-110-rec").is_err());
        assert!(config.validate("different").is_err());
    }

    #[test]
    fn overlay_is_bound_to_its_configuration_and_has_a_typed_change() {
        let overlay = RunOverlay {
            schema_uri: RUN_OVERLAY_SCHEMA_URI.into(),
            schema: 1,
            id: "x70-110-rec-usb".into(),
            configuration_id: "x70-110-rec".into(),
            change: RunOverlayChange::UsbConnection { connected: true },
        };
        assert!(overlay.validate("x70-110-rec-usb", "x70-110-rec").is_ok());
        assert!(overlay.validate("x70-110-rec-usb", "different").is_err());
        assert!(overlay.validate("different", "x70-110-rec").is_err());

        let bundled = RunOverlay::bundled("x70-110-rec-usb").unwrap();
        assert_eq!(bundled.configuration_id, "x70-110-rec");
        assert_eq!(
            bundled.change,
            RunOverlayChange::UsbConnection { connected: true }
        );
    }

    #[test]
    fn xe5_bundles_an_sd_card_start() {
        let config = RunConfiguration::bundled("xe5-112").unwrap();
        let overlay = RunOverlay::bundled("xe5-112-sd-card-start").unwrap();
        overlay.validate(&overlay.id, &config.id).unwrap();
        assert_eq!(
            overlay.change,
            RunOverlayChange::Start {
                start: RunStart::SdCard
            }
        );
    }

    #[test]
    fn statistics_offsets_are_destination_offsets() {
        let mut config = RunConfiguration::bundled("x70-110-rec").unwrap();
        let Some(RunScene::Exr2(scene)) = config.scene.as_mut() else {
            panic!("X70 run scene must be EXR2 scene inputs");
        };
        scene.statistics.live_statistics[0].offset = u32::MAX;
        assert!(config.validate("x70-110-rec").is_ok());
    }
}
