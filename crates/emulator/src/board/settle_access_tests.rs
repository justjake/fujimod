//! A device access of an M-profile processor that changes no interrupt input
//! leaves the interrupt bookkeeping settled. The observer entries after it
//! skip the pin push and the deadline refresh; debug builds recompute every
//! skipped call and refuse a difference. The device sees the same accesses
//! at the same instants in block execution, with the instruction observer,
//! and in one run window or several.
//!
//! When the device declares the polled register stable between events, block
//! execution fast-forwards the poll loop. The processor then reaches every
//! instant, register value and access count it reaches without that.
use super::*;
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::Model;
use fujimod_emulator_devices::{Cx, Device};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Probe {
    due: Option<u64>,
    status: u64,
    /// Per status value: the value and the instant of its first read.
    reads: Vec<(u64, u64)>,
    /// All reads; not part of the saved state.
    count: u64,
    publish: bool,
    read_ticks: Vec<u64>,
}
/// A write starts a transfer that ends 300 to 302 ticks later; the status register
/// then reads one more. Reads have no effect.
struct Transfer(Arc<Mutex<Probe>>, bool);
impl Device for Transfer {
    fn name(&self) -> &'static str {
        "test-transfer"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        let mut p = self.0.lock().unwrap();
        let status = p.status;
        let now = cx.now();
        p.count += 1;
        p.read_ticks.push(now);
        if p.reads.last().is_none_or(|&(value, _)| value != status) {
            p.reads.push((status, now));
        }
        status
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        // The length varies so that the end falls on every phase of a
        // three-instruction poll loop, also exactly on a read.
        let mut p = self.0.lock().unwrap();
        p.due = Some(cx.now() + 300 + p.status % 3);
    }
    fn fast_reads(&self, _: u64, _: u64) -> Vec<fujimod_emulator_devices::bus::FastReadWord> {
        if !self.0.lock().unwrap().publish {
            return Vec::new();
        }
        let probe = self.0.clone();
        vec![fujimod_emulator_devices::bus::FastReadWord::clocked_u32(
            0,
            Arc::new(move |tick| {
                let mut probe = probe.lock().unwrap();
                // The first read of each new state belongs to the device.
                if tick >= probe.due? || probe.reads.last()?.0 != probe.status {
                    return None;
                }
                probe.read_ticks.push(tick);
                Some(probe.status as u32)
            }),
        )]
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.0.lock().unwrap().due
    }
    fn poll_stable(&self, _: u64, _: u8) -> bool {
        self.1
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        let p = self.0.lock().unwrap();
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(p.due, p.status, &p.reads),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let _: (Option<u64>, u64, Vec<(u64, u64)>) = saved.decode(self.name())?;
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let mut p = self.0.lock().unwrap();
        (p.due, p.status, p.reads) = saved.decode(self.name())?;
        Ok(())
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut p = self.0.lock().unwrap();
        if p.due.is_some_and(|at| at <= cx.now()) {
            p.due = None;
            p.status += 1;
        }
    }
    fn save(&self) -> Value {
        let p = self.0.lock().unwrap();
        json!({"due": p.due, "status": p.status, "reads": p.reads})
    }
}

/// movw/movt r0,#0x08000000; ldr r3,[r0]; loop: str r1,[r0]; poll: ldr
/// r2,[r0]; cmp r2,r3; beq poll; movs r3,r2; adds r4,#1; b loop
const PROGRAM: [u16; 12] = [
    0xf240, 0x0000, 0xf6c0, 0x0000, 0x6803, 0x6001, 0x6802, 0x429a, 0xd0fc, 0x0013, 0x3401, 0xe7f8,
];

struct Outcome {
    state: Value,
    registers: Vec<u64>,
    /// Observer entries that skipped, and that ran, the bookkeeping.
    skips: u64,
    full: u64,
    /// Device accesses the board counts, reads the device served, and reads
    /// a fast-forward left out.
    accesses: u64,
    served: u64,
    read_ticks: Vec<u64>,
    left_out: u64,
    /// The portable checkpoint of the final state.
    checkpoint: Vec<u8>,
    /// Exact board access histogram calls and the published subset.
    register_access_calls: u64,
    published_fast_read_hits: u64,
}

fn run(quantum: usize, blocks: bool, windows: &[u64]) -> Outcome {
    run_with(
        quantum,
        blocks,
        windows,
        false,
        true,
        ExecutionPolicy::Exact,
        false,
    )
}

