//! Bounded FAT32 host view and formatter. Guest file publication is exclusively
//! sector writes through the SD host; reading this view never changes media.
//! Supports one MBR partition, mirrored or explicitly active FATs, and 8.3
//! names and bounded ASCII long names; malformed chains and directory aliases refuse.
mod edit;
mod long_names;
pub use edit::merge;

use fujimod_emulator_devices::parts::sd_card::{Card, Options as CardOptions};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
/// Complete startup cards include both boot images and an official update.
/// Host file reads retain their separate per-request bound.
pub const MAX_PRELOAD_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 4096;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub sectors: u32,
    pub max_dirty_sectors: usize,
    pub partition_start: u32,
    pub sectors_per_cluster: u8,
    pub label: String,
    #[serde(default)]
    pub read_only_files: Vec<String>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub enum CardEntryKind {
    File,
    Directory,
}
#[derive(Clone, Debug, Serialize)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct CardEntry {
    pub name: String,
    pub path: String,
    pub kind: CardEntryKind,
    pub bytes: u32,
    pub children: Vec<CardEntry>,
}
#[derive(Clone)]
struct DirEntry {
    name: String,
    attributes: u8,
    first: u32,
    size: u32,
}
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn err<T>(reason: &str) -> Result<T, String> {
    Err(reason.into())
}

pub struct Reader<'a> {
    card: &'a Card,
    start: u32,
    fat: u32,
    data: u32,
    spc: u32,
    fats: u32,
    fat_sectors: u32,
    clusters: u32,
    active: Option<u32>,
    root: u32,
    reserved: u32,
}
#[derive(Serialize)]
pub struct Geometry {
    pub partition_start: u32,
    pub fat_start: u32,
    pub data_start: u32,
    pub fat_sectors: u32,
    pub sectors_per_cluster: u32,
    pub clusters: u32,
    pub root_cluster: u32,
}
impl<'a> Reader<'a> {
    pub fn geometry(&self) -> Geometry {
        Geometry {
            partition_start: self.start,
            fat_start: self.fat,
            data_start: self.data,
            fat_sectors: self.fat_sectors,
            sectors_per_cluster: self.spc,
            clusters: self.clusters,
            root_cluster: self.root,
        }
    }

