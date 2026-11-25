//! Caller cards for the loader start: SD slot 1 (`--card-image`, `--card-file`) and SD slot 2
//! (`--card-image-2`, `--card-file-2`).
//!
//! The default loader start has both SD slots empty (`present = false` in the boot manifest) and
//! polls the hosts, so the shooting screen reports a card error. A run that names a card image
//! or card files for a slot inserts one card there and wires that slot's host interrupt output
//! (the stock OS driver drives a card it finds through interrupts). Slot 1 is the UHS-I host at
//! `0xff9b0000`; its line is the one the ROM start already wires: `[[window_options]]` for
//! `0xff9b0000` in `catalog/soc/xp5/rom-start.toml` (INTID 262). Slot 2 is the UHS-I host at
//! `0xff9b4000`, INTID 269 (`SLOT_2_IRQ`). The changes are made to the boot state, so they
//! are part of the session definition (`boot_wiring_sha256`): a snapshot taken without a card is
//! refused by a card run and the other way round, and the same holds for the second card. A run
//! with none of the options keeps the manifest unchanged, and a run without a slot-2 option keeps
//! the slot-1 wiring it had before slot 2 could hold a card.
//!
//! The card contents are not part of the definition. They live in the SD host's checkpoint, so
//! `--load-snapshot` replaces the image's contents with the card saved in the snapshot.
//!
//! A card image is loaded as the ROM start loads it (sparse, read-only while loading, and
//! refused when its root holds an update or loader image). At the end of the run every sector
//! that differs from the loaded image is written back into the same file, which keeps its
//! length; nothing else is written and the file is never created or truncated. Card files
//! alone go on the manifest's in-memory volume of that slot, which is not written anywhere.

use super::{card_image::CardImage, manifest::BootState, rom_start};
use fujimod_emulator_devices::parts::sd_card::Card;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Slot 1: the UHS-I host of SD instance 0.
pub(crate) const SLOT: u64 = 0xff9b_0000;
/// Slot 2: the UHS-I host of SD instance 1.
pub(crate) const SLOT_2: u64 = 0xff9b_4000;
/// The interrupt line of the slot-2 UHS-I host. The X-T5 4.31 SD driver registers its handlers
/// by interrupt source (0x13a41ec: instance 0 takes source 0x84 for port 0 and 0x85 for the
/// UHS-II port, instance 1 takes 0x86 and 0x87), and the source table at 0x18ea918 (16-byte
/// rows, INTID first) holds 262, 267, 269 and 274 for them. 262 is the slot-1 line already in
/// use, so the same table gives slot 2 its line.
pub(crate) const SLOT_2_IRQ: i64 = 269;
/// The sparse write bound of an in-memory card holding only caller files: 1 GiB, the model's
/// limit, so the camera can save pictures on it.
const FILE_CARD_DIRTY_SECTORS: usize = 1 << 21;

pub(crate) struct LoaderCard {
    /// The slot's UHS-I host base: `SLOT` or `SLOT_2`.
    pub slot: u64,
    /// The image, with any caller files merged in. It replaces the slot's formatted volume.
    pub card: Option<Card>,
    /// Caller files for the formatted volume when no image is given, by name.
    pub assets: BTreeMap<String, Vec<u8>>,
    pub write_back: Option<WriteBack>,
}

/// Where the image came from and what it held, to write back only the changed sectors.
pub(crate) struct WriteBack {
    pub slot: u64,
    pub path: PathBuf,
    pub original: Card,
}

impl LoaderCard {
    /// `None` when the run names no card for `slot`: it stays as the manifest declares it.
    pub fn load(slot: u64, image: Option<&Path>, files: &[(String, PathBuf)]) -> Result<Option<Self>, String> {
        if image.is_none() && files.is_empty() {
            return Ok(None);
        }
        let files = crate::media::read_files(files)?;
        let Some(path) = image else {
            // Slot 1 keeps the bare names it always had; the other slot's assets carry its base
            // so that both cards can hold a file of the same name.
            let assets = files.into_iter()
                .map(|(name, bytes)| (if slot == SLOT { name } else { format!("{slot:#x}/{name}") }, bytes))
                .collect();
            return Ok(Some(Self { slot, card: None, assets, write_back: None }));
        };
        let loaded = CardImage::load(path)?;
        for line in &loaded.layout {
            eprintln!("FUJIMOD card image {}: {line}", path.display());
        }
        eprintln!("FUJIMOD card image root names: {:?}", loaded.names);
        let original = loaded.card;
        let card = fujimod_emulator_session::card::merge_files(original.clone(), &files)?;
        Ok(Some(Self {
            slot,
            card: Some(card),
            assets: BTreeMap::new(),
            write_back: Some(WriteBack { slot, path: path.to_owned(), original }),
        }))
    }

