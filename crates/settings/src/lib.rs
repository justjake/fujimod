//! Camera settings built from a firmware's catalog layout and its update file.
//!
//! A bank starts as zero bytes, takes the stock defaults table read from the
//! update, then the layout's fields and the requested named settings. When the
//! layout locates the user profile, its header and journal records are built
//! the same way from the stock profile defaults. Stock firmware reads the
//! result as it would a camera's own settings.
#![forbid(unsafe_code)]
use fujimod_catalog_settings::{FirmwareSettings, ProfileFieldSource, SectionLength, UpdateRange};
use std::collections::BTreeMap;

/// Settings ready to place before cold boot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub bank: Bank,
    pub profile: Option<Profile>,
}

/// The user profile: the power controller's header and the flash journal records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub header: [u8; 16],
    pub records: Vec<Record>,
}

/// A sealed journal record and its flash block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub bytes: Vec<u8>,
    pub block: u32,
    pub managed_index: u16,
}

/// A settings bank ready to place in flash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bank {
    pub bytes: Vec<u8>,
    pub first_block: u32,
    pub blocks: u32,
    pub logical_first: u16,
}

/// The installed flash image of an update file: its bytes after the flash
/// offset, inverted.
pub fn flash_image(update: &[u8], flash_offset: u64) -> Result<Vec<u8>, String> {
    let offset = usize::try_from(flash_offset).map_err(|_| "flash offset overflow")?;
    let installed = update
        .get(offset..)
        .ok_or("update is shorter than its flash offset")?;
    Ok(fujimod_firmware::invert(installed))
}

/// The layout's source section, decoded from the installed flash image and checked.
fn source(layout: &FirmwareSettings, flash: &[u8]) -> Result<Vec<u8>, String> {
    let section = &layout.source;
    let stream = usize::try_from(section.flash_offset)
        .ok()
        .and_then(|offset| flash.get(offset..))
        .ok_or("settings source section is outside the flash image")?;
    let (mut bytes, _) = fujimod_firmware::decompress(stream).map_err(|error| error.to_string())?;
    bytes.truncate(section.size as usize);
    if fujimod_firmware::sha256(&bytes) != section.sha256 {
        return Err("settings source section differs from the layout".into());
    }
    Ok(bytes)
}

