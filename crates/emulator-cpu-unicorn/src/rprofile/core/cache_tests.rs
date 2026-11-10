//! Cross-processor dirty-line refusals.
//!
//! Regression for an X-E2S camera run (2026-11-10, first shot after power-on
//! of a two-core development job). Core 1 filled lookup tables in ordinary
//! cached `.bss`. CPU0 then stored its own statics that shared those 32-byte
//! lines, and CPU0's eviction later wrote its stale copy of the table bytes
//! back: whole rows of the job's output took zero table entries. Execution,
//! whose memory is coherent across processors, completed the same job with
//! correct output. Here the shared line is 0x2000..0x2020: core 1 stores the
//! last table byte at 0x2000 and CPU0 stores a static at 0x2008.
use super::tests::{run, set, words};
use super::*;
use crate::rprofile::data_cache::Geometry;

const TABLE: u32 = 0x2000;
const STATIC: u32 = 0x2008;
const UNCACHED: u32 = 0x4000_0000;
const STRB: u32 = 0xe5c1_2000; // strb r2, [r1]
const LDRB: u32 = 0xe5d1_0000; // ldrb r0, [r1]
const WFI: u32 = 0xe320_f003;
const DCCMVAC: u32 = 0xee07_1f3a; // mcr p15, 0, r1, c7, c10, 1
const DCCISW: u32 = 0xee07_3f5e; // mcr p15, 0, r3, c7, c14, 2

fn system(crn: u32, crm: u32, opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn,
        crm,
        opc2,
        secure: 0,
        wide: false,
    }
}

/// Two processors sharing 64 KiB of RAM through a write-back alias at 0 and
/// a Normal non-cacheable alias at 0x40000000, as their MPUs declare.
fn cluster() -> (Core, Core, Backing) {
    let memory = Memory::default();
    let backing = Backing::aligned_zeros(0x10000, 4096).unwrap();
    let options = Options {
        data_cache: Some(Geometry {
            line_bytes: 32,
            sets: 128,
        }),
        ..Options::default()
    };
    let make = || {
        let cpu = super::tests::processor_with_options(&memory, options);
        unsafe {
            for address in [0, UNCACHED as u64] {
                cpu.map_physical(
                    address,
                    0,
                    0x10000,
                    7,
                    NonNull::new(backing.as_ptr()).unwrap(),
                )
                .unwrap();
            }
        }
        // Region 0: 1 GiB at 0, Normal write-back write-allocate, full
        // access. Region 1: 1 GiB at 0x40000000, Normal non-cacheable.
        for (region, base, access) in [(0, 0, 0x30b), (1, UNCACHED, 0x308)] {
            cpu.coprocessor(system(6, 2, 0), Some(region)).unwrap();
            cpu.coprocessor(system(6, 1, 0), Some(base as u64)).unwrap();
            cpu.coprocessor(system(6, 1, 2), Some((29 << 1) | 1))
                .unwrap();
            cpu.coprocessor(system(6, 1, 4), Some(access)).unwrap();
        }
        let sctlr = cpu.coprocessor(system(1, 0, 0), None).unwrap();
        cpu.coprocessor(system(1, 0, 0), Some(sctlr | 0b101))
            .unwrap();
        set(&cpu, arm::CPSR, 0xd3);
        cpu
    };
    let core0 = make();
    let core1 = make();
    (core0, core1, backing)
}

/// Run `code` at `at` on `cpu` with r1 = `address` until its WFI.
fn execute(cpu: &Core, at: u64, address: u32, code: &[u32]) -> Result<(), String> {
    words(cpu, at, &[code, &[WFI]].concat());
    set(cpu, arm::CORE[1], address);
    set(cpu, arm::CORE[2], 0xff);
    set(cpu, arm::CORE[3], 0);
    match run(cpu, at, 100) {
        Ok(Exit::Hint(Hint::WaitForInterrupt)) => Ok(()),
        Ok(exit) => Err(format!("unexpected exit {exit:?}")),
        Err(error) => Err(format!("{error:?}")),
    }
}

