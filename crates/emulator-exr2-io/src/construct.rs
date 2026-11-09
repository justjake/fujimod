//! EXR II board constructors for this crate's models.
use crate::{adc, hdmi, usb};
use fujimod_emulator_exr2_plan::{Model, construction::{Builder, Built, Constructors, decode, line}};
use fujimod_emulator_exr2_resources::wire;
use fujimod_emulator_parts_controls::{
    encoder::Encoder, gpio_contact_response::GpioContactResponse,
};
use fujimod_emulator_parts_io::usb_endpoint;

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("usb", usb);
    constructors.add("adc", adc);
    constructors.add("hdmi", hdmi);
    constructors.add("usb-endpoints", usb_endpoints);
    constructors.add("encoder", encoder);
    constructors.add("gpio-contact-response", gpio_contact_response);
    constructors.add("contact-actuator", contact_actuator);
}

fn usb(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        id,
        windows,
        resources,
        ..
    } = builder;
    let Model::Usb {
        options,
        cable,
        endpoints,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::usb::Options = decode(options)?;
        let endpoints: Option<fujimod_emulator_parts_io::usb_endpoint::Wiring> = decode(endpoints)?;
        if windows.len() != 2
            || windows[0].base != 0xfff54000
            || windows[0].size != 0x1000
            || windows[1].base != 0xfff55000
            || windows[1].size != 0x1000
        {
            return Err("USB requires its physical core and PHY windows".into());
        }
        if let Some(wiring) = &endpoints {
            line(wiring.irq)?;
            line(wiring.dma_irq)?;
            if wiring.irq == wiring.dma_irq {
                return Err("USB core and DMA IRQ sources must differ".into());
            }
        }
        Box::new(usb::UsbController::new(
            options,
            usb::Wiring {
                cable: wire!(resources, id, cable, LogicLevel),
                endpoints,
            },
        )?)
    }))
}

fn adc(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { name, resources, .. } = builder;
    let Model::Adc { options } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::adc::controller::Options = decode(options)?;
        let adc = adc::controller::Converter::new(options)?;
        resources.analog.insert(name.to_owned(), adc.external_inputs());
        Box::new(adc)
    }))
}

fn hdmi(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { resources, .. } = builder;
    let Model::Hdmi {
        options,
        sink,
        hot_plug,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::hdmi::transmitter::Options = decode(options)?;
        Box::new(hdmi::Transmitter::new(
            options,
            resources.hdmi_sink(&sink)?,
            hdmi::transmitter::Wiring {
                hot_plug: line(hot_plug)?,
            },
        )?)
    }))
}

fn usb_endpoints(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::UsbEndpoints {
        options,
        irq,
        dma_irq,
        ram_bytes,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: fujimod_emulator_parts_io::usb_endpoint::EndpointOptions = decode(options)?;
        Box::new(usb_endpoint::Endpoint::new(
            options,
            usb_endpoint::Wiring {
                irq: line(irq)?,
                dma_irq: line(dma_irq)?,
                ram_bytes,
            },
        )?)
    }))
}

fn encoder(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        name, resources, ..
    } = builder;
    let Model::Encoder {
        options,
        gpio,
        adc,
        counter,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: fujimod_emulator_parts_controls::encoder::Options = decode(options)?;
        use fujimod_emulator_parts_controls::encoder::{OutputWiring, Outputs};
        let outputs = match &options.outputs {
            Outputs::Gpio => {
                if adc.is_some() {
                    return Err("GPIO encoder must not declare an ADC connection".into());
                }
                OutputWiring::Gpio(Box::new(resources.pads(
                    gpio.as_deref().ok_or("GPIO encoder requires a GPIO connection")?,
                )?))
            }
            Outputs::Adc { .. } => {
                if gpio.is_some() {
                    return Err("ADC encoder must not declare a GPIO connection".into());
                }
                let node = adc.as_deref().ok_or("ADC encoder requires an ADC connection")?;
                OutputWiring::Adc(
                    resources.analog.get(node)
                        .ok_or_else(|| format!("ADC input {node} is not connected"))?
                        .clone(),
                )
            }
        };
        let encoder = Encoder::new(
            options,
            fujimod_emulator_parts_controls::encoder::Wiring {
                outputs,
                counter: counter.map(|n| resources.pulses(&n)).transpose()?,
            },
        )?;
        resources.encoders.insert(name.to_owned(), encoder.input());
        Box::new(encoder)
    }))
}

fn gpio_contact_response(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { resources, .. } = builder;
    let Model::GpioContactResponse { options, gpio } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: fujimod_emulator_parts_controls::gpio_contact_response::Options =
            decode(options)?;
        Box::new(GpioContactResponse::new(
            options,
            fujimod_emulator_parts_controls::gpio_contact_response::Wiring {
                pins: Box::new(resources.pads(&gpio)?),
            },
        )?)
    }))
}

fn contact_actuator(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        windows, resources, ..
    } = builder;
    let Model::ContactActuator { options, gpio } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: fujimod_emulator_parts_controls::contact_actuator::Options = decode(options)?;
        if !windows.is_empty() || options.contacts.iter().any(|pin| pin.port >= 16) {
            return Err(
                "contact actuator requires GPIO input pads and no register aperture".into(),
            );
        }
        Box::new(
            fujimod_emulator_parts_controls::contact_actuator::ContactActuator::new(
                options,
                fujimod_emulator_parts_controls::contact_actuator::Wiring {
                    pins: Box::new(resources.pads(&gpio)?),
                },
            )?,
        )
    }))
}
