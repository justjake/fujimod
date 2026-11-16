//! Raw card images for `--start rom --sd-image PATH` (emulator only).
//!
//! The image is a byte-for-byte card: an MBR with one partition, or a volume with no partition
//! table, holding FAT32 or exFAT. It is opened read-only and streamed once into FujiMod's sparse
//! SD model (only nonzero sectors are kept, 1 GiB at most), so the emulated guest can never write
//! to the file and a 128 GB image that is mostly holes costs a few hundred KiB of memory, not
//! 128 GB. Nothing here writes, formats or creates an image.
//!
//! Safety: this start must not be able to carry an install card. Before the image is offered to
//! the ROM, a small root-directory reader (below) lists the root of every volume in it, long and
//! short names, and the image is refused when any root entry is `FPUPDATE*`, `FFBOOT*` or `*.DAT`
//! (any case), or when a volume cannot be read well enough to list. The reader is not a file
//! system: it follows one cluster chain and reads names, nothing else. Deleted entries are not
//! listed (a deleted name has lost its first character).

use fujimod_emulator_devices::parts::sd_card::{Card, Options, SECTOR};
use std::{fs::File, io::Read, path::Path};

/// The sparse model's bound: 2^21 nonzero sectors (1 GiB). A boot card holds a few hundred.
const MAX_NONZERO_SECTORS: usize = 1 << 21;
/// A root directory is read whole; refuse one larger than this.
const MAX_DIRECTORY_BYTES: usize = 16 << 20;
const MAX_DIRECTORY_CLUSTERS: usize = 1 << 16;
const SCAN_CHUNK: usize = 4 << 20;

pub(crate) struct CardImage {
    pub card: Card,
    /// One line per volume: partition, file system and geometry, for the run log.
    pub layout: Vec<String>,
    /// Every name found in the roots (long and short forms), for the run log.
    pub names: Vec<String>,
}

impl CardImage {
    pub fn load(path: &Path) -> Result<Self, String> {
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
            let size = ((length - offset) as usize).min(SCAN_CHUNK);
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
        let listing = list_roots(&card).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(name) = listing.names.iter().find(|name| forbidden(name)) {
            return Err(format!(
                "{}: root entry {name:?} refused: a card image for this emulator start may hold \
                 startup-card files only (no FPUPDATE*, FFBOOT* or *.DAT)",
                path.display()
            ));
        }
        Ok(Self { card, layout: listing.layout, names: listing.names })
    }
}

/// Names an image may not carry in a root: the update, the loader's RAM image, any `.DAT`.
pub(crate) fn forbidden(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("fpupdate") || lower.starts_with("ffboot") || lower.ends_with(".dat")
}

