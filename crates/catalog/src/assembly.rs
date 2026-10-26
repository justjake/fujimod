//! Physical attachments and reusable endpoint protocols for camera assemblies.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    pub serial_attachments: Vec<SerialAttachment>,
    /// Connections between GPIO output and input pads.
    pub gpio_feedback: Vec<GpioFeedback>,
    /// Named keypad positions and their declared analog sample levels.
    pub keypad_samples: BTreeMap<String, u16>,
    /// Body connections carrying thermal sensor voltages.
    pub thermal_connection_ids: Vec<String>,
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
    #[serde(default)]
    pub select_connection_id: Option<String>,
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
        for feedback in &self.gpio_feedback {
            connection(&feedback.output_connection_id)?;
            connection(&feedback.input_connection_id)?;
        }
        for id in &self.thermal_connection_ids {
            connection(id)?;
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
    /// Power-controller register protocol.
    PowerController,
    /// Serial calibration-register protocol.
    SerialCalibrationRegisters,
    /// BIS configuration-register protocol.
    BisConfigurationRegisters,
    /// LSM6DS configuration-register protocol.
    Lsm6dsConfigurationRegisters,
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
        jedec_id: Option<u32>,
        /// Erase-unit capacity in bytes, when declared.
        #[serde(default)]
        erase_bytes: Option<u64>,
        /// Program-page capacity in bytes, when declared.
        #[serde(default)]
        page_bytes: Option<u64>,
    },
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
