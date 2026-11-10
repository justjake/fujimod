//! ARMv7-A cluster identity and architectural instruction boundaries.
//! These inputs are processor wiring, independent of any guest program.
use super::{Board, scalar, set};
use fujimod_emulator_cpu::{Access, Cpu, Hint, Model, Register, arm};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub mpidr: u32,
    pub configuration_base: u32,
    pub cycles_per_tick: u64,
    /// Declared L1 data, L1 instruction and L2 capacities in bytes; CCSIDR
    /// reads report them. Absent, the processor model's own values apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_sizes: Option<(u32, u32, u32)>,
}
impl Config {
    /// CCSIDR for CSSELR 0 (L1 data), 1 (L1 instruction) and 2 (L2):
    /// 64-byte lines; 4-way L1 data, 2-way L1 instruction, 8-way L2.
    fn cache_ids(&self) -> Option<[u32; 3]> {
        self.cache_sizes.map(|(data, instruction, l2)| {
            [
                0x7000001a | ((data / 256 - 1) << 13),
                0x20000009 | ((instruction / 64 - 1) << 13),
                0x7000003a | ((l2 / 512 - 1) << 13),
            ]
        })
    }
}
pub enum Entry {
    Execute,
    Completed,
    Hint(Hint),
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub config: Config,
    pmcr: u32,
    enabled: bool,
    counter: u32,
    anchor: u64,
}
impl Profile {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            pmcr: 0,
            enabled: false,
            counter: 0,
            anchor: 0,
        }
    }
    fn cycles(&self, now: u64) -> u32 {
        if self.pmcr & 1 == 0 || !self.enabled {
            return self.counter;
        }
        let cycles = u128::from(now - self.anchor) * u128::from(self.config.cycles_per_tick);
        self.counter
            .wrapping_add((cycles / if self.pmcr & 8 != 0 { 64 } else { 1 }) as u32)
    }
    pub fn before(&mut self, cpu: &dyn Cpu, pc: u64, now: u64) -> Result<Entry, String> {
        let cpsr = scalar(cpu, arm::CPSR)? as u32;
        if cpsr & 0x20 != 0 {
            return Ok(Entry::Execute);
        }
        let mut bytes = [0; 4];
        cpu.read_virtual(pc, Access::Fetch, &mut bytes)?;
        let word = u32::from_le_bytes(bytes);
        let body = word & 0x0fff_ffff;
        if word >> 28 != 15 && !arm::condition_passes(word >> 28, cpsr) {
            return Ok(Entry::Execute);
        }
        if body & 0x0f00_0f10 == 0x0e00_0f10 && (body >> 16) & 15 == 9 {
            let crm = body & 15;
            let op1 = (body >> 21) & 7;
            let op2 = (body >> 5) & 7;
            if op1 == 0 && matches!(crm, 12 | 13) {
                let rd = ((word >> 12) & 15) as usize;
                if rd == 15 {
                    return Err("PMU access to APSR is unsupported".into());
                }
                let read = body & (1 << 20) != 0;
                let counter = self.cycles(now);
                if read {
                    let value = match (crm, op2) {
                        (12, 0) => 0x4103_3000 | self.pmcr,
                        (12, 1 | 2) => u32::from(self.enabled) << 31,
                        (13, 0) => counter,
                        _ => return Err("ARMv7-A PMU register is unsupported".into()),
                    };
                    set(cpu, arm::CORE[rd], value.into())?;
                } else {
                    let value = scalar(cpu, arm::CORE[rd])? as u32;
                    self.counter = counter;
                    self.anchor = now;
                    match (crm, op2) {
                        (12, 0) if value & !15 == 0 => {
                            self.pmcr = value & 9;
                            if value & 4 != 0 {
                                self.counter = 0;
                            }
                        }
                        (12, 1) if value & !0x80000000 == 0 => self.enabled |= value != 0,
                        (12, 2) if value & !0x80000000 == 0 => self.enabled &= value == 0,
                        (13, 0) => self.counter = value,
                        _ => return Err("ARMv7-A PMU operation is unsupported".into()),
                    }
                }
                set(cpu, Register::Pc, pc.wrapping_add(4))?;
                cpu.stop()?;
                return Ok(Entry::Completed);
            }
        }
        let cp = |opc1, crn, crm, opc2| arm::Coprocessor {
            cp: 15,
            opc1,
            crn,
            crm,
            opc2,
            secure: 0,
            wide: false,
        };
        let identity = match body & !0xf000 {
            0x0e10_0fb0 => Some(self.config.mpidr),
            0x0e9f_0f10 => Some(self.config.configuration_base),
            0x0e30_0f10 => Some(match self.config.cache_ids() {
                Some(ids) => {
                    let selector = cpu.coprocessor(cp(2, 0, 0, 0), None)? as usize;
                    *ids.get(selector)
                        .ok_or("Reserved Cortex-A7 cache selector")?
                }
                None => cpu.coprocessor(cp(1, 0, 0, 0), None)? as u32,
            }),
            _ => None,
        };
        let entry = if let Some(value) = identity {
            let rd = ((word >> 12) & 15) as usize;
            if rd == 15 {
                return Err("ARMv7-A identity read to APSR is unsupported".into());
            }
            set(cpu, arm::CORE[rd], u64::from(value))?;
            Entry::Completed
        } else {
            match body {
                0x0320_f002 => Entry::Hint(Hint::WaitForEvent),
                0x0320_f003 => Entry::Hint(Hint::WaitForInterrupt),
                0x0320_f004 => Entry::Hint(Hint::SendEvent),
                _ => return Ok(Entry::Execute),
            }
        };
        set(cpu, Register::Pc, pc.wrapping_add(4))?;
        cpu.stop()?;
        Ok(entry)
    }
    pub(super) fn enter_irq(&self, cpu: &dyn Cpu) -> Result<bool, String> {
        if !fujimod_emulator_cpu::armv7a::enter_irq(cpu)? {
            return Ok(false);
        }
        cpu.stop()?;
        Ok(true)
    }
}
impl Board {
    /// ARMv7-A exception entries on `core` from the board and its backend.
    pub fn armv7a_exception_entries(
        &self,
        core: usize,
    ) -> Result<fujimod_emulator_cpu::armv7a::Entries, String> {
        let board = *self
            .state
            .borrow()
            .armv7a_entries
            .get(core)
            .ok_or("unknown core")?;
        Ok(board.plus(self.cpu(core)?.exception_entries().unwrap_or_default()))
    }
    pub fn configure_armv7a(&mut self, core: usize, config: Config) -> Result<(), String> {
        if self.cpu(core)?.model() != Model::CortexA7
            || config.configuration_base & 0x7fff != 0
            || config.cycles_per_tick == 0
            || config.cache_sizes.is_some_and(|(data, instruction, l2)| {
                ![8192, 16384, 32768, 65536].contains(&data)
                    || ![8192, 16384, 32768, 65536].contains(&instruction)
                    || ![131072, 262144, 524288, 1048576].contains(&l2)
            })
        {
            return Err("ARMv7-A configuration requires an A7 and aligned peripheral base".into());
        }
        let mut state = self.state.borrow_mut();
        if state.instructions[core] != 0 || state.armv7a[core].is_some() {
            return Err("ARMv7-A reset wiring must be configured once before execution".into());
        }
        state.armv7a[core] = Some(Profile::new(config));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{
        bus::{Bus, Mode},
        factory,
    };
    use serde_json::json;

    #[test]
    fn relocated_a7_code_observes_each_core_identity_cache_ids_and_guest_time_then_waits() {
        let processors: Vec<_> = (0..2)
            .map(|core| {
                json!({
                    "model":"cortex-a7", "released":true, "mprofile":null, "rprofile":null,
                    "registers":[["Pc",0x1000],["Pstate",0xd3],[{"Arm32":{"R":1}},0x2000+core*16]]
                })
            })
            .collect();
        let graph: factory::Graph = serde_json::from_value(json!({
            "processors":processors,"ram":[{"base":0,"size":0x4000}],
            "mappings":(0..2).map(|core|json!({"core":core,"address":0,"physical":0,"size":0x4000,"permissions":7})).collect::<Vec<_>>(),
            "dma_mappings":[{"address":0,"physical":0,"size":0x4000,"permissions":3}],
            "peripherals":[],"decoders":[],"instructions_per_tick":[1,1]
        })).unwrap();
        let mut board = graph
            .build(
                factory::Options::new(factory::Engine::Unicorn)
                    .with_translation_cache_size_bytes(1 << 20),
                Bus::new(Mode::Strict),
                None,
            )
            .unwrap();
        for core in 0..2 {
            board
                .configure_armv7a(
                    core,
                    Config {
                        mpidr: 0x80000000 | core as u32,
                        configuration_base: 0xffff0000,
                        cycles_per_tick: 1,
                        // Core 1 keeps the processor model's own cache IDs.
                        cache_sizes: (core == 0).then_some((32768, 32768, 1048576)),
                    },
                )
                .unwrap();
        }
        // MPIDR, CBAR, PMU cycle counter, then CSSELR = L2 and its CCSIDR.
        let words: [u32; 15] = [
            0xee100fb0, 0xe5810000, 0xee9f0f10, 0xe5810004, 0xe3a00007, 0xee090f1c, 0xe3a00102,
            0xee090f3c, 0xee192f1d, 0xe5812008, 0xe3a00002, 0xee400f10, 0xee303f10, 0xe581300c,
            0xe320f003,
        ];
        board
            .write_physical(
                0x1000,
                &words
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        board.run_for_instructions(100, 10).unwrap();
        let mut bytes = [0; 32];
        board.read_physical(0x2000, &mut bytes).unwrap();
        for core in 0usize..2 {
            let w = |offset: usize| {
                u32::from_le_bytes(
                    bytes[core * 16 + offset..core * 16 + offset + 4]
                        .try_into()
                        .unwrap(),
                )
            };
            assert_eq!(w(0), 0x80000000 | core as u32);
            assert_eq!(w(4), 0xffff0000);
            assert!(w(8) > 0);
        }
        let l2 = |core: usize| {
            u32::from_le_bytes(bytes[core * 16 + 12..core * 16 + 16].try_into().unwrap())
        };
        assert_eq!(l2(0), 0x70ffe03a);
        assert!(l2(1) != 0 && l2(1) != l2(0));
        assert_eq!(board.instruction_counts(), [15, 15]);
        assert!(
            board
                .configure_armv7a(
                    0,
                    Config {
                        mpidr: 0,
                        configuration_base: 0,
                        cycles_per_tick: 1,
                        cache_sizes: None,
                    }
                )
                .is_err()
        );
    }
    #[test]
    fn disabled_vfp_enters_the_undefined_vector_and_the_handler_retries_it() {
        let graph: factory::Graph = serde_json::from_value(json!({
            "processors":[{"model":"cortex-a7","released":true,"mprofile":null,"rprofile":null,
                "registers":[["Pc",0x1000],["Pstate",0xd3]]}],
            "ram":[{"base":0,"size":0x2000}],
            "mappings":[{"core":0,"address":0,"physical":0,"size":0x2000,"permissions":7}],
            "dma_mappings":[{"address":0,"physical":0,"size":0x2000,"permissions":3}],
            "peripherals":[],"decoders":[],"instructions_per_tick":[1,1]
        }))
        .unwrap();
        let mut b = graph
            .build(
                factory::Options::new(factory::Engine::Unicorn)
                    .with_translation_cache_size_bytes(1 << 20),
                Bus::new(Mode::Strict),
                None,
            )
            .unwrap();
        b.configure_armv7a(
            0,
            Config {
                mpidr: 0,
                configuration_base: 0xffff0000,
                cycles_per_tick: 1,
                cache_sizes: None,
            },
        )
        .unwrap();
        let words = |words: &[u32]| {
            words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>()
        };
        // Undefined vector: b 0x100.
        b.write_physical(4, &words(&[0xea00003d])).unwrap();
        // Enable CP10/CP11 in CPACR and FPEXC.EN, count, then retry the
        // faulting instruction: subs pc, lr, #4.
        b.write_physical(
            0x100,
            &words(&[
                0xe3a0060f, 0xee010f50, 0xf57ff06f, 0xe3a00101, 0xeee80a10, 0xe2833001, 0xe25ef004,
            ]),
        )
        .unwrap();
        // vmov s0, r0; mov r2, #2; wfi.
        b.write_physical(0x1000, &words(&[0xee000a10, 0xe3a02002, 0xe320f003]))
            .unwrap();
        b.run_for_instructions(100, 10).unwrap();
        assert_eq!(b.read_register(0, arm::CORE[3]).unwrap(), 1);
        assert_eq!(b.read_register(0, arm::CORE[2]).unwrap(), 2);
        let entries = b.armv7a_exception_entries(0).unwrap();
        assert_eq!(
            (entries.undefined, entries.irq, entries.data_abort),
            (1, 0, 0)
        );
    }
    #[test]
    fn zero_address_observation_stops_before_emulation_and_counts_each_resumed_instruction_once() {
        let graph: factory::Graph = serde_json::from_value(json!({
            "processors":[{"model":"cortex-a7","released":true,"mprofile":null,"rprofile":null,
                "registers":[["Pc",0],["Pstate",0xd3]]}],
            "ram":[{"base":0,"size":0x1000}],
            "mappings":[{"core":0,"address":0,"physical":0,"size":0x1000,"permissions":7}],
            "dma_mappings":[{"address":0,"physical":0,"size":0x1000,"permissions":3}],
            "peripherals":[],"decoders":[],"instructions_per_tick":[1,1]
        }))
        .unwrap();
        let mut b = graph
            .build(
                factory::Options::new(factory::Engine::Unicorn)
                    .with_translation_cache_size_bytes(1 << 20),
                Bus::new(Mode::Strict),
                None,
            )
            .unwrap();
        b.configure_armv7a(
            0,
            Config {
                mpidr: 0,
                configuration_base: 0xffff0000,
                cycles_per_tick: 1,
                cache_sizes: None,
            },
        )
        .unwrap();
        b.write_physical(
            0,
            &[0xe3a00007u32, 0xe2800001, 0xeafffffc]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let mut points = b.instruction_points_begin(0, vec![0]).unwrap();
        b.run_for_instructions(100, 10).unwrap();
        assert_eq!(points.event(), Some(0));
        assert_eq!(b.instruction_counts(), [0]);
        assert_eq!(b.read_register(0, arm::CORE[0]).unwrap(), 0);
        assert_eq!(b.read_register(0, Register::Pc).unwrap(), 0);
        b.run_for_instructions(100, 10).unwrap();
        assert_eq!(points.event(), Some(0));
        assert_eq!(b.instruction_counts(), [3]);
        assert_eq!(b.read_register(0, arm::CORE[0]).unwrap(), 8);
        b.instruction_points_update(0, &mut points, vec![4])
            .unwrap();
        b.run_for_instructions(100, 10).unwrap();
        assert_eq!(points.event(), Some(4));
        assert_eq!(b.read_register(0, Register::Pc).unwrap(), 4);
        // A deactivated address keeps its hook and stops again once active.
        b.instruction_points_update(0, &mut points, vec![0])
            .unwrap();
        b.run_for_instructions(100, 10).unwrap();
        assert_eq!(points.event(), Some(0));
        assert_eq!(b.instruction_counts(), [6]);
        // A stop whose address is deactivated before resuming leaves no skip
        // behind to swallow that address's next hit.
        b.instruction_points_update(0, &mut points, vec![0x100])
            .unwrap();
        b.run_for_instructions(9, 10).unwrap();
        assert_eq!(b.read_register(0, Register::Pc).unwrap(), 0);
        b.instruction_points_update(0, &mut points, vec![0])
            .unwrap();
        b.run_for_instructions(100, 10).unwrap();
        assert_eq!(points.event(), Some(0));
        assert_eq!(b.instruction_counts(), [15]);
        b.instruction_points_end(0, &mut points).unwrap();
        b.run_for_instructions(3, 10).unwrap();
        assert_eq!(b.instruction_counts(), [18]);
    }
}
