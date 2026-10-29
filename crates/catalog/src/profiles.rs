//! Declared hardware profiles selected independently from run inputs.

use crate::parts::Pin;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An explicitly empty device-reference set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareEmpty"))]
pub struct Empty {}

/// Declared attachments facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwarePartAssemblies"))]
pub struct PartAssemblies {
    /// Declared contact actuators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_actuators: Option<Vec<String>>,
    /// Declared serial outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_outputs: Option<Vec<String>>,
    /// Declared serial part.
    pub serial_part: Vec<String>,
}

/// Declared card contacts facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareCardContacts"))]
pub struct CardContacts {
    /// Declared basis.
    pub basis: String,
    /// Declared detect.
    pub detect: Vec<u64>,
    /// Declared write protect.
    pub write_protect: Vec<u64>,
}

/// Declared signals facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareConnectionSignal"))]
pub struct ConnectionSignal {
    /// Declared active level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_level: Option<u64>,
    /// Declared channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u64>,
    /// Declared label.
    pub label: String,
    /// Declared pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<u64>,
    /// Declared port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u64>,
    /// Declared source.
    pub source: String,
    /// Declared threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u64>,
}

/// Declared connections facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareConnectionSupport"))]
pub struct ConnectionSupport {
    /// Declared id.
    pub id: String,
    /// Declared label.
    pub label: String,
    /// Declared limits.
    pub limits: String,
    /// Declared signals.
    pub signals: Vec<ConnectionSignal>,
    /// Declared support.
    pub support: String,
}

/// Declared command targets facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareCommandTarget"))]
pub struct CommandTarget {
    /// Declared address.
    pub address: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared commands.
    pub commands: Vec<u64>,
    /// Declared prefix mask.
    pub prefix_mask: u64,
    /// Declared prefix value.
    pub prefix_value: u64,
    /// Declared read frames.
    pub read_frames: Vec<(u64, String)>,
    /// Declared register mask.
    pub register_mask: u64,
    /// Declared write masks.
    pub write_masks: Vec<Vec<u64>>,
}

/// Declared sensor facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareBusSensor"))]
pub struct BusSensor {
    /// Declared address.
    pub address: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared reply model.
    pub reply_model: String,
}

/// Declared status targets facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareStatusTarget"))]
pub struct StatusTarget {
    /// Declared address.
    pub address: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared status hex.
    pub status_hex: String,
    /// Declared writable registers.
    pub writable_registers: Vec<u64>,
    /// Declared write replies.
    pub write_replies: Vec<(u64, u64, String)>,
}

/// Declared control bus0 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwarePrimaryControlBus"))]
pub struct PrimaryControlBus {
    /// Declared basis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
    /// Declared command targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_targets: Option<Vec<CommandTarget>>,
    /// Declared profile.
    pub profile: String,
    /// Declared sensor.
    pub sensor: BusSensor,
    /// Declared status targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_targets: Option<Vec<StatusTarget>>,
    /// Declared transfer ticks.
    pub transfer_ticks: u64,
}

/// Declared control bus1 replies facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareSecondaryBusRegisters"))]
pub struct SecondaryBusRegisters {
    /// Declared pwic address.
    pub pwic_address: u64,
    /// Declared pwic modes.
    pub pwic_modes: Vec<u64>,
    /// Declared pwic read prefixes.
    pub pwic_read_prefixes: Vec<(u64, String)>,
    /// Declared pwic values.
    pub pwic_values: Vec<Vec<u64>>,
    /// Declared pwic write masks.
    pub pwic_write_masks: Vec<Vec<u64>>,
    /// Declared register address.
    pub register_address: u64,
    /// Declared register modes.
    pub register_modes: Vec<u64>,
    /// Declared register values.
    pub register_values: Vec<Vec<u64>>,
    /// Declared register write masks.
    pub register_write_masks: Vec<Vec<u64>>,
    /// Declared usb connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_connected: Option<String>,
}

/// Declared control bus1 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareSecondaryControlBus"))]
pub struct SecondaryControlBus {
    /// Declared basis.
    pub basis: String,
    /// Declared profile.
    pub profile: String,
    /// Declared replies.
    pub replies: SecondaryBusRegisters,
    /// Declared transfer ticks.
    pub transfer_ticks: u64,
}

/// Declared display timing facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDisplayTiming"))]
pub struct DisplayTiming {
    /// Declared basis.
    pub basis: String,
    /// Declared frame period ticks.
    pub frame_period_ticks: u64,
    /// Declared phase profile.
    pub phase_profile: String,
    /// Declared stop delay ticks.
    pub stop_delay_ticks: u64,
}

/// Declared fe80 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFe80"))]
pub struct Fe80 {
    /// Declared basis.
    pub basis: String,
    /// Declared periods.
    pub periods: Vec<u64>,
    /// Declared upload setup phases.
    pub upload_setup_phases: Vec<String>,
}

/// Declared input processing facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareInputProcessing"))]
pub struct InputProcessing {
    /// Declared basis.
    pub basis: String,
    /// Declared command controls.
    pub command_controls: Vec<u64>,
}

/// Declared spi1 serial facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareSpi1Serial"))]
pub struct Spi1Serial {
    /// Declared basis.
    pub basis: String,
    /// Declared selection.
    pub selection: Vec<u64>,
    /// Declared transfer ticks.
    pub transfer_ticks: u64,
    /// Declared transports.
    pub transports: Vec<Vec<u64>>,
}

/// Declared stabilizer facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareStabilizer"))]
pub struct Stabilizer {
    /// Declared basis.
    pub basis: String,
    /// Declared status.
    pub status: u64,
    /// Declared telemetry.
    pub telemetry: u64,
}

