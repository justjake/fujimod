//! Explicit SD card media for native emulator sessions.
use fujimod_emulator_devices::parts::sd_card::{Card, Options, SECTOR};
use fujimod_emulator_session::Session;
use std::{fs::File, io::{Read, Seek, SeekFrom, Write as _}, path::{Path, PathBuf}};

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
        Read::by_ref(&mut file).take(MAX_FILE_BYTES - total + 1).read_to_end(&mut bytes)
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

/// Write every sector of `current` that differs from `original` (the medium loaded from `path`)
/// back into the image file at its own offset. The file keeps its length: it is opened for
/// writing without creating or truncating it, and both media must have the file's size.
/// Returns the number of sectors written.
pub fn write_back(path: &Path, original: &Card, current: &Card) -> Result<usize, String> {
    let error = |e: std::io::Error| format!("{}: {e}", path.display());
    if current.sectors() != original.sectors() {
        return Err(format!(
            "{}: the card has {} sectors and the image {}; not written back",
            path.display(), current.sectors(), original.sectors()
        ));
    }
    let length = std::fs::metadata(path).map_err(error)?.len();
    if length != u64::from(original.sectors()) * SECTOR as u64 {
        return Err(format!("{}: the image changed size during the run; not written back", path.display()));
    }
    let mut changed: Vec<u32> = original.nonzero_sectors().map(|(lba, _)| lba)
        .chain(current.nonzero_sectors().map(|(lba, _)| lba))
        .collect();
    changed.sort_unstable();
    changed.dedup();
    changed.retain(|&lba| original.sector(lba).ok() != current.sector(lba).ok());
    if changed.is_empty() {
        return Ok(0);
    }
    let mut file = std::fs::OpenOptions::new().write(true).open(path).map_err(error)?;
    for &lba in &changed {
        file.seek(SeekFrom::Start(u64::from(lba) * SECTOR as u64)).map_err(error)?;
        file.write_all(current.sector(lba)?).map_err(error)?;
    }
    file.sync_all().map_err(error)?;
    Ok(changed.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_back_writes_only_changed_sectors_and_keeps_the_length() {
        let directory = std::env::temp_dir().join(format!("media-write-back-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("card.img");
        // The smallest SDHC card the model accepts: 2 GiB, sparse on the host.
        let length = 2u64 << 30;
        let mut file = File::create(&path).unwrap();
        file.set_len(length).unwrap();
        for (lba, byte) in [(0u64, 1u8), (5, 5), (9, 9)] {
            file.seek(SeekFrom::Start(lba * SECTOR as u64)).unwrap();
            file.write_all(&[byte]).unwrap();
        }
        drop(file);
        let original = read_image(&path).unwrap();
        let mut current = original.clone();
        current.write_bytes(5 * SECTOR as u64, &[0u8; SECTOR]).unwrap();
        current.write_bytes(20 * SECTOR as u64, &[0x20u8; SECTOR]).unwrap();
        assert_eq!(write_back(&path, &original, &current).unwrap(), 2);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), length);
        let mut written = vec![0u8; 32 * SECTOR];
        File::open(&path).unwrap().read_exact(&mut written).unwrap();
        assert_eq!(written[0], 1);
        assert_eq!(written[5 * SECTOR], 0);
        assert_eq!(written[9 * SECTOR], 9);
        assert!(written[20 * SECTOR..21 * SECTOR].iter().all(|byte| *byte == 0x20));
        assert_eq!(write_back(&path, &current, &current).unwrap(), 0);
        let small = Card::new(Options { sectors: 0x40_0400, max_dirty_sectors: 8 }).unwrap();
        assert!(write_back(&path, &original, &small).unwrap_err().contains("sectors"));
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
