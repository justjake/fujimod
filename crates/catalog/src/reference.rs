//! Typed identity-based hardware and firmware reference records.
//!
//! The records contain device facts and named references. They never contain
//! another record's filename or an input asset's location.

use fujimod_catalog_identity::{validate_id, validate_sha256};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const SOC_SCHEMA_URI: &str = "https://fujimod.com/schemas/soc.schema.json";
pub const PART_SCHEMA_URI: &str = "https://fujimod.com/schemas/part.schema.json";
pub const BODY_SCHEMA_URI: &str = "https://fujimod.com/schemas/body.schema.json";
pub const FIRMWARE_SCHEMA_URI: &str = "https://fujimod.com/schemas/firmware.schema.json";

/// A processor architecture described by a system-on-chip record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ProcessorArchitecture {
    /// 32-bit ARM processor architecture.
    Arm32,
    /// 64-bit ARM processor architecture.
    Aarch64,
}

/// A named system-on-chip record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct Soc {
    /// Stable schema URI for this SoC record.
    #[serde(rename = "$schema")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "\"https://fujimod.com/schemas/soc.schema.json\"")
    )]
    pub schema_uri: String,
    /// SoC record version.
    pub schema: u32,
    /// Stable SoC identity.
    pub id: String,
    /// CPU execution architecture implemented by the SoC.
    pub architecture: ProcessorArchitecture,
    /// Processor cores and their hardware reset configuration.
    pub processors: Vec<ProcessorCore>,
    /// Addressable memory regions owned by this SoC.
    pub memory_regions: Vec<MemoryRegion>,
    /// Address and register blocks exposed by the SoC.
    pub blocks: Vec<SocBlock>,
    /// Interrupt routes between SoC blocks and processor cores.
    pub interrupts: Vec<InterruptRoute>,
    /// Clock domains exposed by the SoC.
    pub clocks: Vec<ClockDomain>,
    /// Additional family-specific hardware parameters, when specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "SocHardware | null")
    )]
    pub hardware: Option<crate::soc::SocHardware>,
    /// CPU address-space mappings used to construct a processor graph.
    pub mappings: Vec<AddressMapping>,
    /// DMA address-space mappings used to construct a processor graph.
    pub dma_mappings: Vec<DmaMapping>,
    /// Named hardware-decoder states and supported selections.
    pub decoders: Vec<AddressDecoder>,
    /// Per-core peripheral mappings into named SoC blocks.
    pub peripherals: Vec<PeripheralMapping>,
    /// Hardware reset and coprocessor setup facts.
    pub reset: SocReset,
}

impl Soc {
    /// Validate the record's schema identity and internal references.
    pub fn validate(&self, selected_id: &str) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != SOC_SCHEMA_URI || self.id != selected_id {
            return Err("SoC schema or identity differs".into());
        }
        validate_id(&self.id)?;
        let mut processors = BTreeSet::new();
        for core in &self.processors {
            validate_id(&core.id)?;
            if !processors.insert(core.id.as_str()) {
                return Err(format!("SoC {} repeats processor {}", self.id, core.id));
            }
        }
        let mut blocks = BTreeSet::new();
        for block in &self.blocks {
            validate_id(&block.id)?;
            if !blocks.insert(block.id.as_str()) {
                return Err(format!("SoC {} repeats block {}", self.id, block.id));
            }
        }
        let mut mapping_ids = BTreeSet::new();
        for mapping in &self.mappings {
            validate_id(&mapping.id)?;
            if !mapping_ids.insert(mapping.id.as_str()) {
                return Err(format!(
                    "SoC {} repeats address mapping {}",
                    self.id, mapping.id
                ));
            }
        }
        let cores: BTreeSet<_> = processors.iter().copied().collect();
        let mut dma_ids = BTreeSet::new();
        for mapping in &self.dma_mappings {
            validate_id(&mapping.id)?;
            if !dma_ids.insert(mapping.id.as_str()) {
                return Err(format!(
                    "SoC {} repeats DMA mapping {}",
                    self.id, mapping.id
                ));
            }
        }
        let mut routes = BTreeSet::new();
        for route in &self.interrupts {
            if !cores.contains(route.core.as_str()) || !blocks.contains(route.source_block.as_str())
            {
                return Err(format!(
                    "SoC {} interrupt route has an unresolved core or block",
                    self.id
                ));
            }
            if let Some(output_id) = &route.output_id {
                validate_id(output_id)?;
            }
            if !routes.insert((
                route.source_block.as_str(),
                route.output_id.as_deref(),
                route.core.as_str(),
                route.line,
            )) {
                return Err(format!(
                    "SoC {} repeats interrupt output {} line {} for core {}",
                    self.id,
                    route
                        .output_id
                        .as_deref()
                        .unwrap_or(route.source_block.as_str()),
                    route.line,
                    route.core
                ));
            }
        }
        let mut peripheral_addresses = BTreeSet::new();
        for mapping in &self.peripherals {
            if !cores.contains(mapping.core.as_str()) || !blocks.contains(mapping.block_id.as_str())
            {
                return Err(format!(
                    "SoC {} peripheral mapping has an unresolved core or block",
                    self.id
                ));
            }
            if !peripheral_addresses.insert((mapping.core.as_str(), mapping.address)) {
                return Err(format!("SoC {} repeats a peripheral address", self.id));
            }
        }
        for decoder in &self.decoders {
            validate_id(&decoder.id)?;
            let mut choice_ids = BTreeSet::new();
            for choice in &decoder.choices {
                validate_id(&choice.id)?;
                if !choice_ids.insert(choice.id.as_str()) {
                    return Err(format!(
                        "SoC {} decoder {} repeats choice {}",
                        self.id, decoder.id, choice.id
                    ));
                }
            }
            for selection in std::iter::once(&decoder.initial)
                .chain(decoder.choices.iter().map(|choice| &choice.selection))
            {
                for mapping in &selection.processors {
                    if !mapping_ids.contains(mapping.as_str()) {
                        return Err(format!(
                            "SoC {} decoder {} refers to missing address mapping {mapping}",
                            self.id, decoder.id,
                        ));
                    }
                }
                for mapping in &selection.dma_mappings {
                    if !dma_ids.contains(mapping.as_str()) {
                        return Err(format!(
                            "SoC {} decoder {} refers to missing DMA mapping {mapping}",
                            self.id, decoder.id
                        ));
                    }
                }
            }
        }
        for clock in &self.clocks {
            validate_id(&clock.id)?;
            if let Some(parent) = &clock.parent_id
                && !self.clocks.iter().any(|candidate| candidate.id == *parent)
            {
                return Err(format!("SoC {} refers to missing clock {parent}", self.id));
            }
        }
        for block in &self.blocks {
            if let Some(clock_id) = &block.clock_id
                && !self.clocks.iter().any(|clock| clock.id == *clock_id)
            {
                return Err(format!(
                    "SoC {} block {} refers to missing clock {clock_id}",
                    self.id, block.id
                ));
            }
            if let Some(core_id) = &block.processor_core_id {
                if block.kind != SocBlockKind::InterruptController
                    || !self.processors.iter().any(|core| core.id == *core_id)
                {
                    return Err(format!(
                        "SoC {} block {} has an invalid processor-core attachment",
                        self.id, block.id
                    ));
                }
            }
        }
        for mapping in &self.mappings {
            if !cores.contains(mapping.core.as_str()) {
                return Err(format!(
                    "SoC {} address mapping refers to missing core {}",
                    self.id, mapping.core
                ));
            }
        }
        for write in &self.reset.coprocessor_writes {
            if !cores.contains(write.core.as_str()) {
                return Err(format!(
                    "SoC {} reset refers to missing core {}",
                    self.id, write.core
                ));
            }
        }
        if let Some(hardware) = &self.hardware {
            hardware.validate(self)?;
        }
        Ok(())
    }
}

/// A processor core and its hardware reset state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ProcessorCore {
    /// Core identity within its SoC.
    pub id: String,
    /// Implemented processor model.
    pub model: ProcessorModel,
    /// Whether this core leaves reset when the SoC starts, when known.
    #[serde(default)]
    pub released: Option<bool>,
    /// Initial architectural register values.
    pub registers: Vec<RegisterValue>,
    /// Optional M-profile hardware state.
    #[serde(default)]
    pub mprofile: Option<MProfileState>,
    /// Optional R-profile hardware state.
    #[serde(default)]
    pub rprofile: Option<RProfileState>,
}

/// A processor core model supported by the current catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ProcessorModel {
    /// Cortex-R5F family processor.
    CortexR5f,
    /// Cortex-A7 family processor.
    CortexA7,
    /// Cortex-A53 family processor.
    CortexA53,
    /// Cortex-M4 family processor.
    CortexM4,
}

/// An initial value for an architectural register.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RegisterValue {
    /// Register selected for initialization.
    pub register: ProcessorRegister,
    /// Initial register value.
    pub value: u64,
}

/// Architectural registers explicitly initialized by a catalog record.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ProcessorRegister {
    /// Program counter.
    Pc,
    /// Stack pointer.
    Sp,
    /// Program status register.
    Pstate,
    /// Current program status register.
    Cpsr,
}

/// M-profile processor identification state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct MProfileState {
    /// CPUID register value presented by the processor.
    pub cpuid: u32,
    /// Vector-table address after reset.
    pub vector_base: u64,
}

/// R-profile processor identification and TCM state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RProfileState {
    /// CP14 processor identity register value.
    pub cp14_identity: u32,
    /// ATCM state after reset.
    pub atcm: TcmState,
    /// BTCM state after reset.
    pub btcm: TcmState,
}

/// A tightly coupled memory reset state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct TcmState {
    /// TCM capacity in bytes.
    pub bytes: u64,
    /// Address selected by the processor at reset.
    pub reset_address: u64,
}

