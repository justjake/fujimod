use super::*;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_cpu::Model;
use fujimod_emulator_cpu_unicorn::Library;
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct Probe {
    due: Option<u64>,
    calls: Vec<(u64, u64)>,
    value: u64,
}
struct Timer(Arc<Mutex<Probe>>);
impl Device for Timer {
    fn name(&self) -> &'static str {
        "test-timer"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, _: u8) -> u64 {
        self.0.lock().unwrap().calls.push((cx.now(), address));
        self.0.lock().unwrap().value
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, value: u64) {
        let mut p = self.0.lock().unwrap();
        p.value = value;
        p.due = Some(cx.now() + 5);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.0.lock().unwrap().due
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut p = self.0.lock().unwrap();
        if p.due.is_some_and(|at| at <= cx.now()) {
            p.due = None;
            p.value += 1;
            p.calls.push((cx.now(), 0));
        }
    }
    fn save(&self) -> Value {
        let p = self.0.lock().unwrap();
        json!({"due":p.due,"calls":p.calls,"value":p.value})
    }
}
fn build(mode: bus::Mode) -> (Board, Arc<Mutex<Probe>>) {
    build_rprofile(mode, false)
}
fn build_rprofile(mode: bus::Mode, counted: bool) -> (Board, Arc<Mutex<Probe>>) {
    build_rprofile_with_inputs(mode, counted, None)
}
fn build_rprofile_with_inputs(
    mode: bus::Mode,
    counted: bool,
    inputs: Option<fujimod_emulator_devices::exr2::irq::Inputs>,
) -> (Board, Arc<Mutex<Probe>>) {
    let library = Library::linked().unwrap();
    let raw = library.open(Model::CortexR5F, 1024 * 1024).unwrap();
    let cpu: Box<dyn Cpu> = if counted {
        Box::new(
            fujimod_emulator_cpu_unicorn::rprofile::core::Core::new(
                raw,
                fujimod_emulator_cpu_unicorn::rprofile::core::Options::default(),
                fujimod_emulator_cpu_unicorn::rprofile::core::Wiring {
                    memory: Default::default(),
                },
            )
            .unwrap(),
        )
    } else {
        Box::new(raw)
    };
    cpu.map_zeroed(0x10000, 0x1000, 7).unwrap();
    // mov r0,#41; str r0,[r1]; ldr r2,[r1]; b .-4
    let code = [0xe3a00029u32, 0xe5810000, 0xe5912000, 0xeafffffd];
    cpu.write_memory(
        0x10000,
        &code
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    set(
        &*cpu,
        Register::Arm32(fujimod_emulator_cpu::arm::Register::R(1)),
        0x8000000,
    )
    .unwrap();
    set(&*cpu, Register::Pc, 0x10000).unwrap();
    let probe = Arc::new(Mutex::new(Probe::default()));
    let mut bus = Bus::new(mode);
    bus.register(0x8000000, 4, Box::new(Timer(probe.clone())))
        .unwrap();
    let physical_irq = inputs.map(|inputs| {
        use fujimod_emulator_devices::exr2::irq;
        let controller = irq::InterruptController::new(
            irq::Options {
                arbitration: irq::Arbitration::Unique,
            },
            irq::Wiring {
                base: 0xfffb3000,
                inputs: inputs.clone(),
                enables: None,
                software: vec![],
            },
        )
        .unwrap();
        let id = bus
            .register(0xfffb3000, 0x1000, Box::new(controller))
            .unwrap();
        (id, inputs)
    });
    let mut board = Board::new(
        vec![cpu],
        bus,
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true],
        memory::Memory::default(),
    )
    .unwrap();
    board.map_mmio(0, 0x8000000, 0x1000, 0x8000000).unwrap();
    if let Some((id, inputs)) = physical_irq {
        board
            .configure_core_interrupts(0, id, Box::new(inputs))
            .unwrap();
    }
    (board, probe)
}
#[test]
fn counted_rprofile_services_exact_deadlines_and_preserves_host_pause_state() {
    let run = |pause| {
        let (mut board, probe) = build_rprofile(bus::Mode::Strict, true);
        if pause {
            board.run_until(4, 100).unwrap();
        }
        board.run_until(20, 100).unwrap();
        assert!(probe.lock().unwrap().calls.contains(&(7, 0)));
        assert_eq!(board.instruction_counts(), vec![20]);
        (board.state(), board.read_register(0, Register::Pc).unwrap())
    };
    assert_eq!(run(false), run(true));
}
#[test]
fn real_cpu_deadline_service_is_exact_and_quantum_independent() {
    let run = |quantum| {
        let (mut board, probe) = build(bus::Mode::Strict);
        board.run_until(20, quantum).unwrap();
        let saved = board.state();
        assert_eq!(saved["instructions"], json!([20]));
        assert_eq!(saved["timeline"]["now"], 20);
        assert_eq!(saved, board.state()); // host inspection changes nothing
        let p = probe.lock().unwrap();
        assert!(p.calls.contains(&(7, 0)));
        assert_eq!(p.value, 42);
        let mut comparison = saved;
        comparison.as_object_mut().unwrap().remove("slice_left");
        comparison.as_object_mut().unwrap().remove("quantum");
        (comparison, p.calls.clone())
    };
    assert_eq!(run(1), run(100));
}
#[test]
fn strict_unknown_refuses_and_discovery_retains_evidence() {
    let (mut strict, _) = build(bus::Mode::Strict);
    set(
        strict.cpu(0).unwrap(),
        Register::Arm32(fujimod_emulator_cpu::arm::Register::R(1)),
        0x8000004,
    )
    .unwrap();
    assert!(
        strict
            .run_until(5, 20)
            .unwrap_err()
            .contains("unmodeled MMIO write4")
    );
    let (mut discovery, _) = build(bus::Mode::Discovery);
    set(
        discovery.cpu(0).unwrap(),
        Register::Arm32(fujimod_emulator_cpu::arm::Register::R(1)),
        0x8000004,
    )
    .unwrap();
    discovery.run_until(5, 20).unwrap();
    let state = discovery.state();
    assert_eq!(state["execution_mode"], "discovery");
    assert_eq!(state["counts_as_boot"], false);
    assert_eq!(state["bus"]["unmodeled"][0]["address"], 0x8000004);
}

