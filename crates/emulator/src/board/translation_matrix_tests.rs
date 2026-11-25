//! EL1/EL0 stage-1 translation matrix: running-core execution proofs.
//!
//! The Board helpers in `translation_tests` call the walker directly; these
//! cases execute real instructions on a running Unicorn core, so the refusal
//! is the CPU's own stage-1 abort (surfaced as
//! `Unhandled CPU exception (UC_ERR_EXCEPTION)`) rather than a walker string.
//! Configurations the walker refuses before any fetch has no running-core
//! analog, so those are proven through `guest_physical`, which exercises the
//! same walker the Board uses. Every single-page denied case asserts the
//! touched frame did not change; the page-crossing store faults with its
//! read-only page untouched (its first half commits, as the architecture
//! permits for a page-crossing store).
//!
//! Matrix (register values → expected access → first refusal → proving test):
//!
//! | configuration | expected access | first refusal | test |
//! |---|---|---|---|
//! | TCR_EL1 `0x20` (TG0 4K, T0SZ 32), leaf `0x407` | fetch + store | accepted | `a_running_core_stores_through_a_4k_page` |
//! | TCR_EL1 `0x8011` (TG0 16K, T0SZ 17), leaf `0x407` | store | accepted | `a_running_core_stores_through_a_16k_page` |
//! | TCR_EL1 `0x4010` (TG0 64K, T0SZ 16), leaf `0x407` | store | accepted | `a_running_core_stores_through_a_64k_page` |
//! | TCR_EL1 `0x8020_0020` (TG1 4K, T1SZ 32), VA bit 55 set, TTBR1_EL1 | fetch + store | accepted | `a_running_core_executes_from_a_ttbr1_page` |
//! | two writable `0x407` leaves | `stp` across page boundary | accepted | `a_cross_page_store_pair_spans_two_writable_pages` |
//! | leaf `0x487` (AP[2]) on second page | `stp` spans into it | `Unhandled CPU exception (UC_ERR_EXCEPTION)`; first half commits, RO page untouched | `a_cross_page_store_pair_into_a_read_only_page_is_refused` |
//! | EL0 (pstate 0), leaf `0x40000000000447` (UXN) | fetch | `Unhandled CPU exception (UC_ERR_EXCEPTION)` | `a_fetch_from_a_uxn_page_is_refused_at_el0` |
//! | EL1, leaf `0x407 \| (1<<53)` (PXN) | fetch | `Unhandled CPU exception (UC_ERR_EXCEPTION)` | `a_fetch_from_a_pxn_page_is_refused_at_el1` |
//! | SCTLR_EL1 `1 \| (1<<19)` (WXN), writable leaf `0x407` | fetch | `Unhandled CPU exception (UC_ERR_EXCEPTION)` | `a_fetch_from_a_writable_page_is_refused_under_wxn` |
//! | leaf `0x487` (AP[2]) | store | `Unhandled CPU exception (UC_ERR_EXCEPTION)` | `a_write_to_a_read_only_page_is_refused_without_side_effect` |
//! | leaf `0x3` (access flag clear) | fetch | `Unhandled CPU exception (UC_ERR_EXCEPTION)` | `a_fetch_with_a_clear_access_flag_is_refused` |
//! | leaf `0x3` (access flag clear) | store | `Unhandled CPU exception (UC_ERR_EXCEPTION)` | `a_store_to_a_clear_access_flag_page_is_refused` |
//! | EL1 PAN (pstate `4 \| (1<<22)`), leaf `0x447` (AP[1]) | store | `Unhandled CPU exception (UC_ERR_EXCEPTION)` | `an_el1_write_to_a_user_page_is_refused_under_pan` |
//! | EL1 PAN (pstate `4 \| (1<<22)`), leaf `0x447` (AP[1]) | load | `Unhandled CPU exception (UC_ERR_EXCEPTION)` | `an_el1_read_of_a_user_page_is_refused_under_pan` |
//! | TCR_EL1 `0xc020` (TG0 3) | fetch | `reserved AArch64 translation granule` | `the_walker_refuses_a_reserved_granule` |
//! | TCR_EL1 `0x20 \| (1<<59)` (DS) | fetch | `AArch64 Fetch at 0x10000: unsupported TCR translation extension` | `the_walker_refuses_an_unsupported_tcr_extension` |
//! | TCR_EL1 `0xa0` (EPD0) | fetch | `AArch64 Fetch at 0x10000: translation table walks disabled` | `the_walker_refuses_translation_walks_disabled` |
//! | SCTLR_EL1 `1 \| (1<<25)` (EE) | fetch | `AArch64 Fetch at 0x10000: big-endian translation tables unsupported` | `the_walker_refuses_big_endian_tables` |
//! | pstate 8 or 12 (EL2/EL3) | fetch | `AArch64 Fetch at 0x10000: only AArch64 EL0/EL1 translation is supported` | `the_walker_refuses_el2_and_el3` |
//! | TCR_EL1 `(6<<32) \| 0x20` (IPS 6) | fetch | `AArch64 PA widths above 48 bits unsupported` | `the_walker_refuses_a_pa_width_above_48_bits` |
//! | TCR_EL1 `0x0f` (T0SZ 15, VA 49) | fetch | `unsupported AArch64 VA width 49` | `the_walker_refuses_an_unsupported_va_width` |
//! | TCR_EL1 `0x20`, VA `0x1_0000_0000` | fetch | `AArch64 VA 0x100000000 outside configured range` | `the_walker_refuses_a_va_outside_the_configured_range` |
//! | TCR_EL1 `0x20` (IPS 0), TTBR0_EL1 `0x1_0000_0000` | fetch | `AArch64 table address exceeds IPS` | `the_walker_refuses_a_table_address_exceeding_ips` |
//! | leaf `0x487`, write | write | `AArch64 write permission fault` | `the_walker_reports_exact_permission_faults` |
//! | leaf `0x407 \| (1<<53)`, fetch | fetch | `AArch64 execute permission fault` | `the_walker_reports_exact_permission_faults` |
//! | leaf `0x3`, read | read | `AArch64 access flag fault` | `the_walker_reports_exact_permission_faults` |
//! | leaf `0x447`, PAN set, read | read | `AArch64 PAN permission fault` | `the_walker_reports_exact_permission_faults` |
//! | `write_guest_memory` span, leaf `0x487` on second page | write | `AArch64 write permission fault` | `a_cross_page_span_is_walked_atomically` |
use super::{
    Board,
    bus::{Bus, Mode},
    factory,
};
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::{Access, Register, arm64};
use serde_json::json;

