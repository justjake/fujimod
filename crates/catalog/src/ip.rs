//! Reusable block designs and their register layouts.

use crate::provenance::Evidence;
use crate::soc::BlockDevice;
use fujimod_catalog_identity::validate_id;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Stable schema identity for an IP record.
pub const IP_SCHEMA_URI: &str = "https://fujimod.com/schemas/ip.schema.json";

/// A block design several processors instantiate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct Ip {
    /// Stable schema URI for this IP record.
    #[serde(rename = "$schema")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "\"https://fujimod.com/schemas/ip.schema.json\"")
    )]
    pub schema_uri: String,
    /// IP record version.
    pub schema: u32,
    /// Stable IP identity.
    pub id: String,
    /// Device model shared by every instance, when the design fixes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "BlockDevice | null")
    )]
    pub device: Option<BlockDevice>,
    /// Register layout shared by every instance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registers: Vec<BlockRegister>,
    /// Parameters an instance must supply, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<String>,
    /// How the design's facts were established.
    pub evidence: Vec<Evidence>,
}

impl Ip {
    /// Validate the record identity, layout and instance parameters.
    pub fn validate(&self, selected_id: &str) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != IP_SCHEMA_URI || self.id != selected_id {
            return Err("IP schema or identity differs".into());
        }
        validate_id(&self.id)?;
        validate_register_layout(&self.registers, None)?;
        let mut parameters = BTreeSet::new();
        for parameter in &self.parameters {
            validate_id(parameter)?;
            if !parameters.insert(parameter.as_str()) {
                return Err(format!("IP {} repeats parameter {parameter}", self.id));
            }
        }
        if self.evidence.is_empty() {
            return Err(format!("IP {} has no evidence", self.id));
        }
        for evidence in &self.evidence {
            evidence.validate()?;
        }
        Ok(())
    }
}

/// One named register in a block or IP layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BlockRegister {
    /// Register name within the block.
    pub name: String,
    /// Byte offset within the block.
    pub offset: u64,
    /// Register width in bytes.
    pub width_bytes: u8,
    /// Value the register holds after reset, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<u64>,
    /// Bits a write may set, when the register accepts only some of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_mask: Option<u64>,
    /// Accesses the register accepts.
    pub access: RegisterAccess,
    /// Named bit fields and the values they carry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<RegisterField>,
    /// What reads return, when they do not return the register's last write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "RegisterReads | null")
    )]
    pub reads: Option<RegisterReads>,
}

/// What reads of a register return in place of its last write.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RegisterReads {
    /// Every read returns `value`.
    Fixed { value: u64 },
    /// Reads alternate between 0 and `value`, starting with 0.
    Alternates { value: u64 },
    /// Reads return 0: the register clears itself once written.
    SelfClearing,
    /// Reads return 0 before the first write and `value` after it.
    AfterWrite { value: u64 },
}

/// Accesses a register accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RegisterAccess {
    /// Reads are accepted; writes are refused.
    ReadOnly,
    /// Writes are accepted; reads are refused.
    WriteOnly,
    /// Reads and writes are accepted.
    ReadWrite,
}

/// One named bit field within a register.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RegisterField {
    /// Field name within the register.
    pub name: String,
    /// Least-significant bit of the field.
    pub bit: u8,
    /// Field width in bits.
    pub width_bits: u8,
    /// Named values the field carries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<RegisterFieldValue>,
}

/// One named value of a register bit field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RegisterFieldValue {
    /// Value name.
    pub name: String,
    /// Value the field carries.
    pub value: u64,
}

/// Check a register layout against itself and, when given, its containing window.
pub fn validate_register_layout(
    registers: &[BlockRegister],
    limit: Option<u64>,
) -> Result<(), String> {
    let mut names = BTreeSet::new();
    let mut extents = Vec::new();
    for register in registers {
        if register.name.is_empty() || !names.insert(register.name.as_str()) {
            return Err(format!(
                "register layout repeats or omits name {}",
                register.name
            ));
        }
        if !matches!(register.width_bytes, 1 | 2 | 4 | 8) {
            return Err(format!(
                "register {} has unsupported width {}",
                register.name, register.width_bytes
            ));
        }
        let bits = u64::from(register.width_bytes) * 8;
        if bits < 64 && register.reset.is_some_and(|reset| reset >> bits != 0) {
            return Err(format!(
                "register {} reset value exceeds its width",
                register.name
            ));
        }
        if bits < 64 && register.write_mask.is_some_and(|mask| mask >> bits != 0) {
            return Err(format!(
                "register {} write mask exceeds its width",
                register.name
            ));
        }
        let end = register
            .offset
            .checked_add(u64::from(register.width_bytes))
            .ok_or_else(|| format!("register {} extent overflows", register.name))?;
        if limit.is_some_and(|limit| end > limit) {
            return Err(format!(
                "register {} extends past its block window",
                register.name
            ));
        }
        let mut field_names = BTreeSet::new();
        let mut field_bits = Vec::new();
        for field in &register.fields {
            if field.name.is_empty() || !field_names.insert(field.name.as_str()) {
                return Err(format!(
                    "register {} repeats or omits a field name",
                    register.name
                ));
            }
            let field_end = u16::from(field.bit) + u16::from(field.width_bits);
            if field.width_bits == 0
                || u16::from(field.width_bits) > 64
                || u64::from(field_end) > bits
            {
                return Err(format!(
                    "register {} field {} exceeds its width",
                    register.name, field.name
                ));
            }
            if field_bits
                .iter()
                .any(|(start, end): &(u16, u16)| *start < field_end && u16::from(field.bit) < *end)
            {
                return Err(format!(
                    "register {} fields overlap at {}",
                    register.name, field.name
                ));
            }
            field_bits.push((u16::from(field.bit), field_end));
            let mut values = BTreeSet::new();
            for value in &field.values {
                if value.name.is_empty() || !values.insert(value.name.as_str()) {
                    return Err(format!(
                        "register {} field {} repeats or omits a value name",
                        register.name, field.name
                    ));
                }
                if field.width_bits < 64 && value.value >= 1u64 << field.width_bits {
                    return Err(format!(
                        "register {} field {} value {} exceeds its width",
                        register.name, field.name, value.name
                    ));
                }
            }
        }
        extents.push((register.offset, end, register.name.as_str()));
    }
    extents.sort_unstable();
    if extents.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("register layout overlaps two registers".into());
    }
    Ok(())
}
