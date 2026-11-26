//! An isolated caller's refused store fails the call and leaves memory as it was.
use super::*;
use fujimod_emulator_cpu::{Model, arm};
use fujimod_emulator_cpu_unicorn::Library;

#[test]
fn a_refused_store_fails_the_call_and_its_bytes_are_restored() {
    let library = Library::linked().unwrap();
    let raw = library.open(Model::CortexR5F, 4 << 20).unwrap();
    let cpu: Box<dyn Cpu> = Box::new(
        fujimod_emulator_cpu_unicorn::rprofile::core::Core::new(
            raw,
            fujimod_emulator_cpu_unicorn::rprofile::core::Options::default(),
            fujimod_emulator_cpu_unicorn::rprofile::core::Wiring {
                memory: Default::default(),
            },
        )
        .unwrap(),
    );
    cpu.map_zeroed(0x10000, 0x1000, 7).unwrap();
    // str r0, [r6]; b .
    let code = [0xe586_0000u32, 0xeaff_fffe];
    cpu.write_memory(0x10000, &code.map(u32::to_le_bytes).concat())
        .unwrap();
    cpu.write_memory(0x10900, &[0x11, 0x22, 0x33, 0x44])
        .unwrap();
    for (register, value) in [
        (arm::CORE[0], 0xdead_beef_u32),
        (arm::CORE[6], 0x10900),
        (arm::CPSR, 0xd3),
        (Register::Pc, 0x10000),
    ] {
        cpu.write_register(register, &value.to_le_bytes()).unwrap();
    }
    let mut board = Board::new(
        vec![cpu],
        Bus::new(bus::Mode::Strict),
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true],
        memory::Memory::default(),
    )
    .unwrap();
    board.set_execution_policy(ExecutionPolicy::Exact).unwrap();
    let options = serde_json::from_value(json!({
        "core": 0, "inject_interrupts": false, "deliver_faults": false,
        "memory": {"aliases": [], "observed": [[0x10000, 0x11000]]}, "return_address": 0x10ff0,
        "breakpoints": [], "stop_addresses": [], "unarmed": [], "ranges": null,
        "vectors": [], "vector_opcode": 0, "stack": [0x10800, 0x10880],
        "allowed_stores": [[0x10a00, 0x10a04]], "record_stores": true, "include_stack": false,
        "store_limit": 16, "coverage": false, "count_ranges": [], "trace": 0,
        "device_policy": null,
    }))
    .unwrap();
    let mut session = board.harness_begin(options).unwrap();
    board.run_for_instructions(10, 10).unwrap();
    let event = session.state()["event"].clone();
    assert_eq!(event["kind"], "failure", "{event}");
    assert!(
        event["message"]
            .as_str()
            .unwrap()
            .contains("Store outside allowed ranges at 0x10900"),
        "{event}"
    );
    let mut bytes = [0; 4];
    board
        .cpu(0)
        .unwrap()
        .read_memory(0x10900, &mut bytes)
        .unwrap();
    assert_eq!(bytes, [0x11, 0x22, 0x33, 0x44]);
    board.harness_end(&mut session).unwrap();
}