fn run_with(
    quantum: usize,
    blocks: bool,
    windows: &[u64],
    stable: bool,
    fast_forward: bool,
    policy: ExecutionPolicy,
    publish: bool,
) -> Outcome {
    run_with_diagnostic(
        quantum,
        blocks,
        windows,
        stable,
        fast_forward,
        policy,
        publish,
        false,
    )
}

fn run_with_diagnostic(
    quantum: usize,
    blocks: bool,
    windows: &[u64],
    stable: bool,
    fast_forward: bool,
    policy: ExecutionPolicy,
    publish: bool,
    access_histogram: bool,
) -> Outcome {
    use factory::*;
    let arm32 = |r| Register::Arm32(fujimod_emulator_cpu::arm::Register::R(r));
    let graph = Graph {
        decoders: vec![],
        processors: vec![Processor {
            model: Model::CortexM4,
            registers: vec![],
            released: true,
            parked_until_sgi: false,
            rprofile: None,
            mprofile: Some(super::mprofile::Config {
                clock_hz: 1_000_000_000,
                board_ticks_per_second: 1_000_000_000,
                vector_base: 0x10000,
            }),
        }],
        dma_mappings: vec![crate::board::memory::BusMapping::ram(0x10000, 0x8000)],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x8000,
        }],
        mappings: vec![MemoryMap {
            core: 0,
            address: 0x10000,
            physical: 0x10000,
            size: 0x8000,
            permissions: 7,
        }],
        peripherals: vec![Peripheral {
            core: 0,
            address: 0x8000000,
            physical: 0xff910000,
            size: 0x1000,
        }],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    };
    let probe = Arc::new(Mutex::new(Probe {
        publish,
        ..Probe::default()
    }));
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(0xff910000, 4, Box::new(Transfer(probe.clone(), stable)))
        .unwrap();
    let mut board = graph
        .build(
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            None,
        )
        .unwrap();
    if access_histogram {
        let statistics = super::access_histogram::new_for_test();
        board.access_histogram = Some(statistics.clone());
        board.state.borrow_mut().access_histogram = Some(statistics);
    }
    board.set_execution_policy(policy).unwrap();
    board.set_block_execution(blocks);
    board.set_poll_loops(fast_forward);
    let words = |words: &[u32]| {
        words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>()
    };
    board
        .write_physical(0x10000, &words(&[0x17000, 0x10101]))
        .unwrap();
    board
        .write_physical(
            0x10100,
            &PROGRAM
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
    for &window in windows {
        board.run_until(window, quantum).unwrap();
    }
    let m4 = board.cpu(0).unwrap();
    let mut registers: Vec<u64> = (0..5).map(|r| scalar(m4, arm32(r)).unwrap()).collect();
    registers.push(scalar(m4, Register::Pc).unwrap());
    let checkpoint = board
        .checkpoint(None)
        .unwrap()
        .export()
        .unwrap()
        .to_bytes()
        .unwrap();
    let (register_access_calls, published_fast_read_hits) =
        board.register_access_snapshot().map_or((0, 0), |snapshot| {
            let calls = snapshot
                .totals
                .values()
                .fold(0u64, |total, access| total.saturating_add(access.calls));
            let published = snapshot
                .published_fast_read_hits
                .values()
                .fold(0u64, |total, &hits| total.saturating_add(hits));
            (calls, published)
        });
    let s = board.state.borrow();
    let state = board.state();
    let probe = probe.lock().unwrap();
    Outcome {
        state,
        registers,
        skips: s.settled.skips,
        full: s.settled.full,
        accesses: board.mmio_callback_counts.borrow()[0],
        served: probe.count,
        read_ticks: probe.read_ticks.clone(),
        left_out: s.blocks[0].as_ref().map_or(0, |blocks| blocks.poll_reads),
        checkpoint,
        register_access_calls,
        published_fast_read_hits,
    }
}

