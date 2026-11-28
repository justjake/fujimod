//! Exported device construction topology and hardware model parameters.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An explicitly empty device-reference set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionEmpty"))]
pub struct Empty {}

/// Declared selections facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionSelections"))]
pub struct Selections {
    /// Declared choice.
    pub choice: u64,
    /// Declared word.
    pub word: u64,
}

/// Declared wiring facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionWiring"))]
pub struct Wiring {
    /// Declared decoder.
    pub decoder: u64,
    /// Declared selections.
    pub selections: Vec<Selections>,
}

/// Declared software facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionSoftware"))]
pub struct Software {
    /// Declared line.
    pub line: u64,
    /// Declared pulse.
    pub pulse: String,
}

/// Declared core facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionCore"))]
pub struct Core {
    /// Declared core.
    pub core: u64,
    /// Declared line.
    pub line: u64,
}

/// Declared irqs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionIrqs"))]
pub struct Irqs {
    #[serde(rename = "Core")]
    /// Declared Core.
    pub core: Core,
}

/// Declared pin facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionPin"))]
pub struct Pin {
    /// Declared pin.
    pub pin: u64,
    /// Declared port.
    pub port: u64,
}

/// Declared selection facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionSelection"))]
pub struct Selection {
    /// Declared high.
    pub high: bool,
    /// Declared pin.
    pub pin: Pin,
}

/// Declared activity facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionActivity"))]
pub struct Activity {
    /// Declared gpio.
    pub gpio: String,
    /// Declared selection.
    pub selection: Selection,
}

/// Declared irqs irqs facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionIrqsIrqs"))]
pub struct IrqsIrqs {
    /// Declared edge.
    pub edge: String,
    /// Declared outputs.
    pub outputs: Vec<Irqs>,
    /// Declared pin.
    pub pin: Pin,
}

/// Declared touch facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionTouch"))]
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

/// Declared hardware facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionHardware"))]
pub struct Hardware {
    /// Declared base.
    pub base: u64,
    /// Declared irq.
    pub irq: u64,
    /// Declared parts.
    pub parts: Parts,
}

/// One status frame a register write selects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionStatusTransition"))]
pub struct StatusTransition {
    /// Register whose write selects the frame.
    pub register: u64,
    /// Data byte the write carries.
    pub data: u64,
    /// Status frame the write selects.
    pub frame: [u64; 2],
}

/// One register-byte endpoint with declared status replies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionStatusRegisters"))]
pub struct StatusRegisters {
    /// Seven-bit endpoint address.
    pub address: u64,
    /// Registers the endpoint accepts writes to.
    pub writable_registers: Vec<u64>,
    /// Status frame before any transition selects another.
    pub status: [u64; 2],
    /// Writes that select a different status frame.
    pub transitions: Vec<StatusTransition>,
}

/// One command-register endpoint with its declared prefix and register frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionCommandRegisters"))]
pub struct CommandRegisters {
    /// Seven-bit endpoint address.
    pub address: u64,
    /// Commands that open a register transaction.
    pub commands: Vec<u64>,
    /// Bits of the prefix byte that select this endpoint's command bank.
    pub prefix_mask: u64,
    /// Value the prefix bits must carry.
    pub prefix_value: u64,
    /// Register address bits carried by each data byte.
    pub register_mask: u64,
    /// Registers that accept writes and the mask each write keeps.
    pub write_masks: Vec<Vec<u64>>,
    /// Read frames keyed by the register that selects them.
    pub read_frames: Vec<(u64, Vec<u64>)>,
}

/// Declared module facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionModule"))]
pub struct Module {
    /// Declared firmware bytes.
    pub firmware_bytes: u64,
    /// Declared protocol.
    pub protocol: String,
}

/// Declared focus facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionFocus"))]
pub struct Focus {
    /// Declared infinity position.
    pub infinity_position: i64,
    /// Declared output.
    pub output: String,
    /// Declared steps per mm.
    pub steps_per_mm: f64,
}

/// Declared optics facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionOptics"))]
pub struct Optics {
    /// Declared output.
    pub output: String,
    /// Declared positions.
    pub positions: Vec<Vec<f64>>,
}