/// Declared usb facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareUsb"))]
pub struct Usb {
    /// Declared basis.
    pub basis: String,
    /// Declared in endpoints.
    pub in_endpoints: Vec<Vec<u64>>,
    /// Declared out endpoints.
    pub out_endpoints: Vec<Vec<u64>>,
    /// Declared packet bytes.
    pub packet_bytes: u64,
    /// Declared transfer ticks.
    pub transfer_ticks: u64,
}

/// Declared window analysis facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareWindowAnalysis"))]
pub struct WindowAnalysis {
    /// Declared basis.
    pub basis: String,
    /// Declared profile.
    pub profile: String,
    /// Declared ticks per window.
    pub ticks_per_window: u64,
}

/// Declared controllers facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareControllerModels"))]
pub struct ControllerModels {
    /// Declared control bus0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_bus0: Option<PrimaryControlBus>,
    /// Declared control bus1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_bus1: Option<SecondaryControlBus>,
    /// Declared display timing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_timing: Option<DisplayTiming>,
    /// Declared fe80.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fe80: Option<Fe80>,
    /// Declared input processing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_processing: Option<InputProcessing>,
    /// Declared spi1 serial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spi1_serial: Option<Spi1Serial>,
    /// Declared stabilizer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stabilizer: Option<Stabilizer>,
    /// Declared usb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb: Option<Usb>,
    /// Declared window analysis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_analysis: Option<WindowAnalysis>,
}

/// Declared encoders options facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareEncoderTiming"))]
pub struct EncoderTiming {
    /// Declared phase ticks.
    pub phase_ticks: u64,
    /// Declared pins.
    pub pins: Vec<Pin>,
    /// Declared rest phase.
    pub rest_phase: u64,
}

/// Declared encoders facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareEncoder"))]
pub struct Encoder {
    /// Declared counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter: Option<String>,
    /// Declared name.
    pub name: String,
    /// Declared options.
    pub options: EncoderTiming,
}

/// Declared dependencies facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDeviceDependency"))]
pub struct DeviceDependency {
    /// Declared kind.
    pub kind: DeviceKind,
    /// Declared name.
    pub name: String,
    /// Declared refs.
    pub refs: BTreeMap<String, String>,
}

/// Declared models facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDeviceModel"))]
pub struct DeviceModel {
    /// Nodes absorbed by this composed hardware model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaces: Vec<String>,
    /// Declared dependencies.
    pub dependencies: Vec<DeviceDependency>,
    /// Declared kind.
    pub kind: DeviceKind,
    /// Declared refs.
    pub refs: BTreeMap<String, String>,
}

/// Declared model choices facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDeviceChoices"))]
pub struct DeviceChoices {
    /// Declared models.
    pub models: BTreeMap<String, DeviceModel>,
    /// Declared option.
    pub option: String,
}

/// Declared devices facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeviceNode {
    /// Declared boot entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_entries: Option<Vec<String>>,
    /// Declared kind.
    pub kind: DeviceKind,
    /// Declared model choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_choices: Option<DeviceChoices>,
    /// Declared name.
    pub name: String,
    /// Declared refs.
    pub refs: BTreeMap<String, String>,
}

/// Declared graph facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeviceGraph {
    /// Declared attachment order.
    pub attachment_order: Vec<String>,
    /// Declared construction order.
    pub construction_order: Vec<String>,
    /// Declared devices.
    pub devices: Vec<DeviceNode>,
    /// Declared optional devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional_devices: Option<Vec<DeviceKind>>,
}

/// Declared af extraction facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareAfExtraction"))]
pub struct AfExtraction {
    /// Declared profile.
    pub profile: String,
}

/// Declared projection facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareProjection"))]
pub struct Projection {
    /// Declared focal length mm.
    pub focal_length_mm: f64,
    /// Declared focus.
    pub focus: String,
    /// Declared relative illumination.
    pub relative_illumination: crate::reference::SensorIllumination,
    /// Declared sensor width mm.
    pub sensor_width_mm: f64,
    /// Declared subject distance mm.
    pub subject_distance_mm: f64,
}

/// Declared rolling references facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareRollingReference"))]
pub struct RollingReference {
    /// Declared horizontal.
    pub horizontal: u64,
    /// Declared lines.
    pub lines: u64,
    /// Declared mode.
    pub mode: u64,
    /// Declared vertical.
    pub vertical: u64,
}

/// Declared shutter facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareShutter"))]
pub struct Shutter {
    /// Declared closing delay seconds.
    pub closing_delay_seconds: Vec<Vec<f64>>,
}

/// Declared radiometry facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareRadiometry"))]
pub struct Radiometry {
    /// Declared pixel clock hz.
    pub pixel_clock_hz: u64,
    /// Declared projection.
    pub projection: Projection,
    /// Declared rolling references.
    pub rolling_references: Vec<RollingReference>,
    /// Invalid readouts while a sensor drive-mode change acquires sync.
    pub mode_change_frames: u8,
    /// Declared saturation lux seconds.
    pub saturation_lux_seconds: f64,
    /// Declared shutter.
    pub shutter: Shutter,
    /// Declared shutter output.
    pub shutter_output: u64,
    /// Declared ticks per second.
    pub ticks_per_second: u64,
    /// Declared white luminance.
    pub white_luminance: f64,
}

/// Declared capture color facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareCaptureColor"))]
pub struct CaptureColor {
    /// Declared basis.
    pub basis: String,
    /// Declared black.
    pub black: u64,
    /// Declared full scale.
    pub full_scale: u64,
    /// Declared initial rgb.
    pub initial_rgb: Vec<u64>,
    /// Declared processing cfa.
    pub processing_cfa: Vec<u64>,
    /// Declared radiometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radiometry: Option<Radiometry>,
    /// Declared reference white.
    pub reference_white: Vec<u64>,
    /// Declared sensor cfa.
    pub sensor_cfa: Vec<u64>,
    /// Declared sensor image area.
    pub sensor_image_area: Vec<u64>,
}

