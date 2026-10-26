//! Atomic host insertion into existing FAT media before a cold mount.
//! Unchanged directory entries, file bytes and cluster assignments survive.
use super::*;

pub fn merge(card: &Card, files: &[(String, Vec<u8>)]) -> Result<Card, String> {
    if files.is_empty() {
        return Ok(card.clone());
    }
    let mut names = BTreeSet::new();
    let mut bytes = 0usize;
    for (path, data) in files {
        if !names.insert(normalize_path(path, true)?) {
            return err("Repeated merged card path");
        }
        bytes = bytes
            .checked_add(data.len())
            .ok_or("Merged card bytes overflow")?;
        if bytes > MAX_BYTES || files.len() > MAX_ENTRIES {
            return err("Merged card files exceed bounds");
        }
    }
    let mut editor = Editor {
        card: card.clone(),
        hint: 2,
    };
    // Reject cross-linked chains before replacing or releasing any allocation.
    let reader = Reader::new(&editor.card)?;
    reader.tree()?;
    claim(&reader, reader.root, &mut BTreeSet::new(), 0)?;
    for (path, bytes) in files {
        editor.write(&normalize_path(path, true)?, bytes)?;
    }
    editor.invalidate_free_count()?;
    Reader::new(&editor.card)?.tree()?;
    Ok(editor.card)
}
fn claim(
    reader: &Reader<'_>,
    directory: u32,
    used: &mut BTreeSet<u32>,
    depth: usize,
) -> Result<(), String> {
    if depth > 8 {
        return err("Merged directory depth exceeds bound");
    }
    for cluster in reader.chain(directory, 64)? {
        if !used.insert(cluster) {
            return err("Cross-linked FAT directory");
        }
    }
    for entry in reader.directory(directory)? {
        if entry.attributes & 0x10 != 0 {
            claim(reader, entry.first, used, depth + 1)?;
        } else if entry.size != 0 {
            let limit = (entry.size as usize).div_ceil(reader.spc as usize * 512);
            if entry.size as usize > MAX_BYTES {
                return err("Existing FAT file exceeds merge bound");
            }
            let chain = reader.chain(entry.first, MAX_BYTES.div_ceil(reader.spc as usize * 512))?;
            if chain.len() < limit {
                return err("Existing FAT file allocation is shorter than size");
            }
            for cluster in chain {
                if !used.insert(cluster) {
                    return err("Cross-linked FAT file");
                }
            }
        } else if entry.first != 0 {
            return err("Empty FAT file has an allocation");
        }
    }
    Ok(())
}
struct Editor {
    card: Card,
    hint: u32,
}
impl Editor {
    fn chain(&self, first: u32) -> Result<Vec<u32>, String> {
        Reader::new(&self.card)?.chain(first, MAX_BYTES / 512)
    }
    fn set_allocation(&mut self, cluster: u32, value: u32) -> Result<(), String> {
        let r = Reader::new(&self.card)?;
        let (fat, fats, size, active) = (r.fat, r.fats, r.fat_sectors, r.active);
        r.allocation(cluster)?;
        for copy in 0..fats {
            if active.is_some_and(|n| n != copy) {
                continue;
            }
            let offset =
                (fat + copy * size + cluster / 128) as u64 * 512 + (cluster % 128 * 4) as u64;
            let mut old = [0; 4];
            self.card.read_bytes(offset, &mut old)?;
            let value = (u32::from_le_bytes(old) & 0xf0000000) | (value & 0x0fffffff);
            self.card.write_bytes(offset, &value.to_le_bytes())?;
        }
        Ok(())
    }
    fn cluster_offset(&self, cluster: u32) -> Result<u64, String> {
        let r = Reader::new(&self.card)?;
        Ok((r.data + (cluster - 2) * r.spc) as u64 * 512)
    }
    fn allocate(&mut self) -> Result<u32, String> {
        let r = Reader::new(&self.card)?;
        let end = r.clusters + 2;
        let found = (self.hint..end)
            .chain(2..self.hint)
            .find_map(|c| match r.allocation(c) {
                Ok(0) => Some(Ok(c)),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            })
            .ok_or("FAT medium has no free cluster")??;
        self.hint = if found + 1 < end { found + 1 } else { 2 };
        let bytes = vec![0; r.spc as usize * 512];
        self.set_allocation(found, 0x0fffffff)?;
        self.card.write_bytes(self.cluster_offset(found)?, &bytes)?;
        Ok(found)
    }
    fn slots(&self, dir: u32) -> Result<Vec<u64>, String> {
        let r = Reader::new(&self.card)?;
        let mut offsets = Vec::new();
        for cluster in r.chain(dir, 64)? {
            let base = (r.data + (cluster - 2) * r.spc) as u64 * 512;
            offsets.extend((0..r.spc * 16).map(|n| base + n as u64 * 32));
        }
        Ok(offsets)
    }
    fn record(&self, at: u64) -> Result<[u8; 32], String> {
        let mut bytes = [0; 32];
        self.card.read_bytes(at, &mut bytes)?;
        Ok(bytes)
    }
    fn find(&self, dir: u32, name: [u8; 11]) -> Result<Option<(u64, [u8; 32])>, String> {
        for at in self.slots(dir)? {
            let raw = self.record(at)?;
            if raw[0] == 0 {
                break;
            }
            if raw[0] != 0xe5 && raw[11] != 15 && raw[11] & 8 == 0 && raw[..11] == name {
                return Ok(Some((at, raw)));
            }
        }
        Ok(None)
    }
    fn vacant(&mut self, dir: u32) -> Result<u64, String> {
        let slots = self.slots(dir)?;
        let mut deleted = None;
        for (n, &at) in slots.iter().enumerate() {
            let raw = self.record(at)?;
            if raw[0] == 0 {
                if let Some(at) = deleted {
                    return Ok(at);
                }
                if let Some(&next) = slots.get(n + 1) {
                    self.card.write_bytes(next, &[0; 32])?;
                }
                return Ok(at);
            }
            if raw[0] == 0xe5 && deleted.is_none() {
                deleted = Some(at);
            }
        }
        if let Some(at) = deleted {
            return Ok(at);
        }
        let chain = Reader::new(&self.card)?.chain(dir, 64)?;
        if chain.len() >= 64 {
            return err("FAT directory exceeds merge allocation bound");
        }
        let new = self.allocate()?;
        self.set_allocation(*chain.last().unwrap(), new)?;
        self.cluster_offset(new)
    }
    fn first(raw: &[u8; 32]) -> u32 {
        ((u16_at(raw, 20) as u32) << 16) | u16_at(raw, 26) as u32
    }
    fn directory(&mut self, parent: u32, name: &str) -> Result<u32, String> {
        let short = short_name(name)?;
        if let Some((_, raw)) = self.find(parent, short)? {
            if raw[11] & 0x10 == 0 {
                return err("Merged parent is a file");
            }
            return Ok(Self::first(&raw));
        }
        let at = self.vacant(parent)?;
        let first = self.allocate()?;
        let base = self.cluster_offset(first)?;
        let mut dot = [b' '; 11];
        dot[0] = b'.';
        let mut up = dot;
        up[1] = b'.';
        let root = Reader::new(&self.card)?.root;
        self.card
            .write_bytes(base, &dir_record(dot, 0x10, first, 0))?;
        self.card.write_bytes(
            base + 32,
            &dir_record(up, 0x10, if parent == root { 0 } else { parent }, 0),
        )?;
        self.card
            .write_bytes(at, &dir_record(short, 0x10, first, 0))?;
        Ok(first)
    }
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let mut parts = path.split('/').collect::<Vec<_>>();
        let name = short_name(parts.pop().unwrap())?;
        let mut parent = Reader::new(&self.card)?.root;
        for part in parts {
            parent = self.directory(parent, part)?;
        }
        let (at, mut raw) = if let Some((at, raw)) = self.find(parent, name)? {
            if raw[11] & 0x11 != 0 {
                return err("Merged target is a directory or read-only file");
            }
            if Reader::new(&self.card)?.read(path, MAX_BYTES)? == bytes {
                return Ok(());
            }
            if Self::first(&raw) != 0 {
                for cluster in self.chain(Self::first(&raw))? {
                    self.set_allocation(cluster, 0)?;
                    self.hint = self.hint.min(cluster);
                }
            }
            (at, raw)
        } else {
            (self.vacant(parent)?, dir_record(name, 0x20, 0, 0))
        };
        let cluster_bytes = Reader::new(&self.card)?.spc as usize * 512;
        let mut first = 0;
        let mut last = None;
        for part in bytes.chunks(cluster_bytes) {
            let cluster = self.allocate()?;
            if let Some(old) = last {
                self.set_allocation(old, cluster)?;
            } else {
                first = cluster;
            }
            last = Some(cluster);
            self.card.write_bytes(self.cluster_offset(cluster)?, part)?;
        }
        put16(&mut raw, 20, (first >> 16) as u16);
        put16(&mut raw, 26, first as u16);
        put32(&mut raw, 28, bytes.len() as u32);
        self.card.write_bytes(at, &raw)
    }
    fn invalidate_free_count(&mut self) -> Result<(), String> {
        let r = Reader::new(&self.card)?;
        let (start, reserved) = (r.start, r.reserved);
        let boot = self.card.sector(start)?;
        let fs = u16_at(boot, 48) as u32;
        let backup = u16_at(boot, 50) as u32;
        for relative in [Some(fs), (backup < reserved).then_some(backup + fs)]
            .into_iter()
            .flatten()
        {
            if relative == 0 || relative >= reserved {
                continue;
            }
            let lba = start + relative;
            let block = self.card.sector(lba)?;
            if block[..4] == *b"RRaA" && block[484..488] == *b"rrAa" {
                self.card.write_bytes(lba as u64 * 512 + 488, &[255; 8])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn medium() -> Card {
        format(
            &Options {
                sectors: 0x400000,
                max_dirty_sectors: 65536,
                partition_start: 2048,
                sectors_per_cluster: 1,
                label: "FILES".into(),
                read_only_files: vec![],
            },
            &[
                ("/DCIM/100_FUJI/A.JPG".into(), vec![37; 1700]),
                ("/KEEP.TXT".into(), b"retained".to_vec()),
            ],
        )
        .unwrap()
    }
    #[test]
    fn merge_preserves_other_files_and_directory_metadata_and_replaces_allocation() {
        let original = medium();
        let before = serde_json::to_value(&original).unwrap();
        let root = Reader::new(&original).unwrap().root;
        let root_bytes = original
            .sector(Reader::new(&original).unwrap().data + (root - 2))
            .unwrap()
            .to_vec();
        let changed = merge(
            &original,
            &[
                ("/DCIM/100_FUJI/A.JPG".into(), vec![89; 3200]),
                ("/MODS/TEST.BIN".into(), vec![4; 900]),
            ],
        )
        .unwrap();
        let r = Reader::new(&changed).unwrap();
        assert_eq!(r.read("/KEEP.TXT", 100).unwrap(), b"retained");
        assert_eq!(
            r.read("/DCIM/100_FUJI/A.JPG", 5000).unwrap(),
            vec![89; 3200]
        );
        assert_eq!(r.read("/MODS/TEST.BIN", 1000).unwrap(), vec![4; 900]);
        // Existing DCIM and KEEP directory records retain their exact bytes.
        let newroot = changed.sector(r.data + (r.root - 2)).unwrap();
        assert_eq!(&newroot[..96], &root_bytes[..96]);
        assert_eq!(serde_json::to_value(&original).unwrap(), before);
        assert!(changed.revision() > original.revision());
        let changed = merge(&changed, &[("/DCIM/100_FUJI/A.JPG".into(), vec![])]).unwrap();
        assert!(
            Reader::new(&changed)
                .unwrap()
                .read("/DCIM/100_FUJI/A.JPG", 100)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn full_directory_grows_and_failed_merge_is_atomic() {
        let original = medium();
        let files = (0..40)
            .map(|n| (format!("/NEW/F{n:03}.BIN"), vec![n as u8; 700]))
            .collect::<Vec<_>>();
        let changed = merge(&original, &files).unwrap();
        for (name, data) in &files {
            assert_eq!(
                &Reader::new(&changed).unwrap().read(name, 1000).unwrap(),
                data
            );
        }
        let before = serde_json::to_value(&changed).unwrap();
        assert!(merge(&changed, &[("/NEW/F001.BIN/CHILD".into(), vec![9])]).is_err());
        assert_eq!(serde_json::to_value(&changed).unwrap(), before);
        assert!(
            merge(
                &changed,
                &[("/A.TXT".into(), vec![]), ("/a.txt".into(), vec![])]
            )
            .is_err()
        );
    }
}
