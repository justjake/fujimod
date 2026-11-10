//! Memory watch on A-profile processors: every record is decided on the
//! physical address the processor used, so stores through any MMU alias of a
//! watched range are recorded and stores outside it are not.
use super::bus::{Bus, Mode};
use super::factory::{Engine, Graph, MemoryMap, Options, Processor, Ram};
use super::memory::BusMapping;
use super::*;
use fujimod_emulator_cpu::{Model, Register, arm, timeline::Ratio};

const CODE: u64 = 0x1000;
const PAGE: u64 = 0x100000;
const OTHER: u64 = 0x200000;
const WATCH: (u64, u64) = (PAGE, PAGE + 4);

fn a7_graph() -> Graph {
    Graph {
        processors: vec![Processor {
            model: Model::CortexA7,
            registers: vec![(Register::Pc, CODE), (Register::Pstate, 0xd3)],
            released: true,
            mprofile: None,
            rprofile: None,
        }],
        ram: vec![Ram {
            base: 0,
            size: 0x400000,
        }],
        dma_mappings: vec![BusMapping::ram(0, 0x400000)],
        decoders: vec![],
        mappings: vec![MemoryMap {
            core: 0,
            address: 0,
            physical: 0,
            size: 0x400000,
            permissions: 7,
        }],
        peripherals: vec![],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    }
}

fn a7_board() -> Board {
    let mut board = a7_graph()
        .build(
            Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20),
            Bus::new(Mode::Strict),
            None,
        )
        .unwrap();
    board
        .configure_armv7a(
            0,
            armv7a::Config {
                mpidr: 0x80000000,
                configuration_base: 0xffff0000,
                cycles_per_tick: 1,
                cache_sizes: None,
            },
        )
        .unwrap();
    board
}