/// Declared optics facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareOptics"))]
pub struct Optics {
    /// Declared maximum radius.
    pub maximum_radius: u64,
    /// Declared pulses per radius.
    pub pulses_per_radius: u64,
    /// Declared target position.
    pub target_position: u64,
}

/// Declared fe80 processing options facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFe80ProcessingOptions"))]
pub struct Fe80ProcessingOptions {
    /// Declared irq.
    pub irq: u64,
    /// Declared pixels per tick.
    pub pixels_per_tick: u64,
}

/// Declared fe80 processing facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFe80Processing"))]
pub struct Fe80Processing {
    /// Declared arithmetic.
    pub arithmetic: String,
    /// Declared basis.
    pub basis: String,
    /// Declared memory bytes per tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes_per_tick: Option<u64>,
    /// Declared optics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optics: Option<Optics>,
    /// Declared options.
    pub options: Fe80ProcessingOptions,
}

/// Declared focus axis facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFocusAxis"))]
pub struct FocusAxis {
    /// Declared infinity position.
    pub infinity_position: i64,
    /// Declared output.
    pub output: String,
    /// Declared steps per mm.
    pub steps_per_mm: f64,
}

/// Declared contrast facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareContrast"))]
pub struct Contrast {
    /// Declared kind.
    pub kind: String,
    /// Declared luma bits.
    pub luma_bits: u64,
}

/// Declared face detector facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFaceDetector"))]
pub struct FaceDetector {
    /// Declared mode.
    pub mode: String,
}

/// Declared focus imaging optics facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFocusImagingOptics"))]
pub struct FocusImagingOptics {
    /// Declared maximum radius.
    pub maximum_radius: u64,
    /// Declared pulses per radius.
    pub pulses_per_radius: u64,
    /// Declared target position.
    pub target_position: i64,
}

/// Declared raster facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareRaster"))]
pub struct Raster {
    /// Declared height.
    pub height: u64,
    /// Declared stride.
    pub stride: u64,
    /// Declared width.
    pub width: u64,
}

/// Declared focus imaging facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFocusImaging"))]
pub struct FocusImaging {
    /// Declared basis.
    pub basis: String,
    /// Declared contrast.
    pub contrast: Contrast,
    /// Declared face detector.
    pub face_detector: FaceDetector,
    /// Declared frame phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_phase: Option<u64>,
    /// Declared input clock mode.
    pub input_clock_mode: String,
    /// Declared input command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_command: Option<String>,
    /// Declared input period ticks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_period_ticks: Option<u64>,
    /// Declared memory bytes per tick.
    pub memory_bytes_per_tick: u64,
    /// Declared metering.
    pub metering: String,
    /// Declared optics.
    pub optics: FocusImagingOptics,
    /// Declared raster.
    pub raster: Raster,
    /// Declared statistics pixels per tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statistics_pixels_per_tick: Option<u64>,
}

/// Declared iris mechanics facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareIrisMechanics"))]
pub struct IrisMechanics {
    /// Declared boundary.
    pub boundary: String,
    /// Declared travel limits.
    pub travel_limits: Vec<u64>,
}

/// Declared iris optics facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareIrisOptics"))]
pub struct IrisOptics {
    /// Declared output.
    pub output: String,
    /// Declared positions.
    pub positions: Vec<Vec<f64>>,
}

/// Declared sensor capture facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareSensorCapture"))]
pub struct SensorCapture {
    /// Declared cfa origin.
    pub cfa_origin: Vec<u64>,
    /// Declared dma ticks.
    pub dma_ticks: u64,
    /// Declared height.
    pub height: u64,
    /// Declared period ticks.
    pub period_ticks: u64,
    /// Declared width.
    pub width: u64,
}

/// Declared imaging facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareImaging"))]
pub struct Imaging {
    /// Declared af extraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub af_extraction: Option<AfExtraction>,
    /// Declared capture color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_color: Option<CaptureColor>,
    /// Declared fe80 processing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fe80_processing: Option<Fe80Processing>,
    /// Declared focus axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_axis: Option<FocusAxis>,
    /// Declared focus imaging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_imaging: Option<FocusImaging>,
    /// Declared iris mechanics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iris_mechanics: Option<IrisMechanics>,
    /// Declared iris optics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iris_optics: Option<IrisOptics>,
    /// Declared sensor capture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_capture: Option<SensorCapture>,
}

/// Declared core facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareCore"))]
pub struct Core {
    /// Declared core.
    pub core: u64,
    /// Declared line.
    pub line: u64,
}

/// Declared outputs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareInterruptOutput"))]
pub struct InterruptOutput {
    #[serde(rename = "Core")]
    /// Declared Core.
    pub core: Core,
}

/// Declared pin interrupts facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwarePinInterrupt"))]
pub struct PinInterrupt {
    /// Declared edge.
    pub edge: String,
    /// Declared outputs.
    pub outputs: Vec<InterruptOutput>,
    /// Declared pin.
    pub pin: Pin,
}

/// Declared preview activity facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwarePreviewActivity"))]
pub struct PreviewActivity {
    /// Declared high.
    pub high: bool,
    /// Declared pin.
    pub pin: Pin,
}

/// Declared touch facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareTouch"))]
pub struct Touch {
    /// Declared core.
    pub core: u64,
    /// Declared irq.
    pub irq: u64,
    /// Declared register.
    pub register: u64,
    /// Declared ticks per second.
    pub ticks_per_second: u64,
}

