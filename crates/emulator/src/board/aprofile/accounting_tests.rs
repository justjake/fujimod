//! The block backend and an instruction-observed core must expose the same
//! device timeline, register state, entry stops, and runnable-peer turns.
use super::*;
use crate::board::{
    Board, InterruptMap, InterruptRegisters,
    bus::{Bus, Mode},
    memory::{self, BusMapping},
    set,
};
use fujimod_emulator_cpu::{Cpu, Model, Register, memory::Backing, timeline::Ratio};
use fujimod_emulator_cpu_unicorn::{Library, aprofile, memory::Memory};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_ip_arm::gicv3::Gic;
use serde_json::{Value, json};
use std::{
    ptr::NonNull,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct State {
    due: Option<u64>,
    value: u64,
    events: Vec<(u64, &'static str, u64)>,
}
struct Probe(Arc<Mutex<State>>);
impl Device for Probe {
    fn name(&self) -> &'static str {
        "accounting-probe"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        let mut s = self.0.lock().unwrap();
        let value = s.value;
        s.events.push((cx.now(), "read", value));
        value
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, value: u64) {
        let mut s = self.0.lock().unwrap();
        s.value = value;
        s.due = Some(cx.now() + 5);
        s.events.push((cx.now(), "write", value));
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.0.lock().unwrap().due
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut s = self.0.lock().unwrap();
        if s.due.is_some_and(|d| d <= cx.now()) {
            s.due = None;
            s.value += 100;
            let value = s.value;
            s.events.push((cx.now(), "deadline", value));
        }
    }
    fn save(&self) -> Value {
        let s = self.0.lock().unwrap();
        json!({"due":s.due,"value":s.value,"events":s.events})
    }
}
fn board(counted: bool, cores: usize, ratio: Ratio) -> Board {
    let library = Library::linked().unwrap();
    let shared = Memory::default();
    let backing = Backing::aligned_zeros(0x4000, 4096).unwrap();
    let mut cpus = vec![];
    for core in 0..cores {
        let raw = library.open(Model::ArmV8A, 1 << 20).unwrap();
        let cpu: Box<dyn Cpu> = if counted {
            Box::new(aprofile::Core::new(raw, shared.clone()).unwrap())
        } else {
            Box::new(raw)
        };
        unsafe {
            cpu.map_physical(0, 0, 0x4000, 7, NonNull::new(backing.as_ptr()).unwrap())
                .unwrap()
        }
        set(&*cpu, Register::Pc, 0x1000 + core as u64 * 0x100).unwrap();
        set(&*cpu, Register::Pstate, 0x3c5).unwrap();
        set(&*cpu, Register::X(1), 0x8000).unwrap();
        cpus.push(cpu);
    }
    let mut bus = Bus::new(Mode::Strict);
    bus.register(
        0x8000,
        4,
        Box::new(Probe(Arc::new(Mutex::new(State {
            due: Some(3),
            ..State::default()
        })))),
    )
    .unwrap();
    let gic = InterruptMap {
        gic: Box::new(Gic::new((0..cores as u32).collect(), 64).unwrap()),
        registers: InterruptRegisters::GicV3 {
            distributor: 0xfa000000,
            redistributor: 0xfa080000,
            stride: 0x20000,
        },
        cores,
    };
    let mut board = Board::new(
        cpus,
        bus,
        ratio,
        Some(gic),
        vec![true; cores],
        memory::Memory::new(vec![(0, backing)], vec![BusMapping::ram(0, 0x4000)]).unwrap(),
    )
    .unwrap();
    for core in 0..cores {
        board.map_mmio(core, 0x8000, 0x1000, 0x8000).unwrap();
        board
            .configure_aprofile(
                core,
                Config {
                    clock: generic_timer::Clock {
                        counter_hz: 100,
                        ticks_per_second: 100,
                    },
                    physical_irq: 30,
                    virtual_irq: 27,
                    pmu_hz: 100,
                    implementation: None,
                },
            )
            .unwrap();
    }
    board
}
fn words(board: &Board, address: u64, words: &[u32]) {
    board
        .write_memory(
            0,
            address,
            &words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap()
}
fn assert_same(a: &Board, b: &Board) {
    assert_eq!(a.state(), b.state());
    assert_eq!(
        a.memory_fingerprint().unwrap(),
        b.memory_fingerprint().unwrap()
    );
    for core in 0..a.instruction_counts().len() {
        for r in (0..31)
            .map(Register::X)
            .chain([Register::Pc, Register::Sp, Register::Pstate])
        {
            assert_eq!(
                a.read_register(core, r).unwrap(),
                b.read_register(core, r).unwrap(),
                "CPU{core} {r:?}"
            );
        }
    }
}
#[test]
fn device_deadlines_and_peer_slices_match_at_every_partial_budget() {
    for cores in [1, 2] {
        for quantum in [1, 7, 64] {
            let mut a = board(false, cores, Ratio::new(3, 2).unwrap());
            let mut b = board(true, cores, Ratio::new(3, 2).unwrap());
            for core in 0..cores {
                for board in [&a, &b] {
                    words(
                        board,
                        0x1000 + core as u64 * 0x100,
                        &[0x11000442, 0xb9000022, 0x11000463, 0xb9400024, 0x17fffffc],
                    );
                }
            }
            for budget in [1, 2, 4, 7, 11, 31, 67] {
                a.run_for_instructions(budget, quantum).unwrap();
                b.run_for_instructions(budget, quantum).unwrap();
                assert_same(&a, &b);
            }
        }
    }
}
#[test]
fn framework_reads_and_wait_use_the_before_entry_tick() {
    let mut a = board(false, 1, Ratio::new(1, 1).unwrap());
    let mut b = board(true, 1, Ratio::new(1, 1).unwrap());
    for board in [&a, &b] {
        words(
            board,
            0x1000,
            &[0x91000442, 0xd53be043, 0x91000442, 0xd53be044, 0xd503207f],
        );
    }
    a.run_for_instructions(100, 1000).unwrap();
    b.run_for_instructions(100, 1000).unwrap();
    assert_same(&a, &b);
    assert_eq!(b.read_register(0, Register::X(3)).unwrap(), 1);
    assert_eq!(b.read_register(0, Register::X(4)).unwrap(), 3);
    assert_eq!(b.instruction_counts(), [5]);
}
#[test]
fn instruction_points_pause_and_resume_without_entering_the_point() {
    let mut a = board(false, 1, Ratio::new(1, 1).unwrap());
    let mut b = board(true, 1, Ratio::new(1, 1).unwrap());
    for board in [&a, &b] {
        words(
            board,
            0x1000,
            &[0x91000400, 0x91000400, 0x91000400, 0x14000000],
        );
    }
    let pa = a.instruction_points_begin(0, vec![0x1008]).unwrap();
    let pb = b.instruction_points_begin(0, vec![0x1008]).unwrap();
    a.run_for_instructions(20, 1000).unwrap();
    b.run_for_instructions(20, 1000).unwrap();
    assert_same(&a, &b);
    assert_eq!(pa.event(), Some(0x1008));
    assert_eq!(pb.event(), Some(0x1008));
    assert_eq!(b.instruction_counts(), [2]);
    pa.resume(0x1008, 0x1008);
    pb.resume(0x1008, 0x1008);
    a.run_for_instructions(7, 1000).unwrap();
    b.run_for_instructions(7, 1000).unwrap();
    assert_same(&a, &b);
}
