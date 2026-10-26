//! Closed startup inputs and scheduling limits for native device models.
//! Physical windows, IRQ wiring and device dependencies belong to hardware profiles.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunNativeDeviceInputs {
    #[serde(rename = "exr2-interrupt-controller")]
    Exr2InterruptController(NativeExr2InterruptControllerInputs),
    #[serde(rename = "exr2-system-timers")]
    Exr2SystemTimers(NativeExr2SystemTimersInputs),
    #[serde(rename = "core-release")]
    CoreRelease(NativeCoreReleaseInputs),
    #[serde(rename = "gpio")]
    Gpio(NativeGpioInputs),
    #[serde(rename = "spi-controller")]
    SpiController(NativeSpiControllerInputs),
    #[serde(rename = "dram")]
    Dram(NativeDramInputs),
    #[serde(rename = "dram-phy")]
    DramPhy(NativeDramPhyInputs),
    #[serde(rename = "reset")]
    Reset(NativeResetInputs),
    #[serde(rename = "clocks")]
    Clocks(NativeClocksInputs),
    #[serde(rename = "block-98000")]
    Block98000(NativeBlock98000Inputs),
    #[serde(rename = "sdhci-clocks")]
    SdhciClocks(NativeSdhciClocksInputs),
    #[serde(rename = "adc")]
    Adc(NativeAdcInputs),
    #[serde(rename = "statistics")]
    Statistics(NativeStatisticsInputs),
    #[serde(rename = "output-channel")]
    OutputChannel(NativeOutputChannelInputs),
    #[serde(rename = "face-detector")]
    FaceDetector(NativeFaceDetectorInputs),
    #[serde(rename = "phase-detection")]
    PhaseDetection(NativePhaseDetectionInputs),
    #[serde(rename = "external-frames")]
    ExternalFrames(NativeExternalFramesInputs),
    #[serde(rename = "sdio-host")]
    SdioHost(NativeSdioHostInputs),
    #[serde(rename = "nand")]
    Nand(NativeNandInputs),
    #[serde(rename = "lz")]
    Lz(NativeLzInputs),
    #[serde(rename = "memory-dma")]
    MemoryDma(NativeMemoryDmaInputs),
    #[serde(rename = "serial-dma")]
    SerialDma(NativeSerialDmaInputs),
    #[serde(rename = "usb")]
    Usb(NativeUsbInputs),
    #[serde(rename = "power")]
    Power(NativePowerInputs),
    #[serde(rename = "serial-audio")]
    SerialAudio(NativeSerialAudioInputs),
    #[serde(rename = "exr2-pulse")]
    Exr2Pulse(NativeExr2PulseInputs),
    #[serde(rename = "fe80-processing")]
    Fe80Processing(NativeFe80ProcessingInputs),
    #[serde(rename = "image-input")]
    ImageInput(NativeImageInputInputs),
    #[serde(rename = "block11000")]
    Block11000(NativeBlock11000Inputs),
    #[serde(rename = "parameters")]
    Parameters(NativeParametersInputs),
    #[serde(rename = "hdmi-sink")]
    HdmiSink(NativeHdmiSinkInputs),
    #[serde(rename = "hdmi")]
    Hdmi(NativeHdmiInputs),
    #[serde(rename = "block86000")]
    Block86000(NativeBlock86000Inputs),
    #[serde(rename = "vector-gpu")]
    VectorGpu(NativeVectorGpuInputs),
    #[serde(rename = "scanout")]
    Scanout(NativeScanoutInputs),
    #[serde(rename = "display-timing")]
    DisplayTiming(NativeDisplayTimingInputs),
    #[serde(rename = "formatter")]
    Formatter(NativeFormatterInputs),
    #[serde(rename = "image-combination")]
    ImageCombination(NativeImageCombinationInputs),
    #[serde(rename = "encoder")]
    Encoder(NativeEncoderInputs),
    #[serde(rename = "i2c")]
    I2c(NativeControlBusInputs),
}

/// Closed NativeExr2InterruptControllerInputsArbitration selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeExr2InterruptControllerInputsArbitration {
    #[serde(rename = "active-bitmap")]
    ActiveBitmap,
}

