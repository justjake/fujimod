//! Closed external inputs and scenes for application-profile camera runs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

fn ids<'a>(values: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for value in values {
        fujimod_catalog_identity::validate_id(value)?;
        if !seen.insert(value) {
            return Err(format!("application input repeats identity {value}"));
        }
    }
    Ok(())
}

/// Explicit application-profile input state, separate from reference hardware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ApplicationRunInputs {
    /// Virtual clock ticks per second used by the supplied timing policies.
    pub ticks_per_second: u64,
    /// Initial external values supplied to named SoC blocks.
    pub blocks: Vec<ApplicationBlockInput>,
    /// Initial external values supplied to installed parts.
    pub parts: Vec<ApplicationPartInput>,
    /// Functional device timing policy for this run, not measured silicon timing.
    pub timings: Vec<ApplicationTiming>,
}

impl ApplicationRunInputs {
    /// Validate the supplied values without assuming any hardware defaults.
    pub fn validate(&self) -> Result<(), String> {
        if self.ticks_per_second == 0 {
            return Err("application input timebase must be positive".into());
        }
        ids(self.blocks.iter().map(ApplicationBlockInput::block_id))?;
        ids(self.parts.iter().map(ApplicationPartInput::part_id))?;
        for input in &self.blocks {
            match input {
                ApplicationBlockInput::Adc { samples, .. }
                    if samples.is_empty() || samples.len() > 32 =>
                {
                    return Err("application ADC input needs a bounded channel vector".into());
                }
                ApplicationBlockInput::Gpio {
                    levels, outputs, ..
                } => {
                    if levels.len() > 32 {
                        return Err("application GPIO input vector exceeds its bound".into());
                    }
                    let mut ports = BTreeSet::new();
                    if outputs
                        .iter()
                        .any(|output| output.port >= 32 || !ports.insert(output.port))
                    {
                        return Err("application GPIO output port is invalid or repeated".into());
                    }
                }
                ApplicationBlockInput::Flash { part_id, seeds, .. } => {
                    fujimod_catalog_identity::validate_id(part_id)?;
                    let mut offsets = BTreeSet::new();
                    if seeds.iter().any(|seed| !offsets.insert(seed.offset)) {
                        return Err("application flash input repeats a seeded offset".into());
                    }
                }
                _ => {}
            }
        }
        for input in &self.parts {
            if let ApplicationPartInput::RegisterWords {
                registers,
                rtc_ticks_per_second,
                ..
            } = input
            {
                let mut addresses = BTreeSet::new();
                if registers
                    .iter()
                    .any(|register| !addresses.insert(register.register))
                    || *rtc_ticks_per_second == Some(0)
                {
                    return Err(
                        "application register input has duplicate addresses or zero RTC rate"
                            .into(),
                    );
                }
            }
        }
        for timing in &self.timings {
            match timing {
                ApplicationTiming::Delay {
                    block_ids, ticks, ..
                } => {
                    ids(block_ids.iter().map(String::as_str))?;
                    if block_ids.is_empty() || *ticks == 0 {
                        return Err(
                            "application delay requires blocks and a positive duration".into()
                        );
                    }
                }
                ApplicationTiming::Throughput {
                    block_id,
                    bytes_per_tick,
                    ..
                } => {
                    fujimod_catalog_identity::validate_id(block_id)?;
                    if *bytes_per_tick == 0 {
                        return Err("application throughput must be positive".into());
                    }
                }
                ApplicationTiming::ParentClock {
                    block_ids,
                    ticks_per_parent,
                } => {
                    ids(block_ids.iter().map(String::as_str))?;
                    if block_ids.is_empty() || *ticks_per_parent == 0 {
                        return Err(
                            "application parent-clock ratio must be positive and bound to blocks"
                                .into(),
                        );
                    }
                }
                ApplicationTiming::StorageProgramming {
                    part_id,
                    erase_ticks,
                    page_ticks,
                } => {
                    fujimod_catalog_identity::validate_id(part_id)?;
                    if *erase_ticks == 0 || *page_ticks == 0 {
                        return Err(
                            "application storage programming durations must be positive".into()
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

/// External values supplied to one application-profile SoC block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ApplicationBlockInput {
    /// Explicit analog sample values in channel order.
    Adc { block_id: String, samples: Vec<u16> },
    /// Explicit GPIO input words and known output latches.
    Gpio {
        block_id: String,
        levels: Vec<u16>,
        outputs: Vec<GpioOutputWord>,
    },
    /// Ambient temperature presented to a temperature-monitor block.
    Temperature { block_id: String, millicelsius: i32 },
    /// Idle serial input level on a named controller.
    SerialLine { block_id: String, idle_high: bool },
    /// Backing contents supplied to a nonvolatile-memory aperture.
    Flash {
        block_id: String,
        part_id: String,
        source: ApplicationFlashSource,
        #[serde(default)]
        erased_tail: Option<u8>,
        seeds: Vec<FlashSeed>,
    },
}

impl ApplicationBlockInput {
    /// Named SoC block receiving this input.
    pub fn block_id(&self) -> &str {
        match self {
            Self::Adc { block_id, .. }
            | Self::Gpio { block_id, .. }
            | Self::Temperature { block_id, .. }
            | Self::SerialLine { block_id, .. }
            | Self::Flash { block_id, .. } => block_id,
        }
    }
}

/// One explicitly initialized GPIO output word.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct GpioOutputWord {
    /// GPIO port number.
    pub port: u8,
    /// Supplied output-latch bits for this port.
    pub levels: u16,
}

/// Source of the nonvolatile backing supplied by a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ApplicationFlashSource {
    /// The explicitly supplied, identity-checked selected firmware update.
    FirmwareUpdate,
}

/// An explicit byte overlay on the run's supplied flash backing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FlashSeed {
    /// Offset from the start of flash backing.
    pub offset: u64,
    /// Byte supplied at that offset; this is an intervention, not stock firmware.
    pub value: u8,
}

/// Initial values supplied to one installed serial endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ApplicationPartInput {
    /// Initial word registers and optional RTC timebase.
    RegisterWords {
        part_id: String,
        registers: Vec<RegisterWord>,
        #[serde(default)]
        rtc_ticks_per_second: Option<u64>,
    },
    /// Initial clock state for a power-controller endpoint.
    Clock {
        part_id: String,
        seconds: u32,
        valid: bool,
    },
    /// Idle input level for a serial endpoint.
    IdleLevel { part_id: String, high: bool },
    /// Explicit BIS revision-register and idle-line inputs.
    BisRevision {
        part_id: String,
        revision: [u8; 2],
        idle_high: bool,
    },
}

impl ApplicationPartInput {
    /// Installed part receiving this input.
    pub fn part_id(&self) -> &str {
        match self {
            Self::RegisterWords { part_id, .. }
            | Self::Clock { part_id, .. }
            | Self::IdleLevel { part_id, .. }
            | Self::BisRevision { part_id, .. } => part_id,
        }
    }
}

/// One initial word-sized endpoint register.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RegisterWord {
    /// Endpoint register number.
    pub register: u16,
    /// Supplied register value.
    pub value: u16,
}

/// Explicit functional timing supplied to named blocks or parts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ApplicationTiming {
    /// The same delay policy applied to a set of named SoC blocks.
    Delay {
        block_ids: Vec<String>,
        operation: TimingOperation,
        ticks: u64,
    },
    /// Transfer setup cost and byte throughput for a named SoC block.
    Throughput {
        block_id: String,
        setup_ticks: u64,
        bytes_per_tick: u64,
    },
    /// Ratio between a serial input clock and its parent virtual clock.
    ParentClock {
        block_ids: Vec<String>,
        ticks_per_parent: u64,
    },
    /// Programming delay for an installed storage part.
    StorageProgramming {
        part_id: String,
        erase_ticks: u64,
        page_ticks: u64,
    },
}

