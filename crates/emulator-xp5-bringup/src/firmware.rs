//! The firmware image set, decoded from the caller's update file.
//!
//! The mapped sections come from the bundled stream layout of the selected firmware (offset in
//! the update, encoding, load address, size, SHA-256). The update must match the layout's digest,
//! and every section is hash-checked before it is mapped.

use fujimod_firmware::{Encoding, Mapping};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub struct Region {
    pub name: String,
    pub address: u64,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

pub struct Firmware {
    pub id: String,
    pub source_sha256: String,
    pub regions: Vec<Region>,
    pub update: Vec<u8>,
    /// Lazy overlays the stock loader brings in: checked here, never mapped by the run.
    verification_regions: Vec<Region>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    update_sha256: String,
    sections: Vec<Section>,
    verification: Vec<Section>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    name: String,
    address: u64,
    size: Option<u64>,
    decoded_size: Option<u64>,
    dat_offset: u64,
    encoding: String,
    sha256: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode(update: &[u8], section: &Section) -> Result<Region, String> {
    let size = section.size.or(section.decoded_size).ok_or("section without a size")?;
    let encoding = match section.encoding.as_str() {
        "raw" => Encoding::Raw,
        "method4" => Encoding::Method4,
        other => return Err(format!("{}: unknown encoding {other}", section.name)),
    };
    let mapping = Mapping {
        name: section.name.clone(),
        offset: section.dat_offset,
        size,
        base: section.address,
        executable: false,
        encoding,
    };
    let image = fujimod_firmware::decode(update, std::slice::from_ref(&mapping))
        .map_err(|error| format!("{}: {error}", section.name))?;
    let unit = image.units.into_iter().next().ok_or("decoder returned no unit")?;
    let sha256 = sha256_hex(&unit.bytes);
    if sha256 != section.sha256 {
        return Err(format!(
            "{}: SHA-256 {sha256} does not match the stream layout; is this the right update?",
            section.name
        ));
    }
    Ok(Region { name: section.name.clone(), address: section.address, bytes: unit.bytes, sha256 })
}

impl Firmware {
    /// Verified original update payload for a declarative NOR flash-storage device.
    pub fn dat_payload(&self, origin: usize, xor: u8) -> Result<Vec<u8>, String> {
        Ok(self
            .update
            .get(origin..)
            .ok_or("flash storage origin outside the update file")?
            .iter()
            .map(|b| b ^ xor)
            .collect())
    }

    /// Declarative raw boot preloads read the update file.
    pub fn xor_dat_range(&self, offset: u64, length: u64, hash: &str) -> Result<Vec<u8>, String> {
        let end = offset
            .checked_add(length)
            .ok_or("raw preload extent overflow")?;
        let start = usize::try_from(offset).map_err(|e| e.to_string())?;
        let end = usize::try_from(end).map_err(|e| e.to_string())?;
        let bytes = self
            .update
            .get(start..end)
            .ok_or("raw preload outside the update file")?;
        let bytes: Vec<u8> = bytes.iter().map(|b| b ^ 0xff).collect();
        if sha256_hex(&bytes) != hash {
            return Err("raw preload hash mismatch".into());
        }
        Ok(bytes)
    }

    /// Decode `firmware`'s bundled stream layout from `update`.
    pub fn from_update(firmware: &str, update: Vec<u8>, load_sections: bool) -> Result<Self, String> {
        let layout: Layout = serde_json::from_str(crate::manifest::bundled(firmware)?.streams)
            .map_err(|error| format!("{firmware} stream layout: {error}"))?;
        let source_sha256 = sha256_hex(&update);
        if source_sha256 != layout.update_sha256 {
            return Err(format!(
                "update SHA-256 {source_sha256} is not the {firmware} update ({})",
                layout.update_sha256
            ));
        }
        let mut regions = Vec::new();
        let mut verification_regions = Vec::new();
        if load_sections {
            for section in &layout.sections {
                regions.push(decode(&update, section)?);
            }
            for section in &layout.verification {
                verification_regions.push(decode(&update, section)?);
            }
        }
        Ok(Firmware { id: firmware.to_string(), source_sha256, regions, update, verification_regions })
    }

    pub fn bytes(&self, address: u64, len: usize) -> Option<&[u8]> {
        self.regions.iter().find_map(|r| {
            let start = address.checked_sub(r.address)? as usize;
            r.bytes.get(start..start.checked_add(len)?)
        })
    }

    /// A firmware holding only the given (address, bytes) regions, for unit tests.
    #[cfg(test)]
    pub(crate) fn for_tests(id: &str, source_sha256: &str, regions: Vec<(u64, Vec<u8>)>) -> Self {
        let region = |(address, bytes): (u64, Vec<u8>)| Region {
            name: format!("test-{address:#x}"),
            address,
            sha256: sha256_hex(&bytes),
            bytes,
        };
        Firmware {
            id: id.to_string(),
            source_sha256: source_sha256.to_string(),
            regions: regions.into_iter().map(region).collect(),
            update: Vec::new(),
            verification_regions: Vec::new(),
        }
    }

    /// Checks one instruction word at an address the run relies on.
    pub fn expect_word(&self, address: u64, word: u32, what: &str) -> Result<(), String> {
        let bytes = self
            .bytes(address, 4)
            .or_else(|| {
                self.verification_regions.iter().find_map(|r| {
                    let start = address.checked_sub(r.address)? as usize;
                    r.bytes.get(start..start.checked_add(4)?)
                })
            })
            .ok_or_else(|| format!("{what}: {address:#x} unmapped"))?;
        let actual = u32::from_le_bytes(bytes.try_into().unwrap());
        if actual != word {
            return Err(format!(
                "{what}: {address:#x} holds {actual:#010x}, expected {word:#010x}"
            ));
        }
        Ok(())
    }
}