/// Declared body profile facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BodyProfile {
    /// Firmware descriptor order of body devices; `sensor` refers to the installed sensor endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_bus_order: Option<Vec<String>>,
    /// Exported construction topology, including native node identities and hardware parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub construction: Option<fujimod_catalog_construction::ConstructionProfile>,
    /// Declared attachments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<PartAssemblies>,
    /// Declared card contacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_contacts: Option<CardContacts>,
    /// Declared connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connections: Option<Vec<ConnectionSupport>>,
    /// Declared control bus notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_bus_notes: Option<String>,
    /// Declared controllers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controllers: Option<ControllerModels>,
    /// Declared counter feedback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter_feedback: Option<String>,
    /// Declared dram basis.
    pub dram_basis: String,
    /// Declared encoders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoders: Option<Vec<Encoder>>,
    /// Declared face register mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_register_mode: Option<String>,
    /// Declared graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<DeviceGraph>,
    /// Declared imaging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imaging: Option<Imaging>,
    /// Declared interrupt notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt_notes: Option<String>,
    /// Declared lens notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_notes: Option<String>,
    /// Declared limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Vec<String>>,
    /// Declared panel notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_notes: Option<String>,
    /// Declared pin interrupts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_interrupts: Option<Vec<PinInterrupt>>,
    /// Declared preview activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_activity: Option<PreviewActivity>,
    /// Declared touch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touch: Option<Touch>,
}

/// Declared boot rom facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareBootRom"))]
pub struct BootRom {
    /// Declared basis.
    pub basis: String,
    /// Declared sha256.
    pub sha256: String,
    /// Declared size.
    pub size: u64,
}

/// Declared discovery mmio facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDiscoveryWindow"))]
pub struct DiscoveryWindow {
    /// Declared address.
    pub address: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared size.
    pub size: u64,
}

/// Declared instructions per tick facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareInstructionRate"))]
pub struct InstructionRate {
    /// Declared denominator.
    pub denominator: u64,
    /// Declared numerator.
    pub numerator: u64,
}

/// Declared private mmio facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareCoreWindow"))]
pub struct CoreWindow {
    /// Declared address.
    pub address: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared adc facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareAdc"))]
pub struct Adc {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared sample ticks.
    pub sample_ticks: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared register maps control bus0 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwarePrimaryBusRegisters"))]
pub struct PrimaryBusRegisters {
    /// Declared base.
    pub base: u64,
    /// Declared irq.
    pub irq: u64,
    /// Declared modes.
    pub modes: Vec<u64>,
    /// Declared offsets.
    pub offsets: Vec<u64>,
}

/// Declared register maps control bus1 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    feature = "schema",
    schemars(rename = "HardwareSecondaryBusRegistersAddress")
)]
pub struct SecondaryBusRegistersAddress {
    /// Declared base.
    pub base: u64,
    /// Declared irq.
    pub irq: u64,
}

/// Declared control requests facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareControlRequests"))]
pub struct ControlRequests {
    /// Declared ack ticks.
    pub ack_ticks: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared registers.
    pub registers: Vec<u64>,
}

/// Declared decompressor facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDecompressor"))]
pub struct Decompressor {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
    /// Declared irq route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub irq_route: Option<u64>,
}

/// Declared dma facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDma"))]
pub struct Dma {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared channels.
    pub channels: u64,
    /// Declared irq routes.
    pub irq_routes: Vec<u64>,
    /// Declared native dependencies.
    pub native_dependencies: Vec<String>,
    /// Declared request mux.
    pub request_mux: u64,
    /// Declared timing.
    pub timing: String,
}

/// Display frame interrupts declared as a law without controller registers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDisplayFrames"))]
pub struct DisplayFrames {
    /// Declared basis.
    pub basis: String,
    /// Lines receiving one edge per frame.
    pub irq_routes: Vec<u64>,
    /// Frame period in guest ticks.
    pub period_ticks: u64,
}

/// Status bits of one display output register.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDisplayStatusBits"))]
pub struct DisplayStatusBits {
    /// Word offset from the output base.
    pub offset: u64,
    /// Bits that read with the declared level.
    pub mask: u64,
}

/// Register window of one display output with status bits that read fixed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDisplayOutput"))]
pub struct DisplayOutput {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Bits a completed request leaves clear.
    pub clear_bits: Vec<DisplayStatusBits>,
    /// Bits that report a completed request as set.
    pub set_bits: Vec<DisplayStatusBits>,
    /// Declared size.
    pub size: u64,
    /// OSD layer capture, when the output is scanned out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanout: Option<DisplayScanout>,
}

/// Host output index and capture period of a display output's OSD layers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDisplayScanout"))]
pub struct DisplayScanout {
    /// Host output index of the captured frames.
    pub output: u64,
    /// Capture period in ticks.
    pub period_ticks: u64,
}

