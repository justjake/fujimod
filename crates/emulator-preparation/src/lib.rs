//! Bounded host input preparation, outside device execution: the declared
//! NAND and card media, images, statistics, sensors and card files of a run.
//! The image part's installed preparation builds the images, statistics and
//! sensors.
pub mod media;
pub mod persistent_inputs;

use fujimod_emulator_exr2_resources::{ImageSpec, Resources};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default)]
    nands: BTreeMap<String, crate::media::NandInput>,
    #[serde(default)]
    cards: BTreeMap<String, crate::media::CardInput>,
    #[serde(default)]
    images: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    statistics: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    sensors: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    files: BTreeMap<String, BTreeMap<String, String>>,
}
impl Spec {
    /// Fresh resources holding the run's prepared media and image inputs.
    pub fn build(self, assets: &BTreeMap<String, Vec<u8>>) -> Result<Resources, String> {
        let asset = |name: &str| {
            assets
                .get(name)
                .ok_or_else(|| format!("missing scene asset {name}"))
        };
        let mut resources = Resources::default();
        let inputs = &mut resources.inputs;
        for (name, card) in self.cards {
            inputs.cards.insert(name, card.build(assets)?);
        }
        for (name, nand) in self.nands {
            inputs.nands.insert(name, nand.build(assets)?);
        }
        for (name, files) in self.files {
            let mut total = 0usize;
            if files.len() > fujimod_emulator_storage::fat::MAX_ENTRIES {
                return Err("card input has too many files".into());
            }
            let files = files
                .into_iter()
                .map(|(path, source)| {
                    let bytes = asset(&source)?;
                    total = total.saturating_add(bytes.len());
                    if total > fujimod_emulator_storage::fat::MAX_PRELOAD_BYTES {
                        return Err("card input exceeds 512 MiB".into());
                    }
                    Ok((path, bytes.clone()))
                })
                .collect::<Result<Vec<_>, String>>()?;
            inputs.files.insert(name, files);
        }
        let images = ImageSpec {
            images: self.images,
            statistics: self.statistics,
            sensors: self.sensors,
        };
        if !images.is_empty() {
            resources.images = Some(fujimod_emulator_exr2_resources::prepare_images(images, assets)?);
        }
        Ok(resources)
    }
}
