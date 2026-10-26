//! EXR II board constructors for this crate's models.
use crate::companion;
use fujimod_emulator_exr2_plan::{Model, construction::{Builder, Built, Constructors, decode}};
use fujimod_emulator_exr2_resources::wire;

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("companion", companion);
    constructors.add("fe80-processing", fe80_processing);
}

fn companion(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::Companion { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::companion::Options = decode(options)?;
        Box::new(companion::ProgramUpload::new(options)?)
    }))
}

fn fe80_processing(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::Fe80Processing {
        transport,
        options,
        frames,
        routes,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let transport: crate::companion::Options = decode(transport)?;
        let options: crate::companion_processing::Options = decode(options)?;
        Box::new(crate::companion_processing::Processing::new(
            transport,
            options,
            crate::companion_processing::Wiring {
                frames: fujimod_emulator_exr2_images::images(resources)?.images
                    .get(&frames)
                    .cloned()
                    .ok_or_else(|| format!("missing FE80 frame clock {frames}"))?,
                routes: wire!(resources, id, routes, IrqRoutes),
            },
        )?)
    }))
}
