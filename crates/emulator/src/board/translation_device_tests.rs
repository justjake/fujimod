//! Running-core Device access through EL1 stage-1 translation.
//!
//! `translation_execution_tests` proves that a running AArch64 core walks its
//! EL1 stage-1 tables for Normal-memory fetch, load and store. These cases
//! prove the Device half of ticket O81: a guest Device virtual address reaches
//! the physical `Board` bus and its device scheduler through the same walk.
//!
//! The guest maps virtual page `0x5_0000` to physical page `0x40000` with a
//! Device leaf (MAIR index 0). `0x40000` is the first page past modeled RAM,
//! so a flat execution path cannot reach it, and only the mapped Device VA can.
//! A scratch register peripheral is registered on the bus at `0x40000` and
//! records every access it observes, so the tests assert the exact physical
//! address, width, value and order the device scheduler saw.

use super::{
    Board,
    bus::{Bus, Mode},
    factory,
};
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::{Access, Register, arm64};
use fujimod_emulator_devices::{Cx, Device};
use serde_json::json;
use std::sync::{Arc, Mutex};

const LEVEL1: u64 = 0x1000;
const LEVEL2: u64 = 0x2000;
const LEVEL3: u64 = 0x3000;
const CODE_VA: u64 = 0x1_0000;
const CODE_PA: u64 = 0x6000;
/// The guest Device virtual page the ticket names.
const DEVICE_VA: u64 = 0x5_0000;
/// The peripheral's physical page, the first page past modeled RAM.
const DEVICE_PA: u64 = 0x40000;
/// Normal memory at MAIR index 1; index 0 stays Device (`0x00`).
const MAIR: u64 = 0x4400;
const TABLE: u64 = 0x3;
/// Normal leaf through MAIR index 1, access flag, read/write, valid.
const NORMAL: u64 = 0x407;
/// Device leaf through MAIR index 0, access flag, read/write, valid.
const DEVICE: u64 = 0x403;
const T0SZ_32_BITS: u64 = 0x20;
/// `nop`; four of these sit at the virtual code page as a flat-path decoy.
const NOP: u32 = 0xd503_201f;
/// `movz x1,#0x5,lsl#16`; `movz x2,#0xbeef`; `str w2,[x1]`;
/// `ldr w3,[x1]`. The 32-bit store and load both hit the Device page at
/// `DEVICE_VA`, matching the scratch register's 4-byte window.
const DEVICE_PROGRAM: [u32; 4] = [0xd2a0_00a1, 0xd297_dde2, 0xb900_0022, 0xb940_0023];
/// `movz x1,#0x6,lsl#16`; `movz x2,#0xbeef`; `str x2,[x1]`. The store targets
/// `0x6_0000`, which has no stage-1 entry.
const UNMAPPED_STORE_PROGRAM: [u32; 3] = [0xd2a0_00c1, 0xd297_dde2, 0xf900_0022];
const STORED: u64 = 0xbeef;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Read,
    Write,
}

/// One access the device scheduler observed, in arrival order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Observed {
    op: Op,
    address: u64,
    size: u8,
    value: u64,
}

#[derive(Default)]
struct Scratch {
    events: Vec<Observed>,
    value: u64,
}

/// A one-word scratch register that records every access in arrival order and
/// returns its stored value on read, so a load round-trips the written value.
struct ScratchRegister(Arc<Mutex<Scratch>>);

impl Device for ScratchRegister {
    fn name(&self) -> &'static str {
        "translation-device-scratch"
    }
    fn read(&mut self, _cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let mut scratch = self.0.lock().unwrap();
        let value = scratch.value;
        scratch.events.push(Observed {
            op: Op::Read,
            address,
            size,
            value,
        });
        value
    }
    fn write(&mut self, _cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let mut scratch = self.0.lock().unwrap();
        scratch.value = value;
        scratch.events.push(Observed {
            op: Op::Write,
            address,
            size,
            value,
        });
    }
}

fn words(board: &Board, address: u64, values: &[u64]) {
    let bytes: Vec<u8> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    board.write_physical(address, &bytes).unwrap();
}

fn register(name: &str) -> Register {
    Register::Arm64System(arm64::system_register(name).unwrap())
}