/// Declared NativeExr2InterruptControllerInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeExr2InterruptControllerInputs {
    pub arbitration: NativeExr2InterruptControllerInputsArbitration,
}

/// Declared NativeExr2SystemTimersInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeExr2SystemTimersInputs {
    pub main_targets: Vec<u64>,
}

/// Declared NativeCoreReleaseInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeCoreReleaseInputs {
    pub released: bool,
}

/// Declared NativeGpioInputsOutputLevelsItemPin startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeGpioInputsOutputLevelsItemPin {
    pub port: u64,
    pub pin: u64,
}

/// Declared NativeGpioInputsOutputLevelsItem startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeGpioInputsOutputLevelsItem {
    pub pin: NativeGpioInputsOutputLevelsItemPin,
    pub high: bool,
}

/// Declared NativeGpioInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeGpioInputs {
    pub input_levels: Vec<u64>,
    pub output_levels: Vec<NativeGpioInputsOutputLevelsItem>,
}

/// Declared NativeSpiControllerInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeSpiControllerInputs {}

/// Declared NativeDramInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeDramInputs {}

/// Declared NativeDramPhyInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeDramPhyInputs {}

/// Declared NativeResetInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeResetInputs {}

/// Declared NativeClocksInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeClocksInputs {
    pub output_settle_ticks: u64,
    pub reset_ready: bool,
}

/// Declared NativeBlock98000Inputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeBlock98000Inputs {
    pub boot_source: u64,
    pub selection_reset: u64,
}

/// Declared NativeSdhciClocksInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeSdhciClocksInputs {}

/// Declared NativeAdcInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeAdcInputs {
    pub conversion_ticks: u64,
    pub samples: Vec<u64>,
}

/// Closed NativeStatisticsInputsClockKind selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeStatisticsInputsClockKind {
    #[serde(rename = "fixed")]
    Fixed,
}

/// Declared NativeStatisticsInputsClock startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeStatisticsInputsClock {
    pub kind: NativeStatisticsInputsClockKind,
    pub ticks: u64,
}

/// Declared NativeStatisticsInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeStatisticsInputs {
    pub clock: NativeStatisticsInputsClock,
    pub ack_ticks: u64,
    pub stop_ticks: u64,
    pub histogram: bool,
    pub external_statistics: bool,
}

/// Closed NativeOutputChannelInputsClockKind selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeOutputChannelInputsClockKind {
    #[serde(rename = "fixed")]
    Fixed,
}

/// Declared NativeOutputChannelInputsClock startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeOutputChannelInputsClock {
    pub kind: NativeOutputChannelInputsClockKind,
    pub ticks: u64,
}

/// Closed NativeOutputChannelInputsGeometry selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeOutputChannelInputsGeometry {
    #[serde(rename = "skip_incomplete")]
    SkipIncomplete,
}

/// Declared NativeOutputChannelInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeOutputChannelInputs {
    pub stop_ticks: u64,
    pub clock: NativeOutputChannelInputsClock,
    pub geometry: NativeOutputChannelInputsGeometry,
}

/// Closed NativeFaceDetectorInputsMode selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeFaceDetectorInputsMode {
    #[serde(rename = "retained_words")]
    RetainedWords,
}

/// Declared NativeFaceDetectorInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeFaceDetectorInputs {
    pub mode: NativeFaceDetectorInputsMode,
}

/// Closed NativePhaseDetectionInputsArithmetic selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativePhaseDetectionInputsArithmetic {
    #[serde(rename = "aperture_sad")]
    ApertureSad,
}

/// Declared NativePhaseDetectionInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativePhaseDetectionInputs {
    pub arithmetic: NativePhaseDetectionInputsArithmetic,
    pub data_ticks: u64,
    pub stop_ticks: u64,
}

/// Declared NativeExternalFramesInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeExternalFramesInputs {
    pub period_ticks: u64,
}

/// Declared NativeSdioHostInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeSdioHostInputs {
    pub transfer_ticks: u64,
    pub clock_ticks: u64,
    pub rca: u64,
    pub ram_bytes: u64,
}

/// Declared NativeNandInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeNandInputs {
    pub erase_ticks: u64,
}

/// Closed NativeLzInputsResize selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeLzInputsResize {
    #[serde(rename = "declared_packed_pyramid")]
    DeclaredPackedPyramid,
}