/// A memory region provided by a SoC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct MemoryRegion {
    /// Region identity within the SoC.
    pub id: String,
    /// Region's physical base address.
    pub base: u64,
    /// Region capacity or a reference to body DRAM capacity.
    pub size: RegionSize,
    /// Hardware memory type.
    pub kind: MemoryKind,
}

/// A memory extent with no repeated body-specific capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum RegionSize {
    /// A fixed byte length owned by the SoC.
    Bytes { bytes: u64 },
    /// The selected body's declared DRAM capacity.
    BodyDram,
}

/// Memory technologies exposed by the SoC.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum MemoryKind {
    /// Dynamic random-access memory.
    Dram,
    /// Read-only boot memory.
    Rom,
    /// Static random-access memory.
    Sram,
    /// Tightly coupled processor memory.
    Tcm,
}

/// A named register or address block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SocBlock {
    /// Block identity used by bodies and run inputs.
    pub id: String,
    /// Physical base address of the block.
    pub base: u64,
    /// Addressable block size in bytes.
    pub size: u64,
    /// Block's modeled hardware category.
    pub kind: SocBlockKind,
    /// Processor core directly served by this block, for per-core interrupt controllers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "string | null")
    )]
    pub processor_core_id: Option<String>,
    /// Clock domain supplying this block, when specified.
    #[serde(default)]
    pub clock_id: Option<String>,
}

/// Hardware block categories represented in the catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum SocBlockKind {
    /// General-purpose input/output controller.
    Gpio,
    /// Analog-to-digital converter.
    Adc,
    /// Interrupt controller.
    InterruptController,
    /// Timer controller.
    Timer,
    /// Direct-memory-access controller.
    Dma,
    /// Serial control-bus controller.
    ControlBus,
    /// General-purpose serial controller.
    SerialController,
    /// Nonvolatile memory controller.
    NandController,
    /// Removable-card controller.
    SdCardController,
    /// USB controller.
    UsbController,
    /// Display controller.
    DisplayController,
    /// Imaging controller.
    ImagingController,
    /// System reset controller.
    ResetController,
    /// Clock controller.
    ClockController,
    /// Memory controller.
    MemoryController,
    /// Addressable register window with no assigned functional category.
    AddressWindow,
}

/// A routed interrupt line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct InterruptRoute {
    /// Processor core receiving the interrupt.
    pub core: String,
    /// Interrupt line number on the processor core.
    pub line: u32,
    /// SoC block producing the interrupt.
    pub source_block: String,
    /// Named output or channel within the source block, when known.
    #[serde(default)]
    pub output_id: Option<String>,
}

/// A SoC clock domain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ClockDomain {
    /// Clock-domain identity.
    pub id: String,
    /// Clock frequency in hertz, when known.
    #[serde(default)]
    pub frequency_hz: Option<u64>,
    /// Parent clock-domain identity, when present.
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// A processor address-space mapping.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct AddressMapping {
    /// Stable identity used by decoder selections.
    pub id: String,
    /// Processor core using this mapping.
    pub core: String,
    /// Address visible to the processor.
    pub address: u64,
    /// Physical target address.
    pub physical: u64,
    /// Mapping extent, including a body DRAM reference when applicable.
    pub size: RegionSize,
    /// Read, write and execute permission bits.
    pub permissions: u8,
}

/// A DMA address-space mapping.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct DmaMapping {
    /// Stable identity used by decoder selections.
    pub id: String,
    /// DMA-visible address.
    pub address: u64,
    /// Physical target address.
    pub physical: u64,
    /// Mapping extent, including a body DRAM reference when applicable.
    pub size: RegionSize,
    /// Read, write and execute permission bits.
    pub permissions: u8,
}

/// A selectable SoC address decoder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct AddressDecoder {
    /// Decoder identity.
    pub id: String,
    /// Initially selected target IDs.
    pub initial: DecoderSelection,
    /// Other supported named decoder states.
    pub choices: Vec<DecoderChoice>,
}

/// The processor and DMA routes selected by a decoder state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct DecoderSelection {
    /// Processor address mappings selected by this state.
    pub processors: Vec<String>,
    /// DMA mapping identities selected by this state.
    pub dma_mappings: Vec<String>,
}

/// A named alternate decoder state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct DecoderChoice {
    /// Choice identity selected by a hardware strap or control.
    pub id: String,
    /// Routes enabled by this choice.
    pub selection: DecoderSelection,
}

/// A per-core view into a named SoC block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct PeripheralMapping {
    /// Processor core using this peripheral window.
    pub core: String,
    /// SoC block identity exposed through this window.
    pub block_id: String,
    /// Processor-visible address.
    pub address: u64,
    /// Offset in the selected block for a subwindow, when nonzero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "number | null")
    )]
    pub offset: Option<u64>,
}

/// Hardware state applied when a SoC leaves reset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SocReset {
    /// Optional secondary-core release register.
    #[serde(default)]
    pub release: Option<CoreRelease>,
    /// Coprocessor register writes applied during reset.
    pub coprocessor_writes: Vec<CoprocessorWrite>,
}

/// A secondary-core release condition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CoreRelease {
    /// Release register address.
    pub register: u64,
    /// Register bit set when the core is released.
    pub mask: u32,
    /// Interrupt or reset source used to release the core.
    pub source: u32,
    /// Reset vector address selected on release.
    pub target: u64,
}

/// A CP15 or CP14 coprocessor register write during reset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CoprocessorWrite {
    /// Processor core receiving the write.
    pub core: String,
    /// Coprocessor selector.
    pub cp: u8,
    /// Register width selector.
    pub wide: bool,
    /// Secure-state selector.
    pub secure: u8,
    /// CRn selector.
    pub crn: u8,
    /// CRm selector.
    pub crm: u8,
    /// Opcode selector 1.
    pub opc1: u8,
    /// Opcode selector 2.
    pub opc2: u8,
    /// Bits set in the selected register.
    pub set_bits: u64,
}

/// A named camera body and its physical components.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct Body {
    /// Stable schema URI for this body record.
    #[serde(rename = "$schema")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "\"https://fujimod.com/schemas/body.schema.json\"")
    )]
    pub schema_uri: String,
    /// Body record version.
    pub schema: u32,
    /// Stable body identity.
    pub id: String,
    /// SoC identity implemented by this body.
    pub soc_id: String,
    /// Installed DRAM capacity in bytes.
    pub dram_bytes: u64,
    /// Optional body-specific physical attachments and signal routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "BodyHardware | null")
    )]
    pub hardware: Option<crate::assembly::BodyHardware>,
    /// Body-mapped RAM extents when the board exposes a non-default memory map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ram_regions: Vec<BodyRamRegion>,
    /// Optional installed NAND geometry.
    #[serde(default)]
    pub nand: Option<NandGeometry>,
    /// Optional sensor assembly and its physical characteristics.
    #[serde(default)]
    pub sensor: Option<SensorAssembly>,
    /// Optional lens assembly and its physical characteristics.
    #[serde(default)]
    pub lens: Option<LensAssembly>,
    /// Optional display assembly and its physical characteristics.
    #[serde(default)]
    pub display: Option<DisplayAssembly>,
    /// Optional MCU endpoint and its body-specific serial wiring.
    #[serde(default)]
    pub microcontroller: Option<MicrocontrollerAssembly>,
    /// Optional power-controller endpoint and its body-specific wiring.
    #[serde(default)]
    pub power_control: Option<PowerControlAssembly>,
    /// Optional removable-card slot and its body-specific contacts.
    #[serde(default)]
    pub card_slot: Option<CardSlotAssembly>,
    /// Physical control-bus devices installed in this body.
    pub control_bus_chips: Vec<ControlBusChip>,
    /// Declared physical control and peripheral connections.
    pub connections: Vec<BodyConnection>,
    /// Reusable part identities installed in this body.
    pub parts: Vec<String>,
    /// Physical links between named body connections and SoC blocks or parts.
    pub wiring: Vec<BodyWiring>,
}

