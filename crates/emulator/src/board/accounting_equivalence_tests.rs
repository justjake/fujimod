//! Translated block accounting must land on a known guest and device state.
//!
//! The program exercises a coarse debit inside a block, an exact stop, a guest
//! timer read with its deadline, a self-modifying store and an ARM/Thumb call.
//! Both policies are pinned to the state the engine produced when the
//! translated path replaced the helper path.
use super::*;
use fujimod_emulator_cpu::arm;
use fujimod_emulator_cpu::Model;
use fujimod_emulator_cpu_unicorn::Library;
use fujimod_emulator_devices::{Cx, Device};
use std::sync::{Arc, Mutex};

const BUDGET: u64 = 260;

#[derive(Default)]
struct Probe {
    value: u64,
    due: Option<u64>,
    calls: Vec<(u64, u64)>,
}
struct Timer(Arc<Mutex<Probe>>);
impl Device for Timer {
    fn name(&self) -> &'static str {
        "accounting-timer"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, _: u8) -> u64 {
        let mut probe = self.0.lock().unwrap();
        probe.calls.push((cx.now(), address));
        probe.value
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, value: u64) {
        let mut probe = self.0.lock().unwrap();
        probe.value = value;
        probe.due = Some(cx.now() + 7);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.0.lock().unwrap().due
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut probe = self.0.lock().unwrap();
        if probe.due.is_some_and(|at| at <= cx.now()) {
            probe.due = None;
            probe.value += 1;
            probe.calls.push((cx.now(), 0));
        }
    }
    fn save(&self) -> Value {
        let probe = self.0.lock().unwrap();
        json!({"due":probe.due,"calls":probe.calls,"value":probe.value})
    }
}

fn set(cpu: &dyn Cpu, register: Register, value: u32) {
    cpu.write_register(register, &value.to_le_bytes()).unwrap();
}

fn words(cpu: &dyn Cpu, address: u64, values: &[u32]) {
    cpu.write_memory(
        address,
        &values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
    )
    .unwrap();
}

fn build() -> (Board, Arc<Mutex<Probe>>) {
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
    cpu.map_zeroed(0x20000, 0x1000, 7).unwrap();
    cpu.map_zeroed(0x11000, 0x1000, 7).unwrap();
    words(
        &*cpu,
        0x10000,
        &[
            0xe3a0_0000, // mov r0, #0
            0xe585_0000, // str r0, [r5] (timer write)
            0xe595_2000, // ldr r2, [r5] (timer read)
            0xe080_0002, // add r0, r0, r2
            0xe594_1000, // ldr r1, [r4]
            0xe281_1001, // add r1, r1, #1
            0xe584_1000, // str r1, [r4]
            0xe594_1000, // ldr r1, [r4]
            0xe281_1001, // add r1, r1, #1
            0xe584_1000, // str r1, [r4]
            0xe597_3000, // ldr r3, [r7]
            0xe1a0_800f, // mov r8, pc
            0xe588_3000, // str r3, [r8] (patch code at 0x10034)
            0xe1a0_0000, // nop, patched to str r0, [r4, #16]
            0xe584_0008, // str r0, [r4, #8]
            0xe12f_ff39, // blx r9 (Thumb)
            0xe584_000c, // str r0, [r4, #12]
            0xeaff_fffe, // b .
        ],
    );
    // Thumb leaf: adds r0, #2; bx lr
    words(&*cpu, 0x11000, &[0x4770_3002]);
    // Patch word and initial data.
    words(&*cpu, 0x20000, &[0, 0xe584_0010, 0, 0, 0]);
    set(&*cpu, arm::CORE[4], 0x20000);
    set(&*cpu, arm::CORE[5], 0x8000000);
    set(&*cpu, arm::CORE[7], 0x20004);
    set(&*cpu, arm::CORE[9], 0x11001);
    set(&*cpu, arm::CPSR, 0xd3);
    set(&*cpu, Register::Pc, 0x10000);
    let probe = Arc::new(Mutex::new(Probe::default()));
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(0x8000000, 4, Box::new(Timer(probe.clone())))
        .unwrap();
    let mut board = Board::new(
        vec![cpu],
        bus,
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true],
        memory::Memory::default(),
    )
    .unwrap();
    board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
    (board, probe)
}

struct Case {
    policy: ExecutionPolicy,
    registers: [u32; 16],
    ram: [u8; 20],
    clock: u64,
    probe: Value,
}

fn cases() -> [Case; 2] {
    [
        Case {
            policy: ExecutionPolicy::Interactive,
            registers: [
                3, 2, 1, 3_850_633_232, 0x20000, 0x8000000, 0, 0x20004, 0x10034, 0x11001,
                0, 0, 0, 0, 0x10040, 0x10044,
            ],
            ram: [
                2, 0, 0, 0, 0x10, 0, 0x84, 0xe5, 1, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0,
            ],
            clock: 252,
            probe: json!({"calls":[[24,0],[24,0x8000000]],"due":null,"value":1}),
        },
        Case {
            policy: ExecutionPolicy::Exact,
            registers: [
                2, 2, 0, 3_850_633_232, 0x20000, 0x8000000, 0, 0x20004, 0x10034, 0x11001,
                0, 0, 0, 0, 0x10040, 0x10044,
            ],
            ram: [
                2, 0, 0, 0, 0x10, 0, 0x84, 0xe5, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0,
            ],
            clock: 260,
            probe: json!({"calls":[[3,0x8000000],[9,0]],"due":null,"value":1}),
        },
    ]
}

#[test]
fn translated_accounting_lands_on_the_expected_state() {
    for case in cases() {
        let (mut board, probe) = build();
        board.set_execution_policy(case.policy).unwrap();
        board.run_for_instructions(BUDGET, 1000).unwrap();

        assert_eq!(
            board.instruction_counts(),
            vec![BUDGET],
            "{:?}: instruction count",
            case.policy
        );
        let fingerprint = board.cpu(0).unwrap().fingerprint().unwrap();
        let registers: Vec<u32> = fingerprint["architecture"]["registers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| u32::try_from(value.as_u64().unwrap()).unwrap())
            .collect();
        assert_eq!(
            registers,
            case.registers.to_vec(),
            "{:?}: architectural registers",
            case.policy
        );
        assert_eq!(
            fingerprint["clock"]["now"].as_u64(),
            Some(case.clock),
            "{:?}: board clock",
            case.policy
        );

        let mut ram = vec![0; case.ram.len()];
        board.read_memory(0, 0x20000, &mut ram).unwrap();
        assert_eq!(ram, case.ram.to_vec(), "{:?}: guest RAM", case.policy);

        // Serializing the board reads every device, so the probe lock must not
        // be held across it.
        let device = {
            let probe = probe.lock().unwrap();
            json!({"due":probe.due,"calls":probe.calls,"value":probe.value})
        };
        assert_eq!(device, case.probe, "{:?}: timer device", case.policy);
    }
}
