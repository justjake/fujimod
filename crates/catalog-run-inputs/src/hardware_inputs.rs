//! Declared reset inputs kept separately from physical hardware references.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An explicitly empty device-reference set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetEmpty"))]
pub struct Empty {}

/// Declared adc input facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetAdcInput"))]
pub struct AdcInput {
    /// Declared ambient celsius.
    pub ambient_celsius: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared samples.
    pub samples: Vec<u64>,
    /// Declared thermal channels.
    pub thermal_channels: Vec<u64>,
}

/// Declared adc facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetAdc"))]
pub struct Adc {
    /// Declared samples.
    pub samples: Vec<u64>,
}

/// Declared flash facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetFlash"))]
pub struct Flash {
    /// Declared jedec id.
    pub jedec_id: u64,
}

/// Declared seed facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSeed"))]
pub struct Seed {
    /// Declared offset.
    pub offset: u64,
    /// Declared value.
    pub value: u64,
}

/// Declared flash aperture facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetFlashAperture"))]
pub struct FlashAperture {
    /// Declared erased tail.
    pub erased_tail: u64,
    /// Declared seed.
    pub seed: Vec<Seed>,
}

/// Declared feedback facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetFeedback"))]
pub struct Feedback {
    /// Declared bit.
    pub bit: u64,
    /// Declared input pin.
    pub input_pin: u64,
    /// Declared input port.
    pub input_port: u64,
    /// Declared port.
    pub port: u64,
    /// Declared register.
    pub register: u64,
}

/// Declared initial outputs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetInitialOutputs"))]
pub struct InitialOutputs {
    /// Declared levels.
    pub levels: u64,
    /// Declared port.
    pub port: u64,
}

/// Declared input irqs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetInputIrqs"))]
pub struct InputIrqs {
    /// Declared active high.
    pub active_high: bool,
    /// Declared line.
    pub line: u64,
    /// Declared pin.
    pub pin: u64,
    /// Declared port.
    pub port: u64,
}

/// Declared gpio facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetGpio"))]
pub struct Gpio {
    /// Declared feedback.
    pub feedback: Vec<Feedback>,
    /// Declared initial outputs.
    pub initial_outputs: Vec<InitialOutputs>,
    /// Declared input irqs.
    pub input_irqs: Vec<InputIrqs>,
}

/// Declared movdc0 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetMovdc0"))]
pub struct Movdc0 {
    /// Declared frame period us.
    pub frame_period_us: u64,
    /// Declared source luma.
    pub source_luma: u64,
}

/// Declared receiver0 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetReceiver0"))]
pub struct Receiver0 {
    /// Declared frame active us.
    pub frame_active_us: u64,
    /// Declared frame period us.
    pub frame_period_us: u64,
}

/// Declared receiver1 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetReceiver1"))]
pub struct Receiver1 {
    /// Declared frame period us.
    pub frame_period_us: u64,
}

/// Declared resize facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetResize"))]
pub struct Resize {
    /// Declared completion us.
    pub completion_us: u64,
}

/// Declared rifp facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetRifp"))]
pub struct Rifp {
    /// Declared completion us.
    pub completion_us: u64,
    /// Declared source luma.
    pub source_luma: u64,
}

/// Declared part facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetPart"))]
pub struct Part {
    /// Declared idle high.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_high: Option<bool>,
    /// Declared kind.
    pub kind: String,
    /// Declared revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<Vec<u64>>,
}

/// Declared select facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSelect"))]
pub struct Select {
    /// Declared active high.
    pub active_high: bool,
    /// Declared pin.
    pub pin: u64,
    /// Declared port.
    pub port: u64,
}

/// Declared endpoints facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetEndpoints"))]
pub struct Endpoints {
    /// Declared part.
    pub part: Part,
    /// Declared select.
    pub select: Select,
}

/// Declared wire facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetWire"))]
pub struct Wire {
    /// Declared endpoints.
    pub endpoints: Vec<Endpoints>,
    /// Declared kind.
    pub kind: String,
}

/// Declared serial facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSerial"))]
pub struct Serial {
    /// Declared wire.
    pub wire: Wire,
}

