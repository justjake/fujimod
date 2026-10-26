//! EXR II board constructors for this crate's models.
use fujimod_emulator_exr2_plan::{
    Model,
    construction::{Builder, Built, Constructors, decode, route},
};
use fujimod_emulator_exr2_resources::wire;

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("image-combination", image_combination);
    constructors.add("output-channel", output_channel);
    constructors.add("still-processor", still_processor);
    constructors.add("scaler", scaler);
    constructors.add("warp", warp);
    constructors.add("avc-codec", avc_codec);
    constructors.add("image-transfer", image_transfer);
}

fn image_combination(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { windows, .. } = builder;
    let Model::ImageCombination { options, irq } = model else {
        return Err("constructor called for another model kind".into());
    };
    let options = decode(options)?;
    Ok(Built::Device({
        if windows.len() != 1
            || windows[0].base != fujimod_emulator_imaging::exr2_isp::options::combination::BASE
            || windows[0].size != 0x500
        {
            return Err("image combination requires its register window".into());
        }
        Box::new(crate::combination::Combination::new(options, route(irq)?)?)
    }))
}

fn output_channel(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::OutputChannel {
        options,
        base,
        irq,
        channel,
        channels,
        routes,
        frames,
        memory_outputs,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    let options = decode(options)?;
    Ok(Built::Device({
        let memory_outputs = memory_outputs
            .map(|name| {
                Ok(fujimod_emulator_exr2_images::image_wire!(
                    resources,
                    id,
                    name,
                    MemoryOutputs
                ))
            })
            .transpose()?;
        Box::new(crate::output::OutputChannel::new(
            options,
            crate::output::Wiring {
                base,
                irq,
                channel,
                channels: fujimod_emulator_exr2_images::image_wire!(
                    resources,
                    id,
                    channels,
                    OutputChannels
                ),
                routes: wire!(resources, id, routes, IrqRoutes),
                frames: fujimod_emulator_exr2_images::images(resources)?
                    .images
                    .get(&frames)
                    .cloned()
                    .ok_or_else(|| format!("missing image frames {frames}"))?,
                memory_outputs,
            },
        )?)
    }))
}

fn still_processor(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::StillProcessor { options, irq } = model else {
        return Err("constructor called for another model kind".into());
    };
    let options = decode(options)?;
    Ok(Built::Device({
        Box::new(crate::still::StillProcessor::new(options, route(irq)?)?)
    }))
}

fn scaler(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::Scaler { options, irq } = model else {
        return Err("constructor called for another model kind".into());
    };
    let options = decode(options)?;
    Ok(Built::Device(Box::new(crate::scaler::Scaler::new(
        options,
        route(irq)?,
    )?)))
}

fn warp(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::Warp {
        options,
        irq,
        stream,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    let options = decode(options)?;
    Ok(Built::Device(Box::new(crate::warp::Warp::new(
        options,
        (
            route(irq)?,
            fujimod_emulator_exr2_images::image_wire!(resources, id, stream, Stream),
        ),
    )?)))
}

fn avc_codec(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::AvcCodec { options, irq } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        Box::new(crate::avc::AvcCodec::new(
            fujimod_emulator_exr2_plan::construction::decode(options)?,
            route(irq)?,
        )?)
    }))
}

fn image_transfer(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::ImageTransfer { irq } = model else {
        return Err("constructor called for another model kind".into());
    };
    if builder.windows.len() != 1
        || builder.windows[0].base != 0xfff64000
        || builder.windows[0].size != 0x20
    {
        return Err("image transfer requires its register window".into());
    }
    Ok(Built::Device(Box::new(
        crate::transfer::ImageTransfer::new(route(irq)?),
    )))
}
