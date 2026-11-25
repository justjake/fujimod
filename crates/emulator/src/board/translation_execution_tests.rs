//! Running-core execution through EL1 stage-1 translation.
//!
//! The Board helpers in `translation_tests` call the walker directly. These
//! cases execute instructions instead: the guest runs from a virtual address
//! whose page maps to a different physical frame, stores through a second such
//! page, and loads from an identity page. Every virtual address also holds a
//! decoy, so a flat execution path cannot produce the asserted physical effect
//! by accident.
use super::{
    Board,
    bus::{Bus, Mode},
    factory,
};
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::{Access, Register, arm64};
use serde_json::json;

const LEVEL1: u64 = 0x1000;
const LEVEL2: u64 = 0x2000;
const LEVEL3: u64 = 0x3000;
const CODE_VA: u64 = 0x1_0000;
const CODE_PA: u64 = 0x6000;
const DATA_VA: u64 = 0x2_0000;
const DATA_PA: u64 = 0x7000;
const IDENTITY_VA: u64 = 0x3_0000;
/// Normal memory at MAIR index 1; index 0 stays Device (`0x00`).
const MAIR: u64 = 0x4400;
const TABLE: u64 = 0x3;
/// Normal leaf through MAIR index 1, access flag, read/write, valid.
const NORMAL: u64 = 0x407;
/// Normal leaf with AP[2] set: readable, not writable at EL1.
const READ_ONLY: u64 = 0x487;
const T0SZ_32_BITS: u64 = 0x20;
/// `nop`; four of these sit at the virtual code page as a flat-path decoy.
const NOP: u32 = 0xd503_201f;
/// `movz x1,#0x2,lsl#16`; `movz x2,#0xbeef`; `str x2,[x1]`;
/// `movz x3,#0x3,lsl#16`; `ldr x4,[x3]`.
const PROGRAM: [u32; 5] = [
    0xd2a0_0041,
    0xd297_dde2,
    0xf900_0022,
    0xd2a0_0063,
    0xf940_0064,
];
const STORED: u64 = 0xbeef;
const DECOY_DATA: u64 = 0x1111_2222_3333_4444;
const LOADED: u64 = 0xfeed_face_feed_face;

fn board() -> Board {
    let graph: factory::Graph = serde_json::from_value(json!({
        "processors":[{"model":"armv8a","registers":[],"released":true,"mprofile":null,"rprofile":null}],
        "ram":[{"base":0,"size":262144}],
        "mappings":[{"core":0,"address":0,"physical":0,"size":262144,"permissions":7}],
        "dma_mappings":[{"address":0,"physical":0,"size":262144,"permissions":3}],
        "decoders":[],"peripherals":[],"instructions_per_tick":[1,1]
    }))
    .unwrap();
    graph
        .build(
            factory::Options::new(RunEngine::Unicorn).with_translation_cache_size_bytes(1 << 20),
            Bus::new(Mode::Strict),
            None,
        )
        .unwrap()
}

fn words(board: &Board, address: u64, values: &[u64]) {
    let bytes: Vec<u8> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    board.write_physical(address, &bytes).unwrap();
}

fn word(board: &Board, address: u64) -> u64 {
    let mut bytes = [0u8; 8];
    board.read_physical(address, &mut bytes).unwrap();
    u64::from_le_bytes(bytes)
}

fn register(name: &str) -> Register {
    Register::Arm64System(arm64::system_register(name).unwrap())
}

/// Publish the three-level tables, write real program bytes to the code frame
/// while the matching virtual page keeps `nop` decoys, and leave translation
/// enabled at EL1 with the core about to fetch `CODE_VA`.
fn translated_execution(data_leaf: u64) -> Board {
    let board = board();
    words(&board, LEVEL1, &[LEVEL2 | TABLE]);
    words(&board, LEVEL2, &[LEVEL3 | TABLE]);
    let mut level3 = [0u64; 512];
    level3[(CODE_VA >> 12) as usize] = CODE_PA | NORMAL;
    level3[(DATA_VA >> 12) as usize] = DATA_PA | data_leaf;
    level3[(IDENTITY_VA >> 12) as usize] = IDENTITY_VA | NORMAL;
    words(&board, LEVEL3, &level3);
    let program: Vec<u8> = PROGRAM
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    board.write_physical(CODE_PA, &program).unwrap();
    words(
        &board,
        CODE_VA,
        &[u64::from(NOP).wrapping_mul(0x1_0000_0001); 4],
    );
    words(&board, DATA_VA, &[DECOY_DATA; 4]);
    words(&board, IDENTITY_VA, &[LOADED; 4]);
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
    board
}

#[test]
fn a_running_core_stores_through_a_nonidentity_page() {
    let mut board = translated_execution(NORMAL);
    board.run_until(PROGRAM.len() as u64, 1).unwrap();
    assert_eq!(
        word(&board, DATA_PA),
        STORED,
        "the store must land in the mapped frame, not the virtual page"
    );
    assert_eq!(
        word(&board, DATA_VA),
        DECOY_DATA,
        "the virtual address holds a decoy the store must not reach"
    );
    assert_eq!(
        board.read_register(0, Register::Pc).unwrap(),
        CODE_VA + 4 * PROGRAM.len() as u64,
        "the mapped program must run to its last instruction"
    );
}

#[test]
fn a_running_core_loads_through_an_identity_page() {
    let mut board = translated_execution(NORMAL);
    board.run_until(PROGRAM.len() as u64, 1).unwrap();
    assert_eq!(board.read_register(0, Register::X(4)).unwrap(), LOADED);
}

#[test]
fn the_walker_agrees_with_the_running_core() {
    let board = translated_execution(NORMAL);
    assert_eq!(
        board.guest_physical(0, CODE_VA, Access::Fetch).unwrap(),
        CODE_PA
    );
    assert_eq!(
        board.guest_physical(0, DATA_VA, Access::Write).unwrap(),
        DATA_PA
    );
}

#[test]
fn a_read_only_page_refuses_the_store_without_a_side_effect() {
    let mut board = translated_execution(READ_ONLY);
    let failure = board.run_until(PROGRAM.len() as u64, 1).unwrap_err();
    assert!(
        failure.contains("exception") || failure.contains("permission"),
        "{failure}"
    );
    assert_eq!(
        word(&board, DATA_PA),
        0,
        "a refused store must leave the mapped frame untouched"
    );
    assert_eq!(word(&board, DATA_VA), DECOY_DATA);
}