/// Declared endpoints part facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetEndpointsPart"))]
pub struct EndpointsPart {
    /// Declared kind.
    pub kind: String,
    /// Declared miso high.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub miso_high: Option<bool>,
    /// Declared rtc seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtc_seconds: Option<u64>,
    /// Declared rtc valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtc_valid: Option<bool>,
}

/// Declared endpoints endpoints facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetEndpointsEndpoints"))]
pub struct EndpointsEndpoints {
    /// Declared part.
    pub part: EndpointsPart,
    /// Declared select.
    pub select: Select,
}

/// Declared spi0 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSpi0"))]
pub struct Spi0 {
    /// Declared endpoints.
    pub endpoints: Vec<EndpointsEndpoints>,
}

/// Declared spi9 facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSpi9"))]
pub struct Spi9 {
    /// Declared select pin.
    pub select_pin: u64,
    /// Declared select port.
    pub select_port: u64,
}

/// Declared application inputs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetApplicationInputs"))]
pub struct ApplicationInputs {
    /// Declared adc.
    pub adc: Adc,
    /// Declared flash.
    pub flash: Flash,
    /// Declared flash aperture.
    pub flash_aperture: FlashAperture,
    /// Declared gpio.
    pub gpio: Gpio,
    /// Declared movdc0.
    pub movdc0: Movdc0,
    /// Declared movdc1.
    pub movdc1: Movdc0,
    /// Declared movdc2.
    pub movdc2: Movdc0,
    /// Declared preview.
    pub preview: Movdc0,
    /// Declared receiver0.
    pub receiver0: Receiver0,
    /// Declared receiver1.
    pub receiver1: Receiver1,
    /// Declared resize.
    pub resize: Resize,
    /// Declared rifp.
    pub rifp: Rifp,
    /// Declared serial.
    pub serial: Serial,
    /// Declared spi0.
    pub spi0: Spi0,
    /// Declared spi9.
    pub spi9: Spi9,
}

/// Declared sd card format facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSdCardFormat"))]
pub struct SdCardFormat {
    /// Declared filesystem.
    pub filesystem: String,
    /// Declared label.
    pub label: String,
    /// Declared partition start.
    pub partition_start: u64,
    /// Declared sectors per cluster.
    pub sectors_per_cluster: u64,
}

/// Declared card facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetCard"))]
pub struct Card {
    /// Declared sd card format.
    pub sd_card_format: SdCardFormat,
    /// Declared sd card present.
    pub sd_card_present: bool,
    /// Declared sd card profile.
    pub sd_card_profile: String,
    /// Declared sd card sectors.
    pub sd_card_sectors: u64,
}

/// Declared interval facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetInterval"))]
pub struct Interval {
    /// Declared kind.
    pub kind: String,
    /// Declared lower.
    pub lower: u64,
    /// Declared upper.
    pub upper: u64,
}

/// Declared exe m facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetKeyOverride"))]
pub struct KeyOverride {
    /// Declared channel.
    pub channel: u64,
    /// Declared interval.
    pub interval: Interval,
    /// Declared kind.
    pub kind: String,
}

/// Explicit electrical predicates selected for a named control during a rehearsal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetControlOverride"))]
pub struct ControlOverride {
    /// Logical key identities and their electrical predicates.
    pub input: BTreeMap<String, Vec<KeyOverride>>,
}

/// Declared gpio gpio facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetPadLevel"))]
pub struct PadLevel {
    /// Declared level.
    pub level: u64,
    /// Declared pin.
    pub pin: u64,
    /// Declared port.
    pub port: u64,
}

/// Declared external inputs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetExternalInputs"))]
pub struct ExternalInputs {
    /// Declared gpio.
    pub gpio: Vec<PadLevel>,
}

/// Declared code witnesses facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetCodeWindow"))]
pub struct CodeWindow {
    /// Extent of the hash-identified source window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// Declared address.
    pub address: u64,
    /// Declared sha256.
    pub sha256: String,
}

/// Declared diagnostics facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetDiagnostic"))]
pub struct Diagnostic {
    /// Declared address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u64>,
    /// Declared name.
    pub name: String,
    /// Declared offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// Declared pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<u64>,
    /// Declared size.
    pub size: u64,
}