    /// Insert the card in its slot and connect the slot's interrupt line.
    pub fn apply(&self, state: &mut BootState) -> Result<(), String> {
        let base = self.slot;
        let slot = state
            .sd
            .iter_mut()
            .find(|slot| slot.base == base)
            .ok_or_else(|| format!("the boot manifest declares no SD slot at {base:#x}"))?;
        slot.present = true;
        slot.protected = false;
        if self.card.is_none() {
            let prefix = if base == SLOT { String::new() } else { format!("{base:#x}/") };
            slot.files = self.assets.keys()
                .map(|id| (id.strip_prefix(&prefix).unwrap_or(id).to_owned(), id.clone()))
                .collect();
            slot.format.max_dirty_sectors = slot.format.max_dirty_sectors.max(FILE_CARD_DIRTY_SECTORS);
        }
        let host = state
            .device
            .iter_mut()
            .find(|window| window.base == base && window.kind == "sdhci")
            .ok_or_else(|| format!("the boot manifest declares no SD host at {base:#x}"))?;
        let set = if base == SLOT {
            rom_start::Fragment::bundled()?
                .window_options
                .into_iter()
                .find(|options| options.base == SLOT)
                .ok_or("rom-start.toml declares no interrupt line for the SD host at 0xff9b0000")?
                .set
        } else {
            [("irq".to_owned(), toml::Value::Integer(SLOT_2_IRQ))].into_iter().collect()
        };
        for (key, value) in set {
            // A manifest that already wires the host keeps its own line.
            host.options.entry(key).or_insert(value);
        }
        Ok(())
    }
}

impl WriteBack {
    /// Write the sectors of the slot that differ from the loaded image back into the image file.
    pub fn write(&self, session: &fujimod_emulator_session::Session) -> Result<(), String> {
        let current = session.card_medium(&format!("{:#x}", self.slot))?;
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
[[device]]
base = 0xff9b4000
size = 0x1e0
kind = "sdhci"
[device.options.controller]
base = 0xff9b4000
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
[[sd]]
base = 0xff9b4000
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
        assert!(LoaderCard::load(SLOT, None, &[]).unwrap().is_none());
    }

    #[test]
    fn card_files_insert_a_present_card_and_wire_interrupt_262() {
        let directory = std::env::temp_dir().join(format!("loader-card-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("note.txt");
        std::fs::write(&file, b"hello").unwrap();
        let card = LoaderCard::load(SLOT, None, &[("NOTE.TXT".into(), file.clone())]).unwrap().unwrap();
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
        // Slot 1 alone leaves slot 2 as the manifest declares it.
        assert!(!state.sd[1].present && !state.device[1].options.contains_key("irq"));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_second_card_goes_in_slot_two_on_interrupt_269_and_leaves_slot_one_alone() {
        let directory = std::env::temp_dir().join(format!("loader-card-2-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("note.txt");
        std::fs::write(&file, b"second").unwrap();
        let files = [("NOTE.TXT".to_string(), file)];
        let first = LoaderCard::load(SLOT, None, &files).unwrap().unwrap();
        let second = LoaderCard::load(SLOT_2, None, &files).unwrap().unwrap();
        // Both cards may hold a file of the same name: the assets do not collide.
        assert_eq!(first.assets.keys().collect::<Vec<_>>(), ["NOTE.TXT"]);
        assert_eq!(second.assets.keys().collect::<Vec<_>>(), ["0xff9b4000/NOTE.TXT"]);
        let mut alone = state();
        first.apply(&mut alone).unwrap();
        let mut both = state();
        first.apply(&mut both).unwrap();
        second.apply(&mut both).unwrap();
        assert_eq!(serde_json::to_vec(&both.sd[0]).unwrap(), serde_json::to_vec(&alone.sd[0]).unwrap());
        assert_eq!(serde_json::to_vec(&both.device[0]).unwrap(), serde_json::to_vec(&alone.device[0]).unwrap());
        let slot = &both.sd[1];
        assert!(slot.present && !slot.protected);
        assert_eq!(slot.files, [("NOTE.TXT".to_string(), "0xff9b4000/NOTE.TXT".to_string())]);
        assert_eq!(both.device[1].options["irq"].as_integer(), Some(SLOT_2_IRQ));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_manifest_without_slot_one_is_refused() {
        let card = LoaderCard { slot: SLOT, card: None, assets: BTreeMap::new(), write_back: None };
        let mut state = state();
        state.sd.clear();
        assert!(card.apply(&mut state).unwrap_err().contains("no SD slot"));
    }
}
