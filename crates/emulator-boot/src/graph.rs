//! CPU graph assembly from resolved SoC and body facts.
//!
//! This module contains no camera firmware execution policy. Address maps,
//! processor reset state, decoder selections, peripherals, and RAM capacity
//! are read from the selected catalog identities.

use fujimod_catalog::reference::{
    AddressMapping, DecoderSelection, MemoryRegion, ProcessorArchitecture, ProcessorModel,
    ProcessorRegister, RegionSize, ResolvedSelection, Soc,
};
use fujimod_catalog_configuration::RunInstructionRate;
use fujimod_emulator::board::{factory, memory::BusMapping};
use fujimod_emulator_cpu::{Model, Register, arm};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Caller-supplied reset state for one processor in the selected SoC graph.
pub struct ProcessorStart {
    pub processor_id: String,
    pub released: bool,
    pub registers: Vec<(Register, u64)>,
}

fn core_index(soc: &Soc) -> Result<BTreeMap<&str, usize>, String> {
    let mut indexes = BTreeMap::new();
    for (index, processor) in soc.processors.iter().enumerate() {
        if indexes.insert(processor.id.as_str(), index).is_some() {
            return Err(format!("SoC {} repeats processor {}", soc.id, processor.id));
        }
    }
    Ok(indexes)
}

fn extent(size: RegionSize, dram_bytes: u64, label: &str) -> Result<usize, String> {
    let bytes = match size {
        RegionSize::Bytes { bytes } => bytes,
        RegionSize::BodyDram => dram_bytes,
    };
    usize::try_from(bytes).map_err(|_| format!("{label} extent exceeds host address space"))
}

fn register(register: ProcessorRegister) -> Register {
    match register {
        ProcessorRegister::Pc => Register::Pc,
        ProcessorRegister::Sp => Register::Sp,
        ProcessorRegister::Pstate => Register::Pstate,
        ProcessorRegister::Cpsr => Register::Arm32(arm::Register::Cpsr),
    }
}

fn build_processor(
    model: ProcessorModel,
    registers: &[(Register, u64)],
    released: bool,
    mprofile: Option<fujimod_emulator::board::mprofile::Config>,
    rprofile: Option<&fujimod_catalog::reference::RProfileState>,
    architecture: ProcessorArchitecture,
) -> Result<factory::Processor, String> {
    let model = match (architecture, model) {
        (ProcessorArchitecture::Arm32, ProcessorModel::CortexR5f) => Model::CortexR5F,
        (ProcessorArchitecture::Arm32, ProcessorModel::CortexA7) => Model::CortexA7,
        (ProcessorArchitecture::Aarch64, ProcessorModel::CortexA53) => Model::ArmV8A,
        (ProcessorArchitecture::Aarch64, ProcessorModel::CortexM4) => Model::CortexM4,
        _ => return Err("processor model does not match its SoC architecture".into()),
    };
    let rprofile = rprofile
        .map(|state| {
            use fujimod_emulator_cpu_unicorn::rprofile::{
                core::Options,
                tcm::{BankOptions, Options as TcmOptions},
            };
            let bank = |state: fujimod_catalog::reference::TcmState| {
                Ok::<_, String>(BankOptions {
                    bytes: usize::try_from(state.bytes)
                        .map_err(|_| "TCM extent exceeds host address space")?,
                    reset: u32::try_from(state.reset_address)
                        .map_err(|_| "TCM reset address exceeds the R-profile register width")?,
                })
            };
            Ok::<_, String>(Options {
                cp14_identity: state.cp14_identity,
                tcm: TcmOptions {
                    btcm: bank(state.btcm)?,
                    atcm: bank(state.atcm)?,
                },
            })
        })
        .transpose()?;
    Ok(factory::Processor {
        model,
        registers: registers.to_vec(),
        released,
        mprofile,
        rprofile,
    })
}

