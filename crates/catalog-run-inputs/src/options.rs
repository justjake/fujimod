//! Closed emulator policy selections, independent of input file locations.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
#[derive(Default)]
pub struct RunFactoryOptions {
    /// Connect physical sensor frames using the selected clock source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_frame_clock: Option<RunSensorFrameClock>,
    /// Explicit RAW-to-output DMA duration in virtual ticks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_readback_ticks: Option<u64>,
    /// Explicit external serial audio source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_input_profile: Option<RunAudioInputProfile>,
    /// Explicit H.264 backend; absence selects the local platform default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_encoder_backend: Option<fujimod_catalog_construction::VideoEncoderBackend>,
    /// Selected auxiliary control profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auxiliary_control_profile: Option<RunAuxiliaryControlProfile>,
    /// Selected auxiliary control ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auxiliary_control_ticks: Option<u64>,
    /// Selected backdrop profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backdrop_profile: Option<RunBackdropProfile>,
    /// Selected clear profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_profile: Option<RunClearProfile>,
    /// Selected clock seconds 2000. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_seconds_2000: Option<u64>,
    /// Selected control bus profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_bus_profile: Option<RunControlBusProfile>,
    /// Selected copy profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_profile: Option<RunCopyProfile>,
    /// Selected counter input profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter_input_profile: Option<RunCounterInputProfile>,
    /// Selected display frame ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_frame_ticks: Option<u64>,
    /// Selected entry. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<RunEntry>,
    /// Selected face register mode. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_register_mode: Option<RunFaceRegisterMode>,
    /// Declared face DMA stop policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_stop_profile: Option<RunFaceStopProfile>,
    /// Selected graphics profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphics_profile: Option<RunGraphicsProfile>,
    /// Selected histogram data ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram_data_ticks: Option<u64>,
    /// Selected histogram profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram_profile: Option<RunHistogramProfile>,
    /// Selected image work data profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_work_data_profile: Option<RunImageWorkDataProfile>,
    /// Selected image work data ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_work_data_ticks: Option<u64>,
    /// Selected image work stop profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_work_stop_profile: Option<RunImageWorkStopProfile>,
    /// Selected image work stop ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_work_stop_ticks: Option<u64>,
    /// Selected input event profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_event_profile: Option<RunInputEventProfile>,
    /// Selected input event ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_event_ticks: Option<u64>,
    /// Selected input phase profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_phase_profile: Option<RunInputPhaseProfile>,
    /// Selected input processing profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_processing_profile: Option<RunInputProcessingProfile>,
    /// Selected instructions per tick. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_per_tick: Option<u64>,
    /// Selected irq tie profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub irq_tie_profile: Option<RunIrqTieProfile>,
    /// Selected jpeg decode profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jpeg_decode_profile: Option<RunJpegDecodeProfile>,
    /// Selected lens response profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_response_profile: Option<RunLensResponseProfile>,
    /// Selected lens response ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_response_ticks: Option<u64>,
    /// Requested lens continuation policy; consumers must support it explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_continuation_profile: Option<RunLensContinuationProfile>,
    /// Selected mcu profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcu_profile: Option<RunMcuProfile>,
    /// Selected movrsz profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movrsz_profile: Option<RunMovrszProfile>,
    /// Selected movrsz ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movrsz_ticks: Option<u64>,
    /// Selected phase profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_profile: Option<RunPhaseProfile>,
    /// Selected producer ack ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_ack_ticks: Option<u64>,
    /// Selected producer data profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_data_profile: Option<RunProducerDataProfile>,
    /// Selected producer data ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_data_ticks: Option<u64>,
    /// Selected producer stop profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_stop_profile: Option<RunProducerStopProfile>,
    /// Selected producer stop ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_stop_ticks: Option<u64>,
    /// Selected pulse timer profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulse_timer_profile: Option<RunPulseTimerProfile>,
    /// Selected quantum. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantum: Option<u64>,
    /// Selected rectangle profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rectangle_profile: Option<RunRectangleProfile>,
    /// Selected sd card format. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_card_format: Option<RunCardFormat>,
    /// Selected sd card present. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_card_present: Option<bool>,
    /// Selected sd card profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_card_profile: Option<RunSdCardProfile>,
    /// Selected sd card sectors. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_card_sectors: Option<u64>,
    /// Selected secondary bus profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_bus_profile: Option<RunSecondaryBusProfile>,
    /// Selected settings. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<RunSettings>,
    /// Selected spi0 input profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spi0_input_profile: Option<RunSpi0InputProfile>,
    /// Selected storage profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_profile: Option<RunStorageProfile>,
    /// Named camera settings written into the settings bank, with the
    /// `settings` storage profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_settings: Option<std::collections::BTreeMap<String, String>>,
    /// Selected user journals. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_journals: Option<RunUserJournals>,
    /// Selected wifi sdio profile. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi_sdio_profile: Option<RunWifiSdioProfile>,
    /// Selected wifi sdio transfer ticks. Absence preserves the base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi_sdio_transfer_ticks: Option<u64>,
    /// Synthetic HDMI sink input, including the identity of its external EDID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdmi_sink: Option<RunHdmiSink>,
}

