//! Persistent NAND bytes, separate from the SoC controller. Supplied images
//! and declared overlays are immutable; programmed/erased pages override them.
//! Spare prefixes are declared synthetic inputs, not captured OOB/ECC.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub bytes: u64,
    pub page_bytes: usize,
    pub pages_per_block: u32,
    pub spare_bytes: usize,
}
#[derive(Clone)]
pub struct Overlay {
    pub offset: u64,
    pub bytes: Arc<[u8]>,
}
#[derive(Clone)]
pub struct Nand {
    options: Options,
    base: Arc<[u8]>,
    overlays: Vec<Overlay>,
    spare: BTreeMap<u32, Vec<u8>>,
    pages: BTreeMap<u32, Vec<u8>>,
    written_spare: BTreeMap<u32, Vec<u8>>,
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    options: Options,
    overlays: Vec<u64>,
    spare: BTreeMap<u32, Vec<u8>>,
    pages: BTreeMap<u32, Vec<u8>>,
    written_spare: BTreeMap<u32, Vec<u8>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommittedImage {
    schema: u32,
    options: Options,
    base_sha256: String,
    base_bytes: usize,
    overlays: Vec<(u64, Vec<u8>)>,
    spare: Vec<(u32, Vec<u8>)>,
    pages: Vec<(u32, Vec<u8>)>,
    written_spare: Vec<(u32, Vec<u8>)>,
}
/// Portable programmed pages relative to the checked immutable medium inputs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    identity: serde_json::Value,
    pages: BTreeMap<u32, Vec<u8>>,
    written_spare: BTreeMap<u32, Vec<u8>>,
}
impl Nand {
    fn immutable_identity(&self) -> serde_json::Value {
        serde_json::json!({"options":self.options,"base":crate::fingerprint::bytes(&self.base),
            "overlays":self.overlays.iter().map(|o|(o.offset,crate::fingerprint::bytes(&o.bytes))).collect::<Vec<_>>(),
            "spare":self.spare.iter().map(|(row,bytes)|(row,crate::fingerprint::bytes(bytes))).collect::<Vec<_>>()})
    }
    pub fn changes(&self) -> Changes {
        Changes {
            identity: self.immutable_identity(),
            pages: self.pages.clone(),
            written_spare: self.written_spare.clone(),
        }
    }
    pub fn with_changes(&self, changes: &Changes) -> Result<Self, String> {
        if changes.identity != self.immutable_identity() {
            return Err("NAND durable input identity differs".into());
        }
        let rows = self.options.bytes / self.options.page_bytes as u64;
        if changes
            .pages
            .iter()
            .any(|(&row, bytes)| row as u64 >= rows || bytes.len() != self.options.page_bytes)
            || changes
                .written_spare
                .iter()
                .any(|(&row, bytes)| row as u64 >= rows || bytes.len() != self.options.spare_bytes)
        {
            return Err("NAND durable page extent differs".into());
        }
        let mut medium = self.clone();
        medium.pages = changes.pages.clone();
        medium.written_spare = changes.written_spare.clone();
        Ok(medium)
    }
    pub fn checkpoint(&self) -> Result<crate::checkpoint::Checkpoint, String> {
        let state = Snapshot {
            options: self.options.clone(),
            overlays: self.overlays.iter().map(|o| o.offset).collect(),
            spare: self.spare.clone(),
            pages: self.pages.clone(),
            written_spare: self.written_spare.clone(),
        };
        let blobs = std::iter::once(self.base.clone())
            .chain(self.overlays.iter().map(|o| o.bytes.clone()))
            .collect();
        Ok(crate::checkpoint::Checkpoint::encode("nand-medium", &state)?.with_blobs(blobs))
    }
    pub fn from_checkpoint(saved: &crate::checkpoint::Checkpoint) -> Result<Self, String> {
        let state: Snapshot = saved.decode("nand-medium")?;
        if saved.blobs().len() != state.overlays.len() + 1 {
            return Err("NAND immutable blob count differs".into());
        }
        let overlays = state
            .overlays
            .into_iter()
            .zip(&saved.blobs()[1..])
            .map(|(offset, bytes)| Overlay {
                offset,
                bytes: bytes.clone(),
            })
            .collect();
        let mut medium = Self::new(
            state.options,
            saved.blobs()[0].clone(),
            overlays,
            state.spare,
        )?;
        let rows = medium.options.bytes / medium.options.page_bytes as u64;
        if state
            .pages
            .iter()
            .any(|(&r, b)| r as u64 >= rows || b.len() != medium.options.page_bytes)
            || state
                .written_spare
                .iter()
                .any(|(&r, b)| r as u64 >= rows || b.len() != medium.options.spare_bytes)
        {
            return Err("NAND snapshot page extent differs".into());
        }
        medium.pages = state.pages;
        medium.written_spare = state.written_spare;
        Ok(medium)
    }

