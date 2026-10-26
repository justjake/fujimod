//! EXR II board constructors for this crate's models.
use fujimod_emulator_exr2_plan::{Model, construction::{Builder, Built, Constructors, controllers, decode}};
use fujimod_emulator_exr2_resources::wire;

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("i2c", i2c);
}

fn i2c(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        id,
        windows,
        resources,
        built,
        ..
    } = builder;
    let Model::I2c {
        options,
        controllers: names,
        cable,
        bridge,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::Options = decode(options)?;
        if windows.len() != 1
            || windows[0].base != u64::from(options.base)
            || windows[0].size != 0x40
        {
            return Err("I2C requires its full physical channel window".into());
        }
        let cable = cable
            .map(|name| Ok(wire!(resources, id, name, LogicLevel)))
            .transpose()?;
        let bridge = bridge
            .map(|name| Ok(wire!(resources, id, name, BridgeRegisters)))
            .transpose()?;
        Box::new(crate::I2cController::new(
            options,
            crate::Wiring {
                controllers: controllers(built, names)?,
                cable,
                bridge,
            },
        )?)
    }))
}
