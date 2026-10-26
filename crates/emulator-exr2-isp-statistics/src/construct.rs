//! EXR II board constructors for this crate's models.
use fujimod_emulator_exr2_plan::{Model, construction::{Builder, Built, Constructors, decode}};
use fujimod_emulator_exr2_resources::wire;

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("statistics", statistics);
    constructors.add("phase-detection", phase_detection);
    constructors.add("window-analysis", window_analysis);
}

fn statistics(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::Statistics {
        options,
        routes,
        frames,
        images,
        grid,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    let options = decode(options)?;
    Ok(Built::Device(Box::new(crate::producer::Block18000::new(
        options,
        crate::producer::Wiring {
            grid: grid
                .map(|name| Ok(fujimod_emulator_exr2_images::image_wire!(resources, id, name, ImageGrid)))
                .transpose()?,
            routes: wire!(resources, id, routes, IrqRoutes),
            images: fujimod_emulator_exr2_images::images(resources)?.images
                .get(&images)
                .cloned()
                .ok_or_else(|| format!("missing histogram image frames {images}"))?,
            frames: fujimod_emulator_exr2_images::images(resources)?.statistics
                .get(&frames)
                .cloned()
                .ok_or_else(|| format!("missing statistics frames {frames}"))?,
        },
    )?)))
}

fn phase_detection(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::PhaseDetection { options, routes } = model else {
        return Err("constructor called for another model kind".into());
    };
    let options = decode(options)?;
    Ok(Built::Device(Box::new(
        crate::phase_detection::PhaseDetection::new(
            options,
            crate::phase_detection::Wiring {
                routes: wire!(resources, id, routes, IrqRoutes),
            },
        )?,
    )))
}

fn window_analysis(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::WindowAnalysis { options, routes } = model else {
        return Err("constructor called for another model kind".into());
    };
    let options = decode(options)?;
    Ok(Built::Device(Box::new(
        crate::window_analysis::WindowAnalysis::new(
            options,
            crate::window_analysis::Wiring {
                routes: wire!(resources, id, routes, IrqRoutes),
            },
        )?,
    )))
}
