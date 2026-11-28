//! Reset inputs, startup entries and bus facts shared by bodies, firmware and runs.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Run-specific software-visible DRAM mapping extent and its evidence basis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunDramAddressSpace {
    /// Exclusive mapped DRAM address-space extent in bytes.
    pub limit_bytes: u64,
    /// Source and limits for this modeled mapping extent.
    pub basis: String,
}

impl RunDramAddressSpace {
    /// Check the extent is page aligned and has a basis.
    pub fn validate(&self) -> Result<(), String> {
        if self.limit_bytes == 0
            || !self.limit_bytes.is_multiple_of(4096)
            || self.basis.trim().is_empty()
        {
            return Err("DRAM address-space limit must be page-aligned with a basis".into());
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
    /// Enter the firmware's boot-setup stage from internal flash, in place of the boot ROM.
    Setup,
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
// Catalog records are few and long-lived, so the variant size difference costs nothing.
#[allow(clippy::large_enum_variant)]
pub enum RunInputs {
    /// Reset inputs for an EXR2-based body.
    Exr2(Exr2RunInputs),
    /// Reset inputs for an XP4-based body.
    Xp4(crate::application::ApplicationRunInputs),
    /// Reset inputs for an XP5-based body.
    Xp5(crate::application::ApplicationRunInputs),
}

impl RunInputs {
    pub fn validate(&self) -> Result<(), String> {
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

/// The native serial registers that carry an MCU packet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunMcuPacket {
    /// Native node owning the serial endpoint.
    pub node: String,
    /// Route index of the registers endpoint.
    pub route: u64,
    /// First register the packet's bytes occupy.
    pub first_register: u64,
}

impl RunPowerOnSettings {
    pub fn validate(&self) -> Result<(), String> {
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
    pub fn validate(&self) -> Result<(), String> {
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
    pub fn validate(&self) -> Result<(), String> {
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
    pub fn validate(&self) -> Result<(), String> {
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

/// Board ticks one control-bus block takes to move a byte.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunBusByteTime {
    /// Control-bus block the time applies to.
    pub block_id: String,
    /// Board ticks per byte.
    pub ticks: u64,
}

/// The clock-divider ladder a stock SD driver programs on one host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunSdClockSteps {
    /// SD host block.
    pub block_id: String,
    /// Interface clock configuration under which the steps hold.
    pub interface: u8,
    /// Divider for a default-speed card.
    pub default_speed: u16,
    /// Divider for a high-speed card.
    pub high_speed: u16,
}
