//! Physical attachments and reusable endpoint protocols for camera assemblies.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::provenance::Evidence;

/// Additional body-specific physical wiring.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BodyHardware {
    /// Named hardware profiles, selected explicitly by a run preset.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "Record<string, BodyProfile>")
    )]
    pub profiles: BTreeMap<String, crate::profiles::BodyProfile>,
    /// Serial endpoints and their processor-side connections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub serial_attachments: Vec<SerialAttachment>,
    /// Connections between GPIO output and input pads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpio_feedback: Vec<GpioFeedback>,
    /// Named keypad positions and their declared analog sample levels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keypad_samples: BTreeMap<String, u16>,
    /// Body connections carrying thermal sensor voltages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thermal_connection_ids: Vec<String>,
    /// The lens mount's serial controller, connector pads and mounted lens, when the body has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_mount: Option<LensMount>,
    /// The sensor-shift stage, its position sensor's capture channel and its coil drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stabilizer: Option<StabilizerMount>,
    /// Addresses the firmware reads to drain the bus past this body's DRAM map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bus_drain: Vec<BusDrain>,
}

/// One discarded read the firmware issues to drain its write buffers: the
/// first address past the body's DRAM map, which only the board knows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BusDrain {
    /// Physical address of the four-byte read; word aligned.
    pub address: u64,
    /// SoC processors whose firmware issues the read.
    pub processor_ids: Vec<String>,
    /// Evidence the address comes from.
    pub basis: String,
}

/// A physical serial endpoint attached to one SoC controller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SerialAttachment {
    /// Installed part identity.
    pub part_id: String,
    /// SoC serial-controller block identity.
    pub controller_id: String,
    /// Connection carrying endpoint data.
    pub connection_id: String,
    /// Connection selecting the endpoint, when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select_connection_id: Option<String>,
}

/// An interchangeable-lens mount: its serial controller and connector pads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct LensMount {
    /// SoC serial controller carrying lens commands.
    pub controller_id: String,
    /// Connection whose pad reads low while a lens is attached.
    pub detect_connection_id: String,
    /// Connections whose pads read the lens-type pattern before power.
    pub sense_connection_ids: [String; 3],
    /// Connections driving the lens supplies, in power-up order.
    pub supply_connection_ids: [String; 2],
    /// Connection whose pad the lens pulls low while busy.
    pub ready_connection_id: String,
    /// Pad function that routes the ready pad to its interrupt lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_function: Option<u32>,
    /// Interrupt line raised when ready falls.
    pub ready_low_line: u32,
    /// Interrupt line raised when ready rises.
    pub ready_high_line: u32,
    /// Lens mounted on the body, when one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_part_id: Option<String>,
}

/// How a sensor-shift stage is wired: its position sensor and its coil drive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct StabilizerMount {
    /// Installed stage part.
    pub part_id: String,
    /// Input-capture block reading the stage's position sensor.
    pub capture_block_id: String,
    /// Capture channel of that block.
    pub capture_channel: u8,
    /// Extended-timer general channels whose PWM drives each axis's coil.
    pub drive_channels: [u8; 3],
}

/// A declared electrical feedback connection between GPIO pads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct GpioFeedback {
    /// Body connection exposing the output pad.
    pub output_connection_id: String,
    /// Body connection exposing the input pad.
    pub input_connection_id: String,
    /// Time from the output rising to the input reading high, when the far end answers late.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rise_ns: Option<u64>,
    /// Whether the far end holds the input high once it has answered, whatever the output does later.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub latch_high: bool,
}

impl BodyHardware {
    /// Validate installed parts and all physical connection identities.
    pub fn validate(&self, body: &crate::reference::Body) -> Result<(), String> {
        for (id, profile) in &self.profiles {
            fujimod_catalog_identity::validate_id(id)?;
            if let Some(graph) = &profile.graph {
                graph.validate()?;
            }
        }
        let connection = |id: &str| {
            body.connections
                .iter()
                .any(|item| item.id == id)
                .then_some(())
                .ok_or_else(|| {
                    format!(
                        "body {} hardware refers to missing connection {id}",
                        body.id
                    )
                })
        };
        for attachment in &self.serial_attachments {
            if !body.parts.contains(&attachment.part_id) {
                return Err(format!(
                    "body {} serial attachment refers to an uninstalled part",
                    body.id
                ));
            }
            connection(&attachment.connection_id)?;
            if let Some(id) = &attachment.select_connection_id {
                connection(id)?;
            }
        }
        if let Some(mount) = &self.lens_mount {
            for id in std::iter::once(&mount.detect_connection_id)
                .chain(&mount.sense_connection_ids)
                .chain(&mount.supply_connection_ids)
                .chain(std::iter::once(&mount.ready_connection_id))
            {
                connection(id)?;
            }
            if let Some(lens) = &mount.lens_part_id {
                if !body.parts.contains(lens) {
                    return Err(format!("body {} mounts an uninstalled lens {lens}", body.id));
                }
            }
        }
        if let Some(stage) = &self.stabilizer {
            if !body.parts.contains(&stage.part_id) {
                return Err(format!("body {} wires an uninstalled stabilizer {}", body.id, stage.part_id));
            }
        }
        for feedback in &self.gpio_feedback {
            connection(&feedback.output_connection_id)?;
            connection(&feedback.input_connection_id)?;
        }
        for id in &self.thermal_connection_ids {
            connection(id)?;
        }
        let mut drain_addresses = std::collections::BTreeSet::new();
        for drain in &self.bus_drain {
            if drain.address % 4 != 0
                || drain.address.checked_add(4).is_none()
                || drain.processor_ids.is_empty()
                || drain.basis.is_empty()
            {
                return Err(format!("body {} has an invalid bus drain", body.id));
            }
            if !drain_addresses.insert(drain.address) {
                return Err(format!("body {} repeats a bus drain address", body.id));
            }
            let mut seen = std::collections::BTreeSet::new();
            for id in &drain.processor_ids {
                fujimod_catalog_identity::validate_id(id)?;
                if !seen.insert(id) {
                    return Err(format!("body {} bus drain repeats processor {id}", body.id));
                }
            }
        }
        Ok(())
    }
}