#[test]
fn factory_owns_alias_backing_and_maps_peripherals_to_physical_bus_addresses() {
    use factory::*;
    let options = || Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1024 * 1024);
    let graph = || Graph {
        decoders: vec![],
        processors: vec![Processor {
            rprofile: Some(Default::default()),
            mprofile: None,
            model: Model::CortexR5F,
            registers: vec![(Register::Pc, 0x10000)],
            released: true,
        }],
        dma_mappings: vec![crate::board::memory::BusMapping::ram(0x10000, 0x4000)],
        ram: vec![Ram {
            base: 0x10000,
            size: 0x4000,
        }],
        mappings: vec![
            MemoryMap {
                core: 0,
                address: 0x10000,
                physical: 0x10000,
                size: 0x4000,
                permissions: 7,
            },
            MemoryMap {
                core: 0,
                address: 0x20000,
                physical: 0x10000,
                size: 0x4000,
                permissions: 3,
            },
        ],
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
    bus.register(0xff910000, 4, Box::new(Timer(probe.clone())))
        .unwrap();
    let mut built = graph().build(options(), bus, None).unwrap();
    let cpu = built.cpu(0).unwrap();
    let code = [0xe3a00029u32, 0xe5810000, 0xe5912000, 0xeafffffd];
    cpu.write_memory(
        0x10000,
        &code
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut alias = [0; 4];
    cpu.read_memory(0x20000, &mut alias).unwrap();
    assert_eq!(u32::from_le_bytes(alias), code[0]);
    set(
        cpu,
        Register::Arm32(fujimod_emulator_cpu::arm::Register::R(1)),
        0x8000000,
    )
    .unwrap();
    built.run_until(20, 100).unwrap();
    assert!(probe.lock().unwrap().calls.contains(&(3, 0xff910000)));
    assert!(probe.lock().unwrap().calls.contains(&(7, 0)));
    // Geometry errors refuse before loading a backend or allocating the VM.
    let mut invalid = graph();
    invalid.mappings[1].address = 0x10000;
    assert!(
        invalid
            .build(options(), Bus::new(bus::Mode::Strict), None)
            .err()
            .unwrap()
            .contains("overlapping processor RAM")
    );
}

#[test]
fn declared_peer_dependencies_wake_without_periodic_deadlines() {
    struct Source(Arc<Mutex<bool>>);
    impl Device for Source {
        fn name(&self) -> &'static str {
            "source"
        }
        fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            0
        }
        fn write(&mut self, _: &mut Cx<'_>, _: u64, _: u8, _: u64) {
            *self.0.lock().unwrap() = true;
        }
    }
    struct Dependent {
        ready: Arc<Mutex<bool>>,
        effects: Arc<Mutex<Vec<u64>>>,
    }
    impl Device for Dependent {
        fn name(&self) -> &'static str {
            "dependent"
        }
        fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            cx.unmodeled();
            0
        }
        fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
            cx.unmodeled();
        }
        fn next_deadline(&self, now: u64) -> Option<u64> {
            if *self.ready.lock().unwrap() {
                Some(now)
            } else {
                None
            }
        }
        fn advance(&mut self, cx: &mut Cx<'_>) {
            let mut ready = self.ready.lock().unwrap();
            if *ready {
                *ready = false;
                self.effects.lock().unwrap().push(cx.now());
            }
        }
    }
    struct Services {
        now: u64,
    }
    impl Host for Services {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("guest-only condition cannot invent a polling timer");
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!()
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            Err("unconnected".into())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            Err("unconnected".into())
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            Err("unconnected".into())
        }
        fn record(&mut self, _: Record) {}
    }
    let ready = Arc::new(Mutex::new(false));
    let effects = Arc::new(Mutex::new(vec![]));
    let mut graph = fujimod_emulator_devices::graph::DeviceGraph::new();
    let source = graph
        .register(0x1000, 4, Box::new(Source(ready.clone())))
        .unwrap();
    let target = graph.attach(Box::new(Dependent {
        ready: ready.clone(),
        effects: effects.clone(),
    }));
    graph.depend_on(target, vec![source]).unwrap();
    let mut bus = Bus::from_graph(bus::Mode::Strict, graph).unwrap();
    assert_eq!(
        bus.changed_deadlines(0).collect::<Vec<_>>(),
        vec![(source, None), (target, None)]
    );
    let saved = bus.state();
    assert_eq!(saved, bus.state());
    let mut host = Services { now: 17 };
    bus.access(&mut host, 3, 0x2000, 0x1000, 4, Some(1))
        .unwrap();
    assert_eq!(*effects.lock().unwrap(), vec![17]);
    assert!(!*ready.lock().unwrap());
    assert_eq!(
        bus.changed_deadlines(17).collect::<Vec<_>>(),
        vec![(source, None), (target, None)]
    );
}

