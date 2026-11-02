use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
    files: BTreeMap<String, Vec<u8>>,
    calls: Vec<(String, u32, usize)>,
    lists: usize,
    fail_write: bool,
    memory_reads: u8,
}
#[derive(Clone, Default)]
struct TestDevice(Arc<Mutex<State>>);
impl Device for TestDevice {
    fn list(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let mut s = self.0.lock().unwrap();
        s.lists += 1;
        let prefix = format!("{}\\", path.trim_end_matches('\\'));
        let mut entries = BTreeMap::new();
        for (name, data) in &s.files {
            if let Some(name) = name.strip_prefix(&prefix) {
                let (name, directory) = name
                    .split_once('\\')
                    .map_or((name, false), |(n, _)| (n, true));
                entries.insert(
                    name.to_owned(),
                    DirectoryEntry {
                        name: name.into(),
                        bytes: if directory { 0 } else { data.len() as u32 },
                        directory,
                        read_only: false,
                    },
                );
            }
        }
        Ok(entries.into_values().collect())
    }
    fn read(&mut self, path: &str, offset: u32, size: u32) -> Result<Vec<u8>> {
        let mut s = self.0.lock().unwrap();
        s.calls
            .push((format!("read:{path}"), offset, size as usize));
        let data = s.files.get(path).ok_or(libc::ENOENT)?;
        let offset = (offset as usize).min(data.len());
        Ok(data[offset..data.len().min(offset + size as usize)].to_vec())
    }
    fn write(&mut self, path: &str, offset: u32, data: &[u8]) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        if s.fail_write {
            return Err(libc::EIO);
        }
        s.calls.push((format!("write:{path}"), offset, data.len()));
        let file = s.files.get_mut(path).ok_or(libc::ENOENT)?;
        // Deliberately don't promise zeroes in an unwritten seek gap.
        file.resize(file.len().max(offset as usize + data.len()), 0xa5);
        file[offset as usize..offset as usize + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn create(&mut self, path: &str) -> Result<()> {
        self.0.lock().unwrap().files.insert(path.into(), Vec::new());
        Ok(())
    }
    fn shrink(&mut self, path: &str, size: u32) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .files
            .get_mut(path)
            .ok_or(libc::ENOENT)?
            .truncate(size as usize);
        Ok(())
    }
    fn delete(&mut self, path: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .files
            .remove(path)
            .ok_or(libc::ENOENT)?;
        Ok(())
    }
    fn memory(&mut self, address: u32, size: u32) -> Result<Vec<u8>> {
        let mut s = self.0.lock().unwrap();
        s.memory_reads += 1;
        s.calls.push(("memory".into(), address, size as usize));
        Ok(vec![s.memory_reads; size as usize])
    }
    fn synchronize(&mut self) -> Result<()> {
        Ok(())
    }
}

fn filesystem() -> (CameraFs<TestDevice>, TestDevice) {
    let device = TestDevice::default();
    device
        .0
        .lock()
        .unwrap()
        .files
        .insert("A:\\DCIM\\IMAGE.JPG".into(), (0..128).collect());
    let fs = CameraFs::new(
        device.clone(),
        vec![
            Region {
                name: "first".into(),
                address: 0x1000,
                size: 0x100,
            },
            Region {
                name: "second".into(),
                address: 0x1100,
                size: 0x100,
            },
            Region {
                name: "last".into(),
                address: 0xfffffff0,
                size: 16,
            },
        ],
    )
    .unwrap();
    (fs, device)
}

#[test]
fn namespace_is_flat_memory_is_live_and_every_mutation_is_refused() {
    let (mut fs, device) = filesystem();
    assert_eq!(
        fs.directory(ROOT, false)
            .unwrap()
            .1
            .iter()
            .map(|n| n.name.as_str())
            .collect::<Vec<_>>(),
        ["sd", "memory", "regions.json"]
    );
    assert_eq!(fs.node(MEMORY).unwrap().size, 1 << 32);
    assert_eq!(fs.read(MEMORY, 0x1000, 3).unwrap(), [1, 1, 1]);
    assert_eq!(fs.read(MEMORY, 0x1000, 3).unwrap(), [2, 2, 2]);
    assert_eq!(fs.write(MEMORY, 0x1000, b"bad"), Err(libc::EROFS));
    assert_eq!(fs.truncate(MEMORY, 0).unwrap_err(), libc::EROFS);
    assert_eq!(fs.remove(ROOT, "memory"), Err(libc::EROFS));
    assert_eq!(fs.create(ROOT, "NEW", false).unwrap_err(), libc::EROFS);
    assert_eq!(fs.write(REGIONS, 0, b"bad"), Err(libc::EROFS));
    assert!(
        device
            .0
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(name, _, _)| name == "memory")
    );
}

