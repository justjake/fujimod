//! The EXR II board constructor of the boot serial blocks.
use fujimod_emulator_exr2_plan::{
    Model,
    construction::{Builder, Built, Constructors},
};

/// Add the boot serial blocks' EXR II constructor.
pub fn register(constructors: &mut Constructors) {
    constructors.add("power", power);
}

fn power(builder: Builder<'_>, model: Model) -> Result<Built, String> {
    let Builder {
        name,
        windows,
        resources,
        ..
    } = builder;
    let Model::Power { options, gpio } = model else {
        return Err("constructor called for another model kind".into());
    };
    let physical = [
        (0xfff84000, 0x20),
        (0xfff46080, 0x10),
        (0xfff02080, 0x40),
        (0xfff98500, 8),
    ];
    if windows.len() != physical.len()
        || physical.iter().any(|&(base, size)| {
            !windows
                .iter()
                .any(|window| window.base == base && window.size == size)
        })
    {
        return Err(format!(
            "{name} requires its physical SPI0/UART/DMA/request-selector windows"
        ));
    }
    let devices = crate::BootSerialBlocks::new(
        options,
        crate::Wiring {
            gpio: resources.pads(&gpio)?,
        },
    )?
    .devices();
    assert_eq!(devices.len(), 5);
    Ok(Built::Nodes {
        devices: devices
            .into_iter()
            .map(|(mapping, device)| (mapping.map(|mapping| vec![mapping]), device))
            .collect(),
        public: 0,
        // Every physical register block can start or cancel a serial
        // deadline. Wake the shared coordinator when any is accessed.
        edges: (0..4).map(|block| (4, block)).collect(),
    })
}
