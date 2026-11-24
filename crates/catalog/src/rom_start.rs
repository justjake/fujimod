//! ROM start facts one firmware declares for the XP4 boot ROM.
//!
//! The boot-source strap window is a processor block. This document holds what
//! the firmware's first stage and loader need from the ROM start: the ROM's
//! identity, the words each boot source presents, the register ranges and fixed
//! words the first stage writes, and the power registers at reset. Each group
//! carries its declared basis and the evidence behind it, because the words are
//! declared inputs and the DDR training results are stand-ins.
use crate::provenance::Evidence;
use serde::{Deserialize, Serialize};

/// Schema identity for a generated ROM start document.
pub const ROM_START_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/rom-start.generated.schema.json";

/// ROM start facts for one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct FirmwareRomStart {
    /// Schema identifying this generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Firmware the facts belong to.
    pub firmware_id: String,
    /// SHA-256 of the update the facts were derived from.
    pub update_sha256: String,
    /// The boot ROM the firmware starts in.
    pub boot_rom: RomStartBootRom,
    /// Boot-source strap window and the word each selection presents.
    pub boot_strap: RomStartStrap,
    /// Register ranges the first stage writes and no device owns.
    pub last_write_banks: Vec<RomStartBank>,
    /// Fixed register words the first stage requires, per block base.
    pub fixed_words: Vec<RomStartWordBlock>,
    /// Power-controller registers at reset.
    pub power_inputs: Vec<RomStartPowerInput>,
}

/// One firmware's boot ROM identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RomStartBootRom {
    /// SHA-256 of the 32 KiB boot ROM image.
    pub sha256: String,
    /// Declared basis.
    pub basis: String,
    /// How the identity was established.
    pub evidence: Vec<Evidence>,
}

/// The boot-source strap and its two selection words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RomStartStrap {
    /// Processor block holding the strap word.
    pub block_id: String,
    /// Word presented when the SD card boots.
    pub sd_card_word: u32,
    /// Word presented when the serial flash boots.
    pub serial_flash_word: u32,
    /// Declared basis.
    pub basis: String,
    /// How the words were established.
    pub evidence: Vec<Evidence>,
}

/// One half-open register range the first stage writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RomStartBank {
    /// First address of the range.
    pub base: u64,
    /// First address past the range.
    pub end: u64,
    /// Declared basis.
    pub basis: String,
    /// How the range was established.
    pub evidence: Vec<Evidence>,
}

/// The fixed words one register block starts with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RomStartWordBlock {
    /// Block base address the words belong to.
    pub base: u64,
    /// The words.
    pub words: Vec<RomStartWord>,
    /// Declared basis.
    pub basis: String,
    /// How the words were established.
    pub evidence: Vec<Evidence>,
}

/// One fixed register word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct RomStartWord {
    /// Word offset within the block.
    pub offset: u64,
    /// Word value.
    pub value: u32,
}

/// One power-controller register at reset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RomStartPowerInput {
    /// Power-controller register number.
    pub register: u8,
    /// Register value.
    pub value: u16,
    /// Declared basis.
    pub basis: String,
    /// How the value was established.
    pub evidence: Vec<Evidence>,
}

impl FirmwareRomStart {
    /// Check identity, ranges and declared evidence.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != ROM_START_SCHEMA_URI {
            return Err("ROM start schema differs".into());
        }
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        fujimod_catalog_identity::validate_id(&self.boot_strap.block_id)?;
        if !is_sha256(&self.boot_rom.sha256) {
            return Err(format!(
                "ROM start {} has no boot ROM SHA-256",
                self.firmware_id
            ));
        }
        evidence("boot_rom", &self.boot_rom.evidence)?;
        evidence("boot_strap", &self.boot_strap.evidence)?;
        if self.boot_strap.sd_card_word == self.boot_strap.serial_flash_word {
            return Err(format!(
                "ROM start {} presents one strap word for both sources",
                self.firmware_id
            ));
        }
        if self.last_write_banks.is_empty() || self.fixed_words.is_empty() {
            return Err(format!(
                "ROM start {} declares no last-write banks or fixed words",
                self.firmware_id
            ));
        }
        for bank in &self.last_write_banks {
            evidence("last_write_banks", &bank.evidence)?;
            if bank.base >= bank.end || bank.base % 4 != 0 || bank.end % 4 != 0 {
                return Err(format!(
                    "ROM start {} has an invalid last-write bank {:#x}..{:#x}",
                    self.firmware_id, bank.base, bank.end
                ));
            }
        }
        let mut bases = std::collections::BTreeSet::new();
        for block in &self.fixed_words {
            evidence("fixed_words", &block.evidence)?;
            if !bases.insert(block.base) {
                return Err(format!(
                    "ROM start {} repeats fixed words for {:#x}",
                    self.firmware_id, block.base
                ));
            }
            if block.words.is_empty() {
                return Err(format!(
                    "ROM start {} declares an empty fixed-word block {:#x}",
                    self.firmware_id, block.base
                ));
            }
            if block.words.iter().any(|word| word.offset % 4 != 0) {
                return Err(format!(
                    "ROM start {} has an unaligned fixed word",
                    self.firmware_id
                ));
            }
        }
        let mut registers = std::collections::BTreeSet::new();
        for input in &self.power_inputs {
            evidence("power_inputs", &input.evidence)?;
            if input.register > 127 {
                return Err(format!(
                    "ROM start {} has an out-of-range power input",
                    self.firmware_id
                ));
            }
            if !registers.insert(input.register) {
                return Err(format!(
                    "ROM start {} repeats power register {}",
                    self.firmware_id, input.register
                ));
            }
        }
        Ok(())
    }
}

fn evidence(group: &str, values: &[Evidence]) -> Result<(), String> {
    if values.is_empty() {
        return Err(format!("ROM start {group} carries no evidence"));
    }
    for value in values {
        value.validate()?;
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
