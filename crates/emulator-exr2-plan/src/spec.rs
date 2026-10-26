use fujimod_emulator_devices::exr2::{clock, gpio, irq, pulse};
use fujimod_emulator_devices::{bus::InterruptLine, graph::Mapping};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootEntry {
    Rom,
    Loader,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema: u32,
    pub boot_entry: BootEntry,
    pub wires: BTreeMap<String, WireKind>,
    pub nodes: Vec<Node>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardPins {
    pub gpio: String,
    pub detect: fujimod_emulator_devices::parts::pins::Pin,
    pub present_high: bool,
    pub protect: fujimod_emulator_devices::parts::pins::Pin,
    pub protected_high: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireKind {
    LogicLevel(bool),
    FramePeriod,
    ImageGrid,
    Focus,
    Exposure,
    Aperture,
    SoftwarePulse,
    Parameters,
    MemoryOutputs,
    Stream,
    OutputChannels,
    IrqRoutes,
    PhaseEdges,
    BridgeRegisters,
    OpticalPosition,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub name: String,
    pub boot_entries: Vec<BootEntry>,
    pub windows: Vec<Mapping>,
    pub dependencies: Vec<String>,
    pub model: Model,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    pub implementation: String,
    pub options: serde_json::Value,
    pub references: BTreeMap<String, String>,
    pub wires: Vec<String>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Card {
    Blank {
        options: serde_json::Value,
    },
    Formatted {
        options: serde_json::Value,
        files: String,
    },
    Retained {
        input: String,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sensor {
    pub options: serde_json::Value,
    pub frames: String,
    pub controls: Option<SensorControls>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SensorControls {
    Xtrans16 {
        options: serde_json::Value,
        calibration: serde_json::Value,
        exposure: String,
        aperture: String,
        projection: Option<serde_json::Value>,
        frame_clock: Option<serde_json::Value>,
    },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SerialEndpoint {
    #[serde(rename = "lens-unit")]
    LensUnit {
        options: serde_json::Value,
        pulses: String,
        gpio: String,
        completion: [InterruptLine; 2],
        position: Option<String>,
        optics: Option<Iris>,
        focus: Option<FocusAxis>,
    },
    Stabilizer {
        options: serde_json::Value,
    },
    Registers {
        options: serde_json::Value,
    },
    Panel {
        options: serde_json::Value,
    },
    CommandDataOutput {
        options: serde_json::Value,
    },
    Fixed {
        options: serde_json::Value,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Iris {
    pub output: String,
    pub positions: Vec<(i64, f64)>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusAxis {
    pub output: String,
    pub infinity_position: i64,
    pub steps_per_mm: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SerialRoute {
    pub options: serde_json::Value,
    pub gpio: String,
    pub endpoint: SerialEndpoint,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftwareOutput {
    pub line: u32,
    pub pulse: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSignal {
    pub gpio: String,
    pub selection: fujimod_emulator_devices::parts::pins::Selection,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Model {
    Power {
        options: super::serial::Options,
        gpio: String,
    },
    Lz {
        options: serde_json::Value,
        base: u64,
        controllers: Vec<String>,
    },
    MemoryDma {
        options: serde_json::Value,
        base: u64,
        routes: String,
    },
    SdhciClocks {
        words: Vec<(u64, u64)>,
    },
    Usb {
        options: serde_json::Value,
        cable: String,
        #[serde(default)]
        endpoints: serde_json::Value,
    },
    Scanout {
        options: serde_json::Value,
    },
    SerialAudioDuplex {
        options: serde_json::Value,
        routes: String,
    },
    SerialAudioReceiver {
        options: serde_json::Value,
        routes: String,
    },
    SerialAudioTransmitter {
        options: serde_json::Value,
        sinks: serde_json::Value,
        routes: String,
    },
    SerialDmaLink {
        options: serde_json::Value,
        input: serde_json::Value,
    },
    XMountLens {
        transport: serde_json::Value,
        configuration: serde_json::Value,
        startup_ticks: u64,
        pins: serde_json::Value,
        gpio: String,
        controllers: Vec<String>,
    },
    SerialDma {
        options: serde_json::Value,
    },
    ImageCombination {
        options: serde_json::Value,
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
    },
    I2c {
        options: serde_json::Value,
        controllers: Vec<String>,
        cable: Option<String>,
        bridge: Option<String>,
    },
    VectorGpu {
        options: serde_json::Value,
        controllers: Vec<String>,
    },
    Nand {
        options: serde_json::Value,
        medium: String,
    },
    ExternalFrames {
        options: serde_json::Value,
        sensor_clock: Option<String>,
        images: String,
        statistics: String,
        activity: Option<InputSignal>,
        position: Option<String>,
        radiometric: Option<serde_json::Value>,
        phase_edges: Option<String>,
        frame_period: Option<String>,
    },
    DisplayTiming {
        options: serde_json::Value,
        clock: String,
        controllers: Vec<String>,
    },
    Dram {
        options: serde_json::Value,
    },
    DramPhy {
        options: serde_json::Value,
    },
    Reset {
        options: serde_json::Value,
    },
    #[serde(rename = "block-98000")]
    Block98000 {
        options: serde_json::Value,
        wiring: serde_json::Value,
    },
    ExrSystemControl {
        options: serde_json::Value,
        wiring: serde_json::Value,
    },
    ExrClocks { options: serde_json::Value },
    ExrMemoryInterface,
    Clocks {
        options: clock::Options,
    },
    #[serde(rename = "exr2-interrupt-controller")]
    InterruptController {
        options: irq::Options,
        core: usize,
        base: u64,
        routes: Option<String>,
        software: Vec<SoftwareOutput>,
    },
    CoreRelease {
        options: serde_json::Value,
        core: usize,
        request: String,
    },
    #[serde(rename = "exr2-system-timers")]
    Timers {
        options: serde_json::Value,
        controllers: Vec<String>,
    },
    Adc {
        options: serde_json::Value,
    },
    Gpio {
        options: gpio::Options,
        irqs: Vec<gpio::InputIrq>,
    },
    ContactActuator {
        options: serde_json::Value,
        gpio: String,
    },
    #[serde(rename = "exr2-pulse")]
    PulseUnit {
        options: pulse::Options,
        irqs: [Option<InterruptLine>; 12],
    },
    SerialChannel {
        options: serde_json::Value,
        gpio: String,
        endpoint: SerialEndpoint,
    },
    SpiController {
        routes: Vec<SerialRoute>,
    },
    GpioContactResponse {
        options: serde_json::Value,
        gpio: String,
    },
    Encoder {
        options: serde_json::Value,
        gpio: String,
        counter: Option<String>,
    },
    #[serde(rename = "i2c-motor-bridge")]
    I2cMotorBridge {
        options: serde_json::Value,
        registers: String,
        pulses: String,
        gpio: String,
        clock_irq: InterruptLine,
    },
    Block86000 {
        options: pulse::block86000::Options,
    },
    Companion {
        options: serde_json::Value,
    },
    Fe80Processing {
        transport: serde_json::Value,
        options: serde_json::Value,
        frames: String,
        routes: String,
    },
    Formatter {
        options: serde_json::Value,
    },
    Hdmi {
        options: serde_json::Value,
        sink: String,
        hot_plug: InterruptLine,
    },
    HdmiSink {
        options: serde_json::Value,
    },
    SerialAudio {
        options: serde_json::Value,
        sinks: serde_json::Value,
    },
    SdHost {
        options: serde_json::Value,
        card: Card,
        pins: Option<CardPins>,
        present: bool,
        protected: bool,
        irq: InterruptLine,
    },
    SdioHost {
        options: serde_json::Value,
        module: serde_json::Value,
        irq: InterruptLine,
    },
    UsbEndpoints {
        options: serde_json::Value,
        irq: InterruptLine,
        dma_irq: InterruptLine,
        ram_bytes: u64,
    },
    ImageInput {
        options: serde_json::Value,
        phase_irqs: [fujimod_emulator_devices::exr2::irq_routes::Irq; 2],
        raw_irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
        readback_irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
        shutter_output: Option<usize>,
        sensor: Option<Sensor>,
        parameters: Option<String>,
        memory_outputs: Option<String>,
        statistics_grid: Option<String>,
        frame_period: Option<String>,
        edges: String,
    },
    OutputChannel {
        options: serde_json::Value,
        base: u64,
        irq: u32,
        channel: usize,
        channels: String,
        routes: String,
        frames: String,
        memory_outputs: Option<String>,
    },
    Statistics {
        options: serde_json::Value,
        routes: String,
        frames: String,
        images: String,
        grid: Option<String>,
    },
    PhaseDetection {
        options: serde_json::Value,
        routes: String,
    },
    WindowAnalysis {
        options: serde_json::Value,
        routes: String,
    },
    Parameters {
        options: serde_json::Value,
        bank: String,
    },
    RawConversion {
        options: serde_json::Value,
    },
    Prefilter {
        options: serde_json::Value,
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
    },
    RawStatistics {
        options: serde_json::Value,
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
    },
    StillProcessor {
        options: serde_json::Value,
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
    },
    Scaler {
        options: serde_json::Value,
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
    },
    Warp {
        options: serde_json::Value,
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
        stream: String,
    },
    ImageTransfer {
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
    },
    AvcCodec {
        options: serde_json::Value,
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
    },
    JpegEncoder {
        options: serde_json::Value,
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
        stream: String,
    },
    JpegDecoder {
        options: serde_json::Value,
        irq: fujimod_emulator_devices::exr2::irq_routes::Irq,
    },
    Block11000 {
        options: serde_json::Value,
    },
    FaceDetector {
        options: serde_json::Value,
    },
    FaceResults {
        profile: serde_json::Value,
        routes: String,
    },
    Adapter {
        adapter: Adapter,
    },
}

impl Model {
    /// The plan's kind tag, which selects the constructor.
    pub fn kind(&self) -> &'static str {
        match self {
            Model::Power { .. } => "power",
            Model::Lz { .. } => "lz",
            Model::MemoryDma { .. } => "memory-dma",
            Model::SdhciClocks { .. } => "sdhci-clocks",
            Model::Usb { .. } => "usb",
            Model::Scanout { .. } => "scanout",
            Model::SerialAudioDuplex { .. } => "serial-audio-duplex",
            Model::SerialAudioReceiver { .. } => "serial-audio-receiver",
            Model::SerialAudioTransmitter { .. } => "serial-audio-transmitter",
            Model::SerialDmaLink { .. } => "serial-dma-link",
            Model::XMountLens { .. } => "x-mount-lens",
            Model::SerialDma { .. } => "serial-dma",
            Model::ImageCombination { .. } => "image-combination",
            Model::I2c { .. } => "i2c",
            Model::VectorGpu { .. } => "vector-gpu",
            Model::Nand { .. } => "nand",
            Model::ExternalFrames { .. } => "external-frames",
            Model::DisplayTiming { .. } => "display-timing",
            Model::Dram { .. } => "dram",
            Model::DramPhy { .. } => "dram-phy",
            Model::Reset { .. } => "reset",
            Model::Block98000 { .. } => "block-98000",
            Model::ExrSystemControl { .. } => "exr-system-control",
            Model::ExrClocks { .. } => "exr-clocks",
            Model::ExrMemoryInterface => "exr-memory-interface",
            Model::Clocks { .. } => "clocks",
            Model::InterruptController { .. } => "exr2-interrupt-controller",
            Model::CoreRelease { .. } => "core-release",
            Model::Timers { .. } => "exr2-system-timers",
            Model::Adc { .. } => "adc",
            Model::Gpio { .. } => "gpio",
            Model::ContactActuator { .. } => "contact-actuator",
            Model::PulseUnit { .. } => "exr2-pulse",
            Model::SerialChannel { .. } => "serial-channel",
            Model::SpiController { .. } => "spi-controller",
            Model::GpioContactResponse { .. } => "gpio-contact-response",
            Model::Encoder { .. } => "encoder",
            Model::I2cMotorBridge { .. } => "i2c-motor-bridge",
            Model::Block86000 { .. } => "block86000",
            Model::Companion { .. } => "companion",
            Model::Fe80Processing { .. } => "fe80-processing",
            Model::Formatter { .. } => "formatter",
            Model::Hdmi { .. } => "hdmi",
            Model::HdmiSink { .. } => "hdmi-sink",
            Model::SerialAudio { .. } => "serial-audio",
            Model::SdHost { .. } => "sd-host",
            Model::SdioHost { .. } => "sdio-host",
            Model::UsbEndpoints { .. } => "usb-endpoints",
            Model::ImageInput { .. } => "image-input",
            Model::OutputChannel { .. } => "output-channel",
            Model::Statistics { .. } => "statistics",
            Model::PhaseDetection { .. } => "phase-detection",
            Model::WindowAnalysis { .. } => "window-analysis",
            Model::Parameters { .. } => "parameters",
            Model::RawConversion { .. } => "raw-conversion",
            Model::Prefilter { .. } => "prefilter",
            Model::RawStatistics { .. } => "raw-statistics",
            Model::StillProcessor { .. } => "still-processor",
            Model::Scaler { .. } => "scaler",
            Model::Warp { .. } => "warp",
            Model::AvcCodec { .. } => "avc-codec",
            Model::ImageTransfer { .. } => "image-transfer",
            Model::JpegEncoder { .. } => "jpeg-encoder",
            Model::JpegDecoder { .. } => "jpeg-decoder",
            Model::Block11000 { .. } => "block11000",
            Model::FaceDetector { .. } => "face-detector",
            Model::FaceResults { .. } => "face-results",
            Model::Adapter { .. } => "adapter",
        }
    }
}

impl Model {
    pub fn node_references(&self) -> Vec<&str> {
        let mut refs = vec![];
        match self {
            Self::Power { gpio, .. } | Self::ContactActuator { gpio, .. } => {
                refs.push(gpio.as_str())
            }
            Self::SdHost {
                pins: Some(pins), ..
            } => refs.push(pins.gpio.as_str()),
            Self::ExternalFrames {
                activity: Some(signal),
                ..
            } => refs.push(signal.gpio.as_str()),
            Self::DisplayTiming {
                clock, controllers, ..
            } => {
                refs.push(clock.as_str());
                refs.extend(controllers.iter().map(String::as_str));
            }
            Self::Timers { controllers, .. }
            | Self::Lz { controllers, .. }
            | Self::VectorGpu { controllers, .. }
            | Self::I2c { controllers, .. } => {
                refs.extend(controllers.iter().map(String::as_str));
            }
            Self::SerialChannel { gpio, endpoint, .. } => {
                refs.push(gpio.as_str());
                if let SerialEndpoint::LensUnit { pulses, gpio, .. } = endpoint {
                    refs.extend([pulses.as_str(), gpio.as_str()]);
                }
            }
            Self::SpiController { routes } => {
                for route in routes {
                    refs.push(route.gpio.as_str());
                    if let SerialEndpoint::LensUnit { pulses, gpio, .. } = &route.endpoint {
                        refs.extend([pulses.as_str(), gpio.as_str()]);
                    }
                }
            }
            Self::XMountLens {
                gpio, controllers, ..
            } => {
                refs.push(gpio.as_str());
                refs.extend(controllers.iter().map(String::as_str));
            }
            Self::GpioContactResponse { gpio, .. } => refs.push(gpio.as_str()),
            Self::Encoder { gpio, counter, .. } => {
                refs.push(gpio.as_str());
                refs.extend(counter.as_deref());
            }
            Self::I2cMotorBridge { pulses, gpio, .. } => {
                refs.extend([pulses.as_str(), gpio.as_str()])
            }
            Self::Hdmi { sink, .. } => refs.push(sink.as_str()),
            Self::Adapter { adapter } => {
                refs.extend(adapter.references.values().map(String::as_str))
            }
            _ => (),
        }
        refs
    }
}