#[derive(Debug)]
pub(crate) struct Listing {
    pub layout: Vec<String>,
    pub names: Vec<String>,
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn sector(card: &Card, lba: u64) -> Result<&[u8], String> {
    let lba = u32::try_from(lba).map_err(|_| format!("sector {lba} is beyond the card"))?;
    card.sector(lba)
}

/// List the root of every volume on the card: the whole card as one volume (no partition
/// table), or each MBR partition.
pub(crate) fn list_roots(card: &Card) -> Result<Listing, String> {
    let first = sector(card, 0)?;
    if first[510..512] != [0x55, 0xaa] {
        return Err("sector 0 has no 0x55AA signature: not an MBR or a volume".into());
    }
    let mut listing = Listing { layout: Vec::new(), names: Vec::new() };
    if first[3..11] == *b"EXFAT   " || first[82..90] == *b"FAT32   " {
        volume(card, 0, "no partition table (superfloppy)", &mut listing)?;
        return Ok(listing);
    }
    let mut used = 0;
    for index in 0..4 {
        let at = 446 + index * 16;
        let (kind, start, count) = (first[at + 4], le32(first, at + 8), le32(first, at + 12));
        if kind == 0 && count == 0 {
            continue;
        }
        used += 1;
        if kind == 0xee {
            return Err("GPT card (protective MBR): not supported by this reader".into());
        }
        if start == 0 || count == 0 || u64::from(start) + u64::from(count) > u64::from(card.sectors()) {
            return Err(format!("MBR entry {} (type {kind:#04x}) is empty or exceeds the card", index + 1));
        }
        let label = format!("MBR entry {} type {kind:#04x} start LBA {start} ({} MiB) sectors {count}", index + 1, u64::from(start) * 512 >> 20);
        volume(card, u64::from(start), &label, &mut listing)?;
    }
    if used == 0 {
        return Err("MBR holds no partition and sector 0 is not a volume".into());
    }
    Ok(listing)
}

fn volume(card: &Card, start: u64, label: &str, listing: &mut Listing) -> Result<(), String> {
    let boot = sector(card, start)?;
    if boot[510..512] != [0x55, 0xaa] {
        return Err(format!("{label}: no 0x55AA signature in the volume's boot sector"));
    }
    if boot[3..11] == *b"EXFAT   " {
        exfat(card, start, boot, label, listing)
    } else if boot[82..90] == *b"FAT32   " {
        fat32(card, start, boot, label, listing)
    } else {
        Err(format!("{label}: neither exFAT nor FAT32; the root cannot be checked"))
    }
}

fn fat32(card: &Card, start: u64, boot: &[u8], label: &str, listing: &mut Listing) -> Result<(), String> {
    let spc = u64::from(boot[13]);
    let reserved = u64::from(le16(boot, 14));
    let fats = u64::from(boot[16]);
    let fat_sectors = u64::from(le32(boot, 36));
    let total = if le16(boot, 19) != 0 { u64::from(le16(boot, 19)) } else { u64::from(le32(boot, 32)) };
    let root = le32(boot, 44);
    if le16(boot, 11) != 512 || spc == 0 || !spc.is_power_of_two() || reserved == 0 || fats == 0 || fat_sectors == 0 || le16(boot, 17) != 0 {
        return Err(format!("{label}: unsupported FAT32 boot sector"));
    }
    let fat = start + reserved;
    let data = fat + fats * fat_sectors;
    let clusters = total
        .checked_sub(reserved + fats * fat_sectors)
        .ok_or_else(|| format!("{label}: FAT tables exceed the volume"))?
        / spc;
    if clusters < 65525 || !(2..clusters + 2).contains(&u64::from(root)) {
        return Err(format!("{label}: not a FAT32 cluster count or root cluster"));
    }
    let next = |cluster: u32| -> Result<u32, String> {
        let entry = sector(card, fat + u64::from(cluster) / 128)?;
        Ok(le32(entry, (cluster % 128) as usize * 4) & 0x0fff_ffff)
    };
    let mut directory = Vec::new();
    let mut cluster = root;
    for _ in 0..MAX_DIRECTORY_CLUSTERS {
        if !(2..clusters as u32 + 2).contains(&cluster) {
            return Err(format!("{label}: root chain leaves the data area"));
        }
        for index in 0..spc {
            directory.extend_from_slice(sector(card, data + u64::from(cluster - 2) * spc + index)?);
        }
        if directory.len() > MAX_DIRECTORY_BYTES {
            return Err(format!("{label}: root directory is over {MAX_DIRECTORY_BYTES} bytes"));
        }
        let following = next(cluster)?;
        if following >= 0x0fff_fff8 {
            break;
        }
        cluster = following;
    }
    listing.layout.push(format!(
        "{label}: FAT32, {} bytes per cluster, FAT at LBA {fat} ({fats} copies of {fat_sectors} sectors), data at LBA {data}, {clusters} clusters, root cluster {root}",
        spc * 512
    ));
    fat_names(&directory, &mut listing.names);
    Ok(())
}

/// 8.3 and long names of a FAT directory image. A long name is kept next to its short alias.
fn fat_names(directory: &[u8], names: &mut Vec<String>) {
    let mut long: Vec<(u8, Vec<u16>)> = Vec::new();
    for entry in directory.chunks_exact(32) {
        match entry[0] {
            0x00 => break,
            0xe5 => {
                long.clear();
                continue;
            }
            _ => {}
        }
        if entry[11] == 0x0f {
            let mut units = Vec::with_capacity(13);
            for range in [1..11, 14..26, 28..32] {
                units.extend(entry[range].chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])));
            }
            long.push((entry[0] & 0x1f, units));
            continue;
        }
        let pieces = std::mem::take(&mut long);
        if entry[11] & 0x08 != 0 {
            continue; // the volume label
        }
        let mut raw: [u8; 11] = entry[..11].try_into().unwrap();
        if raw[0] == 0x05 {
            raw[0] = 0xe5;
        }
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim_end_matches(' ').to_string();
        let (base, extension) = (text(&raw[..8]), text(&raw[8..]));
        names.push(if extension.is_empty() { base } else { format!("{base}.{extension}") });
        if !pieces.is_empty() {
            let mut ordered = pieces;
            ordered.sort_by_key(|(order, _)| *order);
            let units: Vec<u16> = ordered.into_iter().flat_map(|(_, units)| units).take_while(|unit| *unit != 0).collect();
            names.push(String::from_utf16_lossy(&units));
        }
    }
}

