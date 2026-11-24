use fujimod_catalog::Firmware;
use fujimod_catalog_run_inputs::HardwareInputs;
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
    for entry in fs::read_dir(root.join("configurations/emulator")).unwrap() {
        let path = entry.unwrap().path().join("hardware-inputs.json");
        if path.exists() {
            let inputs: HardwareInputs = serde_json::from_slice(&fs::read(&path).unwrap())
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert_eq!(inputs.schema, 1);
            assert_eq!(
                inputs.schema_uri,
                fujimod_catalog_run_inputs::HARDWARE_INPUTS_SCHEMA_URI
            );
        }
    }
}

#[test]
fn xt1_rec_inputs_release_contacts_and_select_idle_positions() {
    let inputs = HardwareInputs::bundled("xt1-rec").unwrap();
    let reset = inputs.reset.inputs.unwrap();
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
    let inputs = HardwareInputs::bundled("xt30-201-boot").unwrap();
    let cable = inputs.reset.usb_cable.expect("declared USB cable");
    assert_eq!((cable.vbus_bank, cable.vbus_pin), (15, 15));
    assert_eq!(cable.speed, "high");
    let phy = inputs.reset.usb_phy_input.expect("declared USB PHY");
    assert_eq!(phy.parallel, 0);
    assert_eq!(phy.registers, vec![0; 32]);
    let power = inputs.reset.spi_inputs.expect("declared power registers");
    let registers = power["1"].power_registers.as_ref().unwrap();
    assert_eq!(registers.inputs.get("21"), Some(&1));
}

#[test]
fn xt30_boot_inputs_declare_the_sd_host_slot() {
    let inputs = HardwareInputs::bundled("xt30-201-boot").unwrap();
    let cards = inputs.reset.sd_cards;
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
    let inputs = HardwareInputs::bundled("xt3-discovery").unwrap();
    let wired = inputs
        .reset
        .byte_i2c_inputs
        .expect("declared byte I2C buses");
    assert_eq!(wired.keys().collect::<Vec<_>>(), ["2"]);
    let bus = soc
        .blocks
        .iter()
        .find(|block| block.id == "byte-i2c-2")
        .expect("declared byte I2C controller 2");
    let line = bus.interrupts.iter().find(|row| row.output == "interrupt").unwrap().line;
    assert_eq!((bus.base, line), (0xfff3_f000, 0x43));
    assert!(matches!(
        bus.device,
        Some(fujimod_catalog::soc::BlockDevice::ByteI2c { byte_ticks, .. }) if byte_ticks > 0
    ));
}
