use fujimod_catalog::Firmware;
use fujimod_catalog_run_inputs::HardwareReset;
use std::{fs, path::PathBuf};

#[test]
fn firmware_layouts_and_body_inputs_decode_as_closed_records() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for body in fs::read_dir(root.join("catalog/firmware")).unwrap() {
        for version in fs::read_dir(body.unwrap().path()).unwrap() {
            // Bulk-only versions carry a located bulk profile and no firmware record.
            let path = version.unwrap().path().join("firmware.json");
            if !path.exists() {
                continue;
            }
            let firmware: Firmware = serde_json::from_slice(&fs::read(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            firmware
                .validate(&firmware.id)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        }
    }
}

/// A body's resting reset inputs.
fn resting(body: &str) -> HardwareReset {
    fujimod_catalog::ReferenceCatalog::bundled()
        .unwrap()
        .body(body)
        .and_then(|body| body.hardware.as_ref())
        .and_then(|hardware| hardware.reset_inputs.clone())
        .unwrap_or_else(|| panic!("body {body} declares resting reset inputs"))
}

#[test]
fn xt1_rec_inputs_release_contacts_and_select_idle_positions() {
    let inputs = resting("xt1");
    let reset = inputs.inputs.unwrap();
    let gpio = reset.gpio.gpio_inputs;
    assert_eq!(gpio.get("0"), Some(&3630)); // GPIO0.1 grip absent; GPIO0.9 idle.
    assert_eq!(gpio.get("2"), Some(&4608)); // GPIO2.8 remains low.
    assert_eq!(gpio.get("3"), Some(&845)); // PLAY and DELETE released.
    assert_eq!(gpio.get("8"), Some(&24584)); // AF-LOCK released; strap retained.
    assert_eq!(gpio.get("12"), Some(&160)); // Idle input GPIO12.5 high.
    assert_eq!(reset.mcu_packet_hex, "00000000d00700ff008000040000aafc");
}

#[test]
fn xt30_boot_inputs_declare_the_usb_port_and_its_port_detection() {
    let inputs = resting("xt30");
    let cable = inputs.usb_cable.expect("declared USB cable");
    assert_eq!((cable.vbus_bank, cable.vbus_pin), (15, 15));
    assert_eq!(cable.speed, "high");
    let phy = inputs.usb_phy_input.expect("declared USB PHY");
    assert_eq!(phy.parallel, 0);
    assert_eq!(phy.registers, vec![0; 32]);
    let power = inputs.spi_inputs.expect("declared power registers");
    let registers = power["1"].power_registers.as_ref().unwrap();
    assert_eq!(registers.inputs.get("21"), Some(&1));
}

#[test]
fn xt30_boot_inputs_declare_the_sd_host_slot() {
    let inputs = resting("xt30");
    let cards = inputs.sd_cards;
    assert_eq!(cards.len(), 1);
    let slot = &cards[0];
    // Host 0, port 0: the only SDHCI window the X-T30 slot API opens.
    assert_eq!(slot.host_base, 0xFFFC_4000);
    assert_eq!(slot.card.sectors, 16_777_216);
    assert!(slot.card.high_speed);
    assert_eq!(slot.card.rca, 1);
    assert_eq!(slot.card.format.filesystem, "fat32");
    assert_eq!(slot.card.format.partition_start, 8192);
    assert_eq!(slot.card.format.sectors_per_cluster, 64);
}

#[test]
fn xt3_boot_wires_the_touch_bus_to_a_declared_byte_i2c_controller() {
    let soc = fujimod_catalog::ReferenceCatalog::bundled()
        .unwrap()
        .resolve("xt3", "xt3-511")
        .unwrap()
        .soc;
    let inputs = resting("xt3");
    let wired = inputs.byte_i2c_inputs.expect("declared byte I2C buses");
    assert_eq!(wired.keys().collect::<Vec<_>>(), ["2"]);
    let bus = soc
        .blocks
        .iter()
        .find(|block| block.id == "byte-i2c-2")
        .expect("declared byte I2C controller 2");
    let line = bus
        .interrupts
        .iter()
        .find(|row| row.output == "interrupt")
        .unwrap()
        .line;
    assert_eq!((bus.base, line), (0xfff3_f000, 0x43));
    assert!(matches!(
        bus.device,
        Some(fujimod_catalog::soc::BlockDevice::ByteI2c { byte_ticks, .. }) if byte_ticks > 0
    ));
}

#[test]
fn xt3_discovery_inputs_release_the_x_t3_buttons() {
    let catalog = fujimod_catalog::ReferenceCatalog::bundled().unwrap();
    let body = catalog.body("xt3").expect("declared X-T3 body");
    let inputs = resting("xt3");
    let levels = inputs.gpio_input.expect("declared GPIO levels").levels;
    for id in [
        "xt3-shutter-s1",
        "xt3-shutter-s2",
        "xt3-rear-dial-press",
        "xt3-fn1-button",
        "xt3-fn2-button",
    ] {
        let connection = body
            .connections
            .iter()
            .find(|connection| connection.id == id)
            .unwrap_or_else(|| panic!("declared {id}"));
        let (port, pin) = connection
            .signals
            .iter()
            .find_map(|signal| match *signal {
                fujimod_catalog::reference::ConnectionSignal::Gpio {
                    port,
                    pin,
                    active_level: false,
                } => Some((port, pin)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{id} has an active-low GPIO contact"));
        let level = levels.get(usize::from(port)).copied().unwrap_or(0);
        assert_ne!(level >> pin & 1, 0, "{id} reads pressed at reset");
    }
}
