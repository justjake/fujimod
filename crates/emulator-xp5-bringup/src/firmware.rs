//! Firmware images the caller names by path.
//!
//! A firmware is an image set: a `manifest.json` listing mapped sections (name, address, size,
//! SHA-256 and a file path), the original update file, and optionally a symbol ledger. Section
//! paths resolve against `files_root`. Every section is hash-checked before it is mapped, and
//! the update file must match the manifest's `source_sha256`.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Region {
    pub name: String,
    pub address: u64,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// Where a run finds its firmware: every location is explicit.
#[derive(Clone, Debug)]
pub struct FirmwareInputs {
    pub id: String,
    /// The directory holding `manifest.json`.
    pub mapped: PathBuf,
    /// Section file paths in the manifest resolve against this directory.
    pub files_root: PathBuf,
    /// The original update file; preloads and flash storage read it.
    pub update: PathBuf,
    /// Tab-separated symbol ledger (`address`, `end`, `name`, `sha256`), if any.
    pub ledger: Option<PathBuf>,
    /// Tab-separated function list (`start`, `end`, `name`) for naming addresses in stop reports.
    pub functions: Option<PathBuf>,
    /// Base for a preload `file` path.
    pub files: PathBuf,
}

pub struct Firmware {
    pub id: String,
    pub source_sha256: String,
    pub regions: Vec<Region>,
    pub update: Vec<u8>,
    /// Lazy overlays the stock loader brings in: checked here, never mapped by the run.
    verification_regions: Vec<Region>,
    pub ledger: Vec<LedgerRow>,
    pub functions: Option<PathBuf>,
    pub files: PathBuf,
}

