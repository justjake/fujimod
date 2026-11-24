use super::validate_id;

#[test]
fn bundles_the_xp4_boot_rom_nand_record_for_every_body_that_uses_it() {
    let catalog = super::ReferenceCatalog::bundled().unwrap();
    let record = catalog
        .nand_boot_rom("2a8b69986dd3fad0b73808c44e0834fa95c7b4e4f84e6c8664d4b0a82f7b24a4")
        .expect("the XP4 boot ROM NAND record is bundled");
    assert_eq!(record.controller_block_id, "nand-controller");
    assert_eq!(record.source_mode(5).map(|mode| (mode.ecc_mode, mode.address_cycles)), Some((1, 4)));
    assert_eq!(record.source_mode(6).map(|mode| (mode.ecc_mode, mode.address_cycles)), Some((1, 5)));
    assert_eq!(record.source_mode(7).map(|mode| (mode.ecc_mode, mode.address_cycles)), Some((2, 4)));
    assert_eq!(record.source_mode(8).map(|mode| (mode.ecc_mode, mode.address_cycles)), Some((2, 5)));
    assert_eq!((record.header.block, record.header.page), (0, 0));
    assert_eq!(record.program.first_page, 1);
}

#[test]
fn stable_identities_are_bounded_and_not_paths() {
    for identity in ["x70", "x70-110", "exr2.core0", "display_01"] {
        assert!(validate_id(identity).is_ok(), "{identity}");
    }
    for identity in ["", ".", "..", "../x70", "/x70", "x70/1.10", "bad id"] {
        assert!(validate_id(identity).is_err(), "{identity}");
    }
}
