//! Terminal SD entry with GPIO-only writes and no firmware startup.
use crate::{
    Generation, Recipe, Xp5SourceProof,
    image::{self, File, Result, put, require},
};
use serde::Deserialize;

pub(crate) mod initialization;
use initialization::Initialization;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    firmware_id: String,
    proofs: Vec<Xp5SourceProof>,
    initialization: Initialization,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    schema: u32,
    size: usize,
    config: usize,
    capacity: usize,
}

fn binding(id: &str) -> Result<Option<Binding>> {
    let rows: Vec<Binding> = serde_json::from_str(include_str!(
        "../../../catalog/recovery/green-led.generated.json"
    ))
    .map_err(|e| image::Error::Invalid(e.to_string()))?;
    Ok(rows.into_iter().find(|row| row.firmware_id == id))
}

/// Required filenames for the terminal LED entry, independent of updater support.
pub fn filenames(recipe: &Recipe) -> Result<Vec<String>> {
    if binding(&recipe.firmware_id)?.is_none() {
        return Ok(vec![]);
    }
    Ok(match recipe.generation {
        Generation::Xp4 => vec!["ffinit.bin".into()],
        Generation::Xp5 => vec!["marble_init.bin".into()],
        _ => {
            let Some(header) = &recipe.packaging else {
                return Ok(vec![]);
            };
            vec![
                header.header_filename.clone(),
                header.image_filename.clone(),
            ]
        }
    })
}

pub(crate) fn verified_writes(
    recipe: &Recipe,
    source: &[u8],
) -> Result<Vec<initialization::Write>> {
    require(
        source.len() == recipe.source_size,
        "Update size differs from recipe",
    )?;
    image::hash(source, &recipe.source_sha256)?;
    let binding = binding(&recipe.firmware_id)?
        .ok_or_else(|| image::Error::Invalid("Missing green LED binding".into()))?;
    let inverted = fujimod_firmware::invert(source);
    let mut verified = Vec::new();
    for proof in &binding.proofs {
        let bytes = proof.source.read(&inverted)?;
        let span = image::extent(&bytes, proof.offset, proof.size)?;
        image::hash(span, &proof.sha256)?;
        verified.push(span.to_vec());
    }
    binding.writes(recipe, &verified)
}

/// Keep the selected green latch asserted through the stock loader GPIO table.
/// The application and its endpoint writer remain unchanged.
pub(crate) fn preserve_loader_green(
    recipe: &Recipe,
    source: &[u8],
    ram: &mut [u8],
) -> Result<Vec<(usize, u32, u32)>> {
    let _ = verified_writes(recipe, source)?;
    let binding = binding(&recipe.firmware_id)?
        .ok_or_else(|| image::Error::Invalid("Missing green LED binding".into()))?;
    let initialization::Initialization::StockGpio { ports } = &binding.initialization else {
        return Ok(vec![]);
    };
    let inverted = fujimod_firmware::invert(source);
    let pair = &binding.proofs[0];
    let raw = pair.source.read(&inverted)?;
    let id_at = if matches!(
        recipe.generation,
        Generation::Exr | Generation::ExrPro | Generation::Exr2
    ) {
        8
    } else {
        4
    };
    let ident = image::word(&raw, pair.offset + id_at)?;
    let polarity = image::word(&raw, pair.offset + id_at + 4)?;
    let port = ports
        .iter()
        .find(|p| p.port == ident >> 16)
        .ok_or_else(|| image::Error::Invalid("Missing green GPIO port".into()))?;
    let proof = &binding.proofs[port.proof];
    let plan = recipe
        .plan
        .as_ref()
        .ok_or_else(|| image::Error::Invalid("Missing RAM plan".into()))?;
    let regions: Vec<_> = plan
        .regions
        .iter()
        .filter(|r| {
            r.source.dat_offset == proof.source.dat_offset && r.source.sha256 == proof.source.sha256
        })
        .collect();
    require(
        regions.len() == 1 && regions[0].address == 0,
        "Green GPIO table is not in the loader",
    )?;
    let at = regions[0].address + proof.offset + 20;
    let raw = proof.source.read(&inverted)?;
    let old = image::word(&raw, proof.offset + 20)?;
    let bit = 1 << (ident & 0xffff);
    let new = (old & !bit) | (polarity << (ident & 0xffff));
    require(
        image::word(ram, at)? == old,
        "Green loader GPIO default differs",
    )?;
    put(ram, at, new)?;
    Ok(vec![(at, old, new)])
}

/// Verify the selected official update, bind GPIO writes, and package a terminal entry.
pub fn build(recipe: &Recipe, source: &[u8]) -> Result<Vec<File>> {
    require(
        source.len() == recipe.source_size,
        "Update size differs from recipe",
    )?;
    image::hash(source, &recipe.source_sha256)?;
    let writes = verified_writes(recipe, source)?;
    let inverted = fujimod_firmware::invert(source);
    let aarch64 = matches!(recipe.generation, Generation::Xp5);
    let (program, layout) = if aarch64 {
        (
            include_bytes!("../assets/green-led-aarch64.generated.bin").as_slice(),
            include_str!("../assets/green-led-aarch64.generated.json"),
        )
    } else {
        (
            include_bytes!("../assets/green-led-arm.generated.bin").as_slice(),
            include_str!("../assets/green-led-arm.generated.json"),
        )
    };
    let layout: Layout =
        serde_json::from_str(layout).map_err(|e| image::Error::Invalid(e.to_string()))?;
    require(
        layout.schema == 1
            && layout.size == program.len()
            && layout.capacity == 24
            && !writes.is_empty()
            && writes.len() <= layout.capacity,
        "Invalid green LED program layout",
    )?;
    let mut program = program.to_vec();
    require(
        image::extent(&program, layout.config, 4 + layout.capacity * 12)?
            .iter()
            .all(|v| *v == 0),
        "Green LED configuration is not empty",
    )?;
    put(&mut program, layout.config, writes.len() as u32)?;
    for (index, write) in writes.iter().enumerate() {
        require(
            write.address >= 0xf0000000 && write.address < 0xfffff000 && write.address % 4 == 0,
            "Invalid green LED register",
        )?;
        for (field, value) in [write.address, write.clear, write.value]
            .into_iter()
            .enumerate()
        {
            put(
                &mut program,
                layout.config + 4 + index * 12 + field * 4,
                value,
            )?;
        }
    }
    match recipe.generation {
        Generation::Xp4 => Ok(vec![File::new("ffinit.bin", program)?]),
        Generation::Xp5 => Ok(vec![File::new("marble_init.bin", program)?]),
        _ => {
            let header = recipe
                .packaging
                .as_ref()
                .ok_or_else(|| image::Error::Invalid("Missing SD header".into()))?;
            header.package_bytes(&inverted, header.entry, program)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn led_availability_is_independent_of_updater_and_never_includes_the_update() {
        for id in [
            "x10-201",
            "xe1-271",
            "xt1-552",
            "gfx50r-311",
            "x100v-301",
            "x100vi-131",
        ] {
            let mut recipe = Recipe::bundled(id).unwrap().unwrap();
            recipe.updater = None;
            let files = filenames(&recipe).unwrap();
            assert!(!files.is_empty(), "{id}");
            assert!(!files.contains(&recipe.update_filename));
            assert!(!files.iter().any(|name| name == "ffuser.bin"));
            assert!(
                build(&recipe, &[0; 4])
                    .unwrap_err()
                    .to_string()
                    .contains("Update size")
            );
        }
    }
}
