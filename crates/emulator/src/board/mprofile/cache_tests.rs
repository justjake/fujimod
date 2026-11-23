use super::exclusive_tests::{board, board_with_bus};
use super::*;
use crate::board::bus::{Bus, Mode};
use crate::board::factory::{Graph, MemoryMap, Options, Peripheral, Processor, Ram};
use crate::board::memory::BusMapping;
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::{ExecutionPolicy, Model, timeline::Ratio};
use fujimod_emulator_devices::{Cx, Device};

#[test]
fn peer_and_host_code_writes_survive_partial_quanta() {
    for quantum in [1, 64] {
        let mut b = board(&[0xbf00, 0x2601, 0xbf30], &[0xbf30]);
        b.run_for_instructions(1, quantum).unwrap();
        b.write_memory(1, 0x4102, &0x2609u16.to_le_bytes()).unwrap();
        b.run_until(20, quantum).unwrap();
        assert_eq!(get(b.cpu(0).unwrap(), reg(6)).unwrap(), 9);
    }
    // Populate core0's translation before a real STRH through core1's alias.
    let mut b = board(&[0xbf00, 0xbf00, 0x2601, 0xbf30], &[0xbf00, 0x802c, 0xbf30]);
    b.run_for_instructions(2, 1).unwrap();
    set(b.cpu(1).unwrap(), reg(4), 0x2609).unwrap();
    set(b.cpu(1).unwrap(), reg(5), 0x4104).unwrap();
    b.run_until(20, 1).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(6)).unwrap(), 9);
}

#[test]
fn native_store_through_another_local_alias_replaces_cached_code() {
    let mut b = board(&[0xbf00, 0x802c, 0x2601, 0xbf30], &[0xbf30]);
    b.run_for_instructions(1, 64).unwrap();
    set(b.cpu(0).unwrap(), reg(4), 0x2609).unwrap();
    set(b.cpu(0).unwrap(), reg(5), 0x20104).unwrap();
    b.run_until(20, 64).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(6)).unwrap(), 9);
    assert_eq!(b.state()["instructions"], json!([4, 1]));
}

#[test]
fn emulated_exclusive_store_through_another_alias_replaces_cached_code() {
    let mut b = board(
        &[0xbf00, 0xe8d0, 0x1f5f, 0xe8c0, 0x3f52, 0x2601, 0xbf30],
        &[0xbf30],
    );
    b.run_for_instructions(1, 64).unwrap();
    set(b.cpu(0).unwrap(), reg(0), 0x2010a).unwrap();
    set(b.cpu(0).unwrap(), reg(3), 0x2609).unwrap();
    b.run_until(20, 64).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 0);
    assert_eq!(get(b.cpu(0).unwrap(), reg(6)).unwrap(), 9);
    assert_eq!(b.state()["instructions"], json!([5, 1]));
}

struct DmaCodeWrite(bool);
impl Device for DmaCodeWrite {
    fn name(&self) -> &'static str {
        "test-dma-code-write"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        (!self.0).then_some(3)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.0 && cx.now() >= 3 {
            self.0 = true;
            if let Err(e) = cx.ram_write(0x14106, &0x2609u16.to_le_bytes()) {
                cx.fail(e);
            }
        }
    }
}
#[test]
fn due_dma_code_write_restarts_the_current_peer_without_counting_stale_code() {
    for quantum in [1, 64] {
        let mut bus = Bus::new(Mode::Strict);
        bus.attach(Box::new(DmaCodeWrite(false)));
        let mut b = board_with_bus(&[0xbf30], &[0xbf00, 0xbf00, 0xbf00, 0x2601, 0xbf30], bus);
        b.run_until(20, quantum).unwrap();
        assert_eq!(get(b.cpu(1).unwrap(), reg(6)).unwrap(), 9);
        assert_eq!(b.state()["instructions"], json!([1, 5]));
    }
}

struct ForeignDma(bool);
impl Device for ForeignDma {
    fn name(&self) -> &'static str {
        "test-foreign-dma"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        if self.0 {
            if let Err(error) = cx.ram_write(0x11000, &[0x5a]) {
                cx.fail(error);
            }
        }
    }
}

fn run_foreign_dma(
    policy: ExecutionPolicy,
    retain_shadow: bool,
    dma_enabled: bool,
) -> (u64, String, [u8; 1]) {
    let mut bus = Bus::new(Mode::Strict);
    bus.register(0x50000, 4, Box::new(ForeignDma(dma_enabled)))
        .unwrap();
    let graph = Graph {
        processors: vec![Processor {
            model: Model::CortexM4,
            registers: vec![(reg(0), 0), (reg(1), 0x50000)],
            released: true,
            parked_until_sgi: false,
            mprofile: Some(super::Config {
                clock_hz: 600_000_000,
                board_ticks_per_second: 1_000_000_000,
                vector_base: 0x10000,
            }),
            rprofile: None,
        }],
        ram: vec![
            Ram {
                base: 0x10000,
                size: 0x8000,
            },
            Ram {
                base: 0x30000,
                size: 0x1000,
            },
        ],
        dma_mappings: vec![BusMapping {
            // The bus address looks like M4 RAM, but decodes to separate RAM.
            address: 0x11000,
            physical: 0x30000,
            size: 0x1000,
            permissions: 3,
        }],
        decoders: vec![],
        mappings: vec![
            MemoryMap {
                core: 0,
                address: 0x10000,
                physical: 0x10000,
                size: 0x8000,
                permissions: 7,
            },
            MemoryMap {
                core: 0,
                address: 0x20000,
                physical: 0x10000,
                size: 0x8000,
                permissions: 7,
            },
        ],
        peripherals: vec![Peripheral {
            core: 0,
            address: 0x50000,
            physical: 0x50000,
            size: 4,
        }],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    };
    let mut board = graph
        .build(
            Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            None,
        )
        .unwrap();
    board.set_execution_policy(policy).unwrap();
    if !retain_shadow {
        board.state.borrow_mut().code_shadow[0] = None;
    }
    board
        .write_memory(
            0,
            0x10000,
            &[0x17000u32, 0x10101]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board
        .write_memory(
            0,
            0x10100,
            &[0x6008u16, 0xe7fe]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board.run_until(20, 1).unwrap();
    let instructions = board.state()["instructions"][0].as_u64().unwrap();
    let counters = board.state.borrow().code_shadow[0]
        .as_ref()
        .map_or_else(String::new, |shadow| shadow.counters());
    let mut written = [0; 1];
    board.inspect_backing(0x30000, &mut written).unwrap();
    (instructions, counters, written)
}

#[test]
fn foreign_dma_alias_keeps_m4_shadow_without_changing_interactive_boundary() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        let dma = run_foreign_dma(policy, true, true);
        let no_dma = run_foreign_dma(policy, true, false);
        let no_shadow = run_foreign_dma(policy, false, true);
        assert_eq!(dma.0, no_dma.0, "{policy:?}");
        assert_eq!(dma.0, no_shadow.0, "{policy:?}");
        assert_eq!(dma.2, [0x5a], "{policy:?}");
        assert_eq!(no_dma.2, [0], "{policy:?}");
        assert_eq!(
            discarded_code_shadows(&dma.1),
            discarded_code_shadows(&no_dma.1),
            "{policy:?}"
        );
    }
}

fn discarded_code_shadows(counters: &str) -> u64 {
    counters
        .split_whitespace()
        .find_map(|counter| counter.strip_prefix("discarded="))
        .unwrap()
        .parse()
        .unwrap()
}
