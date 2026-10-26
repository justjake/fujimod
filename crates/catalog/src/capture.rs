//! Generated flash-read contracts and safely readable camera-state regions.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub const FLASH_SCHEMA_URI: &str = "https://fujimod.com/schemas/flash.generated.schema.json";
pub const STATE_SCHEMA_URI: &str = "https://fujimod.com/schemas/state.generated.schema.json";

/// Stock flash-read contract for one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareFlash {
    /// Schema identifying this generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// Firmware whose bytes establish this contract.
    pub firmware_id: String,
    /// Installed device, checked against the stock driver.
    pub device: FlashDevice,
    /// Reader source span: CPU aperture or physical NAND byte offsets.
    pub window: FlashRange,
    /// Update packaging that supplies comparison bytes.
    pub update_image: FlashUpdateImage,
    /// Stock reader ABI and alignment.
    pub reader: FlashReader,
    /// Processor-specific payload transport, absent when unresolved.
    pub transport: Option<String>,
    /// Stock instruction spans checked before running the reader.
    pub code_ranges: Vec<FlashCodeRange>,
    /// Live driver preconditions.
    pub state_checks: Vec<FlashStateCheck>,
    /// Initial update-backed control read.
    pub control: FlashControl,
}

/// Installed internal storage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FlashDevice {
    /// Serial NAND, serial NOR or raw NAND.
    pub kind: String,
    /// Six lowercase hexadecimal JEDEC digits in wire order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jedec_id: Option<String>,
    /// Manufacturer and part number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
}

/// Half-open reader source span.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FlashRange {
    /// First source address or physical byte offset.
    pub address: u64,
    /// Length in bytes.
    pub size: u64,
}

/// Mapping from update file bytes to flash offsets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FlashUpdateImage {
    /// File offset corresponding to flash offset zero.
    pub dat_offset: u64,
    /// Bytewise XOR applied to update bytes.
    pub xor: u8,
}

/// Stock eight-argument page read: (page, count, destination, controller_mode,
/// event_mode, timeout_ms, NULL observer, zero-initialized progress word).
/// The caller must acquire `lock` before the read and call `unlock` afterward.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NandPageReader {
    pub page_bytes: u32,
    pub pages_per_block: u32,
    pub destination_alignment: u32,
    pub controller_mode: u32,
    pub event_mode: u32,
    pub timeout_ms: u32,
    /// One page per call, matching the stock physical-page caller.
    pub max_pages_per_call: u32,
    pub lock: u64,
    pub unlock: u64,
}

/// Stock read request ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FlashReader {
    /// Symbol used to link the payload.
    pub symbol: String,
    /// Required destination, source and size alignment.
    pub alignment: u32,
    /// Parallel NAND page ABI, absent for memory-window readers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nand: Option<NandPageReader>,
}

/// Stock instruction range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FlashCodeRange {
    /// Role of these instructions.
    pub name: String,
    /// First CPU address.
    pub address: u64,
    /// Length in bytes.
    pub size: u64,
}

/// Driver state that must match before reading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FlashStateCheck {
    /// Meaning of the state.
    pub name: String,
    /// CPU address.
    pub address: u64,
    /// Word width in bytes.
    pub size: u32,
    /// Required little-endian value.
    pub value: u64,
}

/// Update-backed initial read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FlashControl {
    /// Offset in the flash window.
    pub offset: u64,
    /// Length in bytes.
    pub size: u64,
}

/// Safely readable regions captured as camera state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareState {
    /// Schema identifying this generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// Firmware whose bytes establish this contract.
    pub firmware_id: String,
    /// Capture order. Device registers with read side effects are excluded.
    pub regions: Vec<StateRegion>,
}

/// One safely readable CPU-visible region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct StateRegion {
    /// Region identity.
    pub name: String,
    /// First CPU address.
    pub address: u64,
    /// Length in bytes.
    pub size: u64,
    /// Read twice and require equal bytes.
    pub constant: bool,
}

