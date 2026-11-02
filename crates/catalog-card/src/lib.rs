//! Optional, located card access. A missing document means unavailable.
use fujimod_catalog_bulk::CodeCheck;
use serde::{Deserialize, Serialize};

pub const SCHEMA_URI: &str = "https://fujimod.com/schemas/card-files.generated.schema.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
#[serde(deny_unknown_fields)]
pub struct FirmwareCard {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub firmware_id: String,
    pub product: String,
    pub internal_version: String,
    pub code_checks: Vec<CodeCheck>,
    pub mechanism: Mechanism,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mechanism {
    /// An installed file service, discovered by its FIL1 descriptor. The
    /// descriptor, pointers and build identity are checked on every operation.
    Mailbox {
        search_address: u32,
        search_bytes: u32,
        address_alias: u32,
        build: Option<u32>,
    },
    /// Existing asynchronous file SDK, invoked through a stock dispatcher.
    StockSdk(Box<StockSdk>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
#[serde(deny_unknown_fields)]
pub struct StockSdk {
    pub callback_table_pointer: u32,
    pub callback_offset: u32,
    pub stock_callback: u32,
    pub table_bytes: u32,
    pub table_sha256: String,
    pub query_output: u32,
    /// Optional checked E1 source pair. Each read borrows and restores it.
    pub bulk_source: Option<u32>,
    pub dispatcher: u32,
    pub request: u32,
    pub completion: u32,
    pub status: u32,
    pub value: u32,
    pub error: u32,
    pub pool_id: u32,
    pub pool_begin: u32,
    pub pool_bytes: u32,
    pub allocate: u32,
    pub release: u32,
    pub cache: u32,
    pub sdk_table_cell: u32,
    pub sdk_table: u32,
    pub drive_map: u32,
    pub ui_address: u32,
    pub normal_ui: u32,
    pub open: u32,
    pub read: u32,
    pub write: u32,
    pub close: u32,
    pub seek: u32,
    pub unlink: u32,
    /// The SDK request sender takes a persistent command descriptor in RAM.
    pub send: u32,
    pub list_first_code: u16,
    pub list_next_code: u16,
    pub directory_first_code: u16,
    pub directory_next_code: u16,
    pub receiver_cell: u32,
    pub directory_context_bytes: u32,
    pub directory_attribute_offset: u32,
    pub directory_size_offset: u32,
    pub chunk_bytes: u32,
    pub end_of_directory_status: u32,
}

impl FirmwareCard {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_uri != SCHEMA_URI
            || self.schema != 1
            || self.firmware_id.is_empty()
            || self.product.trim().is_empty()
            || self.internal_version.is_empty()
        {
            return Err("Invalid card capability identity".into());
        }
        for c in &self.code_checks {
            if c.size == 0
                || c.address
                    .checked_add(c.size)
                    .is_none_or(|end| end > 1u64 << 32)
                || c.sha256.len() != 64
                || !c.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("Invalid card code check".into());
            }
        }
        match &self.mechanism {
            Mechanism::Mailbox {
                search_address,
                search_bytes,
                address_alias,
                ..
            } => {
                if search_address % 32 != 0
                    || *search_bytes < 64
                    || *search_bytes > 16 * 1024 * 1024
                    || *address_alias != 0 && !address_alias.is_power_of_two()
                    || search_address & address_alias != 0
                    || u64::from(*search_address | *address_alias) + u64::from(*search_bytes)
                        > 1u64 << 32
                {
                    return Err("Invalid file-service reservation".into());
                }
            }
            Mechanism::StockSdk(p) => {
                if p.bulk_source
                    .is_some_and(|source| source != p.query_output || p.chunk_bytes % 32 != 0)
                {
                    return Err(
                        "E1 card source does not match the checked query output pair".into(),
                    );
                }
                let capacity = u64::from(p.chunk_bytes) + 544;
                if p.chunk_bytes == 0
                    || p.chunk_bytes > 1024 * 1024
                    || u64::from(p.pool_bytes) < capacity
                    || u64::from(p.pool_begin) + u64::from(p.pool_bytes) > u64::from(u32::MAX)
                    || p.table_bytes < 4
                    || p.table_bytes > 4096
                    || u64::from(p.callback_offset) + 4 > u64::from(p.table_bytes)
                    || p.directory_context_bytes < 48
                    || p.directory_context_bytes > 256
                    || p.directory_attribute_offset >= p.directory_context_bytes
                    || u64::from(p.directory_size_offset) + 8 > u64::from(p.directory_context_bytes)
                {
                    return Err("Invalid stock card SDK geometry".into());
                }
                let ranges = [
                    (p.request, 36u64),
                    (p.query_output, 8),
                    (p.status, 4),
                    (p.value, 4),
                    (p.error, 4),
                ];
                for (i, &(a, n)) in ranges.iter().enumerate() {
                    if a % 4 != 0 || u64::from(a) + n > 1u64 << 32 {
                        return Err("Invalid stock card RAM range".into());
                    }
                    for &(b, m) in &ranges[i + 1..] {
                        if u64::from(a) < u64::from(b) + m && u64::from(b) < u64::from(a) + n {
                            return Err("Stock card RAM ranges overlap".into());
                        }
                    }
                    if u64::from(a) < u64::from(p.pool_begin) + u64::from(p.pool_bytes)
                        && u64::from(p.pool_begin) < u64::from(a) + n
                    {
                        return Err("Stock card control range overlaps its pool".into());
                    }
                }
            }
        }
        Ok(())
    }
}