fn words(board: &Board, address: u64, code: &[u32]) {
    board
        .write_physical(
            address,
            &code
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
}

fn cp15(cpu: &dyn Cpu, crn: u32, crm: u32, opc2: u32, value: u64) {
    cpu.coprocessor(
        arm::Coprocessor {
            cp: 15,
            opc1: 0,
            crn,
            crm,
            opc2,
            secure: 0,
            wide: false,
        },
        Some(value),
    )
    .unwrap();
}

/// Section descriptor: 1 MiB section at `physical`, full access, cacheable.
fn section(physical: u64) -> u32 {
    (physical as u32 & 0xfff0_0000) | 0x0c0e
}

/// Short-descriptor page tables: the low 4 MiB identity, VA 0x80000000 and
/// VA 0x90000000 both on the watched page, and VA 0x98000000 on `alias`
/// (one more MiB of the same RAM unless a test points it at a board alias).
struct Mmu {
    table: u64,
    alias: u64,
}

impl Mmu {
    fn new(board: &Board, physical: u64) -> Self {
        let mmu = Self {
            table: 0x4000,
            alias: physical + 0x100000,
        };
        mmu.apply(board);
        let cpu = board.cpu(0).unwrap();
        cp15(cpu, 2, 0, 2, 0); // TTBCR: TTBR0 covers everything.
        cp15(cpu, 2, 0, 0, mmu.table); // TTBR0.
        cp15(cpu, 3, 0, 0, 1); // DACR: domain 0 client.
        cp15(cpu, 1, 0, 0, 0x0080_0001); // SCTLR.XP | SCTLR.M
        mmu
    }
    fn write(&self, board: &Board, identity: &[u32]) {
        let mut table = vec![0u32; 4096];
        table[..identity.len()].copy_from_slice(identity);
        table[0x800] = section(WATCH.0); // VA 0x80000000
        table[0x900] = section(WATCH.0); // VA 0x90000000
        table[0x980] = section(self.alias); // VA 0x98000000
        board
            .write_physical(
                self.table,
                &table
                    .iter()
                    .flat_map(|w| w.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    /// Rewrite the tables with `alias` at VA 0x98000000.
    fn apply(&self, board: &Board) {
        let identity: Vec<u32> = (0..4).map(|i| section((i as u64) << 20)).collect();
        self.write(board, &identity);
    }
    /// Remap VA 0x80000000 onto `physical` in the live page table; the guest
    /// must flush its TLB (`tlbiall`) before the change is visible.
    fn remap(&self, board: &Board, physical: u64) {
        board
            .write_physical(self.table + 0x800 * 4, &section(physical).to_le_bytes())
            .unwrap();
    }
}

fn movw(register: u32, low: u16) -> u32 {
    0xe300_0000 | ((low as u32 >> 12) << 16) | (register << 12) | (low as u32 & 0xfff)
}

fn movt(register: u32, high: u16) -> u32 {
    0xe340_0000 | ((high as u32 >> 12) << 16) | (register << 12) | (high as u32 & 0xfff)
}

/// `movw/movt r0, va; mov r1, #value; str r1, [r0, #offset]`, in four words.
fn store_at(va: u32, value: u32, offset: u32) -> [u32; 4] {
    [
        movw(0, va as u16),
        movt(0, (va >> 16) as u16),
        0xe3a0_1000 | value,
        0xe580_1000 | offset,
    ]
}

fn records(watch: &observation::MemoryWatch) -> Vec<serde_json::Value> {
    watch.records.borrow().state()["records"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn run(board: &mut Board, instructions: u64) {
    board.run_for_instructions(instructions, 1).unwrap();
}

#[test]
fn a7_watch_records_both_page_table_aliases_with_the_physical_address() {
    let mut board = a7_board();
    let _mmu = Mmu::new(&board, PAGE);
    let mut code = Vec::new();
    code.extend(store_at(0x8000_0000, 0x33, 0)); // VA 0x80000000 -> PA 0x100000
    code.extend(store_at(0x9000_0000, 0x11, 0)); // VA 0x90000000 -> PA 0x100000
    code.push(0xeaff_fffe); // b .
    words(&board, CODE, &code);
    let mut watch = board
        .memory_watch_begin(vec![WATCH], 8, board.mappings().to_vec())
        .unwrap();
    run(&mut board, code.len() as u64 - 1);
    let records = records(&watch);
    assert_eq!(records.len(), 2, "{records:?}");
    for (record, (storing_pc, value)) in records
        .iter()
        .zip([(CODE + 0xc, 0x33), (CODE + 0x1c, 0x11)])
    {
        assert_eq!(record["source"], "cpu-write");
        assert_eq!(record["physical"], PAGE);
        assert_eq!(
            record["address"], PAGE,
            "physical address as the CPU used it"
        );
        assert_eq!(record["size"], 4);
        assert_eq!(record["value"], value);
        assert_eq!(record["core"], 0);
        assert_eq!(record["pc"], storing_pc);
        assert_ne!(record["instructions"], serde_json::Value::Null);
        assert_ne!(record["tick"], serde_json::Value::Null);
    }
    let mut bytes = [0u8; 4];
    board.read_physical(PAGE, &mut bytes).unwrap();
    assert_eq!(bytes, 0x11u32.to_le_bytes());
    assert_eq!(board.memory_watch_end(&mut watch), Ok(()));
}

#[test]
fn memory_watch_samples_per_core_ram_at_each_read() {
    let mut board = a7_board();
    let mut code = store_at(0x2000, 0x11, 0).to_vec();
    code.extend([0xe300_0000, 0xe340_0010, 0xe590_1000]); // r0 = 0x100000; ldr r1, [r0]
    code.extend(store_at(0x2000, 0x22, 0));
    code.extend([0xe300_0000, 0xe340_0010, 0xe590_1000, 0xeaff_fffe]);
    words(&board, CODE, &code);
    let mut watch = board.memory_watch_begin_with_samples(
        vec![WATCH], 8, board.mappings().to_vec(), vec![0x2000], vec![], None,
    ).unwrap();
    run(&mut board, code.len() as u64 - 1);
    let rows = records(&watch);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["sample"], 0x11);
    assert_eq!(rows[1]["sample"], 0x22);
    assert_eq!(rows[0]["pc"], CODE + 24);
    board.memory_watch_end(&mut watch).unwrap();
}

#[test]
fn memory_watch_captures_bounded_pointer_chain_at_read() {
    let mut board = a7_board();
    words(&board, 0x2000, &[0x2100]);
    words(&board, 0x2108, &[0x2200]);
    words(&board, 0x2208, &[0x3300]);
    words(&board, CODE, &[0xe300_0000, 0xe340_0010, 0xe590_1000, 0xeaff_fffe]);
    let mut watch = board.memory_watch_begin_with_samples(
        vec![WATCH], 8, board.mappings().to_vec(), vec![0x2000], vec![8, 8], Some((0x2000, 0x3000)),
    ).unwrap();
    run(&mut board, 3);
    let rows = records(&watch);
    assert_eq!(rows[0]["sample_path"], serde_json::json!([0x2100, 0x2200, 0x3300]));
    board.memory_watch_end(&mut watch).unwrap();
}

#[test]
fn memory_watch_detects_the_loss_of_its_board() {
    let board = a7_board();
    let watch = board.memory_watch_begin(vec![WATCH], 8, board.mappings().to_vec()).unwrap();
    assert!(watch.is_live());
    drop(board);
    assert!(!watch.is_live());
}

#[test]
fn a7_watch_records_a_board_alias_window_with_the_canonical_physical_address() {
    // The XPP maps DRAM at 0 and a second, uncached window at 0x40000000 over
    // the same backing. A page table can put either address on a watched byte.
    let mut graph = a7_graph();
    graph.mappings.push(MemoryMap {
        core: 0,
        address: 0x4000_0000,
        physical: 0,
        size: 0x400000,
        permissions: 7,
    });
    let mut board = graph
        .build(
            Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20),
            Bus::new(Mode::Strict),
            None,
        )
        .unwrap();
    board
        .configure_armv7a(
            0,
            armv7a::Config {
                mpidr: 0x80000000,
                configuration_base: 0xffff0000,
                cycles_per_tick: 1,
                cache_sizes: None,
            },
        )
        .unwrap();
    let mut mmu = Mmu::new(&board, PAGE);
    mmu.alias = 0x4010_0000;
    mmu.apply(&board);
    let mut code = Vec::new();
    code.extend(store_at(0x8000_0000, 0x33, 0)); // PA 0x100000 directly
    code.extend(store_at(0x9800_0000, 0x11, 0)); // engine 0x40100000, alias window
    code.push(0xeaff_fffe);
    words(&board, CODE, &code);
    let mut watch = board
        .memory_watch_begin(vec![WATCH], 8, board.mappings().to_vec())
        .unwrap();
    run(&mut board, code.len() as u64 - 1);
    let records = records(&watch);
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0]["address"], PAGE);
    assert_eq!(
        records[1]["address"], 0x4010_0000,
        "the address the CPU used"
    );
    for record in &records {
        assert_eq!(record["physical"], PAGE, "canonical physical identity");
    }
    board.memory_watch_end(&mut watch).unwrap();
}

#[test]
fn a7_watch_ignores_stores_outside_the_range_and_other_pages() {
    let mut board = a7_board();
    let _mmu = Mmu::new(&board, PAGE);
    let mut code = Vec::new();
    code.extend(store_at(0x8000_0000, 0x33, 0)); // in range
    code.extend(store_at(0x9000_0000, 0x44, 4)); // same page, range + 4
    code.extend(store_at(0x9800_0000, 0x55, 0)); // other page
    code.push(0xeaff_fffe);
    words(&board, CODE, &code);
    let mut watch = board
        .memory_watch_begin(vec![WATCH], 8, board.mappings().to_vec())
        .unwrap();
    run(&mut board, code.len() as u64 - 1);
    let records = records(&watch);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["physical"], PAGE);
    assert_eq!(records[0]["value"], 0x33);
    board.memory_watch_end(&mut watch).unwrap();
    let mut bytes = [0u8; 4];
    board.read_physical(PAGE + 4, &mut bytes).unwrap();
    assert_eq!(bytes, 0x44u32.to_le_bytes());
    board.read_physical(OTHER, &mut bytes).unwrap();
    assert_eq!(bytes, 0x55u32.to_le_bytes());
}

#[test]
fn a7_watch_follows_a_page_table_remap_after_a_tlb_flush() {
    let mut board = a7_board();
    let mmu = Mmu::new(&board, PAGE);
    let mut first = store_at(0x8000_0000, 0x33, 0).to_vec();
    first.push(0xeaff_fffe);
    words(&board, CODE, &first);
    let mut watch = board
        .memory_watch_begin(vec![WATCH], 8, board.mappings().to_vec())
        .unwrap();
    run(&mut board, first.len() as u64 - 1);
    assert_eq!(records(&watch).len(), 1, "first mapping is watched");
    // VA 0x80000000 now points elsewhere; VA 0x90000000 still reaches the page.
    mmu.remap(&board, OTHER);
    let mut second = vec![0xee08_0f17]; // mcr p15, 0, r0, c8, c7, 0 (TLBIALL)
    second.extend(store_at(0x8000_0000, 0x44, 0)); // remapped: not watched
    second.extend(store_at(0x9000_0000, 0x55, 0)); // still watched
    second.push(0xeaff_fffe);
    words(&board, CODE + 0x40, &second);
    board.write_register(0, Register::Pc, CODE + 0x40).unwrap();
    run(&mut board, second.len() as u64 - 1);
    let seen = records(&watch);
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[1]["physical"], PAGE);
    assert_eq!(seen[1]["value"], 0x55);
    assert_eq!(seen[1]["pc"], CODE + 0x40 + 0x20);
    let mut bytes = [0u8; 4];
    board.read_physical(OTHER, &mut bytes).unwrap();
    assert_eq!(bytes, 0x44u32.to_le_bytes());
    board.memory_watch_end(&mut watch).unwrap();
}