fn extent(address: u64, size: u64) -> Result<(), String> {
    if size == 0 || address.checked_add(size).is_none() {
        return Err("invalid capture extent".into());
    }
    Ok(())
}
fn identity(
    uri: &str,
    expected: &str,
    schema: u32,
    id: &str,
    firmware: &str,
) -> Result<(), String> {
    fujimod_catalog_identity::validate_id(id)?;
    fujimod_catalog_identity::validate_id(firmware)?;
    if uri != expected || schema != 1 {
        return Err("invalid capture document schema".into());
    }
    Ok(())
}
impl FirmwareFlash {
    /// Validate bounded ranges, reader alignment and update comparison geometry.
    pub fn validate(&self) -> Result<(), String> {
        identity(
            &self.schema_uri,
            FLASH_SCHEMA_URI,
            self.schema,
            &self.id,
            &self.firmware_id,
        )?;
        extent(self.window.address, self.window.size)?;
        let raw_nand = self.device.kind == "raw-nand";
        if !matches!(
            self.device.kind.as_str(),
            "serial-nand" | "serial-nor" | "raw-nand"
        ) || self.device.jedec_id.as_ref().is_some_and(|id| {
            id.len() != 6
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) || self
            .device
            .part
            .as_ref()
            .is_some_and(|part| part.is_empty())
            || (!raw_nand && (self.device.jedec_id.is_none() || self.device.part.is_none()))
            || self.reader.symbol.is_empty()
            || !self.reader.alignment.is_power_of_two()
            || self.code_ranges.is_empty()
            || self.state_checks.is_empty()
            || self
                .transport
                .as_ref()
                .is_some_and(|transport| transport.is_empty())
        {
            return Err("invalid flash device or reader contract".into());
        }
        match (&self.reader.nand, raw_nand) {
            (Some(nand), true) => {
                if self.window.address != 0
                    || !matches!(
                        (nand.page_bytes, nand.pages_per_block),
                        (512, 32) | (2048, 64)
                    )
                    || nand.destination_alignment != 32
                    || nand.controller_mode != 0
                    || nand.event_mode != 0
                    || nand.timeout_ms == 0
                    || nand.max_pages_per_call != 1
                    || self.reader.alignment != nand.page_bytes
                    || ![nand.lock, nand.unlock]
                        .iter()
                        .all(|address| self.code_ranges.iter().any(|r| r.address == *address))
                {
                    return Err("invalid corrected NAND page ABI".into());
                }
            }
            (None, false) => {}
            _ => return Err("NAND reader ABI and device kind differ".into()),
        }
        let alignment = u64::from(self.reader.alignment);
        if !self.window.address.is_multiple_of(alignment)
            || !self.window.size.is_multiple_of(alignment)
            || !self.control.offset.is_multiple_of(alignment)
            || !self.control.size.is_multiple_of(alignment)
            || self.control.size == 0
            || self
                .control
                .offset
                .checked_add(self.control.size)
                .is_none_or(|end| end > self.window.size)
        {
            return Err("invalid flash control geometry".into());
        }
        let mut names = BTreeSet::new();
        for range in &self.code_ranges {
            extent(range.address, range.size)?;
            if range.name.is_empty() || !names.insert(&range.name) {
                return Err("repeated or empty flash code role".into());
            }
        }
        if !names.contains(&self.reader.symbol) {
            return Err("stock reader is not checked".into());
        }
        for check in &self.state_checks {
            extent(check.address, u64::from(check.size))?;
            if check.name.is_empty()
                || !matches!(check.size, 1 | 2 | 4 | 8)
                || (check.size < 8 && check.value >= (1u64 << (check.size * 8)))
            {
                return Err("invalid flash state check".into());
            }
        }
        Ok(())
    }
}
impl FirmwareState {
    /// Validate unique, nonoverlapping ranges and address arithmetic.
    pub fn validate(&self) -> Result<(), String> {
        identity(
            &self.schema_uri,
            STATE_SCHEMA_URI,
            self.schema,
            &self.id,
            &self.firmware_id,
        )?;
        if self.regions.is_empty() {
            return Err("empty state capture".into());
        }
        let mut names = BTreeSet::new();
        let mut spans = vec![];
        for region in &self.regions {
            extent(region.address, region.size)?;
            if region.name.is_empty() || !names.insert(&region.name) {
                return Err("repeated or empty state region".into());
            }
            spans.push((region.address, region.address + region.size));
        }
        spans.sort_unstable();
        if spans.windows(2).any(|p| p[0].1 > p[1].0) {
            return Err("overlapping state regions".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(regions: Vec<StateRegion>) -> FirmwareState {
        FirmwareState {
            schema_uri: STATE_SCHEMA_URI.into(),
            schema: 1,
            id: "test-state".into(),
            firmware_id: "test".into(),
            regions,
        }
    }
    fn region(address: u64, size: u64) -> StateRegion {
        StateRegion {
            name: format!("region-{address}"),
            address,
            size,
            constant: false,
        }
    }
    #[test]
    fn rejects_empty_overlapping_wrapping_and_duplicate_regions() {
        assert!(state(vec![]).validate().is_err());
        assert!(state(vec![region(0, 0)]).validate().is_err());
        assert!(state(vec![region(u64::MAX, 2)]).validate().is_err());
        assert!(
            state(vec![region(0, 16), region(8, 16)])
                .validate()
                .is_err()
        );
        assert!(
            state(vec![region(0, 16), region(0, 16)])
                .validate()
                .is_err()
        );
        assert!(
            state(vec![region(0, 16), region(16, 16)])
                .validate()
                .is_ok()
        );
    }
}