impl Body {
    /// Validate the record's schema identity and component references.
    pub fn validate(&self, selected_id: &str) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != BODY_SCHEMA_URI || self.id != selected_id {
            return Err("body schema or identity differs".into());
        }
        validate_id(&self.id)?;
        validate_id(&self.soc_id)?;
        if self.dram_bytes == 0 {
            return Err(format!("body {} has no DRAM capacity", self.id));
        }
        if let Some(hardware) = &self.hardware {
            hardware.validate(self)?;
        }
        let mut region_ids = BTreeSet::new();
        let mut region_extents = Vec::new();
        let mut dram_region_bytes = 0u64;
        for region in &self.ram_regions {
            validate_id(&region.id)?;
            if region.bytes == 0
                || region.base.checked_add(region.bytes).is_none()
                || !region_ids.insert(region.id.as_str())
                || !matches!(region.kind, MemoryKind::Dram | MemoryKind::Sram)
            {
                return Err(format!("body {} has an invalid RAM region", self.id));
            }
            if region.kind == MemoryKind::Dram {
                dram_region_bytes = dram_region_bytes
                    .checked_add(region.bytes)
                    .ok_or_else(|| format!("body {} DRAM extent overflows", self.id))?;
            }
            region_extents.push((region.base, region.base + region.bytes));
        }
        region_extents.sort_unstable();
        if region_extents.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(format!("body {} RAM regions overlap", self.id));
        }
        if dram_region_bytes != 0 && dram_region_bytes != self.dram_bytes {
            return Err(format!(
                "body {} DRAM regions differ from its capacity",
                self.id
            ));
        }
        unique_ids(self.parts.iter().map(String::as_str), "body part")?;
        unique_ids(
            self.connections.iter().map(|item| item.id.as_str()),
            "body connection",
        )?;
        unique_ids(
            self.control_bus_chips.iter().map(|item| item.id.as_str()),
            "control-bus endpoint",
        )?;
        for chip in &self.control_bus_chips {
            validate_id(&chip.id)?;
            validate_id(&chip.controller_id)?;
            if chip.address > 0x7f || chip.polarity_selector > 1 {
                return Err(format!(
                    "body {} has an invalid control-bus endpoint {}",
                    self.id, chip.id
                ));
            }
        }
        for connection_id in [
            self.sensor.as_ref().map(|item| item.connection_id.as_str()),
            self.lens.as_ref().map(|item| item.connection_id.as_str()),
            self.display
                .as_ref()
                .map(|item| item.connection_id.as_str()),
            self.microcontroller
                .as_ref()
                .map(|item| item.connection_id.as_str()),
            self.power_control
                .as_ref()
                .map(|item| item.connection_id.as_str()),
            self.card_slot
                .as_ref()
                .map(|item| item.connection_id.as_str()),
        ]
        .into_iter()
        .flatten()
        {
            if !self.connections.iter().any(|item| item.id == connection_id) {
                return Err(format!(
                    "body {} component refers to missing connection {connection_id}",
                    self.id
                ));
            }
        }
        for connection_id in self
            .lens
            .as_ref()
            .into_iter()
            .flat_map(|lens| {
                lens.home_connections
                    .iter()
                    .map(|home| home.connection_id.as_str())
                    .chain(lens.encoder_connection_id.iter().map(String::as_str))
                    .chain(std::iter::once(lens.route.select_connection_id.as_str()))
            })
            .chain(
                self.display
                    .as_ref()
                    .and_then(|display| display.route.as_ref())
                    .map(|route| route.select_connection_id.as_str()),
            )
            .chain(
                self.microcontroller
                    .as_ref()
                    .map(|mcu| mcu.select_connection_id.as_str()),
            )
            .chain(
                self.power_control
                    .as_ref()
                    .map(|power| power.select_connection_id.as_str()),
            )
        {
            if !self.connections.iter().any(|item| item.id == connection_id) {
                return Err(format!(
                    "body {} wiring refers to missing connection {connection_id}",
                    self.id
                ));
            }
        }
        for link in &self.wiring {
            validate_id(&link.source_id)?;
            validate_id(&link.target_id)?;
        }
        Ok(())
    }
}

/// A body-specific mapped RAM extent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BodyRamRegion {
    /// Stable region identity within the body.
    pub id: String,
    /// Physical base address of the region.
    pub base: u64,
    /// Region capacity in bytes.
    pub bytes: u64,
    /// Memory type exposed by the body.
    pub kind: MemoryKind,
}

/// NAND geometry installed in a camera body or storage part.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NandGeometry {
    /// Total main-area capacity in bytes.
    pub bytes: u64,
    /// Main-area page size in bytes.
    pub page_bytes: u32,
    /// Pages per erase block.
    pub pages_per_block: u32,
    /// Spare-area size per page in bytes.
    pub spare_bytes: u32,
}

/// A camera sensor assembly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SensorAssembly {
    /// Reusable sensor part identity.
    pub part_id: String,
    /// Body connection identity carrying sensor control and data signals.
    pub connection_id: String,
    /// Body-specific serial control-bus endpoint, when documented.
    #[serde(default)]
    pub control_bus: Option<ControlBusAttachment>,
}

/// Control-bus route for one physical part installed in a camera body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ControlBusAttachment {
    /// Descriptor-table polarity selector, when established for the installed device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polarity_selector: Option<u32>,
    /// SoC control-bus block identity.
    pub controller_id: String,
    /// Controller channel index.
    pub channel: u8,
    /// Seven-bit endpoint address.
    pub address: u8,
}

/// Sensor image and timing geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SensorRaster {
    /// Sensor output width in pixels.
    pub width: u32,
    /// Sensor output height in pixels.
    pub height: u32,
    /// Active image area as `[x, y, width, height]`.
    pub image_area: [u32; 4],
    /// Sensor clock frequency in hertz when known.
    #[serde(default)]
    pub pixel_clock_hz: Option<u64>,
}

/// Radiometric, timing and projection facts of an image sensor. Each value is a
/// declared catalog input; its basis is recorded in the part document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SensorRadiometry {
    /// Unity-gain ADC saturation in lux-seconds.
    pub saturation_lux_seconds: f64,
    /// Declared white-pixel luminance in candela per square metre for scene input.
    pub white_luminance: f64,
    /// Ticks per second of the sensor frame clock.
    pub ticks_per_second: u64,
    /// Sensor scan period in ticks.
    pub period_ticks: u64,
    /// Output index of the sensor's shutter signal.
    pub shutter_output: u32,
    /// Leaf-shutter closing delay as `(aperture coordinate, seconds)` pairs.
    pub shutter_closing_delay_seconds: Vec<(f64, f64)>,
    /// Rolling readout references, one per readout mode.
    pub rolling_references: Vec<SensorRollingReference>,
    /// Invalid readouts while a sensor drive-mode change acquires sync.
    pub mode_change_frames: u8,
    /// Full preview readout field in sensor command-window coordinates.
    pub preview_field: Option<[u32; 2]>,
    /// Lens projection onto the sensor.
    pub projection: SensorProjection,
}

/// One rolling-readout reference of an image sensor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SensorRollingReference {
    /// Readout mode register value.
    pub mode: u8,
    /// Horizontal timing word.
    pub horizontal: u32,
    /// Vertical timing word.
    pub vertical: u32,
    /// Readout line count.
    pub lines: u32,
}

/// Lens projection onto an image sensor, as an ideal thin-lens approximation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SensorProjection {
    /// Focal length in millimetres.
    pub focal_length_mm: f64,
    /// Subject distance in millimetres.
    pub subject_distance_mm: f64,
    /// Sensor width in millimetres.
    pub sensor_width_mm: f64,
    /// Illumination falloff across the image.
    pub relative_illumination: SensorIllumination,
}

/// Image-circle illumination model of a sensor projection.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum SensorIllumination {
    /// No falloff across the image.
    Uniform,
    /// Cosine-fourth falloff across the image.
    CosineFourth,
    /// Measured radial falloff, 1 / (1 + r2 rho^2 + r4 rho^4), with rho the
    /// image-plane radius over the sensor half-diagonal.
    RadialPolynomial {
        /// Coefficient of rho^2.
        r2: f64,
        /// Coefficient of rho^4.
        r4: f64,
    },
}

/// A sensor's physical control-bus connection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SensorControlInterface {
    /// Register-address width in bits.
    pub register_address_bits: u8,
    /// Data width in bits.
    pub data_bits: u8,
}

/// A body lens assembly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct LensAssembly {
    /// Reusable lens part identity.
    pub part_id: String,
    /// Body connection identity carrying lens control signals.
    pub connection_id: String,
    /// Body-specific serial route to the lens controller.
    pub route: SerialRoute,
    /// Body-specific pulse-controller wiring for this lens.
    pub pulse: PulseInterface,
    /// Documented aperture positions and their optical output.
    #[serde(default)]
    pub iris: Option<IrisOptics>,
    /// Documented focus-axis response.
    #[serde(default)]
    pub focus: Option<FocusAxis>,
    /// Body connections wired to motor home switches.
    pub home_connections: Vec<LensHomeConnection>,
    /// Body connection wired to the optional rotary encoder.
    #[serde(default)]
    pub encoder_connection_id: Option<String>,
}

/// A body connection attached to one lens motor's home-switch input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct LensHomeConnection {
    /// Motor index within the installed lens part.
    pub motor: u8,
    /// Body connection carrying the home-switch signal.
    pub connection_id: String,
}

/// Aperture positions and their optical output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct IrisOptics {
    /// Named optical quantity produced by the aperture.
    pub output: OpticalQuantity,
    /// Aperture positions and their f-number values.
    pub positions: Vec<AperturePosition>,
    /// Minimum and maximum mechanical positions.
    pub travel_limits: [i32; 2],
}

/// An aperture mechanism position and f-number.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct AperturePosition {
    /// Mechanical position value.
    pub position: i32,
    /// Optical f-number at this position.
    pub f_number: f64,
}

/// Named optical quantities exposed by a lens assembly.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum OpticalQuantity {
    /// Lens focus position.
    LensFocus,
    /// Lens aperture value.
    LensAperture,
}

/// A focus-axis model.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FocusAxis {
    /// Named optical quantity produced by focus motion.
    pub output: OpticalQuantity,
    /// Focus position representing infinity.
    pub infinity_position: i32,
    /// Focus steps per millimeter.
    pub steps_per_mm: f64,
}

/// A body display assembly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct DisplayAssembly {
    /// Reusable panel or display-controller part identity.
    pub part_id: String,
    /// Body connection identity carrying display data.
    pub connection_id: String,
    /// Body-specific serial wiring for this panel.
    #[serde(default)]
    pub route: Option<SerialRoute>,
}

/// A body-specific attachment for a microcontroller part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct MicrocontrollerAssembly {
    /// Reusable microcontroller part identity.
    pub part_id: String,
    /// Body connection identity carrying MCU signals.
    pub connection_id: String,
    /// SoC serial controller block identity.
    pub controller_id: String,
    /// Channel selected on the controller.
    pub channel: u8,
    /// Transfer word width used by this attachment.
    pub word_bits: u8,
    /// Body connection identity controlling chip select.
    pub select_connection_id: String,
}

/// A body-specific attachment for a power-control part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct PowerControlAssembly {
    /// Reusable power-control part identity.
    pub part_id: String,
    /// Body connection identity carrying power-control signals.
    pub connection_id: String,
    /// SoC serial controller block identity.
    pub controller_id: String,
    /// Channel selected on the controller.
    pub channel: u8,
    /// Body connection controlling chip select.
    pub select_connection_id: String,
}