#[test]
fn pausing_inside_a_quantum_preserves_multicore_interleaving() {
    use factory::*;
    let run = |split| {
        let graph = Graph {
            decoders: vec![],
            processors: (0..2)
                .map(|_| Processor {
                    rprofile: Some(Default::default()),
                    mprofile: None,
                    model: Model::CortexR5F,
                    registers: vec![(Register::Pc, 0x10000)],
                    released: true,
                })
                .collect(),
            dma_mappings: vec![crate::board::memory::BusMapping::ram(0x10000, 0x4000)],
            ram: vec![Ram {
                base: 0x10000,
                size: 0x4000,
            }],
            mappings: (0..2)
                .map(|core| MemoryMap {
                    core,
                    address: 0x10000,
                    physical: 0x10000,
                    size: 0x4000,
                    permissions: 7,
                })
                .collect(),
            peripherals: vec![],
            instructions_per_tick: Ratio::new(1, 1).unwrap(),
        };
        let mut built = graph
            .build(
                Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
                Bus::new(bus::Mode::Strict),
                None,
            )
            .unwrap();
        built
            .cpu(0)
            .unwrap()
            .write_memory(
                0x10000,
                &[0xe2800001u32, 0xeafffffd]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        if split {
            built.run_until(7, 5).unwrap();
        }
        built.run_until(31, 5).unwrap();
        let registers = (0..2)
            .map(|c| {
                let cpu = built.cpu(c).unwrap();
                (
                    scalar(
                        cpu,
                        Register::Arm32(fujimod_emulator_cpu::arm::Register::R(0)),
                    )
                    .unwrap(),
                    scalar(cpu, Register::Pc).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        (built.state(), registers)
    };
    assert_eq!(run(false), run(true));
}

#[test]
fn mprofile_systick_wakes_wfi_and_executes_a_real_exception_handler() {
    use factory::*;
    let run = |quantum| {
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
                    vector_base: 0,
                }),
            }],
            dma_mappings: vec![crate::board::memory::BusMapping::ram(0, 0x4000)],
            ram: vec![Ram {
                base: 0,
                size: 0x4000,
            }],
            mappings: vec![MemoryMap {
                core: 0,
                address: 0,
                physical: 0,
                size: 0x4000,
                permissions: 7,
            }],
            peripherals: vec![],
            instructions_per_tick: Ratio::new(1, 1).unwrap(),
        };
        let mut board = graph
            .build(
                Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
                Bus::new(bus::Mode::Strict),
                None,
            )
            .unwrap();
        let cpu = board.cpu(0).unwrap();
        cpu.write_memory(
            0,
            &[0x3000u32, 0x1001]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        cpu.write_memory(15 * 4, &0x1101u32.to_le_bytes()).unwrap();
        // movw/movt r0,0xe000e010; RVR=19; CTRL=7; wfi;
        // movs r2,#99; b .  Handler disables SysTick and returns with BX LR.
        let main = [
            0x4e, 0xf2, 0x10, 0x00, 0xce, 0xf2, 0x00, 0x00, 0x13, 0x21, 0x41, 0x60, 0x07, 0x21,
            0x01, 0x60, 0x30, 0xbf, 0x63, 0x22, 0xfe, 0xe7,
        ];
        cpu.write_memory(0x1000, &main).unwrap();
        cpu.write_memory(0x1100, &[0x00, 0x21, 0x01, 0x60, 0x70, 0x47])
            .unwrap();
        board.run_until(50, quantum).unwrap();
        let saved = board.state();
        assert_eq!(saved["timeline"]["now"], 50);
        assert_eq!(
            scalar(
                board.cpu(0).unwrap(),
                Register::Arm32(fujimod_emulator_cpu::arm::Register::R(2))
            )
            .unwrap(),
            99
        );
        assert_eq!(saved["mprofile"][0]["nvic"]["control"], 0);
        assert_eq!(saved["mprofile"][0]["nvic"]["active"], json!([]));
        assert_eq!(saved["mprofile"][0]["nvic"]["countflag"], true);
        assert!(saved["instructions"][0].as_u64().unwrap() < 50);
        assert_eq!(saved, board.state());
        (saved["instructions"].clone(), saved["mprofile"].clone())
    };
    assert_eq!(run(8), run(64));
}

