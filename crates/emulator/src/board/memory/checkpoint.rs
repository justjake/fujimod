//! Physical memory archives retain every backing and its current bus views.
//! Region geometry comes from the receiving board before page allocation.
use super::{BusMapping, Checkpoint, Memory};
use fujimod_emulator_cpu::memory::pages::Image;
use fujimod_emulator_devices::checkpoint::Checkpoint as Encoded;
use serde::{Deserialize, Serialize};

const KIND: &str = "board-memory-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    regions: Vec<(u64, usize)>,
    mappings: Vec<(u64, u64, usize, u32)>,
}

impl Checkpoint {
    pub(in crate::board) fn export(&self) -> Result<Encoded, String> {
        let metadata = Metadata {
            regions: self
                .regions
                .iter()
                .map(|(base, image)| (*base, image.len()))
                .collect(),
            mappings: self
                .mappings
                .iter()
                .map(|map| (map.address, map.physical, map.size, map.permissions))
                .collect(),
        };
        let blobs = self
            .regions
            .iter()
            .map(|(_, image)| image.to_bytes().map(Into::into))
            .collect::<Result<_, _>>()?;
        Ok(Encoded::encode(KIND, &metadata)?.with_blobs(blobs))
    }
}

impl Memory {
    pub(in crate::board) fn import_checkpoint(
        &self,
        encoded: &Encoded,
    ) -> Result<Checkpoint, String> {
        let metadata: Metadata = encoded.decode(KIND)?;
        if !encoded.children().is_empty()
            || metadata.regions.len() != encoded.blobs().len()
            || metadata.regions.iter().copied().ne(self
                .regions
                .iter()
                .map(|(base, backing)| (*base, backing.len())))
        {
            return Err("snapshot physical memory geometry differs".into());
        }
        let mappings = metadata
            .mappings
            .into_iter()
            .map(|(address, physical, size, permissions)| BusMapping {
                address,
                physical,
                size,
                permissions,
            })
            .collect::<Vec<_>>();
        super::validate_mappings(
            &mappings,
            &self
                .regions
                .iter()
                .map(|(base, backing)| (*base, *base + backing.len() as u64))
                .collect::<Vec<_>>(),
        )?;
        Ok(Checkpoint {
            regions: metadata
                .regions
                .iter()
                .zip(encoded.blobs())
                .map(|(&(base, len), bytes)| Ok((base, Image::from_bytes(bytes, len)?)))
                .collect::<Result<_, String>>()?,
            mappings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_cpu::{exclusive, memory::Backing};

    fn memory() -> Memory {
        Memory::new(
            vec![
                (0x1000, Backing::zeros(0x2000)),
                (0x8000, Backing::zeros(7)),
            ],
            vec![BusMapping::ram(0x1000, 0x2000), BusMapping::ram(0x8000, 7)],
        )
        .unwrap()
    }

    #[test]
    fn memory_archive_round_trip_uses_fresh_backing_and_restores_bus_aliases() {
        let encoded = {
            let source = memory();
            let domain = exclusive::Shared::default();
            source.write_host(&[], &domain, 0x1000, &[7; 3]).unwrap();
            source.write_host(&[], &domain, 0x8004, &[1, 2, 3]).unwrap();
            source.mappings.borrow_mut().push(BusMapping {
                address: 0x4000,
                physical: 0x1000,
                size: 0x2000,
                permissions: 1,
            });
            let saved = source.checkpoint();
            let encoded = saved.export().unwrap();
            // Serialize the complete envelope, including its binary blobs.
            // The source allocations and immutable images then leave scope.
            encoded.to_bytes().unwrap()
        };
        let archive = Encoded::from_bytes(&encoded).unwrap();
        let target = memory();
        let before = target.fingerprint();
        let saved = target.import_checkpoint(&archive).unwrap();
        assert_eq!(target.fingerprint(), before);
        let domain = exclusive::Shared::default();
        target.restore_checkpoint(&saved, &[], &domain).unwrap();
        let mut bytes = [0; 7];
        target.read_host(0x4000, &mut bytes).unwrap();
        assert_eq!(bytes, [7, 7, 7, 0, 0, 0, 0]);
        target.read_host(0x8000, &mut bytes).unwrap();
        assert_eq!(bytes, [0, 0, 0, 0, 1, 2, 3]);
        assert!(target.write_dma(&[], &domain, 0x4000, &[9]).is_err());
        target.write_dma(&[], &domain, 0x1000, &[9]).unwrap();
        target.read_host(0x4000, &mut bytes).unwrap();
        assert_eq!(bytes[0], 9);
    }

    #[test]
    fn mismatched_or_corrupt_memory_archives_leave_the_target_untouched() {
        let source = memory();
        let archive = source.checkpoint().export().unwrap();
        let target = memory();
        let before = target.fingerprint();
        let mut metadata: Metadata = archive.decode(KIND).unwrap();
        metadata.regions[0].1 += 1;
        let bad = Encoded::encode(KIND, &metadata)
            .unwrap()
            .with_blobs(archive.blobs().to_vec());
        assert!(target.import_checkpoint(&bad).is_err());
        let mut metadata: Metadata = archive.decode(KIND).unwrap();
        metadata.mappings[0].1 = 0;
        let bad = Encoded::encode(KIND, &metadata)
            .unwrap()
            .with_blobs(archive.blobs().to_vec());
        assert!(target.import_checkpoint(&bad).is_err());
        assert!(
            target
                .import_checkpoint(&archive.clone().with_blobs(vec![]))
                .is_err()
        );
        assert!(
            target
                .import_checkpoint(&archive.clone().with_children(vec![archive.clone()]))
                .is_err()
        );
        let mut blobs = archive.blobs().to_vec();
        blobs[0] = vec![0; 24].into();
        assert!(
            target
                .import_checkpoint(&archive.with_blobs(blobs))
                .is_err()
        );
        assert_eq!(target.fingerprint(), before);
    }
}
