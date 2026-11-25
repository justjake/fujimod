//! Guest-address translation through the shared Board seam.
//!
//! Every case runs stock-free synthetic data: one level-1 table, one level-2
//! table and one level-3 table describe an identity page, a page whose output
//! address differs from its input address, a Device page, a write-protected
//! page and an execute-never page.
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
const MAPPED_VA: u64 = 0x2000;
const MAPPED_PA: u64 = 0x4000;
const IDENTITY_VA: u64 = 0x10;
const MISSING_VA: u64 = 0x5000;
const DEVICE_VA: u64 = 0x7000;
const DEVICE_PA: u64 = 0x0900_0000;
const READ_ONLY_VA: u64 = 0x8000;
const READ_ONLY_PA: u64 = 0x8000;
const EXECUTE_NEVER_VA: u64 = 0x9000;
const EXECUTE_NEVER_PA: u64 = 0x9000;
/// Normal memory at MAIR index 1; index 0 stays Device (`0x00`).
const MAIR: u64 = 0x4400;
/// Table descriptor: valid, next level.
const TABLE: u64 = 0x3;
/// Normal leaf through MAIR index 1, access flag and valid set.
const NORMAL: u64 = 0x407;
/// Device leaf through MAIR index 0, access flag and valid set.
const DEVICE: u64 = 0x403;
/// Normal leaf with AP[2] set: readable, not writable at EL1.
const READ_ONLY: u64 = 0x487;
/// Normal leaf with PXN set: readable, not executable at EL1.
const EXECUTE_NEVER: u64 = 0x407 | (1 << 53);
const T0SZ_32_BITS: u64 = 0x20;

