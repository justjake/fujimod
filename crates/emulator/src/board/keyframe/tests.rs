use super::*;
use crate::board::{factory::*, memory::BusMapping};
use fujimod_emulator_cpu::{Model, arm};
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct PendingDma {
    due: Option<u64>,
    value: u32,
    completions: Vec<(u64, u64)>,
}
impl Device for PendingDma {
    fn name(&self) -> &'static str {
        "test-pending-dma"
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(serde_json::to_value(self).unwrap())
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), self)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let _: Self = saved.decode(self.name())?;
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        *self = saved.decode(self.name())?;
        Ok(())
    }
    fn read(&mut self, _: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        self.value.into()
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, value: u64) {
        self.value = value as u32;
        self.due = Some(cx.now() + 25);
        cx.schedule(cx.now() + 25, 17);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.due
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.due.is_some_and(|at| at <= cx.now()) {
            self.due = None;
            cx.ram_write(0x3000, &self.value.to_le_bytes()).unwrap();
            self.completions.push((cx.now(), 0));
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        self.completions.push((cx.now(), token));
    }
}
pub(super) fn board() -> Board {
    board_mode(bus::Mode::Strict)
}
pub(super) fn board_mode(mode: bus::Mode) -> Board {
    let processors = (0..2)
        .map(|core| Processor {
            model: Model::CortexR5F,
            released: true,
            mprofile: None,
            rprofile: Some(fujimod_emulator_cpu_unicorn::rprofile::core::Options {
                cp14_identity: 0x55aa,
                tcm: fujimod_emulator_cpu_unicorn::rprofile::tcm::Options {
                    btcm: fujimod_emulator_cpu_unicorn::rprofile::tcm::BankOptions {
                        bytes: 4096,
                        reset: 0x70000001,
                    },
                    atcm: fujimod_emulator_cpu_unicorn::rprofile::tcm::BankOptions {
                        bytes: 8192,
                        reset: 0xffff8001,
                    },
                },
            }),
            registers: vec![
                (Register::Pc, 0x1000 + core * 0x100),
                (Register::Pstate, 0xd3),
                (arm::CORE[1], 0x8000000),
                (arm::CORE[3], 0x8000),
                (arm::CORE[6], 0x40003800),
            ],
        })
        .collect();
    let mappings = (0..2)
        .flat_map(|core| {
            [
                MemoryMap {
                    core,
                    address: 0,
                    physical: 0,
                    size: 0x4000,
                    permissions: 7,
                },
                MemoryMap {
                    core,
                    address: 0x40000000,
                    physical: 0,
                    size: 0x4000,
                    permissions: 7,
                },
                MemoryMap {
                    core,
                    address: 0x8000,
                    physical: 0x8000,
                    size: 0x1000,
                    permissions: 7,
                },
                MemoryMap {
                    core,
                    address: 0x10000,
                    physical: 0x10000,
                    size: 0x1000,
                    permissions: 5,
                },
            ]
        })
        .collect();
    let graph = Graph {
        processors,
        ram: vec![
            Ram {
                base: 0,
                size: 0x4000,
            },
            Ram {
                base: 0x8000,
                size: 0x1000,
            },
            Ram {
                base: 0x10000,
                size: 0x1000,
            },
        ],
        dma_mappings: vec![
            BusMapping::ram(0, 0x4000),
            BusMapping::ram(0x8000, 0x1000),
            BusMapping::ram(0x10000, 0x1000),
        ],
        mappings,
        peripherals: (0..2)
            .map(|core| Peripheral {
                core,
                address: 0x8000000,
                physical: 0x8000000,
                size: 0x1000,
            })
            .collect(),
        decoders: vec![],
        instructions_per_tick: Ratio::new(3, 2).unwrap(),
    };
    let mut bus = Bus::new(mode);
    bus.register(0x8000000, 4, Box::<PendingDma>::default())
        .unwrap();
    let mut board = graph
        .build(
            Options::new(Engine::Unicorn).with_translation_cache_size_bytes(1 << 20),
            bus,
            None,
        )
        .unwrap();
    let code = |words: &[u32]| {
        words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>()
    };
    board
        .write_physical(
            0x1000,
            &code(&[0xe3a00007, 0xe5810000, 0xe2822001, 0xe5832000, 0xeafffffc]),
        )
        .unwrap();
    board
        .write_physical(0x1100, &code(&[0xe2855001, 0xe5865000, 0xeafffffc]))
        .unwrap();
    board
        .write_physical(0x10000, b"immutable boot bytes")
        .unwrap();
    board
        .write_memory(0, 0xffff8000, b"private first core")
        .unwrap();
    board
        .write_memory(1, 0x70000000, b"private second core")
        .unwrap();
    board.watch_pcs(&[0x1008, 0x1100]).unwrap();
    board
}
#[test]
fn both_execution_policies_restore_the_whole_board_and_pending_events() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        let mut board = board();
        board.set_execution_policy(policy).unwrap();
        board.run_until(13, 17).unwrap();
        let before = board.fingerprint().unwrap();
        let saved = board.checkpoint(None).unwrap();
        assert_eq!(before, board.fingerprint().unwrap());
        board.run_until(120, 17).unwrap();
        board.set_execution_policy(ExecutionPolicy::Exact).unwrap();
        board.write_physical(0x3000, &99u32.to_le_bytes()).unwrap();
        board.restore_keyframe(&saved).unwrap();
        assert_eq!(board.execution_policy(), policy);
        assert_eq!(before, board.fingerprint().unwrap());
        board.run_until(120, 17).unwrap();
        assert!(
            board
                .inspect_device::<PendingDma, _>(0, |d| Ok(
                    d.due.is_none() && d.completions.len() == 2
                ))
                .unwrap()
        );
    }
}