/// Declared dmp facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDmp"))]
pub struct Dmp {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared irq route.
    pub irq_route: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared gic facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareGic"))]
pub struct Gic {
    /// Declared basis.
    pub basis: String,
    /// Declared cpu interface.
    pub cpu_interface: u64,
    /// Declared distributor.
    pub distributor: u64,
    /// Declared lines.
    pub lines: u64,
}

/// Declared gpio facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareGpio"))]
pub struct Gpio {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared size.
    pub size: u64,
}

/// X-Processor Pro clock bank: dividers, gates and five PLLs with lock status.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwarePllClocks"))]
pub struct PllClocks {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Ticks from a PLL enable to its lock bit.
    pub lock_ticks: u64,
    /// Declared size.
    pub size: u64,
}

/// X-Processor Pro SD channel: wrapper reset at +0, vendor words and a
/// relocated SDHCI bank, with its interrupt and hardware reset line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDramController"))]
pub struct DramController {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared size.
    pub size: u64,
}

/// X-Processor Pro read-only DRAM alias: physical `base` onwards returns body
/// DRAM from address 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDramAlias"))]
pub struct DramAlias {
    /// Declared physical base of the alias.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
}

/// X-Processor Pro DesignWare USB3 device controller.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareXppUsb"))]
pub struct XppUsb {
    /// Declared base; the core registers start at +0xc100.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Time an endpoint command takes, in device ticks.
    pub command_ticks: u64,
    /// Interrupt line of the event buffer.
    pub irq_route: u32,
    /// Declared size.
    pub size: u64,
    /// Time a bus transaction takes, in device ticks.
    pub transaction_ticks: u64,
}

/// X-Processor Pro power control: supply hold and power domains.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwarePowerControl"))]
pub struct PowerControl {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared size.
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareXppI2cBuses"))]
pub struct XppI2cBuses {
    /// Base of bus 0; bus n follows at `base + n * stride`.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// The `ic_clk` the SCL high and low counts divide.
    pub clock_hz: u64,
    /// TX and RX FIFO depth in entries.
    pub fifo_depth: u64,
    /// Interrupt line of each bus, in bus order.
    pub irq_routes: Vec<u32>,
    /// Declared size of one bus window.
    pub size: u64,
    /// Distance between bus windows.
    pub stride: u64,
    /// Device ticks per second.
    pub tick_hz: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareXppSdHost"))]
pub struct XppSdHost {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Ticks for the internal clock to report stable.
    pub clock_ticks: u64,
    /// Interrupt line of the SDHCI status.
    pub irq_route: u32,
    /// Offset of the SDHCI bank inside the window.
    pub register_offset: u64,
    /// SD channel of the reset-gate line that resets this host.
    pub reset_channel: u64,
    /// Ticks of the wrapper reset at +0.
    pub reset_ticks: u64,
    /// Declared size.
    pub size: u64,
    /// Ticks of one command or data transfer.
    pub transfer_ticks: u64,
}

/// X-Processor Pro reset and gate controls; SD channel reset lines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareResetGates"))]
pub struct ResetGates {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared size.
    pub size: u64,
}

/// Status bits of a register block that always read set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareRegisterIdleBits"))]
pub struct RegisterIdleBits {
    /// Bits that read set.
    pub mask: u32,
    /// Word offset in the block.
    pub offset: u64,
}

/// X-Processor Pro register block with no work in flight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareRegisterBlock"))]
pub struct RegisterBlock {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Status bits that always read set.
    pub idle_bits: Vec<RegisterIdleBits>,
    /// Frames the block finishes while started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<RegisterBlockFrames>,
    /// Declared size.
    pub size: u64,
}

/// Frame law of a register block: one interrupt edge per frame period while
/// the start bit is set; the stopped bit reads set only while it is clear.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareRegisterBlockFrames"))]
pub struct RegisterBlockFrames {
    /// Interrupt line of the frame edge.
    pub irq_route: u32,
    /// Frame period in device ticks, the frame sync's.
    pub period_ticks: u64,
    /// Start bit.
    pub start: RegisterIdleBits,
    /// Bit that reads set while stopped.
    pub stopped: RegisterIdleBits,
}

/// X-Processor Pro imaging statistics engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareImagingStatistics"))]
pub struct ImagingStatistics {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Interrupt line of the completion status.
    pub irq_route: u32,
    /// Frame period in device ticks, the frame sync's.
    pub period_ticks: u64,
    /// Declared size.
    pub size: u64,
}

/// X-Processor Pro face detection engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFaceDetection"))]
pub struct FaceDetection {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Interrupt line of the completion status.
    pub irq_route: u32,
    /// Time from a start to the completion of its run, in device ticks.
    pub run_ticks: u64,
    /// Declared size.
    pub size: u64,
}

/// DesignWare HDMI TX controller with no sink attached.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareHdmiTransmitter"))]
pub struct HdmiTransmitter {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared size.
    pub size: u64,
}

/// One frame interrupt source of the X-Processor Pro frame sync.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFrameSource"))]
pub struct FrameSource {
    /// Interrupt line the source drives.
    pub irq_route: u32,
    /// Status and enable bit of the source: 0, 8 or 16.
    pub status_bit: u32,
}

/// X-Processor Pro imaging frame sync and its frame interrupt words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFrameSync"))]
pub struct FrameSync {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Address of the frame interrupt enable word; the status word follows.
    pub interrupts: u64,
    /// Frame period in device ticks.
    pub period_ticks: u64,
    /// Declared size.
    pub size: u64,
    /// Sources every frame latches.
    pub sources: Vec<FrameSource>,
}

/// X-Processor Pro imaging control page: block stop and register-bus channels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareImagingControl"))]
pub struct ImagingControl {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared size.
    pub size: u64,
}

/// Declared input buffer facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareInputBuffer"))]
pub struct InputBuffer {
    /// Declared bank size.
    pub bank_size: u64,
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared irq.
    pub irq: u64,
    /// Declared reset ticks.
    pub reset_ticks: u64,
    /// Declared sram base.
    pub sram_base: u64,
}

/// Declared register maps imaging facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareImagingRegisters"))]
pub struct ImagingRegisters {
    /// Declared basis.
    pub basis: String,
    /// Declared command base.
    pub command_base: u64,
    /// Declared command ticks.
    pub command_ticks: u64,
    /// Declared error irqs.
    pub error_irqs: Vec<u64>,
    /// Declared input buffer.
    pub input_buffer: InputBuffer,
    /// Declared normal irqs.
    pub normal_irqs: Vec<u64>,
    /// Declared receiver base.
    pub receiver_base: u64,
    /// Declared reset ticks.
    pub reset_ticks: u64,
    /// Declared timing irqs.
    pub timing_irqs: Vec<u64>,
}

/// Declared interrupts facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareInterrupts"))]
pub struct Interrupts {
    /// Declared bases.
    pub bases: Vec<u64>,
    /// Declared timer base.
    pub timer_base: u64,
}

/// Declared routes facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareTimerRoute"))]
pub struct TimerRoute {
    /// Declared channel.
    pub channel: u64,
    /// Declared irq.
    pub irq: u64,
}

/// Declared interval timer facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareIntervalTimer"))]
pub struct IntervalTimer {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared clock.
    pub clock: String,
    /// Register layout, which selects the timer implementation.
    pub layout: TimerLayout,
    /// Declared routes.
    pub routes: Vec<TimerRoute>,
}

/// Interval-timer register layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareTimerLayout"))]
pub enum TimerLayout {
    /// Eight channels at +0x10 + n*8; control select bits set.
    Xp4,
    /// Sixteen channels at +0x100 + n*0x10; control select bits clear.
    Xpp,
}

/// Declared memory-mapped flash controller facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareFlashController"))]
pub struct FlashController {
    /// Controller register base.
    pub base: u64,
    /// Read aperture base.
    pub aperture: u64,
    /// Read aperture size in bytes.
    pub size: u64,
    /// Interrupt line of the controller status, when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub irq_route: Option<u64>,
    /// Declared basis.
    pub basis: String,
}

/// Declared PL330 DMA controller facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwarePl330"))]
pub struct Pl330 {
    /// Register base.
    pub base: u64,
    /// Interrupt line for each event, by event number.
    pub irq_routes: Vec<u64>,
    /// Guest ticks before a program's first byte moves.
    pub setup_ticks: u64,
    /// Bytes moved per guest tick.
    pub bytes_per_tick: u64,
    /// Declared timing qualification.
    pub timing: String,
    /// Declared basis.
    pub basis: String,
}

/// Declared serial port facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareSerialPorts"))]
pub struct SerialPorts {
    /// Register base of each port.
    pub bases: Vec<u64>,
    /// Declared basis.
    pub basis: String,
}

/// Declared io groups facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareIoGroups"))]
pub struct IoGroups {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
}

/// Declared lcdc facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareLcdc"))]
pub struct Lcdc {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared irq routes.
    pub irq_routes: Vec<u64>,
    /// Declared picture layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture_layout: Option<String>,
    /// Declared pixel clock ticks.
    pub pixel_clock_ticks: u64,
    /// Declared video layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_layout: Option<String>,
}

/// Declared loader dma facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareLoaderDma"))]
pub struct LoaderDma {
    /// Declared base.
    pub base: u64,
}

/// Declared movdc facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareMovdc"))]
pub struct Movdc {
    /// Declared base.
    pub base: u64,
    /// Declared irq.
    pub irq: u64,
    /// Declared reset ticks.
    pub reset_ticks: u64,
}

/// Declared sd host clocks facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareSdHostClocks"))]
pub struct SdHostClocks {
    /// Declared basis.
    pub basis: String,
    /// Declared settle ticks.
    pub settle_ticks: u64,
    /// Declared words.
    pub words: Vec<u64>,
}

/// Declared serial flash facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareSerialFlash"))]
pub struct SerialFlash {
    /// Declared aperture.
    pub aperture: u64,
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared irq route.
    pub irq_route: u64,
    /// Declared native dependencies.
    pub native_dependencies: Vec<String>,
    /// Declared size.
    pub size: u64,
    /// Declared timing.
    pub timing: String,
}

/// Declared spi facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareSpi"))]
pub struct Spi {
    /// Declared base.
    pub base: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared parent clock ticks.
    pub parent_clock_ticks: u64,
}

/// Declared register maps usb facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareUsbRegisters"))]
pub struct UsbRegisters {
    /// Declared basis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
    /// Declared core.
    pub core: u64,
    /// Declared dma irq.
    pub dma_irq: u64,
    /// Declared irq.
    pub irq: u64,
    /// Declared phy.
    pub phy: u64,
}

/// Declared register maps facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareRegisterLayouts"))]
pub struct RegisterLayouts {
    /// Declared adc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adc: Option<Adc>,
    /// Declared control bus0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_bus0: Option<PrimaryBusRegisters>,
    /// Declared control bus1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_bus1: Option<SecondaryBusRegistersAddress>,
    /// Declared control requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_requests: Option<ControlRequests>,
    /// Declared decompressor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decompressor: Option<Decompressor>,
    /// Declared dma.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dma: Option<Dma>,
    /// Declared display frame interrupts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_frames: Option<DisplayFrames>,
    /// Declared display output register windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_outputs: Option<Vec<DisplayOutput>>,
    /// Declared dmp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dmp: Option<Dmp>,
    /// Declared flash controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash_controller: Option<FlashController>,
    /// Declared gic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gic: Option<Gic>,
    /// Declared gpio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpio: Option<Gpio>,
    /// Declared imaging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imaging: Option<ImagingRegisters>,
    /// Declared X-Processor Pro clock bank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pll_clocks: Option<PllClocks>,
    /// Declared X-Processor Pro imaging control page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imaging_control: Option<ImagingControl>,
    /// Declared X-Processor Pro imaging frame sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_sync: Option<FrameSync>,
    /// Declared X-Processor Pro register blocks with no work in flight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub register_blocks: Vec<RegisterBlock>,
    /// Declared X-Processor Pro imaging statistics engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imaging_statistics: Option<ImagingStatistics>,
    /// Declared X-Processor Pro face detection engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_detection: Option<FaceDetection>,
    /// Declared HDMI transmitter without a sink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdmi_tx: Option<HdmiTransmitter>,
    /// Declared X-Processor Pro SD channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpp_sd: Option<XppSdHost>,
    /// Declared X-Processor Pro reset and gate controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_gates: Option<ResetGates>,
    /// Declared X-Processor Pro DesignWare I2C buses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpp_i2c: Option<XppI2cBuses>,
    /// Declared X-Processor Pro DRAM controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dram_controller: Option<DramController>,
    /// Declared X-Processor Pro read-only DRAM alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dram_alias: Option<DramAlias>,
    /// Declared X-Processor Pro power control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_control: Option<PowerControl>,
    /// Declared X-Processor Pro USB device controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpp_usb: Option<XppUsb>,
    /// Declared interrupts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupts: Option<Interrupts>,
    /// Declared interval timer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_timer: Option<IntervalTimer>,
    /// Declared io groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_groups: Option<IoGroups>,
    /// Declared lcdc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lcdc: Option<Vec<Lcdc>>,
    /// Declared loader dma.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_dma: Option<LoaderDma>,
    /// Declared movdc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movdc: Option<Vec<Movdc>>,
    /// Declared nand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nand: Option<LoaderDma>,
    /// Declared PL330 DMA controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pl330: Option<Pl330>,
    /// Declared sd host clocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_host_clocks: Option<SdHostClocks>,
    /// Declared serial flash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_flash: Option<SerialFlash>,
    /// Declared serial ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_ports: Option<SerialPorts>,
    /// Declared spi.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spi: Option<BTreeMap<String, Spi>>,
    /// Declared temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<Adc>,
    /// Declared usb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb: Option<UsbRegisters>,
}

/// Declared reset release facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareResetRelease"))]
pub struct ResetRelease {
    /// Qualification of the release mechanism for this firmware.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
    /// Immutable instruction words expected at the release destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<ResetCheck>>,
    /// Declared mask.
    pub mask: u64,
    /// Declared register.
    pub register: u64,
    /// Declared source.
    pub source: u64,
    /// Declared target.
    pub target: u64,
}

/// Declared atcm facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareAtcm"))]
pub struct Atcm {
    /// Declared basis.
    pub basis: String,
    /// Declared reset base.
    pub reset_base: u64,
    /// Declared reset enabled.
    pub reset_enabled: bool,
    /// Declared size.
    pub size: u64,
}

/// Declared tcm facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareTcm"))]
pub struct Tcm {
    /// Declared atcm.
    pub atcm: Atcm,
    /// Declared btcm.
    pub btcm: Atcm,
}

/// Declared diagnostic headers facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareDiagnosticHeaders"))]
pub struct DiagnosticHeaders {
    /// Declared address.
    pub address: u64,
    /// Declared name.
    pub name: String,
    /// Declared offset.
    pub offset: u64,
}

/// Declared images facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareBootImage"))]
pub struct BootImage {
    /// Declared address.
    pub address: u64,
    /// Declared name.
    pub name: String,
    /// Declared offset.
    pub offset: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared nand layouts facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareNandBootLayout"))]
pub struct NandBootLayout {
    /// Declared basis.
    pub basis: String,
    /// Declared entry labels.
    pub entry_labels: BTreeMap<String, String>,
    /// Declared images.
    pub images: Vec<BootImage>,
    /// Declared name.
    pub name: String,
}

/// Declared update boot facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareUpdateBoot"))]
pub struct UpdateBoot {
    /// Declared diagnostic headers.
    pub diagnostic_headers: Vec<DiagnosticHeaders>,
    /// Declared flash dat bias.
    pub flash_dat_bias: u64,
    /// Declared nand layouts.
    pub nand_layouts: BTreeMap<String, NandBootLayout>,
}

/// Declared windows options facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareBlockParameters"))]
pub struct BlockParameters {
    /// Declared memory-configuration commit latency in guest ticks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_ticks: Option<u64>,
    /// Declared command latency us.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_latency_us: Option<u64>,
    /// Declared read-only clock configuration input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_word: Option<u32>,
    /// Declared companion irq base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub companion_irq_base: Option<u64>,
    /// Declared receive request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_request: Option<u64>,
    /// Declared source hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hz: Option<u64>,
    /// Declared ticks per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks_per_second: Option<u64>,
    /// Declared self-clearing reset-request offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_offset: Option<u64>,
    /// Declared transmit request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transmit_request: Option<u64>,
}

/// Declared windows facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareRegisterWindow"))]
pub struct RegisterWindow {
    /// Declared attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<String>,
    /// Declared base.
    pub base: u64,
    /// Declared kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Declared name.
    pub name: String,
    /// Declared options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<BlockParameters>,
    /// Declared size.
    pub size: u64,
}

/// Declared soc profile facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SocProfile {
    /// Qualification of the declared processor cache capacities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_basis: Option<String>,
    /// Declared board ticks per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_ticks_per_second: Option<u64>,
    /// Declared boot rom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_rom: Option<BootRom>,
    /// Declared cpu model.
    pub cpu_model: String,
    /// Declared discovery mmio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_mmio: Option<Vec<DiscoveryWindow>>,
    /// Declared engines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<Vec<String>>,
    /// Declared instructions per tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_per_tick: Option<InstructionRate>,
    /// Declared limits.
    pub limits: Vec<String>,
    /// Declared mmu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmu: Option<String>,
    /// Declared private mmio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_mmio: Option<Vec<CoreWindow>>,
    /// Declared register maps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register_maps: Option<RegisterLayouts>,
    /// Declared reset release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_release: Option<ResetRelease>,
    /// Declared runtime abi.
    pub runtime_abi: String,
    /// Declared tcm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcm: Option<Tcm>,
    /// Declared update boot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_boot: Option<UpdateBoot>,
    /// Declared windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<RegisterWindow>>,
}

/// Device implementations selected by a declared hardware graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DeviceKind {
    /// Optional removable-card controller.
    SDCardController,
    /// Optional wireless SDIO controller.
    WiFiSDIOController,
    /// Optional autofocus frame source.
    AfsFrameSource,
    /// Optional JPEG decoder.
    JPEGDecodeControl,
    /// Optional lens-homing fixture.
    LensHomingFixture,
    /// A F Extraction Control.
    AFExtractionControl,
    /// Approximate J P E G Encode Control.
    ApproximateJPEGEncodeControl,
    /// H.264 compression, decoding and addressed DMA controller.
    AvcCodecControl,
    /// Addressed image transfer controller.
    ImageTransferControl,
    /// Banked Lens Controller.
    BankedLensController,
    /// Boot Controls.
    BootControls,
    /// Control Bus.
    ControlBus,
    /// Display Formatter.
    DisplayFormatter,
    /// Display Scanout.
    DisplayScanout,
    /// Display Timing.
    DisplayTiming,
    /// F E80 Control.
    FE80Control,
    /// Face Analysis Control.
    FaceAnalysisControl,
    /// Fuji Decompressor.
    FujiDecompressor,
    /// Graphics Accelerator.
    GraphicsAccelerator,
    /// H D M I Sink.
    HDMISink,
    /// H D M I Startup.
    HDMIStartup,
    /// Image Combination.
    ImageCombination,
    /// Image Parameter Control.
    ImageParameterControl,
    /// Image Scaler.
    ImageScaler,
    /// Image Warp Control.
    ImageWarpControl,
    /// Image Work Control.
    ImageWorkControl,
    /// Input Auxiliary Control.
    InputAuxiliaryControl,
    /// Input Processing Control.
    InputProcessingControl,
    /// Kernel Interrupts.
    KernelInterrupts,
    /// Lens Bridge.
    LensBridge,
    /// Lens Endpoint.
    LensEndpoint,
    /// Lens Register Motor.
    LensRegisterMotor,
    /// Loader Memory D M A.
    LoaderMemoryDMA,
    /// Memory Readback.
    MemoryReadback,
    /// Mixed Width Lens Controller.
    MixedWidthLensController,
    /// Nand Controller.
    NandController,
    /// Output Control.
    OutputControl,
    /// Power Devices.
    PowerDevices,
    /// Prefilter Control.
    PrefilterControl,
    /// Producer Control.
    ProducerControl,
    /// Pulse Input Capture.
    PulseInputCapture,
    /// Pulse Outputs.
    PulseOutputs,
    /// Pulse Timer Control.
    PulseTimerControl,
    /// Quadrature Counters.
    QuadratureCounters,
    /// Raw Conversion Registers.
    RawConversionRegisters,
    /// Raw Statistics Control.
    RawStatisticsControl,
    /// S P I1 Serial Bus.
    SPI1SerialBus,
    /// S P I2 Serial.
    SPI2Serial,
    /// Secondary Control Bus.
    SecondaryControlBus,
    /// Serial Audio Control Registers.
    SerialAudioControlRegisters,
    /// Serial Audio D M A.
    SerialAudioDMA,
    /// Serial Audio Transfer Control Registers.
    SerialAudioTransferControlRegisters,
    /// Serial Audio Transmit Sink.
    SerialAudioTransmitSink,
    /// Clocked external PCM input and serial receive DMA.
    SerialAudioReceiveSource,
    /// Three Byte D C Zoom Lens Controller.
    ThreeByteDCZoomLensController,
    /// Three Byte Lens Controller.
    ThreeByteLensController,
    /// U S B Control.
    USBControl,
    /// Window Analysis Control.
    WindowAnalysisControl,
    /// Word Stabilizer.
    WordStabilizer,
    /// X70 Sensor Capture D M A.
    X70SensorCaptureDMA,
    /// X70 Still I S P Control.
    X70StillISPControl,
}

impl DeviceGraph {
    /// Check unique nodes, complete ordering and references, including model alternatives.
    pub fn validate(&self) -> Result<(), String> {
        use std::collections::BTreeSet;
        let names: BTreeSet<_> = self.devices.iter().map(|node| node.name.as_str()).collect();
        if names.len() != self.devices.len() {
            return Err("hardware graph repeats a device name".into());
        }
        let mut available = names.clone();
        for node in &self.devices {
            fujimod_catalog_identity::validate_id(&node.name)?;
            if let Some(choices) = &node.model_choices {
                for model in choices.models.values() {
                    for dependency in &model.dependencies {
                        fujimod_catalog_identity::validate_id(&dependency.name)?;
                        available.insert(&dependency.name);
                    }
                }
            }
        }
        for node in &self.devices {
            let mut references: Vec<&String> = node.refs.values().collect();
            if let Some(choices) = &node.model_choices {
                for model in choices.models.values() {
                    references.extend(model.refs.values());
                    for dependency in &model.dependencies {
                        references.extend(dependency.refs.values());
                    }
                }
            }
            if references
                .iter()
                .any(|name| !available.contains(name.as_str()))
            {
                return Err(format!(
                    "hardware device {} refers to an absent device",
                    node.name
                ));
            }
        }
        for order in [&self.construction_order, &self.attachment_order] {
            let ordered: BTreeSet<_> = order.iter().map(String::as_str).collect();
            if ordered != names || ordered.len() != order.len() {
                return Err("hardware graph order must name every device exactly once".into());
            }
        }
        Ok(())
    }
}

/// A checked word at a secondary processor release destination.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "HardwareResetCheck"))]
pub struct ResetCheck {
    /// Address of the expected word.
    pub address: u64,
    /// Expected bytes in memory order.
    pub expected_hex: String,
}