/// Synthetic FAT card formatting parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunCardFormat {
    pub label: String,
    pub partition_start: u64,
    pub sectors_per_cluster: u32,
}

/// A bounded external binary input identified independently of its location.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunBinaryInput {
    pub id: String,
    pub size: u64,
    pub sha256: String,
}

/// Synthetic HDMI cable and display-identification input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RunHdmiSink {
    pub connected: bool,
    pub edid: RunBinaryInput,
    pub basis: String,
}

/// Closed selection for auxiliary control profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunAuxiliaryControlProfile {
    #[serde(rename = "stock-control3-bit0-clear-ack-unmeasured")]
    StockControl3Bit0ClearAckUnmeasured,
}

/// Closed selection for backdrop profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunBackdropProfile {
    #[serde(rename = "declared-black-canvas-unmeasured")]
    DeclaredBlackCanvasUnmeasured,
}

/// Closed selection for clear profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunClearProfile {
    #[serde(rename = "declared-vg-ui-primitives-unmeasured")]
    DeclaredVgUiPrimitivesUnmeasured,
}

/// Closed selection for control bus profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunControlBusProfile {
    #[serde(rename = "endpoint7-stationary-and9-heartbeat-latched-irq-unmeasured")]
    Endpoint7StationaryAnd9HeartbeatLatchedIrqUnmeasured,
}

/// Closed selection for copy profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunCopyProfile {
    #[serde(rename = "vg-32bpp-untile-unrotated")]
    Vg32bppUntileUnrotated,
}

/// Closed selection for counter input profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunCounterInputProfile {
    #[serde(rename = "declared-lens-step-feedback-unmeasured")]
    DeclaredLensStepFeedbackUnmeasured,
}

/// Closed selection for entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunEntry {
    #[serde(rename = "loader")]
    Loader,
}

/// Closed selection for face register mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunFaceRegisterMode {
    #[serde(rename = "retained_words")]
    RetainedWords,
}

/// Closed selection for graphics profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunGraphicsProfile {
    #[serde(rename = "mode1-abgr8888-straight")]
    Mode1Abgr8888Straight,
}

/// Closed selection for histogram profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunHistogramProfile {
    #[serde(rename = "declared-postisp-histogram256-u32-unmeasured")]
    DeclaredPostispHistogram256U32Unmeasured,
}

/// Closed selection for image work data profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunImageWorkDataProfile {
    #[serde(rename = "declared-postisp-yycbcr-scene-unmeasured")]
    DeclaredPostispYycbcrSceneUnmeasured,
    #[serde(rename = "xm1-150-declared-continuous-postisp-scene-unmeasured")]
    Xm1150DeclaredContinuousPostispSceneUnmeasured,
}

/// Closed selection for image work stop profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunImageWorkStopProfile {
    #[serde(rename = "stock-idle-image-banks-bit0-stop-ack-unmeasured")]
    StockIdleImageBanksBit0StopAckUnmeasured,
    #[serde(rename = "stock-initial-image-banks-bit0-stop-ack-unmeasured")]
    StockInitialImageBanksBit0StopAckUnmeasured,
}

/// Clock connection for an external physical sensor scene.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
#[serde(rename_all = "snake_case")]
pub enum RunSensorFrameClock {
    Sensor,
    Periodic,
}

/// Closed selection for input event profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunInputEventProfile {
    #[serde(rename = "declared-rec-display-card-late-source-staging-unmeasured")]
    DeclaredRecDisplayCardLateSourceStagingUnmeasured,
    #[serde(rename = "declared-rec-display-close-reentry-plus-one-ready-source0-edge-unmeasured")]
    DeclaredRecDisplayCloseReentryPlusOneReadySource0EdgeUnmeasured,
    #[serde(rename = "xm1-150-configured-phase-clock-unmeasured")]
    Xm1150ConfiguredPhaseClockUnmeasured,
}