/// Declared result query facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetResultQuery"))]
pub struct ResultQuery {
    /// Declared address.
    pub address: u64,
    /// Declared class offset.
    pub class_offset: u64,
    /// Declared position offset.
    pub position_offset: u64,
    /// Declared valid offset.
    pub valid_offset: u64,
}

/// Declared firmwares facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetAutofocusObservations"))]
pub struct AutofocusObservations {
    /// Declared code witnesses.
    pub code_witnesses: Vec<CodeWindow>,
    /// Declared complete state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete_state: Option<u64>,
    /// Declared diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<Diagnostic>>,
    /// Declared primary states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_states: Option<u64>,
    /// Declared rec ui.
    pub rec_ui: u64,
    /// Declared result query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_query: Option<ResultQuery>,
    /// Declared sample slots.
    pub sample_slots: Vec<u64>,
    /// Declared system.
    pub system: u64,
    /// Declared ui.
    pub ui: u64,
}

/// Declared shutter facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetShutter"))]
pub struct Shutter {
    /// Declared active level.
    pub active_level: u64,
    /// Declared pin.
    pub pin: u64,
    /// Declared port.
    pub port: u64,
}

/// Declared autofocus gate facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetAutofocusGate"))]
pub struct AutofocusGate {
    /// Declared boot entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_entry: Option<String>,
    /// Declared card.
    pub card: Card,
    /// Declared completion hold ticks.
    pub completion_hold_ticks: u64,
    /// Declared control overrides.
    pub control_overrides: BTreeMap<String, ControlOverride>,
    /// Declared external inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_inputs: Option<ExternalInputs>,
    /// Declared firmwares.
    pub firmwares: BTreeMap<String, AutofocusObservations>,
    /// Declared first boot instructions.
    pub first_boot_instructions: u64,
    /// Declared key hold ticks.
    pub key_hold_ticks: u64,
    /// Declared key release ticks.
    pub key_release_ticks: u64,
    /// Declared restart instructions.
    pub restart_instructions: u64,
    /// Declared scan instructions.
    pub scan_instructions: u64,
    /// Declared scene settle ticks.
    pub scene_settle_ticks: u64,
    /// Declared setup keys.
    pub setup_keys: Vec<String>,
    /// Declared shutter.
    pub shutter: Shutter,
}

/// Declared gpio input facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetGpioInput"))]
pub struct GpioInput {
    /// Declared basis.
    pub basis: String,
    /// Declared levels.
    pub levels: Vec<u64>,
}

/// Declared inputs gpio facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetInputsGpio"))]
pub struct InputsGpio {
    /// Declared boot source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_source: Option<u64>,
    /// Declared gpio inputs.
    pub gpio_inputs: BTreeMap<String, u64>,
    /// Declared profile name.
    pub profile_name: String,
}

/// Declared power facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetPower"))]
pub struct Power {
    /// Declared adc samples.
    pub adc_samples: Vec<u64>,
    /// Declared boot reply.
    pub boot_reply: u64,
    /// Declared mcu registers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcu_registers: Option<Vec<u64>>,
    /// Declared panel contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_contract: Option<PanelContract>,
    /// Declared profile name.
    pub profile_name: String,
    /// Declared spi0 input profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spi0_input_profile: Option<String>,
    /// Declared spi endpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spi_endpoints: Option<Vec<SerialEndpointInputs>>,
    /// Declared spi status registers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spi_status_registers: Option<Vec<Vec<u64>>>,
}

/// Declared hardwarereset inputs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetHardwareresetInputs"))]
pub struct HardwareresetInputs {
    /// Declared gpio.
    pub gpio: InputsGpio,
    /// Declared mcu packet basis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcu_packet_basis: Option<String>,
    /// Declared mcu packet hex.
    pub mcu_packet_hex: String,
    /// Declared power.
    pub power: Power,
    /// Declared usb connected.
    pub usb_connected: bool,
}

