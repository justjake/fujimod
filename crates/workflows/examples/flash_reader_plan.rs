//! Validate a built flash-reader artifact against the bundled catalog, offline.
use fujimod_catalog::ReferenceCatalog;
use fujimod_workflows::flash::Payload;
use serde::Deserialize;
use std::{fs, path::PathBuf};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    payload: Payload,
    image: String,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::args().nth(1).ok_or("artifact JSON required")?);
    let artifact: Artifact = serde_json::from_slice(&fs::read(&path)?)?;
    if artifact.image.contains(['/', '\\']) {
        return Err("image must be beside metadata".into());
    }
    let selection =
        ReferenceCatalog::bundled()?.resolve_firmware(artifact.payload.firmware_id())?;
    let flash = selection.flash.ok_or("catalog has no flash fact")?;
    let bulk = selection.bulk.ok_or("catalog has no bulk fact")?;
    if selection
        .firmware
        .update
        .as_ref()
        .is_none_or(|u| u.sha256 != artifact.payload.firmware_sha256())
    {
        return Err("firmware hash differs".into());
    }
    let image = fs::read(
        path.parent()
            .ok_or("artifact directory")?
            .join(artifact.image),
    )?;
    artifact.payload.validate(&flash, &bulk, &image)?;
    println!(
        "{}: {} payload bytes, {} flash bytes, {}-byte buffer",
        flash.firmware_id,
        image.len(),
        flash.window.size,
        artifact.payload.buffer_capacity()
    );
    Ok(())
}
