//! Inspect the Agate read contract in an update supplied by the caller.
use fujimod_recovery::{Generation, Recipe, xpp};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("Usage: xpp_read_map FIRMWARE UPDATE.DAT".into());
    }
    let recipe = Recipe::bundled(&args[0])?.ok_or("Missing recovery recipe")?;
    if !matches!(recipe.generation, Generation::Xpp) {
        return Err("Requires X-Processor Pro".into());
    }
    let bytes = std::fs::read(&args[1])?;
    if bytes.len() != recipe.source_size || fujimod_firmware::sha256(&bytes) != recipe.source_sha256
    {
        return Err("Update differs from recipe".into());
    }
    let reads = xpp::locate(
        &fujimod_firmware::invert(&bytes),
        recipe.plan.as_ref().ok_or("Missing plan")?,
    )?;
    println!("{}", serde_json::to_string_pretty(&reads)?);
    Ok(())
}