#[test]
fn a7_watch_is_observation_only() {
    fn boot() -> Board {
        let board = a7_board();
        Mmu::new(&board, PAGE);
        let mut code = Vec::new();
        code.extend(store_at(0x8000_0000, 0x33, 0));
        code.extend(store_at(0x9000_0000, 0x11, 0));
        code.push(0xeaff_fffe);
        words(&board, CODE, &code);
        board
    }
    let mut plain = boot();
    run(&mut plain, 12);
    let mut watched = boot();
    let mut watch = watched
        .memory_watch_begin(vec![WATCH], 8, watched.mappings().to_vec())
        .unwrap();
    run(&mut watched, 12);
    assert_eq!(records(&watch).len(), 2);
    watched.memory_watch_end(&mut watch).unwrap();
    assert_eq!(plain.complete_state().unwrap(), watched.complete_state().unwrap());
    assert_eq!(plain.instruction_counts(), watched.instruction_counts());
}

// The R-profile path is unchanged: an identity-mapped store is recorded with
// the same fields as before.
fn r5f_board() -> Board {
    Graph {
        processors: vec![Processor {
            model: Model::CortexR5F,
            registers: vec![(Register::Pc, 0x10000)],
            released: true,
            mprofile: None,
            rprofile: Some(Default::default()),
        }],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x4000,
        }],
        dma_mappings: vec![BusMapping::ram(0x10000, 0x4000)],
        decoders: vec![],
        mappings: vec![MemoryMap {
            core: 0,
            address: 0x10000,
            physical: 0x10000,
            size: 0x4000,
            permissions: 7,
        }],
        peripherals: vec![],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    }
    .build(
        Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20),
        Bus::new(Mode::Strict),
        None,
    )
    .unwrap()
}

