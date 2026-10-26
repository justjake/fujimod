//! Hardware parameters shared by application-profile SoC records.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Additional hardware parameters whose meaning depends on the processor family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct SocHardware {
    /// Named hardware profiles, selected explicitly by a run preset.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "Record<string, SocProfile>")
    )]
    pub profiles: BTreeMap<String, crate::profiles::SocProfile>,
    /// Application-processor identification and cache topology, when declared.
    #[serde(default)]
    pub application: Option<ApplicationHardware>,
    /// Hardware interrupt-controller interfaces.
    #[serde(default)]
    pub interrupt_controller: Option<InterruptControllerHardware>,
    /// Inputs to a programmable interrupt controller; core routing is selected by its registers.
    pub interrupt_inputs: Vec<ControllerInterruptInput>,
    /// Companion-processor clock and mailbox connections, when present.
    #[serde(default)]
    pub companion: Option<CompanionHardware>,
    /// Additional block parameters, keyed by the block's identity.
    pub blocks: BTreeMap<String, BlockHardware>,
}

impl SocHardware {
    /// Check every hardware connection against the containing SoC's identities.
    pub fn validate(&self, soc: &crate::reference::Soc) -> Result<(), String> {
        let block = |id: &str| {
            soc.blocks
                .iter()
                .any(|item| item.id == id)
                .then_some(())
                .ok_or_else(|| format!("SoC {} hardware refers to missing block {id}", soc.id))
        };
        let core = |id: &str| {
            soc.processors
                .iter()
                .any(|item| item.id == id)
                .then_some(())
                .ok_or_else(|| format!("SoC {} hardware refers to missing processor {id}", soc.id))
        };
        let clock = |id: &str| {
            soc.clocks
                .iter()
                .any(|item| item.id == id)
                .then_some(())
                .ok_or_else(|| format!("SoC {} hardware refers to missing clock {id}", soc.id))
        };
        if let Some(ApplicationHardware::Armv8a {
            affinities,
            counter_clock_id,
            pmu_clock_id,
            ..
        }) = &self.application
        {
            let mut values = std::collections::BTreeSet::new();
            for (id, affinity) in affinities {
                core(id)?;
                if !values.insert(affinity) {
                    return Err(format!("SoC {} repeats a processor affinity", soc.id));
                }
            }
            clock(counter_clock_id)?;
            clock(pmu_clock_id)?;
        }
        if let Some(controller) = &self.interrupt_controller {
            let (distributor, interface, lines) = match controller {
                InterruptControllerHardware::GicV2 {
                    distributor_block_id,
                    cpu_interface_block_id,
                    lines,
                } => (distributor_block_id, cpu_interface_block_id, lines),
                InterruptControllerHardware::GicV3 {
                    distributor_block_id,
                    redistributor_block_id,
                    lines,
                    ..
                } => (distributor_block_id, redistributor_block_id, lines),
            };
            block(distributor)?;
            block(interface)?;
            if *lines == 0 {
                return Err(format!("SoC {} interrupt controller has no lines", soc.id));
            }
        }
        if let Some(companion) = &self.companion {
            core(&companion.processor_id)?;
            clock(&companion.clock_id)?;
            block(&companion.mailbox_block_id)?;
        }
        let mut interrupt_outputs = std::collections::BTreeSet::new();
        for input in &self.interrupt_inputs {
            block(&input.source_block_id)?;
            if !interrupt_outputs.insert((&input.source_block_id, &input.output_id)) {
                return Err(format!(
                    "SoC {} repeats an interrupt producer output",
                    soc.id
                ));
            }
        }
        for (id, parameters) in &self.blocks {
            block(id)?;
            match parameters {
                BlockHardware::Dma {
                    channels,
                    request_mux_block_id,
                } => {
                    block(request_mux_block_id)?;
                    if *channels == 0 {
                        return Err(format!("SoC {} DMA block {id} has no channels", soc.id));
                    }
                }
                BlockHardware::SerialFlash { aperture_block_id } => block(aperture_block_id)?,
                BlockHardware::ImagingInputBuffer {
                    sram_region_id,
                    bank_bytes,
                } => {
                    if !soc
                        .memory_regions
                        .iter()
                        .any(|item| item.id == *sram_region_id)
                        || *bank_bytes == 0
                    {
                        return Err(format!(
                            "SoC {} input buffer {id} has invalid SRAM banks",
                            soc.id
                        ));
                    }
                }
                BlockHardware::Display { .. }
                | BlockHardware::SerialDma { .. }
                | BlockHardware::CompanionDma { .. } => {}
            }
        }
        Ok(())
    }
}