/// Declared NativeLzInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeLzInputs {
    pub resize: NativeLzInputsResize,
    pub resize_ticks: u64,
}

/// Declared NativeMemoryDmaInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeMemoryDmaInputs {}

/// Declared NativeSerialDmaInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeSerialDmaInputs {
    pub ram_bytes: u64,
    pub max_transfer_bytes: u64,
}

/// Declared NativeUsbInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeUsbInputs {
    pub endpoints: Option<NativeUsbEndpoints>,
}

/// Declared NativePowerInputsSpi0SelectionPin startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativePowerInputsSpi0SelectionPin {
    pub port: u64,
    pub pin: u64,
}

/// Declared NativePowerInputsSpi0Selection startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativePowerInputsSpi0Selection {
    pub pin: NativePowerInputsSpi0SelectionPin,
    pub high: bool,
}

/// Declared NativePowerInputsSpi0PresencePin startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativePowerInputsSpi0PresencePin {
    pub port: u64,
    pub pin: u64,
}

/// Declared NativePowerInputsSpi0Presence startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativePowerInputsSpi0Presence {
    pub pin: NativePowerInputsSpi0PresencePin,
    pub high: bool,
}

/// Declared NativePowerInputsSpi0 startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativePowerInputsSpi0 {
    pub required: Vec<u64>,
    pub selection: NativePowerInputsSpi0Selection,
    pub presence: NativePowerInputsSpi0Presence,
}

/// Declared NativePowerInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativePowerInputs {
    pub transfer_ticks: u64,
    pub max_program_bytes: u64,
    pub spi0: NativePowerInputsSpi0,
}

/// Declared NativeSerialAudioInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeSerialAudioInputs {}

/// Declared NativeExr2PulseInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeExr2PulseInputs {
    pub counter_channels: Vec<u64>,
    pub timer_channels: Vec<u64>,
    pub quadrature: bool,
    pub initial_phase: u64,
    pub capture_channel: Option<u64>,
}

/// Declared NativeFe80ProcessingInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeFe80ProcessingInputs {
    pub irq: u64,
    pub pixels_per_tick: u64,
}

/// Closed NativeImageInputInputsCommand selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeImageInputInputsCommand {
    #[serde(rename = "packed_writes")]
    PackedWrites,
}

/// Closed NativeImageInputInputsPhaseAck selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeImageInputInputsPhaseAck {
    #[serde(rename = "inactive")]
    Inactive,
}

/// Closed NativeImageInputInputsClockMode selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeImageInputInputsClockMode {
    #[serde(rename = "input_enable")]
    InputEnable,
}

/// Closed NativeImageInputInputsClockSourceKind selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeImageInputInputsClockSourceKind {
    #[serde(rename = "periodic")]
    Periodic,
}

/// Declared NativeImageInputInputsClockSource startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeImageInputInputsClockSource {
    pub kind: NativeImageInputInputsClockSourceKind,
    pub period_ticks: u64,
    pub first_delay_ticks: Option<u64>,
}

/// Declared NativeImageInputInputsClock startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeImageInputInputsClock {
    pub mode: NativeImageInputInputsClockMode,
    pub source: NativeImageInputInputsClockSource,
    pub secondary: bool,
    pub companion_secondary: bool,
}

/// Declared NativeImageInputInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeImageInputInputs {
    pub reset_ticks: u64,
    pub command_ticks: u64,
    pub phase_ticks: u64,
    pub command: NativeImageInputInputsCommand,
    pub command_controls: Vec<u64>,
    pub phase_ack: NativeImageInputInputsPhaseAck,
    pub clock: NativeImageInputInputsClock,
    pub sensor_period_ticks: Option<u64>,
    pub raw_dma_ticks: u64,
    pub readback_ticks: u64,
    pub cfa: Vec<u64>,
}

/// Declared NativeBlock11000InputsControlModeClearbit0ack startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeBlock11000InputsControlModeClearbit0ack {
    pub delay_ticks: u64,
}

/// Declared NativeBlock11000InputsControlMode startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeBlock11000InputsControlMode {
    #[serde(rename = "ClearBit0Ack")]
    pub clear_bit0_ack: NativeBlock11000InputsControlModeClearbit0ack,
}

