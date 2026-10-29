//! Declarative external NAND inputs shared by native and browser frontends.
//! Preparation verifies declared asset windows before constructing a medium.
use crate::persistent_inputs::{PreparePersistentInputs, tag_managed_blocks};
use fujimod_emulator_devices::parts::nand::{Nand, Options, Overlay};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NandInput {
    Committed {
        options: Options,
        base: String,
        image: String,
    },
    Raw {
        options: Options,
        base: String,
        overlays: Vec<(u64, String)>,
        spare: Vec<(u32, String)>,
    },
    Catalog {
        options: Options,
        definition: fujimod_catalog_settings::persistent_inputs::PersistentInputs,
        set: String,
        base: String,
        assets: BTreeMap<String, String>,
    },
    /// A settings bank built from the installed flash image (`base`) with the
    /// requested named settings, placed in its stock flash blocks before cold boot.
    /// `camera` names the zstd-compressed assets holding a camera's bank
    /// sections (section name to asset); the bank is then that camera's.
    /// `factory_fresh` leaves the user profile as a new camera ships it: no
    /// journal records, so only bank settings may be requested.
    Settings {
        options: Options,
        base: String,
        layout: fujimod_catalog_settings::FirmwareSettings,
        requested: BTreeMap<String, String>,
        #[serde(default)]
        camera: BTreeMap<String, String>,
        #[serde(default)]
        factory_fresh: bool,
    },
    /// The stock user profile's journal records built from the installed flash
    /// image (`base`) with the requested named settings; the settings bank stays
    /// as the flash image holds it. Requested bank settings are refused.
    Profile {
        options: Options,
        base: String,
        layout: fujimod_catalog_settings::FirmwareSettings,
        requested: BTreeMap<String, String>,
    },
}
impl NandInput {
    pub fn build(self, loaded: &BTreeMap<String, Vec<u8>>) -> Result<Nand, String> {
        let asset = |name: &str| {
            loaded
                .get(name)
                .ok_or_else(|| format!("Missing NAND asset {name}"))
        };
        match self {
            Self::Committed {
                options,
                base,
                image,
            } => Nand::new(
                options,
                Arc::from(asset(&base)?.as_slice()),
                vec![],
                BTreeMap::new(),
            )?
            .restore_committed(asset(&image)?),
            Self::Raw {
                options,
                base,
                overlays,
                spare,
            } => Nand::new(
                options,
                Arc::from(asset(&base)?.as_slice()),
                overlays
                    .into_iter()
                    .map(|(offset, name)| {
                        Ok(Overlay {
                            offset,
                            bytes: Arc::from(asset(&name)?.as_slice()),
                        })
                    })
                    .collect::<Result<_, String>>()?,
                spare
                    .into_iter()
                    .try_fold(BTreeMap::new(), |mut tags, (row, name)| {
                        if tags.insert(row, asset(&name)?.clone()).is_some() {
                            return Err("Duplicate initial NAND spare row".to_owned());
                        }
                        Ok(tags)
                    })?,
            ),
            Self::Catalog {
                options,
                definition,
                set,
                base,
                assets,
            } => {
                let windows = assets
                    .into_iter()
                    .map(|(name, source)| Ok((name, asset(&source)?.clone())))
                    .collect::<Result<_, String>>()?;
                definition.prepare(options, &set, &windows, Arc::from(asset(&base)?.as_slice()))
            }
            Self::Settings {
                options,
                base,
                layout,
                requested,
                camera,
                factory_fresh,
            } => {
                if factory_fresh
                    && let Some(name) = requested
                        .keys()
                        .find(|name| layout.settings.get(*name).is_none_or(|s| s.store != "bank"))
                {
                    return Err(format!("A factory-fresh profile cannot hold setting {name}"));
                }
                let sections = camera
                    .into_iter()
                    .map(|(section, name)| Ok((section, decompress(asset(&name)?)?)))
                    .collect::<Result<BTreeMap<_, _>, String>>()?;
                let camera = (!sections.is_empty()).then_some(&sections);
                let profile = !factory_fresh;
                settings_medium(options, asset(&base)?, &layout, &requested, camera, true, profile)
            }
            Self::Profile {
                options,
                base,
                layout,
                requested,
            } => {
                if let Some(name) = requested
                    .keys()
                    .find(|name| layout.settings.get(*name).is_some_and(|s| s.store == "bank"))
                {
                    return Err(format!("Profile input cannot hold bank setting {name}"));
                }
                if layout.profile.is_none() {
                    return Err(format!("{} has no located user profile", layout.firmware_id));
                }
                settings_medium(options, asset(&base)?, &layout, &requested, None, false, true)
            }
        }
    }
}

