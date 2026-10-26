//! Batch JSON-lines locator driver. Inputs name the index, hardware and decoded
//! update packaging; stdout contains facts or per-locator diagnostics.
use fujimod_corpus::{Store, locators};
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};
#[derive(Deserialize)]
struct Input {
    firmware_id: String,
    processor: String,
    jedec_id: Option<u32>,
    update_path: Option<String>,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open(std::env::args().nth(1).ok_or("index filename required")?)?;
    for line in io::stdin().lock().lines() {
        let input: Input = serde_json::from_str(&line?)?;
        let flash = input
            .update_path
            .as_ref()
            .ok_or_else(|| "update input missing".to_string())
            .and_then(|p| std::fs::read(p).map_err(|e| e.to_string()))
            .and_then(|bytes| {
                locators::flash_update(
                    &store,
                    &input.firmware_id,
                    &input.processor,
                    input.jedec_id,
                    &bytes,
                )
            });
        let rom = locators::boot_rom(&input.processor);
        let audit = if matches!(input.processor.as_str(), "xp4" | "xpp" | "exr" | "exrpro") {
            input.update_path.as_ref().map(|p| {
                std::fs::read(p)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        locators::boot_rom_audit(
                            &store,
                            &input.firmware_id,
                            &input.processor,
                            &fujimod_firmware::invert(&bytes),
                        )
                    })
            })
        } else {
            None
        };
        println!(
            "{}",
            json!({"firmware_id":input.firmware_id,"flash":flash.as_ref().ok(),"flash_error":flash.as_ref().err(),"boot_rom":rom.as_ref().ok(),"boot_rom_error":rom.as_ref().err(),"boot_rom_audit":audit})
        );
    }
    Ok(())
}
