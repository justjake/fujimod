use super::*;
use fujimod_emulator_cpu::{Model, arm, memory::Backing};
use fujimod_emulator_cpu_unicorn::{Library, rprofile};
use fujimod_emulator_devices::{Cx, Device, bus::FastReadWord};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

struct Flag {
    value: Arc<AtomicU32>,
    due: Option<u64>,
    declared: bool,
    irq: bool,
}
impl Device for Flag {
    fn name(&self) -> &'static str {
        "test-stable-flag"
    }
    fn fast_reads(&self, _: u64, _: u64) -> Vec<FastReadWord> {
        if self.declared {
            vec![FastReadWord::atomic_u32(0, self.value.clone())]
        } else {
            vec![]
        }
    }
    fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        self.value.load(Ordering::Acquire) as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.due
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.due.is_some_and(|due| due <= cx.now()) {
            self.due = None;
            self.value.store(1, Ordering::Release);
            cx.ram_write(0x3000, &1u32.to_le_bytes()).unwrap();
            // Exercise a DMA replacement of code already proven by this CPU.
            cx.ram_write(0x1004, &0xe3110001u32.to_le_bytes()).unwrap();
            if self.irq {
                cx.set_core_irq(0, 3, true).unwrap();
            }
        }
    }
    fn save(&self) -> Value {
        json!({"value":self.value.load(Ordering::Acquire),"due":self.due})
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
}
fn board(mmio: bool, declared: bool, peer: bool, irq: bool, mode: LoopMode) -> Board {
    let library = Library::linked().unwrap();
    let backing = Backing::aligned_zeros(0x4000, 4096).unwrap();
    let memory = rprofile::memory::Memory::default();
    let cpus = (0..if peer { 2 } else { 1 })
        .map(|core| {
            let cpu = rprofile::core::Core::new(
                library.open(Model::CortexR5F, 1024 * 1024).unwrap(),
                Default::default(),
                rprofile::core::Wiring {
                    memory: memory.clone(),
                },
            )
            .unwrap();
            unsafe {
                cpu.map_physical(
                    0,
                    0,
                    0x4000,
                    7,
                    std::ptr::NonNull::new(backing.as_ptr()).unwrap(),
                )
                .unwrap();
            }
            set(&cpu, Register::Pc, 0x1000 + core * 0x1000).unwrap();
            set(&cpu, Register::Pstate, if irq { 0x53 } else { 0xd3 }).unwrap();
            set(
                &cpu,
                arm::CORE[2],
                if mmio && core == 0 { 0x8000000 } else { 0x3000 },
            )
            .unwrap();
            Box::new(cpu) as Box<dyn Cpu>
        })
        .collect();
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(
        0x8000000,
        4,
        Box::new(Flag {
            value: Arc::new(AtomicU32::new(0)),
            due: Some(53),
            declared,
            irq,
        }),
    )
    .unwrap();
    let mut board = Board::new(
        cpus,
        bus,
        Ratio::new(7, 3).unwrap(),
        None,
        vec![true; if peer { 2 } else { 1 }],
        memory::Memory::new(vec![(0, backing)], vec![memory::BusMapping::ram(0, 0x4000)]).unwrap(),
    )
    .unwrap();
    board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
    let code = [0xe5921000u32, 0xe3110001, 0x0afffffc, 0xe320f003];
    board
        .write_memory(
            0,
            0x1000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    // Runnable peer writes the RAM flag after its counted delay.
    let code = [
        0xe3a00004u32,
        0xe2500001,
        0x1afffffd,
        0xe3a01001,
        0xe5821000,
        0xe320f003,
    ];
    board
        .write_memory(
            0,
            0x2000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board.set_loop_mode(mode).unwrap();
    if irq {
        board
            .configure_core_interrupts(0, 0, Box::new(Pin(Arc::new(AtomicBool::new(false)))))
            .unwrap();
        let code = [0xe3a07077u32, 0xe320f003];
        board
            .write_memory(
                0,
                0x18,
                &code
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    board
}
#[test]
fn loop_execution_stops_at_dma_deadlines_and_peer_turns_with_rational_time() {
    for mmio in [false, true] {
        for peer in [false, true] {
            for quantum in [1, 7, 1000] {
                let mut off = board(mmio, true, peer, false, LoopMode::Disabled);
                let mut on = board(mmio, true, peer, false, LoopMode::Accelerate);
                for tick in [9, 28, 52, 53, 54, 100] {
                    off.run_until(tick, quantum).unwrap();
                    on.run_until(tick, quantum).unwrap();
                    assert_eq!(
                        on.fingerprint().unwrap(),
                        off.fingerprint().unwrap(),
                        "mmio={mmio} peer={peer} quantum={quantum} tick={tick}"
                    );
                }
                if !peer {
                    assert!(
                        on.loop_statistics().unwrap()[0]["loop_accelerated_instructions"]
                            .as_u64()
                            .unwrap()
                            > 0
                    );
                }
            }
        }
    }
}
#[test]
fn an_undeclared_device_read_is_never_accelerated() {
    let mut board = board(true, false, false, false, LoopMode::Accelerate);
    board.run_until(100, 1000).unwrap();
    assert_eq!(
        board.loop_statistics().unwrap()[0]["loop_accelerated_instructions"],
        0
    );
}

struct Pin(Arc<AtomicBool>);
impl fujimod_emulator_devices::bus::CoreInterrupts for Pin {
    fn pulse(&self, _: u32) -> Result<(), String> {
        self.0.store(true, Ordering::Release);
        Ok(())
    }
    fn level(&self, _: u32, high: bool) -> Result<(), String> {
        self.0.store(high, Ordering::Release);
        Ok(())
    }
    fn irq(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
#[test]
fn an_interrupt_at_the_input_deadline_enters_before_the_next_loop_instruction() {
    let mut off = board(false, true, false, true, LoopMode::Disabled);
    let mut on = board(false, true, false, true, LoopMode::Accelerate);
    for tick in [52, 53, 54, 70] {
        off.run_until(tick, 1000).unwrap();
        on.run_until(tick, 1000).unwrap();
        assert_eq!(
            on.fingerprint().unwrap(),
            off.fingerprint().unwrap(),
            "tick={tick}"
        );
    }
    assert_eq!(on.read_register(0, arm::CORE[7]).unwrap(), 0x77);
}

#[test]
fn store_continuations_allow_loops_and_stop_at_the_due_dma() {
    let mut off = board(false, true, false, false, LoopMode::Disabled);
    let mut on = board(false, true, false, false, LoopMode::Accelerate);
    for board in [&mut off, &mut on] {
        board
            .continuation_begin(vec![(0x3000, 0x3004)], Default::default(), vec![])
            .unwrap();
        board.run_until(100, 1000).unwrap();
        assert!(board.continuation_end().unwrap());
        assert_eq!(board.now(), 53);
    }
    assert_eq!(on.fingerprint().unwrap(), off.fingerprint().unwrap());
    assert!(
        on.loop_statistics().unwrap()[0]["loop_accelerated_instructions"]
            .as_u64()
            .unwrap()
            > 0
    );
}

#[test]
fn device_continuations_keep_every_poll_observable() {
    let mut board = board(true, true, false, false, LoopMode::Accelerate);
    board
        .continuation_begin(vec![], [0].into_iter().collect(), vec![])
        .unwrap();
    board.run_until(100, 1000).unwrap();
    assert!(board.continuation_end().unwrap());
    assert_eq!(
        board.loop_statistics().unwrap()[0]["loop_accelerated_instructions"],
        0
    );
}

#[test]
fn interactive_clock_poll_windows_require_one_runnable_processor() {
    let mut single = board(false, true, false, false, LoopMode::Accelerate);
    single
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    assert!(single.state.borrow().loop_window());
    let mut peers = board(false, true, true, false, LoopMode::Accelerate);
    peers
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    assert!(!peers.state.borrow().loop_window());
}

#[test]
fn clock_poll_acceleration_preserves_device_deadlines_and_complete_board_state() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        let mut off = board(false, true, false, false, LoopMode::Disabled);
        let mut on = board(false, true, false, false, LoopMode::Accelerate);
        for board in [&mut off, &mut on] {
            board.set_execution_policy(policy).unwrap();
            board
                .write_memory(
                    0,
                    0x1000,
                    &[0xee190f1du32, 0xe3500064, 0x3afffffc, 0xe320f003]
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect::<Vec<_>>(),
                )
                .unwrap();
            let cpu = board.cpu(0).unwrap();
            cpu.set_guest_clock(fujimod_emulator_cpu::Clock {
                now: 0,
                phase: 0,
                ratio: board.state.borrow().timeline.ratio(),
            })
            .unwrap();
            for (crm, opc2, value) in [(12, 0, 1), (12, 1, 0x80000000)] {
                cpu.coprocessor(
                    fujimod_emulator_cpu::arm::Coprocessor {
                        cp: 15,
                        opc1: 0,
                        crn: 9,
                        crm,
                        opc2,
                        secure: 0,
                        wide: false,
                    },
                    Some(value),
                )
                .unwrap();
            }
            board.run_until(200, 1000).unwrap();
        }
        assert_eq!(on.fingerprint().unwrap(), off.fingerprint().unwrap());
        assert!(
            on.loop_statistics().unwrap()[0]["loop_clock_poll_instructions"]
                .as_u64()
                .unwrap()
                > 0
        );
    }
}

#[test]
fn interactive_fifo_mapping_bypasses_only_the_published_prefix() {
    use fujimod_emulator_devices::parts::read_fifo::ReadFifo;
    struct Fifo {
        words: ReadFifo,
        calls: Arc<AtomicU32>,
    }
    impl Device for Fifo {
        fn name(&self) -> &'static str {
            "test-read-fifo"
        }
        fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            self.calls.fetch_add(1, Ordering::Relaxed);
            u64::from(self.words.read().unwrap())
        }
        fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
            cx.unmodeled();
        }
        fn fast_reads(&self, _: u64, _: u64) -> Vec<FastReadWord> {
            let words = self.words.clone();
            vec![FastReadWord::consuming_u32(
                0,
                Arc::new(move || words.read_fast()),
            )]
        }
        fn save(&self) -> Value {
            json!({})
        }
    }
    let library = Library::linked().unwrap();
    let backing = Backing::aligned_zeros(0x4000, 4096).unwrap();
    let shared = rprofile::memory::Memory::default();
    let cpu = rprofile::core::Core::new(
        library.open(Model::CortexR5F, 1024 * 1024).unwrap(),
        Default::default(),
        rprofile::core::Wiring { memory: shared },
    )
    .unwrap();
    unsafe {
        cpu.map_physical(
            0,
            0,
            0x4000,
            7,
            std::ptr::NonNull::new(backing.as_ptr()).unwrap(),
        )
        .unwrap();
    }
    set(&cpu, Register::Pc, 0x1000).unwrap();
    set(&cpu, Register::Pstate, 0xd3).unwrap();
    set(&cpu, arm::CORE[2], 0x8000000).unwrap();
    let words = ReadFifo::new(12).unwrap();
    words
        .restore(
            &[1u32, 2, 3]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
            0,
        )
        .unwrap();
    words.publish(true);
    let calls = Arc::new(AtomicU32::new(0));
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(
        0x8000000,
        4,
        Box::new(Fifo {
            words: words.clone(),
            calls: calls.clone(),
        }),
    )
    .unwrap();
    let mut board = Board::new(
        vec![Box::new(cpu)],
        bus,
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true],
        memory::Memory::new(vec![(0, backing)], vec![memory::BusMapping::ram(0, 0x4000)]).unwrap(),
    )
    .unwrap();
    board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
    board
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    let code = [0xe5920000u32, 0xe5921000, 0xe5923000, 0xe320f003];
    board
        .write_memory(
            0,
            0x1000,
            &code
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board.run_until(30, 1000).unwrap();
    assert_eq!(board.read_register(0, arm::CORE[0]).unwrap(), 1);
    assert_eq!(board.read_register(0, arm::CORE[1]).unwrap(), 2);
    assert_eq!(board.read_register(0, arm::CORE[3]).unwrap(), 3);
    assert_eq!(words.position(), 12);
    assert_eq!(calls.load(Ordering::Acquire), 1);
}
