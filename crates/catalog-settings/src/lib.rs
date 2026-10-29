//! Generated settings layout for one firmware.
//!
//! Stock firmware loads its settings from a bank of flash blocks at startup,
//! and its user profile from a header the power controller keeps and from
//! journal records in flash. X-Processor Pro firmware keeps no such bank: it
//! copies a configuration window of the update into its erased configuration
//! store, and the profile initialization reads that copy instead. The document locates the bank, the stock defaults
//! tables in the update, the fields every valid bank and record carries, and
//! the named settings a run may request. It holds addresses, sizes and hashes;
//! bank and record bytes are built from the update.
use serde::{Deserialize, Serialize};

pub mod persistent_inputs;
use std::collections::{BTreeMap, BTreeSet};

pub const SETTINGS_SCHEMA_URI: &str = "https://fujimod.com/schemas/settings.generated.schema.json";

/// Located settings bank and named settings for one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareSettings {
    /// Schema identifying the generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// Firmware whose stock loader reads this bank.
    pub firmware_id: String,
    /// The decoded update section holding the bank descriptor and defaults table.
    pub source: SettingsSource,
    /// Where the bank lives in flash, when the firmware keeps one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bank: Option<SettingsBank>,
    /// The update window the firmware copies into its erased configuration
    /// store, for firmware without a bank. Derived profile fields read it as
    /// the `bank` store; it is never placed in flash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<ConfigurationWindow>,
    /// Stock default values applied to an otherwise zero bank, when the
    /// firmware has a defaults table.
    pub defaults: Option<UpdateRange>,
    /// Bytes every valid bank carries: signature, identity, body hardware values.
    pub fields: Vec<SettingsField>,
    /// The stock user profile, when located.
    pub profile: Option<SettingsProfile>,
    /// Settings a run may request by name, each with its named values.
    pub settings: BTreeMap<String, NamedSetting>,
    /// Parts of the bank the firmware reads, which a camera's NAND files supply
    /// by name (`nand-<name>.bin.zst`). Empty when not located.
    pub sections: Vec<SettingsSection>,
}

/// One part of the bank the stock loader reads into its configuration RAM.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SettingsSection {
    /// File name part, such as `configuration`.
    pub name: String,
    /// Byte offset in the bank.
    pub offset: u64,
    /// How many bytes the loader reads.
    pub length: SectionLength,
    /// The loader that reads it.
    pub basis: String,
}

/// A section's length: fixed, or counted from bytes the bank itself holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum SectionLength {
    /// Always this many bytes.
    Fixed {
        /// Byte count.
        bytes: u64,
    },
    /// Two little-endian 24-bit entry counts at `counts` and `counts + 3`,
    /// `bits` per entry: the first list rounded up to 64-bit words, the
    /// second to bytes.
    Counted {
        /// Bank offset of the first count.
        counts: u64,
        /// Bits per entry.
        bits: u32,
    },
    /// One little-endian 24-bit entry count at `count`, `bits` per entry,
    /// rounded up to bytes.
    CountedList {
        /// Bank offset of the count.
        count: u64,
        /// Bits per entry.
        bits: u32,
    },
}

impl SectionLength {
    /// The byte count for a bank whose counts are already in place.
    pub fn bytes(&self, bank: &[u8]) -> Result<u64, String> {
        let count = |offset: u64| -> Result<u64, String> {
            let at = usize::try_from(offset).map_err(|_| "count offset too large")?;
            let b = bank
                .get(at..at + 3)
                .ok_or("section counts outside the bank")?;
            Ok(u64::from(b[0]) | u64::from(b[1]) << 8 | u64::from(b[2]) << 16)
        };
        match self {
            Self::Fixed { bytes } => Ok(*bytes),
            Self::Counted { counts, bits } => {
                let bits = u64::from(*bits);
                Ok(((count(*counts)? * bits + 63) & !63) / 8
                    + (count(counts + 3)? * bits).div_ceil(8))
            }
            Self::CountedList { count: at, bits } => {
                Ok((count(*at)? * u64::from(*bits)).div_ceil(8))
            }
        }
    }
}