/// Closed selection for input phase profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunInputPhaseProfile {
    #[serde(rename = "declared-enabled-or-inactive-phase2-stop-ack-unmeasured")]
    DeclaredEnabledOrInactivePhase2StopAckUnmeasured,
    #[serde(rename = "stock-inactive-phase2-ack")]
    StockInactivePhase2Ack,
    #[serde(rename = "xm1-150-phase2-ack-unmeasured")]
    Xm1150Phase2AckUnmeasured,
}

/// Closed selection for input processing profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunInputProcessingProfile {
    #[serde(rename = "packed-three-byte-command-words")]
    PackedThreeByteCommandWords,
    #[serde(rename = "stock-mode0-81-packed-writes")]
    StockMode081PackedWrites,
    #[serde(rename = "xm1-150-packed-command-writes")]
    Xm1150PackedCommandWrites,
}

/// Closed selection for irq tie profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunIrqTieProfile {
    #[serde(rename = "kernel-active-scan-order")]
    KernelActiveScanOrder,
}

/// Closed selection for jpeg decode profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunJpegDecodeProfile {
    #[serde(rename = "declared-channel9-libjpeg-yycbcr-playback-unmeasured")]
    DeclaredChannel9LibjpegYycbcrPlaybackUnmeasured,
}

/// Closed selection for lens response profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunLensResponseProfile {
    #[serde(rename = "declared-lens-startup-homing-lattice-unmeasured")]
    DeclaredLensStartupHomingLatticeUnmeasured,
}

/// Closed selection for mcu profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunMcuProfile {
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "captured-backup")]
    CapturedBackup,
}

/// Closed selection for movrsz profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunMovrszProfile {
    #[serde(rename = "declared-packed-yycbcr-pyramid-unmeasured")]
    DeclaredPackedYycbcrPyramidUnmeasured,
}

/// Closed selection for phase profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunPhaseProfile {
    #[serde(rename = "configured-clocked")]
    ConfiguredClocked,
}

/// Closed selection for producer data profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunProducerDataProfile {
    /// Replay externally supplied statistics through the declared producer input.
    #[serde(rename = "recorded-statistics")]
    RecordedStatistics,
    #[serde(rename = "xm1-150-declared-continuous-statistics-scene-unmeasured")]
    Xm1150DeclaredContinuousStatisticsSceneUnmeasured,
}

/// Closed selection for producer stop profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunProducerStopProfile {
    #[serde(rename = "stock-idle-stop-handshakes-unmeasured")]
    StockIdleStopHandshakesUnmeasured,
}

/// Closed selection for pulse timer profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunPulseTimerProfile {
    #[serde(rename = "exr2-configured-interval-clocks-unmeasured")]
    Exr2ConfiguredIntervalClocksUnmeasured,
}

/// Closed selection for rectangle profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunRectangleProfile {
    #[serde(rename = "exr2-signed-ycbcr-rectangle-overlay-unmeasured")]
    Exr2SignedYcbcrRectangleOverlayUnmeasured,
    #[serde(rename = "xm1-150-declared-luma-rectangle-overlay-unmeasured")]
    Xm1150DeclaredLumaRectangleOverlayUnmeasured,
}

/// Closed selection for sd card profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunSdCardProfile {
    #[serde(rename = "declared-blank-sdhc-default-speed-unmeasured")]
    DeclaredBlankSdhcDefaultSpeedUnmeasured,
}

/// Closed selection for secondary bus profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunSecondaryBusProfile {
    #[serde(rename = "usbcharge-disconnected-pwic-startup-tagged")]
    UsbchargeDisconnectedPwicStartupTagged,
    /// Explicitly disable the inherited endpoint policy.
    #[serde(rename = "disabled")]
    Disabled,
}

/// Closed selection for settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunSettings {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "owner")]
    Owner,
}

/// Closed selection for spi0 input profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunSpi0InputProfile {
    #[serde(rename = "absent-hot-shoe-flash-fixed-low-miso")]
    AbsentHotShoeFlashFixedLowMiso,
}