#[test]
fn a_status_poll_keeps_the_bookkeeping_settled_with_the_same_result() {
    for quantum in [1, 7, 100] {
        let reference = run(quantum, false, &[5000]);
        // The loop finished transfers: each takes about 300 ticks.
        let rounds = reference.registers[4];
        assert!((14..=17).contains(&rounds), "rounds {rounds}");
        assert_eq!(reference.accesses, reference.served + rounds + 1);
        for windows in [&[5000][..], &[333, 334, 2111, 5000][..]] {
            for blocks in [false, true] {
                let got = run(quantum, blocks, windows);
                assert_eq!(
                    got.registers, reference.registers,
                    "quantum {quantum} blocks {blocks}"
                );
                assert!(
                    got.state == reference.state,
                    "quantum {quantum} blocks {blocks}: board state differs"
                );
                assert_eq!(got.accesses, reference.accesses);
                assert_eq!((got.served, got.left_out), (reference.served, 0));
                // Almost every entry follows a status read. Without the
                // settled access each of them ran the full bookkeeping.
                assert!(
                    got.skips > 8 * got.full,
                    "quantum {quantum} blocks {blocks}: settled {}, full {}",
                    got.skips,
                    got.full
                );
            }
        }
    }
}

#[test]
fn a_poll_loop_on_a_stable_register_is_fast_forwarded_with_the_same_result() {
    for quantum in [1, 7, 100] {
        let reference = run(quantum, false, &[5000]);
        for windows in [
            &[5000][..],
            &[333, 334, 2111, 5000][..],
            &[1, 2, 3, 4, 5, 6, 7, 5000][..],
        ] {
            let got = run_with(
                quantum,
                true,
                windows,
                true,
                true,
                ExecutionPolicy::Exact,
                false,
            );
            assert_eq!(got.registers, reference.registers, "quantum {quantum}");
            assert!(
                got.state == reference.state,
                "quantum {quantum}: board state differs"
            );
            // Every read counts as serviced; the device served a few per wait.
            assert_eq!(got.accesses, reference.accesses, "quantum {quantum}");
            assert_eq!(
                got.served + got.left_out,
                reference.served,
                "quantum {quantum}"
            );
            assert!(
                got.left_out > 9 * got.served,
                "quantum {quantum}: served {}, left out {}",
                got.served,
                got.left_out
            );
            // Switched off, or with the instruction observer, every read is served.
            for (blocks, fast_forward) in [(true, false), (false, true)] {
                let all = run_with(
                    quantum,
                    blocks,
                    windows,
                    true,
                    fast_forward,
                    ExecutionPolicy::Exact,
                    false,
                );
                assert!(all.state == reference.state);
                assert_eq!((all.served, all.left_out), (reference.served, 0));
                // The portable checkpoint is the same bytes with and without
                // the fast-forward.
                if blocks {
                    assert!(
                        all.checkpoint == got.checkpoint,
                        "quantum {quantum}: checkpoint differs"
                    );
                }
            }
        }
    }
}

#[test]
fn interactive_stable_poll_preserves_the_checkpoint_and_read_count() {
    let windows = &[333, 334, 2111, 5000];
    let policy = ExecutionPolicy::Interactive;
    let reference = run_with(100, true, windows, true, false, policy, false);
    let accelerated = run_with(100, true, windows, true, true, policy, false);
    assert!(accelerated.left_out > 0);
    assert_eq!(accelerated.accesses, reference.accesses);
    assert_eq!(accelerated.served + accelerated.left_out, reference.served);
    assert!(accelerated.checkpoint == reference.checkpoint);
}

#[test]
fn m4_published_reads_observe_the_same_bus_ticks_across_transfer_deadlines() {
    let windows = &[300, 301, 625, 2111, 5000];
    for blocks in [false, true] {
        let run = |policy, publish| run_with(63, blocks, windows, false, false, policy, publish);
        let reference = run(ExecutionPolicy::Interactive, false);
        let published = run(ExecutionPolicy::Interactive, true);
        assert_eq!(published.read_ticks, reference.read_ticks);
        assert_eq!(published.registers, reference.registers);
        assert_eq!(published.accesses, reference.accesses);
        assert_eq!(published.checkpoint, reference.checkpoint);
        assert!(published.served * 10 < reference.served);
        assert_eq!(run(ExecutionPolicy::Exact, true).served, reference.served);
    }
}

