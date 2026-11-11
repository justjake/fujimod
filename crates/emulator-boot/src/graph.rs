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

/// CPU apertures cover each processor's mapping pages; device bus windows retain
/// their register bounds.
fn peripheral_pages(
    processors: &[factory::Processor],
    mappings: Vec<factory::Peripheral>,
) -> Result<Vec<factory::Peripheral>, String> {
    let mut pages = BTreeMap::new();
    for mapping in mappings {
        let page = processors
            .get(mapping.core)
            .ok_or("peripheral mapping names an absent processor")?
            .mapping_granule();
        if mapping.size == 0 || mapping.address % page != mapping.physical % page {
            return Err("peripheral mapping requires a nonempty, page-congruent range".into());
        }
        let start = mapping.address & !(page - 1);
        let physical = mapping.physical & !(page - 1);
        let end = mapping
            .address
            .checked_add(mapping.size as u64)
            .and_then(|end| end.checked_add(page - 1))
            .ok_or("peripheral page range overflows")?
            & !(page - 1);
        physical
            .checked_add(end - start)
            .ok_or("peripheral physical page range overflows")?;
        for address in (start..end).step_by(page as usize) {
            let physical = physical + (address - start);
            if pages
                .insert((mapping.core, address), (physical, page))
                .is_some_and(|(old, _)| old != physical)
            {
                return Err("peripheral page has conflicting physical mappings".into());
            }
        }
    }
    let mut runs: Vec<factory::Peripheral> = Vec::new();
    for ((core, address), (physical, page)) in pages {
        if let Some(last) = runs.last_mut()
            && last.core == core
            && last.address + last.size as u64 == address
            && last.physical + last.size as u64 == physical
        {
            last.size = last
                .size
                .checked_add(page as usize)
                .ok_or("peripheral aperture exceeds host address space")?;
        } else {
            runs.push(factory::Peripheral {
                core,
                address,
                physical,
                size: page as usize,
            });
        }
    }
    Ok(runs)
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
                data_cache: state.data_cache.map(|cache| {
                    fujimod_emulator_cpu_unicorn::rprofile::data_cache::Options {
                        line_bytes: cache.line_bytes,
                        sets: cache.sets,
                        ways: cache.ways,
                        checked: Vec::new(),
                    }
                }),
                ..Default::default()
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
    dram_address_space_limit_bytes: u64,
    rate: RunInstructionRate,
    board_ticks_per_second: Option<u64>,
) -> Result<factory::Graph, String> {
    assemble_with_starts(
        selection,
        dram_address_space_limit_bytes,
        rate,
        board_ticks_per_second,
        &[],
    )
}

