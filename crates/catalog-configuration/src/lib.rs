//! Closed run-configuration documents kept outside the reference catalog.
//!
//! Configurations select body and firmware identities, a boot entry, execution
//! policy, synthetic reset inputs, and generated external scene inputs. They
//! do not own hardware maps, reference-document paths, firmware bytes, or
//! caller input paths.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub mod application_inputs;
use fujimod_catalog_run_inputs::*;

pub const RUN_CONFIGURATION_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-run-configuration.schema.json";
pub const RUN_OVERLAY_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-run-overlay.schema.json";

include!(concat!(env!("OUT_DIR"), "/bundled_configurations.rs"));

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
    /// Firmware entry mode selected for this run.
    pub start: RunStart,
    /// Execution policy for this run.
    #[serde(default)]
    pub execution: Option<RunExecution>,
    /// Declared model-specific reset inputs, addressed by catalog identity.
    #[serde(default)]
    pub inputs: Option<RunInputs>,
    /// External scene inputs generated for this run.
    #[serde(default)]
    pub scene: Option<RunScene>,
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
        if let Some(inputs) = &self.inputs {
            inputs.validate()?;
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

/// A supported firmware start entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunStart {
    /// Reset through the selected firmware's real boot ROM.
    Rom,
    /// Enter through the selected firmware's loader entry.
    Loader,
    /// Enter through a removable-card boot entry.
    SdCard,
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

/// Model-specific synthetic startup inputs applied to resolved hardware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunInputs {
    /// Reset inputs for an EXR2-based body.
    Exr2(Exr2RunInputs),
    /// Reset inputs for an XP4-based body.
    Xp4(crate::application_inputs::ApplicationRunInputs),
    /// Reset inputs for an XP5-based body.
    Xp5(crate::application_inputs::ApplicationRunInputs),
}

impl RunInputs {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Exr2(inputs) => inputs.validate(),
            Self::Xp4(inputs) => inputs.validate(),
            Self::Xp5(inputs) => inputs.validate(),
        }
    }
}

/// Synthetic EXR2 startup inputs applied to resolved blocks and parts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct Exr2RunInputs {
    /// GPIO reset state and explicitly released controls.
    pub gpio: RunGpioInputs,
    /// Initial phases for physical quadrature encoders.
    pub encoders: Vec<RunEncoderInput>,
    /// ADC reset samples.
    pub adc: RunAdcInputs,
    /// Power-controller reset input.
    pub power: RunPowerInput,
    /// MCU persistent input and RTC start state.
    pub mcu: RunMcuInput,
    /// Other declared reset values for serial-backed parts.
    pub serial_registers: Vec<RunSerialRegisters>,
    /// Initial USB cable state.
    pub usb: RunUsbInput,
    /// Initial SD card insertion and protection state.
    pub card: RunCardInput,
    /// Policy for accepting a firmware first-start prompt, when needed.
    #[serde(default)]
    pub first_start: Option<RunFirstStart>,
    /// The settings the camera starts with. Absent, the flash holds what the
    /// update installs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<RunPowerOnSettings>,
}

/// A camera's settings at power-on: its settings bank, the firmware's default
/// user settings and the named changes a run requests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunPowerOnSettings {
    /// The body's camera whose NAND sections the run supplies for the settings
    /// bank (`catalog/bodies/<body>/cameras/<camera>`). Absent, the bank is
    /// built from the update's defaults table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// Requested settings by name and value name, such as `setup: done`.
    pub requested: BTreeMap<String, String>,
}

impl RunPowerOnSettings {
    fn validate(&self) -> Result<(), String> {
        if let Some(camera) = &self.camera {
            fujimod_catalog_identity::validate_id(camera)?;
        }
        if self.requested.len() > 64 {
            return Err("run settings request too many changes".into());
        }
        for (name, value) in &self.requested {
            fujimod_catalog_identity::validate_id(name)?;
            fujimod_catalog_identity::validate_id(value)?;
        }
        Ok(())
    }
}

