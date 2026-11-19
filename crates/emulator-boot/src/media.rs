//! Explicit SD card media for native emulator sessions.
use fujimod_emulator_devices::parts::sd_card::{Card, Options, SECTOR};
use fujimod_emulator_session::Session;
use std::{fs::File, io::Read, path::{Path, PathBuf}};

/// Bounds backing storage even when the image describes a large sparse volume.
const MAX_NONZERO_SECTORS: usize = 1 << 21;
const SCAN_CHUNK: usize = 4 << 20;
const MAX_FILE_BYTES: u64 = 64 << 20;

/// Load an image into an independent sparse medium. The source is opened read-only.
pub fn read_image(path: &Path) -> Result<Card, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let length = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
    if length == 0 || length % SECTOR as u64 != 0 {
        return Err(format!("{}: {length} bytes is not a whole number of 512-byte sectors", path.display()));
    }
    let sectors = u32::try_from(length / SECTOR as u64)
        .map_err(|_| format!("{}: {length} bytes is over the 2 TiB card limit", path.display()))?;
    let mut card = Card::new(Options { sectors, max_dirty_sectors: MAX_NONZERO_SECTORS })
        .map_err(|e| format!("{}: card of {length} bytes refused: {e}", path.display()))?;
    let mut buffer = vec![0u8; SCAN_CHUNK];
    let mut offset = 0u64;
    let mut kept = 0usize;
    while offset < length {
        let size = (length - offset).min(SCAN_CHUNK as u64) as usize;
        file.read_exact(&mut buffer[..size]).map_err(|e| format!("{}: {e}", path.display()))?;
        for (index, sector) in buffer[..size].chunks(SECTOR).enumerate() {
            if sector.iter().any(|byte| *byte != 0) {
                kept += 1;
                if kept > MAX_NONZERO_SECTORS {
                    return Err(format!("{}: more than {MAX_NONZERO_SECTORS} nonzero sectors", path.display()));
                }
                card.write_bytes(offset + (index * SECTOR) as u64, sector)?;
            }
        }
        offset += size as u64;
    }
    Ok(card)
}

/// Read caller-supplied SD card files before editing the selected medium.
pub fn read_files(files: &[(String, PathBuf)]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut total = 0u64;
    files.iter().map(|(name, path)| {
        let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut bytes = Vec::new();
        file.by_ref().take(MAX_FILE_BYTES - total + 1).read_to_end(&mut bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        total += bytes.len() as u64;
        if total > MAX_FILE_BYTES {
            return Err("SD card files exceed 64 MiB total".into());
        }
        Ok((name.clone(), bytes))
    }).collect()
}

/// Load an image or merge files into the primary modeled SD card slot.
pub fn attach(session: &mut Session, image: Option<&Path>, files: &[(String, PathBuf)]) -> Result<(), String> {
    if image.is_none() && files.is_empty() {
        return Ok(());
    }
    let slot = session.contacts.keys().next().cloned().ok_or("board has no modeled SD card slot")?;
    let medium = image.map(read_image).transpose()?;
    session.install_card_files(&slot, medium, &read_files(files)?)
}
