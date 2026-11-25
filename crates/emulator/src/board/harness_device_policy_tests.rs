//! An isolated caller's device policy admits only the register accesses it
//! names and records them; anything else refuses before the device sees it.
use super::*;
use fujimod_emulator_cpu::{Model, arm};
use fujimod_emulator_cpu_unicorn::Library;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const DEVICE: u64 = 0x8000000;

struct Counter(Arc<AtomicU64>);
impl fujimod_emulator_devices::Device for Counter {
    fn name(&self) -> &'static str {
        "counter"
    }
    fn read(&mut self, _: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn write(&mut self, _: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8, _: u64) {}
    fn save(&self) -> Value {
        json!({})
    }
}

fn board(reads: Arc<AtomicU64>) -> Board {
    let library = Library::linked().unwrap();
    let raw = library.open(Model::CortexR5F, 4 << 20).unwrap();
    let cpu: Box<dyn Cpu> = Box::new(
        fujimod_emulator_cpu_unicorn::rprofile::core::Core::new(
            raw,
            fujimod_emulator_cpu_unicorn::rprofile::core::Options::default(),
            fujimod_emulator_cpu_unicorn::rprofile::core::Wiring { memory: Default::default() },
        )
        .unwrap(),
    );
    cpu.map_zeroed(0x10000, 0x1000, 7).unwrap();
    // ldr r0, [r5]; ldr r1, [r5, #4]; b .
    let code = [0xe595_0000u32, 0xe595_1004, 0xeaff_fffe];
    cpu.write_memory(0x10000, &code.map(u32::to_le_bytes).concat()).unwrap();
    for (register, value) in [(arm::CORE[5], DEVICE as u32), (arm::CPSR, 0xd3), (Register::Pc, 0x10000)] {
        cpu.write_register(register, &value.to_le_bytes()).unwrap();
    }
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(DEVICE, 0x1000, Box::new(Counter(reads))).unwrap();
    let mut board = Board::new(vec![cpu], bus, Ratio::new(1, 1).unwrap(), None, vec![true], memory::Memory::default())
        .unwrap();
    board.map_mmio(0, DEVICE, 0x1000, DEVICE).unwrap();
    board.set_execution_policy(ExecutionPolicy::Exact).unwrap();
    board
}

fn options(mmio: Value) -> harness::Options {
    serde_json::from_value(json!({
        "core": 0, "inject_interrupts": false, "deliver_faults": false,
        "memory": {"aliases": [], "observed": []}, "return_address": 0x10ff0,
        "breakpoints": [], "stop_addresses": [], "unarmed": [], "ranges": null,
        "vectors": [], "vector_opcode": 0, "stack": [0x10800, 0x10c00],
        "allowed_stores": null, "record_stores": false, "include_stack": false,
        "store_limit": 16, "coverage": false, "count_ranges": [], "trace": 0,
        "device_policy": {"mmio": mmio, "model_readable": [], "model_writable": []},
    }))
    .unwrap()
}

#[test]
fn an_unnamed_register_access_refuses_before_the_device() {
    let reads = Arc::new(AtomicU64::new(0));
    let mut board = board(reads.clone());
    let mut session = board
        .harness_begin(options(json!([{"start": DEVICE, "end": DEVICE + 4, "width": 4, "read": true, "write": false}])))
        .unwrap();
    let error = board.run_for_instructions(10, 10).unwrap_err();
    assert!(error.contains("Device policy refuses a 4-byte MMIO read at 0x8000004"), "{error}");
    assert_eq!(reads.load(Ordering::SeqCst), 1, "the refused read never reached the device");
    assert_eq!(session.state()["devices"]["mmio"], json!([[DEVICE, 4, false]]));
    board.harness_end(&mut session).unwrap();
    assert_eq!(session.state()["devices"]["mmio"], json!([[DEVICE, 4, false]]));
}

#[test]
fn named_register_accesses_run() {
    let reads = Arc::new(AtomicU64::new(0));
    let mut board = board(reads.clone());
    let mut session = board
        .harness_begin(options(json!([{"start": DEVICE, "end": DEVICE + 8, "width": 4, "read": true, "write": false}])))
        .unwrap();
    board.run_for_instructions(10, 10).unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    board.harness_end(&mut session).unwrap();
}

#[test]
fn an_invalid_policy_refuses_the_session() {
    let board = board(Arc::new(AtomicU64::new(0)));
    let error = board
        .harness_begin(options(json!([{"start": DEVICE + 2, "end": DEVICE + 6, "width": 4, "read": true, "write": false}])))
        .err()
        .unwrap();
    assert!(error.contains("aligned"), "{error}");
}
