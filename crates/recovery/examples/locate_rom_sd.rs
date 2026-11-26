//! Inspect an update supplied by the caller; emit portable startup facts.
use fujimod_recovery::rom_sd::{self, Processor};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (generation, path, selector) = match args.as_slice() {
        [generation, path] => (generation, path, None),
        [generation, path, option, selector]
            if generation == "xpp" && option == "--device-selector" =>
        {
            (generation, path, Some(selector.parse::<u8>()?))
        }
        _ => {
            return Err(
                "Usage: locate_rom_sd exr|exrpro|xpp UPDATE.DAT [--device-selector N (xpp)]".into(),
            );
        }
    };
    let processor = match generation.as_str() {
        "exr" => Processor::Exr,
        "exrpro" => Processor::ExrPro,
        "xpp" => Processor::Xpp,
        _ => return Err("Unknown processor".into()),
    };
    let bytes = std::fs::read(path)?;
    let output = if let Some(device) = selector {
        let range = rom_sd::select_xpp_device(&bytes, device)?;
        let startup = rom_sd::locate(processor, &fujimod_firmware::invert(&bytes[range.clone()]))?;
        serde_json::to_string_pretty(&serde_json::json!({
            "device_selector": device,
            "dat_offset": range.start,
            "dat_size": range.len(),
            "startup": startup,
        }))?
    } else {
        serde_json::to_string_pretty(&rom_sd::locate(
            processor,
            &fujimod_firmware::invert(&bytes),
        )?)?
    };
    println!("{output}");
    Ok(())
}