pub struct LedgerRow {
    pub address: u64,
    pub end: u64,
    pub name: String,
    pub sha256: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex(text: &str) -> Result<u64, String> {
    let digits = text.trim().trim_start_matches("0x");
    u64::from_str_radix(digits, 16).map_err(|e| format!("bad hex {text:?}: {e}"))
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

    pub fn load(inputs: &FirmwareInputs) -> Result<Self, String> {
        let update = fs::read(&inputs.update)
            .map_err(|e| format!("{}: {e}", inputs.update.display()))?;
        let source_sha256 = sha256_hex(&update);
        let manifest_path = inputs.mapped.join("manifest.json");
        let manifest: Value = serde_json::from_str(
            &fs::read_to_string(&manifest_path)
                .map_err(|e| format!("{}: {e}", manifest_path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        if manifest["source_sha256"].as_str() != Some(source_sha256.as_str()) {
            return Err(format!(
                "{} was unpacked from a different update file",
                manifest_path.display()
            ));
        }
        let mut regions = Vec::new();
        for section in manifest["sections"]
            .as_array()
            .ok_or("manifest has no sections")?
        {
            let name = section["name"].as_str().unwrap_or_default().to_string();
            let file = inputs
                .files_root
                .join(section["file"].as_str().ok_or("section without file")?);
            let size = section["size"].as_u64().ok_or("section without size")? as usize;
            let mut bytes = fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            if bytes.len() < size {
                return Err(format!(
                    "{} is shorter than its manifest size",
                    file.display()
                ));
            }
            bytes.truncate(size);
            let sha256 = sha256_hex(&bytes);
            if Some(sha256.as_str()) != section["sha256"].as_str() {
                return Err(format!(
                    "{name}: SHA-256 {sha256} does not match the manifest"
                ));
            }
            regions.push(Region {
                name,
                address: section["address"]
                    .as_u64()
                    .ok_or("section without address")?,
                bytes,
                sha256,
            });
        }
        let mut verification_regions = Vec::new();
        for section in manifest["verification"].as_array().into_iter().flatten() {
            let name = section["name"].as_str().ok_or("verification section without name")?;
            let file = inputs
                .files_root
                .join(section["file"].as_str().ok_or("verification section without file")?);
            let bytes = fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let sha256 = sha256_hex(&bytes);
            if Some(sha256.as_str()) != section["sha256"].as_str() {
                return Err(format!("{name}: overlay bytes do not match the manifest"));
            }
            verification_regions.push(Region {
                name: name.into(),
                address: section["address"].as_u64().ok_or("verification section without address")?,
                bytes,
                sha256,
            });
        }
        let mut ledger = Vec::new();
        if let Some(ledger_path) = &inputs.ledger {
            let text = fs::read_to_string(ledger_path)
                .map_err(|e| format!("{}: {e}", ledger_path.display()))?;
            let mut lines = text.lines();
            let header: Vec<&str> = lines.next().unwrap_or_default().split('\t').collect();
            let col = |name: &str| header.iter().position(|h| *h == name);
            let (Some(a), Some(e), Some(n), Some(h)) =
                (col("address"), col("end"), col("name"), col("sha256"))
            else {
                return Err(format!("{}: unexpected header", ledger_path.display()));
            };
            for line in lines {
                let fields: Vec<&str> = line.split('\t').collect();
                if fields.len() <= h.max(a).max(e).max(n) || fields[e].is_empty() {
                    continue;
                }
                ledger.push(LedgerRow {
                    address: parse_hex(fields[a])?,
                    end: parse_hex(fields[e])?,
                    name: fields[n].to_string(),
                    sha256: fields[h].to_string(),
                });
            }
        }
        Ok(Firmware {
            id: inputs.id.clone(),
            source_sha256,
            regions,
            update,
            verification_regions,
            ledger,
            functions: inputs.functions.clone(),
            files: inputs.files.clone(),
        })
    }

    pub fn bytes(&self, address: u64, len: usize) -> Option<&[u8]> {
        self.regions.iter().find_map(|r| {
            let start = address.checked_sub(r.address)? as usize;
            r.bytes.get(start..start.checked_add(len)?)
        })
    }

    /// Checks a ledger routine's instruction bytes and returns its row.
    pub fn verify_routine(&self, name: &str) -> Result<&LedgerRow, String> {
        let row = self
            .ledger
            .iter()
            .find(|r| r.name == name)
            .ok_or_else(|| format!("{name} is not in the {} ledger", self.id))?;
        let bytes = self
            .bytes(row.address, (row.end - row.address) as usize)
            .ok_or_else(|| format!("{name} lies outside the mapped regions"))?;
        let actual = sha256_hex(bytes);
        if actual != row.sha256 {
            return Err(format!(
                "{name}: bytes hash {actual}, ledger says {}",
                row.sha256
            ));
        }
        Ok(row)
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
            ledger: Vec::new(),
            functions: None,
            files: PathBuf::new(),
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

/// Resolve a path from a manifest against a base directory.
pub fn resolve(base: &Path, relative: &str) -> PathBuf {
    base.join(relative)
}

/// Address-to-name lookup over a function list.
pub struct Symbols {
    rows: Vec<(u64, u64, String)>,
}

impl Symbols {
    pub fn load(firmware: &Firmware) -> Self {
        let mut rows = Vec::new();
        if let Some(text) = firmware
            .functions
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
        {
            for line in text.lines().skip(1) {
                let f: Vec<_> = line.split('\t').collect();
                if f.len() < 3 {
                    continue;
                }
                if let (Ok(a), Ok(e)) = (
                    u64::from_str_radix(f[0].trim_start_matches("0x"), 16),
                    u64::from_str_radix(f[1].trim_start_matches("0x"), 16),
                ) {
                    rows.push((a, e + 1, f[2].into()));
                }
            }
        }
        for row in &firmware.ledger {
            rows.push((row.address, row.end, row.name.clone()));
        }
        rows.sort_by_key(|row| row.0);
        Self { rows }
    }

    pub fn at(&self, address: u64) -> String {
        match self.rows.partition_point(|row| row.0 <= address).checked_sub(1) {
            Some(index) if address < self.rows[index].1 => {
                let row = &self.rows[index];
                format!("{}+{:#x}", row.2, address - row.0)
            }
            _ => format!("{address:#x}"),
        }
    }
}
