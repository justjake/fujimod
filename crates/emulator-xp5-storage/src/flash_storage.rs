//! Read-only NOR flash-storage aperture and sparse session NOR storage. Evidence and
//! erased-tail behavior is an explicit storage assumption.
use fujimod_emulator_devices::storage_reads::StorageReads;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub const BASE: u64 = 0xf0000000;
pub const SIZE: u64 = 0x08000000;
const PAGE: usize = 4096;
pub type Shared = Arc<Mutex<Storage>>;
#[derive(Default, Serialize, Deserialize)]
pub struct Session {
    pages: BTreeMap<u64, Vec<u8>>,
    pub write_enable: bool,
    pub program_mode: bool,
    #[serde(default)]
    pub controller_complete: bool,
    pub reads: StorageReads,
}
pub struct Storage {
    image: Vec<u8>,
    pub session: Session,
}
impl Storage {
    pub fn new(image: Vec<u8>) -> Result<Self, String> {
        if image.len() as u64 > SIZE {
            return Err("update payload exceeds flash storage aperture".into());
        }
        Ok(Self {
            image,
            session: Session::default(),
        })
    }
    fn extent(offset: u64, n: usize) -> Result<(), String> {
        if offset.checked_add(n as u64).is_none_or(|e| e > SIZE) {
            return Err("flash storage access outside aperture".into());
        }
        Ok(())
    }
    pub fn read(&self, offset: u64, n: usize) -> Result<Vec<u8>, String> {
        Self::extent(offset, n)?;
        let mut bytes = vec![0xff; n];
        let start = offset as usize;
        if start < self.image.len() {
            let count = n.min(self.image.len() - start);
            bytes[..count].copy_from_slice(&self.image[start..start + count]);
        }
        if n != 0 {
            let end = offset + n as u64;
            for (&index, page) in self
                .session
                .pages
                .range(offset / PAGE as u64..=(end - 1) / PAGE as u64)
            {
                let page_start = index * PAGE as u64;
                let lo = offset.max(page_start);
                let hi = end.min(page_start + PAGE as u64);
                bytes[(lo - offset) as usize..(hi - offset) as usize]
                    .copy_from_slice(&page[(lo - page_start) as usize..(hi - page_start) as usize]);
            }
        }
        Ok(bytes)
    }
    /// A controller or DMA transfer; observational host reads use `read`.
    pub fn read_device(&mut self, offset: u64, n: usize) -> Result<Vec<u8>, String> {
        let bytes = self.read(offset, n)?;
        self.session.reads.record(offset, n as u64, SIZE)?;
        Ok(bytes)
    }
    fn page(&mut self, index: u64) -> &mut Vec<u8> {
        self.session.pages.entry(index).or_insert_with(|| {
            let start = index as usize * PAGE;
            (start..start + PAGE)
                .map(|a| self.image.get(a).copied().unwrap_or(0xff))
                .collect()
        })
    }
    /// Explicit declarative data seeds, never an implicit firmware change.
    pub fn seed(&mut self, offset: u64, value: u8) -> Result<(), String> {
        Self::extent(offset, 1)?;
        self.page(offset / PAGE as u64)[offset as usize % PAGE] = value;
        Ok(())
    }
    pub fn program(&mut self, offset: u64, bytes: &[u8]) -> Result<(), String> {
        Self::extent(offset, bytes.len())?;
        if bytes.is_empty()
            || bytes.len() > 256
            || offset / 256 != (offset + bytes.len() as u64 - 1) / 256
        {
            return Err("unsupported flash storage program extent/page crossing".into());
        }
        if !self.session.write_enable {
            return Err("flash storage program without WREN".into());
        }
        for (a, &b) in (offset..).zip(bytes) {
            self.page(a / PAGE as u64)[a as usize % PAGE] &= b;
        }
        self.session.write_enable = false;
        Ok(())
    }
    pub fn erase(&mut self, offset: u64, size: usize) -> Result<(), String> {
        Self::extent(offset, size)?;
        if size == 0
            || !size.is_multiple_of(PAGE)
            || !self.session.write_enable
            || !offset.is_multiple_of(size as u64)
        {
            return Err("flash storage erase without WREN or unaligned sector".into());
        }
        for a in offset..offset + size as u64 {
            self.page(a / PAGE as u64)[a as usize % PAGE] = 0xff;
        }
        self.session.write_enable = false;
        Ok(())
    }
}
pub struct Aperture(pub Shared);
impl Device for Aperture {
    fn name(&self) -> &'static str {
        "firmware-flash-storage"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(BASE);
        match self.0.lock().unwrap().read_device(o, n as usize) {
            Ok(b) => b
                .iter()
                .enumerate()
                .fold(0, |v, (i, b)| v | (*b as u64) << (i * 8)),
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.fail("CPU write to read-only flash storage aperture");
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.0.lock().unwrap().session).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let session: Session = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid flash storage checkpoint: {e}"))?;
        session.reads.validate(SIZE)?;
        self.0.lock().unwrap().session = session;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_reads_are_checkpointed_but_host_inspection_is_observational() {
        use fujimod_emulator_devices::test_host::recording::RecordingHost;
        let storage = Arc::new(Mutex::new(Storage::new(vec![0xa5; 16]).unwrap()));
        let mut aperture = Aperture(storage.clone());
        let mut host = RecordingHost::default();
        assert_eq!(
            aperture.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 4, 4),
            0xa5a5a5a5
        );
        assert_eq!(storage.lock().unwrap().session.reads.ranges, [(4, 8)]);
        let saved = aperture.save();
        for _ in 0..2 {
            storage.lock().unwrap().read(0, 16).unwrap();
            aperture.save();
        }
        assert_eq!(aperture.save(), saved);
        // The erased tail is still an internal-medium dependency.
        storage.lock().unwrap().read_device(SIZE - 1, 1).unwrap();
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 5);
        aperture.restore(&saved).unwrap();
        assert_eq!(aperture.save(), saved);
        let mut invalid = saved;
        invalid["reads"]["ranges"] = serde_json::json!([[SIZE, SIZE + 1]]);
        assert!(aperture.restore(&invalid).is_err());
        assert!(storage.lock().unwrap().read_device(SIZE - 1, 2).is_err());
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 4);
    }
    #[test]
    fn nor_overlay_crosses_pages_and_restores_without_mutating_image() {
        let mut s = Storage::new(vec![0xa5; PAGE + 2]).unwrap();
        assert!(s.program(PAGE as u64 - 1, &[0, 0]).is_err());
        s.session.write_enable = true;
        s.program(PAGE as u64 - 1, &[0x0f]).unwrap();
        s.session.write_enable = true;
        s.program(PAGE as u64, &[0xff, 0x50]).unwrap();
        assert_eq!(s.read(PAGE as u64 - 1, 3).unwrap(), [5, 0xa5, 0]);
        let saved = serde_json::to_value(&s.session).unwrap();
        s.session.write_enable = true;
        s.erase(0, PAGE).unwrap();
        assert_eq!(s.read(PAGE as u64 - 1, 2).unwrap(), [0xff, 0xa5]);
        s.session = serde_json::from_value(saved).unwrap();
        assert_eq!(s.read(PAGE as u64 - 1, 3).unwrap(), [5, 0xa5, 0]);
        assert_eq!(s.image[PAGE - 1], 0xa5);
        assert_eq!(s.read(SIZE - 1, 1).unwrap(), [0xff]);
        assert!(s.read(SIZE - 1, 2).is_err());
    }
}
