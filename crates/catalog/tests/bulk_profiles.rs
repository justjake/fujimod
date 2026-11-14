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

    for (id, body_id) in [("xt30ii-204", "xt30ii"), ("gfx100rf-113", "gfx100rf")] {
        assert!(catalog.body(body_id).is_none());
        assert!(catalog.firmware(id).is_none());
        assert!(catalog.execution(id).is_none());
        assert!(catalog.resolve_firmware(id).is_err());
    }

    assert!(catalog.bulk("unreported-100").is_none());
}