const TABLE: u64 = 0x3;
const MAIR: u64 = 0x4400;
/// Normal leaf through MAIR index 1, access flag, read/write, valid.
const NORMAL: u64 = 0x407;
/// Normal leaf with AP[2] set: readable, not writable at EL1.
const READ_ONLY: u64 = 0x487;
/// Valid page descriptor with the access flag clear.
const ACCESS_FLAG_CLEAR: u64 = 0x3;
/// EL1 execute-never (PXN, bit 53).
const PXN: u64 = NORMAL | (1 << 53);
/// EL0-accessible, writable at EL1 (AP[1] set, AP[2] clear): PAN target.
const USER_RW: u64 = 0x447;
/// EL0-accessible with UXN (bit 54) set: fetch refused at EL0.
const USER_UXN: u64 = NORMAL | (1 << 6) | (1 << 54);
const STORED: u64 = 0xbeef;
const SECOND: u64 = 0xfeed_face_feed_face;

fn board(ram_size: u64) -> Board {
    let graph: factory::Graph = serde_json::from_value(json!({
        "processors":[{"model":"armv8a","registers":[],"released":true,"mprofile":null,"rprofile":null}],
        "ram":[{"base":0,"size":ram_size}],
        "mappings":[{"core":0,"address":0,"physical":0,"size":ram_size,"permissions":7}],
        "dma_mappings":[{"address":0,"physical":0,"size":ram_size,"permissions":3}],
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

fn sys(board: &Board, name: &str, value: u64) {
    board.write_register(0, register(name), value).unwrap();
}

fn pstate(board: &Board, el0: bool) {
    board
        .write_register(0, Register::Pstate, if el0 { 0 } else { 4 })
        .unwrap();
}

/// `movz`/`movk` sequence loading a 64-bit immediate into `rd`.
fn mov_imm(rd: u32, value: u64) -> Vec<u32> {
    let mut out = Vec::new();
    let mut first = true;
    for (shift, hw) in [(0u32, 0u32), (16, 1), (32, 2), (48, 3)] {
        let chunk = ((value >> shift) & 0xffff) as u32;
        if chunk != 0 || first {
            let base = if first { 0xd280_0000 } else { 0xf280_0000 };
            out.push(base | (hw << 21) | (chunk << 5) | rd);
            first = false;
        }
    }
    if out.is_empty() {
        out.push(0xd280_0000 | rd);
    }
    out
}

/// Program: `x1 = data_va; x2 = STORED; str x2, [x1]`.
fn store_program(data_va: u64) -> Vec<u32> {
    let mut program = mov_imm(1, data_va);
    program.extend(mov_imm(2, STORED));
    program.push(0xf900_0022); // str x2, [x1]
    program
}

/// Program: `x1 = data_va; ldr x2, [x1]`.
fn load_program(data_va: u64) -> Vec<u32> {
    let mut program = mov_imm(1, data_va);
    program.push(0xf940_0022); // ldr x2, [x1]
    program
}

/// Program: `x1 = span_va; x2 = STORED; x3 = SECOND; stp x2, x3, [x1]`.
fn stp_program(span_va: u64) -> Vec<u32> {
    let mut program = mov_imm(1, span_va);
    program.extend(mov_imm(2, STORED));
    program.extend(mov_imm(3, SECOND));
    program.push(0xa900_0c22); // stp x2, x3, [x1]
    program
}

#[derive(Clone, Copy)]
struct Geometry {
    page: u32,
    index: u32,
    first: u32,
    first_bits: u32,
}

fn geometry(page: u32, index: u32, bits: u32) -> Geometry {
    let levels = (bits - page).div_ceil(index);
    Geometry {
        page,
        index,
        first: 4 - levels,
        first_bits: bits - page - index * (levels - 1),
    }
}

/// Physical base address of each level's table, laid out from `region`.
fn table_bases(geo: &Geometry, region: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let mut cursor = region;
    for level in geo.first..4 {
        let nbits = if level == geo.first {
            geo.first_bits
        } else {
            geo.index
        };
        let size = 1u64 << (nbits + 3);
        cursor = (cursor + size - 1) & !(size - 1);
        out.push(cursor);
        cursor += size;
    }
    out
}

/// Install translation tables for `leaves: &[(va, pa, leaf_bits)]`, then
/// program the system registers. Returns the table bases.
fn install(
    board: &Board,
    geo: &Geometry,
    ttbr_name: &str,
    tcr: u64,
    sctlr: u64,
    leaves: &[(u64, u64, u64)],
    el0: bool,
) -> Vec<u64> {
    let bases = table_bases(geo, 0x1000);
    for index in 0..bases.len() - 1 {
        words(board, bases[index], &[bases[index + 1] | TABLE]);
    }
    let last = bases[bases.len() - 1];
    let mut entries = vec![0u64; 1 << geo.index];
    for &(va, pa, bits) in leaves {
        let idx = ((va >> geo.page) & ((1 << geo.index) - 1)) as usize;
        entries[idx] = (pa & !((1 << geo.page) - 1)) | bits;
    }
    words(board, last, &entries);
    sys(board, ttbr_name, bases[0]);
    sys(board, "TCR_EL1", tcr);
    sys(board, "SCTLR_EL1", sctlr);
    sys(board, "MAIR_EL1", MAIR);
    pstate(board, el0);
    bases
}

fn run(board: &mut Board, code_va: u64, code_pa: u64, program: &[u32]) -> Result<(), String> {
    let bytes: Vec<u8> = program
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    board.write_physical(code_pa, &bytes).unwrap();
    board.write_register(0, Register::Pc, code_va).unwrap();
    board.run_until(program.len() as u64, 1).map(|_| ())
}

/// The exact running-core refusal for an instruction fetch that never happened.
fn fetch_failure(code_va: u64) -> String {
    format!(
        "CPU0 backend at PC {code_va:#x}, LR 0x0, instruction unavailable, X0 0x0: \
         Unhandled CPU exception (UC_ERR_EXCEPTION)"
    )
}

/// The exact running-core refusal for a data access that faulted at `pc`.
fn data_failure(pc: u64, instruction: u32) -> String {
    format!(
        "CPU0 backend at PC {pc:#x}, LR 0x0, instruction {instruction:#010x}, X0 0x0: \
         Unhandled CPU exception (UC_ERR_EXCEPTION)"
    )
}

// 4 KiB, 32-bit VA geometry shared by the TTBR0 execution cases.
fn geo4() -> Geometry {
    geometry(12, 9, 32)
}
const TCR4: u64 = 0x20;
const CODE_VA: u64 = 0x1_0000;
const CODE_PA: u64 = 0x6000;
const DATA_VA: u64 = 0x2_0000;
const DATA_PA: u64 = 0x7000;

/// Standard 4K board with `data_leaf` on the data page, code writable.
fn translated4(data_leaf: u64, el0: bool) -> Board {
    let board = board(0x100_0000);
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[
            (CODE_VA, CODE_PA, if el0 { USER_UXN } else { NORMAL }),
            (DATA_VA, DATA_PA, data_leaf),
        ],
        el0,
    );
    board
}

#[test]
fn a_running_core_stores_through_a_4k_page() {
    let mut board = translated4(NORMAL, false);
    let program = store_program(DATA_VA);
    run(&mut board, CODE_VA, CODE_PA, &program).unwrap();
    assert_eq!(word(&board, DATA_PA), STORED);
    assert_eq!(
        board.read_register(0, Register::Pc).unwrap(),
        CODE_VA + 4 * program.len() as u64
    );
}

#[test]
fn a_running_core_stores_through_a_16k_page() {
    let mut board = board(0x100_0000);
    let geo = geometry(14, 11, 47);
    let tcr = (2u64 << 14) | 17;
    let code_va = 0x4_0000;
    let code_pa = 0x2_0000;
    let data_va = 0x8_0000;
    let data_pa = 0x2_4000;
    install(
        &board,
        &geo,
        "TTBR0_EL1",
        tcr,
        1,
        &[(code_va, code_pa, NORMAL), (data_va, data_pa, NORMAL)],
        false,
    );
    let program = store_program(data_va);
    run(&mut board, code_va, code_pa, &program).unwrap();
    assert_eq!(word(&board, data_pa), STORED);
    assert_eq!(word(&board, data_va), 0, "the store must not land flat");
}

#[test]
fn a_running_core_stores_through_a_64k_page() {
    let mut board = board(0x100_0000);
    let geo = geometry(16, 13, 48);
    let tcr = (1u64 << 14) | 16;
    let code_va = 0x8_0000;
    let code_pa = 0x3_0000;
    let data_va = 0xc_0000;
    let data_pa = 0x4_0000;
    install(
        &board,
        &geo,
        "TTBR0_EL1",
        tcr,
        1,
        &[(code_va, code_pa, NORMAL), (data_va, data_pa, NORMAL)],
        false,
    );
    let program = store_program(data_va);
    run(&mut board, code_va, code_pa, &program).unwrap();
    assert_eq!(word(&board, data_pa), STORED);
    assert_eq!(word(&board, data_va), 0, "the store must not land flat");
}

#[test]
fn a_running_core_executes_from_a_ttbr1_page() {
    let mut board = board(0x100_0000);
    // TG1 = 2 (4K), T1SZ = 32; addresses in the TTBR1 half have bit 55 set.
    let tcr = (2u64 << 30) | (32 << 16) | 32;
    let code_va = 0xffff_ffff_0001_0000;
    let code_pa = 0x6000;
    let data_va = 0xffff_ffff_0002_0000;
    let data_pa = 0x7000;
    install(
        &board,
        &geo4(),
        "TTBR1_EL1",
        tcr,
        1,
        &[(code_va, code_pa, NORMAL), (data_va, data_pa, NORMAL)],
        false,
    );
    let program = store_program(data_va);
    run(&mut board, code_va, code_pa, &program).unwrap();
    assert_eq!(word(&board, data_pa), STORED);
}

#[test]
fn a_cross_page_store_pair_spans_two_writable_pages() {
    let mut board = board(0x100_0000);
    let span_va = 0x2_0ff8;
    let next_pa = 0x8000;
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[
            (CODE_VA, CODE_PA, NORMAL),
            (0x2_0000, DATA_PA, NORMAL),
            (0x2_1000, next_pa, NORMAL),
        ],
        false,
    );
    let program = stp_program(span_va);
    run(&mut board, CODE_VA, CODE_PA, &program).unwrap();
    assert_eq!(word(&board, DATA_PA + 0xff8), STORED);
    assert_eq!(word(&board, next_pa), SECOND);
}

#[test]
fn a_cross_page_store_pair_into_a_read_only_page_is_refused() {
    let mut board = board(0x100_0000);
    let span_va = 0x2_0ff8;
    let next_pa = 0x8000;
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[
            (CODE_VA, CODE_PA, NORMAL),
            (0x2_0000, DATA_PA, NORMAL),
            (0x2_1000, next_pa, READ_ONLY),
        ],
        false,
    );
    let program = stp_program(span_va);
    let failure = run(&mut board, CODE_VA, CODE_PA, &program).unwrap_err();
    assert_eq!(
        failure,
        data_failure(CODE_VA + 4 * (program.len() as u64 - 1), 0xa900_0c22),
        "the pair spans a writable page then a read-only page"
    );
    // A page-crossing store is not architecturally atomic: the first half
    // commits before the second page's permission fault.
    assert_eq!(word(&board, DATA_PA + 0xff8), STORED, "first half commits");
    assert_eq!(word(&board, next_pa), 0, "read-only page stays untouched");
}

#[test]
fn a_fetch_from_a_uxn_page_is_refused_at_el0() {
    let mut board = translated4(NORMAL, true);
    let program = store_program(DATA_VA);
    let failure = run(&mut board, CODE_VA, CODE_PA, &program).unwrap_err();
    assert_eq!(failure, fetch_failure(CODE_VA));
    assert_eq!(
        word(&board, DATA_PA),
        0,
        "the refused fetch means the store never ran"
    );
}

#[test]
fn a_fetch_from_a_pxn_page_is_refused_at_el1() {
    let mut board = board(0x100_0000);
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[(CODE_VA, CODE_PA, PXN), (DATA_VA, DATA_PA, NORMAL)],
        false,
    );
    let program = store_program(DATA_VA);
    let failure = run(&mut board, CODE_VA, CODE_PA, &program).unwrap_err();
    assert_eq!(failure, fetch_failure(CODE_VA));
    assert_eq!(
        word(&board, DATA_PA),
        0,
        "the refused fetch means the store never ran"
    );
}