fn read(section: &[u8], layout: &FirmwareSettings, range: &UpdateRange) -> Result<Vec<u8>, String> {
    let start = (range.address - layout.source.address) as usize;
    let bytes = section[start..start + range.size as usize].to_vec();
    if fujimod_firmware::sha256(&bytes) != range.sha256 {
        return Err(format!(
            "stock bytes at {:#x} differ from the layout",
            range.address
        ));
    }
    Ok(bytes)
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

/// Seal a stock journal record: one parity bit per little-endian data word
/// before `parity_offset`, then the XOR of those words in the last four bytes.
pub fn seal_journal(record: &mut [u8], parity_offset: usize) -> Result<(), String> {
    let words = parity_offset / 4;
    let checksum_offset = record
        .len()
        .checked_sub(4)
        .ok_or("journal record is shorter than its checksum")?;
    if !parity_offset.is_multiple_of(4) || parity_offset + words.div_ceil(8) > checksum_offset {
        return Err("journal parity does not fit before the checksum".into());
    }
    record[parity_offset..].fill(0);
    let mut checksum = 0u32;
    for index in 0..words {
        let value = word(record, index * 4);
        checksum ^= value;
        record[parity_offset + index / 8] |= (value.count_ones() as u8 & 1) << (index % 8);
    }
    record[checksum_offset..].copy_from_slice(&checksum.to_le_bytes());
    Ok(())
}

/// Build the settings for `requested` settings (`name -> value name`) from the
/// installed flash image. With a camera's NAND sections (`name -> bytes`), the
/// bank is that camera's: erased except for the sections the layout declares.
/// Without, it is the update's defaults table and the layout's fields.
pub fn build(
    layout: &FirmwareSettings,
    flash: &[u8],
    requested: &BTreeMap<String, String>,
    camera: Option<&BTreeMap<String, Vec<u8>>>,
) -> Result<Settings, String> {
    layout.validate()?;
    let section = source(layout, flash)?;
    let bank = &layout.bank;
    let descriptor = read(&section, layout, &bank.descriptor)?;
    if [
        word(&descriptor, 0),
        word(&descriptor, 4),
        word(&descriptor, 8),
    ] != [bank.kind, bank.blocks, bank.first_block]
    {
        return Err("stock bank descriptor differs from the layout".into());
    }
    let size = usize::try_from(bank.bytes).map_err(|_| "bank too large")?;
    let mut stores: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    let mut bytes = match camera {
        Some(sections) => camera_bank(layout, size, sections)?,
        None => vec![0; size],
    };
    if let (None, Some(table)) = (camera, &layout.defaults) {
        let table = read(&section, layout, table)?;
        if table[table.len() - 4..] != [0xff, 0xff, 0, 0] {
            return Err("defaults table has no terminator".into());
        }
        for entry in table[..table.len() - 4].as_chunks::<4>().0 {
            let offset = usize::from(u16::from_le_bytes([entry[0], entry[1]]));
            if offset >= size || entry[3] != 0 {
                return Err("defaults entry outside the bank".into());
            }
            bytes[offset] = entry[2];
        }
    }
    if camera.is_none() {
        for field in &layout.fields {
            put(&mut bytes, field.offset, &field.bytes);
        }
    }
    stores.insert("bank", bytes);
    apply_requested(layout, requested, &mut stores, true)?;
    if let Some(profile) = &layout.profile {
        let header = read(&section, layout, &profile.header)?;
        if header[14] != 0xaa {
            return Err("stock profile header has no marker".into());
        }
        stores.insert("header", header);
        for (name, record) in &profile.records {
            let mut bytes = vec![0; record.bytes as usize];
            put(
                &mut bytes,
                record.defaults_offset,
                &read(&section, layout, &record.defaults)?,
            );
            for field in &record.fields {
                put(&mut bytes, field.offset, &field.bytes);
            }
            stores.insert(name, bytes);
        }
        for field in &profile.derived_fields {
            let start = field.offset as usize;
            let end = start + field.bytes as usize;
            let value = match &field.source {
                ProfileFieldSource::Fill { value } => vec![*value; field.bytes as usize],
                ProfileFieldSource::BankCopy { offset } => stores["bank"]
                    [*offset as usize..*offset as usize + field.bytes as usize]
                    .to_vec(),
                ProfileFieldSource::BankBits { offset, mask } => {
                    vec![
                        (stores[field.store.as_str()][start] & !mask)
                            | (stores["bank"][*offset as usize] & mask),
                    ]
                }
                ProfileFieldSource::BankMap {
                    offset,
                    mask,
                    values,
                    otherwise,
                    otherwise_high,
                } => {
                    let input = stores["bank"][*offset as usize];
                    let fallback = if input & 0x80 != 0 {
                        otherwise_high.as_ref().unwrap_or(otherwise)
                    } else {
                        otherwise
                    };
                    vec![
                        (stores[field.store.as_str()][start] & !mask)
                            | values
                                .iter()
                                .find(|value| value.input == input)
                                .map_or(*fallback, |value| value.output),
                    ]
                }
            };
            stores
                .get_mut(field.store.as_str())
                .expect("validated profile store")[start..end]
                .copy_from_slice(&value);
        }
    }
    apply_requested(layout, requested, &mut stores, false)?;
    let profile = match &layout.profile {
        None => None,
        Some(located) => {
            let mut header: [u8; 16] = stores["header"].clone().try_into().expect("sixteen bytes");
            header[15] = header[..15].iter().fold(0u8, |sum, b| sum.wrapping_sub(*b));
            let mut records = Vec::new();
            for (name, record) in &located.records {
                let mut bytes = stores.remove(name.as_str()).expect("record store");
                seal_journal(&mut bytes, record.parity_offset as usize)?;
                records.push(Record {
                    bytes,
                    block: record.block,
                    managed_index: record.managed_index,
                });
            }
            Some(Profile { header, records })
        }
    };
    Ok(Settings {
        bank: Bank {
            bytes: stores.remove("bank").expect("bank store"),
            first_block: bank.first_block,
            blocks: bank.blocks,
            logical_first: bank.logical_first,
        },
        profile,
    })
}

fn apply_requested(
    layout: &FirmwareSettings,
    requested: &BTreeMap<String, String>,
    stores: &mut BTreeMap<&str, Vec<u8>>,
    bank_only: bool,
) -> Result<(), String> {
    for (name, value) in requested {
        let setting = layout
            .settings
            .get(name)
            .ok_or_else(|| format!("{} has no located setting {name}", layout.firmware_id))?;
        if (setting.store == "bank") != bank_only {
            continue;
        }
        let encoded = setting
            .values
            .get(value)
            .ok_or_else(|| format!("setting {name} has no value {value}"))?;
        let store = stores
            .get_mut(setting.store.as_str())
            .ok_or_else(|| format!("setting {name} names an absent store"))?;
        put(store, setting.offset, encoded);
    }
    Ok(())
}

/// An erased bank holding a camera's sections, each checked against the length
/// the layout declares. Fixed sections go first: counted lengths read them.
fn camera_bank(
    layout: &FirmwareSettings,
    size: usize,
    sections: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>, String> {
    if let Some(name) = sections
        .keys()
        .find(|name| !layout.sections.iter().any(|s| &s.name == *name))
    {
        return Err(format!(
            "{} declares no bank section {name}",
            layout.firmware_id
        ));
    }
    let mut bank = vec![0xff; size];
    let mut ordered: Vec<_> = layout.sections.iter().collect();
    ordered.sort_by_key(|s| !matches!(s.length, SectionLength::Fixed { .. }));
    for section in ordered {
        let expected = section.length.bytes(&bank)?;
        let bytes = sections.get(&section.name).map_or(&[][..], Vec::as_slice);
        if bytes.len() as u64 != expected {
            return Err(format!(
                "camera section {} holds {} bytes; the bank declares {expected}",
                section.name,
                bytes.len()
            ));
        }
        put(&mut bank, section.offset, bytes);
    }
    Ok(bank)
}

fn put(store: &mut [u8], offset: u64, value: &[u8]) {
    let offset = offset as usize;
    store[offset..offset + value.len()].copy_from_slice(value);
}

/// The value name a bank holds for the bank setting `name`, when it matches one.
pub fn value(layout: &FirmwareSettings, bank: &[u8], name: &str) -> Option<String> {
    let setting = layout.settings.get(name).filter(|s| s.store == "bank")?;
    setting.values.iter().find_map(|(value, encoded)| {
        let offset = setting.offset as usize;
        (bank.get(offset..offset + encoded.len()) == Some(encoded.as_slice()))
            .then(|| value.clone())
    })
}

#[cfg(test)]
mod tests;
