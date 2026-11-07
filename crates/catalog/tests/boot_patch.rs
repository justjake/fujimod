use fujimod_catalog::ReferenceCatalog;
use fujimod_catalog::boot_patch::{BootPatchCompression, BootPatchTriggerKind};

#[test]
fn located_boot_patch_facts_resolve() {
    let catalog = ReferenceCatalog::bundled().expect("bundled catalog loads");
    let facts = catalog.boot_patch("x70-110").expect("X70 boot-patch facts");
    assert_eq!(facts.isa, fujimod_catalog::boot_patch::BootPatchIsa::Arm32);
    assert_eq!(facts.stage3_resident, ["boot"]);
    assert_eq!(facts.units.len(), 11);
    assert_eq!(facts.units[0].name, "boot");
    assert_eq!(facts.units[0].size, 0x20000);
    assert_eq!(facts.units[0].sha256, facts.units[0].sha256.to_lowercase());
    assert_eq!(facts.units[6].name, "section4");
    assert_eq!(facts.units[6].compression, BootPatchCompression::Method4);

    assert_eq!(facts.triggers.len(), 2);
    let first = &facts.triggers[0];
    assert_eq!(first.name, "section4-decoded");
    assert_eq!(first.site, 0x1d84);
    assert_eq!(first.expected, 0xeb00013b);
    assert_eq!(first.kind, BootPatchTriggerKind::Once);
    assert!(first.units.contains(&"section4".to_string()));
    assert!(!first.units.contains(&"section5".to_string()));
    let second = &facts.triggers[1];
    assert_eq!(second.site, 0x94ac20);
    assert_eq!(second.expected, 0xe30b02ac);
    assert!(second.units.contains(&"section5".to_string()));

    assert_eq!(facts.clears.len(), 6);
    let section5 = facts
        .clears
        .iter()
        .find(|clear| clear.name == "section5-bss")
        .expect("section-5 BSS clear");
    assert_eq!(section5.address, 0x1514160);
    assert_eq!(section5.size, 0x473000);
    assert_eq!(section5.done_after, "section5-loaded");

    assert_eq!(facts.trampoline_space.address, 0x840000);
    assert_eq!(facts.trampoline_space.size, 0x1000);
    assert_eq!(facts.resident_space.address, 0x1d040000);
    assert_eq!(facts.resident_space.size, 0x2E0000);
    assert!(catalog.boot_patch("missing-100").is_none());
}

#[test]
fn boot_patch_spaces_stay_clear_of_units_and_clears() {
    let catalog = ReferenceCatalog::bundled().expect("bundled catalog loads");
    let facts = catalog.boot_patch("x70-110").expect("X70 boot-patch facts");
    for (space, name) in [
        (facts.trampoline_space, "trampoline"),
        (facts.resident_space, "resident"),
    ] {
        for unit in &facts.units {
            assert!(
                space.address + space.size <= unit.address
                    || unit.address + unit.size <= space.address,
                "{name} space overlaps unit {}",
                unit.name
            );
        }
        for clear in &facts.clears {
            assert!(
                space.address + space.size <= clear.address
                    || clear.address + clear.size <= space.address,
                "{name} space overlaps clear {}",
                clear.name
            );
        }
    }
}