/// Declared built-in ND filter facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionNdFilter"))]
pub struct NdFilter {
    /// Aperture wire the filter scales.
    pub aperture: String,
    /// Transmission factor while inserted.
    pub factor: f64,
    /// Insert coil mask.
    pub insert_mask: u16,
    /// Register the filter reports through.
    pub register: u16,
    /// Remove coil mask.
    pub remove_mask: u16,
    /// Transmission wire the filter drives.
    pub transmission: String,
}

/// Declared counters facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionCounters"))]
pub struct Counters {
    /// Declared channel.
    pub channel: u64,
    /// Declared divider.
    pub divider: u64,
}

/// Declared home facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionHome"))]
pub struct Home {
    /// Declared pin.
    pub pin: Pin,
    /// Declared side.
    pub side: String,
    /// Declared threshold.
    pub threshold: u64,
}

/// Declared motors facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionMotors"))]
pub struct Motors {
    /// Declared counters.
    pub counters: Vec<Counters>,
    /// Declared home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<Home>,
    /// Declared initial position.
    pub initial_position: u64,
    /// Declared settle ticks.
    pub settle_ticks: u64,
    /// Declared ticks per step.
    pub ticks_per_step: u64,
    /// Declared travel limits.
    pub travel_limits: Vec<i64>,
}

/// Declared lensunit options facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionLensunitOptions"))]
pub struct LensunitOptions {
    /// Declared encoder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    /// Declared motors.
    pub motors: Vec<Motors>,
    /// Declared protocol.
    pub protocol: Protocol,
    /// Declared zoom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<String>,
}

/// Declared panel options facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionPanelOptions"))]
pub struct PanelOptions {
    /// Declared used mask.
    pub used_mask: u64,
}

/// Declared options clock facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionOptionsClock"))]
pub struct OptionsClock {
    /// Declared initial wall-clock seconds since 2000-01-01.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_seconds: Option<u64>,
    /// Declared maximum seconds.
    pub maximum_seconds: u64,
    /// Declared registers.
    pub registers: Vec<u64>,
    /// Declared started.
    pub started: u64,
    /// Declared ticks per second.
    pub ticks_per_second: u64,
}

/// Declared registers options facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionRegistersOptions"))]
pub struct RegistersOptions {
    /// Declared clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<OptionsClock>,
    /// Declared readable.
    pub readable: bool,
    /// Declared registers.
    pub registers: Vec<Vec<u64>>,
    /// Declared status registers.
    pub status_registers: Vec<u64>,
}

/// Declared transports facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionTransports"))]
pub struct Transports {
    /// Declared bits.
    pub bits: u64,
    /// Declared configuration.
    pub configuration: u64,
    /// Declared control.
    pub control: u64,
    /// Declared receive.
    pub receive: bool,
}

/// Declared routes options facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionRoutesOptions"))]
pub struct RoutesOptions {
    /// Declared base.
    pub base: u64,
    /// Declared queue.
    pub queue: String,
    /// Declared selection.
    pub selection: Vec<Selection>,
    /// Declared transfer ticks.
    pub transfer_ticks: u64,
    /// Declared transports.
    pub transports: Vec<Transports>,
}

/// Declared routes facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionRoutes"))]
pub struct Routes {
    /// Declared endpoint.
    pub endpoint: Endpoint,
    /// Declared gpio.
    pub gpio: String,
    /// Declared options.
    pub options: RoutesOptions,
}

/// Declared windows facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionWindows"))]
pub struct Windows {
    /// Declared base.
    pub base: u64,
    /// Declared size.
    pub size: u64,
}

/// Declared nodes facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionNodes"))]
pub struct Nodes {
    /// Declared boot entries.
    pub boot_entries: Vec<String>,
    /// Declared dependencies.
    pub dependencies: Vec<String>,
    /// Declared name.
    pub name: String,
    /// Declared windows.
    pub windows: Vec<Windows>,
    /// Declared model.
    pub model: ConstructionModel,
}