fn exfat(card: &Card, start: u64, boot: &[u8], label: &str, listing: &mut Listing) -> Result<(), String> {
    let (sector_shift, cluster_shift) = (boot[108], boot[109]);
    if sector_shift != 9 || cluster_shift > 25 - 9 || boot[110] == 0 || boot[110] > 2 {
        return Err(format!("{label}: unsupported exFAT geometry (sector shift {sector_shift}, cluster shift {cluster_shift})"));
    }
    let (fat_offset, fat_length) = (u64::from(le32(boot, 80)), u64::from(le32(boot, 84)));
    let (heap, clusters, root) = (u64::from(le32(boot, 88)), le32(boot, 92), le32(boot, 96));
    let volume_length = le64(boot, 72);
    if !(2..clusters.saturating_add(2)).contains(&root) || fat_length == 0 || heap < fat_offset + fat_length || heap + (u64::from(clusters) << cluster_shift) > volume_length {
        return Err(format!("{label}: exFAT heap, FAT or root cluster out of bounds"));
    }
    let per_cluster = 1u64 << cluster_shift;
    let mut directory = Vec::new();
    let mut cluster = root;
    for _ in 0..MAX_DIRECTORY_CLUSTERS {
        if !(2..clusters + 2).contains(&cluster) {
            return Err(format!("{label}: root chain leaves the cluster heap"));
        }
        for index in 0..per_cluster {
            directory.extend_from_slice(sector(card, start + heap + u64::from(cluster - 2) * per_cluster + index)?);
        }
        if directory.len() > MAX_DIRECTORY_BYTES {
            return Err(format!("{label}: root directory is over {MAX_DIRECTORY_BYTES} bytes"));
        }
        let entry = sector(card, start + fat_offset + u64::from(cluster) / 128)?;
        let following = le32(entry, (cluster % 128) as usize * 4);
        if following >= 0xffff_fff8 {
            break;
        }
        cluster = following;
    }
    listing.layout.push(format!(
        "{label}: exFAT, {} bytes per cluster, FAT at LBA {} ({fat_length} sectors), heap at LBA {}, {clusters} clusters, root cluster {root}, volume {volume_length} sectors",
        per_cluster * 512,
        start + fat_offset,
        start + heap
    ));
    let entries: Vec<&[u8]> = directory.chunks_exact(32).collect();
    let mut index = 0;
    while index < entries.len() {
        let entry = entries[index];
        match entry[0] {
            0x00 => break,
            0x85 => {
                // File entry: `secondary` entries follow (stream extension 0xC0, names 0xC1).
                let secondary = entry[1] as usize;
                let mut units: Vec<u16> = Vec::new();
                let mut length = 0usize;
                for next in entries.iter().skip(index + 1).take(secondary) {
                    match next[0] {
                        0xc0 => length = next[3] as usize,
                        0xc1 => units.extend(next[2..32].chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]]))),
                        _ => {}
                    }
                }
                units.truncate(length);
                listing.names.push(String::from_utf16_lossy(&units));
                index += 1 + secondary;
            }
            _ => index += 1, // bitmap 0x81, up-case table 0x82, label 0x83, unused entries
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    const SECTORS: u32 = 0x40_0000; // 2 GiB, the smallest card the model takes

    fn blank() -> Card {
        Card::new(Options { sectors: SECTORS, max_dirty_sectors: 65536 }).unwrap()
    }
    fn put(card: &mut Card, lba: u64, bytes: &[u8]) {
        card.write_bytes(lba * 512, bytes).unwrap();
    }
    fn mbr(card: &mut Card, kind: u8, start: u32, count: u32) {
        let mut sector = vec![0u8; 512];
        let at = 446;
        sector[at + 4] = kind;
        sector[at + 8..at + 12].copy_from_slice(&start.to_le_bytes());
        sector[at + 12..at + 16].copy_from_slice(&count.to_le_bytes());
        sector[510..].copy_from_slice(&[0x55, 0xaa]);
        put(card, 0, &sector);
    }

    /// A FAT32 card made by FujiMod's own formatter (MBR, partition at 4 MiB, 16 KiB clusters,
    /// 8.3 plus long names), the same builder the default ROM start uses.
    fn fat32_card(files: &[(&str, &[u8])]) -> Card {
        let mut input: fujimod_emulator_xp5_board::board::sd::MediaInputs = toml::from_str(
            "base=4288946176\npresent=true\nprotected=false\nfiles=[]\n[format]\nsectors=4194304\n\
             max_dirty_sectors=65536\npartition_start=8192\nsectors_per_cluster=32\nlabel='BOOT'\n",
        )
        .unwrap();
        input.files = files.iter().map(|(name, _)| (name.to_string(), name.to_string())).collect();
        let assets = files.iter().map(|(name, bytes)| (name.to_string(), bytes.to_vec())).collect();
        let mut media = fujimod_emulator_xp5_board::board::sd::resolve(vec![input], &assets).unwrap();
        media.remove(&4288946176).unwrap().card.take().unwrap()
    }

    #[test]
    fn a_formatted_fat32_card_lists_its_long_and_short_names() {
        let card = fat32_card(&[("marble_init.bin", b"abcd")]);
        let listing = list_roots(&card).unwrap();
        assert!(listing.names.iter().any(|name| name.eq_ignore_ascii_case("marble_init.bin")), "{:?}", listing.names);
        assert!(listing.names.iter().all(|name| !forbidden(name)), "{:?}", listing.names);
        assert!(listing.layout[0].contains("FAT32"), "{:?}", listing.layout);
    }

    #[test]
    fn an_update_or_loader_image_in_a_fat32_root_is_found() {
        for name in ["FPUPDATE.DAT", "fpupdate.dat", "FFBOOT.BIN", "ffboot.bin", "NOTES.DAT"] {
            let card = fat32_card(&[("marble_init.bin", b"abcd"), (name, b"x")]);
            let listing = list_roots(&card).unwrap();
            assert!(listing.names.iter().any(|found| forbidden(found)), "{name}: {:?}", listing.names);
        }
    }

    #[test]
    fn forbidden_names_are_the_update_the_loader_image_and_dat_files() {
        for name in ["FPUPDATE.DAT", "FPUPDA~1.DAT", "fpupdate.bin", "FFBOOT.BIN", "FFBOOT~1.BIN", "ffboot", "x.dat", "A.DaT"] {
            assert!(forbidden(name), "{name}");
        }
        for name in ["marble_init.bin", "MARBLE~1.BIN", "readme.txt", "DAT", "data"] {
            assert!(!forbidden(name), "{name}");
        }
    }

    /// An exFAT volume at `start` holding the named files in its root (names only; the
    /// listing never reads file data). Cluster size 128 KiB as the SD formatter writes at 64 GB.
    fn exfat_card(start: u32, files: &[&str], with_mbr: bool) -> Card {
        let mut card = blank();
        let shift_cluster = 8u8; // 256 sectors = 128 KiB
        let (fat_offset, fat_length, heap) = (2048u32, 2048u32, 4096u32);
        let clusters = 8192u32;
        let length = u64::from(heap) + (u64::from(clusters) << shift_cluster);
        let mut boot = vec![0u8; 512];
        boot[0..3].copy_from_slice(&[0xeb, 0x76, 0x90]);
        boot[3..11].copy_from_slice(b"EXFAT   ");
        boot[64..72].copy_from_slice(&u64::from(start).to_le_bytes());
        boot[72..80].copy_from_slice(&length.to_le_bytes());
        boot[80..84].copy_from_slice(&fat_offset.to_le_bytes());
        boot[84..88].copy_from_slice(&fat_length.to_le_bytes());
        boot[88..92].copy_from_slice(&heap.to_le_bytes());
        boot[92..96].copy_from_slice(&clusters.to_le_bytes());
        boot[96..100].copy_from_slice(&4u32.to_le_bytes()); // root cluster
        boot[108] = 9;
        boot[109] = shift_cluster;
        boot[110] = 1;
        boot[510..].copy_from_slice(&[0x55, 0xaa]);
        put(&mut card, u64::from(start), &boot);
        // FAT: clusters 0 and 1 reserved, root (cluster 4) is one cluster.
        let mut fat = vec![0u8; 512];
        fat[0..4].copy_from_slice(&0xffff_fff8u32.to_le_bytes());
        fat[4..8].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        fat[16..20].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        put(&mut card, u64::from(start + fat_offset), &fat);
        let mut root = Vec::new();
        let mut special = [0u8; 32];
        special[0] = 0x81; // allocation bitmap
        root.extend_from_slice(&special);
        special[0] = 0x82; // up-case table
        root.extend_from_slice(&special);
        for name in files {
            let units: Vec<u16> = name.encode_utf16().collect();
            let names = units.len().div_ceil(15);
            let mut file = [0u8; 32];
            file[0] = 0x85;
            file[1] = 1 + names as u8;
            let mut stream = [0u8; 32];
            stream[0] = 0xc0;
            stream[1] = 0x03;
            stream[3] = units.len() as u8;
            root.extend_from_slice(&file);
            root.extend_from_slice(&stream);
            for part in units.chunks(15) {
                let mut entry = [0u8; 32];
                entry[0] = 0xc1;
                for (index, unit) in part.iter().enumerate() {
                    entry[2 + index * 2..4 + index * 2].copy_from_slice(&unit.to_le_bytes());
                }
                root.extend_from_slice(&entry);
            }
        }
        put(&mut card, u64::from(start + heap) + 2 * (1 << shift_cluster), &root);
        if with_mbr {
            mbr(&mut card, 0x07, start, u32::try_from(length).unwrap());
        }
        card
    }

    #[test]
    fn an_exfat_partition_at_4_mib_lists_long_names() {
        let card = exfat_card(8192, &["marble_init.bin"], true);
        let listing = list_roots(&card).unwrap();
        assert_eq!(listing.names, ["marble_init.bin"]);
        assert!(listing.layout[0].contains("exFAT, 131072 bytes per cluster"), "{:?}", listing.layout);
        assert!(listing.layout[0].contains("start LBA 8192 (4 MiB)"), "{:?}", listing.layout);
        assert!(list_roots(&exfat_card(8192, &[], true)).unwrap().names.is_empty());
    }

    #[test]
    fn an_exfat_root_with_an_update_or_a_long_dat_name_is_found() {
        for name in ["FPUPDATE.DAT", "a_rather_long_name_that_ends.dat", "FFBOOT.BIN"] {
            let card = exfat_card(8192, &["marble_init.bin", name], true);
            let listing = list_roots(&card).unwrap();
            assert!(listing.names.iter().any(|found| forbidden(found)), "{name}: {:?}", listing.names);
            assert!(listing.names.iter().any(|found| found == name), "{name}: {:?}", listing.names);
        }
    }

    #[test]
    fn a_volume_with_no_partition_table_is_listed_too() {
        // Move the exFAT volume's boot sector to sector 0 (offsets inside it are volume-relative).
        let source = exfat_card(0, &["marble_init.bin"], false);
        // start 0 is refused as an MBR entry, so this card is read as a superfloppy.
        let boot = source.sector(0).unwrap().to_vec();
        assert_eq!(&boot[3..11], b"EXFAT   ");
        let listing = list_roots(&source).unwrap();
        assert!(listing.layout[0].contains("no partition table"), "{:?}", listing.layout);
        assert_eq!(listing.names, ["marble_init.bin"]);
    }

    #[test]
    fn unreadable_cards_are_refused_not_assumed_clean() {
        let mut card = blank();
        assert!(list_roots(&card).unwrap_err().contains("0x55AA"));
        mbr(&mut card, 0xee, 1, SECTORS - 1);
        assert!(list_roots(&card).unwrap_err().contains("GPT"));
        let mut card = blank();
        mbr(&mut card, 0x0c, 8192, 1 << 20);
        assert!(list_roots(&card).unwrap_err().contains("no 0x55AA"));
        let mut card = blank();
        mbr(&mut card, 0x0c, 8192, 1 << 20);
        put(&mut card, 8192, &{
            let mut sector = vec![0x11u8; 512];
            sector[510..].copy_from_slice(&[0x55, 0xaa]);
            sector
        });
        assert!(list_roots(&card).unwrap_err().contains("neither exFAT nor FAT32"));
        let mut card = blank();
        mbr(&mut card, 0x0c, 8192, SECTORS); // runs past the card
        assert!(list_roots(&card).unwrap_err().contains("exceeds the card"));
    }

    fn write_image(path: &Path, card: &Card) {
        let mut file = File::create(path).unwrap();
        file.set_len(u64::from(card.sectors()) * 512).unwrap();
        for (lba, bytes) in card.nonzero_sectors() {
            file.seek(SeekFrom::Start(u64::from(lba) * 512)).unwrap();
            file.write_all(bytes).unwrap();
        }
    }

    #[test]
    fn an_image_file_loads_sparse_and_is_never_written() {
        let directory = std::env::temp_dir().join(format!("card-image-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("card.img");
        write_image(&path, &exfat_card(8192, &["marble_init.bin"], true));
        let before = std::fs::read(&path).unwrap();
        let image = CardImage::load(&path).unwrap();
        assert_eq!(image.card.sectors(), SECTORS);
        assert!(image.card.dirty_sectors() < 64, "{}", image.card.dirty_sectors());
        assert_eq!(image.names, ["marble_init.bin"]);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn an_image_with_an_update_in_its_root_is_refused() {
        let directory = std::env::temp_dir().join(format!("card-image-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        for (index, card) in [
            exfat_card(8192, &["marble_init.bin", "FPUPDATE.DAT"], true),
            fat32_card(&[("marble_init.bin", b"abcd"), ("FFBOOT.BIN", b"x")]),
        ]
        .iter()
        .enumerate()
        {
            let path = directory.join(format!("card{index}.img"));
            write_image(&path, card);
            let error = CardImage::load(&path).err().expect("refused");
            assert!(error.contains("startup-card files only"), "{error}");
        }
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn odd_image_sizes_are_refused() {
        let directory = std::env::temp_dir().join(format!("card-image-size-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("short.img");
        std::fs::write(&path, vec![0u8; 1000]).unwrap();
        assert!(CardImage::load(&path).err().unwrap().contains("512-byte sectors"));
        std::fs::write(&path, vec![0u8; 1 << 20]).unwrap();
        assert!(CardImage::load(&path).err().unwrap().contains("refused"));
        std::fs::remove_dir_all(&directory).ok();
    }
}
