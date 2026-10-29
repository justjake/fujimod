use fujimod_catalog::ReferenceCatalog;

#[test]
fn located_bulk_profiles_resolve_without_body_or_execution_records() {
    let catalog = ReferenceCatalog::bundled().unwrap();
    for (id, product, internal) in [
        ("xe4-201", "X-E4", "1.43"),
        ("xpro3-200", "X-Pro3", "1.72"),
        ("xt30-201", "X-T30", "1.64"),
        ("xt30ii-204", "X-T30 II", "1.46"),
    ] {
        let profile = catalog.bulk(id).unwrap();
        profile.validate().unwrap();
        assert_eq!(
            (&*profile.product, &*profile.internal_version),
            (product, internal)
        );
        assert!(catalog.resolve_firmware(id).is_err());
        assert!(catalog.bulk_profiles().any(|item| item.firmware_id == id));
    }
    assert!(catalog.bulk("unreported-100").is_none());
}
