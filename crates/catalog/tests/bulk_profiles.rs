use fujimod_catalog::ReferenceCatalog;

#[test]
fn located_bulk_profiles_validate_with_only_xe4_resolved() {
    let catalog = ReferenceCatalog::bundled().unwrap();
    for (id, product, internal) in [
        ("xe4-201", "X-E4", "1.43"),
        ("xt30ii-204", "X-T30 II", "1.46"),
        ("gfx100rf-113", "GFX100RF", "1.57"),
    ] {
        let profile = catalog.bulk(id).unwrap();
        profile.validate().unwrap();
        assert_eq!(
            (&*profile.product, &*profile.internal_version),
            (product, internal)
        );
        assert!(catalog.bulk_profiles().any(|item| item.firmware_id == id));
    }

    let xe4 = catalog
        .resolve_firmware("xe4-201")
        .expect("the registered X-E4 firmware and body should resolve");
    assert_eq!(xe4.firmware.id, "xe4-201");
    assert_eq!(xe4.body.id, "xe4");
    assert_eq!(xe4.soc.id, "xp4");
    assert!(xe4.execution.is_none());
    assert!(catalog.execution("xe4-201").is_none());

    assert!(catalog.body("xt30ii").is_none());
    assert!(catalog.firmware("xt30ii-204").is_none());
    assert!(catalog.execution("xt30ii-204").is_none());
    assert!(catalog.resolve_firmware("xt30ii-204").is_err());
    // The GFX100RF body is registered with firmware 1.12 and 1.13.
    assert!(catalog.body("gfx100rf").is_some());
    for id in ["gfx100rf-112", "gfx100rf-113"] {
        assert_eq!(catalog.resolve_firmware(id).unwrap().body.id, "gfx100rf");
    }

    assert!(catalog.bulk("unreported-100").is_none());
}
