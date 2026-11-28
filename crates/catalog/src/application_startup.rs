//! How an X-Processor 4 or X-Processor Pro firmware's application starts.
//!
//! The stock loader places the firmware's images in RAM, fills its
//! boot-information block and enters the kernel on every core. This record
//! says what that start consists of, as facts about the update: each image by
//! its offset in the update, its encoding and the digest of the bytes placed,
//! the entry registers and labels, the boot-information block's words, where
//! the NOR contents start in the update, and the statistics buffers the
//! processor reads. A camera open builds the start from it and the update.
use serde::{Deserialize, Serialize};

pub const APPLICATION_STARTUP_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/application-startup.generated.schema.json";

/// The processor whose board the start builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ApplicationKind {
    Xp4,
    Xpp,
}

/// How an image's bytes are stored in the bit-inverted update.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ImageEncoding {
    /// The placed bytes themselves.
    Raw,
    /// A method-4 LZ stream with its inner header; the placed bytes are the
    /// first `size` decoded bytes.
    Method4,
}

/// One image the stock loader places.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct PlacedImage {
    /// Physical address of the first placed byte.
    pub address: u64,
    /// Bytes placed.
    pub size: u64,
    /// SHA-256 of the placed bytes.
    pub sha256: String,
    /// Offset of the image in the bit-inverted update.
    pub dat_offset: u64,
    pub encoding: ImageEncoding,
}

/// One word of the loader's boot-information block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct HandoffWord {
    pub offset: u64,
    pub value: u32,
}

/// The boot-information block the loader leaves for the kernel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct LoaderHandoff {
    pub address: u64,
    pub size: u64,
    /// The byte every other position holds.
    pub fill: u8,
    pub words: Vec<HandoffWord>,
    /// SHA-256 of the block.
    pub sha256: String,
}

/// Where the NOR contents start in the bit-inverted update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FlashContents {
    pub dat_offset: u64,
    pub size: u64,
    pub sha256: String,
}

/// The application start of one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ApplicationStartup {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub id: String,
    pub firmware_id: String,
    /// SHA-256 of the update the start was located in.
    pub firmware_sha256: String,
    pub kind: ApplicationKind,
    /// Cores that enter the kernel.
    pub cores: u64,
    /// Each core's entry registers, as `[register, value]`.
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "[unknown, number][]")
    )]
    pub registers: Vec<(serde_json::Value, u64)>,
    pub images: Vec<PlacedImage>,
    pub loader_handoff: LoaderHandoff,
    /// The NOR contents, for a body whose processor reads them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash: Option<FlashContents>,
    /// Entry addresses the image declares (X-Processor 4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<u64>,
    /// Statistics buffers the processor reads, by run asset name (X-Processor Pro).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<String>,
}

fn digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|c| c.is_ascii_hexdigit())
}

impl ApplicationStartup {
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let handoff = &self.loader_handoff;
        if self.schema_uri != APPLICATION_STARTUP_SCHEMA_URI
            || self.schema != 1
            || self.id != format!("{}-application-startup", self.firmware_id)
            || !digest(&self.firmware_sha256)
            || self.cores == 0
            || self.registers.is_empty()
            || self
                .images
                .iter()
                .any(|image| image.size == 0 || !digest(&image.sha256))
            || handoff.size == 0
            || !digest(&handoff.sha256)
            || handoff
                .words
                .iter()
                .any(|word| word.offset % 4 != 0 || word.offset + 4 > handoff.size)
            || self.flash.as_ref().is_some_and(|flash| !digest(&flash.sha256))
        {
            return Err(format!(
                "firmware {} application startup is invalid",
                self.firmware_id
            ));
        }
        Ok(())
    }

    /// The boot-information block's bytes.
    pub fn handoff_block(&self) -> Vec<u8> {
        let handoff = &self.loader_handoff;
        let mut block = vec![handoff.fill; handoff.size as usize];
        for word in &handoff.words {
            let offset = word.offset as usize;
            block[offset..offset + 4].copy_from_slice(&word.value.to_le_bytes());
        }
        block
    }
}