#[test]
fn replay_restores_pending_dma_deadlines_aliases_private_ram_and_partial_scheduler_slices() {
    let mut board = board();
    board.run_until(13, 17).unwrap();
    let before = board.fingerprint().unwrap();
    let saved = board.checkpoint(None).unwrap();
    assert_eq!(before, board.fingerprint().unwrap());
    assert!(
        board
            .inspect_device::<PendingDma, _>(0, |d| Ok(d.due.is_some()))
            .unwrap()
    );
    board.run_until(120, 17).unwrap();
    let straight = board.fingerprint().unwrap();
    assert_eq!(
        board
            .inspect_device::<PendingDma, _>(0, |d| Ok(d.completions.clone()))
            .unwrap()
            .len(),
        2
    );
    let later = board.checkpoint(Some(&saved)).unwrap();
    board.write_physical(0x10000, b"changed host ROM").unwrap();
    board
        .write_memory(0, 0xffff8000, b"changed private")
        .unwrap();
    board
        .write_memory(1, 0x70000000, b"changed private")
        .unwrap();
    board
        .write_memory(0, 0x40003000, &123u32.to_le_bytes())
        .unwrap();
    for _ in 0..3 {
        board.restore_keyframe(&saved).unwrap();
        assert_eq!(before, board.fingerprint().unwrap());
        board.run_until(120, 17).unwrap();
        assert_eq!(straight, board.fingerprint().unwrap());
        board.write_physical(0x3000, &99u32.to_le_bytes()).unwrap();
    }
    board.restore_keyframe(&later).unwrap();
    assert_eq!(straight, board.fingerprint().unwrap());
    let other = self::board();
    assert!(
        other
            .checkpoint(Some(&saved))
            .err()
            .unwrap()
            .contains("different board")
    );
}
#[test]
fn independent_cpu_allocations_refuse_instead_of_omitting_live_ram() {
    let mut board = board();
    let saved = board.checkpoint(None).unwrap();
    board
        .cpu(0)
        .unwrap()
        .map_zeroed(0x20000, 0x1000, 7)
        .unwrap();
    assert!(
        board
            .checkpoint(None)
            .err()
            .unwrap()
            .contains("outside whole-board")
    );
    assert!(
        board
            .restore_keyframe(&saved)
            .unwrap_err()
            .contains("outside whole-board")
    );
}
