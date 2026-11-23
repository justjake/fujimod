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
    /// Application-processor identification and cache topology, when declared.
    #[serde(default)]
    pub application: Option<ApplicationHardware>,
    /// Hardware interrupt-controller interfaces.
    #[serde(default)]
    pub interrupt_controller: Option<InterruptControllerHardware>,
    /// Companion-processor clock and mailbox connections, when present.
    #[serde(default)]
    pub companion: Option<CompanionHardware>,
    /// The byte every RAM cell holds at power-on, as read on a camera. Absent
    /// when unmeasured; the emulator then starts RAM zeroed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_on_ram_fill: Option<u8>,
}

impl SocHardware {
    /// Check every hardware connection against the containing SoC's identities.
    pub fn validate(&self, soc: &crate::reference::Soc) -> Result<(), String> {
        let block = |id: &str| {
            soc.blocks
                .iter()
                .any(|item| item.id == id)
                .then_some(())
                .ok_or_else(|| format!("SoC {} hardware refers to missing block {id}", soc.id))
        };
        let core = |id: &str| {
            soc.processors
                .iter()
                .any(|item| item.id == id)
                .then_some(())
                .ok_or_else(|| format!("SoC {} hardware refers to missing processor {id}", soc.id))
        };
        let clock = |id: &str| {
            soc.clocks
                .iter()
                .any(|item| item.id == id)
                .then_some(())
                .ok_or_else(|| format!("SoC {} hardware refers to missing clock {id}", soc.id))
        };
        if let Some(ApplicationHardware::Armv8a {
            affinities,
            counter_clock_id,
            pmu_clock_id,
            ..
        }) = &self.application
        {
            let mut values = std::collections::BTreeSet::new();
            for (id, affinity) in affinities {
                core(id)?;
                if !values.insert(affinity) {
                    return Err(format!("SoC {} repeats a processor affinity", soc.id));
                }
            }
            clock(counter_clock_id)?;
            clock(pmu_clock_id)?;
        }
        if let Some(controller) = &self.interrupt_controller {
            let (distributor, interface, lines) = match controller {
                InterruptControllerHardware::GicV2 {
                    distributor_block_id,
                    cpu_interface_block_id,
                    lines,
                    ..
                } => (distributor_block_id, cpu_interface_block_id, lines),
                InterruptControllerHardware::GicV3 {
                    distributor_block_id,
                    redistributor_block_id,
                    lines,
                    ..
                } => (distributor_block_id, redistributor_block_id, lines),
            };
            block(distributor)?;
            block(interface)?;
            if *lines == 0 {
                return Err(format!("SoC {} interrupt controller has no lines", soc.id));
            }
        }
        if let Some(companion) = &self.companion {
            core(&companion.processor_id)?;
            clock(&companion.clock_id)?;
            block(&companion.mailbox_block_id)?;
        }
        Ok(())
    }
}

/// Family-specific application-processor identification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ApplicationHardware {
    /// ARMv7-A configuration base and cache capacities.
    Armv7a {
        /// Configuration base address presented to each application core.
        configuration_base: u64,
        /// Declared cache capacities.
        caches: CacheGeometry,
    },
    /// ARMv8-A processor identity, affinities and architectural timer wiring.
    Armv8a {
        /// MIDR_EL1 processor identification value.
        midr_el1: u64,
        /// MPIDR_EL1 affinity values, keyed by processor identity.
        affinities: BTreeMap<String, u64>,
        /// Clock domain supplying the architectural counter.
        counter_clock_id: String,
        /// Clock domain supplying the performance monitor.
        pmu_clock_id: String,
        /// Physical architectural timer interrupt line.
        physical_timer_irq: u32,
        /// Virtual architectural timer interrupt line.
        virtual_timer_irq: u32,
    },
}

/// Cache capacities declared for an application processor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CacheGeometry {
    /// Per-core level-one data-cache capacity.
    pub l1_data_bytes: u64,
    /// Per-core level-one instruction-cache capacity.
    pub l1_instruction_bytes: u64,
    /// Shared level-two cache capacity.
    pub l2_bytes: u64,
}

/// ARM generic interrupt-controller interfaces.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum InterruptControllerHardware {
    /// GICv2 distributor and CPU interface.
    GicV2 {
        /// Distributor register block identity.
        distributor_block_id: String,
        /// CPU-interface register block identity.
        cpu_interface_block_id: String,
        /// Number of implemented interrupt lines.
        lines: u32,
        /// Declared basis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<String>,
    },
    /// GICv3 distributor and per-core redistributors.
    GicV3 {
        /// Distributor register block identity.
        distributor_block_id: String,
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
}

/// A companion processor's hardware clock and mailbox endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CompanionHardware {
    /// Companion processor identity in the SoC processor list.
    pub processor_id: String,
    /// Clock domain supplying the companion processor.
    pub clock_id: String,
    /// Vector-table address presented to the companion after reset.
    pub vector_base: u64,
    /// SoC mailbox block identity.
    pub mailbox_block_id: String,
    /// Mailbox indices used by the companion link.
    pub mailbox: CompanionMailbox,
}

/// Mailbox indices used by a companion processor link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CompanionMailbox {
    /// Request mailbox index.
    pub request: u32,
    /// Control mailbox index.
    pub control: u32,
    /// Reply mailbox index.
    pub reply: u32,
    /// Status mailbox index.
    pub status: u32,
    /// Fault mailbox index.
    pub fault: u32,
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
    },
    /// One control-request gate word.
    ControlRequest {
        /// Ticks for the request acknowledge.
        ack_ticks: u64,
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
}

/// Frame law of a register block, with its interrupt line on the block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
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
            ) | (
                SocBlockKind::SerialController,
                BlockDevice::SerialDma { .. } | BlockDevice::Spi { .. } | BlockDevice::SerialPort { .. }
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
                        | BlockDevice::RegisterBlock { .. }
                )
                | (
                    SocBlockKind::AddressWindow,
                    BlockDevice::SerialFlash { .. }
                        | BlockDevice::Decompressor { .. }
                        | BlockDevice::Adc { .. }
                        | BlockDevice::PowerControl { .. }
                        | BlockDevice::RegisterBlock { .. }
                )
                | (SocBlockKind::ControlBus, BlockDevice::ControlBus { .. } | BlockDevice::SecondaryBus { .. } | BlockDevice::I2c { .. })
                | (SocBlockKind::NandController, BlockDevice::Nand { .. })
                | (SocBlockKind::UsbController, BlockDevice::Usb { .. } | BlockDevice::UsbPhy { .. } | BlockDevice::XppUsb { .. })
                | (SocBlockKind::ClockController, BlockDevice::SdHostClock { .. } | BlockDevice::PllClocks { .. })
                | (SocBlockKind::Gpio, BlockDevice::Gpio { .. } | BlockDevice::IoGroups { .. })
                | (SocBlockKind::Adc, BlockDevice::Adc { .. })
                | (SocBlockKind::DisplayController, BlockDevice::Dmp { .. } | BlockDevice::DisplayOutput { .. } | BlockDevice::HdmiTx { .. })
                | (SocBlockKind::ResetController, BlockDevice::ControlRequest { .. } | BlockDevice::ResetGates { .. })
                | (SocBlockKind::Timer, BlockDevice::IntervalTimer { .. })
                | (SocBlockKind::MemoryController, BlockDevice::RegisterBlock { .. } | BlockDevice::DramController { .. })
                | (SocBlockKind::SdCardController, BlockDevice::SdHost { .. } | BlockDevice::XppSd { .. })
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