#[test]
fn a_fetch_from_a_writable_page_is_refused_under_wxn() {
    let mut board = board(0x100_0000);
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1 | (1 << 19),
        &[(CODE_VA, CODE_PA, NORMAL), (DATA_VA, DATA_PA, NORMAL)],
        false,
    );
    let program = store_program(DATA_VA);
    let failure = run(&mut board, CODE_VA, CODE_PA, &program).unwrap_err();
    assert_eq!(failure, fetch_failure(CODE_VA));
    assert_eq!(
        word(&board, DATA_PA),
        0,
        "the refused fetch means the store never ran"
    );
}

#[test]
fn a_write_to_a_read_only_page_is_refused_without_side_effect() {
    let mut board = translated4(READ_ONLY, false);
    let program = store_program(DATA_VA);
    let failure = run(&mut board, CODE_VA, CODE_PA, &program).unwrap_err();
    assert_eq!(failure, data_failure(CODE_VA + 12, 0xf900_0022));
    assert_eq!(
        word(&board, DATA_PA),
        0,
        "a refused store must leave the frame"
    );
}

#[test]
fn a_fetch_with_a_clear_access_flag_is_refused() {
    let mut board = board(0x100_0000);
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[
            (CODE_VA, CODE_PA, ACCESS_FLAG_CLEAR),
            (DATA_VA, DATA_PA, NORMAL),
        ],
        false,
    );
    let program = store_program(DATA_VA);
    let failure = run(&mut board, CODE_VA, CODE_PA, &program).unwrap_err();
    assert_eq!(failure, fetch_failure(CODE_VA));
    assert_eq!(
        word(&board, DATA_PA),
        0,
        "the refused fetch means the store never ran"
    );
}

