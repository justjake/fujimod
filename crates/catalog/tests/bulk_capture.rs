use fujimod_catalog::ReferenceCatalog;

#[test]
fn x100vi_bulk_is_valid_in_bundled_and_filesystem_catalogs() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog");
    for catalog in [
        ReferenceCatalog::bundled().unwrap(),
        ReferenceCatalog::load(root).unwrap(),
    ] {
        let selection = catalog.resolve_firmware("x100vi-131").unwrap();
        let bulk = selection.bulk.unwrap();
        bulk.validate().unwrap();
        assert!(bulk.probes.iter().all(|r| bulk.window.contains(*r)));
        assert!(
            bulk.scopes
                .iter()
                .flat_map(|s| &s.ranges)
                .all(|r| bulk.window.contains(r.range()))
        );
        let state = selection.state.unwrap();
        let dram = state.regions.iter().find(|r| r.name == "dram").unwrap();
        assert_eq!(
            (bulk.window.address, bulk.window.size),
            (dram.address, dram.size)
        );
    }
}