#[test]
fn r_profile_watch_still_records_an_identity_store() {
    let mut board = r5f_board();
    // mov r1, #0x11; str r1, [r0] with r0 = 0x10080 set by the host.
    words(&board, 0x10000, &[0xe3a0_1011, 0xe580_1000, 0xeaff_fffe]);
    board.write_register(0, arm::CORE[0], 0x10080).unwrap();
    let mut watch = board
        .memory_watch_begin(vec![(0x10080, 0x10084)], 8, board.mappings().to_vec())
        .unwrap();
    run(&mut board, 2);
    let records = records(&watch);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["source"], "cpu-write");
    assert_eq!(records[0]["address"], 0x10080);
    assert_eq!(records[0]["physical"], 0x10080);
    assert_eq!(records[0]["pc"], 0x10004);
    assert_eq!(records[0]["value"], 0x11);
    board.memory_watch_end(&mut watch).unwrap();
}

#[test]
fn r_profile_watch_samples_task_context_at_instruction_execution() {
    let mut board = r5f_board();
    words(&board, 0x10000, &[0xe1a0_0000, 0xeaff_fffe]);
    words(&board, 0x10100, &[0x10200]);
    words(&board, 0x10208, &[0x10300]);
    let mut watch = board.memory_watch_begin_with_samples(
        vec![(0x10000, 0x10004)], 8, board.mappings().to_vec(),
        vec![0x10100], vec![8], Some((0x10000, 0x14000)),
    ).unwrap();
    run(&mut board, 1);
    let rows = records(&watch);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["source"], "cpu-fetch");
    assert_eq!(rows[0]["pc"], 0x10000);
    assert_eq!(rows[0]["sample_path"], serde_json::json!([0x10200, 0x10300]));
    board.memory_watch_end(&mut watch).unwrap();
}