/// Declared construction profile facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConstructionProfile {
    /// Declared nodes.
    pub nodes: Vec<Nodes>,
    /// Declared schema.
    pub schema: u64,
    /// Declared wires.
    pub wires: BTreeMap<String, Wire>,
}

/// One serial audio transmitter sink's declared connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionSerialAudioSink"))]
pub struct SerialAudioSink {
    /// Declared connection state.
    pub connected: bool,
}

/// Closed Parts implementation and its hardware connections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionParts"))]
pub enum Parts {
    /// primary hardware.
    #[serde(rename = "primary")]
    Primary {
        /// commands.
        commands: Vec<CommandRegisters>,
        /// heartbeat register.
        heartbeat_register: u64,
        /// prefix address.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix_address: Option<u64>,
        /// prefix high.
        prefix_high: u64,
        /// sensor address.
        sensor_address: u64,
        /// sensor fields.
        sensor_fields: Vec<u64>,
        /// stationary.
        stationary: bool,
        /// status.
        status: Vec<StatusRegisters>,
        /// topology.
        topology: Vec<u64>,
        /// touch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        touch: Option<Touch>,
    },
    /// secondary hardware.
    #[serde(rename = "secondary")]
    Secondary {
        /// connected.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connected: Option<bool>,
        /// pwic address.
        pwic_address: u64,
        /// pwic masks.
        pwic_masks: Vec<Vec<u64>>,
        /// pwic modes.
        pwic_modes: Vec<u64>,
        /// pwic read prefixes.
        pwic_read_prefixes: Vec<(u64, Vec<u64>)>,
        /// pwic values.
        pwic_values: Vec<Vec<u64>>,
        /// register address.
        register_address: u64,
        /// register masks.
        register_masks: Vec<Vec<u64>>,
        /// register modes.
        register_modes: Vec<u64>,
        /// register values.
        register_values: Vec<Vec<u64>>,
        /// topology.
        topology: Vec<u64>,
    },
}

/// Closed Clock implementation and its hardware connections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionClock"))]
pub enum Clock {
    /// fixed hardware.
    #[serde(rename = "fixed")]
    Fixed,
    /// encoder_stages hardware.
    #[serde(rename = "encoder_stages")]
    EncoderStages {
        /// ticks per quantum.
        ticks_per_quantum: u64,
    },
    /// microstep_stages hardware.
    #[serde(rename = "microstep_stages")]
    MicrostepStages {
        /// ticks per quantum.
        ticks_per_quantum: u64,
    },
}

/// Closed Protocol implementation and its hardware connections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionProtocol"))]
pub enum Protocol {
    /// banked hardware.
    #[serde(rename = "banked")]
    Banked {
        /// clock.
        clock: Vec<Clock>,
        /// positive direction high.
        positive_direction_high: bool,
        /// stop response.
        stop_response: String,
    },
}

/// Closed Endpoint implementation and its hardware connections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionEndpoint"))]
pub enum Endpoint {
    /// lens-unit hardware.
    #[serde(rename = "lens-unit")]
    LensUnit {
        /// completion.
        completion: Vec<Irqs>,
        /// focus.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<Focus>,
        /// gpio.
        gpio: String,
        /// optics.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        optics: Option<Optics>,
        /// ND filter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nd: Option<Box<NdFilter>>,
        /// options.
        options: LensunitOptions,
        /// pulses.
        pulses: String,
    },
    /// panel hardware.
    #[serde(rename = "panel")]
    Panel {
        /// options.
        options: PanelOptions,
    },
    /// registers hardware.
    #[serde(rename = "registers")]
    Registers {
        /// options.
        options: RegistersOptions,
    },
}

/// Explicit H.264 compression implementation, selected before construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum VideoEncoderBackend {
    Videotoolbox,
    Software,
}

/// Compression backend selection; register and DMA behavior is shared.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AvcCodecOptions {
    pub backend: VideoEncoderBackend,
}