fn per_core_irq_board() -> (Board, Arc<Mutex<Vec<u64>>>) {
    per_core_irq_board_with_backend(false)
}
fn per_core_irq_board_with_backend(counted: bool) -> (Board, Arc<Mutex<Vec<u64>>>) {
    use fujimod_emulator_devices::exr2::irq::{Arbitration, Inputs, InterruptController, Options, Wiring};
    struct Setup;
    impl Host for Setup {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("unexpected schedule")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("unexpected IRQ")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            panic!("unexpected IRQ")
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("unexpected DMA")
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            panic!("unexpected DMA")
        }
        fn record(&mut self, _: Record) {
            panic!("unexpected record")
        }
    }
    struct Edge {
        fired: bool,
        times: Arc<Mutex<Vec<u64>>>,
    }
    impl Device for Edge {
        fn name(&self) -> &'static str {
            "test-edge"
        }
        fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
            cx.unmodeled();
            0
        }
        fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
            cx.unmodeled();
        }
        fn next_deadline(&self, _: u64) -> Option<u64> {
            (!self.fired).then_some(7)
        }
        fn advance(&mut self, cx: &mut Cx<'_>) {
            if !self.fired && cx.now() >= 7 {
                self.fired = true;
                self.times.lock().unwrap().push(cx.now());
                if let Err(error) = cx.raise_irq(1, 33) {
                    cx.fail(error);
                }
            }
        }
    }
    let mut bus = Bus::new(bus::Mode::Strict);
    let mut inputs = Vec::new();
    for core in 0..2 {
        let pins = Inputs::default();
        let base = 0x8000000 + core as u64 * 0x1000;
        let mut controller = InterruptController::new(
            Options {
                arbitration: Arbitration::ActiveBitmap,
            },
            Wiring {
                base,
                inputs: pins.clone(),
                enables: None,
                software: vec![],
            },
        )
        .unwrap();
        let mut host = Setup;
        let mut cx = Cx::new(&mut host, 0, 0, core);
        for (offset, value) in [(0x484, 0x100), (0x884, 1), (0x84, 2)] {
            controller.write(&mut cx, base + offset, 4, value);
            assert!(cx.refusal().is_none());
        }
        let id = bus.register(base, 0x1000, Box::new(controller)).unwrap();
        inputs.push((id, pins));
    }
    let times = Arc::new(Mutex::new(Vec::new()));
    bus.attach(Box::new(Edge {
        fired: false,
        times: times.clone(),
    }));
    let graph = factory::Graph {
        decoders: vec![],
        processors: (0..2)
            .map(|core| factory::Processor {
                model: Model::CortexR5F,
                rprofile: Some(Default::default()),
                mprofile: None,
                released: true,
                registers: vec![
                    (Register::Pc, 0x1000 + core * 0x1000),
                    (Register::Pstate, 0x53),
                    (
                        Register::Arm32(fujimod_emulator_cpu::arm::Register::R(1)),
                        0x8000000 + core * 0x1000,
                    ),
                ],
            })
            .collect(),
        dma_mappings: vec![crate::board::memory::BusMapping::ram(0, 0x4000)],
        ram: vec![factory::Ram {
            base: 0,
            size: 0x4000,
        }],
        mappings: (0..2)
            .map(|core| factory::MemoryMap {
                core,
                address: 0,
                physical: 0,
                size: 0x4000,
                permissions: 7,
            })
            .collect(),
        peripherals: (0..2)
            .map(|core| factory::Peripheral {
                core,
                address: 0x8000000,
                physical: 0x8000000,
                size: 0x2000,
            })
            .collect(),
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    };
    let mut board = if counted {
        let memory = fujimod_emulator_cpu_unicorn::memory::Memory::default();
        let backing = fujimod_emulator_cpu::memory::Backing::aligned_zeros(0x4000, 0x4000).unwrap();
        let library = Library::linked().unwrap();
        let cpus = graph
            .processors
            .iter()
            .map(|p| {
                let cpu = fujimod_emulator_cpu_unicorn::rprofile::core::Core::new(
                    library.open(p.model, 1024 * 1024).unwrap(),
                    fujimod_emulator_cpu_unicorn::rprofile::core::Options::default(),
                    fujimod_emulator_cpu_unicorn::rprofile::core::Wiring {
                        memory: memory.clone(),
                    },
                )
                .unwrap();
                unsafe {
                    cpu.map_backing(
                        0,
                        0x4000,
                        7,
                        std::ptr::NonNull::new(backing.as_ptr()).unwrap(),
                    )
                    .unwrap();
                }
                for &(r, value) in &p.registers {
                    set(&cpu, r, value).unwrap();
                }
                Box::new(cpu) as Box<dyn Cpu>
            })
            .collect();
        let mut board = Board::new(
            cpus,
            bus,
            graph.instructions_per_tick,
            None,
            vec![true; 2],
            memory::Memory::new(vec![(0, backing)], vec![memory::BusMapping::ram(0, 0x4000)])
                .unwrap(),
        )
        .unwrap();
        for p in graph.peripherals {
            board
                .map_mmio(p.core, p.address, p.size, p.physical)
                .unwrap();
        }
        board
    } else {
        graph
            .build(
                factory::Options::new(factory::Engine::Unicorn)
                    .with_translation_cache_size_bytes(1024 * 1024),
                bus,
                None,
            )
            .unwrap()
    };
    for (core, (device, pins)) in inputs.into_iter().enumerate() {
        board
            .configure_core_interrupts(core, device, Box::new(pins))
            .unwrap();
    }
    for (address, words) in [
        (0x18, vec![0xea000018u32]), // architectural vector -> acknowledge stub
        (0x80, vec![0xe5910200, 0xe12fff10]), // ldr r0,[r1,#0x200]; bx r0
        (
            0x100,
            vec![
                0xe3a02002, 0xe5812124, 0xe3a00000, 0xe5810200, 0xe2833001, 0xe25ef004,
            ],
        ),
        (0x1000, vec![0xe320f000, 0xeafffffd]),
        (0x2000, vec![0xe320f000, 0xeafffffd]),
    ] {
        board
            .write_memory(
                0,
                address,
                &words
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    (board, times)
}

#[test]
fn a_parked_rprofile_counter_observes_peer_and_idle_guest_time_without_host_mutation() {
    let (mut board, _) = per_core_irq_board_with_backend(true);
    board
        .write_memory(0, 0x1000, &0xe320f003_u32.to_le_bytes())
        .unwrap(); // WFI
    board
        .write_memory(0, 0x2000, &0xe320f003_u32.to_le_bytes())
        .unwrap();
    let counter = |crm, opc2| fujimod_emulator_cpu::arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn: 9,
        crm,
        opc2,
        secure: 0,
        wide: false,
    };
    board
        .cpu(0)
        .unwrap()
        .set_guest_clock(fujimod_emulator_cpu::Clock {
            now: 0,
            phase: 0,
            ratio: Ratio::new(1, 1).unwrap(),
        })
        .unwrap();
    board
        .cpu(0)
        .unwrap()
        .coprocessor(counter(12, 0), Some(1))
        .unwrap();
    board
        .cpu(0)
        .unwrap()
        .coprocessor(counter(12, 1), Some(0x80000000))
        .unwrap();
    board.run_until(150, 50).unwrap();
    assert_eq!(board.instruction_counts()[0], 1);
    assert!(board.instruction_counts().iter().sum::<u64>() < 150);
    let before = board.state();
    assert_eq!(
        board
            .cpu(0)
            .unwrap()
            .coprocessor(counter(13, 0), None)
            .unwrap(),
        150
    );
    assert_eq!(
        board
            .cpu(0)
            .unwrap()
            .coprocessor(counter(13, 0), None)
            .unwrap(),
        150
    );
    assert_eq!(board.state(), before);
}
#[test]
fn counted_rprofile_runs_both_controllers_through_the_shared_scheduler() {
    let run = |pause, quantum| {
        let (mut board, times) = per_core_irq_board_with_backend(true);
        if pause {
            board.run_until(7, quantum).unwrap();
        }
        board.run_until(150, quantum).unwrap();
        assert_eq!(*times.lock().unwrap(), vec![7]);
        assert_eq!(
            board
                .read_register(0, fujimod_emulator_cpu::arm::CORE[3])
                .unwrap(),
            0
        );
        assert_eq!(
            board
                .read_register(1, fujimod_emulator_cpu::arm::CORE[3])
                .unwrap(),
            1
        );
        assert_eq!(board.instruction_counts().iter().sum::<u64>(), 150);
        (
            board.state(),
            [
                board.read_register(0, Register::Pc).unwrap(),
                board.read_register(1, Register::Pc).unwrap(),
            ],
        )
    };
    for quantum in [1, 50] {
        assert_eq!(run(false, quantum), run(true, quantum));
    }
}
#[test]
fn per_core_interrupt_device_routes_the_due_edge_and_executes_guest_acknowledge_end_and_return() {
    for quantum in [1, 50] {
        let (mut board, times) = per_core_irq_board();
        board.run_until(150, quantum).unwrap();
        assert_eq!(*times.lock().unwrap(), vec![7]);
        assert_eq!(
            board
                .read_register(
                    0,
                    Register::Arm32(fujimod_emulator_cpu::arm::Register::R(3))
                )
                .unwrap(),
            0
        );
        assert_eq!(
            board
                .read_register(
                    1,
                    Register::Arm32(fujimod_emulator_cpu::arm::Register::R(3))
                )
                .unwrap(),
            1
        );
        assert_eq!(board.instruction_counts().iter().sum::<u64>(), 150);
        assert_eq!(board.now(), 150);
    }
    let run = |pause| {
        let (mut board, _) = per_core_irq_board();
        if pause {
            board.run_until(7, 50).unwrap();
        }
        board.run_until(150, 50).unwrap();
        (
            board.state(),
            [
                board.read_register(0, Register::Pc).unwrap(),
                board.read_register(1, Register::Pc).unwrap(),
            ],
        )
    };
    assert_eq!(run(false), run(true));
}

