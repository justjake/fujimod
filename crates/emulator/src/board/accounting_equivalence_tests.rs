//! The translated and helper budget paths must produce identical guest state.
use super::*;
use fujimod_emulator_cpu::arm;
use fujimod_emulator_cpu::Model;
use fujimod_emulator_cpu_unicorn::Library;
use fujimod_emulator_devices::{Cx, Device};
use std::sync::{Arc, Mutex};

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

fn build(legacy: bool) -> (Board, Arc<Mutex<Probe>>) {
    let library = Library::linked().unwrap();
    let raw = library.open(Model::CortexR5F, 4 << 20).unwrap();
    let cpu: Box<dyn Cpu> = Box::new(
        fujimod_emulator_cpu_unicorn::rprofile::core::Core::new(
            raw,
            fujimod_emulator_cpu_unicorn::rprofile::core::Options {
                legacy_block_accounting: legacy,
                ..Default::default()
            },
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

fn compare(inline: &Board, legacy: &Board, a: &Arc<Mutex<Probe>>, b: &Arc<Mutex<Probe>>) {
    assert_eq!(inline.instruction_counts(), legacy.instruction_counts());
    assert_eq!(inline.state(), legacy.state());
    assert_eq!(
        inline.cpu(0).unwrap().fingerprint().unwrap(),
        legacy.cpu(0).unwrap().fingerprint().unwrap()
    );
    let mut ram_a = vec![0; 0x1000];
    let mut ram_b = vec![0; 0x1000];
    inline.read_memory(0, 0x20000, &mut ram_a).unwrap();
    legacy.read_memory(0, 0x20000, &mut ram_b).unwrap();
    assert_eq!(ram_a, ram_b);
    assert_eq!(
        {
            let p = a.lock().unwrap();
            json!({"due":p.due,"calls":p.calls,"value":p.value})
        },
        {
            let p = b.lock().unwrap();
            json!({"due":p.due,"calls":p.calls,"value":p.value})
        }
    );
}

#[test]
fn inline_and_helper_block_accounting_preserve_the_same_board_state() {
    let (mut inline, probe_inline) = build(false);
    let (mut legacy, probe_legacy) = build(true);
    for board in [&mut inline, &mut legacy] {
        board.set_execution_policy(ExecutionPolicy::Interactive).unwrap();
    }
    compare(&inline, &legacy, &probe_inline, &probe_legacy);

    // Caps inside a block, zero work, a deadline, a timer read and a
    // self-modifying store all enter the accounting paths under comparison.
    for count in [0, 1, 7, 13, 29, 41, 101, 211, 257] {
        inline.run_for_instructions(count, 1000).unwrap();
        legacy.run_for_instructions(count, 1000).unwrap();
        compare(&inline, &legacy, &probe_inline, &probe_legacy);
    }
    // The self-modifying store and the Thumb call must both have executed, or
    // the runs above would not compare the paths they claim to.
    let mut reached = vec![0; 0x1000];
    inline.read_memory(0, 0x20000, &mut reached).unwrap();
    let word = |at: usize| u32::from_le_bytes(reached[at * 4..at * 4 + 4].try_into().unwrap());
    assert_eq!(word(0), 2, "both counted stores must run");
    assert_ne!(word(2), 0, "the timer read must reach r0");
    assert_eq!(word(4), word(2), "the patched store must see the same r0");
    assert_eq!(word(3), word(2) + 2, "the Thumb leaf must add two to r0");
    let due = {
        let p = probe_inline.lock().unwrap();
        p.due.unwrap_or(7)
    };
    inline.run_until(due, 1000).unwrap();
    legacy.run_until(due, 1000).unwrap();
    compare(&inline, &legacy, &probe_inline, &probe_legacy);

    // Exact accounting keeps its ring write in the helper on both boards; the
    // policy transition and final state must still match.
    for board in [&mut inline, &mut legacy] {
        board.set_execution_policy(ExecutionPolicy::Exact).unwrap();
    }
    inline.run_for_instructions(17, 1000).unwrap();
    legacy.run_for_instructions(17, 1000).unwrap();
    compare(&inline, &legacy, &probe_inline, &probe_legacy);
}
