//! Generated stock diagnostic bulk-read facts for one firmware.
//!
//! The firmware's E1 data request reads memory through a source pointer that a
//! stock routine prepares. The document names that routine's kind, the
//! transfer state it uses, the stock code that must match before any bulk
//! read, probes compared against slow reads, and named capture scopes.
use serde::{Deserialize, Serialize};

pub const BULK_SCHEMA_URI: &str = "https://fujimod.com/schemas/bulk.generated.schema.json";

/// Located bulk-read mechanism and capture scopes for one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareBulk {
    /// Schema identifying the generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// Firmware whose stock code provides this path.
    pub firmware_id: String,
    /// Product name the camera reports.
    pub product: String,
    /// Internal version the camera reports.
    pub internal_version: String,
    /// Uncached RAM alias: added to code-check, mapped-probe and callback-table
    /// addresses that the firmware stores in their cached form.
    pub code_alias: u64,
    /// Stock routine that points E1 reads at memory.
    pub source: BulkSource,
    /// Addresses bulk reads may cover.
    pub window: BulkRange,
    /// Stock code that must match before any bulk read.
    pub code_checks: Vec<CodeCheck>,
    /// Ranges read by both bulk and slow reads and compared.
    pub probes: Vec<BulkRange>,
    /// A larger bulk read of code, at `address + code_alias`, checked against the update's bytes.
    pub mapped_probe: Option<CodeCheck>,
    /// Named sets of ranges captured together.
    pub scopes: Vec<CaptureScope>,
}

/// The stock routine that prepares the E1 source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mechanism", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum BulkSource {
    /// A stock diagnostic query sets the source to `base`; offsets are
    /// relative to it. The callback slot, `callback_offset` bytes into the
    /// table that `callback_table` points to, must hold `stock_callback`.
    Query {
        /// Diagnostic query tag.
        tag: u32,
        /// Address of the pointer to the diagnostic callback table.
        callback_table: u64,
        /// Byte offset of the query callback in that table.
        callback_offset: u64,
        /// Stock callback value.
        stock_callback: u64,
        /// Source base the query sets.
        base: u64,
        /// Address of the 32-bit source pointer and capacity.
        source_state: u64,
        /// Capacities the query may report.
        capacities: Vec<u32>,
        /// Offsets wrap modulo 2^32 rather than only counting upward.
        wrapping: bool,
    },
    /// Stock pattern setup points the 32-bit source at the registered
    /// diagnostic scratch; offsets wrap from its base.
    Pattern {
        /// Address of the registered scratch pointer and capacity.
        registered_buffer: u64,
        /// Address of the 32-bit source pointer and capacity.
        source_state: u64,
        /// Bytes the stock setup writes and each E1 request may return.
        capacity: u32,
    },
    /// Stock pattern setup with a 64-bit source; the source base is then set
    /// to zero so offsets are addresses, and restored after each read.
    PatternZeroBase {
        /// Address of the registered 64-bit scratch pointer and capacity.
        registered_buffer: u64,
        /// Address of the 64-bit source pointer and 32-bit capacity.
        source_state: u64,
        /// Bytes the stock setup writes and each E1 request may return.
        capacity: u32,
    },
}

/// An address range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct BulkRange {
    /// First address.
    pub address: u64,
    /// Length in bytes.
    pub size: u64,
}
impl BulkRange {
    fn end(self) -> Option<u64> {
        self.address.checked_add(self.size)
    }
    /// True when `inner` lies entirely within this range.
    pub fn contains(self, inner: BulkRange) -> bool {
        inner.address >= self.address
            && matches!((inner.end(), self.end()), (Some(a), Some(b)) if a <= b)
    }
}

/// Stock code bytes identified by their SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct CodeCheck {
    /// Routine or region name.
    pub name: String,
    /// First address, before `code_alias`.
    pub address: u64,
    /// Length in bytes.
    pub size: u64,
    /// Lowercase hexadecimal SHA-256 of the stock bytes.
    pub sha256: String,
}

/// Ranges captured together, such as a quick scope while a control is held
/// and a full scope once per session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct CaptureScope {
    /// Scope name.
    pub name: String,
    /// What the scope covers and what is not established about it.
    pub basis: String,
    /// Ranges in capture order.
    pub ranges: Vec<NamedRange>,
}

/// A named address range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct NamedRange {
    /// Range name.
    pub name: String,
    /// First address.
    pub address: u64,
    /// Length in bytes.
    pub size: u64,
}
impl NamedRange {
    pub fn range(&self) -> BulkRange {
        BulkRange {
            address: self.address,
            size: self.size,
        }
    }
}

impl FirmwareBulk {
    /// Validate identity, bounded ranges, digests and scope containment.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let fail = |message: &str| Err(format!("bulk {}: {message}", self.id));
        if self.schema_uri != BULK_SCHEMA_URI || self.schema != 1 {
            return fail("unsupported schema");
        }
        if self.product.trim().is_empty() || self.internal_version.trim().is_empty() {
            return fail("empty camera identity");
        }
        let positive = |range: BulkRange| range.size > 0 && range.end().is_some();
        if !positive(self.window) {
            return fail("empty window");
        }
        match &self.source {
            BulkSource::Query { capacities, .. }
                if capacities.is_empty() || capacities.contains(&0) =>
            {
                return fail("query capacities must be positive");
            }
            BulkSource::Pattern { capacity: 0, .. }
            | BulkSource::PatternZeroBase { capacity: 0, .. } => {
                return fail("pattern capacity must be positive");
            }
            _ => {}
        }
        if self.code_checks.is_empty() || self.probes.is_empty() {
            return fail("stock code checks and probes are required");
        }
        for check in self.code_checks.iter().chain(&self.mapped_probe) {
            let range = BulkRange {
                address: check.address,
                size: check.size,
            };
            if !positive(range)
                || check.sha256.len() != 64
                || !check
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return fail("invalid code check");
            }
        }
        let aliased = self.mapped_probe.as_ref().map(|probe| BulkRange {
            address: probe.address | self.code_alias,
            size: probe.size,
        });
        let scoped = self.scopes.iter().flat_map(|scope| &scope.ranges);
        if !self
            .probes
            .iter()
            .copied()
            .chain(aliased)
            .chain(scoped.map(NamedRange::range))
            .all(|range| positive(range) && self.window.contains(range))
        {
            return fail("probe or scope outside the window");
        }
        if self.scopes.iter().any(|scope| scope.ranges.is_empty()) {
            return fail("empty capture scope");
        }
        Ok(())
    }
}