/// Declared words facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetWords"))]
pub struct Words {
    /// Declared name.
    pub name: String,
    /// Declared offset.
    pub offset: u64,
    /// Declared value.
    pub value: u64,
}

/// Declared loader handoff facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetLoaderHandoff"))]
pub struct LoaderHandoff {
    /// Declared address.
    pub address: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared fill.
    pub fill: u64,
    /// Declared size.
    pub size: u64,
    /// Declared words.
    pub words: Vec<Words>,
}

/// Declared processed frame input facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetProcessedFrameInput"))]
pub struct ProcessedFrameInput {
    /// Declared basis.
    pub basis: String,
    /// Declared frame period ticks.
    pub frame_period_ticks: u64,
    /// Declared luma.
    pub luma: u64,
}

/// Declared read gate facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetReadGate"))]
pub struct ReadGate {
    /// Declared mask.
    pub mask: u64,
    /// Declared register.
    pub register: u64,
}

/// Declared statistics scene facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetStatisticsScene"))]
pub struct StatisticsScene {
    /// Declared black level.
    pub black_level: u64,
    /// Declared luma.
    pub luma: u64,
    /// Declared name.
    pub name: String,
    /// Declared samples per cell.
    pub samples_per_cell: u64,
    /// Declared signal shift.
    pub signal_shift: u64,
}

/// Declared sensor input facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSensorInput"))]
pub struct SensorInput {
    /// Declared basis.
    pub basis: String,
    /// Declared frame active ticks.
    pub frame_active_ticks: u64,
    /// Declared frame period ticks.
    pub frame_period_ticks: u64,
    /// Declared read gate.
    pub read_gate: ReadGate,
    /// Declared statistics scene.
    pub statistics_scene: StatisticsScene,
}

/// Contents behind a memory-mapped flash read aperture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetFlashApertureContents"))]
pub struct FlashApertureContents {
    /// Declared basis.
    pub basis: String,
    /// Declared contents.
    pub contents: String,
}

/// Declared serial flash geometry facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSerialFlashGeometry"))]
pub struct SerialFlashGeometry {
    /// Declared basis.
    pub basis: String,
    /// Declared erase bytes.
    pub erase_bytes: u64,
    /// Declared erase ticks.
    pub erase_ticks: u64,
    /// Declared page bytes.
    pub page_bytes: u64,
    /// Declared page ticks.
    pub page_ticks: u64,
}

/// Declared serial flash facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSerialFlash"))]
pub struct SerialFlash {
    /// Declared basis.
    pub basis: String,
    /// Declared contents.
    pub contents: String,
    /// Declared geometry.
    pub geometry: SerialFlashGeometry,
    /// Declared jedec id.
    pub jedec_id: u64,
}

/// Declared power registers facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetPowerRegisters"))]
pub struct PowerRegisters {
    /// Declared ambient celsius.
    pub ambient_celsius: u64,
    /// Declared basis.
    pub basis: String,
    /// Declared chip select.
    pub chip_select: Select,
    /// Declared inputs.
    pub inputs: BTreeMap<String, u64>,
    /// Declared rtc basis.
    pub rtc_basis: String,
    /// Declared rtc ticks per second.
    pub rtc_ticks_per_second: u64,
}

/// Declared disable writes facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetDisableWrites"))]
pub struct DisableWrites {
    /// Declared register.
    pub register: u64,
    /// Declared value.
    pub value: u64,
}

/// Declared strobe facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetStrobe"))]
pub struct Strobe {
    /// Declared clear from ticks.
    pub clear_from_ticks: u64,
    /// Declared mask.
    pub mask: u64,
    /// Declared set through ticks.
    pub set_through_ticks: u64,
}

/// Declared rtc facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetRtc"))]
pub struct Rtc {
    /// Declared commit value.
    pub commit_value: u64,
    /// Declared control.
    pub control: u64,
    /// Declared initial control.
    pub initial_control: u64,
    /// Declared initial seconds.
    pub initial_seconds: u64,
    /// Declared read high.
    pub read_high: u64,
    /// Declared read low.
    pub read_low: u64,
    /// Declared retained valid mask.
    pub retained_valid_mask: u64,
    /// Declared stage high.
    pub stage_high: u64,
    /// Declared stage low.
    pub stage_low: u64,
    /// Declared strobe.
    pub strobe: Strobe,
    /// Declared ticks per second.
    pub ticks_per_second: u64,
    /// Declared valid mask.
    pub valid_mask: u64,
}