/// Reusable endpoint protocols with no processor addresses or startup state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum PeripheralPart {
    /// Focus actuator accepting serial commands.
    FocusActuator,
    /// Iris actuator accepting serial commands.
    IrisActuator,
    /// Audio serial output endpoint.
    AudioSerialOutput,
    /// Imaging serial output endpoint.
    ImagingSerialOutput,
    /// Image sensor whose serial register selects the readout gate.
    ImagingSensor {
        /// Sensor register controlling readout.
        read_gate_register: u16,
        /// Bits enabling the readout gate.
        read_gate_mask: u8,
    },
    /// Image sensor with a fast autofocus readout of known line length.
    TimedReadoutSensor {
        /// Value of sensor register 0x01 that selects the fast readout.
        fast_readout: u8,
        /// Length of one line of that readout.
        line_ns: u64,
    },
    /// Full-duplex framed peer whose frame repeats the last command frame it received.
    FramedCommandPeer {
        /// Frame the peer sends before its first command.
        initial_frame: u32,
    },
    /// Sensor-shift stage with a position sensor and voice-coil drive.
    StabilizerStage {
        /// Delay from the segment-end word to the latched capture.
        latency_ns: u64,
        /// Raw sample of each axis at stage position zero.
        center: [u16; 3],
        /// Stage position of each axis at rest, in raw counts from center.
        rest: [i32; 3],
        /// First-order lag of the stage: position moves by (command - position) >> lag_shift.
        lag_shift: u8,
        /// Displacement (raw counts) per PWM count of drive, per axis.
        drive_gain: [i32; 3],
    },
    /// Display panel programmed over a chip-selected serial port.
    DisplayPanelSerial,
    /// Interchangeable X-mount lens on a body's lens mount.
    XMountLens {
        /// Emulated lens preset.
        preset: String,
        /// Lens-type pattern the sense pads read before power.
        sense_unpowered: [bool; 3],
        /// From both supplies high until the lens pulls ready low.
        startup_us: u64,
        /// From then until the lens releases ready high.
        busy_us: u64,
        /// Shortest ready-low pulse for a focus or aperture request.
        operation_us: u64,
    },
    /// Power-controller register protocol.
    PowerController {
        /// Registers this controller answers and what each carries.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        registers: Vec<PowerControllerRegister>,
        /// Transport the controller speaks, when located.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transport: Option<PowerControllerTransport>,
        /// USB port detection this controller answers, when located.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port_detection: Option<PowerPortDetection>,
    },
    /// Serial calibration-register protocol.
    SerialCalibrationRegisters,
    /// BIS configuration-register protocol.
    BisConfigurationRegisters,
    /// LSM6DS configuration-register protocol.
    Lsm6dsConfigurationRegisters,
    /// Serial register file addressed by a command word, of unknown function.
    SerialCommandRegisters {
        /// Word width, 8 or 16 bits.
        bits: u8,
        /// Motorola SPI mode 0..3.
        mode: u8,
        /// Command bits that mark a read, when the command word encodes one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        read_flag: Option<u16>,
        /// Data words one transaction carries at most, when bounded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data_words: Option<u16>,
        /// MISO level while the part is written.
        miso_high: bool,
    },
    /// Serial device not yet identified: it accepts every frame and answers a fixed level.
    UnidentifiedSerialDevice {
        /// MISO level it answers.
        miso_high: bool,
    },
    /// Battery fuel gauge on a control bus.
    FuelGauge,
    /// TMD2772 ambient-light and proximity sensor on a control bus.
    Tmd2772Proximity,
    /// Control-bus device of unknown function whose registers read back their last write.
    RetainedRegisters,
    /// Lens image-stabilizer command and status protocol.
    Stabilizer {
        /// Supported command encodings.
        commands: Vec<StabilizerCommand>,
        /// Status register value exposed by this endpoint.
        status: u32,
        /// Telemetry register address exposed by this endpoint.
        telemetry: u32,
    },
    /// Command/data serial output endpoint.
    CommandDataOutput {
        /// Prefix byte used for command transfers.
        command_prefix: u8,
        /// Prefix byte used for data transfers.
        data_prefix: u8,
        /// Maximum data payload size in bytes.
        payload_bytes: u32,
    },

    /// Serial NOR flash geometry and device identification, when declared.
    SerialNorFlash {
        /// JEDEC identification value, when supplied by the part definition.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        jedec_id: Option<u32>,
        /// Erase-unit capacity in bytes, when declared.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        erase_bytes: Option<u64>,
        /// Program-page capacity in bytes, when declared.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        page_bytes: Option<u64>,
        /// Sample-point window and miss byte of the controller's read states, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sampling: Option<NorSampling>,
    },

    /// Serial NAND flash geometry and device identification, when declared.
    SerialNandFlash {
        /// JEDEC identification value, when supplied by the part definition.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        jedec_id: Option<u32>,
        /// Program-page capacity in bytes, when declared.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        page_bytes: Option<u64>,
        /// Pages per erase block, when declared.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pages_per_block: Option<u64>,
        /// Spare bytes per page, when declared.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        spare_bytes: Option<u64>,
    },
}