#[test]
fn many_watch_ranges_keep_exact_filtering_after_backend_grouping() {
    let mut board = r5f_board();
    words(&board, 0x10000, &[0xe3a0_1011, 0xe580_1000, 0xe580_1004, 0xeaff_fffe]);
    board.write_register(0, arm::CORE[0], 0x10080).unwrap();
    let ranges = (0..300).map(|i| (0x10080 + i * 16, 0x10084 + i * 16)).collect();
    let mut watch = board.memory_watch_begin(ranges, 8, board.mappings().to_vec()).unwrap();
    run(&mut board, 3);
    let rows = records(&watch);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["source"], "cpu-write");
    assert_eq!(rows[0]["physical"], 0x10080);
    board.memory_watch_end(&mut watch).unwrap();
}

#[test]
fn watch_refuses_processors_it_cannot_observe() {
    let mut graph = a7_graph();
    graph.processors[0].model = Model::ArmV8A;
    graph.processors[0].registers = vec![(Register::Pstate, 0x3c5)];
    let board = graph
        .build(
            Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20),
            Bus::new(Mode::Strict),
            None,
        )
        .unwrap();
    let error = board
        .memory_watch_begin(vec![WATCH], 8, board.mappings().to_vec())
        .err()
        .expect("an unobservable processor must be refused");
    assert!(error.contains("R5F or A7"), "{error}");
}

/// Measurement, not a test: `cargo test -p fujimod-emulator --lib
/// a7_watch_cost -- --ignored --nocapture`.
///
/// An A7 loop stores into a 64 KiB buffer. Three scenarios measure the watch
/// overhead in the shapes the mod audit uses: a cold watched range (the
/// persistent-settings case, written only occasionally), the same loop with
/// no watch at all, and a watched byte the loop stores to every iteration
/// (one record per store).
#[test]
#[ignore = "measurement"]
fn a7_watch_cost_measurement() {
    // r2 = buffer VA 0x20000 (identity section), r3 = 0x80000000, r5 = length.
    fn board(hot: bool) -> Board {
        let mut code = vec![
            movw(2, 0x0000),
            movt(2, 0x0002),
            movw(3, 0x0000),
            movt(3, 0x8000),
            movw(5, 0x0000),
            movt(5, 0x0001),
            0xe3a0_105a, // mov r1, #0x5a
            0xe280_0004, // inner: add r0, r0, #4
            0xe782_1000, // str r1, [r2, r0]
        ];
        if hot {
            code.push(0xe583_1000); // str r1, [r3]
        }
        code.extend([
            0xe150_0005, // cmp r0, r5
            0x1aff_fffa, // bne inner
            0xe3a0_0000, // mov r0, #0
            0xeaff_fff5, // b inner
        ]);
        let board = a7_board();
        Mmu::new(&board, PAGE);
        words(&board, CODE, &code);
        board
    }
    fn measure(name: &str, hot: bool, watch: Option<(u64, u64)>, budget: u64) {
        let mut board = board(hot);
        let mut active = watch.map(|range| {
            board
                .memory_watch_begin(vec![range], 1 << 20, board.mappings().to_vec())
                .unwrap()
        });
        let start = std::time::Instant::now();
        run(&mut board, budget);
        let elapsed = start.elapsed();
        let records = active.as_ref().map_or(0, |w| records(w).len());
        println!(
            "{name}: {budget} instructions in {elapsed:?} = {:.2} MIPS, {records} records, counts {:?}",
            budget as f64 / elapsed.as_secs_f64() / 1e6,
            board.instruction_counts()
        );
        if let Some(watch) = active.as_mut() {
            board.memory_watch_end(watch).unwrap();
        }
    }
    let budget = 20_000_000;
    measure("unwatched", false, None, budget);
    measure("cold watch", false, Some(WATCH), budget);
    // The hot store is on the watched page but outside the range: the
    // backend's page filter slows the access, the watch records nothing.
    measure(
        "same page, no record",
        true,
        Some((PAGE + 4, PAGE + 8)),
        budget,
    );
    measure("record per store", true, Some(WATCH), 2_000_000);
    measure("record-free baseline", true, None, 2_000_000);
}