/// A body-specific attachment for a removable-card slot part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CardSlotAssembly {
    /// Reusable card-slot part identity.
    pub part_id: String,
    /// Body connection identity for the card data interface.
    pub connection_id: String,
    /// Body connection identity carrying card-detect state.
    pub detect_connection_id: String,
    /// Body connection identity carrying write-protect state.
    pub write_protect_connection_id: String,
    /// SoC removable-card controller block, when implemented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "string | null")
    )]
    pub controller_id: Option<String>,
}

/// Display orientation relative to the camera body.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum DisplayOrientation {
    /// Fixed landscape panel.
    Landscape,
    /// Fixed portrait panel.
    Portrait,
    /// Panel that can tilt relative to the body.
    Tilting,
}

/// A physical body connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BodyConnection {
    /// Connection identity referenced by body wiring and run inputs.
    pub id: String,
    /// Physical connection category.
    pub kind: ConnectionKind,
    /// Electrical or physical signals carried by the connection.
    pub signals: Vec<ConnectionSignal>,
}

/// Physical connection categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ConnectionKind {
    /// User button or control switch.
    Control,
    /// USB cable or host connection.
    Usb,
    /// HDMI cable or display connection.
    Hdmi,
    /// SD card slot.
    SdCard,
    /// Wired remote or microphone connector.
    Remote,
    /// Hot-shoe accessory connector.
    HotShoe,
    /// External power connector.
    ExternalPower,
    /// Battery-door interlock.
    BatteryDoor,
    /// Sensor data or control connection.
    Sensor,
    /// Lens or actuator connection.
    Lens,
    /// Display data or control connection.
    Display,
    /// Microcontroller connection.
    Microcontroller,
    /// Serial power-control endpoint.
    PowerControl,
}

/// One electrical signal exposed by a body connection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ConnectionSignal {
    /// A GPIO pin with an active logic level.
    Gpio {
        port: u8,
        pin: u8,
        active_level: bool,
    },
    /// An ADC channel and threshold.
    Adc {
        channel: u8,
        #[serde(default)]
        threshold: Option<u16>,
    },
    /// A named SoC block signal.
    Block {
        block_id: String,
        signal: BlockSignal,
    },
    /// A routed serial control channel.
    Serial {
        block_id: String,
        channel: u8,
        address: u8,
    },
    /// Direct interrupt wire to a processor interrupt-controller input.
    Interrupt { controller_id: String, line: u32 },
    /// Contact reported as bits of a register read through a serial controller.
    SerialStatus {
        block_id: String,
        register: u8,
        mask: u8,
    },
}

/// Named control signals carried by SoC blocks.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum BlockSignal {
    /// Presence or cable-detect signal.
    Detect,
    /// Write-protect signal.
    WriteProtect,
    /// Data ready signal.
    DataReady,
    /// Clock output.
    Clock,
}

/// A physical wire between a body connection, part, or SoC block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BodyWiring {
    /// Source component identity.
    pub source_id: String,
    /// Target component identity.
    pub target_id: String,
    /// Named signal carried by the wire.
    pub signal: WireSignal,
}

/// Named physical wire types.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum WireSignal {
    /// GPIO line.
    Gpio,
    /// ADC input.
    Adc,
    /// Serial control data.
    SerialData,
    /// Serial control clock.
    SerialClock,
    /// Sensor image data.
    ImageData,
    /// Display output data.
    DisplayData,
    /// MCU command and response.
    Microcontroller,
    /// Memory card data and control.
    Storage,
    /// Pulse-counter or timer input.
    Pulse,
    /// Power or reset state.
    Power,
}

/// A physical control-bus device installed in a camera body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ControlBusChip {
    /// Stable endpoint identity within this body.
    pub id: String,
    /// SoC control-bus block identity carrying this device.
    pub controller_id: String,
    /// Control-bus channel index.
    pub channel: u8,
    /// Seven-bit device address.
    pub address: u8,
    /// Polarity selector for this device.
    pub polarity_selector: u8,
}

/// A reusable physical or protocol component referenced by a body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct Part {
    /// Stable schema URI for this part record.
    #[serde(rename = "$schema")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "\"https://fujimod.com/schemas/part.schema.json\"")
    )]
    pub schema_uri: String,
    /// Part record version.
    pub schema: u32,
    /// Stable reusable part identity.
    pub id: String,
    /// Typed physical or protocol model for this part.
    pub model: PartModel,
    /// Declared board attachment for this reusable part assembly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "PartAttachment | null")
    )]
    pub attachment: Option<crate::parts::PartAttachment>,
}

impl Part {
    /// Validate the record's schema identity.
    pub fn validate(&self, selected_id: &str) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != PART_SCHEMA_URI || self.id != selected_id {
            return Err("part schema or identity differs".into());
        }
        validate_id(&self.id)?;
        if let Some(attachment) = &self.attachment {
            attachment.validate()?;
        }
        if let PartModel::LensMechanism {
            motors,
            motor_counters,
            ..
        } = &self.model
        {
            let mut assignments = BTreeSet::new();
            if motor_counters.iter().any(|counter| {
                usize::from(counter.motor) >= motors.len()
                    || counter.divider == 0
                    || !assignments.insert((counter.motor, counter.channel))
            }) {
                return Err(format!(
                    "lens part {} has an invalid counter assignment",
                    self.id
                ));
            }
        }
        if let PartModel::Microcontroller {
            registers,
            packet_registers,
            clock_registers,
            status_register,
            initial_registers,
        } = &self.model
        {
            if registers.count == 0
                || registers.width_bytes != 1
                || packet_registers.count != 16
                || packet_registers.width_bytes != 1
                || clock_registers.count != 4
                || clock_registers.width_bytes != 1
            {
                return Err(format!(
                    "MCU part {} has an invalid register layout",
                    self.id
                ));
            }
            let in_range = |range: &RegisterRange, address: u32| {
                address >= range.first && address < range.first.saturating_add(range.count)
            };
            let window_in_range = |window: &RegisterRange| {
                window.first >= registers.first
                    && window.first.saturating_add(window.count)
                        <= registers.first.saturating_add(registers.count)
            };
            let intersects = |left: &RegisterRange, right: &RegisterRange| {
                left.first < right.first.saturating_add(right.count)
                    && right.first < left.first.saturating_add(left.count)
            };
            if !window_in_range(packet_registers)
                || !window_in_range(clock_registers)
                || intersects(packet_registers, clock_registers)
            {
                return Err(format!(
                    "MCU part {} has overlapping register windows",
                    self.id
                ));
            }
            let mut initial = BTreeSet::new();
            for address in initial_registers {
                if !in_range(registers, *address)
                    || in_range(packet_registers, *address)
                    || in_range(clock_registers, *address)
                    || *status_register == Some(*address)
                    || !initial.insert(address)
                {
                    return Err(format!(
                        "MCU part {} has an invalid auxiliary register address",
                        self.id
                    ));
                }
            }
            if status_register.is_some_and(|address| {
                !in_range(registers, address)
                    || in_range(packet_registers, address)
                    || in_range(clock_registers, address)
            }) {
                return Err(format!(
                    "MCU part {} has an invalid status register address",
                    self.id
                ));
            }
        }
        Ok(())
    }
}

/// A closed set of reusable physical part models.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum PartModel {
    /// A serially controlled optical lens mechanism.
    LensMechanism {
        /// Lens protocol and electrical timing.
        protocol: LensProtocol,
        /// Protocol direction polarity, when declared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction_high: Option<bool>,
        /// Protocol behavior when a command requests a stop.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_response: Option<LensStopResponse>,
        /// Protocol-specific clock modes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clock_modes: Option<Vec<LensClockMode>>,
        /// Protocol period in guest ticks, when specified.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        period_ticks: Option<u64>,
        /// Pulse-counter channels assigned to each lens motor by the part protocol.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        motor_counters: Vec<LensMotorCounter>,
        /// Lens motors and their travel/response mechanics.
        motors: Vec<LensMotor>,
        /// Optional physical encoder input.
        #[serde(default)]
        encoder: Option<PartEncoder>,
        /// Optional zoom mechanism.
        #[serde(default)]
        zoom: Option<ZoomMechanism>,
    },
    /// A multi-pin contact actuator.
    ContactActuator {
        /// Supported drive values and resulting contact state.
        positions: Vec<ContactPosition>,
        /// Time required for a contact transition to settle.
        settle_ticks: u64,
    },
    /// A serial output endpoint.
    SerialOutput {
        /// Serial output endpoint category.
        endpoint: SerialOutputKind,
    },
    /// A microcontroller packet endpoint.
    Microcontroller {
        /// Register address range used by the endpoint.
        registers: RegisterRange,
        /// Register window populated by the firmware command packet.
        packet_registers: RegisterRange,
        /// Register window containing the clock counter.
        clock_registers: RegisterRange,
        /// Status register exposed by the endpoint, when present.
        #[serde(default)]
        status_register: Option<u32>,
        /// Other explicitly initialized register addresses accepted by a run.
        initial_registers: Vec<u32>,
    },
    /// A byte-addressed register bank carried in sixteen-bit SPI words.
    SerialRegisterBank {
        /// Register addresses admitted by this protocol bank.
        registers: Vec<u8>,
        /// Whether read commands are supported by this bank.
        readable: bool,
    },
    /// A power-control endpoint with named startup inputs.
    PowerControl {
        /// SPI status registers exposed by the power controller.
        status_registers: Vec<StatusRegister>,
    },
    /// An SD card slot and its physical contacts.
    SdCardSlot,
    /// An image sensor protocol and pixel-array component.
    ImageSensor {
        /// Sensor raster geometry.
        raster: SensorRaster,
        /// Sensor register and control-bus interface.
        control: SensorControlInterface,
        /// Color-filter array pattern.
        color_filter: Vec<u8>,
        /// Black-level value.
        black_level: u32,
        /// Full-scale value.
        full_scale: u32,
        /// Per-channel reference-white values.
        reference_white: Vec<u32>,
        /// Radiometric, timing and projection facts; absent for a sensor without a declared exposure model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(
            all(target_arch = "wasm32", not(target_os = "emscripten")),
            tsify(type = "SensorRadiometry | null")
        )]
        radiometry: Option<SensorRadiometry>,
    },
    /// A display panel endpoint.
    DisplayPanel {
        /// Display raster width in pixels.
        #[serde(default)]
        width: Option<u32>,
        /// Display raster height in pixels.
        #[serde(default)]
        height: Option<u32>,
        /// Display orientation relative to the body.
        #[serde(default)]
        orientation: Option<DisplayOrientation>,
        /// Display endpoint serial protocol.
        serial: SerialOutputKind,
        /// Number of panel register bits used by the endpoint.
        used_mask: u32,
    },
    /// A reusable serial or register peripheral endpoint.
    Peripheral {
        /// Closed protocol model for the endpoint.
        interface: crate::assembly::PeripheralPart,
    },
}