#[test]
fn holes_mmio_boundaries_and_empty_reads_never_send_invalid_usb_reads() {
    let (mut fs, device) = filesystem();
    for (offset, size) in [(0, 1), (0x0fff, 2), (0x11ff, 2), (0x2000, 1)] {
        assert_eq!(fs.read(MEMORY, offset, size), Err(libc::EIO));
    }
    assert!(device.0.lock().unwrap().calls.is_empty());
    assert_eq!(fs.read(MEMORY, 0x10ff, 2).unwrap().len(), 2);
    assert_eq!(fs.read(MEMORY, 0xffffffff, 8).unwrap().len(), 1);
    assert!(fs.read(MEMORY, 1 << 32, 1).unwrap().is_empty());
    assert!(fs.read(MEMORY, 0, 0).unwrap().is_empty());
    assert_eq!(device.0.lock().unwrap().calls.len(), 2);
}

#[test]
fn small_random_io_preserves_other_bytes_without_fetching_the_file() {
    let (mut fs, device) = filesystem();
    let folder = fs.lookup(SD, "dcim").unwrap();
    let file = fs.lookup(folder.id, "image.jpg").unwrap();
    assert_eq!(fs.read(file.id, 91, 3).unwrap(), [91, 92, 93]);
    assert_eq!(fs.write(file.id, 10, b"xyz"), Ok(3));
    let state = device.0.lock().unwrap();
    let bytes = &state.files["A:\\DCIM\\IMAGE.JPG"];
    assert_eq!(&bytes[10..13], b"xyz");
    assert_eq!(&bytes[..10], &(0..10).collect::<Vec<_>>());
    assert_eq!(bytes.len(), 128);
    assert_eq!(
        state.calls,
        [
            ("read:A:\\DCIM\\IMAGE.JPG".into(), 91, 3),
            ("write:A:\\DCIM\\IMAGE.JPG".into(), 10, 3)
        ]
    );
}

#[test]
fn create_sparse_write_truncate_and_delete_are_committed_to_device() {
    let (mut fs, device) = filesystem();
    let node = fs.create(SD, "new.bin", false).unwrap();
    assert_eq!(fs.write(node.id, IO_BYTES as u64 + 3, b"hi"), Ok(2));
    {
        let state = device.0.lock().unwrap();
        let data = &state.files["A:\\NEW.BIN"];
        assert!(data[..IO_BYTES + 3].iter().all(|b| *b == 0));
        assert_eq!(&data[IO_BYTES + 3..], b"hi");
        assert!(state.calls.iter().all(|(_, _, n)| *n <= IO_BYTES));
    }
    assert_eq!(fs.truncate(node.id, 2).unwrap().size, 2);
    assert_eq!(fs.read(node.id, 0, 8).unwrap(), vec![0, 0]);
    assert_eq!(fs.truncate(node.id, 0).unwrap().size, 0);
    assert!(device.0.lock().unwrap().files["A:\\NEW.BIN"].is_empty());
    fs.remove(SD, "new.bin").unwrap();
    assert!(!device.0.lock().unwrap().files.contains_key("A:\\NEW.BIN"));
}

#[test]
fn listing_continuations_reuse_one_scan_and_mutations_invalidate_it() {
    let (mut fs, device) = filesystem();
    let (generation, _) = fs.directory(SD, false).unwrap();
    for _ in 0..5 {
        assert_eq!(fs.directory(SD, true).unwrap().0, generation);
    }
    assert_eq!(device.0.lock().unwrap().lists, 1);
    fs.create(SD, "NEW.BIN", false).unwrap();
    assert_ne!(fs.directory(SD, false).unwrap().0, generation);
    assert_eq!(device.0.lock().unwrap().lists, 2);
}

#[test]
fn bad_paths_and_failed_writes_leave_metadata_and_other_files_intact() {
    let (mut fs, device) = filesystem();
    for name in [
        "..",
        ".",
        "A/B",
        "B:\\BAD",
        "TOOLONGNAME.BIN",
        "BAD.LONG",
        "A.B.C",
        "",
    ] {
        assert!(fs.create(SD, name, false).is_err(), "{name}");
    }
    let folder = fs.lookup(SD, "DCIM").unwrap();
    let file = fs.lookup(folder.id, "IMAGE.JPG").unwrap();
    device.0.lock().unwrap().fail_write = true;
    assert_eq!(fs.write(file.id, 128, b"x"), Err(libc::EIO));
    assert_eq!(fs.node(file.id).unwrap().size, 128);
    assert_eq!(
        device.0.lock().unwrap().files["A:\\DCIM\\IMAGE.JPG"].len(),
        128
    );
}
