use fujimod_catalog_configuration::{ProcessorEmulation, RunInstructionRate};

fn emulation(id: &str) -> ProcessorEmulation {
    ProcessorEmulation::bundled(id).unwrap_or_else(|error| panic!("{id}: {error}"))
}

#[test]
fn every_processor_declares_its_emulation_choices() {
    assert_eq!(
        ProcessorEmulation::bundled_ids().collect::<Vec<_>>(),
        ["exr2", "xp4", "xp5", "xpp"]
    );
    for id in ["exr2", "xp4", "xp5", "xpp"] {
        let choices = emulation(id);
        assert_eq!(choices.id, id);
        assert!(choices.cpu_model.starts_with("Cortex-"));
    }
}

#[test]
fn processors_keep_the_choices_their_soc_profiles_held() {
    let exr2 = emulation("exr2");
    assert_eq!(exr2.cpu_model, "Cortex-R5F proxy for R4");
    assert_eq!(exr2.runtime_abi, "exr2-r5f-fixed-map-v1");
    assert_eq!(exr2.limits.len(), 1);
    assert!(exr2.limits[0].contains("EXR II register subset"));
    assert_eq!(exr2.discovery_mmio.len(), 2);
    assert_eq!(exr2.private_mmio.len(), 4);

    let xp4 = emulation("xp4");
    assert_eq!(xp4.cpu_model, "Cortex-A7");
    assert_eq!(xp4.runtime_abi, "armv7a-unicorn-v1");
    assert!(
        xp4.mmu
            .as_deref()
            .is_some_and(|mmu| mmu.contains("ARMv7-A"))
    );
    assert_eq!(xp4.engines, ["rust-unicorn-armv7a"]);
    assert_eq!(
        xp4.instructions_per_tick,
        Some(RunInstructionRate {
            numerator: 6,
            denominator: 1,
        })
    );
    assert_eq!(xp4.pmu_cycles_per_tick, Some(24));

    let xp5 = emulation("xp5");
    assert_eq!(xp5.cpu_model, "Cortex-A53");
    assert_eq!(xp5.runtime_abi, "armv8a-shared-v1");
    assert_eq!(xp5.engines, ["unicorn", "hypervisor"]);
    assert_eq!(
        xp5.instructions_per_tick,
        Some(RunInstructionRate {
            numerator: 1,
            denominator: 1,
        })
    );
    assert_eq!(xp5.board_ticks_per_second, Some(1_000_000_000));
    assert_eq!(xp5.limits.len(), 3);

    let xpp = emulation("xpp");
    assert_eq!(xpp.cpu_model, "Cortex-A7");
    assert_eq!(xpp.runtime_abi, "armv7a-unicorn-v1");
    assert_eq!(
        xpp.instructions_per_tick,
        Some(RunInstructionRate {
            numerator: 4,
            denominator: 1,
        })
    );
    assert!(xpp.limits.iter().any(|limit| limit.contains("Cortex-A7")));
}

#[test]
fn a_processor_without_a_document_is_refused() {
    let error = ProcessorEmulation::bundled("xp3").expect_err("unknown processor");
    assert!(error.contains("unsupported processor emulation"));
}
