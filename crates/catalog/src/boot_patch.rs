//! Located boot-patch facts for one firmware.
//!
//! The document names every unit the stock loader places in RAM, the trigger
//! sites where the patch may apply a group of runs, the RAM stock clears after
//! stage 3, and the free ranges for the trampolines and the resident apply
//! part. It holds addresses, sizes and hashes, never firmware bytes; the host
//! builder decodes the update for those. See `research/boot-patch.md`.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const BOOT_PATCH_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/boot-patch.generated.schema.json";

/// Boot-patch facts for one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FirmwareBootPatch {
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
    /// Instruction set the image uses.
    pub isa: BootPatchIsa,
    /// Units the stock loader places in RAM, in boot order.
    pub units: Vec<BootPatchUnit>,
    /// Trigger sites in boot order.
    pub triggers: Vec<BootPatchTrigger>,
    /// RAM stock clears after stage 3.
    pub clears: Vec<BootPatchClear>,
    /// RAM range for the generated trampolines.
    pub trampoline_space: BootPatchRange,
    /// RAM range for the resident apply part.
    pub resident_space: BootPatchRange,
    /// Units already in RAM when stage 3 runs.
    pub stage3_resident: Vec<String>,
}

/// Instruction set of the patched image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum BootPatchIsa {
    /// 32-bit ARM.
    Arm32,
}

/// Compression of one unit's stored stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum BootPatchCompression {
    /// Stored bytes are the loaded bytes.
    Raw,
    /// The stock method-4 decoder expands the stream.
    Method4,
}

/// One initialized RAM image or overlay bank.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BootPatchUnit {
    /// Name the DAT reader gives the unit.
    pub name: String,
    /// Cached RAM address of the first loaded byte.
    pub address: u64,
    /// Loaded bytes.
    pub size: u64,
    /// Offset of the stored stream in the update.
    pub dat_offset: u64,
    /// Stored-stream compression.
    pub compression: BootPatchCompression,
    /// Whether the unit holds executable code.
    pub executable: bool,
    /// SHA-256 of the loaded bytes.
    pub sha256: String,
}

/// How a trigger's group is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum BootPatchTriggerKind {
    /// The site fires once per boot.
    Once,
    /// The site fires for a copied range held in two registers.
    Range,
}

/// One site whose completion means its group can be applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BootPatchTrigger {
    /// Trigger name, used by clears and progress records.
    pub name: String,
    /// RAM address whose instruction the trampoline replaces.
    pub site: u64,
    /// Stock word expected at the site.
    pub expected: u32,
    /// Whether the site is once or reports a range.
    pub kind: BootPatchTriggerKind,
    /// ARM register holding the copied base for a range trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_register: Option<String>,
    /// ARM register holding the copied length for a range trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_register: Option<String>,
    /// Mask applied to the base register before reporting a range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_mask: Option<u32>,
    /// Resident units and units streamed by this or earlier triggers.
    pub units: Vec<String>,
}

/// A RAM range stock zeroes after stage 3.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BootPatchClear {
    /// Clear name.
    pub name: String,
    /// First zeroed byte.
    pub address: u64,
    /// Zeroed bytes.
    pub size: u64,
    /// Trigger after which the clear is done.
    pub done_after: String,
}

/// A half-open RAM range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BootPatchRange {
    /// First byte.
    pub address: u64,
    /// Bytes.
    pub size: u64,
}