/// The bytes of a zstd frame.
fn decompress(compressed: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(compressed)
        .map_err(|failure| format!("Camera section is not zstd: {failure}"))?;
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .map_err(|failure| format!("Camera section does not decompress: {failure}"))?;
    Ok(bytes)
}

/// Place the built settings, with or without the bank and the user profile's
/// records, in their managed blocks.
fn settings_medium(
    options: Options,
    flash: &[u8],
    layout: &fujimod_catalog_settings::FirmwareSettings,
    requested: &BTreeMap<String, String>,
    camera: Option<&BTreeMap<String, Vec<u8>>>,
    with_bank: bool,
    with_profile: bool,
) -> Result<Nand, String> {
    let settings = fujimod_settings::build(layout, flash, requested, camera)?;
    let block_bytes = options.page_bytes as u64 * options.pages_per_block as u64;
    let mut placements = Vec::new();
    if with_bank {
        let bank = settings
            .bank
            .ok_or_else(|| format!("{} has no settings bank", layout.firmware_id))?;
        let mut bytes = vec![0xff; (u64::from(bank.blocks) * block_bytes) as usize];
        bytes
            .get_mut(..bank.bytes.len())
            .ok_or("Settings bank exceeds its flash blocks")?
            .copy_from_slice(&bank.bytes);
        placements.push((u64::from(bank.first_block), bytes, bank.logical_first));
    }
    let profile = settings.profile.filter(|_| with_profile);
    for record in profile.into_iter().flat_map(|p| p.records) {
        let mut block = vec![0xff; block_bytes as usize];
        block
            .get_mut(..record.bytes.len())
            .ok_or("Profile record exceeds its flash block")?
            .copy_from_slice(&record.bytes);
        let managed_index = record
            .managed_index
            .ok_or("A NAND profile record needs a managed block index")?;
        placements.push((u64::from(record.block), block, managed_index));
    }
    let mut overlays: Vec<Overlay> = Vec::new();
    let mut spare = BTreeMap::new();
    for (first_block, bytes, logical) in placements {
        let offset = first_block * block_bytes;
        if offset + bytes.len() as u64 > options.bytes {
            return Err("Settings exceed NAND geometry".into());
        }
        if overlays.iter().any(|o| {
            offset < o.offset + o.bytes.len() as u64 && o.offset < offset + bytes.len() as u64
        }) {
            return Err("Settings placements overlap".into());
        }
        tag_managed_blocks(&options, offset, bytes.len() as u64, logical, &mut spare)?;
        overlays.push(Overlay {
            offset,
            bytes: Arc::from(bytes),
        });
    }
    Nand::new(options, Arc::from(flash), overlays, spare)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn committed_nand_input_keeps_program_erase_and_base_identity() {
        let options = Options {
            bytes: 262144,
            page_bytes: 2048,
            pages_per_block: 64,
            spare_bytes: 64,
        };
        let base = vec![0xa5; 2048];
        let mut medium = Nand::new(
            options.clone(),
            Arc::from(base.as_slice()),
            vec![],
            BTreeMap::new(),
        )
        .unwrap();
        medium.erase(0).unwrap();
        medium
            .program(64, &vec![0x85; 2048], &vec![0x12; 64])
            .unwrap();
        let mut assets = BTreeMap::from([
            ("base".into(), base),
            ("saved".into(), medium.committed_image().unwrap()),
        ]);
        let spec =
            serde_json::json!({"kind":"committed","options":options,"base":"base","image":"saved"});
        let restored = serde_json::from_value::<NandInput>(spec.clone())
            .unwrap()
            .build(&assets)
            .unwrap();
        assert_eq!(restored.fingerprint(), medium.fingerprint());
        let mut bytes = [0; 2048];
        restored.read(0, &mut bytes).unwrap();
        assert_eq!(bytes, [255; 2048]);
        assets.insert("base".into(), vec![0; 2048]);
        assert!(
            serde_json::from_value::<NandInput>(spec)
                .unwrap()
                .build(&assets)
                .err()
                .unwrap()
                .contains("identity")
        );
    }

    #[test]
    fn declared_asset_names_build_the_same_owned_medium_without_a_frontend_model() {
        let spec = serde_json::json!({"kind":"raw", "options":{
            "bytes":262144,"page_bytes":2048,"pages_per_block":64,"spare_bytes":64},
            "base":"update","overlays":[[131072,"settings"]],"spare":[[64,"tag"]]});
        let inputs = BTreeMap::from([
            ("update".into(), vec![255; 2048]),
            ("settings".into(), vec![0x85; 2048]),
            ("tag".into(), vec![0x12, 0x34]),
        ]);
        let medium = serde_json::from_value::<NandInput>(spec.clone())
            .unwrap()
            .build(&inputs)
            .unwrap();
        let mut data = [0; 2048];
        medium.read(131072, &mut data).unwrap();
        assert_eq!(data, [0x85; 2048]);
        let mut tag = [0; 64];
        medium.read_spare(64, &mut tag).unwrap();
        assert_eq!(&tag[..2], &[0x12, 0x34]);
        assert!(tag[2..].iter().all(|b| *b == 255));
        let mut duplicate = spec.clone();
        duplicate["spare"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!([64, "tag"]));
        assert!(
            serde_json::from_value::<NandInput>(duplicate)
                .unwrap()
                .build(&inputs)
                .is_err()
        );
        let mut missing = inputs.clone();
        missing.remove("settings");
        assert!(
            serde_json::from_value::<NandInput>(spec)
                .unwrap()
                .build(&missing)
                .err()
                .unwrap()
                .contains("settings")
        );
    }
}