/// One register range a power controller answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct PowerControllerRegister {
    /// First register number this role covers.
    pub first: u8,
    /// Consecutive register numbers this role covers.
    #[serde(default = "one_register", skip_serializing_if = "is_one_register")]
    pub count: u8,
    /// What the firmware reads or writes there.
    pub role: PowerControllerRole,
    /// How the role was established.
    pub evidence: Evidence,
}

/// What a power-controller register carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum PowerControllerRole {
    /// Cold-boot reason the loader copies into its handoff block.
    BootReason,
    /// Accessory command selector.
    AccessoryCommand,
    /// Accessory status word.
    AccessoryStatus,
    /// Battery monitor's raw sample.
    BatterySample,
    /// RTC control word.
    RtcControl,
    /// RTC seconds, low word.
    RtcSecondsLow,
    /// RTC seconds, high word.
    RtcSecondsHigh,
    /// RTC seconds staged for a clock load, low word.
    RtcStageLow,
    /// RTC seconds staged for a clock load, high word.
    RtcStageHigh,
    /// One word of the stored settings header.
    SettingsHeader,
    /// USB port-detection request.
    PortDetectionRequest,
    /// USB port-detection result.
    PortDetectionResult,
    /// USB power status word.
    UsbPowerStatus,
}

/// How a power controller exchanges registers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum PowerControllerTransport {
    /// One register selector per exchange over a 24-bit frame.
    DirectRegister,
    /// Four 8-bit frames: register and write flag, two data bytes, checksum.
    ChecksumPowerSpi {
        /// Bits of the first frame carrying the register number.
        register_mask: u8,
        /// Bits of the first frame marking a write.
        write_mask: u8,
        /// Data bytes in each frame, most significant first.
        data_bytes: u8,
        /// Sum the four request bytes reach modulo 256.
        request_sum: u8,
    },
}

/// USB port detection a power controller answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct PowerPortDetection {
    /// Register receiving the request word.
    pub request_register: u8,
    /// Word written to the request register.
    pub request_value: u16,
    /// Register returning the detection result.
    pub result_register: u8,
    /// Bits of the result the firmware tests.
    pub accept_mask: u16,
    /// Result the firmware accepts as a detected port.
    pub accept_value: u16,
}

/// Sample-point window and miss byte of a serial NOR read state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NorSampling {
    /// Lowest sample index that captures data.
    pub sample_first: u8,
    /// Highest sample index that captures data.
    pub sample_last: u8,
    /// Byte the array returns for a sampled miss.
    pub miss_byte: u8,
    /// How the window was established.
    pub evidence: Evidence,
}

fn one_register() -> u8 {
    1
}

fn is_one_register(count: &u8) -> bool {
    *count == 1
}

/// A command accepted by a lens stabilizer endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
pub struct StabilizerCommand {
    /// Command address.
    pub code: u8,
    /// Payload width in bits.
    pub bits: u8,
    /// Command operation.
    pub operation: StabilizerCommandOperation,
}

/// Operation performed by a lens stabilizer command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
pub enum StabilizerCommandOperation {
    /// Write a register value.
    Write,
    /// Select an endpoint bank.
    Select,
    /// Write a register in a named RAM bank.
    RamWrite {
        /// RAM bank selected by the command.
        bank: u8,
    },
}
