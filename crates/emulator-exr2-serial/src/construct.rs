//! EXR II board constructors for this crate's models.
use crate::{dma, spi};
use fujimod_emulator_devices::Device;
use fujimod_emulator_exr2_plan::{Model, SerialEndpoint, construction::{Builder, Built, Constructors, controllers, decode, line}};
use fujimod_emulator_exr2_resources::wire;
use fujimod_emulator_parts_lens::{
    lens::{LensUnit, bridge::Bridge},
    stabilizer::Stabilizer,
};

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("memory-dma", memory_dma);
    constructors.add("x-mount-lens", x_mount_lens);
    constructors.add("serial-dma", serial_dma);
    constructors.add("serial-channel", serial_channel);
    constructors.add("spi-controller", spi_controller);
    constructors.add("i2c-motor-bridge", i2c_motor_bridge);
    constructors.add("serial-dma-link", serial_dma_link);
    constructors.set_dma_controller(dma_controller);
}

fn memory_dma(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        id,
        windows,
        resources,
        ..
    } = builder;
    let Model::MemoryDma {
        options,
        base,
        routes,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::dma::memory::Options = decode(options)?;
        if base.checked_add(0x304).is_none()
            || windows.len() != 2
            || windows[0].base != base
            || windows[0].size != 0x40
            || windows[1].base != base + 0x300
            || windows[1].size != 4
        {
            return Err("memory DMA requires channel 0 and its global word".into());
        }
        Box::new(dma::memory::MemoryDma::new(
            options,
            dma::memory::Wiring {
                base,
                routes: wire!(resources, id, routes, IrqRoutes),
            },
        )?)
    }))
}

fn x_mount_lens(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        windows,
        resources,
        built,
        ..
    } = builder;
    let Model::XMountLens {
        transport,
        configuration,
        startup_ticks,
        pins,
        gpio,
        controllers: names,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let transport: crate::serial_dma::Options = decode(transport)?;
        let configuration: fujimod_emulator_parts_lens::x_mount_link::Configuration =
            decode(configuration)?;
        let pins: crate::x_mount_connector::Options = decode(pins)?;
        let device = crate::x_mount_connector::XMountLens::new(
            transport,
            configuration,
            startup_ticks,
            pins,
            resources.pads(&gpio)?,
            controllers(built, names)?,
        )?;
        let expected: std::collections::BTreeSet<_> =
            device.windows().iter().map(|&(a, b)| (a, b - a)).collect();
        if expected != windows.iter().map(|w| (w.base, w.size)).collect() {
            return Err("X-mount lens requires exactly its serial/DMA register windows".into());
        }
        Box::new(device)
    }))
}

fn serial_dma(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { windows, .. } = builder;
    let Model::SerialDma { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::dma::audio::Options = decode(options)?;
        if windows.len() != 1 || windows[0].base != dma::audio::BASE || windows[0].size != 0x40 {
            return Err("serial DMA requires its channel register window".into());
        }
        Box::new(dma::audio::SerialDma::new(options)?)
    }))
}

fn serial_channel(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::SerialChannel {
        options,
        gpio,
        endpoint,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::spi::channel::Options = decode(options)?;
        let endpoint = spi_endpoint(endpoint, resources, id)?;
        Box::new(spi::channel::SerialChannel::new(
            options,
            spi::channel::Wiring {
                pins: Box::new(resources.pads(&gpio)?),
                endpoint,
            },
        )?)
    }))
}

fn spi_controller(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::SpiController { routes } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let channels = routes
            .into_iter()
            .map(|route| {
                let endpoint = spi_endpoint(route.endpoint, resources, id)?;
                spi::channel::SerialChannel::new(
                    decode(route.options)?,
                    spi::channel::Wiring {
                        pins: Box::new(resources.pads(&route.gpio)?),
                        endpoint,
                    },
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
        Box::new(spi::SpiController::new(channels)?)
    }))
}

fn i2c_motor_bridge(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { id, resources, .. } = builder;
    let Model::I2cMotorBridge {
        options,
        registers,
        pulses,
        gpio,
        clock_irq,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: fujimod_emulator_parts_lens::lens::bridge::Options = decode(options)?;
        Box::new(Bridge::new(
            options,
            fujimod_emulator_parts_lens::lens::bridge::Wiring {
                registers: wire!(resources, id, registers, BridgeRegisters),
                pulses: resources.pulses(&pulses)?,
                gpio: resources.pads(&gpio)?,
                clock_irq: line(clock_irq)?,
            },
        )?)
    }))
}

