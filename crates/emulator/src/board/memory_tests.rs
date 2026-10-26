use super::*;
use fujimod_emulator_cpu::{Model, arm, memory::Backing};
use fujimod_emulator_cpu_unicorn::rprofile;
use fujimod_emulator_devices::{Cx, Device};
use std::sync::{Arc, Mutex};

#[test]
fn physical_ranges_are_checked_before_any_byte_changes() {
    let memory = memory::Memory::new(
        vec![
            (0x1000, Backing::zeros(4)),
            (0x1004, Backing::zeros(4)),
            (0x1009, Backing::zeros(4)),
        ],
        vec![
            memory::BusMapping::ram(0x1000, 8),
            memory::BusMapping::ram(0x1009, 4),
        ],
    )
    .unwrap();
    let domain = fujimod_emulator_cpu::exclusive::Shared::default();
    memory
        .write_dma(&[], &domain, 0x1002, &[1, 2, 3, 4])
        .unwrap();
    let mut bytes = [0; 8];
    memory.read_host(0x1000, &mut bytes).unwrap();
    assert_eq!(bytes, [0, 0, 1, 2, 3, 4, 0, 0]);
    assert!(memory.write_dma(&[], &domain, 0x1007, &[9, 9, 9]).is_err());
    memory.read_host(0x1000, &mut bytes).unwrap();
    assert_eq!(bytes, [0, 0, 1, 2, 3, 4, 0, 0]);
    bytes.fill(0x55);
    assert!(memory.read_host(0x1007, &mut bytes).is_err());
    assert_eq!(bytes, [0x55; 8]);
    assert!(memory.read_host(u64::MAX, &mut [0; 2]).is_err());
    assert!(memory::Memory::new(vec![(0, Backing::zeros(0))], vec![]).is_err());
    assert!(
        memory::Memory::new(vec![(0, Backing::zeros(4)), (3, Backing::zeros(4))], vec![]).is_err()
    );
    assert!(memory::Memory::new(vec![(u64::MAX, Backing::zeros(4))], vec![]).is_err());
}

#[test]
fn bus_permissions_preflight_a_whole_transfer_and_host_loading_is_explicit() {
    use memory::BusMapping;
    let memory = memory::Memory::new(
        vec![(0x1000, Backing::zeros(8))],
        vec![
            BusMapping {
                address: 0x1000,
                physical: 0x1000,
                size: 4,
                permissions: 3,
            },
            BusMapping {
                address: 0x1004,
                physical: 0x1004,
                size: 4,
                permissions: 1,
            },
            BusMapping {
                address: 0x40001000,
                physical: 0x1000,
                size: 8,
                permissions: 2,
            },
        ],
    )
    .unwrap();
    let domain = fujimod_emulator_cpu::exclusive::Shared::default();
    memory
        .write_host(&[], &domain, 0x1000, &[1, 2, 3, 4, 5, 6, 7, 8])
        .unwrap();
    let owner = domain.lock().unwrap().register(4).unwrap();
    domain.lock().unwrap().load(owner, 0x1000, 4);
    assert!(memory.write_dma(&[], &domain, 0x1002, &[9; 4]).is_err());
    assert!(domain.lock().unwrap().reservation(owner).is_some());
    let mut bytes = [0; 8];
    memory.read_host(0x1000, &mut bytes).unwrap();
    assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert!(memory.read_dma(0x40001000, &mut bytes).is_err());
    memory.write_dma(&[], &domain, 0x40001000, &[0; 4]).unwrap();
    assert!(domain.lock().unwrap().reservation(owner).is_none());
    memory.read_host(0x1000, &mut bytes).unwrap();
    assert_eq!(bytes, [0, 0, 0, 0, 5, 6, 7, 8]);
}

#[test]
fn high_canonical_backing_and_invalid_bus_maps_do_not_wrap_or_fall_back() {
    use memory::BusMapping;
    let base = u64::MAX - 0x1000;
    let map = BusMapping {
        address: 0x2000,
        physical: base,
        size: 0x1000,
        permissions: 3,
    };
    let memory = memory::Memory::new(vec![(base, Backing::zeros(0x1000))], vec![map]).unwrap();
    let domain = fujimod_emulator_cpu::exclusive::Shared::default();
    memory.write_dma(&[], &domain, 0x2001, &[42]).unwrap();
    let mut byte = [0];
    memory.read_dma(0x2001, &mut byte).unwrap();
    assert_eq!(byte, [42]);
    assert!(memory.read_host(base, &mut byte).is_err()); // No implicit identity view.
    for mapping in [
        BusMapping {
            address: u64::MAX,
            size: 2,
            ..map
        },
        BusMapping {
            physical: u64::MAX,
            size: 2,
            ..map
        },
        BusMapping {
            physical: base - 1,
            ..map
        },
        BusMapping {
            permissions: 4,
            ..map
        },
    ] {
        assert!(memory::Memory::new(vec![(base, Backing::zeros(0x1000))], vec![mapping]).is_err());
    }
    assert!(memory::Memory::new(vec![(base, Backing::zeros(0x1000))], vec![map, map]).is_err());
}