/// A producer output connected to an input of the programmable interrupt controller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ControllerInterruptInput {
    /// Register block producing the interrupt.
    pub source_block_id: String,
    /// Producer output identity, such as a channel or error output.
    pub output_id: String,
    /// Interrupt-controller input line.
    pub line: u32,
}

/// Family-specific application-processor identification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ApplicationHardware {
    /// ARMv7-A configuration base and cache capacities.
    Armv7a {
        /// Configuration base address presented to each application core.
        configuration_base: u64,
        /// Declared cache capacities.
        caches: CacheGeometry,
    },
    /// ARMv8-A processor identity, affinities and architectural timer wiring.
    Armv8a {
        /// MIDR_EL1 processor identification value.
        midr_el1: u64,
        /// MPIDR_EL1 affinity values, keyed by processor identity.
        affinities: BTreeMap<String, u64>,
        /// Clock domain supplying the architectural counter.
        counter_clock_id: String,
        /// Clock domain supplying the performance monitor.
        pmu_clock_id: String,
        /// Physical architectural timer interrupt line.
        physical_timer_irq: u32,
        /// Virtual architectural timer interrupt line.
        virtual_timer_irq: u32,
    },
}

/// Cache capacities declared for an application processor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CacheGeometry {
    /// Per-core level-one data-cache capacity.
    pub l1_data_bytes: u64,
    /// Per-core level-one instruction-cache capacity.
    pub l1_instruction_bytes: u64,
    /// Shared level-two cache capacity.
    pub l2_bytes: u64,
}

/// ARM generic interrupt-controller interfaces.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum InterruptControllerHardware {
    /// GICv2 distributor and CPU interface.
    GicV2 {
        /// Distributor register block identity.
        distributor_block_id: String,
        /// CPU-interface register block identity.
        cpu_interface_block_id: String,
        /// Number of implemented interrupt lines.
        lines: u32,
    },
    /// GICv3 distributor and per-core redistributors.
    GicV3 {
        /// Distributor register block identity.
        distributor_block_id: String,
        /// Redistributor register block identity.
        redistributor_block_id: String,
        /// Distance between adjacent redistributor frames.
        redistributor_stride: u64,
        /// Number of implemented interrupt lines.
        lines: u32,
    },
}

/// A companion processor's hardware clock and mailbox endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CompanionHardware {
    /// Companion processor identity in the SoC processor list.
    pub processor_id: String,
    /// Clock domain supplying the companion processor.
    pub clock_id: String,
    /// Vector-table address presented to the companion after reset.
    pub vector_base: u64,
    /// SoC mailbox block identity.
    pub mailbox_block_id: String,
    /// Mailbox indices used by the companion link.
    pub mailbox: CompanionMailbox,
}

/// Mailbox indices used by a companion processor link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CompanionMailbox {
    /// Request mailbox index.
    pub request: u32,
    /// Control mailbox index.
    pub control: u32,
    /// Reply mailbox index.
    pub reply: u32,
    /// Status mailbox index.
    pub status: u32,
    /// Fault mailbox index.
    pub fault: u32,
}

/// Hardware parameters for a named SoC register block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum BlockHardware {
    /// DMA channel count and request-multiplexer connection.
    Dma {
        /// Number of DMA channels.
        channels: u32,
        /// Request-multiplexer register block identity.
        request_mux_block_id: String,
    },
    /// Serial-flash controller's memory-mapped aperture.
    SerialFlash {
        /// Flash aperture block identity.
        aperture_block_id: String,
    },
    /// Imaging input buffer's SRAM banks.
    ImagingInputBuffer {
        /// SRAM memory-region identity.
        sram_region_id: String,
        /// Capacity of one SRAM bank.
        bank_bytes: u64,
    },
    /// Display-channel pixel packing declared by the controller contract.
    Display {
        /// Picture-plane packing, when specified.
        #[serde(default)]
        picture_layout: Option<PictureLayout>,
        /// Video-plane packing, when specified.
        #[serde(default)]
        video_layout: Option<VideoLayout>,
    },
    /// Serial-controller DMA request lines.
    SerialDma {
        /// Receive request identifier.
        receive_request: u32,
        /// Transmit request identifier.
        transmit_request: u32,
    },
    /// DMA interrupt range presented to a companion processor.
    CompanionDma {
        /// First companion interrupt line for the DMA controller.
        companion_irq_base: u32,
    },
}

/// Picture-plane pixel packing declared by a display controller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum PictureLayout {
    /// Blue, green, red and straight-alpha bytes.
    Bgra8888Straight,
}

/// Video-plane pixel packing declared by a display controller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum VideoLayout {
    /// Two luma samples followed by shared chroma samples.
    Yycbcr422,
}
