//! A caller card in SD slot 1 for the loader start (`--card-image`, `--card-file`).
//!
//! The default loader start has both SD slots empty (`present = false` in the boot manifest) and
//! polls the hosts, so the shooting screen reports a card error. A run that names a card image
//! or card files inserts one card in slot 1, the UHS-I host at `0xff9b0000`, and wires that
//! host's interrupt output (the stock OS driver drives a card it finds through interrupts). The
//! line is the one the ROM start already wires: `[[window_options]]` for `0xff9b0000` in
//! `catalog/soc/xp5/rom-start.toml` (INTID 262). Both changes are made to the boot state, so they
//! are part of the session definition (`boot_wiring_sha256`): a snapshot taken without a card is
//! refused by a card run and the other way round. A run with neither option keeps the manifest
//! unchanged.
//!
//! The card contents are not part of the definition. They live in the SD host's checkpoint, so
//! `--load-snapshot` replaces the image's contents with the card saved in the snapshot.
//!
//! `--card-image` is loaded as the ROM start loads it (sparse, read-only while loading, and
//! refused when its root holds an update or loader image). At the end of the run every sector
//! that differs from the loaded image is written back into the same file, which keeps its
//! length; nothing else is written and the file is never created or truncated. `--card-file`
//! alone puts the files on the manifest's in-memory volume, which is not written anywhere.

use super::{card_image::CardImage, manifest::BootState, rom_start};
use fujimod_emulator_devices::parts::sd_card::Card;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Slot 1: the UHS-I host of SD instance 0 (manifest.d/25-sd.toml).
pub(crate) const SLOT: u64 = 0xff9b_0000;
/// The sparse write bound of an in-memory card holding only caller files: 1 GiB, the model's
/// limit, so the camera can save pictures on it.
const FILE_CARD_DIRTY_SECTORS: usize = 1 << 21;

pub(crate) struct LoaderCard {
    /// The image, with any caller files merged in. It replaces the slot's formatted volume.
    pub card: Option<Card>,
    /// Caller files for the formatted volume when no image is given, by name.
    pub assets: BTreeMap<String, Vec<u8>>,
    pub write_back: Option<WriteBack>,
}

/// Where the image came from and what it held, to write back only the changed sectors.
pub(crate) struct WriteBack {
    pub path: PathBuf,
    pub original: Card,
}

impl LoaderCard {
    /// `None` when the run names no card: the slots stay as the manifest declares them.
    pub fn load(image: Option<&Path>, files: &[(String, PathBuf)]) -> Result<Option<Self>, String> {
        if image.is_none() && files.is_empty() {
            return Ok(None);
        }
        let files = crate::media::read_files(files)?;
        let Some(path) = image else {
            return Ok(Some(Self { card: None, assets: files.into_iter().collect(), write_back: None }));
        };
        let loaded = CardImage::load(path)?;
        for line in &loaded.layout {
            eprintln!("FUJIMOD card image {}: {line}", path.display());
        }
        eprintln!("FUJIMOD card image root names: {:?}", loaded.names);
        let original = loaded.card;
        let card = fujimod_emulator_session::card::merge_files(original.clone(), &files)?;
        Ok(Some(Self {
            card: Some(card),
            assets: BTreeMap::new(),
            write_back: Some(WriteBack { path: path.to_owned(), original }),
        }))
    }

    /// Insert the card in slot 1 and connect the slot's interrupt line.
    pub fn apply(&self, state: &mut BootState) -> Result<(), String> {
        let slot = state
            .sd
            .iter_mut()
            .find(|slot| slot.base == SLOT)
            .ok_or("the boot manifest declares no SD slot at 0xff9b0000")?;
        slot.present = true;
        slot.protected = false;
        if self.card.is_none() {
            slot.files = self.assets.keys().map(|name| (name.clone(), name.clone())).collect();
            slot.format.max_dirty_sectors = slot.format.max_dirty_sectors.max(FILE_CARD_DIRTY_SECTORS);
        }
        let host = state
            .device
            .iter_mut()
            .find(|window| window.base == SLOT && window.kind == "sdhci")
            .ok_or("the boot manifest declares no SD host at 0xff9b0000")?;
        let fragment = rom_start::Fragment::bundled()?;
        let options = fragment
            .window_options
            .into_iter()
            .find(|options| options.base == SLOT)
            .ok_or("rom-start.toml declares no interrupt line for the SD host at 0xff9b0000")?;
        for (key, value) in options.set {
            // A manifest that already wires the host keeps its own line.
            host.options.entry(key).or_insert(value);
        }
        Ok(())
    }
}

impl WriteBack {
    /// Write the sectors of slot 1 that differ from the loaded image back into the image file.
    pub fn write(&self, session: &fujimod_emulator_session::Session) -> Result<(), String> {
        let current = session.card_medium(&format!("{SLOT:#x}"))?;
        let written = crate::media::write_back(&self.path, &self.original, &current)?;
        eprintln!("FUJIMOD card image {}: {written} changed sectors written back", self.path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> BootState {
        toml::from_str(
            r#"
firmware = "xt5-431"
ram = []
[[device]]
base = 0xff9b0000
size = 0x1e0
kind = "sdhci"
[device.options.controller]
base = 0xff9b0000
protocol = "sdhc"
transfer_ticks = 1000
clock_ticks = 1000
rca = 1
ram_bytes = 0x80000000
[[sd]]
base = 0xff9b0000
present = false
protected = false
files = []
[sd.format]
sectors = 0x3b9e000
max_dirty_sectors = 65536
partition_start = 8192
sectors_per_cluster = 64
label = "NO NAME"
"#,
        )
        .unwrap()
    }

    #[test]
    fn no_card_leaves_the_manifest_unchanged() {
        assert!(LoaderCard::load(None, &[]).unwrap().is_none());
    }

    #[test]
    fn card_files_insert_a_present_card_and_wire_interrupt_262() {
        let directory = std::env::temp_dir().join(format!("loader-card-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("note.txt");
        std::fs::write(&file, b"hello").unwrap();
        let card = LoaderCard::load(None, &[("NOTE.TXT".into(), file)]).unwrap().unwrap();
        assert!(card.write_back.is_none());
        let mut state = state();
        let before = serde_json::to_vec(&state).unwrap();
        card.apply(&mut state).unwrap();
        assert_ne!(serde_json::to_vec(&state).unwrap(), before, "the wiring hash must change");
        let slot = &state.sd[0];
        assert!(slot.present);
        assert_eq!(slot.files, [("NOTE.TXT".to_string(), "NOTE.TXT".to_string())]);
        assert_eq!(slot.format.max_dirty_sectors, FILE_CARD_DIRTY_SECTORS);
        assert_eq!(state.device[0].options["irq"].as_integer(), Some(262));
        // An irq already in the manifest wins.
        let mut wired = self::state();
        wired.device[0].options.insert("irq".into(), toml::Value::Integer(7));
        card.apply(&mut wired).unwrap();
        assert_eq!(wired.device[0].options["irq"].as_integer(), Some(7));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_manifest_without_slot_one_is_refused() {
        let card = LoaderCard { card: None, assets: BTreeMap::new(), write_back: None };
        let mut state = state();
        state.sd.clear();
        assert!(card.apply(&mut state).unwrap_err().contains("no SD slot"));
    }
}