#[test]
fn access_histogram_calls_match_board_callbacks_and_published_subset() {
    let windows = &[300, 301, 625, 2111, 5000];
    let reference = run_with_diagnostic(
        63,
        false,
        windows,
        false,
        false,
        ExecutionPolicy::Interactive,
        false,
        true,
    );
    let published = run_with_diagnostic(
        63,
        false,
        windows,
        false,
        false,
        ExecutionPolicy::Interactive,
        true,
        true,
    );
    assert_eq!(reference.register_access_calls, reference.accesses);
    assert_eq!(published.register_access_calls, published.accesses);
    assert_eq!(
        published.published_fast_read_hits,
        reference.served - published.served
    );
    assert!(published.published_fast_read_hits <= published.register_access_calls);

    let fast_forwarded = run_with_diagnostic(
        100,
        true,
        &[333, 334, 2111, 5000],
        true,
        true,
        ExecutionPolicy::Exact,
        false,
        true,
    );
    assert_eq!(
        fast_forwarded.register_access_calls + fast_forwarded.left_out,
        fast_forwarded.accesses
    );
}

#[test]
fn store_observer_counts_explicit_write_watch_hits_at_the_m4_store_hook() {
    use factory::*;

    let graph = Graph {
        decoders: vec![],
        processors: vec![Processor {
            model: Model::CortexM4,
            registers: vec![],
            released: true,
            parked_until_sgi: false,
            rprofile: None,
            mprofile: Some(super::mprofile::Config {
                clock_hz: 1_000_000_000,
                board_ticks_per_second: 1_000_000_000,
                vector_base: 0x10000,
            }),
        }],
        dma_mappings: vec![crate::board::memory::BusMapping::ram(0x10000, 0x8000)],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x8000,
        }],
        mappings: vec![MemoryMap {
            core: 0,
            address: 0x10000,
            physical: 0x10000,
            size: 0x8000,
            permissions: 7,
        }],
        peripherals: vec![],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    };
    let mut board = graph
        .build(
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            Bus::new(bus::Mode::Strict),
            None,
        )
        .unwrap();
    board.state.borrow_mut().store_observer = Some(vec![Default::default()]);
    let callback_service_time = super::callback_timing::new_for_test(1);
    board.state.borrow_mut().callback_service_time = Some(callback_service_time.clone());
    board.bindings[0].callback_service_time = Some(callback_service_time);
    board.set_execution_policy(ExecutionPolicy::Exact).unwrap();
    board
        .write_physical(0x10000, &[0x00, 0x70, 0x01, 0x00, 0x01, 0x01, 0x01, 0x00])
        .unwrap();
    let halfwords = [0x4801u16, 0x4902, 0x6001, 0xe7fe];
    let mut program = halfwords
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    program.extend_from_slice(&0x12000u32.to_le_bytes());
    program.extend_from_slice(&0x1234_5678u32.to_le_bytes());
    board.write_physical(0x10100, &program).unwrap();
    *board.state.borrow().memory.write_watch.borrow_mut() =
        Some(super::write_watch::Watch::new(&[(0x12000, 4)]).unwrap());

    board.run_until(32, 64).unwrap();

    let metrics = board.execution_metrics();
    let observer = &metrics["cortex_m4_by_core"][0]["store_observer"];
    let stores = observer["callbacks_total"].as_u64().unwrap();
    assert!(stores > 0, "guest did not reach a RAM store");
    assert_eq!(observer["completed_callbacks"], stores);
    assert_eq!(observer["explicit_write_watch_overlaps"], stores);
    assert_eq!(observer["callbacks_with_any_observed_reason"], stores);
    assert_eq!(observer["no_recorded_reason"], 0);
    assert_eq!(observer["reason_counts_are_nonexclusive"], true);

    let callback_time = &metrics["cortex_m4_by_core"][0]["callback_service_time"];
    assert_eq!(callback_time["sampling_probability"]["denominator"], 1024);
    assert_eq!(callback_time["store"]["samples"], stores);
    assert_eq!(callback_time["block"]["samples"], 0);
    assert_eq!(callback_time["mmio"]["samples"], 0);

    let mut stored = [0; 4];
    board.read_physical(0x12000, &mut stored).unwrap();
    assert_eq!(u32::from_le_bytes(stored), 0x1234_5678);
}

