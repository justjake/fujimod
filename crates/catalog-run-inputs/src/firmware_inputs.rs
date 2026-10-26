//! Identity checks and declared nonvolatile input construction for a run.
use serde::{Deserialize, Serialize};

pub const RUN_FIRMWARE_INPUTS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-firmware-inputs.generated.schema.json";

/// A caller-supplied recipe with no embedded firmware contents or input paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunFirmwareInputs {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub id: String,
    pub firmware_id: String,
    pub source_sha256: String,
    pub firmware: UsbFirmwareChecks,
    pub nand_inputs: Vec<NandByteInput>,
    /// Named camera settings written into the firmware's settings bank before boot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<RunSettingsInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<UsbManifestSelection>,
    pub limits: Vec<String>,
}

/// The USB firmware entry points whose immutable bytes qualify the input recipe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct UsbFirmwareChecks {
    pub usb_irq_wiring: UsbIrqChecks,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct UsbIrqChecks {
    pub views: Vec<RunSourceCheck>,
}

/// A named and hashed immutable source range; contains no source bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunSourceCheck {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub sha256: String,
    pub sampling: RunSourceSampling,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum RunSourceSampling {
    ImmutableSource,
}

/// A checked synthetic NAND byte input, never a RAM patch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NandByteInput {
    pub offset: u64,
    pub expected: u8,
    pub value: u8,
    pub basis: String,
}

/// Settings requested by name; the firmware's catalog settings layout encodes them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunSettingsInput {
    /// The scene NAND that holds the settings bank.
    pub nand: String,
    /// Setting name to value name, such as `debug: on`.
    pub requested: std::collections::BTreeMap<String, String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UsbManifestSelection {
    Exr2 { node: String },
}

impl RunFirmwareInputs {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != RUN_FIRMWARE_INPUTS_SCHEMA_URI {
            return Err("run firmware inputs schema differs".into());
        }
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let digest =
            |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !digest(&self.source_sha256) {
            return Err("run firmware source identity is invalid".into());
        }
        if let Some(settings) = &self.settings {
            fujimod_catalog_identity::validate_id(&settings.nand)?;
            if settings
                .requested
                .iter()
                .any(|(name, value)| name.is_empty() || value.is_empty())
            {
                return Err("run settings name an empty setting or value".into());
            }
        }
        for check in &self.firmware.usb_irq_wiring.views {
            if check.size == 0
                || !digest(&check.sha256)
                || check.address.checked_add(check.size).is_none()
            {
                return Err("run source check is empty or has an invalid identity".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_recipes_refuse_empty_settings_and_bad_source_identities() {
        let value = serde_json::json!({
            "$schema": RUN_FIRMWARE_INPUTS_SCHEMA_URI, "schema": 1, "id": "test-usb-inputs",
            "firmware_id": "test-100", "source_sha256": "0".repeat(64),
            "firmware": {"usb_irq_wiring": {"views": []}}, "nand_inputs": [], "limits": [],
            "settings": {"nand": "nand", "requested": {"debug": "on"}}
        });
        let mut inputs: RunFirmwareInputs = serde_json::from_value(value).unwrap();
        assert!(inputs.validate().is_ok());
        inputs.settings.as_mut().unwrap().requested.insert("language".into(), String::new());
        assert!(inputs.validate().is_err());
        inputs.settings.as_mut().unwrap().requested.remove("language");
        inputs.source_sha256 = "invalid".into();
        assert!(inputs.validate().is_err());
    }
}

/// Caller-owned locations for optional factory inputs, kept outside run policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunFactoryInputs {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub id: String,
    pub firmware_id: String,
    pub source_sha256: String,
    pub producer_data_fixture: String,
}

pub const RUN_FACTORY_INPUTS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-factory-inputs.generated.schema.json";
