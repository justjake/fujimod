//! EXR II board constructors for this crate's models.
use crate::{formatter, timing};
use fujimod_emulator_exr2_plan::{
    Model,
    construction::{Builder, Built, Constructors, decode},
};

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("scanout", scanout);
    constructors.add("display-timing", display_timing);
    constructors.add("formatter", formatter);
}

fn scanout(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { windows, .. } = builder;
    let Model::Scanout { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::device::Options = decode(options)?;
        let expected = [0xfff6b000, 0xfff6e000];
        if windows.len() != 2
            || windows
                .iter()
                .zip(expected)
                .any(|(w, b)| w.base != b || w.size != 0x1000)
        {
            return Err("scanout requires both complete physical register windows".into());
        }
        Box::new(crate::Scanout::new(options)?)
    }))
}

fn display_timing(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        resources,
        built,
        start_tick,
        ..
    } = builder;
    let Model::DisplayTiming {
        options,
        clock,
        controllers,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::timing::Options = decode(options)?;
        let clock = resources.clock(&clock)?;
        let controllers =
            fujimod_emulator_exr2_plan::construction::controllers(built, controllers)?;
        Box::new(timing::DisplayTiming::new(
            options,
            timing::Wiring {
                clock,
                controllers,
                start_tick,
            },
        )?)
    }))
}

fn formatter(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::Formatter { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::formatter::Options = decode(options)?;
        Box::new(formatter::Formatter::new(options)?)
    }))
}
