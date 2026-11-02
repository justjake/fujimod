use fujimod_catalog::ReferenceCatalog;

#[test]
fn card_files_are_optional_and_follow_the_firmware_selection() {
    let catalog = ReferenceCatalog::bundled().unwrap();
    for id in ["x70-110", "x100v-301", "xpro3-200"] {
        let profile = catalog.card_files(id).unwrap();
        profile.validate().unwrap();
        assert_eq!(profile.firmware_id, id);
        assert!(catalog.card_profiles().any(|p| p.firmware_id == id));
    }
    assert!(catalog.card_files("unreported-100").is_none());
}