/// Sparse sectors supplied as external media, never synthesized file results.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CardInput {
    Sparse {
        options: fujimod_emulator_devices::parts::sd_card::Options,
        sectors: Vec<(u32, String)>,
    },
    Committed {
        image: String,
    },
}
impl CardInput {
    pub fn build(
        self,
        loaded: &BTreeMap<String, Vec<u8>>,
    ) -> Result<fujimod_emulator_devices::parts::sd_card::Card, String> {
        let (options, sectors) = match self {
            Self::Committed { image } => {
                let bytes = loaded.get(&image).ok_or("Missing committed card asset")?;
                let card: fujimod_emulator_devices::parts::sd_card::Card =
                    serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
                card.validate()?;
                return Ok(card);
            }
            Self::Sparse { options, sectors } => (options, sectors),
        };
        let mut card = fujimod_emulator_devices::parts::sd_card::Card::new(options)?;
        let mut seen = std::collections::BTreeSet::new();
        for (lba, source) in sectors {
            if !seen.insert(lba) {
                return Err("Duplicate initial card sector".into());
            }
            let bytes = loaded.get(&source).ok_or("Missing initial card sector")?;
            if bytes.len() != 512 {
                return Err("Initial card input must contain whole individual sectors".into());
            }
            card.write_bytes(lba as u64 * 512, bytes)?;
        }
        Ok(card)
    }
}

#[cfg(test)]
mod card_tests {
    use super::*;
    #[test]
    fn committed_card_input_preserves_revision_and_sparse_allocation_bound() {
        let mut card = fujimod_emulator_devices::parts::sd_card::Card::new(fujimod_emulator_devices::parts::sd_card::Options {
            sectors: 0x800000,
            max_dirty_sectors: 2,
        })
        .unwrap();
        card.write_bytes(512, &[0x85; 512]).unwrap();
        card.write_bytes(512, &[0x42; 512]).unwrap();
        let mut assets = BTreeMap::from([("saved".into(), serde_json::to_vec(&card).unwrap())]);
        let spec = serde_json::json!({"kind":"committed","image":"saved"});
        let restored = serde_json::from_value::<CardInput>(spec.clone())
            .unwrap()
            .build(&assets)
            .unwrap();
        assert_eq!(restored.fingerprint(), card.fingerprint());
        let mut invalid = serde_json::to_value(&card).unwrap();
        invalid["max_dirty"] = serde_json::json!(0);
        assets.insert("saved".into(), serde_json::to_vec(&invalid).unwrap());
        assert!(
            serde_json::from_value::<CardInput>(spec)
                .unwrap()
                .build(&assets)
                .is_err()
        );
    }

    #[test]
    fn sparse_card_assets_require_whole_distinct_sectors() {
        let spec = serde_json::json!({"kind":"sparse","options":{"sectors":0x800000,"max_dirty_sectors":8},
            "sectors":[[7,"payload"]]});
        let assets = BTreeMap::from([("payload".into(), vec![0x85; 512])]);
        let card = serde_json::from_value::<CardInput>(spec.clone())
            .unwrap()
            .build(&assets)
            .unwrap();
        assert_eq!(card.sector(7).unwrap(), [0x85; 512]);
        let mut duplicate = spec.clone();
        duplicate["sectors"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!([7, "payload"]));
        assert!(
            serde_json::from_value::<CardInput>(duplicate)
                .unwrap()
                .build(&assets)
                .is_err()
        );
        let bad = BTreeMap::from([("payload".into(), vec![0x85; 511])]);
        assert!(
            serde_json::from_value::<CardInput>(spec)
                .unwrap()
                .build(&bad)
                .is_err()
        );
    }
}