    pub fn fingerprint(&self) -> serde_json::Value {
        let pages = |data: &BTreeMap<u32, Vec<u8>>| {
            data.iter()
                .map(|(row, bytes)| (*row, crate::fingerprint::bytes(bytes)))
                .collect::<Vec<_>>()
        };
        serde_json::json!({"options":self.options,"base":crate::fingerprint::bytes(&self.base),
            "overlays":self.overlays.iter().map(|o|(o.offset,crate::fingerprint::bytes(&o.bytes))).collect::<Vec<_>>(),
            "spare":pages(&self.spare),"pages":pages(&self.pages),"written_spare":pages(&self.written_spare)})
    }
    pub fn new(
        options: Options,
        base: Arc<[u8]>,
        mut overlays: Vec<Overlay>,
        spare: BTreeMap<u32, Vec<u8>>,
    ) -> Result<Self, String> {
        if options.bytes == 0
            || options.bytes > 0x100000000
            || !matches!(options.page_bytes, 512 | 2048)
            || !matches!(options.pages_per_block, 32 | 64)
            || options.spare_bytes < 16
            || options.spare_bytes > 512
            || !options
                .bytes
                .is_multiple_of((options.page_bytes as u64) * options.pages_per_block as u64)
            || base.len() as u64 > options.bytes
        {
            return Err("Invalid NAND geometry or base extent".into());
        }
        overlays.sort_by_key(|o| o.offset);
        let mut end = 0;
        for overlay in &overlays {
            if overlay.bytes.is_empty()
                || overlay.offset < end
                || overlay
                    .offset
                    .checked_add(overlay.bytes.len() as u64)
                    .is_none_or(|e| e > options.bytes)
            {
                return Err("NAND overlays overlap or exceed medium".into());
            }
            end = overlay.offset + overlay.bytes.len() as u64;
        }
        if spare.iter().any(|(&row, b)| {
            row as u64 >= options.bytes / options.page_bytes as u64
                || b.is_empty()
                || b.len() > options.spare_bytes
        }) {
            return Err("NAND spare input out of range".into());
        }
        Ok(Self {
            options,
            base,
            overlays,
            spare,
            pages: BTreeMap::new(),
            written_spare: BTreeMap::new(),
        })
    }
    /// Add explicit external initialization inputs before any page has been
    /// programmed or erased. Validation completes before returning a new part.
    pub fn with_inputs(
        &self,
        overlays: Vec<Overlay>,
        spare: BTreeMap<u32, Vec<u8>>,
    ) -> Result<Self, String> {
        if !self.pages.is_empty() || !self.written_spare.is_empty() {
            return Err("Initial NAND inputs require an unprogrammed medium".into());
        }
        if spare.keys().any(|row| self.spare.contains_key(row)) {
            return Err("Initial NAND spare inputs overlap".into());
        }
        let mut all = self.overlays.clone();
        all.extend(overlays);
        let mut tags = self.spare.clone();
        tags.extend(spare);
        Self::new(self.options.clone(), self.base.clone(), all, tags)
    }
    /// Replace a checked external initialization window. Programmed media is
    /// refused; the controller's separate cold-retain boundary enforces time.
    pub fn replace_input(
        &self,
        offset: u64,
        expected: &[u8],
        bytes: &[u8],
    ) -> Result<Self, String> {
        if !self.pages.is_empty()
            || !self.written_spare.is_empty()
            || expected.is_empty()
            || expected.len() != bytes.len()
        {
            return Err("NAND input replacement requires unprogrammed, equal extents".into());
        }
        let mut overlays = self.overlays.clone();
        let target = overlays
            .iter_mut()
            .find(|o| {
                o.offset <= offset
                    && offset
                        .checked_add(bytes.len() as u64)
                        .is_some_and(|end| end <= o.offset + o.bytes.len() as u64)
            })
            .ok_or("NAND input replacement is outside a declared overlay")?;
        let start = (offset - target.offset) as usize;
        if &target.bytes[start..start + expected.len()] != expected {
            return Err("NAND input replacement expected bytes differ".into());
        }
        let mut data = target.bytes.to_vec();
        data[start..start + bytes.len()].copy_from_slice(bytes);
        target.bytes = data.into();
        Self::new(
            self.options.clone(),
            self.base.clone(),
            overlays,
            self.spare.clone(),
        )
    }
    /// Only committed external backing crosses a power cycle. CPU/controller
    /// state and pending operations never enter this format.
    pub fn committed_image(&self) -> Result<Vec<u8>, String> {
        use sha2::{Digest, Sha256};
        serde_json::to_vec(&CommittedImage {
            schema: 1,
            options: self.options.clone(),
            base_sha256: format!("{:x}", Sha256::digest(&self.base)),
            base_bytes: self.base.len(),
            overlays: self
                .overlays
                .iter()
                .map(|o| (o.offset, o.bytes.to_vec()))
                .collect(),
            spare: self.spare.iter().map(|(&r, b)| (r, b.clone())).collect(),
            pages: self.pages.iter().map(|(&r, b)| (r, b.clone())).collect(),
            written_spare: self
                .written_spare
                .iter()
                .map(|(&r, b)| (r, b.clone()))
                .collect(),
        })
        .map_err(|e| e.to_string())
    }
    /// Imports media against this receiver's geometry and immutable base identity.
    /// Overlays and spare inputs come from the image.
    pub fn restore_committed(&self, bytes: &[u8]) -> Result<Self, String> {
        use sha2::{Digest, Sha256};
        if bytes.len() > 128 << 20 {
            return Err("NAND committed image exceeds bound".into());
        }
        let image: CommittedImage = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if image.schema != 1
            || image.options != self.options
            || image.base_bytes != self.base.len()
            || image.base_sha256 != format!("{:x}", Sha256::digest(&self.base))
        {
            return Err("NAND committed image identity or geometry differs".into());
        }
        let collect = |rows: Vec<(u32, Vec<u8>)>| -> Result<BTreeMap<u32, Vec<u8>>, String> {
            let mut map = BTreeMap::new();
            for (row, data) in rows {
                if map.insert(row, data).is_some() {
                    return Err("Duplicate NAND image row".into());
                }
            }
            Ok(map)
        };
        let mut nand = Self::new(
            self.options.clone(),
            self.base.clone(),
            image
                .overlays
                .into_iter()
                .map(|(offset, bytes)| Overlay {
                    offset,
                    bytes: bytes.into(),
                })
                .collect(),
            collect(image.spare)?,
        )?;
        let pages = collect(image.pages)?;
        let spare = collect(image.written_spare)?;
        for (&row, bytes) in &pages {
            nand.row(row)?;
            if bytes.len() != nand.options.page_bytes {
                return Err("NAND committed page extent differs".into());
            }
        }
        for (&row, bytes) in &spare {
            nand.row(row)?;
            if bytes.len() != nand.options.spare_bytes {
                return Err("NAND committed spare extent differs".into());
            }
        }
        nand.pages = pages;
        nand.written_spare = spare;
        Ok(nand)
    }
    pub fn same_base(&self, other: &Self) -> bool {
        self.options == other.options
            && (Arc::ptr_eq(&self.base, &other.base) || self.base == other.base)
    }
    pub fn options(&self) -> &Options {
        &self.options
    }
    pub fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), String> {
        if offset
            .checked_add(out.len() as u64)
            .is_none_or(|e| e > self.options.bytes)
        {
            return Err("NAND read outside medium".into());
        }
        out.fill(255);
        let base_end = (offset + out.len() as u64).min(self.base.len() as u64);
        if offset < base_end {
            out[..(base_end - offset) as usize]
                .copy_from_slice(&self.base[offset as usize..base_end as usize]);
        }
        for overlay in &self.overlays {
            let lo = offset.max(overlay.offset);
            let hi = (offset + out.len() as u64).min(overlay.offset + overlay.bytes.len() as u64);
            if lo < hi {
                out[(lo - offset) as usize..(hi - offset) as usize].copy_from_slice(
                    &overlay.bytes[(lo - overlay.offset) as usize..(hi - overlay.offset) as usize],
                );
            }
        }
        let page_bytes = self.options.page_bytes as u64;
        let end = (offset + out.len() as u64).div_ceil(page_bytes) as u32;
        for (row, bytes) in self.pages.range((offset / page_bytes) as u32..end) {
            let base = *row as u64 * page_bytes;
            let lo = offset.max(base);
            let hi = (offset + out.len() as u64).min(base + page_bytes);
            out[(lo - offset) as usize..(hi - offset) as usize]
                .copy_from_slice(&bytes[(lo - base) as usize..(hi - base) as usize]);
        }
        Ok(())
    }
    pub fn read_spare(&self, row: u32, out: &mut [u8]) -> Result<(), String> {
        self.row(row)?;
        if out.len() > self.options.spare_bytes {
            return Err("NAND spare read exceeds extent".into());
        }
        out.fill(255);
        if let Some(bytes) = self
            .written_spare
            .get(&row)
            .or_else(|| self.spare.get(&row))
        {
            let count = out.len().min(bytes.len());
            out[..count].copy_from_slice(&bytes[..count]);
        }
        Ok(())
    }
    fn row(&self, row: u32) -> Result<(), String> {
        if row as u64 >= self.options.bytes / self.options.page_bytes as u64 {
            Err("NAND row outside medium".into())
        } else {
            Ok(())
        }
    }
    /// Controller calls this only at program-confirm completion. Enforce NAND
    /// one-to-zero programming before committing either data or spare bytes.
    pub fn program(&mut self, row: u32, bytes: &[u8], spare: &[u8]) -> Result<(), String> {
        self.row(row)?;
        if bytes.len() != self.options.page_bytes || spare.len() > self.options.spare_bytes {
            return Err("NAND program geometry mismatch".into());
        }
        let mut old = [0; 2048];
        self.read(
            row as u64 * self.options.page_bytes as u64,
            &mut old[..bytes.len()],
        )?;
        let mut old_spare = [0; 512];
        self.read_spare(row, &mut old_spare[..spare.len()])?;
        if bytes.iter().zip(&old).any(|(n, o)| n & o != *n)
            || spare.iter().zip(&old_spare).any(|(n, o)| n & o != *n)
        {
            return Err("NAND program attempts zero-to-one transition without erase".into());
        }
        self.pages.insert(row, bytes.to_vec());
        if !spare.is_empty() {
            let mut complete = vec![255; self.options.spare_bytes];
            self.read_spare(row, &mut complete)?;
            complete[..spare.len()].copy_from_slice(spare);
            self.written_spare.insert(row, complete);
        }
        Ok(())
    }
    pub fn erase(&mut self, first_row: u32) -> Result<(), String> {
        self.row(first_row)?;
        if !first_row.is_multiple_of(self.options.pages_per_block) {
            return Err("NAND erase must start at block boundary".into());
        }
        for row in first_row..first_row + self.options.pages_per_block {
            self.pages.insert(row, vec![255; self.options.page_bytes]);
            self.written_spare
                .insert(row, vec![255; self.options.spare_bytes]);
        }
        Ok(())
    }
}
/// Managed-partition input tag. The board supplies physical placement and
/// logical index; neither is inferred by the part from firmware or RAM.
pub fn managed_tag(index: u16) -> Result<[u8; 16], String> {
    if index >= 0x8000 {
        return Err("Managed NAND tag index exceeds 15 bits".into());
    }
    let value = index << 1;
    let encoded = (value | (value.count_ones() as u16 & 1)).to_be_bytes();
    let mut tag = [255; 16];
    tag[..2].copy_from_slice(&encoded);
    tag[2..4].copy_from_slice(&encoded);
    Ok(tag)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_changes_bind_immutable_inputs_and_preserve_programmed_pages() {
        let initial = Nand::new(
            Options {
                bytes: 0x40000,
                page_bytes: 2048,
                pages_per_block: 64,
                spare_bytes: 64,
            },
            Arc::from([255; 4096]),
            vec![],
            BTreeMap::new(),
        )
        .unwrap();
        let mut written = initial.clone();
        written.program(0, &[42; 2048], &[17; 64]).unwrap();
        let encoded = serde_json::to_vec(&written.changes()).unwrap();
        let changes: Changes = serde_json::from_slice(&encoded).unwrap();
        let restored = initial.with_changes(&changes).unwrap();
        assert_eq!(restored.fingerprint(), written.fingerprint());
        assert_ne!(initial.fingerprint(), written.fingerprint());
        let mut bad = changes.clone();
        bad.pages.insert(500, vec![0; 2048]);
        assert!(initial.with_changes(&bad).is_err());
        bad = changes.clone();
        bad.written_spare.insert(0, vec![0; 63]);
        assert!(initial.with_changes(&bad).is_err());
        bad = changes;
        bad.identity = serde_json::Value::Null;
        assert!(initial.with_changes(&bad).is_err());
    }
    #[test]
    fn overlays_program_erase_and_power_cycle_retention() {
        let options = Options {
            bytes: 0x40000,
            page_bytes: 2048,
            pages_per_block: 64,
            spare_bytes: 64,
        };
        let tag = managed_tag(41).unwrap();
        let mut nand = Nand::new(
            options,
            Arc::from([0xff; 4096]),
            vec![Overlay {
                offset: 0x20000,
                bytes: Arc::from([0xa5; 2048]),
            }],
            BTreeMap::from([(64, tag.to_vec())]),
        )
        .unwrap();
        let mut data = [0; 2048];
        nand.read(0x20000, &mut data).unwrap();
        assert_eq!(data, [0xa5; 2048]);
        assert!(nand.program(64, &[255; 2048], &[]).is_err());
        nand.program(64, &[0x85; 2048], &[]).unwrap();
        let retained = nand.clone();
        retained.read(0x20000, &mut data).unwrap();
        assert_eq!(data, [0x85; 2048]);
        nand.erase(64).unwrap();
        nand.read(0x20000, &mut data).unwrap();
        assert_eq!(data, [255; 2048]);
        let mut spare = [0; 64];
        nand.read_spare(64, &mut spare).unwrap();
        assert_eq!(spare, [255; 64]);
        assert!(nand.erase(65).is_err());
        assert!(nand.read(0x40000, &mut data).is_err());
    }
    #[test]
    fn checkpoint_nand_shares_immutable_image_and_restores_programmed_pages() {
        use crate::checkpoint::Checkpoint;
        let base: Arc<[u8]> = vec![255; 65536].into();
        let mut medium = Nand::new(
            Options {
                bytes: 65536,
                page_bytes: 512,
                pages_per_block: 32,
                spare_bytes: 16,
            },
            base.clone(),
            vec![],
            Default::default(),
        )
        .unwrap();
        medium
            .program(3, &vec![0x55; 512], &vec![0x33; 16])
            .unwrap();
        let state = medium.fingerprint();
        let saved = medium.checkpoint().unwrap();
        let envelope = Checkpoint::encode("medium-keyframe", &saved).unwrap();
        let decoded: Checkpoint = envelope.decode("medium-keyframe").unwrap();
        assert_eq!(
            Nand::from_checkpoint(&decoded).unwrap().fingerprint(),
            state
        );
        assert!(Arc::ptr_eq(&base, &saved.blobs()[0]));
        medium.erase(0).unwrap();
        medium = Nand::from_checkpoint(&saved).unwrap();
        assert_eq!(medium.fingerprint(), state);
        assert!(Arc::ptr_eq(&base, &medium.checkpoint().unwrap().blobs()[0]));
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    #[test]
    fn external_initialization_is_atomic_and_refuses_programmed_media() {
        let mut medium = Nand::new(
            Options {
                bytes: 0x40000,
                page_bytes: 2048,
                pages_per_block: 64,
                spare_bytes: 64,
            },
            Arc::from([255; 2048]),
            vec![],
            BTreeMap::new(),
        )
        .unwrap();
        let initialized = medium
            .with_inputs(
                vec![Overlay {
                    offset: 0x20000,
                    bytes: Arc::from([0x85; 2048]),
                }],
                BTreeMap::from([(64, vec![0x12, 0x34])]),
            )
            .unwrap();
        assert!(
            initialized
                .with_inputs(
                    vec![Overlay {
                        offset: 0x20000,
                        bytes: Arc::from([0; 2048])
                    }],
                    BTreeMap::new()
                )
                .is_err()
        );
        let mut data = [0; 2048];
        medium.read(0x20000, &mut data).unwrap();
        assert_eq!(data, [255; 2048]);
        initialized.read(0x20000, &mut data).unwrap();
        assert_eq!(data, [0x85; 2048]);
        medium.program(0, &[0; 2048], &[]).unwrap();
        assert!(medium.with_inputs(vec![], BTreeMap::new()).is_err());
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    fn medium() -> Nand {
        Nand::new(
            Options {
                bytes: 262144,
                page_bytes: 2048,
                pages_per_block: 64,
                spare_bytes: 64,
            },
            Arc::from(vec![255; 2048]),
            vec![],
            BTreeMap::new(),
        )
        .unwrap()
    }
    #[test]
    fn committed_images_preserve_program_and_erase_and_refuse_wrong_identity_atomically() {
        let initial = medium();
        let mut written = initial.clone();
        written.program(64, &vec![0x85; 2048], &[]).unwrap();
        assert!(initial.same_base(&written));
        let different = Nand::new(
            initial.options.clone(),
            Arc::from([0; 2048]),
            vec![],
            BTreeMap::new(),
        )
        .unwrap();
        assert!(!initial.same_base(&different));
        let image = written.committed_image().unwrap();
        let restored = initial.restore_committed(&image).unwrap();
        let mut bytes = [0; 2048];
        restored.read(64 * 2048, &mut bytes).unwrap();
        assert_eq!(bytes, [0x85; 2048]);
        initial.read(64 * 2048, &mut bytes).unwrap();
        assert_eq!(bytes, [255; 2048]);
        let mut bad: serde_json::Value = serde_json::from_slice(&image).unwrap();
        bad["base_sha256"] = serde_json::json!("wrong");
        assert!(
            initial
                .restore_committed(&serde_json::to_vec(&bad).unwrap())
                .is_err()
        );
        bad = serde_json::from_slice(&image).unwrap();
        bad["pages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!([64, [1]]));
        assert!(
            initial
                .restore_committed(&serde_json::to_vec(&bad).unwrap())
                .is_err()
        );
        let mut erased = restored;
        erased.erase(64).unwrap();
        let cold = initial
            .restore_committed(&erased.committed_image().unwrap())
            .unwrap();
        cold.read(64 * 2048, &mut bytes).unwrap();
        assert_eq!(bytes, [255; 2048]);
    }
}