    pub fn new(card: &'a Card) -> Result<Self, String> {
        let mbr = card.sector(0)?;
        if mbr[510..] != [0x55, 0xaa] {
            return err("Card lacks MBR signature");
        }
        let parts: Vec<_> = (0..4)
            .map(|i| 446 + i * 16)
            .filter(|&p| matches!(mbr[p + 4], 0x0b | 0x0c) && u32_at(mbr, p + 12) != 0)
            .collect();
        if parts.len() != 1 {
            return err("Expected one FAT32 partition");
        }
        let start = u32_at(mbr, parts[0] + 8);
        let count = u32_at(mbr, parts[0] + 12);
        if start == 0
            || start
                .checked_add(count)
                .is_none_or(|end| end > card.sectors())
        {
            return err("FAT32 partition exceeds card");
        }
        let boot = card.sector(start)?;
        let spc = boot[13] as u32;
        let reserved = u16_at(boot, 14) as u32;
        let fats = boot[16] as u32;
        let total = u32_at(boot, 32);
        let fat_sectors = u32_at(boot, 36);
        let flags = u16_at(boot, 40);
        let root = u32_at(boot, 44);
        if boot[510..] != [0x55, 0xaa]
            || u16_at(boot, 11) != 512
            || !spc.is_power_of_two()
            || spc > 128
            || reserved == 0
            || !(1..=2).contains(&fats)
            || fat_sectors == 0
            || u16_at(boot, 42) != 0
            || total == 0
            || total > count
            || boot[17..21] != [0; 4]
            || boot[22..24] != [0; 2]
        {
            return err("Unsupported FAT32 boot geometry");
        }
        let fat = start + reserved;
        let data: u32 = (fat as u64 + fats as u64 * fat_sectors as u64)
            .try_into()
            .map_err(|_| "FAT32 geometry overflow")?;
        let clusters = (start + total)
            .checked_sub(data)
            .ok_or("FAT32 tables exceed partition")?
            / spc;
        let active = if flags & 0x80 != 0 {
            Some((flags & 15) as u32)
        } else {
            None
        };
        if !(65525..0x0ffffff5).contains(&clusters)
            || fat_sectors as u64 * 128 < clusters as u64 + 2
            || active.is_some_and(|n| n >= fats)
            || !(2..clusters + 2).contains(&root)
        {
            return err("FAT32 cluster table out of bounds");
        }
        Ok(Self {
            card,
            start,
            fat,
            data,
            spc,
            fats,
            fat_sectors,
            clusters,
            active,
            root,
            reserved,
        })
    }
    fn allocation(&self, cluster: u32) -> Result<u32, String> {
        if !(2..self.clusters + 2).contains(&cluster) {
            return err("FAT32 cluster outside data region");
        }
        let mut previous = None;
        for copy in 0..self.fats {
            if self.active.is_some_and(|n| n != copy) {
                continue;
            }
            let sector = self
                .card
                .sector(self.fat + copy * self.fat_sectors + cluster / 128)?;
            let value = u32_at(sector, (cluster % 128 * 4) as usize) & 0x0fffffff;
            if previous.is_some_and(|p| p != value) {
                return err("Mirrored FAT entries disagree");
            }
            previous = Some(value);
        }
        Ok(previous.unwrap())
    }
    fn next(&self, cluster: u32) -> Result<Option<u32>, String> {
        let value = self.allocation(cluster)?;
        if value >= 0x0ffffff8 {
            return Ok(None);
        }
        if !(2..self.clusters + 2).contains(&value) {
            return err("Incomplete, bad or reserved FAT chain");
        }
        Ok(Some(value))
    }
    fn chain(&self, first: u32, limit: usize) -> Result<Vec<u32>, String> {
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut next = Some(first);
        while let Some(cluster) = next {
            if chain.len() >= limit || !seen.insert(cluster) {
                return err("FAT chain cycles or exceeds bound");
            }
            next = self.next(cluster)?;
            chain.push(cluster);
        }
        Ok(chain)
    }
    fn directory(&self, first: u32) -> Result<Vec<DirEntry>, String> {
        let mut entries = Vec::new();
        let mut names = BTreeSet::new();
        // Validate the complete directory chain even if its first sector ends early.
        let mut long = long_names::Pending::default();
        for cluster in self.chain(first, 64)? {
            for sector in 0..self.spc {
                let block = self
                    .card
                    .sector(self.data + (cluster - 2) * self.spc + sector)?;
                for raw in block.as_chunks::<32>().0 {
                    if raw[0] == 0 {
                        return Ok(entries);
                    }
                    if raw[0] == 0xe5 || raw[11] & 8 != 0 && raw[11] != 15 {
                        long.clear();
                        continue;
                    }
                    if raw[11] == 15 {
                        long.push(raw)?;
                        continue;
                    }
                    if !raw[..11].is_ascii() {
                        return err("Non-ASCII FAT short name");
                    }
                    let base = std::str::from_utf8(&raw[..8]).unwrap().trim_end();
                    let ext = std::str::from_utf8(&raw[8..11]).unwrap().trim_end();
                    if matches!(base, "." | "..") {
                        continue;
                    }
                    let name = if ext.is_empty() {
                        base.to_owned()
                    } else {
                        format!("{base}.{ext}")
                    };
                    let name = long.finish(&raw[..11])?.unwrap_or(name);
                    if name.is_empty() || !names.insert(name.clone()) {
                        return err("Empty or duplicate FAT short name");
                    }
                    entries.push(DirEntry {
                        name,
                        attributes: raw[11],
                        first: ((u16_at(raw, 20) as u32) << 16) | u16_at(raw, 26) as u32,
                        size: u32_at(raw, 28),
                    });
                    if entries.len() > MAX_ENTRIES {
                        return err("Directory exceeds entry bound");
                    }
                }
            }
        }
        Ok(entries)
    }
    pub fn read(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        if !(1..=MAX_BYTES).contains(&max_bytes) {
            return err("File read bound must be 1..64 MiB");
        }
        let path = normalize_path(path, true)?;
        let parts: Vec<_> = path.split('/').collect();
        let mut cluster = self.root;
        let mut visited = BTreeSet::new();
        for (index, part) in parts.iter().enumerate() {
            if !visited.insert(cluster) {
                return err("Directory path cycles");
            }
            let entry = self
                .directory(cluster)?
                .into_iter()
                .find(|e| e.name == *part)
                .ok_or("Card path not found")?;
            if index + 1 < parts.len() {
                if entry.attributes & 0x10 == 0 {
                    return err("Path component is not a directory");
                }
                cluster = entry.first;
                continue;
            }
            if entry.attributes & 0x10 != 0 || entry.size as usize > max_bytes {
                return err("Path is a directory or exceeds read bound");
            }
            if entry.size == 0 {
                if entry.first != 0 {
                    return err("Empty file has an allocated cluster");
                }
                return Ok(Vec::new());
            }
            let allocation =
                self.chain(entry.first, max_bytes.div_ceil(self.spc as usize * 512))?;
            if allocation.len() * (self.spc as usize * 512) < entry.size as usize {
                return err("FAT chain does not cover file size");
            }
            let mut bytes = vec![0; entry.size as usize];
            let mut used = 0;
            for cluster in allocation {
                let size = (self.spc as usize * 512).min(bytes.len() - used);
                if size == 0 {
                    break;
                }
                self.card.read_bytes(
                    (self.data + (cluster - 2) * self.spc) as u64 * 512,
                    &mut bytes[used..used + size],
                )?;
                used += size;
            }
            return Ok(bytes);
        }
        err("Empty card path")
    }
    pub fn tree(&self) -> Result<Vec<CardEntry>, String> {
        self.walk(self.root, "", 0, &mut BTreeSet::new(), &mut 0)
    }
    fn walk(
        &self,
        cluster: u32,
        parent: &str,
        depth: usize,
        visited: &mut BTreeSet<u32>,
        count: &mut usize,
    ) -> Result<Vec<CardEntry>, String> {
        if depth > 8 || !visited.insert(cluster) {
            return err("Directory tree cycles or aliases an earlier directory");
        }
        let chain = self.chain(cluster, 64)?;
        for &tail in &chain[1..] {
            if !visited.insert(tail) {
                return err("Directory chains overlap");
            }
        }
        let mut entries = Vec::new();
        for raw in self.directory(cluster)? {
            *count += 1;
            if depth >= 8 || *count > MAX_ENTRIES {
                return err("Card tree exceeds depth or entry bound");
            }
            let path = format!("{parent}/{}", raw.name);
            let kind = if raw.attributes & 0x10 != 0 {
                CardEntryKind::Directory
            } else {
                CardEntryKind::File
            };
            let children = if kind == CardEntryKind::Directory {
                self.walk(raw.first, &path, depth + 1, visited, count)?
            } else {
                Vec::new()
            };
            entries.push(CardEntry {
                name: raw.name,
                path,
                kind,
                bytes: raw.size,
                children,
            });
        }
        entries.sort_by(|a, b| {
            (a.kind != CardEntryKind::Directory, &a.name)
                .cmp(&(b.kind != CardEntryKind::Directory, &b.name))
        });
        Ok(entries)
    }
}
fn normalize_path(path: &str, drive: bool) -> Result<String, String> {
    if path.is_empty() || path.len() > 260 || !path.is_ascii() {
        return err("Expected bounded ASCII path");
    }
    let replaced = path.replace('\\', "/").to_ascii_uppercase();
    let mut path = replaced.trim_start_matches('/');
    if drive && path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_uppercase() {
        path = path[2..].trim_start_matches('/');
    }
    let parts: Vec<_> = path.split('/').collect();
    if !(1..=8).contains(&parts.len()) || parts.iter().any(|p| !long_names::valid(p)) {
        return err("Expected up to eight valid ASCII path components");
    }
    Ok(parts.join("/"))
}
fn short_name(name: &str) -> Result<[u8; 11], String> {
    let mut split = name.split('.');
    let base = split.next().unwrap_or("");
    let ext = split.next().unwrap_or("");
    if base.is_empty()
        || base.len() > 8
        || ext.len() > 3
        || split.next().is_some()
        || !base
            .bytes()
            .chain(ext.bytes())
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'~'))
    {
        return err("Invalid 8.3 name");
    }
    let mut bytes = [b' '; 11];
    bytes[..base.len()].copy_from_slice(base.as_bytes());
    bytes[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Ok(bytes)
}
struct Folder {
    cluster: u32,
    bytes: Vec<u8>,
    used: usize,
}
fn dir_record(name: [u8; 11], attr: u8, first: u32, size: u32) -> [u8; 32] {
    let mut raw = [0; 32];
    raw[..11].copy_from_slice(&name);
    raw[11] = attr;
    for offset in [16, 18, 24] {
        put16(&mut raw, offset, 0x5321);
    }
    put16(&mut raw, 20, (first >> 16) as u16);
    put16(&mut raw, 26, first as u16);
    put32(&mut raw, 28, size);
    raw
}
fn add(folder: &mut Folder, record: &[u8; 32]) -> Result<(), String> {
    if (folder.used + 1) * 32 > folder.bytes.len() {
        return err("Formatter directory exceeds one cluster");
    }
    folder.bytes[folder.used * 32..(folder.used + 1) * 32].copy_from_slice(record);
    folder.used += 1;
    Ok(())
}
/// Validate path conflicts and input bounds before allocating a medium.
pub fn normalize_paths(files: &[(String, Vec<u8>)]) -> Result<Vec<String>, String> {
    let mut paths = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut normalized = Vec::new();
    let mut bytes = 0usize;
    for (path, payload) in files {
        let path = normalize_path(path, false)?;
        let parts: Vec<_> = path.split('/').collect();
        let parents: Vec<_> = (1..parts.len()).map(|i| parts[..i].join("/")).collect();
        bytes = bytes
            .checked_add(payload.len())
            .ok_or("Preload size overflows")?;
        if bytes > MAX_PRELOAD_BYTES
            || paths.len() >= MAX_ENTRIES
            || paths.contains(&path)
            || directories.contains(&path)
            || parents.iter().any(|p| paths.contains(p))
        {
            return err("Preload exceeds bounds or paths conflict");
        }
        paths.insert(path.clone());
        directories.extend(parents);
        normalized.push(path);
    }
    if paths.len() + directories.len() > MAX_ENTRIES {
        return err("Preload exceeds directory entry bound");
    }
    Ok(normalized)
}
/// Construct a new medium atomically. Existing media is never rebuilt to insert
/// files into a mounted filesystem; session insertion replaces the external part.
pub fn format(options: &Options, files: &[(String, Vec<u8>)]) -> Result<Card, String> {
    let mut card = Card::new(CardOptions {
        sectors: options.sectors,
        max_dirty_sectors: options.max_dirty_sectors,
    })?;
    if !options.sectors_per_cluster.is_power_of_two()
        || options.sectors_per_cluster > 128
        || options.partition_start == 0
        || options.partition_start >= options.sectors
        || options.label.len() > 11
        || !options.label.is_ascii()
    {
        return err("Invalid FAT32 format geometry or label");
    }
    let spc = options.sectors_per_cluster as u32;
    let start = options.partition_start;
    let total = options.sectors - start;
    let fat_sectors = ((total / spc) as u64 * 4).div_ceil(512) as u32;
    let relative_data = 32 + 2 * fat_sectors;
    let clusters = total
        .checked_sub(relative_data)
        .ok_or("FAT tables exceed partition")?
        / spc;
    if clusters < 65525 {
        return err("Geometry is not FAT32");
    }
    let normalized = normalize_paths(files)?;
    let mut reserved = BTreeMap::<String, BTreeSet<[u8; 11]>>::new();
    for path in &normalized {
        let mut parent = String::new();
        for part in path.split('/') {
            if let Ok(short) = short_name(part) {
                reserved.entry(parent.clone()).or_default().insert(short);
            }
            parent = if parent.is_empty() {
                part.into()
            } else {
                format!("{parent}/{part}")
            };
        }
    }
    let read_only = options
        .read_only_files
        .iter()
        .map(|path| normalize_path(path, false))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if read_only.iter().any(|path| !normalized.contains(path)) {
        return err("Read-only path is absent from the formatted files");
    }
    let mut mbr = [0; 512];
    mbr[450] = 0x0c;
    mbr[447..450].copy_from_slice(&[0xfe, 0xff, 0xff]);
    mbr[451..454].copy_from_slice(&[0xfe, 0xff, 0xff]);
    put32(&mut mbr, 454, start);
    put32(&mut mbr, 458, total);
    mbr[510..].copy_from_slice(&[0x55, 0xaa]);
    card.write_bytes(0, &mbr)?;
    let mut boot = [0; 512];
    boot[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    boot[3..11].copy_from_slice(b"MSWIN4.1");
    put16(&mut boot, 11, 512);
    boot[13] = spc as u8;
    put16(&mut boot, 14, 32);
    boot[16] = 2;
    boot[21] = 0xf8;
    put16(&mut boot, 24, 63);
    put16(&mut boot, 26, 255);
    put32(&mut boot, 28, start);
    put32(&mut boot, 32, total);
    put32(&mut boot, 36, fat_sectors);
    put32(&mut boot, 44, 2);
    put16(&mut boot, 48, 1);
    put16(&mut boot, 50, 6);
    boot[64] = 0x80;
    boot[66] = 0x29;
    put32(&mut boot, 67, 0x70707070);
    boot[71..82].fill(b' ');
    boot[71..71 + options.label.len()].copy_from_slice(options.label.as_bytes());
    boot[82..90].copy_from_slice(b"FAT32   ");
    boot[510..].copy_from_slice(&[0x55, 0xaa]);
    for sector in [start, start + 6] {
        card.write_bytes(sector as u64 * 512, &boot)?;
    }
    let mut info = [0; 512];
    put32(&mut info, 0, 0x41615252);
    put32(&mut info, 484, 0x61417272);
    put32(&mut info, 488, u32::MAX);
    put32(&mut info, 492, u32::MAX);
    info[510..].copy_from_slice(&[0x55, 0xaa]);
    card.write_bytes((start + 1) as u64 * 512, &info)?;
    let cluster_bytes = spc as usize * 512;
    let mut root = Folder {
        cluster: 2,
        bytes: vec![0; cluster_bytes],
        used: 1,
    };
    root.bytes[..11].copy_from_slice(&boot[71..82]);
    root.bytes[11] = 8;
    let mut folders = BTreeMap::from([(String::new(), root)]);
    let mut fat = BTreeMap::from([(0, 0x0ffffff8u32), (1, 0x0fffffff), (2, 0x0fffffff)]);
    let mut next = 3u32;
    for (path, (_, payload)) in normalized.into_iter().zip(files) {
        let mut parts: Vec<_> = path.split('/').collect();
        let leaf = parts.pop().unwrap();
        let mut parent = String::new();
        for part in parts {
            let current = if parent.is_empty() {
                part.to_owned()
            } else {
                format!("{parent}/{part}")
            };
            if !folders.contains_key(&current) {
                let cluster = next;
                next += 1;
                fat.insert(cluster, 0x0fffffff);
                let mut folder = Folder {
                    cluster,
                    bytes: vec![0; cluster_bytes],
                    used: 0,
                };
                let mut dot = [b' '; 11];
                dot[0] = b'.';
                add(&mut folder, &dir_record(dot, 0x10, cluster, 0))?;
                dot[1] = b'.';
                add(
                    &mut folder,
                    &dir_record(
                        dot,
                        0x10,
                        if parent.is_empty() {
                            0
                        } else {
                            folders[&parent].cluster
                        },
                        0,
                    ),
                )?;
                let (short, records) =
                    long_names::records(part, reserved.entry(parent.clone()).or_default())?;
                for record in records {
                    add(folders.get_mut(&parent).unwrap(), &record)?;
                }
                add(
                    folders.get_mut(&parent).unwrap(),
                    &dir_record(short, 0x10, cluster, 0),
                )?;
                folders.insert(current.clone(), folder);
            }
            parent = current;
        }
        let count = payload.len().div_ceil(cluster_bytes) as u32;
        let first = if count == 0 { 0 } else { next };
        if next as u64 + count as u64 > clusters as u64 + 2 {
            return err("Files exceed FAT32 volume");
        }
        for cluster in next..next + count {
            fat.insert(
                cluster,
                if cluster + 1 < next + count {
                    cluster + 1
                } else {
                    0x0fffffff
                },
            );
        }
        let (short, records) =
            long_names::records(leaf, reserved.entry(parent.clone()).or_default())?;
        for record in records {
            add(folders.get_mut(&parent).unwrap(), &record)?;
        }
        add(
            folders.get_mut(&parent).unwrap(),
            &dir_record(
                short,
                0x20 | u8::from(read_only.contains(&path)),
                first,
                payload.len() as u32,
            ),
        )?;
        if count > 0 {
            for (i, chunk) in payload.chunks(65536 * 512).enumerate() {
                card.write_bytes(
                    (start + relative_data + (first - 2) * spc) as u64 * 512
                        + (i * 65536 * 512) as u64,
                    chunk,
                )?;
            }
        }
        next += count;
    }
    if next - 2 > clusters {
        return err("Directories exceed FAT32 volume");
    }
    for folder in folders.values() {
        card.write_bytes(
            (start + relative_data + (folder.cluster - 2) * spc) as u64 * 512,
            &folder.bytes,
        )?;
    }
    for (cluster, value) in fat {
        for copy in 0..2 {
            card.write_bytes(
                (start + 32 + copy * fat_sectors) as u64 * 512 + cluster as u64 * 4,
                &value.to_le_bytes(),
            )?;
        }
    }
    Ok(card)
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn options() -> Options {
        Options {
            sectors: 0x800000,
            max_dirty_sectors: 65536,
            partition_start: 8192,
            sectors_per_cluster: 64,
            label: "CAMERA".into(),
            read_only_files: Vec::new(),
        }
    }
    #[test]
    fn nested_files_empty_files_tree_and_observation() {
        let content: Vec<_> = (0..70001).map(|n| n as u8).collect();
        let card = format(
            &options(),
            &[
                ("DCIM/100_FUJI/DSCF0001.JPG".into(), content.clone()),
                ("EMPTY.BIN".into(), vec![]),
            ],
        )
        .unwrap();
        let revision = card.revision();
        let reader = Reader::new(&card).unwrap();
        assert_eq!(
            reader
                .read("A:/dcim/100_fuji/dscf0001.jpg", MAX_BYTES)
                .unwrap(),
            content
        );
        assert!(reader.read("DCIM/100_FUJI/DSCF0001.JPG", 10).is_err());
        assert!(reader.read("../EMPTY.BIN", MAX_BYTES).is_err());
        assert_eq!(reader.read("EMPTY.BIN", 1).unwrap(), Vec::<u8>::new());
        let tree = reader.tree().unwrap();
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].kind, CardEntryKind::Directory);
        assert_eq!(tree[0].children[0].children[0].bytes, 70001);
        assert_eq!(card.revision(), revision);
    }
    #[test]
    fn malformed_geometry_mirrors_and_cycles_refuse() {
        let mut card = format(&options(), &[("A.BIN".into(), vec![1; 40000])]).unwrap();
        let reader = Reader::new(&card).unwrap();
        let fat = reader.fat;
        let fs = reader.fat_sectors;
        card.write_bytes((fat + fs) as u64 * 512 + 12, &0x0fffffffu32.to_le_bytes())
            .unwrap();
        assert!(
            Reader::new(&card)
                .unwrap()
                .read("A.BIN", MAX_BYTES)
                .unwrap_err()
                .contains("Mirrored")
        );
        for copy in 0..2 {
            card.write_bytes((fat + copy * fs) as u64 * 512 + 12, &3u32.to_le_bytes())
                .unwrap();
        }
        assert!(
            Reader::new(&card)
                .unwrap()
                .read("A.BIN", MAX_BYTES)
                .unwrap_err()
                .contains("cycles")
        );
        card.write_bytes(
            (options().partition_start as u64) * 512 + 36,
            &u32::MAX.to_le_bytes(),
        )
        .unwrap();
        assert!(Reader::new(&card).is_err());
    }
    #[test]
    fn conflicting_preloads_and_invalid_geometry_refuse() {
        assert!(format(&options(), &[("A".into(), vec![]), ("A/B".into(), vec![])]).is_err());
        assert!(format(&options(), &[("A/B".into(), vec![]), ("A".into(), vec![])]).is_err());
        let mut o = options();
        o.sectors_per_cluster = 128;
        assert!(format(&o, &[]).is_err());
    }

    #[test]
    fn startup_card_can_place_the_update_after_a_boot_file_larger_than_64_mib() {
        let boot_size = 65 * 1024 * 1024;
        let card = format(
            &options(),
            &[
                ("FFBOOT.BIN".into(), vec![0; boot_size]),
                ("FWUP0008.DAT".into(), vec![0x59; 512]),
            ],
        )
        .unwrap();
        let reader = Reader::new(&card).unwrap();
        let tree = reader.tree().unwrap();
        assert_eq!(
            tree.iter().find(|f| f.name == "FFBOOT.BIN").unwrap().bytes as usize,
            boot_size
        );
        assert_eq!(reader.read("FWUP0008.DAT", 512).unwrap(), vec![0x59; 512]);
        assert!(reader.read("FFBOOT.BIN", MAX_BYTES).is_err());
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    #[test]
    fn long_names_round_trip_and_reserve_explicit_short_aliases() {
        let card = format(
            &super::tests::options(),
            &[
                ("MARBLE_INIT.BIN".into(), vec![1, 2, 3]),
                ("MARBLE~1.BIN".into(), vec![4]),
                ("LONG_DIRECTORY/THIRTEENCHARS".into(), vec![5]),
            ],
        )
        .unwrap();
        let reader = Reader::new(&card).unwrap();
        assert_eq!(reader.read("marble_init.bin", 3).unwrap(), [1, 2, 3]);
        assert_eq!(reader.read("MARBLE~1.BIN", 1).unwrap(), [4]);
        assert_eq!(reader.read("LONG_DIRECTORY/THIRTEENCHARS", 1).unwrap(), [5]);
        let offset = reader.data as u64 * 512 + 32 + 13;
        let mut broken = card.clone();
        broken.write_bytes(offset, &[0]).unwrap();
        assert!(
            Reader::new(&broken)
                .unwrap()
                .tree()
                .unwrap_err()
                .contains("long-name")
        );
    }
    #[test]
    fn formatter_keeps_declared_read_only_attributes_and_short_aliases() {
        let options = Options {
            sectors: 0x800000,
            max_dirty_sectors: 65536,
            partition_start: 8192,
            sectors_per_cluster: 64,
            label: "TEST".into(),
            read_only_files: vec!["/KODACH~1.JSO".into()],
        };
        let card = format(&options, &[("KODACH~1.JSO".into(), b"recipe".to_vec())]).unwrap();
        let reader = Reader::new(&card).unwrap();
        let entries = reader.directory(2).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.name == "KODACH~1.JSO" && e.attributes & 1 != 0)
        );
        assert_eq!(reader.read("/kodach~1.jso", 100).unwrap(), b"recipe");
        assert_eq!(reader.geometry().partition_start, 8192);
    }
}
