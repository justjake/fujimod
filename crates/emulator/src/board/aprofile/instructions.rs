//! Architectural boundaries for instruction-observed AArch64 backends.
use crate::board::{State, scalar, set};
use fujimod_emulator_cpu::{Access, Cpu, Hint, Register, arm};

#[cfg(test)]
fn system(encoding: u16) -> Register {
    Register::Arm64System(arm::SystemRegister::from_encoding(encoding))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{
        Board, InterruptMap, InterruptRegisters, aprofile,
        bus::{Bus, Mode},
        factory,
    };
    use fujimod_emulator_cpu::Model;
    use fujimod_emulator_ip_arm::gicv3::Gic;

    fn board() -> Board {
        let graph = factory::Graph {
            processors: vec![factory::Processor {
                model: Model::ArmV8A,
                registers: vec![(Register::Pc, 0x1000), (Register::Pstate, 0x3cd)],
                released: true,
                mprofile: None,
                rprofile: None,
            }],
            ram: vec![factory::Ram {
                base: 0,
                size: 0x4000,
            }],
            dma_mappings: vec![crate::board::memory::BusMapping::ram(0, 0x4000)],
            decoders: vec![],
            mappings: vec![factory::MemoryMap {
                core: 0,
                address: 0,
                physical: 0,
                size: 0x4000,
                permissions: 7,
            }],
            peripherals: vec![],
            instructions_per_tick: fujimod_emulator_cpu::timeline::Ratio::new(1, 1).unwrap(),
        };
        let interrupts = InterruptMap {
            gic: Box::new(Gic::new(vec![0], 128).unwrap()),
            registers: InterruptRegisters::GicV3 {
                distributor: 0xfa000000,
                redistributor: 0xfa080000,
                stride: 0x20000,
            },
            cores: 1,
        };
        let mut board = graph
            .build(
                factory::Options::new(factory::Engine::Unicorn),
                Bus::new(Mode::Strict),
                Some(interrupts),
            )
            .unwrap();
        board
            .configure_aprofile(
                0,
                aprofile::Config {
                    clock: aprofile::generic_timer::Clock {
                        counter_hz: 100,
                        ticks_per_second: 100,
                    },
                    physical_irq: 30,
                    virtual_irq: 27,
                    pmu_hz: 100,
                    implementation: None,
                },
            )
            .unwrap();
        board
    }

    #[test]
    fn unicorn_counters_consume_guest_ticks_and_wfi_advances_pc_once() {
        let mut board = board();
        let msr = |r: u32, rt: u32| 0xd5000000 | r << 5 | rt;
        let mrs = |r: u32, rt: u32| 0xd5200000 | r << 5 | rt;
        let code = [
            msr(0xdce0, 0),
            msr(0xdce1, 1),
            mrs(0xdce8, 2),
            0xd503201f,
            0xd503201f,
            0xd503201f,
            0xd503201f,
            mrs(0xdce8, 3),
            mrs(0xdf01, 4),
            0xd503207f,
        ];
        board
            .write_memory(
                0,
                0x1000,
                &code
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        board.write_register(0, Register::X(0), 1).unwrap();
        board.write_register(0, Register::X(1), 1 << 31).unwrap();
        board.run_for_instructions(10, 1000).unwrap();
        assert_eq!(
            board.read_register(0, Register::X(3)).unwrap()
                - board.read_register(0, Register::X(2)).unwrap(),
            5
        );
        assert_eq!(board.read_register(0, Register::X(4)).unwrap(), 8);
        assert_eq!(board.read_register(0, Register::Pc).unwrap(), 0x1028);
        assert_eq!(board.instruction_counts(), [10]);
    }

    #[test]
    fn mixed_cluster_sram_branch_rewrite_reaches_the_new_entry() {
        for companion in [false, true] {
            let base = 0xff800000u64;
            let mut graph = factory::Graph {
                processors: (0..4)
                    .map(|core| factory::Processor {
                        model: Model::ArmV8A,
                        released: true,
                        registers: vec![
                            (
                                Register::Pc,
                                base + if core == 0 { 0x31000 } else { 0x30000 },
                            ),
                            (Register::Pstate, 0x3cd),
                        ],
                        mprofile: None,
                        rprofile: None,
                    })
                    .collect(),
                ram: vec![factory::Ram {
                    base,
                    size: 0x40000,
                }],
                dma_mappings: vec![crate::board::memory::BusMapping::ram(base, 0x40000)],
                mappings: (0..4)
                    .map(|core| factory::MemoryMap {
                        core,
                        address: base,
                        physical: base,
                        size: 0x40000,
                        permissions: 7,
                    })
                    .collect(),
                peripherals: vec![],
                decoders: vec![],
                instructions_per_tick: fujimod_emulator_cpu::timeline::Ratio::new(1, 1).unwrap(),
            };
            if companion {
                graph.processors.push(factory::Processor {
                    model: Model::CortexM4,
                    released: false,
                    registers: vec![],
                    mprofile: Some(crate::board::mprofile::Config {
                        clock_hz: 600_000_000,
                        board_ticks_per_second: 1_000_000_000,
                        vector_base: 0,
                    }),
                    rprofile: None,
                });
                graph.ram.push(factory::Ram {
                    base: 0xfa200000,
                    size: 0x100000,
                });
                graph
                    .dma_mappings
                    .push(crate::board::memory::BusMapping::ram(0xfa200000, 0x100000));
                graph.mappings.push(factory::MemoryMap {
                    core: 4,
                    address: 0,
                    physical: 0xfa200000,
                    size: 0x100000,
                    permissions: 7,
                });
            }
            let mut board = graph
                .build(
                    factory::Options::new(factory::Engine::Unicorn),
                    Bus::new(Mode::Strict),
                    Some(InterruptMap {
                        gic: Box::new(Gic::new(vec![0, 1, 2, 3], 128).unwrap()),
                        registers: InterruptRegisters::GicV3 {
                            distributor: 0xfa000000,
                            redistributor: 0xfa080000,
                            stride: 0x20000,
                        },
                        cores: 4,
                    }),
                )
                .unwrap();
            for core in 0..4 {
                board
                    .configure_aprofile(
                        core,
                        aprofile::Config {
                            clock: aprofile::generic_timer::Clock {
                                counter_hz: 1_000_000_000,
                                ticks_per_second: 1_000_000_000,
                            },
                            physical_irq: 30,
                            virtual_irq: 27,
                            pmu_hz: 1_000_000_000,
                            implementation: None,
                        },
                    )
                    .unwrap();
            }
            board
                .write_memory(0, base + 0x30000, &0xd503207fu32.to_le_bytes())
                .unwrap();
            board
                .write_memory(
                    0,
                    base + 0x31000,
                    &[0xd503201fu32, 0x17ffffff]
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect::<Vec<_>>(),
                )
                .unwrap();
            board.run_for_instructions(5000, 1000).unwrap();
            board.write_register(0, Register::Pc, base).unwrap();
            board.write_register(0, Register::X(0), base).unwrap();
            board.write_register(0, Register::X(1), 0x14000014).unwrap();
            board
                .write_memory(0, base, &0x14006000u32.to_le_bytes())
                .unwrap();
            board
                .write_memory(0, base + 0x50, &0xd2800042u32.to_le_bytes())
                .unwrap();
            board
                .write_memory(0, base + 0x54, &0xd503207fu32.to_le_bytes())
                .unwrap();
            let code = [
                0x91000463u32,
                0xb9000001,
                0xd5033f9f,
                0xd508751f,
                0xd5033f9f,
                0xd5033fdf,
                0xd61f0000,
            ];
            board
                .write_memory(
                    0,
                    base + 0x18000,
                    &code
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect::<Vec<_>>(),
                )
                .unwrap();
            board.run_for_instructions(64, 1000).unwrap();
            assert_eq!(
                board.read_register(0, Register::X(2)).unwrap(),
                2,
                "companion={companion}, pc={:#x}, x3={}",
                board.read_register(0, Register::Pc).unwrap(),
                board.read_register(0, Register::X(3)).unwrap()
            );
            assert_eq!(board.read_register(0, Register::X(3)).unwrap(), 1);
        }
    }
    #[test]
    fn el1_irq_saves_interrupted_state_without_counting_the_interrupted_instruction() {
        let board = board();
        let cpu = &*board.cpus[0];
        set(cpu, Register::Pstate, 5).unwrap();
        set(cpu, system(0xf088), 0x404).unwrap();
        set(cpu, system(0xe088), 0).unwrap();
        set(cpu, system(0xc600), 0x2000).unwrap();
        let mut state = board.state.borrow_mut();
        state.pins[0] = (true, false);
        assert!(fujimod_emulator_cpu::arm64::enter_interrupt(cpu, true, false).unwrap());
        assert_eq!(scalar(cpu, Register::Pc).unwrap(), 0x2280);
        assert_eq!(scalar(cpu, system(0xc201)).unwrap(), 0x1000);
        assert_eq!(scalar(cpu, system(0xc200)).unwrap(), 5);
        assert_eq!(scalar(cpu, Register::Pstate).unwrap(), 0x3c5);
        assert_eq!(state.instructions, [0]);
    }
}

impl State {
    pub(in crate::board) fn aarch64_before(
        &mut self,
        cpu: &dyn Cpu,
        core: usize,
        pc: u64,
    ) -> Result<bool, String> {
        if self.aprofile[core].is_none() || cpu.features().architectural_exits {
            return Ok(false);
        }
        if fujimod_emulator_cpu::arm64::enter_interrupt(cpu, self.pins[core].0, self.pins[core].1)?
        {
            cpu.stop()?;
            return Ok(true);
        }
        let mut bytes = [0; 4];
        cpu.read_virtual(pc, Access::Fetch, &mut bytes)?;
        let word = u32::from_le_bytes(bytes);
        self.aarch64_instruction(cpu, core, pc, word)
    }
    pub(in crate::board) fn aarch64_instruction(
        &mut self,
        cpu: &dyn Cpu,
        core: usize,
        pc: u64,
        word: u32,
    ) -> Result<bool, String> {
        let pstate = scalar(cpu, Register::Pstate)?;
        if word & 0xffd00000 == 0xd5100000 {
            let encoding = ((word >> 5) & 0xffff) as u16;
            let r = arm::SystemRegister::from_encoding(encoding);
            let framework = fujimod_emulator_cpu::arm64::framework_instruction(word);
            if framework {
                if encoding == 0xf665 && pstate & 12 != 12 {
                    return Err("ICC_SRE_EL3 requires EL3".into());
                }
                if pstate & 12 == 0 {
                    return Err("AArch64 EL0 framework register access is unsupported".into());
                }
                let read = word & (1 << 21) != 0;
                let rt = (word & 31) as u64;
                let syndrome = (0x18 << 26)
                    | (u64::from(r.op0) << 20)
                    | (u64::from(r.op1) << 14)
                    | (u64::from(r.crn) << 10)
                    | (u64::from(r.crm) << 1)
                    | (u64::from(r.op2) << 17)
                    | (rt << 5)
                    | u64::from(read);
                self.system_register(cpu, core, syndrome)?;
                cpu.stop()?;
            }
            return Ok(false);
        }
        let hint = match word {
            0xd503205f => Hint::WaitForEvent,
            0xd503207f => Hint::WaitForInterrupt,
            0xd503209f => Hint::SendEvent,
            0xd50320bf => Hint::SendEventLocal,
            _ => return Ok(false),
        };
        set(cpu, Register::Pc, pc.wrapping_add(4))?;
        self.hint(core, hint);
        cpu.stop()?;
        Ok(false)
    }
}
