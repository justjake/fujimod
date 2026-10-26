//! Transport-owned lifetime and typed resolved inputs for the common runtime.
//! This is not a second processor/body graph resolver. The EXR II resolver will
//! supply construction here when it lands; unsupported kinds refuse meanwhile.
use fujimod_emulator::board::{
    InterruptMap,
    bus::{Bus, Mode},
    factory,
};
use fujimod_emulator_cpu::{Model, Register, arm, timeline::Ratio};
use fujimod_emulator_devices::{graph::{DeviceGraph, DeviceWindow}, graphics::scanout::Frame, parts::{controls::Source, pins::DriveInput}};
use fujimod_emulator_parts_controls::resistor_keys::{Key, Options as ResistorKeysOptions, ResistorKeys};
use fujimod_emulator_graphics::raster::PixelOrder;
use fujimod_emulator_xp5_board::board;
use fujimod_emulator_xp5_display::display;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    schema: u32,
    processor: Processor,
    body: Body,
    scenario: Scenario,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Processor {
    cores: Vec<Core>,
    interrupts: Option<Interrupts>,
    ram: Vec<Ram>,
    mappings: Vec<Mapping>,
    peripherals: Vec<Peripheral>,
    dma_mappings: Vec<fujimod_emulator::board::memory::BusMapping>,
    instructions_per_tick: [u64; 2],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Interrupts {
    affinities: Vec<u32>,
    lines: usize,
    distributor: u64,
    redistributor: u64,
    stride: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ModelName {
    CortexR5f,
    CortexA7,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Core {
    model: ModelName,
    released: bool,
    pc: u32,
    registers: BTreeMap<u8, u32>,
    rprofile: Option<fujimod_emulator_cpu_unicorn::rprofile::core::Options>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ram {
    base: u64,
    size: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    core: usize,
    address: u64,
    physical: u64,
    size: usize,
    permissions: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Peripheral {
    core: usize,
    address: u64,
    physical: u64,
    size: usize,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Body {
    Bare,
    Xp5 {
        windows: Vec<DeviceWindow>,
        controls: Controls,
        displays: Vec<Display>,
    },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Controls {
    X100vi { resistor_keys: ResistorKeysOptions },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Display {
    base: u64,
    period_ns: Option<u64>,
    raw_formats: [Option<String>; 4],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    quantum: usize,
    firmware: Firmware,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Firmware {
    size: usize,
    pub(super) sha256: String,
    format: Format,
    mappings: Vec<Load>,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Format {
    Raw,
    Dat,
}
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Encoding {
    Raw,
    Method4,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Load {
    name: String,
    offset: u64,
    size: u64,
    base: u64,
    encoding: Encoding,
    executable: bool,
}

pub struct Session {
    pub(super) construction: Option<super::snapshot::Construction>,
    pub(super) capture_order: std::cell::Cell<u64>,
    pub core: fujimod_emulator_session::Session,
    pub(super) reset_image: Option<Arc<fujimod_emulator_session::checkpoint::Snapshot>>,
    pub(super) powered_on: bool,
    pub(super) generation: u64,
    /// Identifies this session in its checkpoint IDs and input logs.
    pub(super) owner: u32,
    pub(crate) camera: Option<super::camera::Metadata>,
    pub(super) quantum: usize,
    pub(super) input: Option<(Arc<ResistorKeys>, usize)>,
    pub frames: BTreeMap<usize, Arc<Frame>>,
    pub(super) history: Option<super::history::History>,
    pub failed: bool,
}
impl Session {
    pub fn create(
        config: &[u8],
        firmware: &[u8],
        policy: fujimod_emulator_cpu::ExecutionPolicy,
    ) -> Result<Self, String> {
        let construction = super::snapshot::Construction::resolved(config, firmware);
        let definition: serde_json::Value =
            serde_json::from_slice(config).map_err(|e| e.to_string())?;
        let config: Config = serde_json::from_slice(config).map_err(|e| e.to_string())?;
        if config.schema != 1 || config.scenario.quantum == 0 {
            return Err("unsupported session schema or zero quantum".into());
        }
        let images = decode(&config.scenario.firmware, firmware)?;
        let mut devices = DeviceGraph::new();
        let input = match config.body {
            Body::Bare => None,
            Body::Xp5 {
                windows,
                controls,
                displays,
            } => {
                let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
                let driver: Arc<dyn DriveInput> = pins.clone();
                let input = Arc::new(match controls {
                    Controls::X100vi { resistor_keys } => ResistorKeys::new(driver, resistor_keys)?,
                });
                input.start(Duration::ZERO);
                let mut scanouts = BTreeMap::new();
                for d in displays {
                    let mut formats = [None; 4];
                    for (out, name) in formats.iter_mut().zip(d.raw_formats) {
                        *out = name.as_deref().map(PixelOrder::parse).transpose()?;
                    }
                    if scanouts
                        .insert(
                            d.base,
                            display::Config {
                                period_ns: d.period_ns,
                                raw_formats: formats,
                            },
                        )
                        .is_some()
                    {
                        return Err("duplicate display configuration".into());
                    }
                }
                let mut resources = board::Resources {
                    controls: input.clone(),
                    pins,
                    clocks: fujimod_emulator_devices::xp5::panel_clocks::Clocks::new(),
                    companion: None,
                    storage: None,
                    displays: scanouts,
                };
                board::register_all(&mut devices, &windows, &mut resources, &mut |w| {
                    Err(format!("unsupported device {}@{:#x}", w.kind, w.base))
                })?;
                if !resources.displays.is_empty() {
                    return Err("unused display configuration".into());
                }
                let source = devices.attach(Box::new(Source::new(input.clone())));
                for w in windows
                    .iter()
                    .filter(|w| matches!(w.kind.as_str(), "gpio" | "gpio-port3" | "adc"))
                {
                    let id = devices.at(w.base)?;
                    let mut causes = devices.nodes[id].native_dependencies.clone();
                    causes.push(source);
                    devices.depend_on(id, causes)?;
                }
                Some((input, source))
            }
        };
        let p = config.processor;
        let interrupts = p
            .interrupts
            .map(|g| {
                Ok::<_, String>(InterruptMap {
                    cores: g.affinities.len(),
                    gic: Box::new(fujimod_emulator_ip_arm::gicv3::Gic::new(
                        g.affinities,
                        g.lines,
                    )?),
                    registers: fujimod_emulator::board::InterruptRegisters::GicV3 {
                        distributor: g.distributor,
                        redistributor: g.redistributor,
                        stride: g.stride,
                    },
                })
            })
            .transpose()?;
        let graph = factory::Graph {
            processors: p
                .cores
                .into_iter()
                .map(|c| {
                    let mut registers = vec![(Register::Pc, u64::from(c.pc))];
                    for (r, value) in c.registers {
                        if r > 14 {
                            return Err("initial general register must be r0..r14".into());
                        }
                        registers.push((Register::Arm32(arm::Register::R(r)), u64::from(value)));
                    }
                    Ok(factory::Processor {
                        model: match c.model {
                            ModelName::CortexR5f => Model::CortexR5F,
                            ModelName::CortexA7 => Model::CortexA7,
                        },
                        registers,
                        released: c.released,
                        mprofile: None,
                        rprofile: c.rprofile,
                    })
                })
                .collect::<Result<_, String>>()?,
            ram: p
                .ram
                .into_iter()
                .map(|r| factory::Ram {
                    base: r.base,
                    size: r.size,
                })
                .collect(),
            mappings: p
                .mappings
                .into_iter()
                .map(|m| factory::MemoryMap {
                    core: m.core,
                    address: m.address,
                    physical: m.physical,
                    size: m.size,
                    permissions: m.permissions,
                })
                .collect(),
            peripherals: p
                .peripherals
                .into_iter()
                .map(|m| factory::Peripheral {
                    core: m.core,
                    address: m.address,
                    physical: m.physical,
                    size: m.size,
                })
                .collect(),
            dma_mappings: p.dma_mappings,
            decoders: vec![],
            instructions_per_tick: Ratio::new(
                p.instructions_per_tick[0],
                p.instructions_per_tick[1],
            )?,
        };
        let mut board = graph.build(
            factory::Options::new(factory::Engine::Unicorn),
            Bus::from_graph(Mode::Strict, devices)?,
            interrupts,
        )?;
        for image in images {
            board.write_memory(0, image.base, &image.bytes)?;
        }
        board.set_execution_policy(policy)?;
        let mut session = Self {
            construction: Some(construction),
            capture_order: std::cell::Cell::new(1),
            core: fujimod_emulator_session::Session {
                definition,
                controls: None,
                contact_driver: None,
                board,
                resources: Default::default(),
                nodes: Default::default(),
                contacts: Default::default(),
                recording_frames: Default::default(),
            },
            owner: next_owner(),
            camera: None,
            reset_image: None,
            powered_on: true,
            generation: 0,
            quantum: config.scenario.quantum,
            input,
            frames: BTreeMap::new(),
            history: None,
            failed: false,
        };
        session.reset_image = session.core.checkpoint(None).ok().map(Arc::new);
        Ok(session)
    }
    pub fn run(&mut self, until: u64) -> Result<(), String> {
        if !self.powered_on {
            return Ok(());
        }
        if until < self.core.board.now() {
            return Err("run deadline precedes current guest time".into());
        }
        let count = self
            .core
            .board
            .instruction_counts()
            .into_iter()
            .try_fold(0u64, |sum, n| sum.checked_add(n))
            .ok_or("board instruction count overflow")?;
        let input = fujimod_emulator_session::history::Input::Run {
            instructions: u64::MAX - count,
            quantum: self.quantum,
            pause_after: None,
            until: Some(until),
        };
        self.mark_input(&input)?;
        let result = self.apply_input(input);
        if result.is_err() {
            self.failed = true;
        }
        self.collect_frames();
        result
    }
    pub fn input(&mut self, key: &str, pressed: bool, at: u64) -> Result<(), String> {
        if self.history.is_some() {
            return Err("legacy queued keys do not have a shared history input".into());
        }
        if at < self.core.board.now() {
            return Err("input edge precedes current guest time".into());
        }
        let (input, source) = self
            .input
            .as_ref()
            .ok_or("board has no physical controls")?;
        input.enqueue(Duration::from_nanos(at), Key::parse(key)?, pressed)?;
        self.core.board.inputs_changed(*source)
    }
}

/// Owner IDs are unique within this module instance.
pub(super) fn next_owner() -> u32 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub(super) fn decode(
    spec: &Firmware,
    firmware: &[u8],
) -> Result<Vec<fujimod_firmware::Unit>, String> {
    if firmware.len() != spec.size || fujimod_firmware::sha256(firmware) != spec.sha256 {
        return Err("firmware size/SHA-256 mismatch".into());
    }
    let images = match spec.format {
        Format::Dat => {
            fujimod_firmware::decode(
                firmware,
                &spec
                    .mappings
                    .iter()
                    .map(|m| fujimod_firmware::Mapping {
                        name: m.name.clone(),
                        offset: m.offset,
                        size: m.size,
                        base: m.base,
                        executable: m.executable,
                        encoding: match m.encoding {
                            Encoding::Raw => fujimod_firmware::Encoding::Raw,
                            Encoding::Method4 => fujimod_firmware::Encoding::Method4,
                        },
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| e.to_string())?
            .units
        }
        Format::Raw => spec
            .mappings
            .iter()
            .map(|m| {
                if !matches!(m.encoding, Encoding::Raw) {
                    return Err("raw image cannot use DAT compression".into());
                }
                Ok(fujimod_firmware::Unit {
                    name: m.name.clone(),
                    base: m.base,
                    executable: m.executable,
                    bytes: fujimod_firmware::extent(firmware, m.offset, m.size)
                        .map_err(|e| e.to_string())?
                        .to_vec(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    };
    Ok(images)
}