#[test]
fn a_store_to_a_clear_access_flag_page_is_refused() {
    let mut board = board(0x100_0000);
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[
            (CODE_VA, CODE_PA, NORMAL),
            (DATA_VA, DATA_PA, ACCESS_FLAG_CLEAR),
        ],
        false,
    );
    let program = store_program(DATA_VA);
    let failure = run(&mut board, CODE_VA, CODE_PA, &program).unwrap_err();
    assert_eq!(failure, data_failure(CODE_VA + 12, 0xf900_0022));
    assert_eq!(word(&board, DATA_PA), 0);
}

#[test]
fn an_el1_write_to_a_user_page_is_refused_under_pan() {
    let mut board = board(0x100_0000);
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[(CODE_VA, CODE_PA, NORMAL), (DATA_VA, DATA_PA, USER_RW)],
        false,
    );
    board
        .write_register(0, Register::Pstate, 4 | (1 << 22))
        .unwrap();
    let program = store_program(DATA_VA);
    let failure = run(&mut board, CODE_VA, CODE_PA, &program).unwrap_err();
    assert_eq!(failure, data_failure(CODE_VA + 12, 0xf900_0022));
    assert_eq!(word(&board, DATA_PA), 0);
}

#[test]
fn an_el1_read_of_a_user_page_is_refused_under_pan() {
    let mut board = board(0x100_0000);
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[(CODE_VA, CODE_PA, NORMAL), (DATA_VA, DATA_PA, USER_RW)],
        false,
    );
    board
        .write_register(0, Register::Pstate, 4 | (1 << 22))
        .unwrap();
    let program = load_program(DATA_VA);
    let failure = run(&mut board, CODE_VA, CODE_PA, &program).unwrap_err();
    assert_eq!(
        failure,
        data_failure(CODE_VA + 4 * (program.len() as u64 - 1), 0xf940_0022)
    );
    assert_eq!(board.read_register(0, Register::X(2)).unwrap(), 0);
}