/// A supported lens serial protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum LensProtocol {
    /// Banked lens protocol.
    Banked,
    /// Three-byte lens protocol.
    ThreeByte,
    /// Mixed-width lens protocol.
    MixedWidth,
    /// Direct-current zoom lens protocol.
    DcZoom,
}

/// Lens command response after a stop request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LensStopResponse {
    /// Report stopped only after the current movement settles.
    Settled,
}

/// A clocking mode declared by a lens protocol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
pub enum LensClockMode {
    /// Use a fixed clock selected by the body attachment.
    Fixed,
    /// Clock phases advance through a divided frequency table.
    Divided {
        /// Frequency used to derive the periods.
        ticks_per_second: u64,
        /// Clock periods in picoseconds.
        period_picoseconds: Vec<u64>,
    },
    /// Clock is advanced for each microstep stage.
    MicrostepStages {
        /// Guest ticks per stage quantum.
        ticks_per_quantum: u64,
    },
    /// Clock is advanced for each encoder stage.
    EncoderStages {
        /// Guest ticks per stage quantum.
        ticks_per_quantum: u64,
    },
}

/// A pulse interface required by an optical mechanism.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct PulseInterface {
    /// Counter channels sampled by the mechanism.
    pub counter_channels: Vec<u8>,
    /// Timer channels sampled by the mechanism.
    pub timer_channels: Vec<u8>,
    /// Whether quadrature signals are active.
    pub quadrature: bool,
    /// Initial pulse phase.
    pub initial_phase: u32,
    /// Timer capture channel assigned to this lens, when documented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_channel: Option<u8>,
}

/// One lens motor and its physical response limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct LensMotor {
    /// Motor's initial position.
    pub initial_position: i32,
    /// Minimum and maximum travel positions.
    pub travel_limits: [i32; 2],
    /// Guest ticks per motor step.
    pub ticks_per_step: u64,
    /// Settling time after motion stops.
    pub settle_ticks: u64,
    /// Optional home-switch pin and threshold.
    #[serde(default)]
    pub home: Option<HomeSwitch>,
}

/// A lens home-switch input.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct HomeSwitch {
    /// Position threshold used to detect home.
    pub threshold: i32,
    /// Side of the position threshold that asserts home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<HomeSwitchSide>,
}

/// Position comparison used by a lens home switch.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum HomeSwitchSide {
    /// Home is asserted above the threshold.
    Above,
    /// Home is asserted below the threshold.
    Below,
}

/// A motor's pulse-counter channel and divider.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LensMotorCounter {
    /// Motor index within the part.
    pub motor: u8,
    /// Counter channel.
    pub channel: u8,
    /// Pulses per counter transition.
    pub divider: u8,
}

/// An optional encoder attached to a physical part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct PartEncoder {
    /// Guest ticks per encoder phase transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_ticks: Option<u64>,
    /// Initial encoder phase.
    pub rest_phase: u8,
    /// Encoder edges per phase in the declared protocol, when specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps_per_phase: Option<u32>,
}

/// A zoom mechanism's travel and pulse settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ZoomMechanism {
    /// Zoom mechanism's initial position.
    pub initial_position: i32,
    /// Minimum zoom position.
    pub minimum_position: i32,
    /// Maximum zoom position.
    pub maximum_position: i32,
    /// Guest ticks per zoom pulse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulse_ticks: Option<u64>,
    /// Counter-to-position scale declared for the zoom mechanism.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_scale: Option<u32>,
    /// Zoom home-switch threshold and polarity, when specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<HomeSwitch>,
}

/// A pin drive position and its resulting contact states.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ContactPosition {
    /// Drive value applied to the actuator.
    pub drive: u32,
    /// Resulting contact states.
    pub contacts: Vec<bool>,
}

/// A serial bus route to a physical endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SerialRoute {
    /// SoC serial controller block identity.
    pub controller_id: String,
    /// Controller channel identity.
    pub channel: u8,
    /// Endpoint identity on the serial bus.
    pub endpoint_id: String,
    /// Body connection driving chip select.
    pub select_connection_id: String,
    /// Serial clock frequency in hertz, when known.
    #[serde(default)]
    pub clock_hz: Option<u64>,
    /// Word width in bits.
    pub word_bits: u8,
}

/// Serial output endpoint categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum SerialOutputKind {
    /// Display panel register endpoint.
    Panel,
    /// Control-bus register endpoint.
    Registers,
    /// Command and data serial endpoint.
    CommandData,
    /// Fixed serial register endpoint.
    Fixed,
}

/// A range of registers exposed by a physical component.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RegisterRange {
    /// First register address.
    pub first: u32,
    /// Number of consecutive registers.
    pub count: u32,
    /// Register width in bytes.
    pub width_bytes: u8,
}

/// An SPI status register initialized by a physical control part.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct StatusRegister {
    /// Register address.
    pub address: u8,
    /// Initial status byte.
    pub value: u8,
}

/// A firmware content identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ContentIdentity {
    /// Content size in bytes.
    pub size: u64,
    /// Lowercase SHA-256 digest.
    pub sha256: String,
}

impl ContentIdentity {
    /// Validate content size and lowercase SHA-256 syntax.
    pub fn validate(&self) -> Result<(), String> {
        if self.size == 0 || validate_sha256(&self.sha256).is_err() {
            return Err("content identity requires a positive size and lowercase SHA-256".into());
        }
        Ok(())
    }
}

/// A named firmware version and the content identities needed to run it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct Firmware {
    /// Stable schema URI for this firmware record.
    #[serde(rename = "$schema")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "\"https://fujimod.com/schemas/firmware.schema.json\"")
    )]
    pub schema_uri: String,
    /// Firmware record version.
    pub schema: u32,
    /// Stable firmware identity.
    pub id: String,
    /// Body identity that carries this firmware.
    pub body_id: String,
    /// Human-readable firmware version string.
    pub version: String,
    /// Firmware update content, when distributed for this version.
    #[serde(default)]
    pub update: Option<ContentIdentity>,
    /// Boot ROM content and its physical load address, when applicable.
    #[serde(default)]
    pub boot_rom: Option<BootRomIdentity>,
    /// Firmware installation and loader facts consumed by the emulator.
    #[serde(default)]
    pub installation: Option<FirmwareInstallation>,
    /// Generated observation facts consumed by supported runs.
    #[serde(default)]
    pub generated_facts: Option<FirmwareGeneratedFacts>,
    /// Located boot, device and persistent-storage layout facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "FirmwareLayouts | null")
    )]
    pub layouts: Option<crate::firmware::FirmwareLayouts>,
}

impl Firmware {
    /// Validate the record's schema identity and content digests.
    pub fn validate(&self, selected_id: &str) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != FIRMWARE_SCHEMA_URI || self.id != selected_id {
            return Err("firmware schema or identity differs".into());
        }
        validate_id(&self.id)?;
        validate_id(&self.body_id)?;
        if let Some(content) = &self.update {
            content.validate()?;
        }
        if let Some(boot_rom) = &self.boot_rom {
            boot_rom.content.validate()?;
        }
        if let Some(installation) = &self.installation {
            let mut section_names = BTreeSet::new();
            for section in &installation.image_sections {
                validate_id(&section.name)?;
                if section.size == 0
                    || section.address.checked_add(section.size).is_none()
                    || validate_sha256(&section.sha256).is_err()
                    || !section_names.insert(section.name.as_str())
                {
                    return Err(format!(
                        "firmware {} has an invalid or repeated mapped section",
                        self.id
                    ));
                }
            }
            if let Some(loader) = &installation.loader_validation {
                if loader.sections.is_empty()
                    || loader
                        .sections
                        .iter()
                        .any(|name| !section_names.contains(name.as_str()))
                {
                    return Err(format!(
                        "firmware {} loader validation refers to missing sections",
                        self.id
                    ));
                }
            }
            for (address, label) in &installation.entry_labels {
                let address = u64::from_str_radix(address.trim_start_matches("0x"), 16)
                    .map_err(|_| format!("firmware {} has an invalid entry address", self.id))?;
                if address == 0 || label.trim().is_empty() {
                    return Err(format!(
                        "firmware {} has an empty entry label or address",
                        self.id
                    ));
                }
            }
        }
        if let Some(facts) = &self.generated_facts {
            if let Some(remote) = &facts.remote {
                if remote
                    .get("firmware_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(&self.id)
                    || remote.get("body_id").and_then(serde_json::Value::as_str)
                        != Some(&self.body_id)
                    || remote
                        .get("firmware_sha256")
                        .and_then(serde_json::Value::as_str)
                        != self.update.as_ref().map(|content| content.sha256.as_str())
                {
                    return Err("remote facts differ from firmware identity".into());
                }
            }
        }
        Ok(())
    }
}

/// A verified boot ROM image and its load address.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BootRomIdentity {
    /// Boot ROM content identity.
    pub content: ContentIdentity,
    /// Address where the ROM image is loaded.
    pub address: u64,
}

