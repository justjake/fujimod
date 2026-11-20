//! Reset through the captured X100VI boot ROM (emulator only).
//!
//! The default boot starts at the loader hand-off (EL1t `0x9920c`). This start resets the four
//! A53 cores into a captured 64 KiB mask ROM at `0xfff00000`, as the ROM sees them
//! (EL3, MMU off, MPIDR affinities 0..3), with an emulated SD card holding only the files the
//! caller names. Nothing here builds or writes an update file, a real card or a card image on
//! disk: the FAT volume is formatted in memory by FujiMod's SD model.

use super::manifest::RamRegion;
use fujimod_emulator_devices::graph::DeviceWindow;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) const ROM_BASE: u64 = 0xfff0_0000;
pub(crate) const ROM_SIZE: usize = 0x1_0000;
pub(crate) const ROM_SHA256: &str =
    "41d68e8cf37b5fb6ba14238fe678eb106cf7c471075ec8e71e8d838afab859eb";

/// The stage file the ROM opens on the card (`rom_load_sd_file`, `0xfff07298`).
pub(crate) const CARD_FILE_NAME: &str = "marble_init.bin";
/// The RAM image the post-DDR loader reads from the card (`loader.c`, exact short name
/// `FFBOOT  BIN`). With `marble_init.bin` it makes the startup card.
pub(crate) const STARTUP_FILE_NAME: &str = "ffboot.bin";
/// The card-name allowlist: exactly these two names, any case, in the root.
const CARD_FILE_NAMES: [&str; 2] = [CARD_FILE_NAME, STARTUP_FILE_NAME];
/// The merged initial-image bound of the session (512 MiB); a card file is read whole into memory.
const MAX_CARD_FILE_BYTES: u64 = 512 << 20;

/// AArch64 reset state of a core in the ROM: EL3h, D/A/I/F masked.
pub(crate) const ROM_PSTATE: u64 = 0x3cd;

/// `0xff90e000` bits 2:1 both zero select the SD file path.
/// Bit 0 then picks direct (1) or page-cache (0) serial flash. How the body's dial chord maps to
/// these straps is not measured; the chord is modeled as the SD selection.
pub(crate) const STRAPS_SD: u32 = 0;
pub(crate) const STRAPS_DIRECT_FLASH: u32 = 1;

/// What boot/rom-start.toml adds to the default boot manifest.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fragment {
    #[serde(default)]
    pub ram: Vec<RamRegion>,
    #[serde(default)]
    pub device: Vec<DeviceWindow>,
    /// Options this start sets on a window the default manifest already declares.
    #[serde(default)]
    pub window_options: Vec<WindowOptions>,
    /// Register address (hex digits) -> value of the power-controller part at reset.
    #[serde(default)]
    pub power_controller_registers: BTreeMap<String, u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WindowOptions {
    pub base: u64,
    pub set: toml::Table,
}

impl Fragment {
    pub fn bundled() -> Result<Self, String> {
        toml::from_str(include_str!("../../../../catalog/soc/xp5/rom-start.toml"))
            .map_err(|error| format!("data/rom-start.toml: {error}"))
    }
}

/// Initial instruction source for an early boot.
pub(crate) enum Entry {
    MaskRom(Vec<u8>),
    CardHandoff(Handoff),
    /// The boot-setup stage, entered from internal flash in place of the boot ROM.
    SetupHandoff(SetupHandoff),
}

/// The boot-setup stage as the boot ROM would leave it: the image copied from internal flash to
/// boot SRAM, the entry the ROM branches to, and the boot-source straps it samples.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetupHandoff {
    pub pc: u64,
    pub sp: u64,
    /// Offset of the stage in the update image's XOR-DAT payload.
    pub dat_offset: u64,
    pub length: u64,
    pub sha256: String,
    pub source_straps: u32,
}