impl Exr2RunInputs {
    fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.gpio.block_id)?;
        if self.gpio.keys.len() > 64
            || self.gpio.connections.len() > 64
            || self.gpio.outputs.len() > 256
        {
            return Err("run GPIO input count exceeds its bound".into());
        }
        let mut keys = BTreeSet::new();
        for key in &self.gpio.keys {
            fujimod_catalog_identity::validate_id(&key.connection_id)?;
            if !keys.insert(key.connection_id.as_str()) {
                return Err("run GPIO repeats a control identity".into());
            }
        }
        let mut connections = BTreeSet::new();
        for connection in &self.gpio.connections {
            fujimod_catalog_identity::validate_id(&connection.connection_id)?;
            if !connections.insert(connection.connection_id.as_str()) {
                return Err("run GPIO repeats a connection identity".into());
            }
        }
        let mut outputs = BTreeSet::new();
        for output in &self.gpio.outputs {
            fujimod_catalog_identity::validate_id(&output.connection_id)?;
            if !outputs.insert((output.connection_id.as_str(), output.signal_index)) {
                return Err("run GPIO repeats an output signal identity".into());
            }
        }
        if self.encoders.len() > 64 {
            return Err("run encoder input count exceeds its bound".into());
        }
        let mut encoders = BTreeSet::new();
        for encoder in &self.encoders {
            fujimod_catalog_identity::validate_id(&encoder.connection_id)?;
            if encoder.phase > 3 || !encoders.insert(encoder.connection_id.as_str()) {
                return Err("run encoder input has a repeated identity or invalid phase".into());
            }
        }
        fujimod_catalog_identity::validate_id(&self.adc.block_id)?;
        if self.adc.samples.is_empty() || self.adc.samples.len() > 64 {
            return Err("run ADC sample count is outside its bound".into());
        }
        fujimod_catalog_identity::validate_id(&self.power.part_id)?;
        if self.power.spi_status.len() > 128 {
            return Err("run power status input count exceeds its bound".into());
        }
        validate_register_values(&self.power.spi_status)?;
        fujimod_catalog_identity::validate_id(&self.mcu.part_id)?;
        if self.mcu.packet[14] != 0xaa
            || self
                .mcu
                .packet
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
                != 0
        {
            return Err("run MCU packet has an invalid marker or checksum".into());
        }
        self.mcu.rtc.validate()?;
        if self.serial_registers.len() > 16 {
            return Err("run serial part count exceeds its bound".into());
        }
        let mut serial_parts = BTreeSet::new();
        for registers in &self.serial_registers {
            fujimod_catalog_identity::validate_id(&registers.part_id)?;
            if !serial_parts.insert(registers.part_id.as_str()) || registers.values.len() > 128 {
                return Err("run serial register target is repeated or too large".into());
            }
            validate_register_values(&registers.values)?;
        }
        fujimod_catalog_identity::validate_id(&self.usb.connection_id)?;
        fujimod_catalog_identity::validate_id(&self.card.connection_id)?;
        if !self.card.present && self.card.write_protected {
            return Err("an absent run card cannot be write protected".into());
        }
        if let Some(settings) = &self.settings {
            settings.validate()?;
        }
        if let Some(first_start) = &self.first_start {
            first_start.validate()?;
        }
        Ok(())
    }
}

/// Policy for navigating and accepting stock first-start setup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunFirstStart {
    /// Optional exact firmware language name to select from START MENU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Firmware logical key identity, such as `EXE_M`.
    pub key_id: String,
    /// Virtual ticks to hold the input active.
    pub hold_ticks: u64,
    /// Virtual ticks to wait after releasing the input.
    pub settle_ticks: u64,
    /// Maximum confirmation presses accepted for this startup policy.
    pub max_presses: u8,
}

impl RunFirstStart {
    fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.key_id)?;
        if self.hold_ticks == 0 || self.settle_ticks == 0 || self.max_presses == 0 {
            return Err("run first-start timing and press limit must be positive".into());
        }
        Ok(())
    }
}

/// GPIO input block and semantic external states for the base run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunGpioInputs {
    /// SoC GPIO block identity.
    pub block_id: String,
    /// Startup input level supplied for pads without a named assignment.
    /// This is a run assumption, not a measured hardware fact.
    pub unassigned_input_level: bool,
    /// Explicit key/control states; released controls are listed as false.
    pub keys: Vec<RunKeyState>,
    /// External connection states resolved through body wiring.
    pub connections: Vec<RunConnectionState>,
    /// Known GPIO output-latch states addressed through body wiring.
    pub outputs: Vec<RunGpioOutputState>,
}

/// Initial output-latch value for one GPIO signal in a body connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunGpioOutputState {
    /// Body connection identity for the output signal.
    pub connection_id: String,
    /// Zero-based signal index in the body connection.
    pub signal_index: u8,
    /// Output level latched at reset.
    pub high: bool,
}

