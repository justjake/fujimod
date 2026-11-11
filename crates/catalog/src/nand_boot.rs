//! Located EXR II NAND bootloader facts for one firmware.
//!
//! The stage-1 reset vector and blocks come from the update header, the code
//! space is the body's stage-1 loader policy, the ROM card routines and their
//! relocated state come from the registered boot ROM, and the stage-2 range is
//! the first part of the stock network heap. The document holds addresses,
//! sizes and hashes, never firmware bytes.
use serde::{Deserialize, Serialize};

pub const NAND_BOOT_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/sd-boot-v2-autostart.generated.schema.json";

/// NAND bootloader facts for one EXR II firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct FirmwareNandBoot {
    /// Schema identifying this generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// Firmware the facts belong to.
    pub firmware_id: String,
    /// SHA-256 of the update the facts were derived from.
    pub update_sha256: String,
    /// Stage-1 load image and the entry the boot ROM reaches.
    pub stage1: NandBootStage1,
    /// Where the bootloader is placed in stage 1.
    pub code_space: NandBootCodeSpace,
    /// SHA-256 of the boot ROM whose card facts this document uses.
    pub boot_rom: String,
    /// Frozen stage-2 file and RAM range.
    pub load_range: NandBootLoadRange,
}

/// Stage-1 image loaded from NAND and its reset vector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
#[serde(deny_unknown_fields)]
pub struct NandBootStage1 {
    /// Native-flash offset of update byte zero.
    pub dat_bias: u64,
    /// RAM address the ROM loads stage 1 to.
    pub load_address: u64,
    /// Bytes in each stage-1 block.
    pub block_bytes: u64,
    /// Bytes in the complete stage-1 image.
    pub image_bytes: u64,
    /// SHA-256 of the complete stage-1 image.
    pub image_sha256: String,
    /// The blocks the ROM loads, in order.
    pub blocks: Vec<NandBootBlock>,
    /// The reset literal and the code entry behind it.
    pub entry: NandBootEntry,
}

/// One stage-1 NAND block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
#[serde(deny_unknown_fields)]
pub struct NandBootBlock {
    /// Good-block ordinal the ROM counts from the medium start.
    pub ordinal: u32,
    /// Native-flash offset of the block.
    pub native_offset: u64,
    /// Bytes in the block.
    pub bytes: u64,
    /// SHA-256 of the stock block.
    pub sha256: String,
}

/// The reset vector and the first instruction behind its literal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
#[serde(deny_unknown_fields)]
pub struct NandBootEntry {
    /// Byte offset of the reset literal in the stage-1 image.
    pub literal_address: u32,
    /// Stock value at the reset literal.
    pub literal: u32,
    /// First word the ROM branches to.
    pub first_word: u32,
    /// Instruction text of the first word.
    pub instruction: String,
    /// RAM address the literal points to.
    pub code_entry: u64,
    /// First word at the code entry.
    pub code_first_word: u32,
}

/// Stage-1 loader reservation for the NAND bootloader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
#[serde(deny_unknown_fields)]
pub struct NandBootCodeSpace {
    /// RAM address of the reservation.
    pub address: u64,
    /// Bytes available at that address.
    pub capacity: u64,
    /// Native-flash offset of the reservation.
    pub native_offset: u64,
}

/// Frozen stage-2 file, address and size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
#[serde(deny_unknown_fields)]
pub struct NandBootLoadRange {
    /// Root 8.3 file name.
    pub file: String,
    /// Cached RAM address of the first byte.
    pub address: u64,
    /// Maximum file size.
    pub capacity: u64,
    /// Uncached alias of the address, where the body establishes one.
    pub alias: Option<String>,
    /// Base of the stock network heap when it owns this range.
    pub heap_base: Option<u64>,
    /// Bytes in the stock network heap when it owns this range.
    pub heap_bytes: Option<u64>,
    /// Why this range is safe: the stock heap or the frozen EXR II range.
    pub basis: String,
}

impl FirmwareNandBoot {
    /// Validate identity, hashes, block arithmetic and the frozen range.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        if self.schema_uri != NAND_BOOT_SCHEMA_URI
            || self.schema != 1
            || self.update_sha256.len() != 64
            || !self.update_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(format!(
                "nand-boot {}: invalid schema or update hash",
                self.id
            ));
        }
        self.stage1.validate()?;
        self.code_space.validate()?;
        self.load_range.validate()?;
        if self.boot_rom.len() != 64 || !self.boot_rom.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("nand-boot: invalid boot-ROM reference".into());
        }
        let Some(code) = self
            .code_space
            .address
            .checked_sub(self.stage1.load_address)
        else {
            return Err("code space is below the stage-1 load address".into());
        };
        if code >= self.stage1.block_bytes
            || code + self.code_space.capacity > self.stage1.block_bytes
        {
            return Err("code space lies outside the first stage-1 block".into());
        }
        Ok(())
    }
}

impl NandBootStage1 {
    fn validate(&self) -> Result<(), String> {
        if self.block_bytes == 0
            || self.image_bytes == 0
            || !self.image_bytes.is_multiple_of(self.block_bytes)
            || self.blocks.is_empty()
            || self.image_sha256.len() != 64
        {
            return Err("invalid stage-1 image geometry".into());
        }
        if self.blocks.len() as u64 * self.block_bytes != self.image_bytes {
            return Err("stage-1 blocks do not fill the image".into());
        }
        for (index, block) in self.blocks.iter().enumerate() {
            if block.ordinal != 2 + index as u32
                || block.bytes != self.block_bytes
                || block.sha256.len() != 64
            {
                return Err("invalid stage-1 block record".into());
            }
            if index > 0
                && block.native_offset != self.blocks[index - 1].native_offset + self.block_bytes
            {
                return Err("stage-1 blocks are not consecutive".into());
            }
        }
        let entry = &self.entry;
        if entry.instruction.is_empty()
            || !entry.literal_address.is_multiple_of(4)
            || entry.literal_address as u64 + 4 > self.image_bytes
            || entry.code_entry >= self.image_bytes
            || entry.code_first_word >> 24 != 0xeb
        {
            return Err("invalid reset-vector record".into());
        }
        Ok(())
    }
}

impl NandBootCodeSpace {
    fn validate(&self) -> Result<(), String> {
        if self.capacity == 0
            || self.address.checked_add(self.capacity).is_none()
            || self.native_offset.checked_add(self.capacity).is_none()
            || !self.address.is_multiple_of(4)
        {
            return Err("invalid code-space extent".into());
        }
        Ok(())
    }
}

impl NandBootLoadRange {
    fn validate(&self) -> Result<(), String> {
        if self.file.is_empty()
            || self.file.len() > 12
            || !self.file.is_ascii()
            || self.capacity == 0
            || !self.address.is_multiple_of(0x1000)
        {
            return Err("invalid stage-2 file or range".into());
        }
        if self.address.checked_add(self.capacity).is_none() || self.basis.is_empty() {
            return Err("invalid stage-2 range".into());
        }
        match (self.heap_base, self.heap_bytes) {
            (Some(base), Some(bytes)) => {
                if base != self.address || bytes < self.capacity || self.basis != "network-heap" {
                    return Err("stage-2 heap owner disagrees with the range".into());
                }
            }
            (None, None) => {
                if self.basis != "frozen-exr2-range" {
                    return Err("stage-2 range without a heap must state the frozen basis".into());
                }
            }
            _ => return Err("stage-2 heap owner is incomplete".into()),
        }
        Ok(())
    }
}
