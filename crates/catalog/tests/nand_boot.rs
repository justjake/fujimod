use fujimod_catalog::ReferenceCatalog;

#[test]
fn located_nand_boot_facts_resolve_without_a_body_or_execution_record() {
    let catalog = ReferenceCatalog::bundled().expect("bundled catalog loads");
    let facts = catalog.nand_boot("x70-110").expect("X70 NAND-boot facts");
    assert_eq!(facts.stage1.blocks.len(), 2);
    assert_eq!(facts.stage1.entry.literal, 0x1800);
    assert_eq!(facts.code_space.address, 0x18000);
    assert_eq!(facts.rom.routines.mount, 0x80001ba0);
    assert_eq!(facts.rom.command_table.bytes, 80);
    assert_eq!(facts.load_range.address, 0x1d000000);
    assert_eq!(facts.load_range.capacity, 0x40000);
    assert!(catalog.nand_boot("missing-100").is_none());
}
