use fujimod_emulator::board::{
    Board,
    bus::{Bus, Mode},
    factory::{Engine, Graph, MemoryMap, Options, Peripheral, Processor, Ram},
};
use fujimod_emulator_cpu::{Model, Register, arm, timeline::Ratio};
use fujimod_emulator_devices::{Cx, Device};

struct Constant(u64);
impl Device for Constant {
    fn name(&self) -> &'static str {
        "synthetic-constant"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, bytes: u8) -> u64 {
        if bytes != 4 {
            cx.unmodeled();
        }
        self.0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
}

fn graph() -> Graph {
    Graph {
        processors: vec![Processor {
            model: Model::CortexR5F,
            registers: vec![
                (Register::Pc, 0x1000),
                (Register::Pstate, 0xd3),
                (arm::CORE[1], 0x0800_0020),
                (arm::CORE[2], 0x0900_0028),
            ],
            released: true,
            mprofile: None,
            rprofile: Some(Default::default()),
        }],
        ram: vec![Ram {
            base: 0x1000,
            size: 0x1000,
        }],
        dma_mappings: vec![],
        decoders: vec![],
        mappings: vec![MemoryMap {
            core: 0,
            address: 0x1000,
            physical: 0x1000,
            size: 0x1000,
            permissions: 7,
        }],
        peripherals: vec![
            Peripheral {
                core: 0,
                address: 0x0800_0020,
                physical: 0x5000_0020,
                size: 4,
            },
            Peripheral {
                core: 0,
                address: 0x0800_0028,
                physical: 0x5000_0028,
                size: 4,
            },
            Peripheral {
                core: 0,
                address: 0x0900_0028,
                physical: 0x5000_0028,
                size: 4,
            },
        ],
        instructions_per_tick: Ratio::new(1, 1).unwrap(),
    }
}

fn board() -> Board {
    let mut bus = Bus::new(Mode::Strict);
    bus.register(0x5000_0020, 4, Box::new(Constant(0x11)))
        .unwrap();
    bus.register(0x5000_0028, 4, Box::new(Constant(0x22)))
        .unwrap();
    let board = graph()
        .into_page_granular_peripherals()
        .unwrap()
        .build(
            Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20),
            bus,
            None,
        )
        .unwrap();
    let code: Vec<u8> = [0xe5910000u32, 0xe5923000, 0xe5914008, 0xeafffffe]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    board.write_memory(0, 0x1000, &code).unwrap();
    board
}

#[test]
fn subpage_devices_and_virtual_aliases_execute_through_shared_cpu_pages() {
    let mut board = board();
    board.run_for_instructions(3, 3).unwrap();
    assert_eq!(board.read_register(0, arm::CORE[0]).unwrap(), 0x11);
    assert_eq!(board.read_register(0, arm::CORE[3]).unwrap(), 0x22);
    assert_eq!(board.read_register(0, arm::CORE[4]).unwrap(), 0x22);
}

#[test]
fn page_coverage_does_not_admit_access_outside_exact_bus_windows() {
    let mut board = board();
    board.write_register(0, arm::CORE[1], 0x0800_0024).unwrap();
    assert!(board.run_for_instructions(1, 1).is_err());
}

#[test]
fn incompatible_aliases_in_one_cpu_page_are_rejected() {
    let mut graph = graph();
    graph.peripherals[1].physical = 0x6000_0028;
    assert!(graph.into_page_granular_peripherals().is_err());
}

#[test]
fn page_boundary_expansion_cannot_overlap_ram() {
    let mut graph = graph();
    graph.peripherals = vec![Peripheral {
        core: 0,
        address: 0x1800,
        physical: 0x5000_0800,
        size: 4,
    }];
    assert!(graph.into_page_granular_peripherals().is_err());
}