/// Device operation to which a run's functional delay applies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum TimingOperation {
    /// Clock or signal settling.
    Settle,
    /// Analog or temperature sampling.
    Sample,
    /// Command acceptance or completion.
    Command,
    /// Device reset completion.
    Reset,
    /// Display pixel clock.
    Pixel,
    /// Control-request acknowledgment.
    Acknowledge,
    /// General processing completion.
    Complete,
}

/// External frame and statistics inputs for an application-profile run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ApplicationRunScene {
    /// Periodic frame sources bound to named SoC blocks.
    pub frames: Vec<ApplicationFrameInput>,
    /// Uniform statistics scenes bound to named SoC blocks.
    pub statistics: Vec<ApplicationStatisticsInput>,
    /// Image values supplied when a declared block requests command-driven work.
    pub command_images: Vec<ApplicationCommandImageInput>,
}

impl ApplicationRunScene {
    /// Validate external frame timing and statistics bounds.
    pub fn validate(&self) -> Result<(), String> {
        for frame in &self.frames {
            ids(frame.block_ids.iter().map(String::as_str))?;
            if frame.block_ids.is_empty()
                || frame.period_ticks == 0
                || frame
                    .active_ticks
                    .is_some_and(|ticks| ticks == 0 || ticks >= frame.period_ticks)
            {
                return Err(
                    "application scene has invalid frame timing or no target blocks".into(),
                );
            }
        }
        ids(self.statistics.iter().map(|input| input.block_id.as_str()))?;
        for image in &self.command_images {
            ids(image.block_ids.iter().map(String::as_str))?;
            if image.block_ids.is_empty() {
                return Err("application command image has no target blocks".into());
            }
        }
        if self
            .statistics
            .iter()
            .any(|input| input.samples_per_cell == 0 || input.signal_shift >= 16)
        {
            return Err("application statistics input has invalid sample or signal bounds".into());
        }
        Ok(())
    }
}

/// Uniform image data supplied to command-driven imaging work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ApplicationCommandImageInput {
    /// Blocks consuming this command-driven image input.
    pub block_ids: Vec<String>,
    /// Uniform source luma sample.
    pub luma: u16,
}

/// A periodic external frame source for one or more declared blocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ApplicationFrameInput {
    /// Blocks consuming this external frame source.
    pub block_ids: Vec<String>,
    /// Period between frames in the run's virtual ticks.
    pub period_ticks: u64,
    /// Active duration within the frame period, when supplied.
    #[serde(default)]
    pub active_ticks: Option<u64>,
    /// Uniform source luma sample, when supplied.
    #[serde(default)]
    pub luma: Option<u16>,
}

/// A uniform statistics scene supplied to one imaging block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ApplicationStatisticsInput {
    /// Imaging block consuming the statistics scene.
    pub block_id: String,
    /// Uniform input luma value.
    pub luma: u16,
    /// Number of samples accumulated in one cell.
    pub samples_per_cell: u32,
    /// Black-level subtraction value.
    pub black_level: u16,
    /// Right shift applied to the input signal.
    pub signal_shift: u8,
}