/// Build the board with the scratch peripheral at `DEVICE_PA`, publish the
/// three-level tables and the chosen program, and leave translation enabled at
/// EL1 with the core about to fetch `CODE_VA`. When `map_device` is false the
/// `DEVICE_VA` page is absent from the level-3 table.
fn translated_device_execution(program: &[u32], map_device: bool) -> (Board, Arc<Mutex<Scratch>>) {
    let scratch = Arc::new(Mutex::new(Scratch::default()));
    let graph: factory::Graph = serde_json::from_value(json!({
        "processors":[{"model":"armv8a","registers":[],"released":true,"mprofile":null,"rprofile":null}],
        "ram":[{"base":0,"size":262144}],
        "mappings":[{"core":0,"address":0,"physical":0,"size":262144,"permissions":7}],
        "dma_mappings":[{"address":0,"physical":0,"size":262144,"permissions":3}],
        "decoders":[],
        "peripherals":[{"core":0,"address":DEVICE_PA,"physical":DEVICE_PA,"size":4}],
        "instructions_per_tick":[1,1]
    }))
    .unwrap();
    let mut bus = Bus::new(Mode::Strict);
    bus.register(DEVICE_PA, 4, Box::new(ScratchRegister(scratch.clone())))
        .unwrap();
    let board = graph
        .build(
            factory::Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1 << 20),
            bus,
            None,
        )
        .unwrap();

    words(&board, LEVEL1, &[LEVEL2 | TABLE]);
    words(&board, LEVEL2, &[LEVEL3 | TABLE]);
    let mut level3 = [0u64; 512];
    level3[(CODE_VA >> 12) as usize] = CODE_PA | NORMAL;
    if map_device {
        level3[(DEVICE_VA >> 12) as usize] = DEVICE_PA | DEVICE;
    }
    words(&board, LEVEL3, &level3);
    let bytes: Vec<u8> = program
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    board.write_physical(CODE_PA, &bytes).unwrap();
    words(
        &board,
        CODE_VA,
        &[u64::from(NOP).wrapping_mul(0x1_0000_0001); 4],
    );
    board.write_register(0, register("MAIR_EL1"), MAIR).unwrap();
    board
        .write_register(0, register("TTBR0_EL1"), LEVEL1)
        .unwrap();
    board
        .write_register(0, register("TCR_EL1"), T0SZ_32_BITS)
        .unwrap();
    board.write_register(0, register("SCTLR_EL1"), 1).unwrap();
    board.write_register(0, Register::Pstate, 4).unwrap();
    board.write_register(0, Register::Pc, CODE_VA).unwrap();
    (board, scratch)
}

#[test]
fn a_running_core_routes_device_loads_and_stores_through_stage1() {
    let (mut board, scratch) = translated_device_execution(&DEVICE_PROGRAM, true);
    board.run_until(DEVICE_PROGRAM.len() as u64, 1).unwrap();
    let events = scratch.lock().unwrap().events.clone();
    assert_eq!(
        events,
        vec![
            Observed {
                op: Op::Write,
                address: DEVICE_PA,
                size: 4,
                value: STORED,
            },
            Observed {
                op: Op::Read,
                address: DEVICE_PA,
                size: 4,
                value: STORED,
            },
        ],
        "the device scheduler must see the mapped physical address, width and \
         value, store before load"
    );
    assert_eq!(
        board.read_register(0, Register::X(3)).unwrap(),
        STORED,
        "the load must round-trip the value the device returned"
    );
    assert_eq!(
        board.read_register(0, Register::Pc).unwrap(),
        CODE_VA + 4 * DEVICE_PROGRAM.len() as u64,
        "the mapped program must run to its last instruction"
    );
}

#[test]
fn an_unmapped_device_va_refuses_without_a_bus_side_effect() {
    let (mut board, scratch) = translated_device_execution(&UNMAPPED_STORE_PROGRAM, false);
    let failure = board
        .run_until(UNMAPPED_STORE_PROGRAM.len() as u64, 1)
        .unwrap_err();
    assert_eq!(
        failure,
        "CPU0 backend at PC 0x10008, LR 0x0, instruction 0xf9000022, \
         X0 0x0: Unhandled CPU exception (UC_ERR_EXCEPTION)",
    );
    let scratch = scratch.lock().unwrap();
    assert!(
        scratch.events.is_empty(),
        "a store through an unmapped Device VA must not reach the bus"
    );
    assert_eq!(scratch.value, 0, "the scratch register must stay unchanged");
}

#[test]
fn the_host_walker_refuses_a_device_leaf() {
    let (board, scratch) = translated_device_execution(&DEVICE_PROGRAM, true);
    let refusal = board
        .guest_physical(0, DEVICE_VA, Access::Write)
        .unwrap_err();
    assert_eq!(
        refusal,
        format!(
            "AArch64 Write at {DEVICE_VA:#x}: translated Device access at \
             {DEVICE_PA:#x} is only performed by the running core's MMIO path"
        ),
        "host inspection must refuse Device pages"
    );
    assert!(
        scratch.lock().unwrap().events.is_empty(),
        "the host walker refusal must not dispatch any bus transaction"
    );
}
