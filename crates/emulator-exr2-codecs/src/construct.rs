//! EXR II board constructors for this crate's models.
use crate::{jpeg, lz};
use fujimod_emulator_exr2_plan::{Model, construction::{Builder, Built, Constructors, controllers, decode, route}};

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("lz", lz);
    constructors.add("jpeg-encoder", jpeg_encoder);
    constructors.add("jpeg-decoder", jpeg_decoder);
}

fn lz(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { windows, built, .. } = builder;
    let Model::Lz {
        options,
        base,
        controllers: names,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::lz::Options = decode(options)?;
        if windows.len() != 1 || windows[0].base != base || windows[0].size != 0x100 {
            return Err("decompressor requires its complete register window".into());
        }
        Box::new(lz::Decompressor::new(
            options,
            lz::Wiring {
                base,
                controllers: controllers(built, names)?,
            },
        )?)
    }))
}

fn jpeg_encoder(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::JpegEncoder {
        options,
        irq,
        stream,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::jpeg::encoder::Options = decode(options)?;
        Box::new(jpeg::Encoder::new(
            options,
            (route(irq)?, fujimod_emulator_exr2_images::image_wire!(resources, id, stream, Stream)),
        )?)
    }))
}

fn jpeg_decoder(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::JpegDecoder { options, irq } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::jpeg::decoder::Options = decode(options)?;
        Box::new(jpeg::Decoder::new(options, route(irq)?)?)
    }))
}