/// The stock user profile: a sixteen-byte header kept by the power controller
/// and journal records in managed flash blocks. Firmware that finds no valid
/// header and records resets the profile to its defaults at startup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SettingsProfile {
    /// Stock header defaults. Byte 14 is the marker `0xaa`; byte 15 holds the
    /// checksum that makes the sixteen bytes sum to zero.
    pub header: UpdateRange,
    /// Journal records the stock loader requires, by name.
    pub records: BTreeMap<String, ProfileRecord>,
    /// Fields initialized from the calibration bank or a declared fill.
    #[serde(default)]
    pub derived_fields: Vec<ProfileField>,
}

/// A stock initialization of a profile field, applied before requested settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct ProfileField {
    /// A profile record name or `header`.
    pub store: String,
    /// Byte offset in that store.
    pub offset: u64,
    /// Field width; mapped sources require exactly one byte.
    pub bytes: u64,
    pub source: ProfileFieldSource,
    /// The stock initialization that establishes this field.
    pub basis: String,
}

/// A bounded fill, bank copy, or masked byte mapping used by stock initialization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object)
)]
pub enum ProfileFieldSource {
    /// Repeat a byte across the field, including erased padding.
    Fill { value: u8 },
    /// Copy an addressed bank window verbatim.
    BankCopy { offset: u64 },
    /// Replace only masked destination bits with the corresponding bank bits.
    BankBits { offset: u64, mask: u8 },
    /// Decode one bank byte and replace only the masked destination bits.
    BankMap {
        offset: u64,
        mask: u8,
        values: Vec<BankByteValue>,
        /// Fallback when no input entry matches.
        otherwise: u8,
        /// Optional signed-byte fallback for unmatched inputs with bit 7 set.
        #[serde(default)]
        otherwise_high: Option<u8>,
    },
}

/// One input/output pair in a bank-byte decoding table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct BankByteValue {
    pub input: u8,
    pub output: u8,
}

/// Bytes of the installed flash image that stock firmware copies into its
/// configuration store when that store is erased.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct ConfigurationWindow {
    /// Offset of the window in the installed flash image.
    pub flash_offset: u64,
    /// Window length in bytes.
    pub bytes: u64,
    /// Lowercase hexadecimal SHA-256 of the window.
    pub sha256: String,
    /// The stock copy that fills the configuration store from this window.
    pub basis: String,
}

/// One journal record: stock defaults and fixed bytes, sealed with one parity
/// bit per data word and an XOR checksum of the data words in its last four bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct ProfileRecord {
    /// Physical flash block holding the record at its start: a NAND block, or
    /// a 64 KiB region unit of a NOR array.
    pub block: u32,
    /// Managed logical index of that block; NOR arrays have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_index: Option<u16>,
    /// Record length in bytes.
    pub bytes: u64,
    /// Stock defaults copied into the record.
    pub defaults: UpdateRange,
    /// Record offset of the defaults.
    pub defaults_offset: u64,
    /// Record offset of the parity bitmap; the data words precede it.
    pub parity_offset: u64,
    /// Bytes every valid record carries outside the defaults.
    pub fields: Vec<SettingsField>,
}

/// A method-4 encoded section of the installed flash image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct SettingsSource {
    /// Offset of the encoded stream in the installed flash image.
    pub flash_offset: u64,
    /// Firmware address of the first decoded byte.
    pub address: u64,
    /// Decoded length in bytes.
    pub size: u64,
    /// Lowercase hexadecimal SHA-256 of the decoded section.
    pub sha256: String,
}