// -- Walker-level refusals: no running-core analog because the walker stops
//    before any instruction fetch. Each asserts the exact walker string.

#[test]
fn the_walker_refuses_a_reserved_granule() {
    let board = translated4(NORMAL, false);
    sys(&board, "TCR_EL1", (3 << 14) | 0x20);
    let failure = board.guest_physical(0, CODE_VA, Access::Fetch).unwrap_err();
    assert_eq!(failure, "reserved AArch64 translation granule");
}

#[test]
fn the_walker_refuses_an_unsupported_tcr_extension() {
    let board = translated4(NORMAL, false);
    sys(&board, "TCR_EL1", TCR4 | (1 << 59));
    let failure = board.guest_physical(0, CODE_VA, Access::Fetch).unwrap_err();
    assert_eq!(
        failure,
        "AArch64 Fetch at 0x10000: unsupported TCR translation extension"
    );
}

#[test]
fn the_walker_refuses_translation_walks_disabled() {
    let board = translated4(NORMAL, false);
    sys(&board, "TCR_EL1", TCR4 | (1 << 7));
    let failure = board.guest_physical(0, CODE_VA, Access::Fetch).unwrap_err();
    assert_eq!(
        failure,
        "AArch64 Fetch at 0x10000: translation table walks disabled"
    );
}

