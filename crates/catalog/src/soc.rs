//! Hardware parameters shared by application-profile SoC records.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Additional hardware parameters whose meaning depends on the processor family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SocHardware {
    /// Named hardware profiles, selected explicitly by a run preset.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "Record<string, SocProfile>")
    )]
    pub profiles: BTreeMap<String, crate::profiles::SocProfile>,
}

/// Hardware parameters for a SoC block, selected by its model tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "model", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum BlockDevice {
    /// DMA channel count and request-multiplexer connection.
    Dma {
        /// Number of DMA channels.
        channels: u32,
        /// Request-multiplexer register block identity.
        request_mux_block_id: String,
        /// Declared transfer timing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timing: Option<String>,
        /// Other device models this controller depends on.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        native_dependencies: Vec<String>,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// Serial-flash controller's memory-mapped aperture.
    SerialFlash {
        /// Flash aperture block identity.
        aperture_block_id: String,
        /// Aperture bytes used by the device model, when it differs from the block size.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aperture_size: Option<u64>,
        /// Declared command timing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timing: Option<String>,
        /// Other device models this controller depends on.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        native_dependencies: Vec<String>,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// Imaging input buffer's SRAM banks.
    ImagingInputBuffer {
        /// SRAM memory-region identity.
        sram_region_id: String,
        /// Capacity of one SRAM bank.
        bank_bytes: u64,
        /// Ticks for the input buffer reset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reset_ticks: Option<u64>,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// Display-channel pixel packing declared by the controller contract.
    Display {
        /// Picture-plane packing, when specified.
        #[serde(default)]
        picture_layout: Option<PictureLayout>,
        /// Video-plane packing, when specified.
        #[serde(default)]
        video_layout: Option<VideoLayout>,
        /// Pixel-clock ticks per pixel, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pixel_clock_ticks: Option<u64>,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
        /// Frame-law basis, kept separately from the output basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_basis: Option<String>,
        /// Frame period in device ticks, when the block declares the frame law.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        period_ticks: Option<u64>,
        /// Programmed timing banks, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timing: Option<crate::profiles::DisplayTimingBanks>,
        /// Bits a completed output request leaves set.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        set_bits: Vec<crate::profiles::DisplayStatusBits>,
        /// Bits a completed output request leaves clear.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        clear_bits: Vec<crate::profiles::DisplayStatusBits>,
        /// Delay of the bit-16 request handshake.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handshake_ticks: Option<u64>,
        /// OSD layer capture for this output.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scanout: Option<crate::profiles::DisplayScanout>,
        /// Position of this window in the declared display-output order.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order: Option<u32>,
    },
    /// Serial-controller DMA request lines.
    SerialDma {
        /// Receive request identifier.
        receive_request: u32,
        /// Transmit request identifier.
        transmit_request: u32,
    },
    /// DMA interrupt range presented to a companion processor.
    CompanionDma {
        /// First companion interrupt line for the DMA controller.
        companion_irq_base: u32,
    },
    /// ARM GICv2 distributor, which names its CPU interface.
    GicV2 {
        /// CPU-interface register block identity.
        cpu_interface_block_id: String,
        /// Number of implemented interrupt lines.
        lines: u32,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// ARM GICv3 distributor, which names its redistributor block.
    GicV3 {
        /// Redistributor register block identity.
        redistributor_block_id: String,
        /// Distance between adjacent redistributor frames.
        redistributor_stride: u64,
        /// Number of implemented interrupt lines.
        lines: u32,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// Imaging sensor command window.
    SensorCommand {
        /// Ticks from command to completion.
        command_ticks: u64,
        /// Ticks for the sensor command window reset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reset_ticks: Option<u64>,
        /// Declared basis.
        basis: String,
    },
    /// Imaging receiver window.
    ImagingReceiver {
        /// Declared basis.
        basis: String,
    },
    /// Primary control-bus register offsets and modes.
    ControlBus {
        /// Register offsets inside the block.
        offsets: Vec<u64>,
        /// Supported bus modes.
        modes: Vec<u64>,
    },
    /// Secondary control bus with no extra parameters.
    SecondaryBus {},
    /// NAND controller with no extra parameters.
    Nand {},
    /// Loader DMA controller with no extra parameters.
    LoaderDma {},
    /// Streaming decompressor.
    Decompressor {
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// USB core and its fixed interrupt outputs.
    Usb {
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// USB PHY programming port.
    UsbPhy {
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// One SD host clock word.
    SdHostClock {
        /// Ticks for the clock to settle.
        settle_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// GPIO controller.
    Gpio {
        /// Declared basis.
        basis: String,
    },
    /// ADC or temperature conversion block.
    Adc {
        /// Ticks for one conversion.
        sample_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// DMP vector-graphics renderer.
    Dmp {
        /// Declared basis.
        basis: String,
    },
    /// MOVDC imaging channel.
    Movdc {
        /// Ticks for the block reset.
        reset_ticks: u64,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// One control-request gate word.
    ControlRequest {
        /// Ticks for the request acknowledge.
        ack_ticks: u64,
        /// Offset of the request word inside the block.
        request_offset: u64,
        /// Declared basis.
        basis: String,
    },
    /// Control group that owns base parameters, request descriptors and its request word.
    ControlGroup {
        /// Ticks for the request acknowledge.
        ack_ticks: u64,
        /// Offset of the request word inside the block.
        request_offset: u64,
        /// Declared basis.
        basis: String,
    },
    /// GPIO group page with no additional parameters.
    IoGroups {
        /// Declared basis.
        basis: String,
    },
    /// SPI controller.
    Spi {
        /// Parent-clock ticks per transfer unit.
        parent_clock_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// Interval timer.
    IntervalTimer {
        /// Register layout, which selects the implementation.
        layout: crate::profiles::TimerLayout,
        /// Declared clock law.
        clock: String,
        /// Declared basis.
        basis: String,
    },
    /// Generic register block with idle, fixed, command, frame or job behavior.
    RegisterBlock {
        /// Status bits that always read set.
        idle_bits: Vec<crate::profiles::RegisterIdleBits>,
        /// Words that always read a fixed value.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fixed_words: Vec<crate::profiles::RegisterFixedWord>,
        /// Command writes that set a status field.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        commands: Vec<crate::profiles::RegisterBlockCommand>,
        /// Frame law, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frames: Option<BlockRegisterBlockFrames>,
        /// Job law, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job: Option<BlockRegisterBlockJob>,
        /// Declared basis.
        basis: String,
        /// Position in the declared register-block order.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order: Option<u32>,
    },
    /// PL330 DMA controller.
    Pl330 {
        /// Ticks before the first byte moves.
        setup_ticks: u64,
        /// Bytes moved per tick.
        bytes_per_tick: u64,
        /// Declared timing qualification.
        timing: String,
        /// Declared basis.
        basis: String,
    },
    /// One UART port with no additional parameters.
    SerialPort {
        /// Declared basis.
        basis: String,
    },
    /// JPEG encoder or decoder.
    JpegCodec {
        /// Ticks for one run.
        run_ticks: u64,
        /// Block holding the encoder's memory type word, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memory_type_block_id: Option<String>,
        /// Declared basis.
        basis: String,
    },
    /// Face detection engine.
    FaceDetection {
        /// Ticks for one run.
        run_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// Standard SDHCI host page.
    SdHost {
        /// Capabilities register.
        capabilities: u32,
        /// Capabilities 2 register.
        capabilities_2: u32,
        /// Ticks for the clock to settle.
        clock_ticks: u64,
        /// Slot status and version word.
        slot_and_version: u32,
        /// Ticks for one transfer.
        transfer_ticks: u64,
        /// Vendor words that read back their last write.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        vendor_offsets: Vec<u64>,
        /// Declared basis.
        basis: String,
    },
    /// DesignWare USB3 device controller.
    XppUsb {
        /// Ticks for one endpoint command.
        command_ticks: u64,
        /// Ticks for one bus transaction.
        transaction_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// One display output window.
    DisplayOutput {
        /// Bits a completed request leaves set.
        set_bits: Vec<crate::profiles::DisplayStatusBits>,
        /// Bits a completed request leaves clear.
        clear_bits: Vec<crate::profiles::DisplayStatusBits>,
        /// Delay of the bit-16 request handshake.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handshake_ticks: Option<u64>,
        /// OSD layer capture for this output.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scanout: Option<crate::profiles::DisplayScanout>,
        /// Declared basis.
        basis: String,
        /// Position of this window in the declared display-output order.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order: Option<u32>,
    },
    /// Clock bank with PLL lock status.
    PllClocks {
        /// Ticks from enable to lock.
        lock_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// Imaging control page.
    ImagingControl {
        /// Sensor read-out law, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        readout: Option<BlockSensorReadout>,
        /// Declared basis.
        basis: String,
    },
    /// Imaging statistics engine.
    ImagingStatistics {
        /// Frame period in device ticks.
        period_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// HDMI transmitter with no sink.
    HdmiTx {
        /// Declared basis.
        basis: String,
    },
    /// Imaging frame sync and its frame interrupt words.
    FrameSync {
        /// Address of the frame interrupt enable word.
        interrupts: u64,
        /// Frame period in device ticks.
        period_ticks: u64,
        /// Sources every frame latches.
        sources: Vec<BlockFrameSource>,
        /// Declared basis.
        basis: String,
    },
    /// Reset and gate controller.
    ResetGates {
        /// Declared basis.
        basis: String,
    },
    /// DRAM controller.
    DramController {
        /// Declared basis.
        basis: String,
    },
    /// Power controller.
    PowerControl {
        /// Declared basis.
        basis: String,
    },
    /// Byte-oriented I2C master (the Socionext controller Linux drives as i2c-synquacer).
    ByteI2c {
        /// Virtual ticks per address, data byte or STOP.
        byte_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// DesignWare I2C bus.
    I2c {
        /// TX and RX FIFO depth.
        fifo_depth: u64,
        /// The ic_clk the SCL counts divide.
        clock_hz: u64,
        /// Device ticks per second.
        tick_hz: u64,
        /// Declared basis.
        basis: String,
    },
    /// X-Processor Pro SD channel.
    XppSd {
        /// Offset of the SDHCI bank inside the window.
        register_offset: u64,
        /// SD channel of the reset-gate line.
        reset_channel: u64,
        /// Ticks for one transfer.
        transfer_ticks: u64,
        /// Ticks for the clock to settle.
        clock_ticks: u64,
        /// Ticks for the wrapper reset.
        reset_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// X-Processor Pro H.264 encoder.
    H264Encoder {
        /// Ticks for one run.
        run_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// One still-processing engine.
    StillEngine {
        /// Engine contract the block runs.
        engine: StillEngineKind,
        /// Ticks for one job.
        run_ticks: u64,
        /// Evidence for the declared timing, when it is a stand-in.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        evidence: Vec<crate::provenance::Evidence>,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// Filter engine that prepares pictures for the still and face paths.
    FilterEngine {
        /// Ticks for one run.
        run_ticks: u64,
        /// Block holding the source and destination memory type words.
        memory_types_block_id: String,
        /// Evidence for the declared timing, when it is a stand-in.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        evidence: Vec<crate::provenance::Evidence>,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// Still DSP host interface.
    Dsp {
        /// Ticks for one command.
        run_ticks: u64,
        /// Evidence for the declared timing, when it is a stand-in.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        evidence: Vec<crate::provenance::Evidence>,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// Bank of countdown timers, each with its own interrupt line.
    CountdownTimer {
        /// Number of channels.
        channels: u32,
        /// Block holding the count and reload words.
        counter_block_id: String,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// USB cable condition word read by the system init task.
    UsbCondition {
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// XP5 general-purpose timer, optionally with the focus-pulse capture capability.
    Xp5Timer {
        /// Whether the timer captures the body's focus pulse.
        focus_capture: bool,
        /// Declared basis.
        basis: String,
    },
    /// XP5 extended timer bank whose PWM outputs feed input capture.
    Xp5TimerExtended {
        /// Declared basis.
        basis: String,
    },
    /// Serial-flash memory aperture.
    SerialFlashAperture {
        /// Declared basis.
        basis: String,
    },
    /// Serial-flash command controller.
    SerialFlashController {
        /// Declared basis.
        basis: String,
    },
    /// Flash-storage DMA channel.
    FlashStorageDma {
        /// Declared basis.
        basis: String,
    },
    /// Clock or power gate page.
    Gates {
        /// Initial configuration word, when the driver writes one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configuration_word: Option<u64>,
        /// Declared basis.
        basis: String,
    },
    /// Clock-bank page with the processor's clock gates.
    ClockBanks {
        /// Declared basis.
        basis: String,
    },
    /// Panel PLL page.
    PanelPll {
        /// Declared basis.
        basis: String,
    },
    /// General-purpose DMA controller.
    GeneralDma {
        /// First interrupt line presented to the companion core, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        companion_irq_base: Option<u32>,
        /// First interrupt line presented to the application cores, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        a53_irq_base: Option<u32>,
        /// Declared basis.
        basis: String,
    },
    /// Audio DMA channel.
    AudioDma {
        /// Ticks before the first byte moves.
        setup_ticks: u64,
        /// Bytes moved per tick.
        bytes_per_tick: u64,
        /// Declared basis.
        basis: String,
    },
    /// GPIO controller with interrupt inputs and pad behavior.
    Xp5Gpio {
        /// Declared basis.
        basis: String,
    },
    /// ADC with run-supplied samples.
    Xp5Adc {
        /// Declared basis.
        basis: String,
    },
    /// SoC temperature conversion page.
    Temperature {
        /// Declared basis.
        basis: String,
    },
    /// Inter-processor mailbox shared with the companion core.
    InterprocessorMailbox {
        /// Declared basis.
        basis: String,
    },
    /// Audio serial interface and its reset handshake.
    AudioSerial {
        /// Declared basis.
        basis: String,
    },
    /// DDR channel memory-configuration page.
    MemoryConfiguration {
        /// Ticks the channel takes to commit a configuration.
        commit_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// DDR PHY mailbox page.
    DdrPhy {
        /// Declared basis.
        basis: String,
    },
    /// SD interface register set beside an SDHCI host.
    SdInterface {
        /// Declared basis.
        basis: String,
    },
    /// UHS-II capabilities page beside an SDHCI host.
    SdhciUhs2 {
        /// Ticks for the clock to settle.
        clock_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// Display output link's request word.
    DisplayOutputLink {
        /// Declared basis.
        basis: String,
    },
    /// 2D/3D renderer.
    Renderer {
        /// Declared basis.
        basis: String,
    },
    /// Imaging PHY page.
    ImagingPhy {
        /// Declared basis.
        basis: String,
    },
    /// Imaging input page.
    ImagingInput {
        /// Declared basis.
        basis: String,
    },
    /// Imaging resizer.
    ImagingResize {
        /// Declared basis.
        basis: String,
    },
    /// Phase-detection autofocus page.
    ImagingPaf {
        /// Declared basis.
        basis: String,
    },
    /// Preview channel.
    ImagingPreview {
        /// Declared basis.
        basis: String,
    },
    /// RIFP input/output page.
    ImagingRifpIo {
        /// Declared basis.
        basis: String,
    },
    /// RIFP processing page.
    ImagingRifp {
        /// Declared basis.
        basis: String,
    },
    /// Temporal noise reduction stage.
    ImagingTnr {
        /// Declared basis.
        basis: String,
    },
    /// Frame conversion stage.
    ImagingFcnv {
        /// Declared basis.
        basis: String,
    },
    /// Filter and scaler stage.
    ImagingFilterScaler {
        /// Declared basis.
        basis: String,
    },
    /// RZL2 scaler.
    ImagingRzl2 {
        /// Declared basis.
        basis: String,
    },
    /// RESZ scaler page.
    ImagingResz {
        /// Declared basis.
        basis: String,
    },
    /// Input-capture channel.
    InputCapture {
        /// Declared basis.
        basis: String,
    },
    /// UART port.
    Pl011 {
        /// Declared basis.
        basis: String,
    },
    /// PL022 serial peripheral interface.
    Pl022 {
        /// Source clock in hertz.
        source_hz: u64,
        /// Device ticks per second.
        ticks_per_second: u64,
        /// DMA receive request line, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        receive_request: Option<u32>,
        /// DMA transmit request line, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transmit_request: Option<u32>,
        /// Companion interrupt line, when connected.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        companion_irq_line: Option<u32>,
        /// Declared basis.
        basis: String,
    },
    /// Register window whose completion handshake is typed.
    Completion {
        /// Words that always read a fixed value.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fixed_words: Vec<crate::profiles::RegisterFixedWord>,
        /// Start writes and their mask.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        starts: Vec<CompletionStart>,
        /// Writes a completed job leaves.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        completion: Vec<CompletionWrite>,
        /// Acknowledgement behavior.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ack: Option<CompletionAck>,
        /// Interrupt-enable word, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        irq_mask: Option<CompletionIrqMask>,
        /// Ticks from start to completion.
        delay_ticks: u64,
        /// Declared basis.
        basis: String,
    },
    /// Video codec page.
    VideoCodec {
        /// Declared basis.
        basis: String,
    },
    /// Program memory and debug page of the imaging DSP.
    ImagingDsp {
        /// Declared basis.
        basis: String,
    },
    /// Address that only discards reads.
    UnmodeledBusDrain {
        /// Declared basis.
        basis: String,
    },
}

/// One start write of a completion block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CompletionStart {
    /// Offset of the start word inside the block.
    pub offset: u64,
    /// Bits that start a job.
    pub mask: u32,
}

/// One write a completed job leaves behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CompletionWrite {
    /// Offset of the word inside the block.
    pub offset: u64,
    /// Bits the write sets, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<u32>,
    /// Bits the write clears, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear: Option<u32>,
}

/// Acknowledgement behavior of a completion block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CompletionAck {
    /// Offset of the acknowledgement word.
    pub offset: u64,
    /// How a write acknowledges the job.
    pub mode: CompletionAckMode,
    /// Offset of the status word an acknowledgement clears, when different.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_offset: Option<u64>,
    /// Bits the acknowledgement covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits: Option<u32>,
}

/// How a completion block acknowledges a job.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum CompletionAckMode {
    /// Writing one to a bit clears it.
    W1c,
}

/// Interrupt-enable word of a completion block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CompletionIrqMask {
    /// Offset of the enable word.
    pub offset: u64,
    /// Enable bit.
    pub bit: u8,
    /// Whether a set bit masks the interrupt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub set_masks: bool,
}

/// The still-processing engine a block runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum StillEngineKind {
    /// The IIFP engine.
    Iifp,
    /// The RECC engine's control page.
    Recc,
    /// The RESZ resizer.
    Resz,
    /// The RESZL resizer.
    Reszl,
    /// The RETCH engine.
    Retch,
    /// The MIX engine.
    Mix,
}

/// Frame law of a register block, with its interrupt line on the block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BlockRegisterBlockFrames {
    /// Start bit.
    pub start: crate::profiles::RegisterIdleBits,
    /// Bit that reads set while stopped.
    pub stopped: crate::profiles::RegisterIdleBits,
    /// Frame period in device ticks.
    pub period_ticks: u64,
}

/// Job law of a register block, with its interrupt line on the block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BlockRegisterBlockJob {
    /// Start bit.
    pub start: crate::profiles::RegisterIdleBits,
    /// Status bit the job sets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<crate::profiles::RegisterIdleBits>,
    /// Stop bit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<crate::profiles::RegisterIdleBits>,
    /// Device ticks a job takes.
    pub ticks: u64,
}

/// Sensor read-out law, with its interrupt line on the block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BlockSensorReadout {
    /// Declared basis.
    pub basis: String,
    /// Channel offset inside the page.
    pub offset: u64,
    /// Device ticks a read-out takes.
    pub ticks: u64,
    /// The read-out begins on the first frame after its start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub starts_on_frame: bool,
}

/// One frame interrupt source; its line is on the block by output name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BlockFrameSource {
    /// Status and enable bit of the source.
    pub status_bit: u32,
}

impl BlockDevice {
    /// Whether a block of `kind` may carry this device model.
    pub fn allowed_for(&self, kind: crate::reference::SocBlockKind) -> bool {
        use crate::reference::SocBlockKind;
        matches!(
            (kind, self),
            (
                SocBlockKind::Dma,
                BlockDevice::Dma { .. }
                    | BlockDevice::CompanionDma { .. }
                    | BlockDevice::LoaderDma { .. }
                    | BlockDevice::Pl330 { .. }
                    | BlockDevice::GeneralDma { .. }
                    | BlockDevice::AudioDma { .. }
                    | BlockDevice::FlashStorageDma { .. }
            ) | (
                SocBlockKind::SerialController,
                BlockDevice::SerialDma { .. }
                    | BlockDevice::Spi { .. }
                    | BlockDevice::SerialPort { .. }
                    | BlockDevice::Pl022 { .. }
                    | BlockDevice::Pl011 { .. }
            ) | (
                SocBlockKind::InterruptController,
                BlockDevice::GicV2 { .. } | BlockDevice::GicV3 { .. }
            ) | (SocBlockKind::DisplayController, BlockDevice::Display { .. })
                | (
                    SocBlockKind::ImagingController,
                    BlockDevice::ImagingInputBuffer { .. }
                        | BlockDevice::SensorCommand { .. }
                        | BlockDevice::ImagingReceiver { .. }
                        | BlockDevice::Decompressor { .. }
                        | BlockDevice::Movdc { .. }
                        | BlockDevice::JpegCodec { .. }
                        | BlockDevice::FaceDetection { .. }
                        | BlockDevice::ImagingControl { .. }
                        | BlockDevice::ImagingStatistics { .. }
                        | BlockDevice::FrameSync { .. }
                        | BlockDevice::H264Encoder { .. }
                        | BlockDevice::StillEngine { .. }
                        | BlockDevice::FilterEngine { .. }
                        | BlockDevice::Dsp { .. }
                        | BlockDevice::RegisterBlock { .. }
                        | BlockDevice::ImagingPhy { .. }
                        | BlockDevice::ImagingInput { .. }
                        | BlockDevice::ImagingResize { .. }
                        | BlockDevice::ImagingPaf { .. }
                        | BlockDevice::ImagingPreview { .. }
                        | BlockDevice::ImagingRifpIo { .. }
                        | BlockDevice::ImagingRifp { .. }
                        | BlockDevice::ImagingTnr { .. }
                        | BlockDevice::ImagingFcnv { .. }
                        | BlockDevice::ImagingFilterScaler { .. }
                        | BlockDevice::ImagingRzl2 { .. }
                        | BlockDevice::ImagingResz { .. }
                        | BlockDevice::VideoCodec { .. }
                        | BlockDevice::ImagingDsp { .. }
                        | BlockDevice::Completion { .. }
                )
                | (
                    SocBlockKind::AddressWindow,
                    BlockDevice::SerialFlash { .. }
                        | BlockDevice::Decompressor { .. }
                        | BlockDevice::Adc { .. }
                        | BlockDevice::PowerControl { .. }
                        | BlockDevice::UsbCondition { .. }
                        | BlockDevice::RegisterBlock { .. }
                        | BlockDevice::SerialFlashAperture { .. }
                        | BlockDevice::Temperature { .. }
                        | BlockDevice::InterprocessorMailbox { .. }
                        | BlockDevice::InputCapture { .. }
                        | BlockDevice::UnmodeledBusDrain { .. }
                )
                | (SocBlockKind::ControlBus, BlockDevice::ControlBus { .. } | BlockDevice::SecondaryBus { .. } | BlockDevice::ByteI2c { .. } | BlockDevice::I2c { .. } | BlockDevice::RegisterBlock { .. })
                | (
                    SocBlockKind::NandController,
                    BlockDevice::Nand { .. } | BlockDevice::SerialFlashController { .. }
                )
                | (
                    SocBlockKind::UsbController,
                    BlockDevice::Usb { .. }
                        | BlockDevice::UsbPhy { .. }
                        | BlockDevice::XppUsb { .. }
                        | BlockDevice::Completion { .. }
                        | BlockDevice::RegisterBlock { .. }
                )
                | (
                    SocBlockKind::ClockController,
                    BlockDevice::SdHostClock { .. }
                        | BlockDevice::PllClocks { .. }
                        | BlockDevice::Gates { .. }
                        | BlockDevice::ClockBanks { .. }
                        | BlockDevice::PanelPll { .. }
                )
                | (
                    SocBlockKind::Gpio,
                    BlockDevice::Gpio { .. }
                        | BlockDevice::IoGroups { .. }
                        | BlockDevice::Xp5Gpio { .. }
                )
                | (
                    SocBlockKind::Adc,
                    BlockDevice::Adc { .. } | BlockDevice::Xp5Adc { .. }
                )
                | (
                    SocBlockKind::DisplayController,
                    BlockDevice::Dmp { .. }
                        | BlockDevice::DisplayOutput { .. }
                        | BlockDevice::HdmiTx { .. }
                        | BlockDevice::DisplayOutputLink { .. }
                        | BlockDevice::Renderer { .. }
                        | BlockDevice::RegisterBlock { .. }
                )
                | (
                    SocBlockKind::ResetController,
                    BlockDevice::ControlRequest { .. }
                        | BlockDevice::ControlGroup { .. }
                        | BlockDevice::ResetGates { .. }
                )
                | (
                    SocBlockKind::Timer,
                    BlockDevice::IntervalTimer { .. }
                        | BlockDevice::CountdownTimer { .. }
                        | BlockDevice::Xp5Timer { .. }
                        | BlockDevice::Xp5TimerExtended { .. }
                )
                | (
                    SocBlockKind::MemoryController,
                    BlockDevice::RegisterBlock { .. }
                        | BlockDevice::DramController { .. }
                        | BlockDevice::MemoryConfiguration { .. }
                        | BlockDevice::DdrPhy { .. }
                )
                | (
                    SocBlockKind::SdCardController,
                    BlockDevice::SdHost { .. }
                        | BlockDevice::XppSd { .. }
                        | BlockDevice::SdInterface { .. }
                        | BlockDevice::SdhciUhs2 { .. }
                )
                | (
                    SocBlockKind::AudioController,
                    BlockDevice::AudioSerial { .. }
                )
        )
    }
}

/// Picture-plane pixel packing declared by a display controller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum PictureLayout {
    /// Straight-alpha, blue, green and red bytes, in memory order.
    Abgr8888Straight,
}

/// Video-plane pixel packing declared by a display controller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum VideoLayout {
    /// Two luma samples followed by shared chroma samples.
    Yycbcr422,
}