/// Construct the checked catalog memory/peripheral graph with caller-owned
/// processor reset state. Missing companion state remains held in reset.
pub fn assemble_with_starts(
    selection: &ResolvedSelection,
    dram_address_space_limit_bytes: u64,
    rate: RunInstructionRate,
    board_ticks_per_second: Option<u64>,
    starts: &[ProcessorStart],
) -> Result<factory::Graph, String> {
    let soc = &selection.soc;
    let body = &selection.body;
    if dram_address_space_limit_bytes == 0 || !dram_address_space_limit_bytes.is_multiple_of(4096) {
        return Err("DRAM address-space limit must be positive and page-aligned".into());
    }
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
    let processors: Vec<factory::Processor> = soc
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
        .map(|region| memory_region(region, dram_address_space_limit_bytes))
        .collect::<Result<_, _>>()?;
    let mapping_by_id = soc
        .mappings
        .iter()
        .map(|mapping| {
            Ok::<_, String>((
                mapping.id.as_str(),
                address_mapping(mapping, &cores, dram_address_space_limit_bytes)?,
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
                        dram_address_space_limit_bytes,
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
    let mut peripherals: Vec<factory::Peripheral> = peripherals;
    if let Some(drain) = body
        .hardware
        .as_ref()
        .and_then(|hardware| hardware.bus_drain.as_ref())
    {
        for id in &drain.processor_ids {
            let core = *cores
                .get(id.as_str())
                .ok_or_else(|| format!("bus drain names absent core {id}"))?;
            peripherals.push(factory::Peripheral {
                core,
                address: drain.address,
                physical: drain.address,
                size: 4,
            });
        }
    }
    let peripherals = peripheral_pages(&processors, peripherals)?;
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

/// Indexed registers keep their index as data, as `arm::Register` deserializes them.
fn arm_register(register: arm::Register) -> Value {
    match register {
        arm::Register::R(index) => json!({"R": index}),
        arm::Register::D(index) => json!({"D": index}),
        arm::Register::Q(index) => json!({"Q": index}),
        arm::Register::S(index) => json!({"S": index}),
        named => json!(format!("{named:?}")),
    }
}

fn processor_register(register: Register) -> Value {
    match register {
        Register::Diagnostic(value) => json!({"Diagnostic": format!("{value:?}")}),
        Register::Arm32(value) => json!({"Arm32": arm_register(value)}),
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

    #[test]
    fn exr2_image_transfer_maps_cpu_pages_and_keeps_device_register_bounds() {
        // The image-transfer block occupies 32 bytes, smaller than a CPU page.
        let prepared = crate::prepare("x70-110-rec", &[]).unwrap();
        for core in 0..2 {
            let aperture = prepared
                .execution
                .peripherals
                .iter()
                .find(|p| {
                    p.core == core
                        && p.address <= 0xfff64000
                        && p.address + p.size as u64 >= 0xfff65000
                })
                .expect("both CPUs must map the image-transfer page");
            assert_eq!(aperture.address % 0x1000, 0);
            assert_eq!(aperture.physical % 0x1000, 0);
            assert_eq!(aperture.size % 0x1000, 0);
        }
        let window = prepared.devices_value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|node| node["windows"].as_array().unwrap())
            .find(|window| window["base"] == 0xfff64000u64)
            .unwrap();
        assert_eq!(window["size"], 0x20);
    }

    #[test]
    fn peripheral_pages_merge_shared_pages_and_preserve_aliases() {
        let mapping = |core, address, physical, size| factory::Peripheral {
            core,
            address,
            physical,
            size,
        };
        let processor = |model| factory::Processor {
            model,
            registers: vec![],
            released: true,
            mprofile: None,
            rprofile: None,
        };
        let processors = [
            processor(Model::CortexR5F),
            processor(Model::CortexR5F),
            processor(Model::ArmV8A),
        ];
        let pages = peripheral_pages(
            &processors,
            vec![
                mapping(0, 0x1020, 0x8020, 0x20),
                mapping(0, 0x1080, 0x8080, 0x1000),
                mapping(1, 0x5020, 0x8020, 0x20),
                mapping(2, 0x5020, 0x9020, 0x20),
            ],
        )
        .unwrap();
        assert_eq!(pages.len(), 3);
        assert_eq!(
            (
                pages[0].core,
                pages[0].address,
                pages[0].physical,
                pages[0].size
            ),
            (0, 0x1000, 0x8000, 0x2000)
        );
        assert_eq!(
            (
                pages[1].core,
                pages[1].address,
                pages[1].physical,
                pages[1].size
            ),
            (1, 0x5000, 0x8000, 0x1000)
        );
        assert_eq!(
            (
                pages[2].core,
                pages[2].address,
                pages[2].physical,
                pages[2].size
            ),
            (2, 0x4000, 0x8000, 0x4000)
        );
        // The 4 KiB-congruent alias is not congruent within a 16 KiB page.
        assert!(peripheral_pages(&processors, vec![mapping(2, 0x5020, 0x8020, 0x20)]).is_err());
        assert!(peripheral_pages(&processors, vec![mapping(3, 0x1000, 0x8000, 0x20)]).is_err());
        assert!(peripheral_pages(&processors, vec![mapping(0, 0x1020, 0x8030, 0x20)]).is_err());
        assert!(
            peripheral_pages(
                &processors,
                vec![
                    mapping(0, 0x1000, 0x8000, 0x20),
                    mapping(0, 0x1020, 0x9020, 0x20)
                ]
            )
            .is_err()
        );
        assert!(
            peripheral_pages(&processors, vec![mapping(0, u64::MAX - 15, 0x8ff0, 0x20)]).is_err()
        );
    }

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
                data_cache: None,
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
            dram_bytes: Some(0x2000),
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
            ui_scene_names: None,
            ui_setting_names: None,
            ui_event_codes: None,
            bulk: None,
            execution: None,
            restart: None,
            card_files: None,
            settings: None,
            flash: None,
            state: None,
        };
        let graph = assemble(
            &selection,
            0x2000,
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
        let graph = assemble(&selection, selection.body.dram_bytes.unwrap(), rate, None).unwrap();
        let decoder = &graph.decoders[0];
        assert_eq!(decoder.initial, 0);
        assert_eq!(decoder.choices.len(), 2);
        assert_eq!(decoder.choices[0].processors[0].physical, 0x8000_0000);
        assert_eq!(decoder.choices[1].processors[0].physical, 0);
        assert_eq!(decoder.choices[1].dma[0].physical, 0);

        selection.soc.decoders[0].initial = selection.soc.decoders[0].choices[1].selection.clone();
        let graph = assemble(&selection, selection.body.dram_bytes.unwrap(), rate, None).unwrap();
        let decoder = &graph.decoders[0];
        assert_eq!(decoder.initial, 1);
        assert_eq!(decoder.choices.len(), 2);
        assert_eq!(decoder.choices[0].processors[0].physical, 0x8000_0000);
        assert_eq!(decoder.choices[1].processors[0].physical, 0);
    }

    #[test]
    fn xp5_bodies_map_their_own_bus_drain_on_the_declared_cores() {
        let rate = RunInstructionRate {
            numerator: 24,
            denominator: 1,
        };
        for (body, firmware, drain, limit) in [
            ("x100vi", "x100vi-131", 0x6800_0000u64, 0x6800_0000u64),
            ("xt5", "xt5-431", 0x6000_0000, 0x6000_0000),
        ] {
            let selection = fujimod_catalog::ReferenceCatalog::bundled()
                .unwrap()
                .resolve(body, firmware)
                .unwrap();
            let starts: Vec<ProcessorStart> = selection
                .soc
                .processors
                .iter()
                .map(|processor| ProcessorStart {
                    processor_id: processor.id.clone(),
                    released: processor.id == "core0",
                    registers: vec![],
                })
                .collect();
            let graph = assemble_with_starts(&selection, limit, rate, Some(1_000_000_000), &starts).unwrap();
            let other = if drain == 0x6800_0000 { 0x6000_0000 } else { 0x6800_0000 };
            let cores = |address: u64| {
                graph
                    .peripherals
                    .iter()
                    .filter(|p| (p.physical..p.physical + p.size as u64).contains(&address))
                    .count()
            };
            assert_eq!(cores(drain), 4, "{body}");
            assert_eq!(cores(other), 0, "{body}");
        }
    }

    #[test]
    fn bus_drain_names_only_present_cores() {
        let mut selection = fujimod_catalog::ReferenceCatalog::bundled()
            .unwrap()
            .resolve("xt5", "xt5-431")
            .unwrap();
        selection
            .body
            .hardware
            .as_mut()
            .unwrap()
            .bus_drain
            .as_mut()
            .unwrap()
            .processor_ids = vec!["core9".into()];
        let rate = RunInstructionRate {
            numerator: 24,
            denominator: 1,
        };
        let starts: Vec<ProcessorStart> = selection
            .soc
            .processors
            .iter()
            .map(|processor| ProcessorStart {
                processor_id: processor.id.clone(),
                released: false,
                registers: vec![],
            })
            .collect();
        let error = assemble_with_starts(&selection, 0x4000_0000, rate, Some(1_000_000_000), &starts)
            .err()
            .unwrap();
        assert!(error.contains("core9"), "{error}");
    }
}
