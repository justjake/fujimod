//! Durable reservations use processor construction order rather than allocated
//! owner IDs. The caller must first match the complete processor/device graph.
use super::{Monitors, Reservation, Slot};

const MAGIC: &[u8; 8] = b"FJMONI01";
const HEADER: usize = 32;
const SLOT: usize = 24;

impl super::Saved {
    pub fn to_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(40);
        bytes.extend_from_slice(b"FJEXCL01");
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&self.reservation.map_or(0, |r| r.address).to_le_bytes());
        bytes.extend_from_slice(&(self.reservation.map_or(0, |r| r.size) as u64).to_le_bytes());
        bytes
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 40 || &bytes[..8] != b"FJEXCL01" {
            return Err("invalid CPU exclusive snapshot format".into());
        }
        let address = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        let size = usize::try_from(u64::from_le_bytes(bytes[32..40].try_into().unwrap()))
            .map_err(|_| "CPU exclusive snapshot size exceeds target")?;
        if (size == 0 && address != 0) || address.checked_add(size as u64).is_none() {
            return Err("invalid CPU exclusive snapshot reservation".into());
        }
        Ok(Self {
            epoch: u128::from_le_bytes(bytes[8..24].try_into().unwrap()),
            reservation: (size != 0).then_some(Reservation { address, size }),
        })
    }
}

impl Monitors {
    pub fn export_checkpoint(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER + self.cores.len() * SLOT);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&(self.cores.len() as u64).to_le_bytes());
        for slot in self.cores.values() {
            bytes.extend_from_slice(&slot.granule.to_le_bytes());
            bytes.extend_from_slice(&slot.reservation.map_or(0, |r| r.address).to_le_bytes());
            bytes.extend_from_slice(&(slot.reservation.map_or(0, |r| r.size) as u64).to_le_bytes());
        }
        bytes
    }

    /// Rebind a durable domain to this domain's owners. Decoding never changes
    /// live reservations; the returned value supports normal whole-board restore.
    pub fn import_checkpoint(&self, bytes: &[u8]) -> Result<Self, String> {
        if bytes.get(..8) != Some(MAGIC) {
            return Err("unknown exclusive snapshot format".into());
        }
        if bytes.len() != HEADER + self.cores.len() * SLOT
            || u64::from_le_bytes(bytes[24..32].try_into().unwrap()) != self.cores.len() as u64
        {
            return Err("exclusive snapshot processor count differs".into());
        }
        let mut cores = self.cores.clone();
        for (slot, words) in cores.values_mut().zip(bytes[HEADER..].chunks_exact(SLOT)) {
            let word = |at| u64::from_le_bytes(words[at..at + 8].try_into().unwrap());
            let granule = word(0);
            let address = word(8);
            let size =
                usize::try_from(word(16)).map_err(|_| "exclusive snapshot size exceeds target")?;
            if granule != slot.granule {
                return Err("exclusive snapshot reservation granule differs".into());
            }
            if (size == 0 && address != 0) || address.checked_add(size as u64).is_none() {
                return Err("invalid exclusive snapshot reservation".into());
            }
            *slot = Slot {
                granule,
                reservation: (size != 0).then_some(Reservation { address, size }),
            };
        }
        Ok(Self {
            epoch: u128::from_le_bytes(bytes[8..24].try_into().unwrap()),
            cores,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_reservation_archive_keeps_wide_epochs_and_write_conflicts() {
        let mut domain = Monitors::default();
        let owner = domain.register(64).unwrap();
        domain.epoch = (1 << 80) + 3;
        domain.load(owner, 0x1000, 4);
        let bytes = domain.save(owner).to_bytes();
        let saved = super::super::Saved::from_bytes(&bytes).unwrap();
        assert_eq!(saved.to_bytes(), bytes);
        assert!(domain.store(owner, 0x1000, 4));
        domain.restore(owner, saved);
        assert!(domain.store(owner, 0x1000, 4));
        domain.written(None).unwrap();
        domain.restore(owner, saved);
        assert!(!domain.store(owner, 0x1000, 4));
        for end in [0, 7, 8, 24, 39] {
            assert!(super::super::Saved::from_bytes(&bytes[..end]).is_err());
        }
        let mut bad = bytes.clone();
        bad[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(super::super::Saved::from_bytes(&bad).is_err());
        let mut bad = bytes;
        bad[32..40].fill(0);
        assert!(super::super::Saved::from_bytes(&bad).is_err());
    }

    #[test]
    fn durable_domain_rebinds_owners_and_retains_exclusive_write_semantics() {
        let mut source = Monitors::default();
        let a = source.register(64).unwrap();
        let b = source.register(128).unwrap();
        source.written(None).unwrap();
        source.load(a, 0x1000, 4);
        source.load(b, 0x2000, 8);
        let encoded = source.export_checkpoint();
        let mut target = Monitors::default();
        let fresh_a = target.register(64).unwrap();
        let fresh_b = target.register(128).unwrap();
        assert_ne!(a, fresh_a);
        assert!(target.validate_checkpoint(&source).is_err());
        let saved = target.import_checkpoint(&encoded).unwrap();
        assert!(!target.active());
        target.restore_checkpoint(&saved).unwrap();
        assert_eq!(target.export_checkpoint(), encoded);
        let cpu_saved = target.save(fresh_a);
        assert!(target.store(fresh_a, 0x1000, 4));
        target.restore(fresh_a, cpu_saved);
        target.written(Some((0x103f, 1))).unwrap();
        assert!(!target.store(fresh_a, 0x1000, 4));
        target.restore(fresh_a, cpu_saved);
        assert!(!target.store(fresh_a, 0x1000, 4));
        assert!(target.store(fresh_b, 0x2000, 8));
    }

    #[test]
    fn malformed_domain_or_different_topology_does_not_change_live_state() {
        let mut target = Monitors::default();
        let owner = target.register(64).unwrap();
        target.load(owner, 0x1000, 4);
        let before = target.export_checkpoint();
        for end in [0, 7, 8, 24, 31, before.len() - 1] {
            assert!(target.import_checkpoint(&before[..end]).is_err());
        }
        for (at, value) in [
            (24, 2u64),
            (HEADER, 128),
            (HEADER + 8, u64::MAX),
            (HEADER + 16, 0),
        ] {
            let mut bad = before.clone();
            bad[at..at + 8].copy_from_slice(&value.to_le_bytes());
            assert!(target.import_checkpoint(&bad).is_err());
        }
        let mut bad = before.clone();
        bad.push(0);
        assert!(target.import_checkpoint(&bad).is_err());
        assert_eq!(target.export_checkpoint(), before);
    }
}