/// The flash blocks holding the settings bank.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct SettingsBank {
    /// The stock bank descriptor `(kind, blocks, first block)` in the update.
    pub descriptor: UpdateRange,
    /// Descriptor kind word.
    pub kind: u32,
    /// Number of flash blocks in the bank.
    pub blocks: u32,
    /// First physical flash block.
    pub first_block: u32,
    /// Managed logical index of the first block.
    pub logical_first: u16,
    /// Bank content length in bytes.
    pub bytes: u64,
}

/// Bytes at a firmware address in the decoded update, identified by SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct UpdateRange {
    /// Firmware address.
    pub address: u64,
    /// Length in bytes.
    pub size: u64,
    /// Lowercase hexadecimal SHA-256 of the stock bytes.
    pub sha256: String,
}

/// Bytes written at a bank or record offset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct SettingsField {
    /// Byte offset in the bank or record.
    pub offset: u64,
    /// Bytes written there; at most 128.
    pub bytes: Vec<u8>,
    /// What the bytes are and where they come from.
    pub basis: String,
}

/// A setting a run requests by name, such as `debug` or `language`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NamedSetting {
    /// Where the setting is stored: `bank`, `header`, or a profile record name.
    pub store: String,
    /// Byte offset in that store.
    pub offset: u64,
    /// Encoded bytes for each value name; all the same length.
    pub values: BTreeMap<String, Vec<u8>>,
}

