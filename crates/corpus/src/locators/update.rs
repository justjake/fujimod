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
    flash_dat_bias: Option<u64>,
) -> Result<Value, String> {
    if !matches!(processor, "xp4" | "xp5" | "xpp" | "exr2") {
        return Err(format!(
            "{processor}: flash reader contract is not implemented"
        ));
    }
    let container_origin = fujimod_firmware::outer_header(source)
        .map_err(|e| e.to_string())?
        .payload_offset as u64;
    let bytes = fujimod_firmware::invert(source);
    let mut value = if processor == "exr2" {
        let control = exr2_control(&bytes[container_origin as usize..])?;
        exr2_flash(index, firmware, container_origin, control)?
    } else if matches!(processor, "xp4" | "xpp") {
        let bias = flash_dat_bias.ok_or("flash locator requires its registered DAT bias")?;
        flash(index, firmware, processor, jedec_id, bias, 0)?
    } else {
        let (origin, control) = image_offsets(index, firmware, &bytes, container_origin)?;
        flash(index, firmware, processor, jedec_id, origin, control)?
    };
    let offset = value["control"]["offset"]
        .as_u64()
        .ok_or("missing flash control")? as usize;
    let size = value["control"]["size"]
        .as_u64()
        .ok_or("missing flash control size")? as usize;
    let start = usize::try_from(
        value["update_image"]["dat_offset"]
            .as_u64()
            .ok_or("missing flash update bias")?,
    )
    .ok()
    .and_then(|origin| origin.checked_add(offset))
    .ok_or("flash control offset overflow")?;
    let end = start
        .checked_add(size)
        .ok_or("flash control extent overflow")?;
    let control = bytes
        .get(start..end)
        .ok_or("truncated flash comparison control")?;
    if value["device"]["kind"] == "serial-nand"
        && control.get(..8) != Some(&[0x50, 1, 0, 0, 0x80, 0, 0, 0])
    {
        return Err("NAND controller parameter page differs".into());
    }
    value["control"]["sha256"] = serde_json::json!(fujimod_firmware::sha256(control));
    Ok(value)
}