impl SetupHandoff {
    pub fn for_firmware(firmware: &str) -> Option<Result<Self, String>> {
        match firmware {
            "xm5-130" => Some(toml::from_str(include_str!(
                "../../../../catalog/firmware/xm5/1.30/xp5-boot/setup-handoff.toml"
            )).map_err(|error| format!("setup-handoff.toml: {error}"))),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Handoff {
    pub pc: u64,
    pub sp: u64,
    pub slot: u64,
    pub stage_max_bytes: usize,
    pub power_controller_registers: BTreeMap<String, u16>,
    /// Device windows this firmware's SD card stage needs beyond the shared ROM-start fragment.
    /// A bring-up stub listed here answers before the fragment's stub of the same page.
    #[serde(default)]
    pub device: Vec<DeviceWindow>,
}

impl Handoff {
    pub fn for_firmware(firmware: &str) -> Option<Result<Self, String>> {
        match firmware {
            "xt5-431" => Some(toml::from_str(include_str!(
                "../../../../catalog/firmware/xt5/4.31/xp5-boot/card-handoff.toml"
            )).map_err(|error| format!("card-handoff.toml: {error}"))),
            _ => None,
        }
    }
}

pub(crate) struct RomStart {
    pub entry: Entry,
    pub source_straps: u32,
    /// `(file name on the card, bytes)`; the card is formatted in memory.
    pub card_files: Vec<(String, Vec<u8>)>,
    pub card_present: bool,
    /// A raw SD card image in place of the in-memory FAT32 volume.
    pub card_image: Option<super::card_image::CardImage>,
}

impl RomStart {
    /// Read and verify the boot ROM at `rom_path` (plain or zstd-compressed), then build the start.
    pub fn load(
        rom_path: &Path,
        dials_held: bool,
        card_files: Vec<(String, PathBuf)>,
        card_present: bool,
    ) -> Result<Self, String> {
        let packed = std::fs::read(rom_path).map_err(|e| format!("{}: {e}", rom_path.display()))?;
        let rom = if rom_path.extension().is_some_and(|ext| ext == "zst") {
            zstd::stream::decode_all(packed.as_slice())
                .map_err(|e| format!("{}: {e}", rom_path.display()))?
        } else {
            packed
        };
        if rom.len() != ROM_SIZE {
            return Err(format!("boot ROM is {} bytes, expected {ROM_SIZE}", rom.len()));
        }
        let digest: String = Sha256::digest(&rom).iter().map(|b| format!("{b:02x}")).collect();
        if digest != ROM_SHA256 {
            return Err(format!("boot ROM SHA-256 {digest} is not the captured ROM"));
        }
        Self::with_media(Entry::MaskRom(rom), dials_held, card_files, card_present)
    }

    pub fn from_setup_handoff(handoff: SetupHandoff) -> Result<Self, String> {
        let straps = handoff.source_straps;
        let mut start = Self::with_media(Entry::SetupHandoff(handoff), false, Vec::new(), false)?;
        start.source_straps = straps;
        Ok(start)
    }

    pub fn from_card_handoff(handoff: Handoff, files: Vec<(String, PathBuf)>) -> Result<Self, String> {
        Self::with_media(Entry::CardHandoff(handoff), true, files, true)
    }

    fn with_media(
        entry: Entry,
        dials_held: bool,
        card_files: Vec<(String, PathBuf)>,
        card_present: bool,
    ) -> Result<Self, String> {
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        for (name, source) in card_files {
            // The card carries the boot files only: exactly `marble_init.bin` (the ROM's stage
            // file) and `ffboot.bin` (the loader's RAM image), any case, in the root. Every other
            // name, with or without a directory, is refused, so this start cannot carry the
            // update (`FPUPDATE.DAT`, `*.DAT`) and cannot be used to prepare an installable card.
            if !CARD_FILE_NAMES.iter().any(|allowed| name.eq_ignore_ascii_case(allowed)) {
                return Err(format!(
                    "card file {name:?} refused: this emulator start carries startup-card files only \
                     (marble_init.bin, ffboot.bin)"
                ));
            }
            if files.iter().any(|(existing, _)| existing.eq_ignore_ascii_case(&name)) {
                return Err(format!("card file {name:?} given twice"));
            }
            let size = std::fs::metadata(&source)
                .map_err(|e| format!("{}: {e}", source.display()))?
                .len();
            if size > MAX_CARD_FILE_BYTES {
                return Err(format!(
                    "card file {} is {size} bytes, over the {MAX_CARD_FILE_BYTES}-byte bound",
                    source.display()
                ));
            }
            let bytes = std::fs::read(&source).map_err(|e| format!("{}: {e}", source.display()))?;
            files.push((name, bytes));
        }
        Ok(Self {
            entry,
            source_straps: if dials_held { STRAPS_SD } else { STRAPS_DIRECT_FLASH },
            card_files: files,
            card_present,
            card_image: None,
        })
    }

    /// Use a raw SD card image instead of the formatted volume.
    pub fn with_card_image(mut self, path: &Path) -> Result<Self, String> {
        if !self.card_files.is_empty() {
            return Err("--sd-image and --sd-file are alternatives: the image is the whole card".into());
        }
        self.card_image = Some(super::card_image::CardImage::load(path)?);
        Ok(self)
    }

    pub fn assets(&self) -> BTreeMap<String, Vec<u8>> {
        self.card_files.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(dials_held: bool, files: Vec<(String, PathBuf)>) -> Result<RomStart, String> {
        RomStart::with_media(Entry::MaskRom(vec![0; ROM_SIZE]), dials_held, files, true)
    }

    #[test]
    fn x_m5_setup_handoff_declares_the_boot_setup_extent() {
        let handoff = SetupHandoff::for_firmware("xm5-130").unwrap().unwrap();
        assert_eq!((handoff.pc, handoff.dat_offset, handoff.length), (0xff80_0000, 0xabc, 0x2c000));
        assert_eq!(handoff.sha256.len(), 64);
        assert_eq!(handoff.source_straps, 1);
        assert!(SetupHandoff::for_firmware("xt5-431").is_none());
        let start = RomStart::from_setup_handoff(handoff).unwrap();
        assert_eq!(start.source_straps, 1);
        assert!(!start.card_present && start.card_files.is_empty());
    }

    #[test]
    fn the_xt5_card_handoff_lists_its_own_setup_windows() {
        let handoff = Handoff::for_firmware("xt5-431").unwrap().unwrap();
        assert_eq!((handoff.pc, handoff.sp, handoff.slot), (0xff80_0000, 0xff83_c000, 0xff9b_0000));
        // The memory-controller status the X-T5 setup polls without writing it first.
        let status = handoff.device.iter().find(|window| window.base == 0xfaf0_7000).unwrap();
        assert_eq!(status.kind, "bringup-stub");
        let registers = status.options["registers"].as_array().unwrap();
        assert!(registers.iter().any(|register| {
            register["offset"].as_integer() == Some(0x780)
                && register["mode"].as_str() == Some("constant")
                && register["value"].as_integer() == Some(1)
        }));
        // No window is declared twice, and none overlaps another.
        let mut extents: Vec<(u64, u64)> =
            handoff.device.iter().map(|window| (window.base, window.base + window.size)).collect();
        extents.sort_unstable();
        assert!(extents.windows(2).all(|pair| pair[0].1 <= pair[1].0));
        assert!(Handoff::for_firmware("x100vi-131").is_none());
    }

    #[test]
    fn held_dials_pick_the_sd_straps() {
        let start = start(true, vec![]).unwrap();
        assert!(matches!(start.entry, Entry::MaskRom(ref bytes) if bytes.len() == ROM_SIZE));
        assert_eq!(start.source_straps, STRAPS_SD);
    }

    #[test]
    fn released_dials_pick_flash_straps() {
        assert_eq!(start(false, vec![]).unwrap().source_straps, STRAPS_DIRECT_FLASH);
    }

    #[test]
    fn install_shaped_card_files_are_refused() {
        for name in [
            "FPUPDATE.DAT",
            "fpupdate.dat",
            "FPUPDATE.BIN",
            "x.DAT",
            "ffboot.dat",
            "/FFBOOT.BIN",
            "/marble_init.bin",
            "a\\FPUPDATE.BIN",
            "dir/marble_init.bin",
            "dir/ffboot.bin",
            "dir\\FFBOOT.BIN",
            "marble_init.bin.bak",
            "marble_init.binx",
            "ffboot.bin ",
            "ffboot.binx",
            "ffboot",
            "FFBOOT.BIN/",
            "..",
            "",
        ] {
            let error = start(true, vec![(name.to_string(), PathBuf::from("/nonexistent"))])
                .err()
                .expect(name);
            assert!(error.contains("startup-card files only"), "{name}: {error}");
        }
    }

    #[test]
    fn exactly_the_two_boot_files_are_accepted_in_any_case_once_each() {
        let directory = std::env::temp_dir().join(format!("rom-start-names-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("file.bin");
        std::fs::write(&file, b"abcd").unwrap();
        let both = |first: &str, second: &str| {
            start(true, vec![(first.to_string(), file.clone()), (second.to_string(), file.clone())])
        };
        let start = both("marble_init.bin", "ffboot.bin").unwrap();
        assert_eq!(start.card_files.len(), 2);
        assert!(start.assets().contains_key("ffboot.bin") && start.assets().contains_key("marble_init.bin"));
        both("MARBLE_INIT.BIN", "FFBOOT.BIN").unwrap();
        for (first, second) in [("marble_init.bin", "MARBLE_INIT.BIN"), ("ffboot.bin", "FFBoot.Bin")] {
            let error = both(first, second).err().unwrap();
            assert!(error.contains("given twice"), "{error}");
        }
        // The update never rides along, even next to valid boot files.
        let error = both("marble_init.bin", "FPUPDATE.DAT").err().unwrap();
        assert!(error.contains("startup-card files only"), "{error}");
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_wrong_rom_is_refused() {
        let directory = std::env::temp_dir().join(format!("rom-start-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rom.bin");
        std::fs::write(&path, vec![0u8; ROM_SIZE]).unwrap();
        let error = RomStart::load(&path, true, vec![], true).err().unwrap();
        assert!(error.contains("not the captured ROM"), "{error}");
        std::fs::remove_dir_all(&directory).ok();
    }
}
