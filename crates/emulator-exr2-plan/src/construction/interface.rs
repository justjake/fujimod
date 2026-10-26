//! Constructors of the EXR II devices the interface crates still hold.
use super::{Builder, Built, Constructors, decode, line};
use crate::Model;
use fujimod_emulator_exr2_resources::CoreInterrupts;
use fujimod_emulator_devices::exr2::{clock, gpio, irq, pulse};

/// Add the constructors of the devices the interface crates hold.
pub fn register_interface_models(constructors: &mut Constructors) {
    constructors.add("clocks", clocks);
    constructors.add("exr2-interrupt-controller", interrupt_controller);
    constructors.add("gpio", gpio);
    constructors.add("exr2-pulse", pulse_unit);
    constructors.add("block86000", block86000);
    constructors.add("hdmi-sink", hdmi_sink);
    constructors.add("exr-system-control", exr_system_control);
    constructors.add("exr-clocks", exr_clocks);
    constructors.add("exr-memory-interface", exr_memory_interface);
}

fn exr_memory_interface(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    if !matches!(model, Model::ExrMemoryInterface) {
        return Err("constructor called for another model kind".into());
    }
    Ok(Built::Device(Box::new(
        fujimod_emulator_devices::exr::MemoryInterface,
    )))
}

fn exr_clocks(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::ExrClocks { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device(Box::new(
        fujimod_emulator_devices::exr::clocks::Clocks::new(decode(options)?)?,
    )))
}

fn exr_system_control(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::ExrSystemControl { options, wiring } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device(Box::new(
        fujimod_emulator_devices::exr::SystemControl::new(decode(options)?, decode(wiring)?)?,
    )))
}

fn clocks(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        name, resources, ..
    } = builder;
    let Model::Clocks { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let clock = clock::Clocks::new(options, ())?;
        resources.clocks.insert(name.into(), clock.output());
        Box::new(clock)
    }))
}

fn interrupt_controller(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        name,
        id,
        resources,
        built,
        ..
    } = builder;
    let Model::InterruptController {
        options,
        core,
        base,
        routes,
        software,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        if core >= 2 || built.core_interrupts.iter().any(|c| c.core == core) {
            return Err("invalid or duplicate EXR II core interrupt controller".into());
        }
        let enables = routes
            .map(|name| {
                let routes = fujimod_emulator_exr2_resources::wire!(resources, id, name, IrqRoutes);
                Ok(Box::new(fujimod_emulator_devices::exr2::irq_routes::RouteOutput::new(routes, core)?)
                    as Box<dyn irq::EnableOutput>)
            })
            .transpose()?;
        let inputs = irq::Inputs::default();
        resources.controllers.insert(name.into(), inputs.clone());
        let controller = irq::InterruptController::new(
            options,
            irq::Wiring {
                base,
                inputs: inputs.clone(),
                enables,
                software: software
                    .into_iter()
                    .map(|output| {
                        if !resources.software_sources.insert(output.pulse.clone()) {
                            return Err("software pulse requires one controller output".into());
                        }
                        Ok(irq::SoftwareOutput {
                            line: output.line,
                            pulse: fujimod_emulator_exr2_resources::wire!(resources, id, output.pulse, SoftwarePulse),
                        })
                    })
                    .collect::<Result<_, String>>()?,
            },
        )?;
        built.core_interrupts.push(CoreInterrupts {
            core,
            device: id,
            inputs,
        });
        Box::new(controller)
    }))
}

fn gpio(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        name, resources, ..
    } = builder;
    let Model::Gpio { options, irqs } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        for route in &irqs {
            for &output in &route.outputs {
                line(output)?;
            }
        }
        let gpio = gpio::Gpio::new(options, gpio::Wiring { irqs })?;
        resources.gpio.insert(name.to_owned(), gpio.pads());
        Box::new(gpio)
    }))
}

fn pulse_unit(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        name, resources, ..
    } = builder;
    let Model::PulseUnit { options, irqs } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        for irq in irqs.into_iter().flatten() {
            line(irq)?;
        }
        let pulse = pulse::PulseUnit::new(options, pulse::Wiring { irqs })?;
        resources.pulses.insert(name.to_owned(), pulse.pins());
        Box::new(pulse)
    }))
}

fn block86000(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::Block86000 { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device(Box::new(pulse::block86000::Block86000::new(
        options,
    ))))
}

fn hdmi_sink(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        name, resources, ..
    } = builder;
    let Model::HdmiSink { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: fujimod_emulator_devices::parts::hdmi_sink::Options = decode(options)?;
        let input = fujimod_emulator_devices::parts::hdmi_sink::Input::new(options)?;
        resources.hdmi_sinks.insert(name.to_owned(), input.sink());
        Box::new(input)
    }))
}
