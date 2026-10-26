//! An external SDHC medium. Capacity and identity are declared inputs;
//! missing sectors contain zeros. Media outlives controller resets.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod faults;

pub const SECTOR: usize = 512;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub sectors: u32,
    pub max_dirty_sectors: usize,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Card {
    sectors: u32,
    max_dirty: usize,
    #[serde(with = "sector_bytes")]
    data: BTreeMap<u32, Vec<u8>>,
    revision: u64,
}

mod sector_bytes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        sectors: &BTreeMap<u32, Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        #[derive(Serialize)]
        struct Bytes<'a>(#[serde(with = "crate::checkpoint::bytes")] &'a [u8]);
        let mut map = serializer.serialize_map(Some(sectors.len()))?;
        for (address, bytes) in sectors {
            map.serialize_entry(address, &Bytes(bytes))?;
        }
        map.end()
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<u32, Vec<u8>>, D::Error> {
        BTreeMap::deserialize(deserializer)
    }
}
impl Card {
    pub fn fingerprint(&self) -> serde_json::Value {
        use sha2::{Digest, Sha256};
        let Self {
            sectors,
            max_dirty,
            data,
            revision,
        } = self;
        let mut hash = Sha256::new();
        hash.update(b"sd-card-sectors-le-v1\0");
        hash.update((data.len() as u64).to_le_bytes());
        for (lba, bytes) in data {
            hash.update(lba.to_le_bytes());
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        serde_json::json!({"sectors":sectors,"max_dirty":max_dirty,"revision":revision,
            "dirty_sectors":data.len(),"data":{"encoding":"sparse-sectors-le-v1","sha256":format!("{:x}",hash.finalize())}})
    }
    pub fn new(options: Options) -> Result<Self, String> {
        if !(0x400000..=0x4000000).contains(&options.sectors)
            || !options.sectors.is_multiple_of(1024)
            || !(1..=1 << 21).contains(&options.max_dirty_sectors)
        {
            return Err(
                "SDHC requires 2..32 GiB in 1024-sector units and a bounded sparse allocation"
                    .into(),
            );
        }
        Ok(Self {
            sectors: options.sectors,
            max_dirty: options.max_dirty_sectors,
            data: BTreeMap::new(),
            revision: 0,
        })
    }
    /// Validate deserialized external media before attaching it to a controller.
    pub fn validate(&self) -> Result<(), String> {
        Self::new(Options {
            sectors: self.sectors,
            max_dirty_sectors: self.max_dirty,
        })?;
        if self.data.len() > self.max_dirty
            || self
                .data
                .iter()
                .any(|(&lba, bytes)| lba >= self.sectors || bytes.len() != SECTOR)
        {
            return Err("SD durable sector extent differs".into());
        }
        Ok(())
    }
    pub fn sectors(&self) -> u32 {
        self.sectors
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn initialize_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
    pub fn supersede(&mut self, revision: u64) -> Result<(), String> {
        self.revision = self
            .revision
            .max(revision.checked_add(1).ok_or("SD revision overflow")?);
        Ok(())
    }
    pub fn nonzero_sectors(&self) -> impl Iterator<Item = (u32, &[u8])> {
        self.data
            .iter()
            .map(|(&lba, bytes)| (lba, bytes.as_slice()))
    }
    pub fn dirty_sectors(&self) -> usize {
        self.data.len()
    }
    pub fn range(&self, first: u32, count: u32) -> Result<(), String> {
        if count == 0
            || count > 65536
            || first
                .checked_add(count)
                .is_none_or(|end| end > self.sectors)
        {
            return Err("SD sector range exceeds card or transaction bound".into());
        }
        Ok(())
    }
    pub fn sector(&self, lba: u32) -> Result<&[u8], String> {
        if lba >= self.sectors {
            return Err("SD sector beyond card capacity".into());
        }
        static ZERO: [u8; SECTOR] = [0; SECTOR];
        Ok(self.data.get(&lba).map_or(&ZERO, Vec::as_slice))
    }
    pub fn read_bytes(&self, offset: u64, out: &mut [u8]) -> Result<(), String> {
        self.byte_range(offset, out.len())?;
        let mut position = offset;
        for chunk in out.chunks_mut(SECTOR) {
            // The first sector may be partial, so copy with an independent cursor.
            let mut used = 0;
            while used < chunk.len() {
                let within = position as usize % SECTOR;
                let size = (SECTOR - within).min(chunk.len() - used);
                chunk[used..used + size].copy_from_slice(
                    &self.sector((position / SECTOR as u64) as u32)?[within..within + size],
                );
                position += size as u64;
                used += size;
            }
        }
        Ok(())
    }
    fn byte_range(&self, offset: u64, len: usize) -> Result<(), String> {
        if len == 0
            || len > 65536 * SECTOR
            || offset
                .checked_add(len as u64)
                .is_none_or(|end| end > self.sectors as u64 * SECTOR as u64)
        {
            return Err("SD byte extent exceeds card or transaction bound".into());
        }
        Ok(())
    }
    pub fn reserve_write(&self, first: u32, count: u32) -> Result<(), String> {
        self.range(first, count)?;
        let replaced = self.data.range(first..first + count).count();
        if self.data.len() - replaced + count as usize > self.max_dirty {
            return Err("SD write exceeds sparse allocation bound".into());
        }
        Ok(())
    }
    pub fn write_bytes(&mut self, offset: u64, bytes: &[u8]) -> Result<(), String> {
        self.byte_range(offset, bytes.len())?;
        // Preflight the exact resulting nonzero count before mutating any sector.
        let first = (offset / SECTOR as u64) as u32;
        let last = ((offset + bytes.len() as u64 - 1) / SECTOR as u64) as u32;
        let mut retained = self.data.len();
        let mut block = [0; SECTOR];
        let mut changed = false;
        for lba in first..=last {
            self.updated_sector(lba, offset, bytes, &mut block)?;
            changed |= self.sector(lba)? != block;
            retained -= usize::from(self.data.contains_key(&lba));
            retained += usize::from(block.iter().any(|b| *b != 0));
        }
        if retained > self.max_dirty {
            return Err("SD sparse allocation bound exceeded".into());
        }
        let revision = if changed {
            self.revision.checked_add(1).ok_or("SD revision overflow")?
        } else {
            self.revision
        };
        for lba in first..=last {
            self.updated_sector(lba, offset, bytes, &mut block)?;
            if self.sector(lba)? == block {
                continue;
            }
            if block.iter().all(|b| *b == 0) {
                self.data.remove(&lba);
            } else {
                self.data
                    .entry(lba)
                    .or_insert_with(|| vec![0; SECTOR])
                    .copy_from_slice(&block);
            }
        }
        self.revision = revision;
        Ok(())
    }
    fn updated_sector(
        &self,
        lba: u32,
        offset: u64,
        bytes: &[u8],
        block: &mut [u8; SECTOR],
    ) -> Result<(), String> {
        block.copy_from_slice(self.sector(lba)?);
        let base = lba as u64 * SECTOR as u64;
        let lo = base.max(offset);
        let hi = (base + SECTOR as u64).min(offset + bytes.len() as u64);
        block[(lo - base) as usize..(hi - base) as usize]
            .copy_from_slice(&bytes[(lo - offset) as usize..(hi - offset) as usize]);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fingerprint_bounds_sparse_bytes_and_retains_addresses_content_and_metadata() {
        let mut a = Card::new(Options {
            sectors: 0x800000,
            max_dirty_sectors: 4096,
        })
        .unwrap();
        a.write_bytes(512, &vec![0xa5; 2 * 1024 * 1024]).unwrap();
        let identity = a.fingerprint();
        assert!(serde_json::to_vec(&identity).unwrap().len() < 350);
        assert_eq!(a.fingerprint(), identity);
        let mut b = a.clone();
        b.data.get_mut(&1).unwrap()[511] ^= 1;
        assert_ne!(b.fingerprint(), identity);
        b = a.clone();
        let bytes = b.data.remove(&1).unwrap();
        b.data.insert(4097, bytes);
        assert_ne!(b.fingerprint(), identity);
        b = a.clone();
        b.revision += 1;
        assert_ne!(b.fingerprint(), identity);
        b = a.clone();
        b.max_dirty += 1;
        assert_ne!(b.fingerprint(), identity);
        b = a.clone();
        b.sectors += 1024;
        assert_ne!(b.fingerprint(), identity);
    }
    #[test]
    fn sector_checkpoint_uses_bulk_bytes_without_changing_card_identity() {
        let mut card = Card::new(Options {
            sectors: 0x800000,
            max_dirty_sectors: 8,
        })
        .unwrap();
        let payload: Vec<u8> = (0..=255).cycle().take(1024).collect();
        card.write_bytes(512, &payload).unwrap();
        let before = serde_json::to_value(&card).unwrap();
        let saved = crate::checkpoint::Checkpoint::encode("card", &card).unwrap();
        assert!(saved.bytes() < 1200);
        let mut restored: Card = saved.decode("card").unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), before);
        let mut actual = vec![0; 1024];
        restored.read_bytes(512, &mut actual).unwrap();
        assert_eq!(actual, payload);
        restored.write_bytes(512, &[0xff; 512]).unwrap();
        card.read_bytes(512, &mut actual).unwrap();
        assert_eq!(actual, payload);
    }
    #[test]
    fn exhausted_revision_refuses_changed_bytes_before_committing() {
        let mut card = Card::new(Options {
            sectors: 0x800000,
            max_dirty_sectors: 2,
        })
        .unwrap();
        card.initialize_revision(u64::MAX);
        card.write_bytes(0, &[0; 512]).unwrap();
        assert!(
            card.write_bytes(510, &[1, 2, 3, 4])
                .unwrap_err()
                .contains("revision")
        );
        assert_eq!(card.dirty_sectors(), 0);
        assert_eq!(card.revision(), u64::MAX);
        assert_eq!(card.sector(0).unwrap(), &[0; 512]);
        assert_eq!(card.sector(1).unwrap(), &[0; 512]);
    }
    #[test]
    fn sparse_capacity_atomicity_and_partial_sector_writes() {
        let mut card = Card::new(Options {
            sectors: 0x800000,
            max_dirty_sectors: 1,
        })
        .unwrap();
        card.write_bytes(510, &[1, 2]).unwrap();
        let revision = card.revision();
        assert!(card.write_bytes(512, &[3]).is_err());
        assert_eq!(card.revision(), revision);
        assert_eq!(&card.sector(0).unwrap()[510..], &[1, 2]);
        card.write_bytes(510, &[1, 2]).unwrap();
        assert_eq!(card.revision(), revision);
        card.write_bytes(510, &[0, 0]).unwrap();
        card.write_bytes(512, &[3]).unwrap();
        let mut bytes = [0; 5];
        card.read_bytes(510, &mut bytes).unwrap();
        assert_eq!(bytes, [0, 0, 3, 0, 0]);
        assert!(
            card.read_bytes(card.sectors() as u64 * 512, &mut bytes)
                .is_err()
        );
    }
}