/// The flash layout used to install a firmware update.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareInstallation {
    /// Offset from the update input to the flash image.
    pub flash_offset: u64,
    /// Named executable entry addresses consumed by firmware tooling.
    pub entry_labels: BTreeMap<String, String>,
    /// Loader validation entry and the mapped sections required by the loader.
    #[serde(default)]
    pub loader_validation: Option<FirmwareLoaderValidation>,
    /// Named sections in the installed firmware image.
    pub image_sections: Vec<FirmwareImageSection>,
}

/// A firmware loader entry and the named sections it validates.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareLoaderValidation {
    /// Firmware entry address validated by the loader.
    pub entry: u64,
    /// Mapped section names required by the validation.
    pub sections: Vec<String>,
}

/// A mapped firmware image section identified by address and content digest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareImageSection {
    /// Stable section name.
    pub name: String,
    /// Address where the section is mapped.
    pub address: u64,
    /// Mapped section size in bytes.
    pub size: u64,
    /// SHA-256 of the mapped section.
    pub sha256: String,
    /// Native flash offset, when identified.
    #[serde(default)]
    pub native_flash_offset: Option<u64>,
}

/// Generated firmware facts that inform state observation and initial setup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareGeneratedFacts {
    /// Layouts and declarations used by remote camera control.
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<fujimod_catalog_remote::FirmwareFacts>")
    )]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "unknown")
    )]
    pub remote: Option<serde_json::Value>,
    /// Firmware configuration-block location.
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<fujimod_catalog_controls::facts::ConfigurationLocation>")
    )]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "unknown")
    )]
    pub cfgdata_location: Option<serde_json::Value>,
    /// Additional playback UI mode values.
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<fujimod_catalog_controls::facts::AdditionalUiModes>")
    )]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "unknown")
    )]
    pub additional_ui_modes: Option<serde_json::Value>,
    /// Firmware menu-cursor structure base address.
    #[serde(default)]
    pub menu_cursor: Option<u64>,
    /// Logical keys used by menu controls.
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<fujimod_catalog_controls::facts::MenuKeys>")
    )]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "unknown")
    )]
    pub menu_keys: Option<serde_json::Value>,
    /// Uncached alias used to access RAM addresses.
    #[serde(default)]
    pub uncached_alias: Option<u64>,
    /// Hash-identified firmware address ranges used for identity checks.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Vec<fujimod_catalog_controls::facts::AddressIdentity>")
    )]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "unknown[]")
    )]
    pub identity_checks: Vec<serde_json::Value>,
    /// Addresses and values used to observe firmware readiness.
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<fujimod_catalog_remote::FirmwareObservation>")
    )]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "unknown")
    )]
    pub observation: Option<serde_json::Value>,
}

/// All resolved reference records for one body and firmware selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ResolvedSelection {
    /// Resolved SoC record selected by the body.
    pub soc: Soc,
    /// Resolved body record.
    pub body: Body,
    /// Resolved firmware record.
    pub firmware: Firmware,
    /// Resolved reusable parts referenced by the body.
    pub parts: Vec<Part>,
    /// The body's hand-authored physical controls, when documented.
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<fujimod_catalog_controls::PhysicalControls>")
    )]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "PhysicalControls | null")
    )]
    pub physical_controls: Option<serde_json::Value>,
    /// The firmware's generated located inputs, when generated.
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<fujimod_catalog_controls::FirmwareControlIds>")
    )]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "unknown")
    )]
    pub control_ids: Option<serde_json::Value>,
    /// Stock first-start menus, when located for this firmware.
    #[serde(default)]
    pub first_start: Option<crate::first_start::FirstStartMenu>,
    /// Stock diagnostic bulk reads, when located for this firmware.
    #[serde(default)]
    pub bulk: Option<fujimod_catalog_bulk::FirmwareBulk>,
    /// Settings-bank layout, when located for this firmware.
    #[serde(default)]
    pub settings: Option<fujimod_catalog_settings::FirmwareSettings>,
    /// Stock flash-read contract, when located for this firmware.
    #[serde(default)]
    pub flash: Option<crate::capture::FirmwareFlash>,
    /// Safely readable state regions, when established for this selection.
    #[serde(default)]
    pub state: Option<crate::capture::FirmwareState>,
}

/// A checked, ID-indexed catalog of processor, part, body and firmware facts.
#[derive(Clone, Debug, Default)]
pub struct ReferenceCatalog {
    firmware_versions: Vec<crate::firmware_versions::FirmwareVersion>,
    socs: BTreeMap<String, Soc>,
    parts: BTreeMap<String, Part>,
    bodies: BTreeMap<String, Body>,
    firmwares: BTreeMap<String, Firmware>,
    /// Hand-authored physical controls, keyed by body identity.
    physical_controls: BTreeMap<String, serde_json::Value>,
    /// Generated located inputs, keyed by firmware identity.
    control_ids: BTreeMap<String, serde_json::Value>,
    first_start: BTreeMap<String, crate::first_start::FirstStartMenu>,
    bulk: BTreeMap<String, fujimod_catalog_bulk::FirmwareBulk>,
    settings: BTreeMap<String, fujimod_catalog_settings::FirmwareSettings>,
    flash: BTreeMap<String, crate::capture::FirmwareFlash>,
    state: BTreeMap<String, crate::capture::FirmwareState>,
}

