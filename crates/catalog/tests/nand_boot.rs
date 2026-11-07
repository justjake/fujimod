use fujimod_catalog::ReferenceCatalog;

const EXR2: [&str; 16] = [
    "x100s-121",
    "x100t-111",
    "x20-103",
    "x30-103",
    "x70-110",
    "xa1-150",
    "xa2-130",
    "xe2-411",
    "xe2s-111",
    "xm1-101",
    "xm1-120",
    "xm1-150",
    "xq1-121",
    "xq2-101",
    "xt1-552",
    "xt10-131",
];

#[test]
fn located_nand_boot_facts_resolve_without_a_body_or_execution_record() {
    let catalog = ReferenceCatalog::bundled().expect("bundled catalog loads");
    let facts = catalog.nand_boot("x70-110").expect("X70 NAND-boot facts");
    assert_eq!(facts.stage1.blocks.len(), 2);
    assert_eq!(facts.stage1.entry.literal, 0x1800);
    assert_eq!(facts.code_space.address, 0x100);
    let rom = catalog
        .boot_rom(&facts.boot_rom)
        .expect("X70 boot ROM facts");
    assert_eq!(rom.routines.mount, 0x80001ba0);
    assert_eq!(rom.command_table.bytes, 80);
    assert_eq!(rom.load_address, facts.stage1.load_address);
    assert_eq!(facts.load_range.address, 0x1d000000);
    assert_eq!(facts.load_range.capacity, 0x40000);
    assert_eq!(facts.load_range.basis, "network-heap");
    assert!(catalog.nand_boot("missing-100").is_none());
    assert!(catalog.boot_rom("0".repeat(64).as_str()).is_none());
}

#[test]
fn every_exr_2_update_names_a_shared_boot_rom() {
    let catalog = ReferenceCatalog::bundled().expect("bundled catalog loads");
    for firmware_id in EXR2 {
        let facts = catalog
            .nand_boot(firmware_id)
            .unwrap_or_else(|| panic!("{firmware_id} NAND-boot facts"));
        let rom = catalog
            .boot_rom(&facts.boot_rom)
            .unwrap_or_else(|| panic!("{firmware_id} boot ROM"));
        assert_eq!(rom.load_address, facts.stage1.load_address, "{firmware_id}");
        assert_eq!(
            facts.stage1.image_bytes,
            facts
                .stage1
                .blocks
                .iter()
                .map(|block| block.bytes)
                .sum::<u64>(),
            "{firmware_id}"
        );
        assert_eq!(
            facts.code_space.address,
            facts.stage1.load_address + 0x100,
            "{firmware_id}"
        );
    }
}