type Readings = Arc<Mutex<Vec<(u64, u32)>>>;
struct Dma {
    reads: Readings,
}
impl Device for Dma {
    fn save(&self) -> Value {
        json!(*self.reads.lock().unwrap())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "test-physical-dma"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, size: u8, value: u64) {
        if size != 4 {
            cx.unmodeled();
            return;
        }
        let result = (|| {
            let mut bytes = [0; 4];
            cx.ram_read(0x10000, &mut bytes)?;
            self.reads
                .lock()
                .unwrap()
                .push((cx.now(), u32::from_le_bytes(bytes)));
            cx.ram_write(0x40010000, &(value as u32).to_le_bytes())?;
            // A physical region need not have a CPU mapping.
            cx.ram_write(0x40000, &0x12345678u32.to_le_bytes())
        })();
        if let Err(error) = result {
            cx.fail(error);
        }
    }
}
fn board(released: Vec<bool>) -> (Board, Readings) {
    use factory::*;
    let graph = Graph {
        decoders: vec![],
        processors: released
            .into_iter()
            .map(|released| Processor {
                model: Model::CortexR5F,
                released,
                mprofile: None,
                rprofile: Some(rprofile::core::Options {
                    cp14_identity: 0,
                    tcm: rprofile::tcm::Options {
                        atcm: rprofile::tcm::BankOptions {
                            bytes: 0x1000,
                            reset: 0x10001,
                        },
                        btcm: Default::default(),
                    },
                }),
                registers: vec![(Register::Pc, 0x30000), (Register::Pstate, 0xd3)],
            })
            .collect(),
        dma_mappings: vec![
            memory::BusMapping {
                address: 0x40010000,
                physical: 0x10000,
                size: 0x1000,
                permissions: 3,
            },
            crate::board::memory::BusMapping::ram(0x10000, 0x1000),
            crate::board::memory::BusMapping::ram(0x30000, 0x1000),
            crate::board::memory::BusMapping::ram(0x40000, 0x1000),
        ],
        ram: vec![
            Ram {
                base: 0x10000,
                size: 0x1000,
            },
            Ram {
                base: 0x30000,
                size: 0x1000,
            },
            Ram {
                base: 0x40000,
                size: 0x1000,
            },
        ],
        mappings: (0..2)
            .flat_map(|core| {
                [
                    MemoryMap {
                        core,
                        address: 0x10000,
                        physical: 0x10000,
                        size: 0x1000,
                        permissions: 7,
                    },
                    MemoryMap {
                        core,
                        address: 0x20000,
                        physical: 0x10000,
                        size: 0x1000,
                        permissions: 7,
                    },
                    MemoryMap {
                        core,
                        address: 0x30000,
                        physical: 0x30000,
                        size: 0x1000,
                        permissions: 7,
                    },
                ]
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
    let reads = Arc::new(Mutex::new(Vec::with_capacity(4)));
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(
        0x8000000,
        4,
        Box::new(Dma {
            reads: reads.clone(),
        }),
    )
    .unwrap();
    let board = graph
        .build(
            Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1024 * 1024),
            bus,
            None,
        )
        .unwrap();
    (board, reads)
}
fn words(board: &Board, core: usize, address: u64, code: &[u32]) {
    board
        .write_memory(
            core,
            address,
            &code
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
}
fn word(board: &Board, core: usize, address: u64) -> u32 {
    let mut bytes = [0; 4];
    board.read_memory(core, address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

#[test]
fn dma_bypasses_private_tcm_and_invalidates_only_the_physical_reservation() {
    for private in [true, false] {
        let (mut board, reads) = board(vec![true, false]);
        board
            .write_physical(0x10000, &0xaaaau32.to_le_bytes())
            .unwrap();
        words(&board, 0, 0x10000, &[0xcccc]);
        words(&board, 1, 0x10000, &[0xdddd]);
        // str r0,[r1]; ldrex r4,[r2]; str r0,[r1]; strex r5,r3,[r2]; b .
        words(
            &board,
            0,
            0x30000,
            &[0xe5810000, 0xe1924f9f, 0xe5810000, 0xe1825f93, 0xeafffffe],
        );
        for (r, value) in [
            (0, 0xbbbb),
            (1, 0x8000000),
            (2, if private { 0x10000 } else { 0x20000 }),
            (3, 0xeeee),
        ] {
            board.write_register(0, arm::CORE[r], value).unwrap();
        }
        board.run_until(5, 50).unwrap();
        assert_eq!(*reads.lock().unwrap(), [(1, 0xaaaa), (3, 0xbbbb)]);
        assert_eq!(
            board.read_register(0, arm::CORE[5]).unwrap(),
            u64::from(!private)
        );
        assert_eq!(
            word(&board, 0, 0x10000),
            if private { 0xeeee } else { 0xcccc }
        );
        assert_eq!(word(&board, 1, 0x10000), 0xdddd);
        for core in 0..2 {
            assert_eq!(word(&board, core, 0x20000), 0xbbbb);
        }
        let mut bytes = [0; 4];
        board.read_physical(0x40000, &mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes), 0x12345678);
        assert!(board.read_memory(0, 0x40000, &mut bytes).is_err());
    }
}

#[test]
fn guest_dma_flushes_peer_code_hidden_under_another_cores_tcm() {
    let (mut board, _) = board(vec![true, true]);
    board
        .write_physical(
            0x10000,
            &[0xe3a04001u32, 0xeafffffd]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    words(&board, 0, 0x10000, &[0xe3a04009, 0xeafffffd]);
    board.write_register(0, Register::Pc, 0x10000).unwrap();
    board.write_register(1, Register::Pc, 0x20000).unwrap();
    board.run_until(4, 1).unwrap();
    assert_eq!(board.read_register(0, arm::CORE[4]).unwrap(), 9);
    assert_eq!(board.read_register(1, arm::CORE[4]).unwrap(), 1);
    words(&board, 0, 0x30000, &[0xe5810000, 0xeafffffe]);
    board.write_register(0, Register::Pc, 0x30000).unwrap();
    board.write_register(0, arm::CORE[0], 0xe3a04002).unwrap();
    board.write_register(0, arm::CORE[1], 0x8000000).unwrap();
    board.write_register(1, Register::Pc, 0x20000).unwrap();
    board.run_until(6, 1).unwrap();
    assert_eq!(board.read_register(1, arm::CORE[4]).unwrap(), 2);
    board.write_register(0, Register::Pc, 0x10000).unwrap();
    board.run_until(7, 1).unwrap();
    assert_eq!(board.read_register(0, arm::CORE[4]).unwrap(), 9);
}

#[test]
fn continuation_store_dma_and_publication_stops_preserve_execution() {
    use factory::MemoryMap;
    let run = |watched: bool| {
        let (mut board, _) = board(vec![true, true]);
        // CPU0 writes the shared alias; CPU1 performs a real peripheral DMA.
        words(&board, 0, 0x30000, &[0xe3a0002a, 0xe5810000, 0xeafffffc]);
        words(&board, 1, 0x30080, &[0xe3a0002b, 0xe5810000, 0xeafffffc]);
        board.write_register(0, arm::CORE[1], 0x20000).unwrap();
        board.write_register(1, Register::Pc, 0x30080).unwrap();
        board.write_register(1, arm::CORE[1], 0x8000000).unwrap();
        let maps: Vec<_> = (0..2)
            .map(|core| MemoryMap {
                core,
                address: 0x20000,
                physical: 0x10000,
                size: 0x1000,
                permissions: 7,
            })
            .collect();
        let mut stops = 0;
        while board.instruction_counts().iter().sum::<u64>() < 90 {
            if watched {
                board
                    .continuation_begin(vec![(0x10000, 0x10004)], [0].into(), maps.clone())
                    .unwrap();
            }
            board.run_window(90, 7, None, None).unwrap();
            if watched {
                stops += usize::from(board.continuation_end().unwrap());
            }
        }
        if watched {
            assert!(stops > 8, "stores must yield repeatedly");
        }
        (
            board.instruction_counts(),
            board.state()["timeline"]["now"].clone(),
            board.fingerprint().unwrap(),
        )
    };
    assert_eq!(run(false), run(true));
}

#[test]
fn continuation_stops_after_a_completed_store_and_tracks_physical_alias_reads() {
    use factory::MemoryMap;
    let (mut board, _) = board(vec![true, false]);
    words(
        &board,
        0,
        0x30000,
        &[0xe3a0002a, 0xe5810000, 0xe3a00063, 0xeafffffe],
    );
    board.write_register(0, arm::CORE[1], 0x20000).unwrap();
    board.read_set_begin().unwrap();
    board.read_physical(0x40010000, &mut [0; 4]).unwrap();
    let reads = board.read_set_end().unwrap();
    assert_eq!(reads.stores, vec![(0x10000, 0x10004)]);
    let maps = vec![MemoryMap {
        core: 0,
        address: 0x20000,
        physical: 0x10000,
        size: 0x1000,
        permissions: 7,
    }];
    board
        .continuation_begin(reads.stores, reads.devices, maps)
        .unwrap();
    board.run_for_instructions(20, 7).unwrap();
    assert!(board.continuation_end().unwrap());
    assert_eq!(board.instruction_counts(), vec![2, 0]);
    assert_eq!(word(&board, 0, 0x20000), 42);
    assert_eq!(board.read_register(0, Register::Pc).unwrap(), 0x30008);
}