/// Closed selection for storage profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunStorageProfile {
    /// A settings bank built from stock defaults and requested camera settings.
    #[serde(rename = "settings")]
    Settings,
    /// The stock user profile built from requested camera settings, over an
    /// erased settings bank.
    #[serde(rename = "profile")]
    Profile,
    /// A new camera as it ships: the body's calibrated settings bank and an
    /// empty user profile, so the firmware runs its first-start setup.
    #[serde(rename = "factory")]
    Factory,
    #[serde(rename = "captured")]
    Captured,
    #[serde(rename = "browser")]
    Browser,
    #[serde(rename = "erased")]
    Erased,
}

/// Closed selection for wifi sdio profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunWifiSdioProfile {
    #[serde(rename = "mt5931-no-peers")]
    Mt5931NoPeers,
    #[serde(rename = "declared-wifi-sdio-one-function-enumeration-unmeasured")]
    DeclaredWifiSdioOneFunctionEnumerationUnmeasured,
}

impl RunFactoryOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.auxiliary_control_ticks == Some(0) {
            return Err("run auxiliary_control_ticks must be positive".into());
        }
        if self.display_frame_ticks == Some(0) {
            return Err("run display_frame_ticks must be positive".into());
        }
        if self.histogram_data_ticks == Some(0) {
            return Err("run histogram_data_ticks must be positive".into());
        }
        if self.image_work_data_ticks == Some(0) {
            return Err("run image_work_data_ticks must be positive".into());
        }
        if self.image_work_stop_ticks == Some(0) {
            return Err("run image_work_stop_ticks must be positive".into());
        }
        if self.input_event_ticks == Some(0) {
            return Err("run input_event_ticks must be positive".into());
        }
        if self.instructions_per_tick == Some(0) {
            return Err("run instructions_per_tick must be positive".into());
        }
        if self.lens_response_ticks == Some(0) {
            return Err("run lens_response_ticks must be positive".into());
        }
        if self.movrsz_ticks == Some(0) {
            return Err("run movrsz_ticks must be positive".into());
        }
        if self.producer_ack_ticks == Some(0) {
            return Err("run producer_ack_ticks must be positive".into());
        }
        if self.producer_data_ticks == Some(0) {
            return Err("run producer_data_ticks must be positive".into());
        }
        if self.producer_stop_ticks == Some(0) {
            return Err("run producer_stop_ticks must be positive".into());
        }
        if self.quantum == Some(0) {
            return Err("run quantum must be positive".into());
        }
        if self.sd_card_sectors == Some(0) {
            return Err("run sd_card_sectors must be positive".into());
        }
        if self.wifi_sdio_transfer_ticks == Some(0) {
            return Err("run wifi_sdio_transfer_ticks must be positive".into());
        }
        if let Some(sink) = &self.hdmi_sink {
            fujimod_catalog_identity::validate_id(&sink.edid.id)?;
            if sink.edid.size == 0
                || sink.edid.size % 128 != 0
                || sink.edid.size > 32768
                || sink.edid.sha256.len() != 64
                || !sink
                    .edid
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err("run HDMI EDID identity is invalid".into());
            }
        }
        if let Some(format) = &self.sd_card_format {
            if format.label.is_empty()
                || format.label.len() > 11
                || !format.label.is_ascii()
                || !format.sectors_per_cluster.is_power_of_two()
                || format.sectors_per_cluster > 128
            {
                return Err("run card format is invalid".into());
            }
        }
        Ok(())
    }
}

/// Selection of externally supplied persistent user journals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
#[serde(rename_all = "kebab-case")]
pub enum RunUserJournals {
    Disabled,
    Restored,
}

/// Named policy inputs that a preset explicitly clears before selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
#[serde(rename_all = "snake_case")]
pub enum RunClearedOption {
    McuProfile,
    UserJournals,
    ProducerDataProfile,
    ProducerDataFixture,
    LensResponseProfile,
    LensResponseTicks,
    LensContinuationProfile,
}

/// Explicit constructor policy selected by a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunFaceStopProfile {
    #[serde(rename = "declared-armed-face-dma-idle-abort-unmeasured")]
    DeclaredArmedFaceDmaIdleAbortUnmeasured,
}

/// Explicit constructor policy selected by a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RunLensContinuationProfile {
    #[serde(rename = "declared-lens-controller-close-focus-reference-target222-unmeasured")]
    DeclaredLensControllerCloseFocusReferenceTarget222Unmeasured,
}

/// External PCM supplied by the selected run, independent of guest execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum RunAudioInputProfile {
    /// Constant zero, 48 kHz, stereo signed 16-bit PCM.
    DeclaredSilentStereoPcm48000,
}
