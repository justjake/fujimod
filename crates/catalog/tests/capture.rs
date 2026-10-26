use fujimod_catalog::ReferenceCatalog;

#[test]
fn capture_contracts_resolve_with_the_selected_firmware() {
    let catalog = ReferenceCatalog::bundled().unwrap();
    for (firmware, kind, count) in [
        ("xm5-130", "serial-nand", 6),
        ("x100vi-131", "serial-nor", 7),
    ] {
        let selection = catalog.resolve_firmware(firmware).unwrap();
        let flash = selection.flash.unwrap();
        flash.validate().unwrap();
        assert_eq!(flash.firmware_id, firmware);
        assert_eq!(flash.device.kind, kind);
        assert_eq!(flash.code_ranges.len(), count);
        assert_eq!(
            (flash.window.address, flash.window.size),
            (0xf0000000, 0x8000000)
        );
        let state = selection.state.unwrap();
        state.validate().unwrap();
        assert_eq!(state.firmware_id, firmware);
        let dram = state
            .regions
            .iter()
            .find(|region| region.name == "dram")
            .unwrap();
        assert_eq!(
            (dram.address, dram.size, dram.constant),
            (0x4000, 0x7fffc000, false)
        );
        assert!(
            state
                .regions
                .iter()
                .any(|r| r.name == "dram" && !r.constant)
        );
        assert!(
            state
                .regions
                .iter()
                .any(|r| r.name == "boot-sram" && r.address == 0xff800000)
        );
        assert!(!state.regions.iter().any(|r| r.name == "boot-rom"));
    }
    for firmware in ["x70-110", "xe2-411", "xm1-150"] {
        let state = catalog.resolve_firmware(firmware).unwrap().state.unwrap();
        let rom = state.regions.iter().find(|r| r.name == "boot-rom").unwrap();
        assert_eq!(
            (rom.address, rom.size, rom.constant),
            (0x80000000, 16384, true)
        );
        state.validate().unwrap();
    }
}

#[test]
fn parallel_nand_requires_the_corrected_main_page_contract() {
    let catalog = ReferenceCatalog::bundled().unwrap();
    for firmware in [
        "x70-110",
        "xa1-150",
        "xa2-130",
        "xe2-411",
        "xe2s-111",
        "xm1-150",
        "xt10-131",
        "x100t-111",
        "x30-103",
        "xq1-121",
        "xq2-101",
        "xt1-552",
    ] {
        let flash = catalog.resolve_firmware(firmware).unwrap().flash.unwrap();
        flash.validate().unwrap();
        assert_eq!(flash.device.kind, "raw-nand");
        assert!(flash.device.jedec_id.is_none());
        assert_eq!(flash.transport.as_deref(), Some("exr2-query-heap"));
        assert_eq!((flash.window.address, flash.window.size), (0, 0x8000000));
        let nand = flash.reader.nand.as_ref().unwrap();
        assert_eq!(
            (
                nand.page_bytes,
                nand.pages_per_block,
                nand.controller_mode,
                nand.max_pages_per_call
            ),
            (2048, 64, 0, 1)
        );
        let mut invalid = flash.clone();
        invalid.reader.nand.as_mut().unwrap().controller_mode = 1;
        assert!(invalid.validate().is_err());
        invalid.reader.nand.as_mut().unwrap().controller_mode = 0;
        invalid.reader.nand.as_mut().unwrap().max_pages_per_call = 2;
        assert!(invalid.validate().is_err());
    }
    for firmware in ["x100s-121", "x20-103"] {
        let flash = catalog.resolve_firmware(firmware).unwrap().flash.unwrap();
        flash.validate().unwrap();
        assert_eq!(flash.device.kind, "raw-nand");
        assert_eq!((flash.window.address, flash.window.size), (0, 0x4000000));
        assert_eq!(flash.code_ranges.len(), 16);
        let nand = flash.reader.nand.as_ref().unwrap();
        assert_eq!(
            (
                nand.page_bytes,
                nand.pages_per_block,
                nand.controller_mode,
                nand.max_pages_per_call
            ),
            (512, 32, 0, 1)
        );
        assert_eq!(flash.transport.as_deref(), Some("exr2-query-heap"));
        for (page, pages) in [(512, 64), (2048, 32), (1024, 32)] {
            let mut invalid = flash.clone();
            let nand = invalid.reader.nand.as_mut().unwrap();
            nand.page_bytes = page;
            nand.pages_per_block = pages;
            invalid.reader.alignment = page;
            assert!(invalid.validate().is_err());
        }
    }
}

#[test]
fn bounded_boot_code_capture_is_specific_to_its_firmware() {
    let catalog = ReferenceCatalog::bundled().unwrap();
    let selection = catalog.resolve_firmware("x100v-301").unwrap();
    let identity = selection.firmware.boot_rom.unwrap();
    assert_eq!(
        (identity.address, identity.content.size),
        (0xfff00000, 0x8000)
    );
    assert_eq!(
        identity.content.sha256,
        "2a8b69986dd3fad0b73808c44e0834fa95c7b4e4f84e6c8664d4b0a82f7b24a4"
    );
    let state = selection.state.unwrap();
    let rom = state.regions.iter().find(|r| r.name == "boot-rom").unwrap();
    assert_eq!(
        (rom.address, rom.size, rom.constant),
        (0xfff00000, 0x8000, true)
    );
    state.validate().unwrap();
    for id in ["xt4-212", "xs10-311"] {
        let state = catalog.resolve_firmware(id).unwrap().state.unwrap();
        assert!(!state.regions.iter().any(|r| r.name == "boot-rom"));
    }
}
