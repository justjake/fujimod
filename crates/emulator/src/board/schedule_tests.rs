use super::*;
use fujimod_emulator_cpu::Model;
use fujimod_emulator_cpu::memory::Backing;
use fujimod_emulator_cpu_unicorn::{Library, rprofile};
use fujimod_emulator_devices::{Cx, Device};
use std::sync::{Arc, Mutex};

struct ResetLine(Arc<Mutex<Vec<u64>>>);
impl Device for ResetLine {
    fn name(&self) -> &'static str {
        "test-reset-line"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, bytes: u8, value: u64) {
        if bytes != 4 || value != 1 {
            cx.unmodeled();
            return;
        }
        self.0.lock().unwrap().push(cx.now());
        if let Err(error) = cx.reset_core(1, false) {
            cx.fail(error);
        }
    }
    fn save(&self) -> Value {
        json!({"writes": *self.0.lock().unwrap()})
    }
}
fn board(counted: bool) -> (Board, Arc<Mutex<Vec<u64>>>) {
    let library = Library::linked().unwrap();
    let backing = Backing::aligned_zeros(0x4000, 4096).unwrap();
    let memory = rprofile::memory::Memory::default();
    let cpus = (0..2)
        .map(|core| {
            let raw = library.open(Model::CortexR5F, 1024 * 1024).unwrap();
            let cpu: Box<dyn Cpu> = if counted {
                Box::new(
                    rprofile::core::Core::new(
                        raw,
                        rprofile::core::Options::default(),
                        rprofile::core::Wiring {
                            memory: memory.clone(),
                        },
                    )
                    .unwrap(),
                )
            } else {
                Box::new(raw)
            };
            unsafe {
                cpu.map_backing(
                    0,
                    0x4000,
                    7,
                    std::ptr::NonNull::new(backing.as_ptr()).unwrap(),
                )
                .unwrap();
            }
            set(&*cpu, Register::Pc, 0x1000 + core * 0x1000).unwrap();
            set(&*cpu, Register::Pstate, 0xd3).unwrap();
            set(&*cpu, fujimod_emulator_cpu::arm::CORE[1], 0x8000000).unwrap();
            set(&*cpu, fujimod_emulator_cpu::arm::CORE[2], 1).unwrap();
            cpu
        })
        .collect();
    let writes = Arc::new(Mutex::new(Vec::with_capacity(1)));
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(0x8000000, 4, Box::new(ResetLine(writes.clone())))
        .unwrap();
    let mut board = Board::new(
        cpus,
        bus,
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true, false],
        memory::Memory::new(vec![(0, backing)], vec![memory::BusMapping::ram(0, 0x4000)]).unwrap(),
    )
    .unwrap();
    board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
    for (address, code) in [
        (
            0x1000,
            vec![
                0xe3a00043u32,
                0xe2500001,
                0x1afffffd,
                0xe5812000,
                0xe2833001,
                0xeafffffd,
            ],
        ),
        (0x2000, vec![0xe2844001, 0xeafffffd]),
    ] {
        board
            .write_memory(
                0,
                address,
                &code
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    (board, writes)
}

#[test]
fn a_parked_peer_does_not_cap_the_counted_run_at_quantum_boundaries() {
    let (mut board, _) = board(true);
    board
        .write_memory(
            0,
            0x1000,
            &[0xe2800001u32, 0xeafffffd]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    board.run_for_instructions(10007, 50).unwrap();
    assert_eq!(board.instruction_counts(), vec![10007, 0]);
    assert_eq!(board.cpu(0).unwrap().instruction_progress(), Some(10007));
    assert_eq!(
        board
            .read_register(0, fujimod_emulator_cpu::arm::CORE[0])
            .unwrap(),
        5004
    );
}

#[test]
fn a_guest_reset_release_tightens_the_active_run_to_the_next_quantum_boundary() {
    for counted in [false, true] {
        let (mut board, writes) = board(counted);
        board.run_until(170, 50).unwrap();
        assert_eq!(*writes.lock().unwrap(), vec![136]);
        assert_eq!(board.instruction_counts(), vec![150, 20]);
        assert_eq!(
            board
                .read_register(0, fujimod_emulator_cpu::arm::CORE[3])
                .unwrap(),
            7
        );
        assert_eq!(
            board
                .read_register(1, fujimod_emulator_cpu::arm::CORE[4])
                .unwrap(),
            10
        );
    }
}

#[test]
fn native_software_interrupt_wiring_releases_the_declared_entry_at_the_existing_slice_boundary() {
    use factory::{Engine, Graph, MemoryMap, Options, Peripheral, Processor, Ram};
    use fujimod_emulator_exr2_plan::{Plan, RefuseAdapters};
    use fujimod_emulator_exr2_resources::Resources;
    for pause in [None, Some(100), Some(137)] {
        let plan=Plan::parse(&serde_json::to_vec(&json!({
            "schema":1,"boot_entry":"loader","wires":{"release":"software-pulse"},"nodes":[
                {"name":"irq1","boot_entries":["loader"],"windows":[{"base":0x8000000,"size":0x1000}],"dependencies":[],
                    "model":{"kind":"exr2-interrupt-controller","options":{"arbitration":"unique"},"core":1,"base":0x8000000,"routes":null,
                        "software":[{"line":120,"pulse":"release"}]}},
                {"name":"release","boot_entries":["loader"],"windows":[],"dependencies":[],
                    "model":{"kind":"core-release","options":{"released":false},"core":1,"request":"release"}}
            ]
        })).unwrap()).unwrap();
        plan.validate_reset_outputs(&[true, false]).unwrap();
        fujimod_emulator_exr2_board::register_board_builder();
        fujimod_emulator_models::register();
        let built = plan
            .build(Resources::default(), 0, &mut RefuseAdapters)
            .unwrap();
        let graph = Graph {
            processors: (0..2)
                .map(|core| Processor {
                    model: Model::CortexR5F,
                    mprofile: None,
                    rprofile: Some(Default::default()),
                    released: core == 0,
                    registers: vec![
                        (Register::Pc, 0x10000 + core as u64 * 0x1000),
                        (Register::Pstate, 0xd3),
                        (fujimod_emulator_cpu::arm::CORE[1], 0x80000cc),
                        (fujimod_emulator_cpu::arm::CORE[2], 1 << 24),
                    ],
                })
                .collect(),
            ram: vec![Ram {
                base: 0x10000,
                size: 0x4000,
            }],
            dma_mappings: vec![memory::BusMapping::ram(0x10000, 0x4000)],
            decoders: vec![],
            mappings: (0..2)
                .map(|core| MemoryMap {
                    core,
                    address: 0x10000,
                    physical: 0x10000,
                    size: 0x4000,
                    permissions: 7,
                })
                .collect(),
            peripherals: (0..2)
                .map(|core| Peripheral {
                    core,
                    address: 0x8000000,
                    physical: 0x8000000,
                    size: 0x1000,
                })
                .collect(),
            instructions_per_tick: Ratio::new(1, 1).unwrap(),
        };
        let mut b = graph
            .build(
                Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
                Bus::from_graph(bus::Mode::Strict, built.graph).unwrap(),
                None,
            )
            .unwrap();
        for c in built.core_interrupts {
            b.configure_core_interrupts(c.core, c.device, Box::new(c.inputs))
                .unwrap();
        }
        for (address, words) in [
            (
                0x10000,
                vec![
                    0xe3a00043u32,
                    0xe2500001,
                    0x1afffffd,
                    0xe5812000,
                    0xe2833001,
                    0xeafffffd,
                ],
            ),
            (0x11000, vec![0xe2844001, 0xeafffffd]),
        ] {
            b.write_physical(
                address,
                &words
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        }
        if let Some(at) = pause {
            b.run_until(at, 50).unwrap();
        }
        b.run_until(170, 50).unwrap();
        assert_eq!(b.instruction_counts(), [150, 20]);
        assert_eq!(
            b.read_register(0, fujimod_emulator_cpu::arm::CORE[3])
                .unwrap(),
            7
        );
        assert_eq!(
            b.read_register(1, fujimod_emulator_cpu::arm::CORE[4])
                .unwrap(),
            10
        );
        assert_eq!(b.state()["bus"]["devices"][1]["state"]["released"], true);
    }
}

#[test]
fn host_pauses_preserve_the_phase_before_during_and_after_a_peer_wakes() {
    for counted in [false, true] {
        for quantum in [1, 50] {
            let simulate = |pause| {
                let (mut board, _) = board(counted);
                if pause != 0 {
                    board.run_until(pause, quantum).unwrap();
                }
                board.run_until(170, quantum).unwrap();
                (
                    board.state(),
                    [
                        board.read_register(0, Register::Pc).unwrap(),
                        board.read_register(1, Register::Pc).unwrap(),
                    ],
                    [
                        board
                            .read_register(0, fujimod_emulator_cpu::arm::CORE[3])
                            .unwrap(),
                        board
                            .read_register(1, fujimod_emulator_cpu::arm::CORE[4])
                            .unwrap(),
                    ],
                )
            };
            let expected = simulate(0);
            for pause in [37, 49, 50, 51, 100, 125, 135, 136, 137, 149, 150, 155, 170] {
                assert_eq!(
                    simulate(pause),
                    expected,
                    "counted={counted}, quantum={quantum}, pause={pause}"
                );
            }
        }
    }
}

#[test]
fn soft_host_pauses_finish_a_cycle_and_hard_limits_preserve_its_phase() {
    for counted in [false, true] {
        let (mut board, _) = board(counted);
        board.run_with_limits(500, 50, Some(171), None).unwrap();
        assert_eq!(board.instruction_counts(), vec![150, 50]);
        assert_eq!(board.now(), 200);
        // A reused threshold still completes a cycle with both peers ready.
        board.run_with_limits(500, 50, Some(171), None).unwrap();
        assert_eq!(board.instruction_counts(), vec![200, 100]);
        // Tick caps are hard, including in the middle of a processor slice.
        board
            .run_with_limits(100, 50, Some(900), Some(323))
            .unwrap();
        assert_eq!(board.now(), 323);
        assert_eq!(board.instruction_counts(), vec![223, 100]);
        board.run_with_limits(8, 50, Some(900), None).unwrap();
        assert_eq!(board.instruction_counts(), vec![231, 100]);
        // Resuming at a partial slice must match uninterrupted execution.
        let (mut whole, _) = self::board(counted);
        whole.run_for_instructions(331, 50).unwrap();
        assert_eq!(board.state(), whole.state());
        for core in 0..2 {
            for register in [
                Register::Pc,
                fujimod_emulator_cpu::arm::CORE[0],
                fujimod_emulator_cpu::arm::CORE[3],
                fujimod_emulator_cpu::arm::CORE[4],
            ] {
                assert_eq!(
                    board.read_register(core, register).unwrap(),
                    whole.read_register(core, register).unwrap()
                );
            }
        }
    }
}

#[test]
fn solo_soft_pause_batches_full_quanta_and_never_exceeds_the_hard_allowance() {
    let (mut board, _) = board(true);
    board.run_with_limits(100, 50, Some(17), None).unwrap();
    assert_eq!(board.instruction_counts(), vec![50, 0]);
    board.run_with_limits(3, 50, Some(17), None).unwrap();
    assert_eq!(board.instruction_counts(), vec![53, 0]);
}

#[test]
fn passive_pc_witnesses_do_not_change_cpu_phase_devices_or_time() {
    let (mut plain, _) = board(true);
    plain.run_for_instructions(331, 50).unwrap();
    let (mut watched, _) = board(true);
    watched.watch_pcs(&[0x1000, 0x100c, 0x2000]).unwrap();
    assert!(watched.watch_pcs(&[0x1000]).is_err());
    watched.watch_pcs(&[0x3000]).unwrap();
    watched.run_for_instructions(331, 50).unwrap();
    assert_eq!(plain.state(), watched.state());
    let witnesses = watched.pc_entries();
    assert_eq!(witnesses, watched.pc_entries());
    assert_eq!(
        witnesses[0]["argument_bus_memory"][0]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    assert_eq!(witnesses[0]["arguments"][1], json!(0x8000000));
    assert!(witnesses[0]["argument_bus_memory"][1].is_null());
    assert_eq!(
        witnesses
            .iter()
            .map(|r| (r["core"].as_u64().unwrap(), r["pc"].as_u64().unwrap()))
            .collect::<Vec<_>>(),
        [(0, 0x1000), (0, 0x100c), (1, 0x2000)]
    );
    assert!(!watched.quiescent());
    for witness in witnesses {
        assert_eq!(witness["arguments"].as_array().unwrap().len(), 4);
        assert_eq!(witness["argument_bus_memory"].as_array().unwrap().len(), 4);
        for bytes in witness["argument_bus_memory"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_array())
        {
            assert_eq!(bytes.len(), 32);
        }
    }
}

#[test]
fn quiescent_observes_released_processors_without_advancing_time() {
    let (mut board, _) = board(true);
    assert!(!board.quiescent());
    board
        .write_memory(0, 0x1000, &0xe320f003u32.to_le_bytes())
        .unwrap();
    board.run_for_instructions(1, 50).unwrap();
    let before = board.state();
    assert!(board.quiescent()); // The unreleased peer does not prevent idle.
    assert!(board.quiescent());
    assert_eq!(board.state(), before);
}

#[test]
fn progress_reporting_preserves_guest_scheduling_and_state() {
    for counted in [false, true] {
        let (mut observed, observed_writes) = board(counted);
        let (mut plain, plain_writes) = board(counted);
        let progress = Rc::new(RefCell::new(Vec::new()));
        let received = progress.clone();
        observed.set_progress_observer(Some(Box::new(move |count| {
            received.borrow_mut().push(count)
        })));
        observed.run_until(250_000, 50).unwrap();
        plain.run_until(250_000, 50).unwrap();
        assert_eq!(observed.state(), plain.state());
        assert_eq!(
            *observed_writes.lock().unwrap(),
            *plain_writes.lock().unwrap()
        );
        for core in 0..2 {
            for register in [
                Register::Pc,
                Register::Pstate,
                fujimod_emulator_cpu::arm::CORE[3],
                fujimod_emulator_cpu::arm::CORE[4],
            ] {
                assert_eq!(
                    observed.read_register(core, register).unwrap(),
                    plain.read_register(core, register).unwrap()
                );
            }
        }
        let counts = progress.borrow();
        assert!(counts.len() >= 3);
        assert_eq!(counts[0], 0);
        assert!(counts.windows(2).all(|pair| pair[0] < pair[1]));
        drop(counts);
        observed.set_progress_observer(None);
        let count = progress.borrow().len();
        observed.run_until(300_000, 50).unwrap();
        assert_eq!(progress.borrow().len(), count);
    }
}
