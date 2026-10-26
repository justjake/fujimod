//! EXR II board constructors for this crate's models.
use crate::{boot, core_release, dram, reset, timer};
use fujimod_emulator_exr2_plan::{Model, construction::{Builder, Built, Constructors, decode}};
use fujimod_emulator_exr2_resources::wire;

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("dram", dram);
    constructors.add("dram-phy", dram_phy);
    constructors.add("reset", reset);
    constructors.add("exr2-system-timers", timers);
    constructors.add("core-release", core_release);
    constructors.add("block-98000", block98000);
}

fn dram(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::Dram { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::dram::Options = decode(options)?;
        Box::new(dram::Controller::new(options, dram::Wiring)?)
    }))
}

fn dram_phy(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::DramPhy { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::dram::Options = decode(options)?;
        Box::new(dram::Phy::new(options, dram::Wiring)?)
    }))
}

fn reset(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::Reset { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::reset::Options = decode(options)?;
        Box::new(reset::Reset::new(options, reset::Wiring)?)
    }))
}

fn timers(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        built, start_tick, ..
    } = builder;
    let Model::Timers {
        options,
        controllers,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::timer::Options = decode(options)?;
        let controllers =
            fujimod_emulator_exr2_plan::construction::controllers(built, controllers)?;
        Box::new(timer::Timers::new(
            options,
            timer::Wiring {
                controllers,
                start_tick,
            },
        )?)
    }))
}

fn core_release(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::CoreRelease {
        options,
        core,
        request,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::core_release::Options = decode(options)?;
        Box::new(core_release::CoreRelease::new(
            options,
            core_release::Wiring {
                core,
                request: wire!(resources, id, request, SoftwarePulse),
            },
        )?)
    }))
}

fn block98000(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::Block98000 { options, wiring } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::boot::Options = decode(options)?;
        let wiring: crate::boot::Wiring = decode(wiring)?;
        Box::new(boot::Block98000::new(options, wiring)?)
    }))
}
