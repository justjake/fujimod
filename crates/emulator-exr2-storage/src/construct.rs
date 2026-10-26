//! EXR II board constructors for this crate's models.
use crate::{nand, sdio};
use fujimod_emulator_ip::sdhci::host as sdhc;
use fujimod_emulator_devices::parts::sd_card;
use fujimod_emulator_exr2_plan::{
    Card, Model,
    construction::{Builder, Built, Constructors, decode, line},
};
use fujimod_emulator_parts_io::wifi_module::WifiModule;
use fujimod_emulator_storage::fat;

/// Add this crate's EXR II constructors.
pub fn register(constructors: &mut Constructors) {
    constructors.add("nand", nand);
    constructors.add("sd-host", sd_host);
    constructors.add("sdio-host", sdio_host);
    constructors.add("sdhci-clocks", sdhci_clocks);
}

fn nand(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { resources, .. } = builder;
    let Model::Nand { options, medium } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::nand::Options = decode(options)?;
        Box::new(nand::NandController::new(
            options,
            resources
                .inputs
                .nands
                .remove(&medium)
                .ok_or_else(|| format!("missing NAND input {medium}"))?,
        )?)
    }))
}

fn sd_host(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        name,
        resources,
        built,
        ..
    } = builder;
    let Model::SdHost {
        options,
        card,
        pins,
        present,
        protected,
        irq,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: sdhc::Options = decode(options)?;
        let card = match card {
            Card::Blank { options } => {
                let options: fujimod_emulator_devices::parts::sd_card::Options = decode(options)?;
                sd_card::Card::new(options)?
            }
            Card::Formatted { options, files } => {
                let options: fujimod_emulator_storage::fat::Options = decode(options)?;
                fat::format(
                    &options,
                    resources
                        .inputs
                        .files
                        .get(&files)
                        .ok_or_else(|| format!("missing card files input {files}"))?,
                )?
            }
            Card::Retained { input } => resources
                .inputs
                .cards
                .remove(&input)
                .ok_or_else(|| format!("missing retained medium {input}"))?,
        };
        if let Some(pins) = pins {
            pins.detect.validate()?;
            pins.protect.validate()?;
            if pins.detect == pins.protect {
                return Err("SD detect and protect must use distinct pads".into());
            }
            let pads = resources.pads(&pins.gpio)?;
            if pads.input_level(pins.detect)? != (present == pins.present_high)
                || pads.input_level(pins.protect)? != (protected == pins.protected_high)
            {
                return Err("SD contact reset levels differ from the connected GPIO pads".into());
            }
            resources.card_pins.insert(
                name.to_owned(),
                fujimod_emulator_exr2_resources::CardPins {
                    gpio: *built
                        .node_ids
                        .get(&pins.gpio)
                        .ok_or("SD GPIO has not been constructed")?,
                    pads,
                    detect: pins.detect,
                    present_high: pins.present_high,
                    protect: pins.protect,
                    protected_high: pins.protected_high,
                },
            );
        }
        let contacts =
            fujimod_emulator_devices::parts::card_contacts::Contacts::new(present, protected);
        let host = sdhc::SdHost::new(
            options,
            sdhc::Wiring {
                card,
                contacts: contacts.clone(),
                irq: line(irq)?,
            },
        )?;
        built.contacts.insert(name.to_owned(), contacts);
        Box::new(host)
    }))
}

fn sdio_host(_builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Model::SdioHost {
        options,
        module,
        irq,
    } = model
    else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        let options: crate::sdio::Options = decode(options)?;
        let module: fujimod_emulator_parts_io::wifi_module::Options = decode(module)?;
        Box::new(sdio::SdioHost::new(
            options,
            sdio::Wiring {
                module: WifiModule::new(module)?,
                irq: line(irq)?,
            },
        )?)
    }))
}

fn sdhci_clocks(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder { windows, .. } = builder;
    let Model::SdhciClocks { words } = model else {
        return Err("constructor called for another model kind".into());
    };
    Ok(Built::Device({
        if words.len() != windows.len()
            || words.iter().any(|&(address, _)| {
                !matches!(address, 0xfff5002c | 0xfff5102c)
                    || !windows.iter().any(|w| w.base == address && w.size == 2)
            })
        {
            return Err("SD clock-only nodes require their physical clock words".into());
        }
        Box::new(fujimod_emulator_ip::sdhci::Clocks::new(&words)?)
    }))
}
