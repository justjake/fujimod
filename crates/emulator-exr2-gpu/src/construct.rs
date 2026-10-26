//! EXR II board constructors for this crate's models.
use fujimod_emulator_exr2_plan::{
    Model,
    construction::{Builder, Built, Constructors, controllers, decode},
};

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("vector-gpu", vector_gpu);
}

fn vector_gpu(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { windows, built, .. } = builder;
    let Model::VectorGpu {
        options,
        controllers: names,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::Options = decode(options)?;
        if windows.len() != 1 || windows[0].base != crate::BASE || windows[0].size != crate::SIZE {
            return Err("vector engine requires its complete physical register window".into());
        }
        Box::new(crate::VectorGpu::new(
            options,
            crate::Wiring {
                controllers: controllers(built, names)?,
            },
        )?)
    }))
}