/// Initial state of a physical key or body control connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunKeyState {
    /// Body connection identity for the control.
    pub connection_id: String,
    /// Whether the control is pressed at reset.
    pub pressed: bool,
}

/// Initial phase for a body-backed quadrature encoder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunEncoderInput {
    /// Body connection identity carrying both encoder signals.
    pub connection_id: String,
    /// Initial two-bit encoder phase in the range zero through three.
    pub phase: u8,
}

/// Initial state of a physical body connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunConnectionState {
    /// Body connection identity.
    pub connection_id: String,
    /// Whether the external connection is present or electrically closed.
    pub connected: bool,
}

/// ADC block and reset sample vector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunAdcInputs {
    /// SoC ADC block identity.
    pub block_id: String,
    /// Per-channel samples at reset.
    pub samples: Vec<u16>,
}

/// Power-controller reset values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunPowerInput {
    /// Body power-controller part identity.
    pub part_id: String,
    /// Whether the camera power input is asserted.
    pub powered_on: bool,
    /// Synthetic boot reply byte.
    pub boot_reply: u8,
    /// Synthetic serial status values presented by the power controller.
    pub spi_status: Vec<RunRegisterValue>,
}

/// MCU packet, serial clock and RTC start state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunMcuInput {
    /// Body MCU-interface part identity.
    pub part_id: String,
    /// Synthetic sixteen-byte persistent settings packet.
    pub packet: [u8; 16],
    /// Synthetic RTC state at reset.
    pub rtc: RunRtcInput,
}

/// Local wall time every run's real-time clock shows at power-on unless the run
/// requests another, in seconds since 2000-01-01: 2026-10-11 02:13:20. The
/// clock runs from power-on, as a camera's battery-backed clock does.
pub const DEFAULT_CLOCK_SECONDS_2000: u32 = 845_000_000;

/// RTC hardware of a run. It starts at the run's requested clock or
/// `DEFAULT_CLOCK_SECONDS_2000`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunRtcInput {
    /// Virtual ticks per RTC second.
    pub ticks_per_second: u64,
    /// Maximum supported seconds in the device clock's epoch.
    pub maximum_seconds: u32,
    /// Starting virtual tick for RTC progression.
    pub started: u64,
}

impl RunRtcInput {
    fn validate(&self) -> Result<(), String> {
        if self.ticks_per_second == 0 || DEFAULT_CLOCK_SECONDS_2000 > self.maximum_seconds {
            return Err("run RTC input has an invalid rate or range".into());
        }
        Ok(())
    }
}

/// Initial serial-register values for one resolved physical part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunSerialRegisters {
    /// Body part identity whose external registers receive these values.
    pub part_id: String,
    /// Register values at reset.
    pub values: Vec<RunRegisterValue>,
}

/// One synthetic external serial register value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunRegisterValue {
    /// External part register number.
    pub register: u8,
    /// Initial register byte.
    pub value: u8,
}

fn validate_register_values(values: &[RunRegisterValue]) -> Result<(), String> {
    let mut registers = BTreeSet::new();
    for value in values {
        if !registers.insert(value.register) {
            return Err("run serial input repeats a register".into());
        }
    }
    Ok(())
}

/// Initial USB cable state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunUsbInput {
    /// Body USB connection identity.
    pub connection_id: String,
    /// Whether a USB cable is present.
    pub connected: bool,
}