impl ReferenceCatalog {
    /// Load and validate reference records from a catalog root directory.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root.as_ref();
        let mut catalog = Self::default();
        let versions = root.join("firmware-versions.generated.json");
        if versions.exists() {
            catalog.firmware_versions = crate::firmware_versions::FirmwareVersions::decode(
                &fs::read(&versions).map_err(|e| e.to_string())?,
            )?
            .releases;
        }
        for (path, bytes) in read_json_files(&root.join("soc"))? {
            let value: Soc = decode(&bytes, &path)?;
            value.validate(&value.id)?;
            insert_unique(&mut catalog.socs, value.id.clone(), value, &path)?;
        }
        for (path, bytes) in read_json_files(&root.join("parts"))? {
            let value: Part = decode(&bytes, &path)?;
            value.validate(&value.id)?;
            insert_unique(&mut catalog.parts, value.id.clone(), value, &path)?;
        }
        for (path, bytes) in read_json_files(&root.join("bodies"))? {
            catalog.insert_document(ReferenceKind::of_body_file(&path)?, &bytes, &path)?;
        }
        for (path, bytes) in read_json_files(&root.join("firmware"))? {
            catalog.insert_document(ReferenceKind::of_firmware_file(&path)?, &bytes, &path)?;
        }
        catalog.validate_references()?;
        Ok(catalog)
    }

    /// Load the normalized reference catalog compiled into this library.
    pub fn bundled() -> Result<Self, String> {
        let mut catalog = Self::default();
        catalog.firmware_versions = crate::firmware_versions::FirmwareVersions::bundled()?.releases;
        for (kind, path, bytes) in bundled_documents() {
            match kind {
                ReferenceKind::Soc => {
                    let value: Soc = decode(bytes.as_bytes(), Path::new(path))?;
                    value.validate(&value.id)?;
                    insert_unique(&mut catalog.socs, value.id.clone(), value, Path::new(path))?;
                }
                ReferenceKind::Part => {
                    let value: Part = decode(bytes.as_bytes(), Path::new(path))?;
                    value.validate(&value.id)?;
                    insert_unique(&mut catalog.parts, value.id.clone(), value, Path::new(path))?;
                }
                kind => catalog.insert_document(kind, bytes.as_bytes(), Path::new(path))?,
            }
        }
        catalog.validate_references()?;
        Ok(catalog)
    }

    /// Resolve a body and an explicitly selected firmware by identity.
    pub fn resolve(&self, body_id: &str, firmware_id: &str) -> Result<ResolvedSelection, String> {
        let body = self
            .bodies
            .get(body_id)
            .ok_or_else(|| format!("unknown body identity {body_id}"))?
            .clone();
        let firmware = self
            .firmwares
            .get(firmware_id)
            .ok_or_else(|| format!("unknown firmware identity {firmware_id}"))?
            .clone();
        if firmware.body_id != body.id {
            return Err(format!(
                "firmware {} belongs to body {}, not {}",
                firmware.id, firmware.body_id, body.id
            ));
        }
        let soc = self
            .socs
            .get(&body.soc_id)
            .ok_or_else(|| format!("body {} refers to missing SoC {}", body.id, body.soc_id))?
            .clone();
        let parts = body
            .parts
            .iter()
            .map(|id| {
                self.parts
                    .get(id)
                    .cloned()
                    .ok_or_else(|| format!("body {} refers to missing part {id}", body.id))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ResolvedSelection {
            physical_controls: self.physical_controls.get(&body.id).cloned(),
            control_ids: self.control_ids.get(&firmware.id).cloned(),
            first_start: self.first_start.get(&firmware.id).cloned(),
            bulk: self.bulk.get(&firmware.id).cloned(),
            settings: self.settings.get(&firmware.id).cloned(),
            flash: self.flash.get(&firmware.id).cloned(),
            state: self.state.get(&firmware.id).cloned(),
            soc,
            body,
            firmware,
            parts,
        })
    }

    /// Resolve firmware and derive its body and SoC identities.
    pub fn resolve_firmware(&self, firmware_id: &str) -> Result<ResolvedSelection, String> {
        let firmware = self
            .firmwares
            .get(firmware_id)
            .ok_or_else(|| format!("unknown firmware identity {firmware_id}"))?;
        self.resolve(&firmware.body_id, firmware_id)
    }

    /// Return a resolved SoC by its stable identity.
    pub fn soc(&self, id: &str) -> Option<&Soc> {
        self.socs.get(id)
    }

    /// Return a resolved body by its stable identity.
    pub fn body(&self, id: &str) -> Option<&Body> {
        self.bodies.get(id)
    }

    /// Return a resolved firmware by its stable identity.
    pub fn firmware(&self, id: &str) -> Option<&Firmware> {
        self.firmwares.get(id)
    }

    /// Every firmware identity, in identity order.
    pub fn firmware_ids(&self) -> impl Iterator<Item = &str> {
        self.firmwares.keys().map(String::as_str)
    }

    /// Public/internal release identities, including cameras without execution profiles.
    pub fn firmware_versions(&self) -> &[crate::firmware_versions::FirmwareVersion] {
        &self.firmware_versions
    }

    /// Located bulk profiles, independent of a body or execution profile.
    pub fn bulk_profiles(&self) -> impl Iterator<Item = &fujimod_catalog_bulk::FirmwareBulk> {
        self.bulk.values()
    }

    /// Return the bulk profile for a firmware release.
    pub fn bulk(&self, firmware_id: &str) -> Option<&fujimod_catalog_bulk::FirmwareBulk> {
        self.bulk.get(firmware_id)
    }

    fn insert_document(
        &mut self,
        kind: ReferenceKind,
        bytes: &[u8],
        path: &Path,
    ) -> Result<(), String> {
        match kind {
            ReferenceKind::FirstStart => {
                let value: crate::first_start::FirstStartMenu = decode(bytes, path)?;
                value.validate()?;
                insert_unique(
                    &mut self.first_start,
                    value.firmware_id.clone(),
                    value,
                    path,
                )
            }
            ReferenceKind::Flash => {
                let value: crate::capture::FirmwareFlash = decode(bytes, path)?;
                value.validate()?;
                insert_unique(&mut self.flash, value.firmware_id.clone(), value, path)
            }
            ReferenceKind::State => {
                let value: crate::capture::FirmwareState = decode(bytes, path)?;
                value.validate()?;
                insert_unique(&mut self.state, value.firmware_id.clone(), value, path)
            }
            ReferenceKind::Settings => {
                let value: fujimod_catalog_settings::FirmwareSettings = decode(bytes, path)?;
                value.validate()?;
                insert_unique(&mut self.settings, value.firmware_id.clone(), value, path)
            }
            ReferenceKind::Bulk => {
                let value: fujimod_catalog_bulk::FirmwareBulk = decode(bytes, path)?;
                value.validate()?;
                insert_unique(&mut self.bulk, value.firmware_id.clone(), value, path)
            }
            ReferenceKind::Body => {
                let value: Body = decode(bytes, path)?;
                value.validate(&value.id)?;
                insert_unique(&mut self.bodies, value.id.clone(), value, path)
            }
            ReferenceKind::Firmware => {
                let value: Firmware = decode(bytes, path)?;
                value.validate(&value.id)?;
                insert_unique(&mut self.firmwares, value.id.clone(), value, path)
            }
            ReferenceKind::PhysicalControls => {
                let value: serde_json::Value = decode(bytes, path)?;
                let body_id = document_string(&value, "body_id", path)?;
                insert_unique(&mut self.physical_controls, body_id, value, path)
            }
            ReferenceKind::ControlIds => {
                let value: serde_json::Value = decode(bytes, path)?;
                let firmware_id = document_string(&value, "firmware_id", path)?;
                insert_unique(&mut self.control_ids, firmware_id, value, path)
            }
            ReferenceKind::Soc | ReferenceKind::Part => Err(format!(
                "{} is not a body or firmware document",
                path.display()
            )),
        }
    }

    fn validate_controls(&self) -> Result<(), String> {
        for firmware_id in self.first_start.keys() {
            if !self.firmwares.contains_key(firmware_id) {
                return Err(format!(
                    "first-start menu refers to missing firmware {firmware_id}"
                ));
            }
        }
        for firmware_id in self.settings.keys() {
            if !self.firmwares.contains_key(firmware_id) {
                return Err(format!(
                    "settings layout refers to missing firmware {firmware_id}"
                ));
            }
        }
        for firmware_id in self.bulk.keys() {
            if !self.firmwares.contains_key(firmware_id)
                && !self
                    .firmware_versions
                    .iter()
                    .any(|release| &release.firmware_id == firmware_id)
            {
                return Err(format!(
                    "bulk facts refer to missing firmware {firmware_id}"
                ));
            }
        }
        for (body_id, controls) in &self.physical_controls {
            let needs_body = controls
                .get("controls")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|items| {
                    items.values().any(|control| {
                        control
                            .get("connection_ids")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|ids| !ids.is_empty())
                    })
                })
                || contains_input_binding(controls);
            if !self.bodies.contains_key(body_id) && needs_body {
                let control_id = controls
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                return Err(format!(
                    "physical controls {} require body {body_id} to validate their bindings or connections",
                    control_id
                ));
            }
        }
        for (firmware_id, ids) in &self.control_ids {
            let firmware = self.firmwares.get(firmware_id).ok_or_else(|| {
                format!(
                    "control IDs {} refer to missing firmware {firmware_id}",
                    ids.get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown")
                )
            })?;
            if !self.physical_controls.contains_key(&firmware.body_id) {
                return Err(format!(
                    "control IDs for firmware {firmware_id} have no physical controls for body {}",
                    firmware.body_id
                ));
            }
        }
        Ok(())
    }

    /// Return a resolved reusable part by its stable identity.
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.parts.get(id)
    }

    fn validate_references(&self) -> Result<(), String> {
        for firmware_id in self.flash.keys().chain(self.state.keys()) {
            if !self.firmwares.contains_key(firmware_id) {
                return Err(format!(
                    "capture facts refer to missing firmware {firmware_id}"
                ));
            }
        }
        for release in &self.firmware_versions {
            if let Some(firmware) = self.firmwares.get(&release.firmware_id) {
                if release.version != firmware.version
                    || firmware
                        .update
                        .as_ref()
                        .is_none_or(|update| update.sha256 != release.update_sha256)
                {
                    return Err(format!(
                        "firmware version map differs for {}",
                        release.firmware_id
                    ));
                }
            }
            if let Some(bulk) = self.bulk.get(&release.firmware_id) {
                if release.product != bulk.product
                    || release.internal_version != bulk.internal_version
                {
                    return Err(format!(
                        "bulk identity differs from firmware version map for {}",
                        release.firmware_id
                    ));
                }
            }
        }
        self.validate_controls()?;
        for body in self.bodies.values() {
            if !self.socs.contains_key(&body.soc_id) {
                return Err(format!(
                    "body {} refers to missing SoC {}",
                    body.id, body.soc_id
                ));
            }
            for part_id in &body.parts {
                if !self.parts.contains_key(part_id) {
                    return Err(format!("body {} refers to missing part {part_id}", body.id));
                }
            }
            for (role, part_id, expected_model) in [
                (
                    "sensor",
                    body.sensor.as_ref().map(|item| item.part_id.as_str()),
                    0u8,
                ),
                (
                    "lens",
                    body.lens.as_ref().map(|item| item.part_id.as_str()),
                    1u8,
                ),
                (
                    "display",
                    body.display.as_ref().map(|item| item.part_id.as_str()),
                    2u8,
                ),
                (
                    "microcontroller",
                    body.microcontroller
                        .as_ref()
                        .map(|item| item.part_id.as_str()),
                    3u8,
                ),
                (
                    "power controller",
                    body.power_control
                        .as_ref()
                        .map(|item| item.part_id.as_str()),
                    4u8,
                ),
                (
                    "card slot",
                    body.card_slot.as_ref().map(|item| item.part_id.as_str()),
                    5u8,
                ),
            ] {
                if let Some(part_id) = part_id {
                    if !body.parts.iter().any(|candidate| candidate == part_id) {
                        return Err(format!(
                            "body {} {role} is not listed among its installed parts",
                            body.id
                        ));
                    }
                    let part = &self.parts[part_id];
                    let model_matches = matches!(
                        (expected_model, &part.model),
                        (0, PartModel::ImageSensor { .. })
                            | (1, PartModel::LensMechanism { .. })
                            | (2, PartModel::DisplayPanel { .. })
                            | (3, PartModel::Microcontroller { .. })
                            | (4, PartModel::PowerControl { .. })
                            | (5, PartModel::SdCardSlot)
                    );
                    if !model_matches {
                        return Err(format!(
                            "body {} {role} part {part_id} has an incompatible model",
                            body.id
                        ));
                    }
                }
            }
            for component in [
                body.sensor.as_ref().map(|item| item.part_id.as_str()),
                body.lens.as_ref().map(|item| item.part_id.as_str()),
                body.display.as_ref().map(|item| item.part_id.as_str()),
                body.microcontroller
                    .as_ref()
                    .map(|item| item.part_id.as_str()),
                body.power_control
                    .as_ref()
                    .map(|item| item.part_id.as_str()),
                body.card_slot.as_ref().map(|item| item.part_id.as_str()),
            ]
            .into_iter()
            .flatten()
            {
                if !body.parts.iter().any(|part_id| part_id == component) {
                    return Err(format!(
                        "body {} component refers to undeclared part {component}",
                        body.id
                    ));
                }
            }
            for connection in [
                body.sensor.as_ref().map(|item| item.connection_id.as_str()),
                body.lens.as_ref().map(|item| item.connection_id.as_str()),
                body.display
                    .as_ref()
                    .map(|item| item.connection_id.as_str()),
                body.microcontroller
                    .as_ref()
                    .map(|item| item.connection_id.as_str()),
                body.power_control
                    .as_ref()
                    .map(|item| item.connection_id.as_str()),
                body.card_slot
                    .as_ref()
                    .map(|item| item.connection_id.as_str()),
            ]
            .into_iter()
            .flatten()
            {
                if !body.connections.iter().any(|item| item.id == connection) {
                    return Err(format!(
                        "body {} component refers to missing connection {connection}",
                        body.id
                    ));
                }
            }
            for wiring in &body.wiring {
                let source_exists = body
                    .connections
                    .iter()
                    .any(|item| item.id == wiring.source_id)
                    || body.parts.contains(&wiring.source_id)
                    || self.socs[&body.soc_id]
                        .blocks
                        .iter()
                        .any(|item| item.id == wiring.source_id);
                let target_exists = body
                    .connections
                    .iter()
                    .any(|item| item.id == wiring.target_id)
                    || body.parts.contains(&wiring.target_id)
                    || self.socs[&body.soc_id]
                        .blocks
                        .iter()
                        .any(|item| item.id == wiring.target_id);
                if !source_exists || !target_exists {
                    return Err(format!(
                        "body {} wiring refers to an unknown component",
                        body.id
                    ));
                }
            }
            let soc = &self.socs[&body.soc_id];
            let block_kind = |id: &str, expected: SocBlockKind, role: &str| {
                let block = soc
                    .blocks
                    .iter()
                    .find(|block| block.id == id)
                    .ok_or_else(|| {
                        format!("body {} {role} refers to missing SoC block {id}", body.id)
                    })?;
                if block.kind != expected {
                    return Err(format!(
                        "body {} {role} uses SoC block {id} with the wrong kind",
                        body.id
                    ));
                }
                Ok(())
            };
            if let Some(hardware) = &body.hardware {
                for attachment in &hardware.serial_attachments {
                    block_kind(
                        &attachment.controller_id,
                        SocBlockKind::SerialController,
                        "serial attachment",
                    )?;
                }
            }
            for chip in &body.control_bus_chips {
                block_kind(
                    &chip.controller_id,
                    SocBlockKind::ControlBus,
                    "control-bus device",
                )?;
            }
            if let Some(sensor_bus) = body
                .sensor
                .as_ref()
                .and_then(|sensor| sensor.control_bus.as_ref())
            {
                block_kind(
                    &sensor_bus.controller_id,
                    SocBlockKind::ControlBus,
                    "sensor",
                )?;
                if sensor_bus.address > 0x7f
                    || body.control_bus_chips.iter().any(|chip| {
                        chip.controller_id == sensor_bus.controller_id
                            && chip.channel == sensor_bus.channel
                            && chip.address == sensor_bus.address
                    })
                {
                    return Err(format!(
                        "body {} sensor control-bus address is invalid or repeated",
                        body.id
                    ));
                }
            }
            if let Some(lens) = &body.lens {
                block_kind(
                    &lens.route.controller_id,
                    SocBlockKind::SerialController,
                    "lens",
                )?;
                if !body.connections.iter().any(|connection| {
                    connection.id == lens.route.select_connection_id
                        && connection
                            .signals
                            .iter()
                            .any(|signal| matches!(signal, ConnectionSignal::Gpio { .. }))
                }) {
                    return Err(format!(
                        "body {} lens route has no GPIO chip-select connection",
                        body.id
                    ));
                }
            }
            if let Some(display) = &body.display {
                if let Some(route) = &display.route {
                    block_kind(
                        &route.controller_id,
                        SocBlockKind::SerialController,
                        "display",
                    )?;
                    if !body.connections.iter().any(|connection| {
                        connection.id == route.select_connection_id
                            && connection
                                .signals
                                .iter()
                                .any(|signal| matches!(signal, ConnectionSignal::Gpio { .. }))
                    }) {
                        return Err(format!(
                            "body {} display route has no GPIO chip-select connection",
                            body.id
                        ));
                    }
                }
            }
            if let Some(mcu) = &body.microcontroller {
                block_kind(
                    &mcu.controller_id,
                    SocBlockKind::SerialController,
                    "microcontroller",
                )?;
                if !matches!(mcu.word_bits, 8 | 16 | 32) {
                    return Err(format!(
                        "body {} microcontroller route has an unsupported word width",
                        body.id
                    ));
                }
                if !body.connections.iter().any(|connection| {
                    connection.id == mcu.select_connection_id
                        && connection
                            .signals
                            .iter()
                            .any(|signal| matches!(signal, ConnectionSignal::Gpio { .. }))
                }) {
                    return Err(format!(
                        "body {} microcontroller refers to a missing GPIO chip-select connection {}",
                        body.id, mcu.select_connection_id
                    ));
                }
            }
            if let Some(power) = &body.power_control {
                block_kind(
                    &power.controller_id,
                    SocBlockKind::SerialController,
                    "power controller",
                )?;
                if !body.connections.iter().any(|connection| {
                    connection.id == power.select_connection_id
                        && connection
                            .signals
                            .iter()
                            .any(|signal| matches!(signal, ConnectionSignal::Gpio { .. }))
                }) {
                    return Err(format!(
                        "body {} power controller has no GPIO chip-select connection",
                        body.id
                    ));
                }
            }
            for connection in &body.connections {
                for signal in &connection.signals {
                    let (block_id, kind) = match signal {
                        ConnectionSignal::Block { block_id, .. } => (block_id, None),
                        ConnectionSignal::Serial { block_id, .. } => {
                            (block_id, Some(SocBlockKind::ControlBus))
                        }
                        ConnectionSignal::Interrupt { controller_id, .. } => {
                            (controller_id, Some(SocBlockKind::InterruptController))
                        }
                        ConnectionSignal::SerialStatus {
                            block_id,
                            register,
                            mask,
                        } => {
                            if *register > 0x7f || *mask == 0 {
                                return Err(format!(
                                    "body {} connection {} has an invalid serial status bit",
                                    body.id, connection.id
                                ));
                            }
                            (block_id, Some(SocBlockKind::SerialController))
                        }
                        ConnectionSignal::Gpio { .. } | ConnectionSignal::Adc { .. } => continue,
                    };
                    let block = soc
                        .blocks
                        .iter()
                        .find(|candidate| candidate.id == *block_id);
                    let Some(block) = block else {
                        return Err(format!(
                            "body {} connection {} refers to missing SoC block {block_id}",
                            body.id, connection.id
                        ));
                    };
                    if kind.is_some_and(|expected| block.kind != expected) {
                        return Err(format!(
                            "body {} connection {} uses a SoC block with the wrong kind",
                            body.id, connection.id
                        ));
                    }
                }
            }
            if let Some(card) = &body.card_slot {
                if let Some(controller_id) = &card.controller_id {
                    block_kind(controller_id, SocBlockKind::SdCardController, "card slot")?;
                }
                for connection_id in [
                    &card.detect_connection_id,
                    &card.write_protect_connection_id,
                ] {
                    if !body
                        .connections
                        .iter()
                        .any(|item| item.id == *connection_id)
                    {
                        return Err(format!(
                            "body {} card slot refers to missing contact connection {connection_id}",
                            body.id
                        ));
                    }
                }
            }
        }
        for firmware in self.firmwares.values() {
            self.bodies.get(&firmware.body_id).ok_or_else(|| {
                format!(
                    "firmware {} refers to missing body {}",
                    firmware.id, firmware.body_id
                )
            })?;
        }
        Ok(())
    }
}

