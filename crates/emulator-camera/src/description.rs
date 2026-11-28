//! The camera description: the JSON every front door takes, turned
//! into a [`Request`].
//!
//! Host paths in a description are relative to `base`: the directory of the file
//! the description came from, or the current directory for inline JSON.
use crate::{
    Card, CardContents, CardFile, Cards, FirmwareChoice, FirmwareInput, Flash, LoaderImage,
    Request, Start,
};
use base64::Engine as _;
use fujimod_catalog_run_inputs::camera::{
    self as schema, CameraDescription, ExecutionPolicyName, StorageCard,
};
use fujimod_emulator_cpu::ExecutionPolicy;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The emulator request a camera description describes. The FujiMod fields
/// (`boot`, `snapshot`) are the caller's to act on.
pub fn request(camera: &CameraDescription, base: &Path) -> Result<Request, String> {
    let path = |text: &str| -> PathBuf { resolve(base, text) };
    if camera.body.trim().is_empty() {
        return Err("body is required: a catalog body identity such as xh2s".into());
    }
    let start = match (&camera.hold, &camera.loader_image) {
        (Some(_), Some(_)) => {
            return Err("hold and loader_image are alternative starts".into());
        }
        (Some(combination), None) => Start::Held {
            combination: combination.clone(),
        },
        (None, Some(image)) => {
            let sha256 = image.sha256.to_ascii_lowercase();
            if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("loader_image sha256 must be 64 hexadecimal characters".into());
            }
            let file = path(&image.path);
            let bytes =
                std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))?;
            Start::Loader(LoaderImage { bytes, sha256 })
        }
        (None, None) => Start::Rom,
    };
    let read = |text: &str| -> Result<Vec<u8>, String> {
        let file = path(text);
        std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))
    };
    let engine = camera
        .engine
        .as_deref()
        .map(|engine| {
            engine
                .parse()
                .map_err(|_| format!("engine must be unicorn or hypervisor, not {engine:?}"))
        })
        .transpose()?;
    Ok(Request {
        body: camera.body.trim().to_owned(),
        firmware: FirmwareChoice::parse(camera.firmware.as_deref()),
        files: match &camera.firmware_files {
            None => FirmwareInput::Library,
            Some(files) => FirmwareInput::Files {
                update: path(&files.update),
                boot_rom: files.boot_rom.as_deref().map(path),
            },
        },
        application_plan: camera.application_plan.as_deref().map(path),
        overlays: camera.overlays.clone(),
        start,
        flash: match &camera.flash_image {
            Some(file) => Flash::Image(read(file)?),
            None => Flash::Stock,
        },
        cards: Cards {
            sd_card: camera
                .sd_card
                .as_ref()
                .map(|card| card_from(card, base, "sd_card"))
                .transpose()?,
            sd_card_2: camera
                .sd_card_2
                .as_ref()
                .map(|card| card_from(card, base, "sd_card_2"))
                .transpose()?,
            cfexpress_card: camera
                .cfexpress_card
                .as_ref()
                .map(|card| card_from(card, base, "cfexpress_card"))
                .transpose()?,
        },
        policy: match camera.execution_policy {
            None | Some(ExecutionPolicyName::Exact) => ExecutionPolicy::Exact,
            Some(ExecutionPolicyName::Interactive) => ExecutionPolicy::Interactive,
        },
        engine,
        cache: if camera.boot_cache.unwrap_or(true) && camera.snapshot.is_none() {
            crate::BootCache::Reuse
        } else {
            crate::BootCache::Bypass
        },
        run_to_rec: camera.run_to_rec.unwrap_or(true),
        nand_image: camera.nand_image.as_deref().map(read).transpose()?,
        face_result_profile: camera.face_result_profile.clone(),
        settings: camera.settings.clone(),
        clock_seconds_2000: camera.date.as_deref().map(clock_seconds).transpose()?,
        scene: camera.scene.as_deref().map(path),
        kept: Default::default(),
    })
}

