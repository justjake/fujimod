//! Update-container composition for the flash locators.
use super::{exr2_control, exr2_flash, flash, image_offsets};
use crate::Lookup;
use serde_json::Value;

/// Locate a flash contract from the holder's original update bytes. Packaging
/// and per-processor reader semantics share one entry point across callers.
pub fn flash_update<L: Lookup>(
    index: &L,
    firmware: &str,
    processor: &str,
    jedec_id: Option<u32>,
    source: &[u8],
) -> Result<Value, String> {
    if !matches!(processor, "xp5" | "exr2") {
        return Err(format!(
            "{processor}: flash reader contract is not implemented"
        ));
    }
    let origin = fujimod_firmware::outer_header(source)
        .map_err(|e| e.to_string())?
        .payload_offset as u64;
    let bytes = fujimod_firmware::invert(source);
    let value = if processor == "exr2" {
        let control = exr2_control(&bytes[origin as usize..])?;
        exr2_flash(index, firmware, origin, control)?
    } else {
        let (origin, control) = image_offsets(index, firmware, &bytes, origin)?;
        flash(index, firmware, processor, jedec_id, origin, control)?
    };
    let offset = value["control"]["offset"]
        .as_u64()
        .ok_or("missing flash control")? as usize;
    let size = value["control"]["size"]
        .as_u64()
        .ok_or("missing flash control size")? as usize;
    if bytes
        .get(origin as usize + offset..origin as usize + offset + size)
        .is_none()
    {
        return Err("truncated flash comparison control".into());
    }
    if value["device"]["kind"] == "serial-nand"
        && bytes.get(origin as usize + offset..origin as usize + offset + 8)
            != Some(&[0x50, 1, 0, 0, 0x80, 0, 0, 0])
    {
        return Err("NAND controller parameter page differs".into());
    }
    Ok(value)
}
