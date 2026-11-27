use fujimod_catalog_configuration::RunConfiguration;
use fujimod_catalog_resolution::ResolvedConfiguration;

#[test]
fn every_exr2_run_fills_ram_with_the_measured_x70_byte() {
    let mut runs = 0;
    for id in RunConfiguration::bundled_ids() {
        let resolved =
            ResolvedConfiguration::bundled(id).unwrap_or_else(|error| panic!("{id}: {error}"));
        if resolved.reference.soc.id != "exr2" {
            continue;
        }
        assert_eq!(resolved.power_on_ram_fill(), Some(0xff), "run {id}");
        runs += 1;
    }
    assert!(runs > 0, "no EXR II run configuration found");
}

#[test]
fn a_body_measurement_overrides_the_processor_dram_fill() {
    let mut resolved = ResolvedConfiguration::bundled("x70-110-rec").unwrap();
    assert_eq!(resolved.power_on_ram_fill(), Some(0xff));
    resolved.reference.body.power_on_ram_fill = Some(fujimod_catalog::reference::PowerOnFill {
        byte: 0,
        evidence: fujimod_catalog::provenance::Evidence {
            kind: fujimod_catalog::provenance::EvidenceKind::Measured,
            firmware_ids: vec!["x70-110".into()],
            addresses: vec![],
        },
    });
    assert_eq!(resolved.power_on_ram_fill(), Some(0));
}
