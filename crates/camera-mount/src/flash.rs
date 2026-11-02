//! Validated offline reader artifacts. Loading never opens a camera.
use fujimod_catalog::{ResolvedSelection, capture::FirmwareFlash};
use fujimod_workflows::flash::{Artifact, Payload};
use std::{fs, path::Path};

pub(crate) struct Reader {
    pub facts: FirmwareFlash,
    pub payload: Payload,
    pub image: Vec<u8>,
}
impl Reader {
    pub fn load(path: &Path, selected: &ResolvedSelection) -> Result<Self, String> {
        let artifact: Artifact =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if artifact.image.is_empty()
            || artifact.image.contains(['/', '\\'])
            || matches!(artifact.image.as_str(), "." | "..")
        {
            return Err("Flash image must be beside the reader metadata".into());
        }
        let image = fs::read(
            path.parent()
                .ok_or("Reader directory is missing")?
                .join(&artifact.image),
        )
        .map_err(|e| e.to_string())?;
        let facts = selected
            .flash
            .as_ref()
            .ok_or("Selected firmware has no internal-storage reader")?;
        let bulk = selected
            .bulk
            .as_ref()
            .ok_or("Selected firmware has no bulk transport")?;
        if selected
            .firmware
            .update
            .as_ref()
            .is_none_or(|update| update.sha256 != artifact.payload.firmware_sha256())
        {
            return Err("Flash reader update identity differs".into());
        }
        artifact
            .payload
            .validate(facts, bulk, &image)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            facts: facts.clone(),
            payload: artifact.payload,
            image,
        })
    }
}