/// Initial removable-card state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunCardInput {
    /// Body connection identity for the removable-card slot.
    pub connection_id: String,
    /// Whether a card is inserted.
    pub present: bool,
    /// Whether an inserted card is write protected.
    pub write_protected: bool,
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
pub enum RunOverlayChange {
    /// Select a closed set of startup inputs and device policies.
    Preset { preset: RunPreset },
    /// Connect or disconnect the declared USB cable.
    UsbConnection { connected: bool },
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
    /// Named reset-input document for the selected body hardware profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_inputs_id: Option<String>,
    /// Provenance for caller-supplied hardware inputs, keyed by source field path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hardware_notes: BTreeMap<String, String>,
    /// Named scanout selected by firmware observation without firmware addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observer: Option<RunObserver>,
    /// Startup values and scheduling limits keyed by native device identity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub native_device_inputs: BTreeMap<String, RunNativeDeviceInputs>,
    /// Caller-supplied persistent scene inputs keyed by medium identity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nand_inputs: BTreeMap<String, RunNandSceneInput>,
    /// Card file paths and their external asset identities, grouped by medium.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, BTreeMap<String, String>>,
    /// Named SoC and body implementations selected through the hardware graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_selection: Option<fujimod_catalog_construction::HardwareSelection>,
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
        if let Some(id) = &self.hardware_inputs_id {
            fujimod_catalog_identity::validate_id(id)?;
        }
        for (id, inputs) in &self.native_device_inputs {
            fujimod_catalog_identity::validate_id(id)?;
            inputs.validate()?;
        }
        for (id, medium) in &self.nand_inputs {
            fujimod_catalog_identity::validate_id(id)?;
            medium.validate()?;
        }
        if let Some(observer) = &self.observer {
            fujimod_catalog_identity::validate_id(&observer.scanout)?;
        }
        if let Some(hardware) = &self.hardware_selection {
            fujimod_catalog_identity::validate_id(&hardware.soc_profile_id)?;
            fujimod_catalog_identity::validate_id(&hardware.body_profile_id)?;
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
    Xp4(crate::application_inputs::ApplicationRunScene),
    /// External scene inputs for an XP5-based body.
    Xp5(crate::application_inputs::ApplicationRunScene),
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
    fn runtime_native_inputs_refuse_invalid_reset_values() {
        let mut config = RunConfiguration::bundled("x70-110-runtime").unwrap();
        let preset = config.preset.as_mut().unwrap();
        let Some(RunNativeDeviceInputs::Adc(adc)) = preset.native_device_inputs.get_mut("adc")
        else {
            panic!("ADC input");
        };
        adc.samples[0] = 1024;
        assert!(config.validate("x70-110-runtime").is_err());
        let mut config = RunConfiguration::bundled("x70-110-runtime").unwrap();
        let preset = config.preset.as_mut().unwrap();
        let Some(RunNativeDeviceInputs::ImageInput(input)) =
            preset.native_device_inputs.get_mut("image_input")
        else {
            panic!("image input");
        };
        input.cfa.pop();
        assert!(config.validate("x70-110-runtime").is_err());
    }

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
        let overlay = RunOverlay::bundled("xm1-150-sd-boot-rec").unwrap();
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
    fn bundled_configuration_is_identity_checked_and_has_explicit_reset_inputs() {
        let config = RunConfiguration::bundled("x70-110-rec").unwrap();
        let Some(RunInputs::Exr2(inputs)) = config.inputs.as_ref() else {
            panic!("X70 run inputs must be EXR2 inputs");
        };
        let Some(RunScene::Exr2(scene)) = config.scene.as_ref() else {
            panic!("X70 run scene must be EXR2 scene inputs");
        };
        assert_eq!(config.id, "x70-110-rec");
        assert_eq!(config.body_id, "x70");
        assert_eq!(config.firmware_id, "x70-110");
        assert_eq!(config.purpose, RunPurpose::RomBoot);
        assert_eq!(config.start, RunStart::Rom);
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
        assert!(!inputs.gpio.unassigned_input_level);
        assert_eq!(
            inputs
                .gpio
                .keys
                .iter()
                .filter(|key| key.pressed)
                .map(|key| key.connection_id.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from(["controls-gpio-2-11", "controls-gpio-1-1"])
        );
        assert_eq!(inputs.gpio.outputs.len(), 3);
        assert!(inputs.gpio.outputs.iter().all(|output| output.high));
        assert_eq!(inputs.encoders.len(), 1);
        assert_eq!(inputs.encoders[0].connection_id, "controls-encoder-3-6-3-7");
        assert_eq!(inputs.encoders[0].phase, 0);
        assert_eq!(inputs.adc.block_id, "adc");
        assert!(config.overlays.is_empty());
        assert!(RunConfiguration::bundled("unknown").is_err());
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
    fn statistics_offsets_are_destination_offsets() {
        let mut config = RunConfiguration::bundled("x70-110-rec").unwrap();
        let Some(RunScene::Exr2(scene)) = config.scene.as_mut() else {
            panic!("X70 run scene must be EXR2 scene inputs");
        };
        scene.statistics.live_statistics[0].offset = u32::MAX;
        assert!(config.validate("x70-110-rec").is_ok());
    }
}