/// Declared power checksum registers facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetPowerChecksumRegisters"))]
pub struct PowerChecksumRegisters {
    /// Declared basis.
    pub basis: String,
    /// Declared chip select.
    pub chip_select: Select,
    /// Declared disable writes.
    pub disable_writes: Vec<DisableWrites>,
    /// Declared inputs.
    pub inputs: BTreeMap<String, u64>,
    /// Declared retained registers.
    pub retained_registers: Vec<u64>,
    /// Declared rtc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtc: Option<Rtc>,
}

/// Declared spi inputs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetSpiInputs"))]
pub struct SpiInputs {
    /// Declared basis.
    pub basis: String,
    /// Declared level.
    pub level: String,
    /// Declared power checksum registers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_checksum_registers: Option<PowerChecksumRegisters>,
    /// Declared power registers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_registers: Option<PowerRegisters>,
}

/// Declared temperature input facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ResetTemperatureInput"))]
pub struct TemperatureInput {
    /// Declared basis.
    pub basis: String,
    /// Declared millicelsius.
    pub millicelsius: u64,
}

/// Declared hardware reset facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HardwareReset {
    /// Declared adc input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adc_input: Option<AdcInput>,
    /// Declared application inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_inputs: Option<ApplicationInputs>,
    /// Declared autofocus gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autofocus_gate: Option<AutofocusGate>,
    /// Declared execution scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_scope: Option<String>,
    /// Declared flash aperture contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash_aperture: Option<FlashApertureContents>,
    /// Declared gpio input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpio_input: Option<GpioInput>,
    /// Declared inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<HardwareresetInputs>,
    /// Declared kernel unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_unit: Option<String>,
    /// Declared loader handoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_handoff: Option<LoaderHandoff>,
    /// Declared processed frame input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processed_frame_input: Option<ProcessedFrameInput>,
    /// Declared sensor input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_input: Option<SensorInput>,
    /// Declared serial flash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_flash: Option<SerialFlash>,
    /// Declared spi inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spi_inputs: Option<BTreeMap<String, SpiInputs>>,
    /// Declared temperature input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_input: Option<TemperatureInput>,
}

/// Stable schema identity for explicit body reset inputs.
pub const HARDWARE_INPUTS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-hardware-inputs.generated.schema.json";

/// A named set of reset inputs for one body hardware profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HardwareInputs {
    /// Schema identity.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Input profile identity.
    pub id: String,
    /// Body receiving these inputs.
    pub body_id: String,
    /// Declared external and reset values.
    pub reset: HardwareReset,
}

impl HardwareInputs {
    /// Resolve a bundled body reset-input document by its stable identity.
    pub fn bundled(id: &str) -> Result<Self, String> {
        fujimod_catalog_identity::validate_id(id)?;
        let document = super::BUNDLED_HARDWARE_INPUTS
            .iter()
            .find_map(|(candidate, document)| (*candidate == id).then_some(*document))
            .ok_or_else(|| format!("unsupported hardware-input profile: {id}"))?;
        let inputs: Self = serde_json::from_str(document)
            .map_err(|error| format!("hardware-input profile {id}: {error}"))?;
        if inputs.schema != 1
            || inputs.schema_uri != HARDWARE_INPUTS_SCHEMA_URI
            || inputs.id != id
        {
            return Err(format!("hardware-input profile {id} schema or identity differs"));
        }
        fujimod_catalog_identity::validate_id(&inputs.body_id)?;
        Ok(inputs)
    }
}

/// Panel transport words, chip-selection conditions, framing and endpoint index.
pub type PanelContract = (Vec<u64>, Vec<u64>, Vec<Option<u64>>, u64);

/// Serial endpoint index, configuration, selection, read support and register inputs.
pub type SerialEndpointInputs = (u64, u64, Vec<u64>, bool, Vec<Vec<u64>>);