fn serial_dma_link(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::SerialDmaLink { options, input } = model else {
        return Err("constructor called for another model kind".into());
    };
    let options: crate::serial_dma::Options = decode(options)?;
    let input: crate::serial_dma::Input = decode(input)?;
    let devices = crate::serial_dma::Nodes::new(options, input)?;
    let expected: std::collections::BTreeSet<_> = devices
        .windows()
        .iter()
        .map(|&(begin, end)| (begin, end - begin))
        .collect();
    let declared = builder
        .windows
        .iter()
        .map(|window| (window.base, window.size))
        .collect();
    if expected != declared {
        return Err("serial/DMA nodes require exactly their wired register windows".into());
    }
    let devices = devices
        .devices()
        .into_iter()
        .map(|(mappings, device)| (Some(mappings), device))
        .collect::<Vec<_>>();
    assert_eq!(devices.len(), 3);
    // Serial, request selector and DMA wake one another.
    Ok(Built::Nodes {
        devices,
        public: 0,
        edges: vec![(2, 0), (0, 2), (2, 1)],
    })
}

fn dma_controller(
    channels: Vec<(
        Vec<fujimod_emulator_devices::graph::Mapping>,
        Box<dyn Device>,
    )>,
) -> Result<Box<dyn Device>, String> {
    Ok(Box::new(crate::dma::controller::Controller::new(
        channels
            .into_iter()
            .map(|(mappings, device)| crate::dma::controller::ChannelMode { mappings, device })
            .collect(),
    )?))
}

fn spi_endpoint(
    endpoint: SerialEndpoint,
    resources: &mut fujimod_emulator_exr2_resources::Resources,
    id: usize,
) -> Result<Box<dyn fujimod_emulator_parts_io::spi::Endpoint>, String> {
    use fujimod_emulator_parts_io::spi::{FixedInput, Panel, RegisterFile};
    Ok(match endpoint {
        SerialEndpoint::LensUnit {
            options,
            pulses,
            gpio,
            completion,
            position,
            optics,
            focus,
        } => {
            let options: Box<fujimod_emulator_parts_lens::lens::Options> = decode(options)?;
            let mut lens = LensUnit::new(
                *options,
                fujimod_emulator_parts_lens::lens::Wiring {
                    pulses: resources.pulses(&pulses)?,
                    gpio: Box::new(resources.pads(&gpio)?),
                    completion: [line(completion[0])?, line(completion[1])?],
                    position: position
                        .map(|n| resources.optical_position(&n))
                        .transpose()?,
                },
            )?;
            if let Some(optics) = optics {
                lens.attach_aperture(
                    wire!(resources, id, optics.output, Aperture),
                    optics.positions,
                )?;
            }
            if let Some(focus) = focus {
                lens.attach_focus(
                    wire!(resources, id, focus.output, Focus),
                    focus.infinity_position,
                    focus.steps_per_mm,
                )?;
            }
            Box::new(lens)
        }
        SerialEndpoint::Stabilizer { options } => {
            let options: fujimod_emulator_parts_lens::stabilizer::Options = decode(options)?;
            Box::new(Stabilizer::new(options, ())?)
        }
        SerialEndpoint::Registers { options } => {
            let options: fujimod_emulator_parts_io::spi::RegisterOptions = decode(options)?;
            Box::new(RegisterFile::new(options)?)
        }
        SerialEndpoint::Panel { options } => {
            let options: fujimod_emulator_parts_io::spi::PanelOptions = decode(options)?;
            Box::new(Panel::new(options))
        }
        SerialEndpoint::CommandDataOutput { options } => {
            let options: fujimod_emulator_parts_io::command_data_output::Options = decode(options)?;
            Box::new(
                fujimod_emulator_parts_io::command_data_output::CommandDataOutput::new(options)?,
            )
        }
        SerialEndpoint::Fixed { options } => {
            let options: fujimod_emulator_parts_io::spi::FixedOptions = decode(options)?;
            Box::new(FixedInput::new(options)?)
        }
    })
}