fn memory_region(region: &MemoryRegion, dram_bytes: u64) -> Result<factory::Ram, String> {
    Ok(factory::Ram {
        base: region.base,
        size: extent(
            region.size,
            dram_bytes,
            &format!("memory region {}", region.id),
        )?,
    })
}

fn address_mapping(
    mapping: &AddressMapping,
    cores: &BTreeMap<&str, usize>,
    dram_bytes: u64,
) -> Result<factory::MemoryMap, String> {
    Ok(factory::MemoryMap {
        core: *cores.get(mapping.core.as_str()).ok_or_else(|| {
            format!(
                "address mapping {} names absent core {}",
                mapping.id, mapping.core
            )
        })?,
        address: mapping.address,
        physical: mapping.physical,
        size: extent(
            mapping.size,
            dram_bytes,
            &format!("address mapping {}", mapping.id),
        )?,
        permissions: u32::from(mapping.permissions),
    })
}

fn decoder_selection(
    selection: &DecoderSelection,
    mappings: &BTreeMap<&str, factory::MemoryMap>,
    dma: &BTreeMap<&str, BusMapping>,
    decoder_id: &str,
) -> Result<fujimod_emulator::board::decoder::Choice, String> {
    let processors = selection
        .processors
        .iter()
        .map(|id| {
            mappings.get(id.as_str()).cloned().ok_or_else(|| {
                format!("decoder {decoder_id} refers to absent address mapping {id}")
            })
        })
        .collect::<Result<_, _>>()?;
    let dma = selection
        .dma_mappings
        .iter()
        .map(|id| {
            dma.get(id.as_str())
                .copied()
                .ok_or_else(|| format!("decoder {decoder_id} refers to absent DMA mapping {id}"))
        })
        .collect::<Result<_, _>>()?;
    Ok(fujimod_emulator::board::decoder::Choice { processors, dma })
}

/// Construct and validate the processor/address graph for one resolved body.
pub(super) fn assemble(
    selection: &ResolvedSelection,
    rate: RunInstructionRate,
    board_ticks_per_second: Option<u64>,
) -> Result<factory::Graph, String> {
    assemble_with_starts(selection, rate, board_ticks_per_second, &[])
}