#[test]
fn external_per_core_edge_is_queued_without_host_observation_applying_it() {
    let (mut board, _) = per_core_irq_board();
    board.raise_irq(0, 33).unwrap();
    let state = board.state();
    assert_eq!(state, board.state());
    assert_eq!(board.now(), 0);
    assert!(board.raise_irq(0, 128).is_err());
    assert!(board.raise_irq(2, 33).is_err());
    board.run_until(150, 50).unwrap();
    for core in 0..2 {
        assert_eq!(
            board
                .read_register(
                    core,
                    Register::Arm32(fujimod_emulator_cpu::arm::Register::R(3))
                )
                .unwrap(),
            1
        );
    }
}

#[test]
fn rprofile_thumb_resumes_after_irq_return_and_a_host_pause() {
    let (mut board, _) = per_core_irq_board();
    board
        .write_memory(0, 0x2000, &[0x01, 0x34, 0xfd, 0xe7])
        .unwrap(); // adds r4,#1; b .-2
    board.write_register(1, Register::Pstate, 0x73).unwrap();
    board.run_until(75, 50).unwrap();
    assert_eq!(
        board.read_register(1, Register::Pstate).unwrap() & 0x20,
        0x20
    );
    board.run_until(150, 50).unwrap();
    assert_eq!(
        board.read_register(1, Register::Pstate).unwrap() & 0x20,
        0x20
    );
    assert_eq!(
        board
            .read_register(
                1,
                Register::Arm32(fujimod_emulator_cpu::arm::Register::R(3))
            )
            .unwrap(),
        1
    );
    assert!(
        board
            .read_register(
                1,
                Register::Arm32(fujimod_emulator_cpu::arm::Register::R(4))
            )
            .unwrap()
            > 1
    );
}

