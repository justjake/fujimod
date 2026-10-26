//! Explicit, identity-checked persistent inputs supplied for a run.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Declared source windows and selected placements, independent of file locations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PersistentInputs {
    /// Source window identities and explicit normalization edits.
    pub assets: BTreeMap<String, PersistentAsset>,
    /// Named, independently selected input sets.
    pub sets: BTreeMap<String, PersistentSet>,
}
/// A bounded source window verified before and after normalization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PersistentAsset {
    /// Exact source window length in bytes.
    pub bytes: usize,
    /// SHA-256 of the supplied source window.
    pub sha256: String,
    /// SHA-256 after applying the declared edits.
    pub normalized_sha256: String,
    /// Explicit edits applied only to an exact source identity.
    pub patches: Vec<PersistentPatch>,
}
/// An explicit byte edit within a source window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PersistentPatch {
    /// Byte offset within the source window.
    pub offset: usize,
    /// Replacement bytes.
    pub bytes: Vec<u8>,
}
/// External NAND and MCU state selected together by identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PersistentSet {
    /// NAND main-area windows and their optional managed block tags.
    pub placements: Vec<PersistentPlacement>,
    /// Explicit journal append, when selected.
    pub append: Option<PersistentJournalAppend>,
    /// Optional persistent MCU packet source.
    pub mcu_packet: Option<PersistentPacket>,
}
/// Place a bounded source window into the selected body's NAND.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PersistentPlacement {
    /// Source asset identity.
    pub asset: String,
    /// Starting byte within the source asset.
    pub source_offset: usize,
    /// Number of main-area bytes.
    pub bytes: usize,
    /// Main-area offset within the selected NAND.
    pub physical_offset: u64,
    /// First logical managed-block index, if synthetic tags are requested.
    pub managed_index: Option<u16>,
}
/// Explicit journal-record construction and destination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PersistentJournalAppend {
    /// Source record asset identity.
    pub record_asset: String,
    /// Source payload asset identity.
    pub payload_asset: String,
    /// Source payload offset.
    pub payload_offset: usize,
    /// Payload length.
    pub payload_bytes: usize,
    /// Payload destination offset in the record.
    pub record_payload_offset: usize,
    /// Parity bitmap offset in the record.
    pub parity_offset: usize,
    /// XOR checksum offset in the record.
    pub checksum_offset: usize,
    /// Destination offset in the NAND main area.
    pub physical_offset: u64,
}
/// A complete packet or a bounded source window, never a file path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersistentPacket {
    /// Explicit packet bytes.
    Bytes {
        /// Packet content.
        bytes: Vec<u8>,
    },
    /// Packet extracted from an identity-checked asset.
    Window {
        /// Asset identity.
        asset: String,
        /// Byte offset in that asset.
        offset: usize,
        /// Window length.
        bytes: usize,
    },
}
impl PersistentAsset {
    /// Verify the source, apply declared edits, and verify the final identity.
    pub fn prepare(&self, input: &[u8]) -> Result<Vec<u8>, String> {
        self.validate()?;
        if input.len() != self.bytes {
            return Err("Persistent input window length differs".into());
        }
        let digest = format!("{:x}", Sha256::digest(input));
        let mut bytes = input.to_vec();
        if digest == self.sha256 {
            for patch in &self.patches {
                bytes[patch.offset..patch.offset + patch.bytes.len()].copy_from_slice(&patch.bytes);
            }
        } else if digest != self.normalized_sha256 {
            return Err("Persistent input SHA-256 differs".into());
        }
        if format!("{:x}", Sha256::digest(&bytes)) != self.normalized_sha256 {
            return Err("Normalized persistent input SHA-256 differs".into());
        }
        Ok(bytes)
    }
    fn validate(&self) -> Result<(), String> {
        if self.bytes == 0 || self.bytes > 128 << 20 || self.patches.len() > 4096 {
            return Err("Persistent input extent exceeds its bound".into());
        }
        for digest in [&self.sha256, &self.normalized_sha256] {
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err("Persistent input SHA-256 must be lowercase hexadecimal".into());
            }
        }
        for patch in &self.patches {
            if patch.bytes.is_empty()
                || patch
                    .offset
                    .checked_add(patch.bytes.len())
                    .is_none_or(|end| end > self.bytes)
            {
                return Err("Persistent patch exceeds input".into());
            }
        }
        Ok(())
    }
}
impl PersistentInputs {
    /// Check identities, references and bounded source extents before loading bytes.
    pub fn validate(&self) -> Result<(), String> {
        if self.assets.len() > 256 || self.sets.is_empty() || self.sets.len() > 256 {
            return Err("Persistent input asset or set count exceeds its bound".into());
        }
        for (id, asset) in &self.assets {
            fujimod_catalog_identity::validate_id(id)?;
            asset.validate()?;
        }
        let window = |id: &str, offset: usize, bytes: usize| -> Result<(), String> {
            let asset = self
                .assets
                .get(id)
                .ok_or_else(|| format!("Unknown persistent asset {id}"))?;
            if bytes == 0
                || offset
                    .checked_add(bytes)
                    .is_none_or(|end| end > asset.bytes)
            {
                return Err("Persistent source window exceeds input".into());
            }
            Ok(())
        };
        for (id, set) in &self.sets {
            fujimod_catalog_identity::validate_id(id)?;
            if set.placements.len() > 1024 {
                return Err("Persistent placement count exceeds its bound".into());
            }
            for placement in &set.placements {
                window(&placement.asset, placement.source_offset, placement.bytes)?;
            }
            if let Some(append) = &set.append {
                window(
                    &append.payload_asset,
                    append.payload_offset,
                    append.payload_bytes,
                )?;
                if !self.assets.contains_key(&append.record_asset) {
                    return Err("Unknown persistent record asset".into());
                }
            }
            if let Some(packet) = &set.mcu_packet {
                match packet {
                    PersistentPacket::Bytes { bytes } if bytes.len() != 16 => {
                        return Err("MCU packet must contain sixteen bytes".into());
                    }
                    PersistentPacket::Bytes { .. } => (),
                    PersistentPacket::Window {
                        asset,
                        offset,
                        bytes,
                    } => {
                        if *bytes != 16 {
                            return Err("MCU packet must contain sixteen bytes".into());
                        }
                        window(asset, *offset, *bytes)?;
                    }
                }
            }
        }
        Ok(())
    }
}
