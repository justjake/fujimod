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
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.0.lock().unwrap().due
    }
    fn poll_stable(&self, _: u64, _: u8) -> bool {
        self.1
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
    0xf240, 0x0000, 0xf6c0, 0x0000, 0x6803, 0x6001, 0x6802, 0x429a, 0xd0fc, 0x0013, 0x3401,
    0xe7f8,
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
    left_out: u64,
}

fn run(quantum: usize, blocks: bool, windows: &[u64]) -> Outcome {
    run_with(quantum, blocks, windows, false, true)
}

fn run_with(quantum: usize, blocks: bool, windows: &[u64], stable: bool, fast_forward: bool) -> Outcome {
    use factory::*;
    let arm32 = |r| Register::Arm32(fujimod_emulator_cpu::arm::Register::R(r));
    let graph = Graph {
        decoders: vec![],
        processors: vec![Processor {
            model: Model::CortexM4,
            registers: vec![],
            released: true,
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
    board.set_block_execution(blocks);
    board.set_poll_loops(fast_forward);
    let words = |words: &[u32]| words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
    board.write_physical(0x10000, &words(&[0x17000, 0x10101])).unwrap();
    board
        .write_physical(
            0x10100,
            &PROGRAM.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>(),
        )
        .unwrap();
    for &window in windows {
        board.run_until(window, quantum).unwrap();
    }
    let m4 = board.cpu(0).unwrap();
    let mut registers: Vec<u64> = (0..5).map(|r| scalar(m4, arm32(r)).unwrap()).collect();
    registers.push(scalar(m4, Register::Pc).unwrap());
    let s = board.state.borrow();
    Outcome {
        state: board.state(),
        registers,
        skips: s.settled.skips,
        full: s.settled.full,
        accesses: board.mmio_callback_counts.borrow()[0],
        served: probe.lock().unwrap().count,
        left_out: s.blocks[0].as_ref().map_or(0, |blocks| blocks.poll_reads),
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
                assert_eq!(got.registers, reference.registers, "quantum {quantum} blocks {blocks}");
                assert!(got.state == reference.state, "quantum {quantum} blocks {blocks}: board state differs");
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
        for windows in [&[5000][..], &[333, 334, 2111, 5000][..], &[1, 2, 3, 4, 5, 6, 7, 5000][..]] {
            let got = run_with(quantum, true, windows, true, true);
            assert_eq!(got.registers, reference.registers, "quantum {quantum}");
            assert!(got.state == reference.state, "quantum {quantum}: board state differs");
            // Every read counts as serviced; the device served a few per wait.
            assert_eq!(got.accesses, reference.accesses, "quantum {quantum}");
            assert_eq!(got.served + got.left_out, reference.served, "quantum {quantum}");
            assert!(
                got.left_out > 9 * got.served,
                "quantum {quantum}: served {}, left out {}",
                got.served,
                got.left_out
            );
            // Switched off, or with the instruction observer, every read is served.
            for (blocks, fast_forward) in [(true, false), (false, true)] {
                let all = run_with(quantum, blocks, windows, true, fast_forward);
                assert!(all.state == reference.state);
                assert_eq!((all.served, all.left_out), (reference.served, 0));
            }
        }
    }
}