/// Closed ConstructionModel implementation and its hardware connections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ConstructionModel {
    /// adc hardware.
    #[serde(rename = "adc")]
    Adc,
    /// block-98000 hardware.
    #[serde(rename = "block-98000")]
    Block98000 {
        /// wiring.
        wiring: Wiring,
    },
    /// block11000 hardware.
    #[serde(rename = "block11000")]
    Block11000,
    /// block86000 hardware.
    #[serde(rename = "block86000")]
    Block86000,
    /// clocks hardware.
    #[serde(rename = "clocks")]
    Clocks,
    /// companion hardware.
    #[serde(rename = "companion")]
    Companion,
    /// core-release hardware.
    #[serde(rename = "core-release")]
    CoreRelease {
        /// core.
        core: u64,
        /// request.
        request: String,
    },
    /// display-timing hardware.
    #[serde(rename = "display-timing")]
    DisplayTiming {
        /// clock.
        clock: String,
        /// controllers.
        controllers: Vec<String>,
    },
    /// dram hardware.
    #[serde(rename = "dram")]
    Dram,
    /// dram-phy hardware.
    #[serde(rename = "dram-phy")]
    DramPhy,
    /// encoder hardware.
    #[serde(rename = "encoder")]
    Encoder {
        /// counter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        counter: Option<String>,
        /// Optional GPIO output connection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gpio: Option<String>,
        /// Optional ADC output connection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        adc: Option<String>,
    },
    /// contact-actuator hardware.
    #[serde(rename = "contact-actuator")]
    ContactActuator {
        /// GPIO output connection.
        gpio: String,
    },
    /// exr2-interrupt-controller hardware.
    #[serde(rename = "exr2-interrupt-controller")]
    Exr2InterruptController {
        /// base.
        base: u64,
        /// core.
        core: u64,
        /// routes.
        routes: String,
        /// software.
        software: Vec<Software>,
    },
    /// exr2-pulse hardware.
    #[serde(rename = "exr2-pulse")]
    Exr2Pulse {
        /// irqs.
        irqs: Vec<Irqs>,
    },
    /// exr2-system-timers hardware.
    #[serde(rename = "exr2-system-timers")]
    Exr2SystemTimers {
        /// controllers.
        controllers: Vec<String>,
    },
    /// external-frames hardware.
    #[serde(rename = "external-frames")]
    ExternalFrames {
        /// activity.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        activity: Option<Activity>,
        /// images.
        images: String,
        /// statistics.
        statistics: String,
    },
    /// face-detector hardware.
    #[serde(rename = "face-detector")]
    FaceDetector,
    /// fe80-processing hardware.
    #[serde(rename = "fe80-processing")]
    Fe80Processing {
        /// frames.
        frames: String,
        /// routes.
        routes: String,
        /// transport.
        transport: Empty,
    },
    /// formatter hardware.
    #[serde(rename = "formatter")]
    Formatter,
    /// gpio hardware.
    #[serde(rename = "gpio")]
    Gpio {
        /// irqs.
        irqs: Vec<IrqsIrqs>,
    },
    /// hdmi hardware.
    #[serde(rename = "hdmi")]
    Hdmi {
        /// hot plug.
        hot_plug: Irqs,
        /// sink.
        sink: String,
    },
    /// hdmi-sink hardware.
    #[serde(rename = "hdmi-sink")]
    HdmiSink,
    /// i2c hardware.
    #[serde(rename = "i2c")]
    I2c {
        /// bridge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bridge: Option<String>,
        /// cable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cable: Option<String>,
        /// controllers.
        controllers: Vec<String>,
        /// hardware.
        hardware: Hardware,
    },
    /// image-combination hardware.
    #[serde(rename = "image-combination")]
    ImageCombination {
        /// irq.
        irq: Core,
    },
    /// image-input hardware.
    #[serde(rename = "image-input")]
    ImageInput {
        /// edges.
        edges: String,
        /// frame period.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_period: Option<String>,
        /// memory outputs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memory_outputs: Option<String>,
        /// parameters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parameters: Option<String>,
        /// phase irqs.
        phase_irqs: Vec<Core>,
        /// raw irq.
        raw_irq: Core,
        /// readback irq.
        readback_irq: Core,
        /// sensor.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sensor: Option<String>,
        /// shutter output.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shutter_output: Option<String>,
        /// statistics grid.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        statistics_grid: Option<String>,
    },
    /// lz hardware.
    #[serde(rename = "lz")]
    Lz {
        /// base.
        base: u64,
        /// controllers.
        controllers: Vec<String>,
    },
    /// memory-dma hardware.
    #[serde(rename = "memory-dma")]
    MemoryDma {
        /// base.
        base: u64,
        /// routes.
        routes: String,
    },
    /// nand hardware.
    #[serde(rename = "nand")]
    Nand {
        /// medium.
        medium: String,
    },
    /// output-channel hardware.
    #[serde(rename = "output-channel")]
    OutputChannel {
        /// base.
        base: u64,
        /// channel.
        channel: u64,
        /// channels.
        channels: String,
        /// frames.
        frames: String,
        /// irq.
        irq: u64,
        /// memory outputs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memory_outputs: Option<String>,
        /// routes.
        routes: String,
    },
    /// H.264 compression, decoding and addressed DMA.
    #[serde(rename = "avc-codec")]
    AvcCodec {
        /// Compression implementation selected by the run configuration.
        options: AvcCodecOptions,
        /// Bitstream completion interrupt.
        irq: Core,
    },
    /// Addressed image transfer registers.
    #[serde(rename = "image-transfer")]
    ImageTransfer {
        /// Completion interrupt.
        irq: Core,
    },
    /// parameters hardware.
    #[serde(rename = "parameters")]
    Parameters {
        /// bank.
        bank: String,
    },
    /// phase-detection hardware.
    #[serde(rename = "phase-detection")]
    PhaseDetection {
        /// routes.
        routes: String,
    },
    /// power hardware.
    #[serde(rename = "power")]
    Power {
        /// gpio.
        gpio: String,
    },
    /// reset hardware.
    #[serde(rename = "reset")]
    Reset,
    /// scanout hardware.
    #[serde(rename = "scanout")]
    Scanout,
    /// sdhci-clocks hardware.
    #[serde(rename = "sdhci-clocks")]
    SdhciClocks {
        /// words.
        words: Vec<Vec<u64>>,
    },
    /// sdio-host hardware.
    #[serde(rename = "sdio-host")]
    SdioHost {
        /// irq.
        irq: Irqs,
        /// module.
        module: Module,
    },
    /// serial-audio hardware.
    #[serde(rename = "serial-audio")]
    SerialAudio {
        /// sinks.
        sinks: Vec<Empty>,
    },
    /// serial-audio-transmitter hardware.
    #[serde(rename = "serial-audio-transmitter")]
    SerialAudioTransmitter {
        /// routes.
        routes: String,
        /// sinks.
        sinks: Vec<SerialAudioSink>,
    },
    /// serial-dma hardware.
    #[serde(rename = "serial-dma")]
    SerialDma,
    /// spi-controller hardware.
    #[serde(rename = "spi-controller")]
    SpiController {
        /// routes.
        routes: Vec<Routes>,
    },
    /// statistics hardware.
    #[serde(rename = "statistics")]
    Statistics {
        /// frames.
        frames: String,
        /// images.
        images: String,
        /// routes.
        routes: String,
    },
    /// usb hardware.
    #[serde(rename = "usb")]
    Usb {
        /// cable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cable: Option<String>,
        /// endpoints.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoints: Option<String>,
    },
    /// vector-gpu hardware.
    #[serde(rename = "vector-gpu")]
    VectorGpu {
        /// controllers.
        controllers: Vec<String>,
    },
}

/// A named signal wire or its declared initial logic level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionWire"))]
pub enum Wire {
    /// Named wire mechanism.
    Signal(WireKind),
    /// Logic-level wire with its declared initial value.
    Logic {
        #[serde(rename = "logic-level")]
        high: bool,
    },
}
/// Closed signal mechanisms used by device graphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", schemars(rename = "ConstructionWireKind"))]
pub enum WireKind {
    /// Software-triggered pulse.
    SoftwarePulse,
    /// Lens aperture output.
    Aperture,
    /// Lens focus output.
    Focus,
    /// Device interrupt routing fabric.
    IrqRoutes,
    /// Image output channels.
    OutputChannels,
    /// Memory image destinations.
    MemoryOutputs,
    /// Image parameter storage.
    Parameters,
    /// Input phase edges.
    PhaseEdges,
}