fn board() -> Board {
    let graph: factory::Graph = serde_json::from_value(json!({
        "processors":[{"model":"armv8a","registers":[],"released":true,"mprofile":null,"rprofile":null}],
        "ram":[{"base":0,"size":65536}],
        "mappings":[{"core":0,"address":0,"physical":0,"size":65536,"permissions":7}],
        "dma_mappings":[{"address":0,"physical":0,"size":65536,"permissions":3}],
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

fn register(name: &str) -> Register {
    Register::Arm64System(arm64::system_register(name).unwrap())
}

/// Publish tables that identity-map page 0 and give every other case its own
/// page, then turn translation on for EL1 with a 4 KiB granule and a 32-bit
/// input width.
fn translated_board() -> Board {
    let board = board();
    words(&board, LEVEL1, &[LEVEL2 | TABLE, 0, 0, 0]);
    words(&board, LEVEL2, &[LEVEL3 | TABLE]);
    let mut level3 = [0u64; 512];
    level3[(IDENTITY_VA >> 12) as usize] = NORMAL;
    level3[(MAPPED_VA >> 12) as usize] = MAPPED_PA | NORMAL;
    level3[(DEVICE_VA >> 12) as usize] = DEVICE_PA | DEVICE;
    level3[(READ_ONLY_VA >> 12) as usize] = READ_ONLY_PA | READ_ONLY;
    level3[(EXECUTE_NEVER_VA >> 12) as usize] = EXECUTE_NEVER_PA | EXECUTE_NEVER;
    words(&board, LEVEL3, &level3);
    board.write_register(0, register("MAIR_EL1"), MAIR).unwrap();
    board
        .write_register(0, register("TTBR0_EL1"), LEVEL1)
        .unwrap();
    board
        .write_register(0, register("TCR_EL1"), T0SZ_32_BITS)
        .unwrap();
    board.write_register(0, register("SCTLR_EL1"), 1).unwrap();
    board.write_register(0, Register::Pstate, 4).unwrap();
    board
}

#[test]
fn a_nonidentity_page_carries_fetch_load_and_store() {
    let board = translated_board();
    board.write_physical(MAPPED_PA, b"DATA").unwrap();
    let mut bytes = [0u8; 4];
    board.read_guest_memory(0, MAPPED_VA, &mut bytes).unwrap();
    assert_eq!(&bytes, b"DATA");
    assert_eq!(
        board.guest_physical(0, MAPPED_VA, Access::Fetch).unwrap(),
        MAPPED_PA
    );
    board.write_guest_memory(0, MAPPED_VA, b"NEW!").unwrap();
    let mut physical = [0u8; 4];
    board.read_physical(MAPPED_PA, &mut physical).unwrap();
    assert_eq!(&physical, b"NEW!");
}

#[test]
fn an_identity_page_keeps_its_offset() {
    let board = translated_board();
    assert_eq!(
        board.guest_physical(0, IDENTITY_VA, Access::Read).unwrap(),
        IDENTITY_VA
    );
    board.write_guest_memory(0, IDENTITY_VA, b"id").unwrap();
    let mut physical = [0u8; 2];
    board.read_physical(IDENTITY_VA, &mut physical).unwrap();
    assert_eq!(&physical, b"id");
}

#[test]
fn a_missing_descriptor_refuses_at_the_guest_address() {
    let board = translated_board();
    let mut bytes = [0u8; 4];
    let error = board
        .read_guest_memory(0, MISSING_VA, &mut bytes)
        .unwrap_err();
    assert!(error.contains("0x5000"), "{error}");
    assert!(error.contains("invalid AArch64 L3 descriptor"), "{error}");
}

#[test]
fn a_device_page_is_only_reached_through_the_core_mmio_path() {
    let board = translated_board();
    let error = board
        .guest_physical(0, DEVICE_VA, Access::Read)
        .unwrap_err();
    assert!(error.contains(&format!("{DEVICE_PA:#x}")), "{error}");
    assert!(error.contains("running core's MMIO path"), "{error}");
}

#[test]
fn a_write_protected_page_refuses_without_storing() {
    let board = translated_board();
    board.write_physical(READ_ONLY_PA, b"KEEP").unwrap();
    let error = board
        .write_guest_memory(0, READ_ONLY_VA, b"LOST")
        .unwrap_err();
    assert!(error.contains("write permission fault"), "{error}");
    let mut bytes = [0u8; 4];
    board
        .read_guest_memory(0, READ_ONLY_VA, &mut bytes)
        .unwrap();
    assert_eq!(&bytes, b"KEEP");
    board
        .write_guest_memory(0, READ_ONLY_VA, b"DENY")
        .expect_err("the store must stay refused");
    board.read_physical(READ_ONLY_PA, &mut bytes).unwrap();
    assert_eq!(&bytes, b"KEEP");
}

#[test]
fn an_execute_never_page_refuses_a_fetch_but_reads() {
    let board = translated_board();
    board.write_physical(EXECUTE_NEVER_PA, b"CODE").unwrap();
    let error = board
        .guest_physical(0, EXECUTE_NEVER_VA, Access::Fetch)
        .unwrap_err();
    assert!(error.contains("execute permission fault"), "{error}");
    let mut bytes = [0u8; 4];
    board
        .read_guest_memory(0, EXECUTE_NEVER_VA, &mut bytes)
        .unwrap();
    assert_eq!(&bytes, b"CODE");
}

#[test]
fn a_write_back_memory_attribute_is_ordinary_ram() {
    // The X-H2 map marks its RAM pages with MAIR index 2 = 0xff (write-back),
    // which must behave like any other Normal attribute without modeled caches.
    let board = translated_board();
    board
        .write_register(0, register("MAIR_EL1"), MAIR | (0xff << 16))
        .unwrap();
    let mut level3 = [0u64; 512];
    level3[(MAPPED_VA >> 12) as usize] = MAPPED_PA | (2 << 2) | 0x403;
    words(&board, LEVEL3, &level3);
    board.write_physical(MAPPED_PA, b"WB!!").unwrap();
    let mut bytes = [0u8; 4];
    board.read_guest_memory(0, MAPPED_VA, &mut bytes).unwrap();
    assert_eq!(&bytes, b"WB!!");
}

#[test]
fn translation_off_keeps_the_flat_address() {
    let board = board();
    board.write_physical(MAPPED_VA, b"FLAT").unwrap();
    let mut bytes = [0u8; 4];
    board.read_guest_memory(0, MAPPED_VA, &mut bytes).unwrap();
    assert_eq!(&bytes, b"FLAT");
    assert_eq!(
        board.guest_physical(0, MAPPED_VA, Access::Write).unwrap(),
        MAPPED_VA
    );
}

#[test]
fn a_span_crossing_into_an_unmapped_page_refuses() {
    let board = translated_board();
    let mut bytes = [0u8; 8];
    let error = board
        .read_guest_memory(0, MAPPED_VA + 0xffc, &mut bytes)
        .unwrap_err();
    assert!(error.contains("0x3000"), "{error}");
}