#[test]
fn a_store_into_a_line_another_core_holds_dirty_is_refused() {
    let (core0, core1, _backing) = cluster();
    execute(&core1, 0x1000, TABLE, &[STRB]).unwrap();
    let error = execute(&core0, 0x1100, STATIC, &[STRB]).unwrap_err();
    assert!(error.contains("data-cache coherence hazard"), "{error}");
    assert!(
        error.contains("processor 0 writes 1 byte(s) at 0x2008"),
        "{error}"
    );
    assert!(error.contains("physical line 0x2000..0x2020"), "{error}");
    assert!(error.contains("processor 1's data cache"), "{error}");
    assert!(error.contains("store at pc 0x1000"), "{error}");
}

#[test]
fn a_load_from_a_line_another_core_holds_dirty_is_refused() {
    let (core0, core1, _backing) = cluster();
    execute(&core1, 0x1000, TABLE, &[STRB]).unwrap();
    let error = execute(&core0, 0x1100, STATIC, &[LDRB]).unwrap_err();
    assert!(
        error.contains("processor 0 reads 1 byte(s) at 0x2008"),
        "{error}"
    );
}

#[test]
fn an_uncached_store_under_a_dirty_line_is_refused() {
    let (core0, core1, _backing) = cluster();
    execute(&core1, 0x1000, TABLE, &[STRB]).unwrap();
    let error = execute(&core0, 0x1100, UNCACHED | STATIC, &[STRB]).unwrap_err();
    assert!(error.contains("at 0x40002008 (NonCacheable"), "{error}");
}

#[test]
fn tables_written_through_the_uncached_alias_share_lines_safely() {
    // The fix: the tables use the uncached alias, so no cache holds them.
    let (core0, core1, _backing) = cluster();
    execute(&core1, 0x1000, UNCACHED | TABLE, &[STRB]).unwrap();
    execute(&core0, 0x1100, STATIC, &[STRB, LDRB]).unwrap();
}

#[test]
fn cleaning_by_address_or_by_set_and_way_releases_the_line() {
    for clean in [DCCMVAC, DCCISW] {
        let (core0, core1, _backing) = cluster();
        execute(&core1, 0x1000, TABLE, &[STRB, clean]).unwrap();
        execute(&core0, 0x1100, STATIC, &[STRB]).unwrap();
        // CPU0 now holds the line; core 1 must not read it either.
        let error = execute(&core1, 0x1200, TABLE, &[LDRB]).unwrap_err();
        assert!(error.contains("processor 0's data cache"), "{error}");
    }
}

#[test]
fn maintenance_through_the_uncached_alias_cleans_nothing() {
    let (core0, core1, _backing) = cluster();
    execute(&core1, 0x1000, TABLE, &[STRB]).unwrap();
    execute(&core1, 0x1200, UNCACHED | TABLE, &[DCCMVAC]).unwrap();
    assert!(execute(&core0, 0x1100, STATIC, &[STRB]).is_err());
}

#[test]
fn a_disabled_data_cache_holds_no_lines() {
    let (core0, core1, _backing) = cluster();
    let sctlr = core1.coprocessor(system(1, 0, 0), None).unwrap();
    core1
        .coprocessor(system(1, 0, 0), Some(sctlr & !0b100))
        .unwrap();
    execute(&core1, 0x1000, TABLE, &[STRB]).unwrap();
    execute(&core0, 0x1100, STATIC, &[STRB]).unwrap();
}

#[test]
fn snapshots_restore_dirty_lines() {
    let (core0, core1, _backing) = cluster();
    let clean = core1.capture(None).unwrap();
    execute(&core1, 0x1000, TABLE, &[STRB]).unwrap();
    let dirty = core1.capture(None).unwrap();
    let exported = core1.import_captured(&dirty.export().unwrap()).unwrap();
    core1.restore_captured(clean.as_ref()).unwrap();
    execute(&core0, 0x1100, STATIC, &[LDRB]).unwrap();
    for saved in [dirty.as_ref(), exported.as_ref()] {
        core1.restore_captured(saved).unwrap();
        assert!(execute(&core0, 0x1100, STATIC, &[LDRB]).is_err());
    }
}
