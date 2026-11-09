//! Serial-NAND flash-storage medium: the die-backed linear aperture view. The
//! mapped window exposes main-area bytes only; erased space reads 0xff. Guest
//! reads are recorded exactly like the NOR medium; page writes wait for the
//! controller protocol and refuse until then.
use crate::flash_storage::SIZE;
use fujimod_emulator_devices::parts::nand::{Nand, Options};
use fujimod_emulator_devices::storage_reads::StorageReads;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub const PAGE: usize = 4096;
pub const PAGES_PER_BLOCK: u32 = 64;
pub const SPARE: usize = 128;
pub type NandShared = Arc<Mutex<NandStorage>>;

#[derive(Default, Serialize, Deserialize)]
pub struct NandSession {
    pub reads: StorageReads,
    pub write_enable: bool,
}

pub struct NandStorage {
    die: Nand,
    pub session: NandSession,
}

impl NandStorage {
    /// The die base image is the update payload as the vendor stores it on the
    /// part (bit-inverted relative to the DAT file); the caller supplies those
    /// bytes already inverted, exactly like the NOR storage image.
    pub fn new(base: Vec<u8>) -> Result<Self, String> {
        let die = Nand::new(
            Options {
                bytes: SIZE,
                page_bytes: PAGE,
                pages_per_block: PAGES_PER_BLOCK,
                spare_bytes: SPARE,
            },
            Arc::from(base.into_boxed_slice()),
            Vec::new(),
            std::collections::BTreeMap::new(),
        )?;
        Ok(Self {
            die,
            session: NandSession::default(),
        })
    }

    pub fn read(&self, offset: u64, n: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0xff; n];
        self.die.read(offset, &mut bytes)?;
        Ok(bytes)
    }

    /// A controller or DMA transfer; observational host reads use `read`.
    pub fn read_device(&mut self, offset: u64, n: usize) -> Result<Vec<u8>, String> {
        let bytes = self.read(offset, n)?;
        self.session.reads.record(offset, n as u64, SIZE)?;
        Ok(bytes)
    }

    pub fn program_page(&mut self, row: u32, bytes: &[u8], spare: &[u8]) -> Result<(), String> {
        if !self.session.write_enable {
            return Err("serial NAND program without WEL".into());
        }
        self.die.program(row, bytes, spare)?;
        self.session.write_enable = false;
        Ok(())
    }

    pub fn erase_block(&mut self, first_row: u32) -> Result<(), String> {
        if !self.session.write_enable {
            return Err("serial NAND erase without WEL".into());
        }
        self.die.erase(first_row)?;
        self.session.write_enable = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash_storage::{Aperture, BASE, Medium};
    use fujimod_emulator_devices::{Cx, Device, test_host::recording::RecordingHost};

    fn storage(base: Vec<u8>) -> NandShared {
        Arc::new(Mutex::new(NandStorage::new(base).unwrap()))
    }

    #[test]
    fn aperture_reads_base_image_and_erased_tail() {
        let storage = storage(vec![0xa5; PAGE + 2]);
        let mut aperture = Aperture(Medium::Nand(storage.clone()));
        let mut host = RecordingHost::default();
        assert_eq!(
            aperture.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 4, 4),
            0xa5a5a5a5
        );
        assert_eq!(
            storage.lock().unwrap().read(SIZE - 1, 1).unwrap(),
            [0xff],
            "erased tail beyond the image must read 0xff"
        );
        assert!(storage.lock().unwrap().read(SIZE, 1).is_err());
        assert_eq!(storage.lock().unwrap().session.reads.ranges, [(4, 8)]);
    }

    #[test]
    fn guest_reads_are_checkpointed_and_restore_is_observational() {
        let storage = storage(vec![0xa5; 16]);
        let mut aperture = Aperture(Medium::Nand(storage.clone()));
        let saved = aperture.save();
        assert_eq!(saved["medium"], "nand");
        for _ in 0..2 {
            storage.lock().unwrap().read(0, 16).unwrap();
            aperture.save();
        }
        assert_eq!(aperture.save(), saved);
        storage.lock().unwrap().read_device(SIZE - 1, 1).unwrap();
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 1);
        aperture.restore(&saved).unwrap();
        assert_eq!(aperture.save(), saved);
        let mut invalid = saved;
        invalid["session"]["reads"]["ranges"] = serde_json::json!([[SIZE, SIZE + 1]]);
        assert!(aperture.restore(&invalid).is_err());
    }

    #[test]
    fn page_writes_need_wel_and_geometry_matches_the_die() {
        let mut s = NandStorage::new(vec![0xff; PAGE]).unwrap();
        assert!(s.program_page(0, &[0; PAGE], &[0; SPARE]).is_err());
        s.session.write_enable = true;
        s.program_page(0, &[0x11; PAGE], &[0; SPARE]).unwrap();
        assert_eq!(s.read(0, 4).unwrap(), [0x11; 4]);
        assert!(!s.session.write_enable, "program clears WEL");
        s.session.write_enable = true;
        s.erase_block(0).unwrap();
        assert_eq!(s.read(0, 4).unwrap(), [0xff; 4]);
        assert!(NandStorage::new(vec![0; (SIZE + 1) as usize]).is_err());
    }
}