#[test]
fn fresh_epoch_preserves_host_time_and_instruction_totals() {
    let mut board = a7_graph().build(
        Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20)
            .with_epoch(9000, vec![321]),
        Bus::new(Mode::Strict), None,
    ).unwrap();
    assert_eq!(board.now(), 9000);
    assert_eq!(board.instruction_counts(), [321]);
    assert!(board.epoch_is_unexecuted());
    words(&board, CODE, &[0xe1a00000, 0xeafffffe]);
    let initial = board.checkpoint(None).unwrap();
    board.run_for_instructions(4, 2).unwrap();
    assert_eq!(board.instruction_counts(), [325]);
    assert!(!board.epoch_is_unexecuted());
    assert_eq!(board.now(), 9004);
    board.restore_keyframe(&initial).unwrap();
    assert!(board.epoch_is_unexecuted());
    assert_eq!(board.instruction_counts(), [321]);
    assert!(a7_graph().build(
        Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20)
            .with_epoch(9000, vec![1, 2]),
        Bus::new(Mode::Strict), None,
    ).is_err());
}

#[test]
fn supply_off_deadline_stops_execution_and_only_power_time_can_advance() {
    struct SupplyOff { due: Option<u64> }
    impl fujimod_emulator_devices::Device for SupplyOff {
        fn name(&self) -> &'static str { "test-supply" }
        fn read(&mut self, _: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8) -> u64 { 0 }
        fn write(&mut self, _: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8, _: u64) {}
        fn next_deadline(&self, _: u64) -> Option<u64> { self.due }
        fn advance(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>) {
            if self.due.is_some_and(|due| due <= cx.now()) {
                self.due = None;
                cx.power_off().unwrap();
            }
        }
    }
    struct LateDma;
    impl fujimod_emulator_devices::Device for LateDma {
        fn name(&self) -> &'static str { "test-late-dma" }
        fn read(&mut self, _: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8) -> u64 { 0 }
        fn write(&mut self, _: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8, _: u64) {}
        fn next_deadline(&self, _: u64) -> Option<u64> { Some(12) }
        fn advance(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>) {
            if cx.now() >= 12 { cx.ram_write(0x2000, &[0x7f]).unwrap(); }
        }
    }
    let mut bus = Bus::new(Mode::Strict);
    bus.attach(Box::new(SupplyOff { due: Some(8) }));
    bus.attach(Box::new(LateDma));
    let mut board = a7_graph().build(
        Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20),
        bus, None,
    ).unwrap();
    words(&board, CODE, &[0xeafffffe]);
    assert!(board.advance_unpowered(1).is_err());
    board.run_for_instructions(100, 2).unwrap();
    assert!(board.is_powered_off());
    assert_eq!(board.now(), 8);
    let counts = board.instruction_counts();
    assert!(board.run_for_instructions(1, 2).is_err());
    assert!(board.run_until(20, 2).is_err());
    assert!(board.state()["failure"].is_null());
    board.advance_unpowered(50).unwrap();
    assert_eq!(board.now(), 50);
    assert_eq!(board.instruction_counts(), counts);
    let mut byte = [0];
    board.read_physical(0x2000, &mut byte).unwrap();
    assert_eq!(byte, [0]);
    assert!(board.advance_unpowered(49).is_err());
}

#[test]
fn connected_clock_removes_power_at_the_guest_hold_deadline() {
    use fujimod_emulator_devices::exr2::{clock, wake_alarm::{Supply, WakeAlarm}};
    use std::sync::{Arc, Mutex};
    let power = Arc::new(Mutex::new(WakeAlarm::new(12_000_000, clock::POWER_HOLD_RELEASE_LIMIT_TICKS).unwrap()));
    let clocks = clock::Clocks::new(clock::Options { output_settle_ticks: 2, reset_ready: true }, ()).unwrap();
    clocks.output().attach_power(power.clone()).unwrap();
    let mut bus = Bus::new(Mode::Strict);
    bus.register(clock::BASE, 0x100, Box::new(clocks)).unwrap();
    let mut graph = a7_graph();
    graph.peripherals.push(super::factory::Peripheral { core: 0, address: clock::BASE, physical: clock::BASE, size: 0x100 });
    let mut board = graph.build(Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20), bus, None).unwrap();
    let mut code = store_at(clock::BASE as u32 + 0x84, 0, 0).to_vec();
    code.push(0xeafffffe);
    words(&board, CODE, &code);
    board.run_for_instructions(200_000, 1000).unwrap();
    assert!(board.is_powered_off());
    assert_eq!(power.lock().unwrap().supply(), Supply::Off);
    assert!(board.now() >= clock::POWER_HOLD_RELEASE_LIMIT_TICKS);
}