/// An AArch64 processor polls the same device through a read helper:
/// 0x14000: ldr w20,[x19]; loop: str w1,[x19]; poll: mov x0,x19; bl read;
/// cmp w0,w20; b.eq poll; mov w20,w0; add x21,x21,#1; b loop.
/// 0x14040 read: ldr w0,[x0]; ret
fn run_a64(quantum: usize, windows: &[u64], stable: bool, fast_forward: bool) -> Outcome {
    use factory::*;
    let graph = Graph {
        decoders: vec![],
        processors: vec![Processor {
            model: Model::ArmV8A,
            registers: vec![
                (Register::Pc, 0x14000),
                (Register::Pstate, 0x3c5),
                (Register::X(19), 0x8000000),
            ],
            released: true,
            parked_until_sgi: false,
            rprofile: None,
            mprofile: None,
        }],
        dma_mappings: vec![crate::board::memory::BusMapping::ram(0x10000, 0x8000)],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x8000,
        }],
        mappings: vec![MemoryMap {
            core: 0,
            address: 0x10000,
            physical: 0x10000,
            size: 0x8000,
            permissions: 7,
        }],
        peripherals: vec![Peripheral {
            core: 0,
            address: 0x8000000,
            physical: 0xff910000,
            size: 0x4000,
        }],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    };
    let probe = Arc::new(Mutex::new(Probe::default()));
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(0xff910000, 4, Box::new(Transfer(probe.clone(), stable)))
        .unwrap();
    let mut board = graph
        .build(
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            None,
        )
        .unwrap();
    board.set_poll_loops(fast_forward);
    let words = |words: &[u32]| {
        words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>()
    };
    board
        .write_physical(
            0x14000,
            &words(&[
                0xb9400274, 0xb9000261, 0xaa1303e0, 0x9400000d, 0x6b14001f, 0x54ffffa0, 0x2a0003f4,
                0x910006b5, 0x17fffff9,
            ]),
        )
        .unwrap();
    board
        .write_physical(0x14040, &words(&[0xb9400000, 0xd65f03c0]))
        .unwrap();
    for &window in windows {
        board.run_until(window, quantum).unwrap();
    }
    let cpu = board.cpu(0).unwrap();
    let registers = [
        Register::X(0),
        Register::X(20),
        Register::X(21),
        Register::X(30),
        Register::Pc,
    ]
    .into_iter()
    .map(|r| scalar(cpu, r).unwrap())
    .collect();
    let checkpoint = board
        .checkpoint(None)
        .unwrap()
        .export()
        .unwrap()
        .to_bytes()
        .unwrap();
    let s = board.state.borrow();
    let state = board.state();
    let probe = probe.lock().unwrap();
    Outcome {
        state,
        registers,
        skips: s.settled.skips,
        full: s.settled.full,
        accesses: board.mmio_callback_counts.borrow()[0],
        served: probe.count,
        read_ticks: probe.read_ticks.clone(),
        left_out: s.polls.reads[0],
        checkpoint,
        register_access_calls: 0,
        published_fast_read_hits: 0,
    }
}

#[test]
fn an_aarch64_poll_loop_through_a_read_helper_is_fast_forwarded_with_the_same_result() {
    for quantum in [1, 7, 100] {
        let reference = run_a64(quantum, &[9000], true, false);
        let rounds = reference.registers[2];
        assert!((25..=31).contains(&rounds), "rounds {rounds}");
        assert_eq!(reference.left_out, 0);
        assert_eq!(reference.accesses, reference.served + rounds + 1);
        for windows in [
            &[9000][..],
            &[333, 334, 2111, 9000][..],
            &[1, 2, 3, 4, 5, 6, 7, 9000][..],
        ] {
            let got = run_a64(quantum, windows, true, true);
            assert_eq!(got.registers, reference.registers, "quantum {quantum}");
            assert!(
                got.state == reference.state,
                "quantum {quantum}: board state differs"
            );
            assert_eq!(got.accesses, reference.accesses, "quantum {quantum}");
            assert_eq!(
                got.served + got.left_out,
                reference.served,
                "quantum {quantum}"
            );
            assert!(
                got.left_out > 4 * got.served,
                "quantum {quantum}: served {}, left out {}",
                got.served,
                got.left_out
            );
            // A register that is not declared stable is read every round.
            let all = run_a64(quantum, windows, false, true);
            assert!(all.state == reference.state);
            assert_eq!((all.served, all.left_out), (reference.served, 0));
            // The portable checkpoint is the same bytes with and without the
            // fast-forward.
            let off = run_a64(quantum, windows, true, false);
            assert!(
                off.checkpoint == got.checkpoint,
                "quantum {quantum}: checkpoint differs"
            );
        }
    }
}