/// Declared NativeBlock11000Inputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeBlock11000Inputs {
    pub control_mode: NativeBlock11000InputsControlMode,
}

/// Declared NativeParametersInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeParametersInputs {}

/// Declared NativeHdmiSinkInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeHdmiSinkInputs {
    pub connected: bool,
    pub edid: Vec<u8>,
}

/// Declared NativeHdmiInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeHdmiInputs {}

/// Declared NativeBlock86000Inputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeBlock86000Inputs {}

/// Closed NativeVectorGpuInputsCopyProfile selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeVectorGpuInputsCopyProfile {
    #[serde(rename = "vg-32bpp-untile-unrotated")]
    Vg32bppUntileUnrotated,
}

/// Closed NativeVectorGpuInputsClearProfile selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeVectorGpuInputsClearProfile {
    #[serde(rename = "declared-vg-ui-primitives-unmeasured")]
    DeclaredVgUiPrimitivesUnmeasured,
}

/// Declared NativeVectorGpuInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeVectorGpuInputs {
    pub ram_bytes: u64,
    pub max_fill_bytes: u64,
    pub max_copy_bytes: u64,
    pub max_clear_bytes: u64,
    pub bytes_per_tick: u64,
    pub max_command_capture_bytes: u64,
    pub copy_profile: NativeVectorGpuInputsCopyProfile,
    pub clear_profile: NativeVectorGpuInputsClearProfile,
}

/// Closed NativeScanoutInputsPacking selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeScanoutInputsPacking {
    #[serde(rename = "yycbcr")]
    Yycbcr,
}

/// Closed NativeScanoutInputsMatrix selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeScanoutInputsMatrix {
    #[serde(rename = "bt601-full")]
    Bt601Full,
}

/// Closed NativeScanoutInputsGraphicsProfile selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeScanoutInputsGraphicsProfile {
    #[serde(rename = "mode1-abgr8888-straight")]
    Mode1Abgr8888Straight,
}

/// Closed NativeScanoutInputsBackdropProfile selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeScanoutInputsBackdropProfile {
    #[serde(rename = "declared-black-canvas-unmeasured")]
    DeclaredBlackCanvasUnmeasured,
}

/// Closed NativeScanoutInputsRectangleProfile selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeScanoutInputsRectangleProfile {
    #[serde(rename = "exr2-signed-ycbcr-rectangle-overlay-unmeasured")]
    Exr2SignedYcbcrRectangleOverlayUnmeasured,
}

/// Declared NativeScanoutInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeScanoutInputs {
    pub ram_bytes: u64,
    pub packing: NativeScanoutInputsPacking,
    pub matrix: NativeScanoutInputsMatrix,
    pub graphics_profile: NativeScanoutInputsGraphicsProfile,
    pub backdrop_profile: NativeScanoutInputsBackdropProfile,
    pub rectangle_profile: NativeScanoutInputsRectangleProfile,
    pub max_pixels: u64,
    pub reset_ticks: u64,
}

/// Closed NativeDisplayTimingInputsMode selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeDisplayTimingInputsMode {
    #[serde(rename = "configured_clocked")]
    ConfiguredClocked,
}

/// Declared NativeDisplayTimingInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeDisplayTimingInputs {
    pub mode: NativeDisplayTimingInputsMode,
    pub frame_ticks: u64,
    pub stop_ticks: u64,
}

/// Declared NativeFormatterInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeFormatterInputs {
    pub latch_delay: u64,
}

/// Closed NativeImageCombinationInputsArithmetic selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeImageCombinationInputsArithmetic {
    #[serde(rename = "packed_image_unmeasured")]
    PackedImageUnmeasured,
}

/// Closed NativeImageCombinationInputsClockKind selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeImageCombinationInputsClockKind {
    #[serde(rename = "throughput")]
    Throughput,
}

/// Declared NativeImageCombinationInputsClock startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeImageCombinationInputsClock {
    pub kind: NativeImageCombinationInputsClockKind,
    pub bytes_per_tick: u64,
}