fn unique_ids<'a>(ids: impl IntoIterator<Item = &'a str>, label: &str) -> Result<(), String> {
    let mut unique = BTreeSet::new();
    for id in ids {
        validate_id(id)?;
        if !unique.insert(id) {
            return Err(format!("duplicate {label} identity {id}"));
        }
    }
    Ok(())
}

fn insert_unique<T>(
    target: &mut BTreeMap<String, T>,
    id: String,
    value: T,
    path: &Path,
) -> Result<(), String> {
    validate_id(&id)?;
    if target.insert(id.clone(), value).is_some() {
        return Err(format!("duplicate identity {id} in {}", path.display()));
    }
    Ok(())
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8], path: &Path) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn document_string(value: &serde_json::Value, field: &str, path: &Path) -> Result<String, String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{}: missing string field {field}", path.display()))
}

fn contains_input_binding(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => {
            matches!(
                object.get("kind").and_then(serde_json::Value::as_str),
                Some("debug_key" | "native_input")
            ) || object.values().any(contains_input_binding)
        }
        serde_json::Value::Array(items) => items.iter().any(contains_input_binding),
        _ => false,
    }
}

fn read_json_files(directory: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("could not list {}: {error}", directory.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("could not read {}: {error}", directory.display()))?
            .path();
        if path.is_dir() {
            paths.extend(read_json_files(&path)?);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            paths.push((
                path.clone(),
                fs::read(&path)
                    .map_err(|error| format!("could not read {}: {error}", path.display()))?,
            ));
        }
    }
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(paths)
}

#[derive(Clone, Copy, Debug)]
enum ReferenceKind {
    Soc,
    Part,
    Body,
    Firmware,
    PhysicalControls,
    ControlIds,
    FirstStart,
    Bulk,
    Settings,
    Flash,
    State,
}

impl ReferenceKind {
    fn of_body_file(path: &Path) -> Result<Self, String> {
        match path.file_name().and_then(|name| name.to_str()) {
            Some("body.json") => Ok(Self::Body),
            Some("controls.json") => Ok(Self::PhysicalControls),
            _ => Err(format!("{} is not a body document", path.display())),
        }
    }

    fn of_firmware_file(path: &Path) -> Result<Self, String> {
        match path.file_name().and_then(|name| name.to_str()) {
            Some("firmware.json") => Ok(Self::Firmware),
            Some("control-ids.generated.json") => Ok(Self::ControlIds),
            Some("first-start.generated.json") => Ok(Self::FirstStart),
            Some("bulk.generated.json") => Ok(Self::Bulk),
            Some("settings.generated.json") => Ok(Self::Settings),
            Some("flash.generated.json") => Ok(Self::Flash),
            Some("state.generated.json") => Ok(Self::State),
            _ => Err(format!("{} is not a firmware document", path.display())),
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/bundled_catalog.rs"));

fn bundled_documents() -> impl Iterator<Item = (ReferenceKind, &'static str, &'static str)> {
    BUNDLED_CATALOG.iter().copied()
}

#[cfg(test)]
mod tests;