#[test]
fn the_walker_refuses_big_endian_tables() {
    let board = translated4(NORMAL, false);
    sys(&board, "SCTLR_EL1", 1 | (1 << 25));
    let failure = board.guest_physical(0, CODE_VA, Access::Fetch).unwrap_err();
    assert_eq!(
        failure,
        "AArch64 Fetch at 0x10000: big-endian translation tables unsupported"
    );
}

#[test]
fn the_walker_refuses_el2_and_el3() {
    for el in [8u64, 12] {
        let board = translated4(NORMAL, false);
        board.write_register(0, Register::Pstate, el).unwrap();
        let failure = board.guest_physical(0, CODE_VA, Access::Fetch).unwrap_err();
        assert_eq!(
            failure,
            "AArch64 Fetch at 0x10000: only AArch64 EL0/EL1 translation is supported"
        );
    }
}

#[test]
fn the_walker_refuses_a_pa_width_above_48_bits() {
    let board = translated4(NORMAL, false);
    sys(&board, "TCR_EL1", (6 << 32) | 0x20);
    let failure = board.guest_physical(0, CODE_VA, Access::Fetch).unwrap_err();
    assert_eq!(failure, "AArch64 PA widths above 48 bits unsupported");
}

#[test]
fn the_walker_refuses_an_unsupported_va_width() {
    let board = translated4(NORMAL, false);
    sys(&board, "TCR_EL1", 0x0f);
    let failure = board.guest_physical(0, CODE_VA, Access::Fetch).unwrap_err();
    assert_eq!(failure, "unsupported AArch64 VA width 49");
}