impl FirmwareBootPatch {
    /// Validate identity, layout, trigger order and the free spaces.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        if self.schema_uri != BOOT_PATCH_SCHEMA_URI
            || self.schema != 1
            || self.update_sha256.len() != 64
            || !self
                .update_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!(
                "boot-patch {}: invalid schema or update hash",
                self.id
            ));
        }
        if self.units.is_empty() || self.triggers.is_empty() {
            return Err("boot-patch: an image needs units and triggers".into());
        }
        let mut names = BTreeSet::new();
        for unit in &self.units {
            if unit.name.is_empty() || !names.insert(unit.name.as_str()) {
                return Err(format!("boot-patch: duplicate or empty unit {}", unit.name));
            }
            unit.validate()?;
        }
        for (index, unit) in self.units.iter().enumerate() {
            for other in &self.units[index + 1..] {
                if unit.address < other.address + other.size
                    && other.address < unit.address + unit.size
                {
                    return Err(format!(
                        "boot-patch: units {} and {} overlap",
                        unit.name, other.name
                    ));
                }
            }
        }
        if self.stage3_resident.is_empty() {
            return Err("boot-patch: stage 3 needs at least the stage-1 image".into());
        }
        for name in &self.stage3_resident {
            if !names.contains(name.as_str()) {
                return Err(format!("boot-patch: unknown stage-3 resident unit {name}"));
            }
        }
        let mut trigger_names = BTreeSet::new();
        let mut previous_units: BTreeSet<&str> = BTreeSet::new();
        for (index, trigger) in self.triggers.iter().enumerate() {
            if trigger.name.is_empty() || !trigger_names.insert(trigger.name.as_str()) {
                return Err(format!(
                    "boot-patch: duplicate or empty trigger {}",
                    trigger.name
                ));
            }
            if trigger.site % 4 != 0 || trigger.expected == 0 {
                return Err(format!(
                    "boot-patch: trigger {} has an invalid site or word",
                    trigger.name
                ));
            }
            let site_unit = self
                .units
                .iter()
                .filter(|unit| {
                    unit.address <= trigger.site && trigger.site + 4 <= unit.address + unit.size
                })
                .count();
            if site_unit != 1 {
                return Err(format!(
                    "boot-patch: trigger {} lies outside every unit or in several",
                    trigger.name
                ));
            }
            match (
                trigger.kind,
                &trigger.base_register,
                &trigger.length_register,
                trigger.address_mask,
            ) {
                (BootPatchTriggerKind::Once, None, None, None) => {}
                (BootPatchTriggerKind::Range, Some(base), Some(length), Some(mask))
                    if register(base) && register(length) && mask != 0 => {}
                (BootPatchTriggerKind::Range, _, _, _) => {
                    return Err(format!(
                        "boot-patch: range trigger {} needs base and length registers and a nonzero address mask",
                        trigger.name
                    ));
                }
                _ => {
                    return Err(format!(
                        "boot-patch: once trigger {} names registers",
                        trigger.name
                    ));
                }
            }
            let mut listed = BTreeSet::new();
            for name in &trigger.units {
                if !names.contains(name.as_str()) || !listed.insert(name.as_str()) {
                    return Err(format!(
                        "boot-patch: trigger {} names an unknown or repeated unit {name}",
                        trigger.name
                    ));
                }
            }
            if !previous_units.is_subset(&listed) {
                return Err(format!(
                    "boot-patch: trigger {} loses a unit an earlier trigger loaded",
                    trigger.name
                ));
            }
            if index == 0 {
                for name in &self.stage3_resident {
                    if !listed.contains(name.as_str()) {
                        return Err(format!(
                            "boot-patch: the first trigger does not keep {name} resident"
                        ));
                    }
                }
            }
            previous_units = listed;
        }
        let mut clear_names = BTreeSet::new();
        for clear in &self.clears {
            if clear.name.is_empty() || !clear_names.insert(clear.name.as_str()) {
                return Err(format!(
                    "boot-patch: duplicate or empty clear {}",
                    clear.name
                ));
            }
            if clear.address % 4 != 0
                || clear.size == 0
                || clear.address.checked_add(clear.size).is_none()
            {
                return Err(format!(
                    "boot-patch: clear {} has an invalid range",
                    clear.name
                ));
            }
            if !trigger_names.contains(clear.done_after.as_str()) {
                return Err(format!(
                    "boot-patch: clear {} names unknown trigger {}",
                    clear.name, clear.done_after
                ));
            }
        }
        self.trampoline_space.validate("trampoline")?;
        self.resident_space.validate("resident")?;
        if self.trampoline_space.overlaps(&self.resident_space) {
            return Err("boot-patch: the trampoline and resident spaces overlap".into());
        }
        let mut blocked: Vec<(u64, u64, String)> = self
            .units
            .iter()
            .map(|unit| (unit.address, unit.size, format!("unit {}", unit.name)))
            .chain(
                self.clears
                    .iter()
                    .map(|clear| (clear.address, clear.size, format!("clear {}", clear.name))),
            )
            .collect();
        blocked.push((
            self.trampoline_space.address,
            self.trampoline_space.size,
            "trampoline space".into(),
        ));
        for (address, size, name) in [(
            self.resident_space.address,
            self.resident_space.size,
            "resident space",
        )] {
            for (low, length, other) in &blocked {
                if address < low + length && *low < address + size {
                    return Err(format!("boot-patch: {name} overlaps {other}"));
                }
            }
        }
        Ok(())
    }
}

fn register(name: &str) -> bool {
    name.strip_prefix('r')
        .and_then(|index| index.parse::<u8>().ok())
        .is_some_and(|index| index < 16)
}

impl BootPatchUnit {
    fn validate(&self) -> Result<(), String> {
        if !self.name.is_ascii()
            || !self.address.is_multiple_of(4)
            || self.size == 0
            || self.address.checked_add(self.size).is_none()
            || !self.dat_offset.is_multiple_of(4)
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!("boot-patch: invalid unit {}", self.name));
        }
        Ok(())
    }
}

impl BootPatchRange {
    fn validate(&self, what: &str) -> Result<(), String> {
        if !self.address.is_multiple_of(4)
            || self.size == 0
            || !self.size.is_multiple_of(4)
            || self.address.checked_add(self.size).is_none()
            || self.address + self.size > 0x1_0000_0000
        {
            return Err(format!("boot-patch: invalid {what} range"));
        }
        Ok(())
    }

    fn overlaps(&self, other: &BootPatchRange) -> bool {
        self.address < other.address + other.size && other.address < self.address + self.size
    }
}