impl FirmwareSettings {
    /// Validate identity, store extents and field bounds.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let fail = |message: &str| Err(format!("settings {}: {message}", self.id));
        if self.schema_uri != SETTINGS_SCHEMA_URI || self.schema != 1 {
            return fail("unsupported schema");
        }
        let bank_bytes = match (&self.bank, &self.configuration) {
            (Some(bank), None) => {
                if bank.blocks == 0 || bank.bytes == 0 || bank.descriptor.size != 12 {
                    return fail("empty bank or descriptor");
                }
                bank.bytes
            }
            (None, Some(window)) => {
                if window.bytes == 0
                    || window.flash_offset.checked_add(window.bytes).is_none()
                    || window.basis.is_empty()
                    || !hex_digest(&window.sha256)
                    || self.defaults.is_some()
                    || !self.fields.is_empty()
                    || !self.sections.is_empty()
                    || self
                        .settings
                        .values()
                        .any(|setting| setting.store == "bank")
                {
                    return fail("configuration window with bank contents or settings");
                }
                window.bytes
            }
            _ => return fail("settings need exactly one bank or configuration window"),
        };
        let source_end = self.source.address.checked_add(self.source.size);
        let profile = self.profile.as_ref();
        let ranges = self
            .bank
            .iter()
            .map(|bank| &bank.descriptor)
            .chain(&self.defaults)
            .chain(profile.map(|profile| &profile.header))
            .chain(
                profile
                    .into_iter()
                    .flat_map(|p| p.records.values().map(|r| &r.defaults)),
            );
        for range in ranges {
            if range.address < self.source.address
                || range
                    .address
                    .checked_add(range.size)
                    .zip(source_end)
                    .is_none_or(|(end, limit)| end > limit)
            {
                return fail("stock range outside the source section");
            }
            if range.size == 0
                || range.address.checked_add(range.size).is_none()
                || !hex_digest(&range.sha256)
            {
                return fail("invalid update range");
            }
        }
        if self
            .defaults
            .as_ref()
            .is_some_and(|table| table.size % 4 != 0)
        {
            return fail("defaults table is not whole entries");
        }
        // Store extents, each with the offsets its fields and settings occupy.
        let mut stores: BTreeMap<&str, (u64, BTreeSet<u64>)> = BTreeMap::new();
        stores.insert("bank", (bank_bytes, BTreeSet::new()));
        if let Some(profile) = profile {
            if profile.header.size != 16 {
                return fail("profile header is not sixteen bytes");
            }
            stores.insert("header", (14, BTreeSet::new()));
            for (name, record) in &profile.records {
                let parity_bytes = (record.parity_offset / 4).div_ceil(8);
                if name.is_empty()
                    || stores.contains_key(name.as_str())
                    || record.parity_offset % 4 != 0
                    || record.defaults_offset + record.defaults.size > record.parity_offset
                    || record.parity_offset + parity_bytes + 4 > record.bytes
                    || record.bytes > 0x10_0000
                {
                    return fail("profile record name, defaults or parity outside the record");
                }
                stores.insert(name, (record.parity_offset, BTreeSet::new()));
            }
        }
        let place = |stores: &mut BTreeMap<&str, (u64, BTreeSet<u64>)>,
                     store: &str,
                     offset: u64,
                     size: usize| {
            let Some((extent, offsets)) = stores.get_mut(store) else {
                return false;
            };
            size > 0
                && size <= 128
                && offset
                    .checked_add(size as u64)
                    .is_some_and(|end| end <= *extent)
                && offsets.insert(offset)
        };
        for field in &self.fields {
            if !place(&mut stores, "bank", field.offset, field.bytes.len()) {
                return fail("field outside the bank or repeated");
            }
        }
        for (name, record) in profile.into_iter().flat_map(|p| &p.records) {
            for field in &record.fields {
                if !place(&mut stores, name, field.offset, field.bytes.len()) {
                    return fail("record field outside its record or repeated");
                }
            }
        }
        for (name, setting) in &self.settings {
            let mut sizes = setting.values.values().map(Vec::len);
            let size = sizes.next().unwrap_or(0);
            if name.is_empty()
                || !sizes.all(|other| other == size)
                || !place(&mut stores, &setting.store, setting.offset, size)
            {
                return fail("named setting is empty, uneven, unstored or overlaps a field");
            }
        }
        for field in profile.into_iter().flat_map(|p| &p.derived_fields) {
            let within = |start: u64, bytes: u64, extent: u64| {
                bytes > 0 && start.checked_add(bytes).is_some_and(|end| end <= extent)
            };
            let Some((extent, _)) = stores.get(field.store.as_str()) else {
                return fail("derived field names an absent store");
            };
            if field.store == "bank"
                || field.basis.is_empty()
                || !within(field.offset, field.bytes, *extent)
            {
                return fail("derived field outside its profile store");
            }
            match &field.source {
                ProfileFieldSource::Fill { .. } => {}
                ProfileFieldSource::BankCopy { offset }
                    if within(*offset, field.bytes, bank_bytes) => {}
                ProfileFieldSource::BankBits { offset, mask }
                    if field.bytes == 1 && *offset < bank_bytes && *mask != 0 => {}
                ProfileFieldSource::BankMap {
                    offset,
                    mask,
                    values,
                    otherwise,
                    otherwise_high,
                } if field.bytes == 1
                    && *offset < bank_bytes
                    && *mask != 0
                    && values
                        .iter()
                        .map(|v| v.input)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == values.len()
                    && values
                        .iter()
                        .map(|v| &v.output)
                        .chain(std::iter::once(otherwise))
                        .chain(otherwise_high.iter())
                        .all(|v| v & !mask == 0) => {}
                _ => return fail("derived field has invalid bank bounds or byte mapping"),
            }
        }
        let mut names = BTreeSet::new();
        for section in &self.sections {
            let within = |offset: u64, size: u64| {
                offset
                    .checked_add(size)
                    .is_some_and(|end| end <= bank_bytes)
            };
            let length = match section.length {
                SectionLength::Fixed { bytes } if bytes > 0 => within(section.offset, bytes),
                SectionLength::Fixed { .. } => false,
                SectionLength::Counted { counts, bits } => bits > 0 && within(counts, 6),
                SectionLength::CountedList { count, bits } => bits > 0 && within(count, 3),
            };
            if fujimod_catalog_identity::validate_id(&section.name).is_err()
                || !names.insert(section.name.as_str())
                || !within(section.offset, 0)
                || !length
            {
                return fail("section name repeated or invalid, or outside the bank");
            }
        }
        Ok(())
    }
}

/// A lowercase hexadecimal SHA-256.
fn hex_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests;