#[test]
fn the_walker_refuses_a_va_outside_the_configured_range() {
    let board = translated4(NORMAL, false);
    let failure = board
        .guest_physical(0, 0x1_0000_0000, Access::Fetch)
        .unwrap_err();
    assert_eq!(failure, "AArch64 VA 0x100000000 outside configured range");
}

#[test]
fn the_walker_refuses_a_table_address_exceeding_ips() {
    let board = translated4(NORMAL, false);
    sys(&board, "TTBR0_EL1", 0x1_0000_0000);
    let failure = board.guest_physical(0, CODE_VA, Access::Fetch).unwrap_err();
    assert_eq!(failure, "AArch64 table address exceeds IPS");
}

#[test]
fn the_walker_reports_exact_permission_faults() {
    let read_only = board(0x100_0000);
    install(
        &read_only,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[(DATA_VA, DATA_PA, READ_ONLY)],
        false,
    );
    assert_eq!(
        read_only
            .guest_physical(0, DATA_VA, Access::Write)
            .unwrap_err(),
        "AArch64 write permission fault"
    );

    let pxn = board(0x100_0000);
    install(
        &pxn,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[(CODE_VA, CODE_PA, PXN)],
        false,
    );
    assert_eq!(
        pxn.guest_physical(0, CODE_VA, Access::Fetch).unwrap_err(),
        "AArch64 execute permission fault"
    );

    let af = board(0x100_0000);
    install(
        &af,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[(DATA_VA, DATA_PA, ACCESS_FLAG_CLEAR)],
        false,
    );
    assert_eq!(
        af.guest_physical(0, DATA_VA, Access::Read).unwrap_err(),
        "AArch64 access flag fault"
    );

    let pan = board(0x100_0000);
    install(
        &pan,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[(DATA_VA, DATA_PA, USER_RW)],
        false,
    );
    pan.write_register(0, Register::Pstate, 4 | (1 << 22))
        .unwrap();
    assert_eq!(
        pan.guest_physical(0, DATA_VA, Access::Read).unwrap_err(),
        "AArch64 PAN permission fault"
    );
}

#[test]
fn a_cross_page_span_is_walked_atomically() {
    let board = board(0x100_0000);
    let span_va = 0x2_0ff8;
    let next_pa = 0x8000;
    install(
        &board,
        &geo4(),
        "TTBR0_EL1",
        TCR4,
        1,
        &[(0x2_0000, DATA_PA, NORMAL), (0x2_1000, next_pa, READ_ONLY)],
        false,
    );
    let bytes = [0u8; 16];
    let failure = board.write_guest_memory(0, span_va, &bytes).unwrap_err();
    assert_eq!(failure, "AArch64 write permission fault");
    assert_eq!(
        word(&board, DATA_PA + 0xff8),
        0,
        "writable first page untouched"
    );
    assert_eq!(word(&board, next_pa), 0, "read-only second page untouched");
}
