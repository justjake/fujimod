//! Inspect an update supplied by the caller; emit portable startup facts.
use fujimod_recovery::rom_sd::{self, Processor};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("Usage: locate_rom_sd exr|exrpro|xpp UPDATE.DAT".into());
    }
    let processor = match args[0].as_str() {
        "exr" => Processor::Exr,
        "exrpro" => Processor::ExrPro,
        "xpp" => Processor::Xpp,
        _ => return Err("Unknown processor".into()),
    };
    let bytes = std::fs::read(&args[1])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&rom_sd::locate(
            processor,
            &fujimod_firmware::invert(&bytes)
        )?)?
    );
    Ok(())
}
