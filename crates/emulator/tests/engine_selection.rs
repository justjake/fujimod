//! Configuration values reach CPU construction without a launcher-specific engine model.
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator::board::{bus::{Bus, Mode}, factory};
use serde_json::json;

fn graph(model: &str) -> factory::Graph {
    serde_json::from_value(json!({
        "processors": [{"model": model, "released": true, "mprofile": null,
            "rprofile": null, "registers": [["Pc", 65536], ["Pstate", 965], [{"X": 1}, 69632]]}],
        "ram": [{"base": 65536, "size": 16384}],
        "mappings": [{"core": 0, "address": 65536, "physical": 65536,
            "size": 16384, "permissions": 7}],
        "dma_mappings": [{"address": 65536, "physical": 65536,
            "size": 16384, "permissions": 3}],
        "decoders": [], "peripherals": [], "instructions_per_tick": [1, 1]
    })).unwrap()
}

#[test]
fn configuration_engine_values_keep_their_wire_contract() {
    for (name, expected) in [
        ("unicorn", RunEngine::Unicorn),
        ("hypervisor", RunEngine::Hypervisor),
    ] {
        let engine: RunEngine = serde_json::from_value(json!(name)).unwrap();
        assert_eq!(engine, expected);
        assert_eq!(serde_json::to_value(engine).unwrap(), json!(name));
        assert_eq!(factory::Options::new(engine).engine, expected);
    }
    for value in [json!("hvf"), json!("Unicorn"), json!(""), json!(null), json!(1)] {
        assert!(serde_json::from_value::<RunEngine>(value).is_err());
    }
}

#[test]
fn configured_unicorn_executes_the_declared_aarch64_core() {
    let engine = serde_json::from_value(json!("unicorn")).unwrap();
    let mut board = graph("armv8a").build(
        factory::Options::new(engine).with_translation_cache_size_bytes(1 << 20),
        Bus::new(Mode::Strict), None,
    ).unwrap();
    // mov x0, #42; str w0, [x1]; b .
    let program: Vec<u8> = [0xd2800540u32, 0xb9000020, 0x14000000]
        .into_iter().flat_map(u32::to_le_bytes).collect();
    board.write_memory(0, 0x10000, &program).unwrap();
    board.run_until(4, 4).unwrap();
    let mut result = [0; 4];
    board.read_memory(0, 0x11000, &mut result).unwrap();
    assert_eq!(u32::from_le_bytes(result), 42);
}

#[test]
fn configured_hypervisor_refuses_unsupported_processors_before_vm_creation() {
    let engine = serde_json::from_value(json!("hypervisor")).unwrap();
    let error = graph("cortex-a7").build(
        factory::Options::new(engine), Bus::new(Mode::Strict), None,
    ).err().expect("a Cortex-A7 cannot use Hypervisor.framework");
    assert!(error.contains("Hypervisor boards support AArch64 cores"), "{error}");
}