/// Construct the checked catalog memory/peripheral graph with caller-owned
/// processor reset state. Missing companion state remains held in reset.
pub fn assemble_with_starts(
    selection: &ResolvedSelection,
    rate: RunInstructionRate,
    board_ticks_per_second: Option<u64>,
    starts: &[ProcessorStart],
) -> Result<factory::Graph, String> {
    let soc = &selection.soc;
    let body = &selection.body;
    if body.soc_id != soc.id {
        return Err("resolved body and SoC identities differ".into());
    }
    let cores = core_index(soc)?;
    let companion = soc
        .hardware
        .as_ref()
        .and_then(|hardware| hardware.companion.as_ref());
    let mut start_by_id = BTreeMap::new();
    for start in starts {
        if start_by_id
            .insert(start.processor_id.as_str(), start)
            .is_some()
        {
            return Err(format!(
                "caller repeats reset state for processor {}",
                start.processor_id
            ));
        }
        if !soc
            .processors
            .iter()
            .any(|processor| processor.id == start.processor_id)
        {
            return Err(format!(
                "caller reset state names absent processor {}",
                start.processor_id
            ));
        }
    }
    let processors = soc
        .processors
        .iter()
        .map(|processor| {
            let start = start_by_id.get(processor.id.as_str()).copied();
            let registers = if let Some(start) = start {
                start.registers.clone()
            } else {
                processor
                    .registers
                    .iter()
                    .map(|item| (register(item.register), item.value))
                    .collect::<Vec<_>>()
            };
            let mprofile = if processor.model == ProcessorModel::CortexM4 {
                let companion = companion
                    .filter(|companion| companion.processor_id == processor.id)
                    .ok_or_else(|| {
                        format!(
                            "SoC {} has no companion facts for core {}",
                            soc.id, processor.id
                        )
                    })?;
                let clock_hz = soc
                    .clocks
                    .iter()
                    .find(|clock| clock.id == companion.clock_id)
                    .and_then(|clock| clock.frequency_hz)
                    .ok_or_else(|| {
                        format!(
                            "SoC {} has no frequency for companion clock {}",
                            soc.id, companion.clock_id
                        )
                    })?;
                Some(fujimod_emulator::board::mprofile::Config {
                    clock_hz,
                    board_ticks_per_second: board_ticks_per_second
                        .ok_or("M-profile companion requires a run timebase")?,
                    vector_base: u32::try_from(companion.vector_base)
                        .map_err(|_| "M-profile vector base exceeds 32 bits")?,
                })
            } else {
                None
            };
            build_processor(
                processor.model,
                &registers,
                start
                    .map(|state| state.released)
                    .or(processor.released)
                    .or_else(|| {
                        companion
                            .is_some_and(|companion| companion.processor_id == processor.id)
                            .then_some(false)
                    })
                    .ok_or_else(|| {
                        format!(
                            "SoC {} does not declare the reset-release state for core {}",
                            soc.id, processor.id
                        )
                    })?,
                mprofile,
                processor.rprofile.as_ref(),
                soc.architecture.clone(),
            )
        })
        .collect::<Result<_, _>>()?;
    let ram = soc
        .memory_regions
        .iter()
        .map(|region| memory_region(region, body.dram_bytes))
        .collect::<Result<_, _>>()?;
    let mapping_by_id = soc
        .mappings
        .iter()
        .map(|mapping| {
            Ok::<_, String>((
                mapping.id.as_str(),
                address_mapping(mapping, &cores, body.dram_bytes)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut decoder_mappings = BTreeSet::new();
    let mut initial_mappings = BTreeSet::new();
    let mut decoder_dma = BTreeSet::new();
    let mut initial_dma = BTreeSet::new();
    for decoder in &soc.decoders {
        initial_mappings.extend(decoder.initial.processors.iter().map(String::as_str));
        decoder_mappings.extend(decoder.initial.processors.iter().map(String::as_str));
        initial_dma.extend(decoder.initial.dma_mappings.iter().map(String::as_str));
        decoder_dma.extend(decoder.initial.dma_mappings.iter().map(String::as_str));
        for choice in &decoder.choices {
            decoder_mappings.extend(choice.selection.processors.iter().map(String::as_str));
            decoder_dma.extend(choice.selection.dma_mappings.iter().map(String::as_str));
        }
    }
    let mappings = mapping_by_id
        .iter()
        .filter_map(|(id, mapping)| {
            (!decoder_mappings.contains(id) || initial_mappings.contains(id))
                .then_some(mapping.clone())
        })
        .collect();
    let dma_by_id = soc
        .dma_mappings
        .iter()
        .map(|mapping| {
            Ok::<_, String>((
                mapping.id.as_str(),
                BusMapping {
                    address: mapping.address,
                    physical: mapping.physical,
                    size: extent(
                        mapping.size,
                        body.dram_bytes,
                        &format!("DMA mapping {}", mapping.id),
                    )?,
                    permissions: u32::from(mapping.permissions),
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let dma_mappings = dma_by_id
        .iter()
        .filter_map(|(id, mapping)| {
            (!decoder_dma.contains(id) || initial_dma.contains(id)).then_some(*mapping)
        })
        .collect();
    let decoders = soc
        .decoders
        .iter()
        .map(|decoder| {
            let initial =
                decoder_selection(&decoder.initial, &mapping_by_id, &dma_by_id, &decoder.id)?;
            let mut choices = decoder
                .choices
                .iter()
                .map(|choice| {
                    decoder_selection(&choice.selection, &mapping_by_id, &dma_by_id, &decoder.id)
                })
                .collect::<Result<Vec<_>, _>>()?;
            // Control bindings index the declared choices. Reuse a listed reset
            // state, or append an unlisted one without shifting those indexes.
            let initial_index = decoder
                .choices
                .iter()
                .position(|choice| choice.selection == decoder.initial)
                .unwrap_or_else(|| {
                    let index = choices.len();
                    choices.push(initial);
                    index
                });
            Ok::<_, String>(fujimod_emulator::board::decoder::Decoder {
                initial: initial_index,
                choices,
            })
        })
        .collect::<Result<_, _>>()?;
    let peripherals = soc
        .peripherals
        .iter()
        .map(|mapping| {
            let core = *cores
                .get(mapping.core.as_str())
                .ok_or_else(|| format!("peripheral mapping names absent core {}", mapping.core))?;
            let block = soc
                .blocks
                .iter()
                .find(|block| block.id == mapping.block_id)
                .ok_or_else(|| {
                    format!("peripheral mapping names absent block {}", mapping.block_id)
                })?;
            let offset = mapping.offset.unwrap_or(0);
            if offset >= block.size {
                return Err(format!(
                    "peripheral mapping offset exceeds block {}",
                    block.id
                ));
            }
            Ok(factory::Peripheral {
                core,
                address: mapping.address,
                physical: block
                    .base
                    .checked_add(offset)
                    .ok_or("peripheral physical address overflows")?,
                size: usize::try_from(block.size - offset).map_err(|_| {
                    format!("peripheral block {} exceeds host address space", block.id)
                })?,
            })
        })
        .collect::<Result<_, String>>()?;
    let (numerator, denominator) = (rate.numerator, rate.denominator);
    let graph = factory::Graph {
        processors,
        ram,
        dma_mappings,
        decoders,
        mappings,
        peripherals,
        instructions_per_tick: fujimod_emulator_cpu::timeline::Ratio::new(numerator, denominator)?,
    };
    graph.validate()?;
    Ok(graph)
}

fn processor_model(model: Model) -> &'static str {
    match model {
        Model::CortexR5F => "cortex-r5f",
        Model::CortexA7 => "cortex-a7",
        Model::CortexM4 => "cortex-m4",
        Model::ArmV8A => "armv8a",
    }
}

fn processor_register(register: Register) -> Value {
    match register {
        Register::Diagnostic(value) => json!({"Diagnostic": format!("{value:?}")}),
        Register::Arm32(value) => json!({"Arm32": format!("{value:?}")}),
        Register::X(value) => json!({"X": value}),
        Register::Pc => json!("Pc"),
        Register::Sp => json!("Sp"),
        Register::Pstate => json!("Pstate"),
        Register::Fpcr => json!("Fpcr"),
        Register::Fpsr => json!("Fpsr"),
        Register::V(value) => json!({"V": value}),
        Register::Arm64System(value) => json!({
            "Arm64System": {
                "op0": value.op0,
                "op1": value.op1,
                "crn": value.crn,
                "crm": value.crm,
                "op2": value.op2,
            }
        }),
    }
}

fn memory_map(map: &factory::MemoryMap) -> Value {
    json!({
        "core": map.core,
        "address": map.address,
        "physical": map.physical,
        "size": map.size,
        "permissions": map.permissions,
    })
}

fn dma_map(map: &BusMapping) -> Value {
    json!({
        "address": map.address,
        "physical": map.physical,
        "size": map.size,
        "permissions": map.permissions,
    })
}

/// Serialize the validated processor graph in the same closed shape accepted
/// by `factory::Graph` deserialization for native callers.
pub(super) fn value(
    selection: &ResolvedSelection,
    graph: &factory::Graph,
    rate: RunInstructionRate,
) -> Value {
    let processors = graph
        .processors
        .iter()
        .enumerate()
        .map(|(index, processor)| {
            let rprofile = selection.soc.processors[index].rprofile.map(|profile| {
                json!({
                    "cp14_identity": profile.cp14_identity,
                    "tcm": {
                        "btcm": {
                            "bytes": profile.btcm.bytes,
                            "reset": profile.btcm.reset_address,
                        },
                        "atcm": {
                            "bytes": profile.atcm.bytes,
                            "reset": profile.atcm.reset_address,
                        },
                    },
                })
            });
            json!({
                "model": processor_model(processor.model),
                "released": processor.released,
                "registers": processor.registers.iter().map(|(register, value)| {
                    json!([processor_register(*register), value])
                }).collect::<Vec<_>>(),
                "mprofile": processor.mprofile.map(|profile| json!({
                    "clock_hz": profile.clock_hz,
                    "board_ticks_per_second": profile.board_ticks_per_second,
                    "vector_base": profile.vector_base,
                })),
                "rprofile": rprofile,
            })
        })
        .collect::<Vec<_>>();
    let decoders = graph
        .decoders
        .iter()
        .map(|decoder| {
            json!({
                "initial": decoder.initial,
                "choices": decoder.choices.iter().map(|choice| {
                    json!({
                        "processors": choice.processors.iter().map(memory_map).collect::<Vec<_>>(),
                        "dma": choice.dma.iter().map(dma_map).collect::<Vec<_>>(),
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "processors": processors,
        "ram": graph.ram.iter().map(|ram| json!({"base":ram.base,"size":ram.size})).collect::<Vec<_>>(),
        "dma_mappings": graph.dma_mappings.iter().map(dma_map).collect::<Vec<_>>(),
        "decoders": decoders,
        "mappings": graph.mappings.iter().map(memory_map).collect::<Vec<_>>(),
        "peripherals": graph.peripherals.iter().map(|peripheral| json!({
            "core": peripheral.core,
            "address": peripheral.address,
            "physical": peripheral.physical,
            "size": peripheral.size,
        })).collect::<Vec<_>>(),
        "instructions_per_tick": [rate.numerator, rate.denominator],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_catalog::reference::{
        AddressDecoder, Body, DecoderChoice, Firmware, MemoryKind, ProcessorCore, RProfileState,
        SocReset, TcmState,
    };

    fn r5(id: &str) -> ProcessorCore {
        ProcessorCore {
            id: id.into(),
            model: ProcessorModel::CortexR5f,
            released: Some(true),
            registers: vec![],
            mprofile: None,
            rprofile: Some(RProfileState {
                cp14_identity: 0,
                atcm: TcmState {
                    bytes: 0,
                    reset_address: 0,
                },
                btcm: TcmState {
                    bytes: 0,
                    reset_address: 0,
                },
            }),
        }
    }

    #[test]
    fn decoder_selectors_resolve_address_mapping_ids_not_core_ids() {
        let soc = Soc {
            schema_uri: fujimod_catalog::reference::SOC_SCHEMA_URI.into(),
            schema: 1,
            id: "test-soc".into(),
            architecture: ProcessorArchitecture::Arm32,
            processors: vec![r5("core0")],
            memory_regions: vec![MemoryRegion {
                id: "ram".into(),
                base: 0x2000_0000,
                size: RegionSize::Bytes { bytes: 0x2000 },
                kind: MemoryKind::Dram,
            }],
            blocks: vec![],
            interrupts: vec![],
            clocks: vec![],
            hardware: None,
            mappings: vec![
                AddressMapping {
                    id: "default-window".into(),
                    core: "core0".into(),
                    address: 0,
                    physical: 0x2000_0000,
                    size: RegionSize::Bytes { bytes: 0x1000 },
                    permissions: 7,
                },
                AddressMapping {
                    id: "alternate-window".into(),
                    core: "core0".into(),
                    address: 0,
                    physical: 0x2000_1000,
                    size: RegionSize::Bytes { bytes: 0x1000 },
                    permissions: 7,
                },
            ],
            dma_mappings: vec![
                fujimod_catalog::reference::DmaMapping {
                    id: "default-dma".into(),
                    address: 0,
                    physical: 0x2000_0000,
                    size: RegionSize::Bytes { bytes: 0x1000 },
                    permissions: 3,
                },
                fujimod_catalog::reference::DmaMapping {
                    id: "alternate-dma".into(),
                    address: 0,
                    physical: 0x2000_1000,
                    size: RegionSize::Bytes { bytes: 0x1000 },
                    permissions: 3,
                },
            ],
            decoders: vec![AddressDecoder {
                id: "boot-window".into(),
                initial: DecoderSelection {
                    processors: vec!["default-window".into()],
                    dma_mappings: vec!["default-dma".into()],
                },
                choices: vec![DecoderChoice {
                    id: "alternate".into(),
                    selection: DecoderSelection {
                        processors: vec!["alternate-window".into()],
                        dma_mappings: vec!["alternate-dma".into()],
                    },
                }],
            }],
            peripherals: vec![],
            reset: SocReset {
                release: None,
                coprocessor_writes: vec![],
            },
        };
        let body = Body {
            schema_uri: fujimod_catalog::reference::BODY_SCHEMA_URI.into(),
            schema: 1,
            id: "test-body".into(),
            soc_id: "test-soc".into(),
            dram_bytes: 0x2000,
            hardware: None,
            ram_regions: vec![],
            nand: None,
            sensor: None,
            lens: None,
            display: None,
            microcontroller: None,
            power_control: None,
            card_slot: None,
            control_bus_chips: vec![],
            connections: vec![],
            parts: vec![],
            wiring: vec![],
        };
        let selection = ResolvedSelection {
            soc,
            body,
            firmware: Firmware {
                schema_uri: fujimod_catalog::reference::FIRMWARE_SCHEMA_URI.into(),
                schema: 1,
                id: "test-firmware".into(),
                body_id: "test-body".into(),
                version: "test".into(),
                update: None,
                boot_rom: None,
                installation: None,
                generated_facts: None,
                layouts: None,
            },
            parts: vec![],
            physical_controls: None,
            control_ids: None,
            first_start: None,
            bulk: None,
            settings: None,
            flash: None,
            state: None,
        };
        let graph = assemble(
            &selection,
            RunInstructionRate {
                numerator: 24,
                denominator: 1,
            },
            None,
        )
        .unwrap();
        assert_eq!(graph.mappings.len(), 1);
        assert_eq!(graph.mappings[0].physical, 0x2000_0000);
        assert_eq!(graph.dma_mappings.len(), 1);
        assert_eq!(graph.decoders.len(), 1);
        assert_eq!(graph.decoders[0].initial, 1);
        assert_eq!(graph.decoders[0].choices.len(), 2);
        assert_eq!(
            graph.decoders[0].choices[0].processors[0].physical,
            0x2000_1000
        );
        assert_eq!(graph.decoders[0].choices[0].dma[0].physical, 0x2000_1000);
    }

    #[test]
    fn listed_reset_selection_preserves_rom_and_ram_handoff_indexes() {
        let mut selection = fujimod_catalog::ReferenceCatalog::bundled()
            .unwrap()
            .resolve("x70", "x70-110")
            .unwrap();
        let rate = RunInstructionRate {
            numerator: 24,
            denominator: 1,
        };
        let graph = assemble(&selection, rate, None).unwrap();
        let decoder = &graph.decoders[0];
        assert_eq!(decoder.initial, 0);
        assert_eq!(decoder.choices.len(), 2);
        assert_eq!(decoder.choices[0].processors[0].physical, 0x8000_0000);
        assert_eq!(decoder.choices[1].processors[0].physical, 0);
        assert_eq!(decoder.choices[1].dma[0].physical, 0);

        selection.soc.decoders[0].initial = selection.soc.decoders[0].choices[1].selection.clone();
        let graph = assemble(&selection, rate, None).unwrap();
        let decoder = &graph.decoders[0];
        assert_eq!(decoder.initial, 1);
        assert_eq!(decoder.choices.len(), 2);
        assert_eq!(decoder.choices[0].processors[0].physical, 0x8000_0000);
        assert_eq!(decoder.choices[1].processors[0].physical, 0);
    }
}
