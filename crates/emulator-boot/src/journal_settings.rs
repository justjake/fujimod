//! Requested camera settings placed in the user-profile journal of a NOR image.
//!
//! An application-profile plan supplies the camera's NOR. A record that
//! already holds sealed copies keeps every byte except the requested ones:
//! each sealed copy takes the encoded values and is sealed again, so the
//! firmware reads them whichever copy it trusts. A record with no copy yet
//! takes the record `fujimod_settings::build` makes from the update's defaults.
use fujimod_catalog_resolution::ResolvedConfiguration;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A NOR array's region unit, which `ProfileRecord::block` counts.
const NOR_UNIT: u64 = 0x1_0000;

/// Place `requested` in the journal records of `nor`. Returns what changed, per record.
pub(crate) fn place(
    resolved: &ResolvedConfiguration,
    nor: &mut [u8],
    requested: &BTreeMap<String, String>,
) -> Result<Value, String> {
    let layout = resolved
        .reference
        .settings
        .as_ref()
        .ok_or("selected firmware has no settings layout")?;
    let built = fujimod_settings::build(layout, nor, requested, None)?;
    let profile = layout
        .profile
        .as_ref()
        .ok_or_else(|| format!("{} has no user profile", layout.firmware_id))?;
    let built = built
        .profile
        .ok_or("the settings layout built no user profile")?;
    let mut changes: BTreeMap<&str, Vec<(u64, Vec<u8>)>> = BTreeMap::new();
    for (name, value) in requested {
        let setting = layout
            .settings
            .get(name)
            .ok_or_else(|| format!("unknown camera setting {name}"))?;
        if !profile.records.contains_key(&setting.store) {
            return Err(format!(
                "camera setting {name} is stored in the {}, which an application-profile run does not place",
                setting.store
            ));
        }
        let value = setting
            .values
            .get(value)
            .ok_or_else(|| format!("camera setting {name} has no value {value}"))?;
        changes
            .entry(&setting.store)
            .or_default()
            .push((setting.offset, value.clone()));
    }
    let mut placed = Vec::new();
    for (store, bytes) in changes {
        let record = &profile.records[store];
        let fresh = built
            .records
            .iter()
            .find(|candidate| candidate.block == record.block)
            .ok_or("the settings layout built no record for a requested store")?;
        let geometry = Journal {
            block: record.block,
            size: record.bytes,
            parity: record.parity_offset,
        };
        placed.push(place_record(store, &geometry, &fresh.bytes, &bytes, nor)?);
    }
    Ok(Value::Array(placed))
}

/// Where one record's sealed copies lie: back to back from the start of its block.
struct Journal {
    block: u32,
    size: u64,
    parity: u64,
}

/// Patch every sealed copy of one record, or write `fresh` when the journal holds none.
fn place_record(
    store: &str,
    record: &Journal,
    fresh: &[u8],
    changes: &[(u64, Vec<u8>)],
    nor: &mut [u8],
) -> Result<Value, String> {
    let size = usize::try_from(record.size).map_err(|_| "profile record is too large")?;
    let parity = usize::try_from(record.parity).map_err(|_| "parity offset is too large")?;
    let start = usize::try_from(u64::from(record.block) * NOR_UNIT)
        .map_err(|_| "record block is too large")?;
    let mut copies = Vec::new();
    let mut slot = start;
    while let Some(copy) = nor.get(slot..slot + size) {
        if copy.iter().all(|&byte| byte == 0xff) {
            break;
        }
        let mut sealed = copy.to_vec();
        fujimod_settings::seal_journal(&mut sealed, parity)?;
        if sealed != copy {
            return Err(format!(
                "the {store} record copy at {slot:#x} is not sealed"
            ));
        }
        copies.push(slot);
        slot += size;
    }
    if copies.is_empty() {
        nor.get_mut(start..start + fresh.len())
            .ok_or("the profile record is outside the NOR")?
            .copy_from_slice(fresh);
        return Ok(json!({"store": store, "built_at": start}));
    }
    let mut patched = Vec::new();
    for slot in copies {
        let copy = &mut nor[slot..slot + size];
        for (offset, bytes) in changes {
            let offset = usize::try_from(*offset).map_err(|_| "setting offset is too large")?;
            copy.get_mut(offset..offset + bytes.len())
                .filter(|_| offset + bytes.len() <= parity)
                .ok_or("a requested setting lies outside the record's data words")?
                .copy_from_slice(bytes);
        }
        fujimod_settings::seal_journal(copy, parity)?;
        patched.push(json!({"address": slot, "copy": u32::from_le_bytes(copy[..4].try_into().expect("four bytes"))}));
    }
    Ok(json!({"store": store, "copies": patched}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(size: u64, parity: u64) -> Journal {
        Journal {
            block: 1,
            size,
            parity,
        }
    }

    fn sealed(number: u32, fill: u8) -> Vec<u8> {
        let mut copy = vec![fill; 64];
        copy[..4].copy_from_slice(&number.to_le_bytes());
        fujimod_settings::seal_journal(&mut copy, 48).unwrap();
        copy
    }

    #[test]
    fn every_sealed_copy_takes_the_setting_and_is_sealed_again() {
        let mut nor = vec![0xff; 0x2_0000];
        nor[0x1_0000..0x1_0040].copy_from_slice(&sealed(4, 7));
        nor[0x1_0040..0x1_0080].copy_from_slice(&sealed(5, 7));
        nor[0x1_00c0..0x1_0100].copy_from_slice(&sealed(9, 7));
        let before = nor.clone();
        let placed =
            place_record("main", &record(64, 48), &[0; 64], &[(9, vec![2])], &mut nor).unwrap();
        assert_eq!(
            placed,
            json!({"store": "main", "copies": [
                {"address": 0x1_0000, "copy": 4}, {"address": 0x1_0040, "copy": 5},
            ]})
        );
        for (at, number) in [(0x1_0000, 4), (0x1_0040, 5)] {
            let mut expected = sealed(number, 7);
            expected[9] = 2;
            fujimod_settings::seal_journal(&mut expected, 48).unwrap();
            assert_eq!(&nor[at..at + 64], expected.as_slice());
        }
        // The scan stops at the first erased slot, so the copy past it is untouched.
        assert_eq!(nor[0x1_0080..], before[0x1_0080..]);
        assert_eq!(nor[..0x1_0000], before[..0x1_0000]);
    }

    #[test]
    fn an_empty_journal_takes_the_built_record_and_a_broken_copy_is_refused() {
        let mut nor = vec![0xff; 0x2_0000];
        let fresh = sealed(0, 1);
        let placed =
            place_record("main", &record(64, 48), &fresh, &[(9, vec![2])], &mut nor).unwrap();
        assert_eq!(placed, json!({"store": "main", "built_at": 0x1_0000}));
        assert_eq!(&nor[0x1_0000..0x1_0040], fresh.as_slice());
        nor[0x1_0005] ^= 1;
        assert!(
            place_record("main", &record(64, 48), &fresh, &[(9, vec![2])], &mut nor)
                .unwrap_err()
                .contains("is not sealed")
        );
        let mut nor = vec![0xff; 0x2_0000];
        nor[0x1_0000..0x1_0040].copy_from_slice(&sealed(1, 0));
        assert!(
            place_record("main", &record(64, 48), &fresh, &[(50, vec![2])], &mut nor)
                .unwrap_err()
                .contains("outside the record's data words")
        );
    }
}