/// Declared NativeImageCombinationInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeImageCombinationInputs {
    pub reset_ticks: u64,
    pub arithmetic: NativeImageCombinationInputsArithmetic,
    pub clock: NativeImageCombinationInputsClock,
}

/// Declared NativeEncoderInputsPinsItem startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeEncoderInputsPinsItem {
    pub port: u64,
    pub pin: u64,
}

/// Declared NativeEncoderInputs startup inputs and execution limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeEncoderInputs {
    pub pins: Vec<NativeEncoderInputsPinsItem>,
    pub phase_ticks: u64,
    pub rest_phase: u64,
}

/// Control-bus startup model selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeControlBusInputs {
    pub modes: Vec<u8>,
    pub transfer_ticks: u64,
    pub profile: NativeControlBusProfile,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum NativeControlBusProfile {
    #[serde(rename = "endpoint7-stationary-and9-heartbeat-latched-irq-unmeasured")]
    Primary,
    #[serde(rename = "usbcharge-disconnected-pwic-startup-tagged")]
    Secondary,
}

/// External raw-NAND scene selection; geometry must match the selected medium.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunNandSceneInput {
    Raw {
        options: RunNandGeometry,
        base: String,
        overlays: Vec<RunNandAssetPlacement>,
        spare: Vec<RunNandAssetPlacement>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunNandGeometry {
    pub bytes: u64,
    pub page_bytes: u32,
    pub pages_per_block: u32,
    pub spare_bytes: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunNandAssetPlacement {
    pub offset: u64,
    pub asset_id: String,
}

/// Native USB endpoint queue limits and transfer cadence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeUsbEndpoints {
    pub transfer_ticks: u64,
    pub packet_bytes: u32,
    pub dma_limit: u32,
    pub out_endpoints: Vec<(u8, u32)>,
    pub in_endpoints: Vec<(u8, u32)>,
}

impl RunNandSceneInput {
    pub fn validate(&self) -> Result<(), String> {
        let Self::Raw {
            options,
            base,
            overlays,
            spare,
        } = self;
        fujimod_catalog_identity::validate_id(base)?;
        if options.bytes == 0
            || !options.page_bytes.is_power_of_two()
            || options.pages_per_block == 0
            || options.spare_bytes == 0
            || options.bytes % (u64::from(options.page_bytes) * u64::from(options.pages_per_block))
                != 0
        {
            return Err("run NAND input geometry is invalid".into());
        }
        for placement in overlays.iter().chain(spare) {
            fujimod_catalog_identity::validate_id(&placement.asset_id)?;
            if placement.offset >= options.bytes {
                return Err("run NAND input placement exceeds its medium".into());
            }
        }
        Ok(())
    }
}

impl RunNativeDeviceInputs {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Gpio(input) => {
                if input.input_levels.len() != 16
                    || input.input_levels.iter().any(|level| *level > 0xffff)
                    || input
                        .output_levels
                        .iter()
                        .any(|level| level.pin.port > 15 || level.pin.pin > 15)
                {
                    return Err("native GPIO reset input is invalid".into());
                }
            }
            Self::Adc(input) => {
                if input.conversion_ticks == 0
                    || input.samples.len() != 12
                    || input.samples.iter().any(|sample| *sample > 1023)
                {
                    return Err("native ADC reset input is invalid".into());
                }
            }
            Self::ImageInput(input) => {
                if input.cfa.len() != 36
                    || input.cfa.iter().any(|color| *color > 2)
                    || input.raw_dma_ticks == 0
                    || input.readback_ticks == 0
                {
                    return Err("native image input is invalid".into());
                }
            }
            Self::VectorGpu(input) => {
                if input.bytes_per_tick == 0
                    || input.ram_bytes == 0
                    || input.max_fill_bytes > input.ram_bytes
                    || input.max_copy_bytes > input.ram_bytes
                    || input.max_clear_bytes > input.ram_bytes
                {
                    return Err("native graphics execution limits are invalid".into());
                }
            }
            Self::Usb(input) => {
                if input.endpoints.as_ref().is_some_and(|endpoints| {
                    endpoints.packet_bytes == 0
                        || endpoints.dma_limit == 0
                        || endpoints.transfer_ticks == 0
                }) {
                    return Err("native USB queue limits are invalid".into());
                }
            }
            _ => {}
        }
        Ok(())
    }
}
