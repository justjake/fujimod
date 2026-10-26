//! Host preparation of declared persistent inputs. Asset identity, placement,
//! byte edits and journal layout come from explicit typed run inputs. No guest execution,
//! camera transport or capture-pipeline module participates in preparation.
use fujimod_emulator_devices::parts::nand::{Nand, Options as Geometry, Overlay, managed_tag};
use fujimod_catalog_settings::persistent_inputs::{
    PersistentInputs, PersistentJournalAppend, PersistentPacket,
};
use std::{collections::BTreeMap, sync::Arc};

pub trait PreparePersistentInputs {
    fn mcu_packet(
        &self,
        set: &str,
        loaded: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Option<[u8; 16]>, String>;
    fn prepare(
        &self,
        geometry: Geometry,
        set: &str,
        loaded: &BTreeMap<String, Vec<u8>>,
        base: Arc<[u8]>,
    ) -> Result<Nand, String>;
}
/// Tag each whole block of a managed placement with its logical index, in the
/// spare area of the block's first and last pages.
pub(crate) fn tag_managed_blocks(
    geometry: &Geometry,
    physical_offset: u64,
    bytes: u64,
    index: u16,
    spare: &mut BTreeMap<u32, Vec<u8>>,
) -> Result<(), String> {
    let block_bytes = geometry.page_bytes as u64 * geometry.pages_per_block as u64;
    if physical_offset % block_bytes != 0 || !bytes.is_multiple_of(block_bytes) {
        return Err("Managed NAND input must cover whole blocks".into());
    }
    for n in 0..bytes / block_bytes {
        let logical = index
            .checked_add(n.try_into().map_err(|_| "Managed tag count overflow")?)
            .ok_or("Managed tag index overflow")?;
        let tag = managed_tag(logical)?.to_vec();
        let first = ((physical_offset / block_bytes + n) * geometry.pages_per_block as u64) as u32;
        spare.insert(first, tag.clone());
        spare.insert(first + geometry.pages_per_block - 1, tag);
    }
    Ok(())
}

impl PreparePersistentInputs for PersistentInputs {
    fn mcu_packet(
        &self,
        set: &str,
        loaded: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Option<[u8; 16]>, String> {
        self.validate()?;
        let set = self.sets.get(set).ok_or("Unknown persistent input set")?;
        let bytes = match &set.mcu_packet {
            None => return Ok(None),
            Some(PersistentPacket::Bytes { bytes }) => bytes.clone(),
            Some(PersistentPacket::Window {
                asset,
                offset,
                bytes,
            }) => {
                let prepared = self
                    .assets
                    .get(asset)
                    .ok_or("Unknown MCU asset")?
                    .prepare(loaded.get(asset).ok_or("Missing MCU asset")?)?;
                prepared
                    .get(*offset..offset.checked_add(*bytes).ok_or("MCU packet overflow")?)
                    .ok_or("MCU packet exceeds input")?
                    .to_vec()
            }
        };
        let packet: [u8; 16] = bytes
            .try_into()
            .map_err(|_| "MCU packet must contain sixteen bytes")?;
        if packet[14] != 0xaa || packet.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)) != 0 {
            return Err("MCU packet framing differs".into());
        }
        Ok(Some(packet))
    }
    /// Loaded values are bounded source windows, not whole capture files.
    fn prepare(
        &self,
        geometry: Geometry,
        set: &str,
        loaded: &BTreeMap<String, Vec<u8>>,
        base: Arc<[u8]>,
    ) -> Result<Nand, String> {
        self.validate()?;
        let set = self.sets.get(set).ok_or("Unknown persistent input set")?;
        Nand::new(geometry.clone(), base.clone(), Vec::new(), BTreeMap::new())?;
        let mut assets = BTreeMap::new();
        for name in set.placements.iter().map(|p| p.asset.as_str()).chain(
            set.append
                .iter()
                .flat_map(|a| [a.record_asset.as_str(), a.payload_asset.as_str()]),
        ) {
            if !assets.contains_key(name) {
                assets.insert(
                    name.to_owned(),
                    self.assets
                        .get(name)
                        .ok_or("Unknown persistent asset")?
                        .prepare(loaded.get(name).ok_or("Missing persistent asset")?)?,
                );
            }
        }
        let mut overlays = Vec::new();
        let mut spare = BTreeMap::new();
        for p in &set.placements {
            if p.physical_offset
                .checked_add(p.bytes as u64)
                .is_none_or(|end| end > geometry.bytes)
            {
                return Err("Persistent placement exceeds NAND geometry".into());
            }
            let data = assets[&p.asset]
                .get(
                    p.source_offset
                        ..p.source_offset
                            .checked_add(p.bytes)
                            .ok_or("Placement extent overflow")?,
                )
                .ok_or("Placement exceeds input window")?;
            overlays.push(Overlay {
                offset: p.physical_offset,
                bytes: Arc::from(data),
            });
            if let Some(index) = p.managed_index {
                tag_managed_blocks(
                    &geometry,
                    p.physical_offset,
                    p.bytes as u64,
                    index,
                    &mut spare,
                )?;
            }
        }
        if let Some(a) = &set.append {
            let payload = assets[&a.payload_asset]
                .get(
                    a.payload_offset
                        ..a.payload_offset
                            .checked_add(a.payload_bytes)
                            .ok_or("Journal payload overflow")?,
                )
                .ok_or("Journal payload exceeds input")?;
            let record = append_record(&assets[&a.record_asset], payload, a)?;
            let record_end = a
                .physical_offset
                .checked_add(record.len() as u64)
                .ok_or("Journal placement overflow")?;
            let mut found = false;
            for overlay in &mut overlays {
                if a.physical_offset >= overlay.offset
                    && record_end <= overlay.offset + overlay.bytes.len() as u64
                {
                    let at = (a.physical_offset - overlay.offset) as usize;
                    let bytes = Arc::make_mut(&mut overlay.bytes);
                    if bytes[at..at + record.len()].iter().any(|b| *b != 255) {
                        return Err("Journal append slot is not erased".into());
                    }
                    bytes[at..at + record.len()].copy_from_slice(&record);
                    found = true;
                    break;
                }
            }
            if !found {
                return Err("Journal append does not fit declared placement".into());
            }
        }
        Nand::new(geometry.clone(), base, overlays, spare)
    }
}
/// Stock journal format: increment LE16 sequence, then seal with one parity bit
/// per LE32 data word plus an XOR checksum. Layout is supplied by the firmware data.
fn append_record(
    record: &[u8],
    payload: &[u8],
    layout: &PersistentJournalAppend,
) -> Result<Vec<u8>, String> {
    if record.len() < 2
        || layout.record_payload_offset < 2
        || layout
            .record_payload_offset
            .checked_add(payload.len())
            .is_none_or(|e| e > layout.parity_offset)
        || layout.checksum_offset.checked_add(4) != Some(record.len())
    {
        return Err("Invalid journal record layout".into());
    }
    let mut result = record.to_vec();
    let sequence = u16::from_le_bytes(result[..2].try_into().unwrap()).wrapping_add(1);
    result[..2].copy_from_slice(&sequence.to_le_bytes());
    result[layout.record_payload_offset..layout.record_payload_offset + payload.len()]
        .copy_from_slice(payload);
    fujimod_settings::seal_journal(&mut result, layout.parity_offset)
        .map_err(|_| "Invalid journal record layout")?;
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_catalog_settings::persistent_inputs::{PersistentAsset, PersistentPatch};
    use sha2::{Digest, Sha256};
    #[test]
    fn authored_catalog_decodes_and_erased_inputs_need_no_assets() {
        let mut packet = vec![0; 16];
        packet[14] = 0xaa;
        packet[15] = 0x56;
        let catalog: PersistentInputs = serde_json::from_value(serde_json::json!({
            "assets":{},
            "sets":{"erased":{"placements":[],"append":null,
                "mcu_packet":{"kind":"bytes","bytes":packet}}}
        }))
        .unwrap();
        let nand = catalog
            .prepare(
                Geometry {
                    bytes: 16384,
                    page_bytes: 512,
                    pages_per_block: 32,
                    spare_bytes: 16,
                },
                "erased",
                &BTreeMap::new(),
                Arc::from([]),
            )
            .unwrap();
        let mut bytes = [0; 16];
        nand.read(0, &mut bytes).unwrap();
        assert_eq!(bytes, [255; 16]);
        let packet = catalog
            .mcu_packet("erased", &BTreeMap::new())
            .unwrap()
            .unwrap();
        assert_eq!(packet[14], 0xaa);
        assert!(
            catalog
                .prepare(
                    Geometry {
                        bytes: 16384,
                        page_bytes: 512,
                        pages_per_block: 32,
                        spare_bytes: 16
                    },
                    "absent",
                    &BTreeMap::new(),
                    Arc::from([])
                )
                .is_err()
        );
    }
    #[test]
    fn patches_are_identity_checked_and_journal_parity_is_recomputed() {
        let source = [1u8, 2, 3];
        let normalized = [1u8, 5, 3];
        let asset = PersistentAsset {
            bytes: 3,
            sha256: format!("{:x}", Sha256::digest(source)),
            normalized_sha256: format!("{:x}", Sha256::digest(normalized)),
            patches: vec![PersistentPatch {
                offset: 1,
                bytes: vec![5],
            }],
        };
        assert_eq!(asset.prepare(&source).unwrap(), normalized);
        assert_eq!(asset.prepare(&normalized).unwrap(), normalized);
        assert!(asset.prepare(&[1, 2, 4]).is_err());
        let layout = PersistentJournalAppend {
            record_asset: "r".into(),
            payload_asset: "p".into(),
            payload_offset: 0,
            payload_bytes: 4,
            record_payload_offset: 4,
            parity_offset: 16,
            checksum_offset: 20,
            physical_offset: 0,
        };
        let mut record = [0; 24];
        record[..2].copy_from_slice(&65535u16.to_le_bytes());
        let updated = append_record(&record, &[1, 0, 0, 0], &layout).unwrap();
        assert_eq!(&updated[..2], &[0, 0]);
        assert_eq!(updated[16], 2);
        assert_eq!(&updated[20..], &1u32.to_le_bytes());
    }
}