/// A path in a camera, relative to the camera's directory.
pub fn resolve(base: &Path, text: &str) -> PathBuf {
    let path = Path::new(text);
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

/// A camera card as a request card.
pub fn card_from(card: &StorageCard, base: &Path, slot: &str) -> Result<Card, String> {
    let contents = match (&card.files, &card.directory, &card.image) {
        (None, None, None) => CardContents::Files(BTreeMap::new()),
        (Some(files), None, None) => CardContents::Files(
            files
                .iter()
                .map(|(name, file)| {
                    let name = name.trim_start_matches('/').to_owned();
                    if name.is_empty() {
                        return Err(format!("{slot}: a card path is empty"));
                    }
                    let content = match file {
                        schema::CardFile::Source { source } => {
                            CardFile::Source(resolve(base, source))
                        }
                        schema::CardFile::Bytes { bytes } => CardFile::Bytes(
                            base64::engine::general_purpose::STANDARD
                                .decode(bytes)
                                .map_err(|error| format!("{slot} {name}: bytes: {error}"))?,
                        ),
                    };
                    Ok((name, content))
                })
                .collect::<Result<_, String>>()?,
        ),
        (None, Some(directory), None) => CardContents::Files(
            crate::directory_files(&resolve(base, directory))
                .map_err(|error| error.detail)?
                .into_iter()
                .map(|(name, bytes)| (name, CardFile::Bytes(bytes)))
                .collect(),
        ),
        (None, None, Some(image)) => CardContents::Image(resolve(base, image)),
        _ => {
            return Err(format!(
                "{slot}: files, directory and image are alternatives"
            ));
        }
    };
    Ok(Card {
        present: card.present,
        write_protected: card.write_protected,
        contents,
        layout: card.layout.as_ref().map(|layout| crate::CardLayout {
            sectors: layout.sectors,
            label: layout.label.clone(),
            partition_start: layout.partition_start,
            sectors_per_cluster: layout.sectors_per_cluster,
        }),
        transfer_ticks: card.transfer_ticks,
        initialization_fault: card.initialization_fault.clone(),
    })
}

/// `YYYY-MM-DDTHH:MM:SS` local time as seconds since 2000-01-01 00:00:00.
pub fn clock_seconds(date: &str) -> Result<u32, String> {
    let refuse = || format!("date {date:?} must be YYYY-MM-DDTHH:MM:SS, from 2000 on");
    let (day, time) = date.split_once(['T', ' ']).ok_or_else(refuse)?;
    let numbers = |text: &str, separator: char| -> Result<Vec<i64>, String> {
        text.split(separator)
            .map(|part| part.parse::<i64>().map_err(|_| refuse()))
            .collect()
    };
    let [year, month, day_of_month] = numbers(day, '-')?[..] else {
        return Err(refuse());
    };
    let [hour, minute, second] = numbers(time, ':')?[..] else {
        return Err(refuse());
    };
    if !(2000..=2135).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day_of_month)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
    {
        return Err(refuse());
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * m + 2) / 5 + day_of_month - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_1970 = era * 146_097 + day_of_era - 719_468;
    let days_2000 = days_1970 - 10_957;
    let seconds = days_2000 * 86_400 + hour * 3600 + minute * 60 + second;
    u32::try_from(seconds).map_err(|_| refuse())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_count_seconds_from_2000() {
        assert_eq!(clock_seconds("2000-01-01T00:00:00").unwrap(), 0);
        assert_eq!(
            clock_seconds("2000-03-01T00:00:01").unwrap(),
            60 * 86_400 + 1
        );
        assert_eq!(clock_seconds("2026-11-28T12:30:00").unwrap(), 849_184_200);
        assert!(clock_seconds("1999-12-31T23:59:59").is_err());
        assert!(clock_seconds("2026-11-28").is_err());
    }

    #[test]
    fn a_document_needs_a_body_and_takes_relative_paths_from_its_directory() {
        let base = Path::new("/requests");
        let error = request(&CameraDescription::default(), base).err().unwrap();
        assert!(error.contains("body is required"), "{error}");
        let camera: CameraDescription = serde_json::from_value(serde_json::json!({
            "body": "xh2s",
            "firmware": "7.30",
            "settings": {"language": "en"},
            "sd_card": {"files": {"DCIM/100_FUJI/A.TXT": {"source": "a.txt"},
                                  "B.TXT": {"bytes": "aGk="}}},
            "scene": "scene.json",
        }))
        .unwrap();
        let built = request(&camera, base).unwrap();
        assert_eq!(built.body, "xh2s");
        assert_eq!(built.firmware, FirmwareChoice::Named("7.30".into()));
        assert_eq!(
            built.scene.as_deref(),
            Some(Path::new("/requests/scene.json"))
        );
        let CardContents::Files(files) = built.cards.first().contents else {
            panic!("files");
        };
        assert_eq!(
            files["DCIM/100_FUJI/A.TXT"],
            CardFile::Source(PathBuf::from("/requests/a.txt"))
        );
        assert_eq!(files["B.TXT"], CardFile::Bytes(b"hi".to_vec()));
        let none: CameraDescription =
            serde_json::from_value(serde_json::json!({"body": "x70", "firmware": "none"})).unwrap();
        assert_eq!(
            request(&none, base).unwrap().firmware,
            FirmwareChoice::Empty
        );
    }

    #[test]
    fn a_card_names_one_kind_of_contents() {
        let card: StorageCard = serde_json::from_value(serde_json::json!({
            "files": {}, "image": "card.img"
        }))
        .unwrap();
        let error = card_from(&card, Path::new("."), "sd_card").unwrap_err();
        assert!(error.contains("alternatives"), "{error}");
    }
}
