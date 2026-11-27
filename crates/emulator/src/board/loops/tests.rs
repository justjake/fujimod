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
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode("test-stable-flag", &self.save())
    }
}
fn board(mmio: bool, declared: bool, peer: bool, irq: bool, mode: LoopMode) -> Board {
    let library = Library::linked().unwrap();
    let backing = Backing::aligned_zeros(0x4000, 4096).unwrap();
    let memory = fujimod_emulator_cpu_unicorn::memory::Memory::default();
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
                    let left = on.complete_state().unwrap();
                    let right = off.complete_state().unwrap();
                    if let Some(message) = left.diff(&right) {
                        panic!("mmio={mmio} peer={peer} quantum={quantum} tick={tick}: {message}");
                    }
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
            on.complete_state().unwrap(),
            off.complete_state().unwrap(),
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
    assert_eq!(on.complete_state().unwrap(), off.complete_state().unwrap());
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
        assert_eq!(on.complete_state().unwrap(), off.complete_state().unwrap());
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
    let shared = fujimod_emulator_cpu_unicorn::memory::Memory::default();
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

/// A Cortex-M4 with the M-profile controller and the flag device at
/// 0x8000000. Thumb code at 0x2000: a counted SUBS/BNE delay (40 instructions,
/// about 17 ticks), then a poll of
/// bit 0 of the RAM flag (0x3000) or the device register, then `B .`.
/// `poll` is the instruction between the load and the branch.
fn m4_board(mmio: bool, declared: bool, mode: LoopMode, poll: u16) -> Board {
    let (base, shift) = if mmio {
        (0x2201, 0x06d2)
    } else {
        (0x2203, 0x0312)
    };
    let code = [
        0x2014, // MOVS R0,#20
        0x3801, // SUBS R0,#1
        0xd1fd, // BNE  0x2002
        base,   // MOVS R2,#1 or #3
        shift,  // LSLS R2,R2,#27 or #12
        0x6811, // LDR  R1,[R2]
        poll, 0xd5fc, // BPL  0x200a
        0x2777, // MOVS R7,#0x77
        0xe7fe, // B    .
    ];
    let flag = Flag {
        value: Arc::new(AtomicU32::new(0)),
        due: Some(53),
        declared,
        irq: false,
    };
    m4_board_with(Box::new(flag), &code, mode)
}
/// A Cortex-M4 with the M-profile controller, `device` at 0x8000000 and
/// Thumb `code` at 0x2000.
fn m4_board_with(device: Box<dyn Device>, code: &[u16], mode: LoopMode) -> Board {
    use crate::board::factory::{Graph, MemoryMap, Options, Processor, Ram};
    use fujimod_catalog_configuration::RunEngine;
    let graph = Graph {
        decoders: vec![],
        processors: vec![Processor {
            cpu_backend: Default::default(),
            model: Model::CortexM4,
            registers: vec![],
            released: true,
            parked_until_sgi: false,
            rprofile: None,
            mprofile: Some(crate::board::mprofile::Config {
                clock_hz: 1_000_000_000,
                board_ticks_per_second: 1_000_000_000,
                vector_base: 0,
            }),
        }],
        dma_mappings: vec![memory::BusMapping::ram(0, 0x4000)],
        ram: vec![Ram {
            base: 0,
            size: 0x4000,
        }],
        mappings: vec![MemoryMap {
            core: 0,
            address: 0,
            physical: 0,
            size: 0x4000,
            permissions: 7,
        }],
        peripherals: vec![],
        instructions_per_tick: Ratio::new(7, 3).unwrap(),
    };
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(0x8000000, 4, device).unwrap();
    let mut board = graph
        .build(
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            None,
        )
        .unwrap();
    board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
    let vectors = [0x3f00u32, 0x2001];
    board
        .write_memory(
            0,
            0,
            &vectors
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board
        .write_memory(
            0,
            0x2000,
            &code
                .iter()
                .flat_map(|h| h.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board.set_loop_mode(mode).unwrap();
    board
}
const LSLS_31: u16 = 0x07c9;
/// Board state (virtual time, instruction counts, NVIC and devices), every
/// core register and the polled RAM. Durable CPU state is not available for
/// the M-profile backend.
fn m4_state(board: &Board) -> (Value, Vec<u32>, Vec<u8>) {
    let registers = arm::CORE
        .iter()
        .chain([arm::CPSR, Register::Pc].iter())
        .map(|&r| board.read_register(0, r).unwrap() as u32)
        .collect();
    let mut ram = vec![0; 0x10];
    board.cpu(0).unwrap().read_memory(0x3000, &mut ram).unwrap();
    (board.state(), registers, ram)
}
fn m4_statistic(board: &Board, name: &str) -> u64 {
    board.loop_statistics().unwrap()[0][name]
        .as_u64()
        .unwrap_or(0)
}
#[test]
fn m4_polls_and_delays_stop_at_device_deadlines_with_rational_time() {
    for mmio in [false, true] {
        for quantum in [1, 7, 1000] {
            let mut off = m4_board(mmio, true, LoopMode::Disabled, LSLS_31);
            let mut measured = m4_board(mmio, true, LoopMode::Measure, LSLS_31);
            let mut on = m4_board(mmio, true, LoopMode::Accelerate, LSLS_31);
            for tick in [9, 28, 52, 53, 54, 100] {
                for board in [&mut off, &mut measured, &mut on] {
                    board.run_until(tick, quantum).unwrap();
                }
                let right = m4_state(&off);
                for (name, left) in [("measure", &measured), ("accelerate", &on)] {
                    assert_eq!(
                        m4_state(left),
                        right,
                        "{name} mmio={mmio} quantum={quantum} tick={tick}"
                    );
                }
            }
            assert_eq!(on.read_register(0, arm::CORE[7]).unwrap(), 0x77);
            assert_eq!(m4_statistic(&measured, "loop_accelerated_instructions"), 0);
            assert!(m4_statistic(&on, "loop_accelerated_instructions") > 0);
            assert!(m4_statistic(&on, "loop_counted_instructions") > 0);
            assert!(m4_statistic(&on, "loop_poll_instructions") > 0);
        }
    }
}
#[test]
fn m4_undeclared_device_reads_and_stores_never_accelerate_the_poll() {
    // An undeclared register, and a poll that stores (STR R1,[R2,#4]).
    for (mmio, declared, poll, failure) in [
        (
            true,
            false,
            LSLS_31,
            "loop_proofs_failed_loop_read_unavailable",
        ),
        (false, true, 0x6051, "loop_proofs_failed_structural"),
    ] {
        let mut off = m4_board(mmio, declared, LoopMode::Disabled, poll);
        let mut on = m4_board(mmio, declared, LoopMode::Accelerate, poll);
        for board in [&mut off, &mut on] {
            board.run_until(50, 1000).unwrap();
        }
        assert_eq!(
            m4_state(&on),
            m4_state(&off),
            "mmio={mmio} declared={declared}"
        );
        assert_eq!(m4_statistic(&on, "loop_poll_instructions"), 0, "{failure}");
        assert!(m4_statistic(&on, failure) > 0, "{failure}");
        // The counted delay before the poll still accelerates.
        assert!(m4_statistic(&on, "loop_counted_instructions") > 0);
    }
}

/// A one-shot alarm: a write arms a deadline that many ticks ahead; at the
/// deadline it sets the RAM flag at 0x3000.
struct Alarm {
    due: Option<u64>,
}
impl Device for Alarm {
    fn name(&self) -> &'static str {
        "test-alarm"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, value: u64) {
        self.due = Some(cx.now() + value);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.due
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.due.is_some_and(|due| due <= cx.now()) {
            self.due = None;
            cx.ram_write(0x3000, &1u32.to_le_bytes()).unwrap();
        }
    }
    fn save(&self) -> Value {
        json!({"due":self.due})
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode("test-alarm", &self.save())
    }
}
#[test]
fn m4_windows_end_at_a_deadline_the_core_schedules_during_its_run() {
    // Arm the alarm 9 ticks ahead, then poll the RAM flag it sets. The run
    // began with no deadline pending, so only the alarm, scheduled by the
    // core's own store during the run, bounds the window.
    let code = [
        0x2201, // MOVS R2,#1
        0x06d2, // LSLS R2,R2,#27
        0x2109, // MOVS R1,#9
        0x6011, // STR  R1,[R2]
        0x2303, // MOVS R3,#3
        0x031b, // LSLS R3,R3,#12
        0x6819, // LDR  R1,[R3]
        0x07c9, // LSLS R1,R1,#31
        0xd5fc, // BPL  0x200c
        0x2777, // MOVS R7,#0x77
        0xbf30, // WFI
    ];
    for quantum in [1, 7, 1000] {
        let mut off = m4_board_with(Box::new(Alarm { due: None }), &code, LoopMode::Disabled);
        let mut on = m4_board_with(Box::new(Alarm { due: None }), &code, LoopMode::Accelerate);
        // One run spans the arming store and the alarm's deadline.
        for tick in [40, 60] {
            off.run_until(tick, quantum).unwrap();
            on.run_until(tick, quantum).unwrap();
            assert_eq!(
                m4_state(&on),
                m4_state(&off),
                "quantum={quantum} tick={tick}"
            );
        }
        assert_eq!(on.read_register(0, arm::CORE[7]).unwrap(), 0x77);
        assert!(m4_statistic(&on, "loop_accelerated_instructions") > 0);
    }
}

/// Raises NVIC line 0 of core 1 on any write.
struct Doorbell;
impl Device for Doorbell {
    fn name(&self) -> &'static str {
        "test-doorbell"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.set_core_irq(1, 0, true).unwrap();
    }
    fn save(&self) -> Value {
        json!({})
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode("test-doorbell", &self.save())
    }
}
/// Two Cortex-M4 cores. Core 1 enables NVIC line 0 with interrupts masked and
/// waits in WFI. Core 0 rings the doorbell, which wakes core 1 during core 0's
/// run, then polls the RAM flag core 1 sets after a counted delay.
fn m4_pair(mode: LoopMode) -> Board {
    use crate::board::factory::{Graph, MemoryMap, Options, Processor, Ram};
    use fujimod_catalog_configuration::RunEngine;
    let processor = |vector_base| Processor {
        cpu_backend: Default::default(),
        model: Model::CortexM4,
        registers: vec![],
        released: true,
        parked_until_sgi: false,
        rprofile: None,
        mprofile: Some(crate::board::mprofile::Config {
            clock_hz: 1_000_000_000,
            board_ticks_per_second: 1_000_000_000,
            vector_base,
        }),
    };
    let ram = |core| MemoryMap {
        core,
        address: 0,
        physical: 0,
        size: 0x4000,
        permissions: 7,
    };
    let graph = Graph {
        decoders: vec![],
        processors: vec![processor(0), processor(0x1000)],
        dma_mappings: vec![memory::BusMapping::ram(0, 0x4000)],
        ram: vec![Ram {
            base: 0,
            size: 0x4000,
        }],
        mappings: vec![ram(0), ram(1)],
        peripherals: vec![],
        instructions_per_tick: Ratio::new(7, 3).unwrap(),
    };
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(0x8000000, 4, Box::new(Doorbell)).unwrap();
    let mut board = graph
        .build(
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            None,
        )
        .unwrap();
    board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
    let words = |board: &mut Board, address, values: &[u32]| {
        board
            .write_memory(
                0,
                address,
                &values
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    };
    words(&mut board, 0, &[0x3f00, 0x2001]);
    words(&mut board, 0x1000, &[0x3e00, 0x2101]);
    let halfwords = |board: &mut Board, address, code: &[u16]| {
        board
            .write_memory(
                0,
                address,
                &code
                    .iter()
                    .flat_map(|h| h.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    };
    halfwords(
        &mut board,
        0x2000,
        &[
            0x201e, // MOVS R0,#30
            0x3801, // SUBS R0,#1
            0xd1fd, // BNE  0x2002
            0x2201, // MOVS R2,#1
            0x06d2, // LSLS R2,R2,#27
            0x6011, // STR  R1,[R2]   (doorbell)
            0x2303, // MOVS R3,#3
            0x031b, // LSLS R3,R3,#12
            0x6819, // LDR  R1,[R3]
            0x07c9, // LSLS R1,R1,#31
            0xd5fc, // BPL  0x2010
            0x2777, // MOVS R7,#0x77
            0xbf30, // WFI
        ],
    );
    halfwords(
        &mut board,
        0x2100,
        &[
            0xb672, // CPSID I
            0x20e0, // MOVS R0,#0xe0
            0x0600, // LSLS R0,R0,#24
            0x21e1, // MOVS R1,#0xe1
            0x0209, // LSLS R1,R1,#8
            0x1840, // ADDS R0,R0,R1  (NVIC ISER0)
            0x2101, // MOVS R1,#1
            0x6001, // STR  R1,[R0]
            0xbf30, // WFI
            0x2032, // MOVS R0,#50
            0x3801, // SUBS R0,#1
            0xd1fd, // BNE  0x2114
            0x2203, // MOVS R2,#3
            0x0312, // LSLS R2,R2,#12
            0x2101, // MOVS R1,#1
            0x6011, // STR  R1,[R2]
            0xe7fe, // B    .
        ],
    );
    board.set_loop_mode(mode).unwrap();
    board
}
fn m4_pair_state(board: &Board) -> (Value, Vec<u32>, Vec<u8>) {
    let registers = (0..2)
        .flat_map(|core| {
            arm::CORE
                .iter()
                .chain([arm::CPSR, Register::Pc].iter())
                .map(move |&r| board.read_register(core, r).unwrap() as u32)
        })
        .collect();
    let mut ram = vec![0; 0x10];
    board.cpu(0).unwrap().read_memory(0x3000, &mut ram).unwrap();
    (board.state(), registers, ram)
}
#[test]
fn m4_windows_end_at_the_slice_of_a_peer_woken_during_the_run() {
    for quantum in [1, 7, 64, 1000] {
        let mut off = m4_pair(LoopMode::Disabled);
        let mut on = m4_pair(LoopMode::Accelerate);
        for tick in [20, 60, 120, 200, 1200] {
            off.run_until(tick, quantum).unwrap();
            on.run_until(tick, quantum).unwrap();
            assert_eq!(
                m4_pair_state(&on),
                m4_pair_state(&off),
                "quantum={quantum} tick={tick}"
            );
        }
        assert_eq!(
            on.read_register(0, arm::CORE[7]).unwrap(),
            0x77,
            "quantum={quantum}"
        );
        assert!(
            m4_statistic(&on, "loop_accelerated_instructions") > 0,
            "quantum={quantum}"
        );
    }
}

/// Cortex-M4 core 0 drives a loopback PL022 at 0x8000000: it writes a byte to
/// DR, polls SR.RNE with the X-H2S sequence (LDRH; AND.W #4; UXTH; CMP; BEQ),
/// reads DR and repeats. With `peer`, core 1 runs a counted delay and reads
/// CR0 forever. Its device read stalls it to the frontier, so core 0 resumes
/// behind the frontier and its windows are bounded by slices as well as by
/// serial deadlines.
fn m4_ssp(mode: LoopMode, peer: bool) -> Board {
    use crate::board::factory::{Graph, MemoryMap, Options, Processor, Ram};
    use fujimod_catalog_configuration::RunEngine;
    use fujimod_emulator_ip_arm::pl022::{Clock, Controller};
    let processor = |vector_base| Processor {
        cpu_backend: Default::default(),
        model: Model::CortexM4,
        registers: vec![],
        released: true,
        parked_until_sgi: false,
        rprofile: None,
        mprofile: Some(crate::board::mprofile::Config {
            clock_hz: 1_000_000_000,
            board_ticks_per_second: 1_000_000_000,
            vector_base,
        }),
    };
    let ram = |core| MemoryMap {
        core,
        address: 0,
        physical: 0,
        size: 0x4000,
        permissions: 7,
    };
    let cores = if peer { 2 } else { 1 };
    let graph = Graph {
        decoders: vec![],
        processors: [processor(0), processor(0x1000)]
            .into_iter()
            .take(cores)
            .collect(),
        dma_mappings: vec![memory::BusMapping::ram(0, 0x4000)],
        ram: vec![Ram {
            base: 0,
            size: 0x4000,
        }],
        mappings: (0..cores).map(ram).collect(),
        peripherals: vec![],
        instructions_per_tick: Ratio::new(7, 3).unwrap(),
    };
    let ssp = Controller::new(
        0x8000000,
        Clock {
            source_hz: 48_000_000,
            ticks_per_second: 1_000_000_000,
        },
        vec![],
        Box::new(fujimod_emulator_parts_io::serial::Unconnected),
    )
    .unwrap();
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(0x8000000, 0x1000, Box::new(ssp)).unwrap();
    let mut board = graph
        .build(
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            None,
        )
        .unwrap();
    for core in 0..cores {
        board.map_mmio(core, 0x8000000, 0x1000, 0x8000000).unwrap();
    }
    let write = |board: &mut Board, address, bytes: Vec<u8>| {
        board.write_memory(0, address, &bytes).unwrap();
    };
    let words = |values: &[u32]| {
        values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    };
    let halfwords = |code: &[u16]| {
        code.iter()
            .flat_map(|h| h.to_le_bytes())
            .collect::<Vec<_>>()
    };
    write(&mut board, 0, words(&[0x3f00, 0x2001]));
    write(&mut board, 0x1000, words(&[0x3e00, 0x2101]));
    write(
        &mut board,
        0x2000,
        halfwords(&[
            0x2201, // MOVS R2,#1
            0x06d2, // LSLS R2,R2,#27
            0x21c7, // MOVS R1,#0xc7      (8 bits, Motorola mode 3)
            0x6011, // STR  R1,[R2]       CR0
            0x2106, // MOVS R1,#6
            0x6111, // STR  R1,[R2,#0x10] CPSR
            0x2103, // MOVS R1,#3
            0x6051, // STR  R1,[R2,#4]    CR1 = SSE | LBM
            0x215a, // MOVS R1,#0x5a      (0x2010)
            0x6091, // STR  R1,[R2,#8]    DR
            0x8995, // LDRH R5,[R2,#0xc]  SR (0x2014)
            0xf005, 0x0504, // AND.W R5,R5,#4
            0xb2ad, // UXTH R5,R5
            0x2d00, // CMP  R5,#0
            0xd0f9, // BEQ  0x2014
            0x6891, // LDR  R1,[R2,#8]    DR
            0x3601, // ADDS R6,#1
            0xe7f4, // B    0x2010
        ]),
    );
    write(
        &mut board,
        0x2100,
        halfwords(&[
            0x2201, // MOVS R2,#1
            0x06d2, // LSLS R2,R2,#27
            0x2032, // MOVS R0,#50       (0x2104)
            0x3801, // SUBS R0,#1
            0xd1fd, // BNE  0x2106
            0x6813, // LDR  R3,[R2]      CR0
            0xe7fa, // B    0x2104
        ]),
    );
    board.set_loop_mode(mode).unwrap();
    board
}
fn m4_ssp_state(board: &Board, cores: usize) -> (Value, Vec<u32>) {
    let registers = (0..cores)
        .flat_map(|core| {
            arm::CORE
                .iter()
                .chain([arm::CPSR, Register::Pc].iter())
                .map(move |&r| board.read_register(core, r).unwrap() as u32)
        })
        .collect();
    (board.state(), registers)
}
#[test]
fn m4_ssp_receive_polls_match_instruction_execution() {
    for peer in [false, true] {
        let cores = if peer { 2 } else { 1 };
        for quantum in [7, 100, 1000] {
            let mut off = m4_ssp(LoopMode::Disabled, peer);
            let mut on = m4_ssp(LoopMode::Accelerate, peer);
            for tick in (1..=20).map(|i| i * 997) {
                off.run_until(tick, quantum).unwrap();
                on.run_until(tick, quantum).unwrap();
                assert_eq!(
                    m4_ssp_state(&on, cores),
                    m4_ssp_state(&off, cores),
                    "peer={peer} quantum={quantum} tick={tick}"
                );
            }
            assert!(
                on.read_register(0, arm::CORE[6]).unwrap() > 10,
                "peer={peer}"
            );
            assert!(
                m4_statistic(&on, "loop_poll_instructions") > 0,
                "peer={peer} quantum={quantum}"
            );
        }
    }
}
