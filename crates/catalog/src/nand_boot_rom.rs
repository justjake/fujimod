//! The NAND boot path one XP4 boot ROM image implements, keyed by its SHA-256.
//!
//! The controller's registers belong to the processor's `nand-controller`
//! block. This document holds what the ROM does with them: which boot-source
//! modes load from NAND and with what ECC and address-cycle settings, the
//! header page it reads and its signature, and the program pages it copies.
//! Every XP4 body whose boot ROM resolves to this image shares the record.
use crate::provenance::Evidence;
use serde::{Deserialize, Serialize};

/// Schema identity for a generated NAND boot record.
pub const NAND_BOOT_ROM_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/nand-boot-rom.generated.schema.json";

/// NAND boot behavior of one boot ROM image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct NandBootRom {
    /// Schema identifying this generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// SHA-256 of the boot ROM image this record describes.
    pub rom_sha256: String,
    /// Processor block holding the controller's registers.
    pub controller_block_id: String,
    /// Base of the controller's page-data aperture.
    pub data_window: u64,
    /// Boot-source modes that load from NAND.
    pub source_modes: Vec<NandBootSourceMode>,
    /// The header page the ROM reads first.
    pub header: NandBootHeader,
    /// The program pages the ROM copies after the header.
    pub program: NandBootProgram,
    /// How these facts were established.
    pub evidence: Vec<Evidence>,
}

/// One boot-source mode's NAND settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct NandBootSourceMode {
    /// Boot-source mode the strap selects.
    pub mode: u8,
    /// ECC mode the ROM configures, where zero disables it.
    pub ecc_mode: u8,
    /// Address cycles the ROM presents.
    pub address_cycles: u8,
}

/// The header page and its signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NandBootHeader {
    /// RAM address the header page is read to.
    pub address: u64,
    /// Block the header is read from.
    pub block: u32,
    /// Page the header is read from.
    pub page: u32,
    /// Words the ROM compares before loading the program.
    pub signature: Vec<u32>,
    /// Word index holding the program length in bytes.
    pub length_word: u32,
}

/// The program pages copied after the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct NandBootProgram {
    /// RAM address the program is copied to.
    pub destination: u64,
    /// Bytes one page holds.
    pub page_bytes: u64,
    /// First program page the ROM reads.
    pub first_page: u32,
    /// Mask the page argument wraps with.
    pub page_mask: u32,
}

impl NandBootRom {
    /// Check identity, ranges and declared evidence.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != NAND_BOOT_ROM_SCHEMA_URI {
            return Err("NAND boot schema differs".into());
        }
        if !is_sha256(&self.rom_sha256) {
            return Err("NAND boot record has no ROM SHA-256".into());
        }
        if self.id != format!("nand-boot-{}", self.rom_sha256) {
            return Err(format!(
                "NAND boot identity {} differs from its ROM SHA-256",
                self.id
            ));
        }
        fujimod_catalog_identity::validate_id(&self.controller_block_id)?;
        if self.evidence.is_empty() {
            return Err("NAND boot record carries no evidence".into());
        }
        for value in &self.evidence {
            value.validate()?;
        }
        if self.source_modes.is_empty() {
            return Err("NAND boot record names no boot-source mode".into());
        }
        let mut modes = std::collections::BTreeSet::new();
        for source in &self.source_modes {
            if !modes.insert(source.mode) {
                return Err(format!("NAND boot repeats mode {}", source.mode));
            }
            if source.ecc_mode > 3 || !(1..=7).contains(&source.address_cycles) {
                return Err(format!(
                    "NAND boot mode {} has invalid ECC or address cycles",
                    source.mode
                ));
            }
        }
        let header = &self.header;
        if header.signature.is_empty() || !header.address.is_multiple_of(4) {
            return Err("NAND boot header is invalid".into());
        }
        let program = &self.program;
        if program.page_bytes < 4
            || !program.page_bytes.is_power_of_two()
            || !program.destination.is_multiple_of(4)
            || program.page_mask == 0
        {
            return Err("NAND boot program is invalid".into());
        }
        if !self.data_window.is_multiple_of(4) {
            return Err("NAND boot data window is not word aligned".into());
        }
        Ok(())
    }

    /// The settings a boot-source mode loads with, when it loads from NAND.
    pub fn source_mode(&self, mode: u8) -> Option<&NandBootSourceMode> {
        self.source_modes.iter().find(|source| source.mode == mode)
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