#[test]
fn external_device_inputs_preserve_time_and_keep_effects_due_after_refusal() {
    let (mut board, probe) =
        build_rprofile_with_inputs(bus::Mode::Strict, true, Some(Default::default()));
    // An isolated WFI caller leaves only the device's guest-time deadline.
    board
        .cpu(0)
        .unwrap()
        .write_memory(0x10000, &0xe320f003u32.to_le_bytes())
        .unwrap();
    let at = board
        .device_input::<Timer, _>(0, |timer, cx| {
            assert_eq!((cx.cpu, cx.pc), (usize::MAX, 0));
            timer.write(cx, 0, 4, 73);
            Ok(cx.now())
        })
        .unwrap();
    assert_eq!(at, 0);
    assert_eq!(board.now(), 0);
    assert_eq!(board.instruction_counts(), vec![0]);
    let error = board
        .device_input::<Timer, _>(0, |timer, cx| {
            timer.write(cx, 0, 4, 91);
            cx.raise_irq(0, 17).unwrap();
            cx.fail("explicit refused external input");
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error, "explicit refused external input");
    assert_eq!(board.state.borrow().timeline.next_event().unwrap().at, 0);
    board.run_until(5, 1).unwrap();
    let saved = probe.lock().unwrap();
    assert_eq!(saved.value, 92);
    assert_eq!(saved.calls, vec![(5, 0)]);
    assert_eq!(saved.due, None);
    assert_eq!(board.instruction_counts(), vec![1]);
    let irq = board
        .inspect_device::<fujimod_emulator_devices::exr2::irq::InterruptController, _>(
            1,
            |irq| Ok(irq.save()),
        )
        .unwrap();
    assert_ne!(
        irq["registers"]["pending"][0].as_u64().unwrap() & (1 << 17),
        0
    );
}

#[test]
fn continuation_observes_due_device_publications_without_changing_the_timeline() {
    let (mut board, _) = build_rprofile(bus::Mode::Strict, true);
    // Finish the initial MMIO write, which schedules a tick at 7.
    board.run_for_instructions(2, 100).unwrap();
    board
        .continuation_begin(vec![], [0].into(), vec![])
        .unwrap();
    // Reads also publish this probe's access ledger, so the next stop is the
    // completed read at tick 3, before its following branch instruction.
    board.run_until(20, 100).unwrap();
    assert!(board.continuation_end().unwrap());
    assert_eq!(board.instruction_counts(), vec![3]);
    // Park the CPU with architectural WFI. A due effect must wake the host
    // even without any guest stores or MMIO accesses.
    board
        .write_memory(0, 0x10010, &0xe320f003u32.to_le_bytes())
        .unwrap();
    board.write_register(0, Register::Pc, 0x10010).unwrap();
    board
        .continuation_begin(vec![], [0].into(), vec![])
        .unwrap();
    board.run_until(20, 100).unwrap();
    assert!(board.continuation_end().unwrap());
    assert_eq!(board.state()["timeline"]["now"], 7);
    assert_eq!(board.instruction_counts(), vec![4]);
}

#[test]
fn continuation_reports_a_publication_before_time_or_instructions_advance() {
    let (mut board, probe) = build_rprofile(bus::Mode::Strict, true);
    probe.lock().unwrap().due = Some(0);
    board.inputs_changed(0).unwrap();
    board
        .continuation_begin(vec![], [0].into(), vec![])
        .unwrap();
    board.run_for_instructions(20, 7).unwrap();
    assert!(board.continuation_end().unwrap());
    assert_eq!(board.instruction_counts(), vec![0]);
    assert_eq!(board.state()["timeline"]["now"], 0);
    assert_eq!(probe.lock().unwrap().value, 1);
    board.run_for_instructions(20, 7).unwrap();
    assert_eq!(board.instruction_counts(), vec![20]);
}

#[test]
fn serialized_interactive_cores_share_execution_before_the_host_deadline() {
    let (mut board, _) = per_core_irq_board();
    board
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    for core in 0..2 {
        assert!(!board.cpu(core).unwrap().features().asynchronous_runs);
        board.write_register(core, Register::Pstate, 0xd3).unwrap();
    }
    // Both cores spin without MMIO or architectural exits. The device edge
    // at tick 7 must not let the next core monopolize the rest of this run.
    board.run_until(200, 8).unwrap();
    let counts = board.instruction_counts();
    assert!(counts.iter().all(|&count| count >= 64), "{counts:?}");
    assert_eq!(board.state()["timeline"]["now"], 200);
}
