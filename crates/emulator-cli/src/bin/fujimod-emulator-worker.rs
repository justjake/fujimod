//! Signed native owner for the common board factory. JSON-lines transport and
//! raw-frame publication are outside guest execution and never supply its time.
use fujimod_catalog_assets::Asset;
use fujimod_emulator::board::{self, Board, InterruptMap, aprofile, bus::Bus, factory, mprofile};
use fujimod_catalog_assets::files::{Files, Location};
use fujimod_emulator_cpu::{self as cpu, Access, Model, Register};
use fujimod_emulator_devices::{graph::{DeviceGraph, DeviceWindow}, parts::{controls::Source, pins::DriveInput}};
use fujimod_emulator_parts_controls::resistor_keys::{Key, Options as ResistorKeysOptions, ResistorKeys};
use fujimod_emulator_xp5_board::board as xp5_board;
use fujimod_emulator_xp5_display::display;
use fujimod_emulator_xp5_storage::flash_storage;
use fujimod_emulator_xp5_io::mailbox;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Engine {
    Unicorn,
    Hypervisor,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Strict,
    Discovery,
}
impl Mode {
    fn runtime(&self) -> board::bus::Mode {
        match self {
            Self::Strict => board::bus::Mode::Strict,
            Self::Discovery => board::bus::Mode::Discovery,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterValue {
    name: String,
    value: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mprofile {
    clock_hz: u64,
    board_ticks_per_second: u64,
    vector_base: u32,
}
impl Mprofile {
    fn runtime(self) -> mprofile::Config {
        mprofile::Config {
            clock_hz: self.clock_hz,
            board_ticks_per_second: self.board_ticks_per_second,
            vector_base: self.vector_base,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CortexA53 {
    cores: usize,
    peripheral_base: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Aprofile {
    clock: fujimod_emulator::board::aprofile::generic_timer::Clock,
    physical_irq: u32,
    virtual_irq: u32,
    pmu_hz: u64,
    implementation: Option<CortexA53>,
}
impl Aprofile {
    fn runtime(self) -> aprofile::Config {
        aprofile::Config {
            clock: self.clock,
            physical_irq: self.physical_irq,
            virtual_irq: self.virtual_irq,
            pmu_hz: self.pmu_hz,
            implementation: self.implementation.map(|c| aprofile::CortexA53 {
                cores: c.cores,
                peripheral_base: c.peripheral_base,
            }),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Processor {
    model: Model,
    registers: Vec<RegisterValue>,
    released: bool,
    mprofile: Option<Mprofile>,
    rprofile: Option<fujimod_emulator_cpu_unicorn::rprofile::core::Options>,
    aprofile: Option<Aprofile>,
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
#[serde(tag = "bank", rename_all = "kebab-case", deny_unknown_fields)]
enum GicWrite {
    Distributor {
        offset: u64,
        size: u8,
        value: u64,
    },
    Redistributor {
        core: usize,
        offset: u64,
        size: u8,
        value: u64,
    },
    System {
        core: usize,
        register: String,
        value: u64,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Gic {
    affinities: Vec<u32>,
    lines: usize,
    distributor: u64,
    redistributor: u64,
    stride: u64,
    initial: Vec<GicWrite>,
}
impl Gic {
    fn runtime(self) -> Result<InterruptMap, String> {
        let mut gic = fujimod_emulator_ip_arm::gicv3::Gic::new(
            self.affinities.clone(),
            self.lines,
        )?;
        for write in self.initial {
            match write {
                GicWrite::Distributor {
                    offset,
                    size,
                    value,
                } => {
                    gic.distributor(offset, size, Some(value))?;
                }
                GicWrite::Redistributor {
                    core,
                    offset,
                    size,
                    value,
                } => {
                    gic.redistributor(core, offset, size, Some(value))?;
                }
                GicWrite::System {
                    core,
                    register,
                    value,
                } => {
                    gic.system(
                        core,
                        cpu::arm64::system_register(&register)?.encoding()?,
                        Some(value),
                    )?;
                }
            }
        }
        Ok(InterruptMap {
            gic: Box::new(gic),
            registers: fujimod_emulator::board::InterruptRegisters::GicV3 {
                distributor: self.distributor,
                redistributor: self.redistributor,
                stride: self.stride,
            },
            cores: self.affinities.len(),
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputFiles {
    root: PathBuf,
    files: BTreeMap<String, Location>,
    max_asset_bytes: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Image {
    address: u64,
    asset: String,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum ControlInput {
    X100vi { resistor_keys: ResistorKeysOptions },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Companion {
    processor: usize,
    request: u32,
    control: u32,
    reply: u32,
    status: u32,
    fault: u32,
}
impl Companion {
    fn pins(self) -> mailbox::Pins {
        mailbox::Pins {
            companion: self.processor,
            request: self.request,
            control: self.control,
            reply: self.reply,
            status: self.status,
            fault: self.fault,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scanout {
    base: u64,
    period_ns: Option<u64>,
    raw_formats: [Option<String>; 4],
}
impl Scanout {
    fn config(self) -> Result<(u64, display::Config), String> {
        let mut config = display::Config {
            period_ns: self.period_ns,
            ..Default::default()
        };
        for (slot, format) in config.raw_formats.iter_mut().zip(self.raw_formats) {
            *slot = format
                .as_deref()
                .map(fujimod_emulator_graphics::raster::PixelOrder::parse)
                .transpose()?;
        }
        Ok((self.base, config))
    }
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Devices {
    Xp5 {
        windows: Vec<DeviceWindow>,
        controls: ControlInput,
        companion: Option<Companion>,
        storage: Option<String>,
        #[serde(default)]
        sd: Vec<xp5_board::sd::MediaInputs>,
        displays: Vec<Scanout>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema: u32,
    engine: Engine,
    mode: Mode,
    instructions_per_tick: [u64; 2],
    quantum: usize,
    ram: Vec<Ram>,
    processors: Vec<Processor>,
    mappings: Vec<Mapping>,
    peripherals: Vec<Peripheral>,
    interrupts: Option<Gic>,
    devices: Devices,
    images: Vec<Image>,
    assets: Vec<Asset>,
    inputs: InputFiles,
    output_directory: PathBuf,
}
#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
enum Command {
    Create {
        config: Box<Config>,
    },
    Run {
        until: u64,
    },
    State,
    Progress,
    InstructionPoints { core: usize, addresses: Vec<u64> },
    InstructionEvent,
    RunInstructions {
        count: u64,
    },
    ReadMemory {
        core: usize,
        address: u64,
        size: usize,
        #[serde(default)]
        virtual_address: bool,
    },
    WriteMemory {
        core: usize,
        address: u64,
        hex: String,
    },
    ReadRegister {
        core: usize,
        name: String,
    },
    WriteRegister {
        core: usize,
        name: String,
        value: u64,
    },
    Translate {
        core: usize,
        address: u64,
        access: String,
    },
    Input {
        key: String,
        pressed: bool,
        at: Option<u64>,
    },
    Frames,
    Close,
}
fn register(name: &str) -> Result<Register, String> {
    match name {
        "pc" => Ok(Register::Pc),
        "sp" => Ok(Register::Sp),
        "pstate" => Ok(Register::Pstate),
        "fpcr" => Ok(Register::Fpcr),
        "fpsr" => Ok(Register::Fpsr),
        _ => {
            if let Some(r) = name
                .strip_prefix('x')
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| *n <= 30)
            {
                return Ok(Register::X(r));
            }
            if let Some(r) = name
                .strip_prefix('r')
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| *n <= 15)
            {
                return Ok(Register::Arm32(cpu::arm::Register::R(r)));
            }
            if let Some(r) = name
                .strip_prefix("sys:")
                .and_then(|n| u16::from_str_radix(n.trim_start_matches("0x"), 16).ok())
            {
                return Ok(Register::Arm64System(
                    cpu::arm::SystemRegister::from_encoding(r),
                ));
            }
            Ok(Register::Arm64System(cpu::arm64::system_register(name)?))
        }
    }
}
fn access(name: &str) -> Result<Access, String> {
    match name {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        "fetch" => Ok(Access::Fetch),
        _ => Err("invalid translation access".into()),
    }
}
fn bytes(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.is_ascii() || !hex.len().is_multiple_of(2) || hex.len() > 32 * 1024 * 1024 {
        return Err("invalid hex byte string".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|n| u8::from_str_radix(&hex[n..n + 2], 16).map_err(|e| e.to_string()))
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
struct Worker {
    board: Board,
    quantum: usize,
    input: Arc<ResistorKeys>,
    input_source: usize,
    output_directory: PathBuf,
    archive: u64,
    unmodeled_devices: Vec<String>,
    instruction_points: Option<(usize, Box<board::observation::InstructionPoints>)>,
}
impl Worker {
    fn new(config: Config) -> Result<Self, String> {
        if config.schema != 1 || config.quantum == 0 {
            return Err("unsupported worker configuration".into());
        }
        let mut assets = BTreeMap::new();
        for asset in config.assets {
            asset.validate()?;
            if assets.insert(asset.id.clone(), asset).is_some() {
                return Err("duplicate worker asset identity".into());
            }
        }
        let storage_id = match &config.devices {
            Devices::Xp5 { storage, .. } => storage.as_deref(),
        };
        let sd_ids: Vec<_> = match &config.devices { Devices::Xp5 { sd, .. } => sd.iter().flat_map(|media| media.files.iter().map(|(_, id)| id.as_str())).collect() };
        let used: BTreeSet<_> = config
            .images
            .iter()
            .map(|image| image.asset.as_str())
            .chain(storage_id)
            .chain(sd_ids.iter().copied())
            .collect();
        if used != assets.keys().map(String::as_str).collect() {
            return Err("worker image/storage references differ from declared assets".into());
        }
        let provider = Files::new(
            &config.inputs.root,
            &config.inputs.root,
            BTreeMap::new(),
            config.inputs.files,
            config.inputs.max_asset_bytes,
        )?;
        let load = |id: &str| {
            assets
                .get(id)
                .ok_or("worker asset is missing")?
                .load(&provider)
        };
        // Verify every initial byte and backing image before constructing devices or a VM.
        let images = config
            .images
            .iter()
            .map(|image| Ok((image.address, load(&image.asset)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let storage_image = storage_id.map(load).transpose()?;
        let sd_assets = sd_ids.into_iter().map(|id| Ok((id.to_owned(), load(id)?))).collect::<Result<BTreeMap<_,_>,String>>()?;
        let mode = config.mode.runtime();
        let mut graph = DeviceGraph::new();
        let mut unmodeled_devices = vec![];
        let (input, input_source) = match config.devices {
            Devices::Xp5 {
                windows,
                controls,
                companion,
                storage: _,
                sd,
                displays,
            } => {
                let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
                let driver: Arc<dyn DriveInput> = pins.clone();
                let input = Arc::new(match controls {
                    ControlInput::X100vi { resistor_keys } => {
                        ResistorKeys::new(driver, resistor_keys)?
                    }
                });
                input.start(Duration::ZERO);
                if companion.as_ref().is_some_and(|c| {
                    config
                        .processors
                        .get(c.processor)
                        .is_none_or(|p| !matches!(p.model, Model::CortexM4))
                }) {
                    return Err(
                        "XP5 mailbox companion must select a declared M-profile processor".into(),
                    );
                }
                let mut resources = xp5_board::Resources {
                    sd: xp5_board::sd::resolve(sd, &sd_assets)?,
                    controls: input.clone(),
                    pins,
                    clocks: fujimod_emulator_devices::xp5::panel_clocks::Clocks::new(),
                    companion: companion.map(|c| xp5_board::Companion {
                        pins: c.pins(),
                        channel: mailbox::Channel::default(),
                    }),
                    storage: storage_image
                        .map(|s| {
                            Ok::<_, String>(Arc::new(Mutex::new(flash_storage::Storage::new(s)?)))
                        })
                        .transpose()?,
                    displays: displays
                        .into_iter()
                        .map(Scanout::config)
                        .collect::<Result<BTreeMap<_, _>, _>>()?,
                };
                xp5_board::register_all(&mut graph, &windows, &mut resources, &mut |window| {
                    let missing = format!("{}@{:#x}", window.kind, window.base);
                    if mode == board::bus::Mode::Strict {
                        Err(format!("unknown device {missing}"))
                    } else {
                        unmodeled_devices.push(missing);
                        Ok(())
                    }
                })?;
                if !resources.sd.is_empty() { return Err("unused SD media configuration".into()); }
                if !resources.displays.is_empty() {
                    return Err("unused scanout configuration".into());
                }
                let source = graph.attach(Box::new(Source::new(input.clone())));
                for window in windows
                    .iter()
                    .filter(|w| matches!(w.kind.as_str(), "gpio" | "gpio-port3"))
                {
                    let id = graph.at(window.base)?;
                    let mut causes = graph.nodes[id].native_dependencies.clone();
                    causes.push(source);
                    graph.depend_on(id, causes)?;
                }
                (input, source)
            }
        };
        let bus = Bus::from_graph(mode, graph)?;
        let mut controllers = vec![];
        let processors = config
            .processors
            .into_iter()
            .map(|p| {
                controllers.push(p.aprofile.map(Aprofile::runtime));
                Ok(factory::Processor {
                    model: p.model,
                    released: p.released,
                    mprofile: p.mprofile.map(Mprofile::runtime),
                    rprofile: p.rprofile,
                    registers: p
                        .registers
                        .into_iter()
                        .map(|r| Ok((register(&r.name)?, r.value)))
                        .collect::<Result<_, String>>()?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let dma_mappings = config
            .ram
            .iter()
            .map(|r| board::memory::BusMapping::ram(r.base, r.size))
            .collect();
        let graph = factory::Graph {
            decoders: vec![],
            processors,
            dma_mappings,
            ram: config
                .ram
                .into_iter()
                .map(|r| factory::Ram {
                    base: r.base,
                    size: r.size,
                })
                .collect(),
            mappings: config
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
            peripherals: config
                .peripherals
                .into_iter()
                .map(|m| factory::Peripheral {
                    core: m.core,
                    address: m.address,
                    physical: m.physical,
                    size: m.size,
                })
                .collect(),
            instructions_per_tick: fujimod_emulator_cpu::timeline::Ratio::new(
                config.instructions_per_tick[0],
                config.instructions_per_tick[1],
            )?,
        };
        let mut board = graph.build(
            factory::Options::new(match config.engine {
                Engine::Unicorn => factory::Engine::Unicorn,
                Engine::Hypervisor => factory::Engine::Hypervisor,
            }),
            bus,
            config.interrupts.map(Gic::runtime).transpose()?,
        )?;
        for (core, controller) in controllers.into_iter().enumerate() {
            if let Some(c) = controller {
                board.configure_aprofile(core, c)?;
            }
        }
        for (address, image) in images {
            board.write_memory(0, address, &image)?;
        }
        Ok(Self {
            board,
            quantum: config.quantum,
            input,
            input_source,
            output_directory: config.output_directory,
            archive: 0,
            unmodeled_devices,
            instruction_points: None,
        })
    }
    fn state(&self) -> Value {
        json!({"board":self.board.state(),"unmodeled_devices":self.unmodeled_devices})
    }
    fn command(&mut self, command: Command) -> Result<Value, String> {
        match command {
            Command::Create { .. } => Err("worker already owns a board".into()),
            Command::Run { until } => {
                let result = self.board.run_until(until, self.quantum);
                Ok(
                    json!({"now":self.board.now(),"failure":result.err(),"console":self.board.take_console()}),
                )
            }
            Command::RunInstructions { count } => {
                let result = self.board.run_for_instructions(count, self.quantum);
                Ok(
                    json!({"now":self.board.now(),"instructions":self.board.instruction_counts(),"failure":result.err(),"console":self.board.take_console()}),
                )
            }
            Command::Progress => {
                Ok(json!({"now":self.board.now(),"instructions":self.board.instruction_counts()}))
            }
            Command::InstructionPoints { core, addresses } => {
                self.board.cpu(core)?;
                if let Some((previous, mut points)) = self.instruction_points.take() {
                    self.board.instruction_points_end(previous, &mut points)?;
                }
                if !addresses.is_empty() {
                    self.instruction_points = Some((core, self.board.instruction_points_begin(core, addresses)?));
                }
                Ok(Value::Null)
            }
            Command::InstructionEvent => Ok(self.instruction_points.as_ref().and_then(|(core, points)| {
                points.event().map(|address| json!({"core":core,"address":address}))
            }).unwrap_or(Value::Null)),
            Command::State => Ok(self.state()),
            Command::ReadMemory {
                core,
                address,
                size,
                virtual_address,
            } => {
                if size > 16 * 1024 * 1024 {
                    return Err("memory read exceeds16MiB".into());
                }
                let mut out = vec![0; size];
                if virtual_address {
                    self.board
                        .cpu(core)?
                        .read_virtual(address, Access::Read, &mut out)?;
                } else {
                    self.board.read_memory(core, address, &mut out)?;
                }
                Ok(json!({"hex":hex(&out)}))
            }
            Command::WriteMemory { core, address, hex } => {
                self.board.write_memory(core, address, &bytes(&hex)?)?;
                Ok(Value::Null)
            }
            Command::ReadRegister { core, name } => {
                Ok(json!(self.board.read_register(core, register(&name)?)?))
            }
            Command::WriteRegister { core, name, value } => {
                self.board.write_register(core, register(&name)?, value)?;
                Ok(Value::Null)
            }
            Command::Translate {
                core,
                address,
                access: kind,
            } => Ok(json!(
                self.board.cpu(core)?.translate(address, access(&kind)?)?
            )),
            Command::Input { key, pressed, at } => {
                let at = at.unwrap_or(self.board.now());
                if at < self.board.now() {
                    return Err("input edge precedes current guest time".into());
                }
                self.input
                    .enqueue(Duration::from_nanos(at), Key::parse(&key)?, pressed)?;
                self.board.inputs_changed(self.input_source)?;
                Ok(json!({"at":at}))
            }
            Command::Frames => {
                let mut frames = vec![];
                for frame in self.board.take_frames() {
                    std::fs::create_dir_all(&self.output_directory).map_err(|e| e.to_string())?;
                    let path = loop {
                        self.archive = self
                            .archive
                            .checked_add(1)
                            .ok_or("frame archive counter overflow")?;
                        let path = self
                            .output_directory
                            .join(format!("output{}-{:06}.rgba", frame.output, self.archive));
                        if !path.exists() {
                            break path;
                        }
                    };
                    std::fs::write(&path, &frame.rgba).map_err(|e| e.to_string())?;
                    frames.push(json!({"output":frame.output,"sequence":frame.sequence,"width":frame.width,"height":frame.height,"format":"rgba8888","file":path}));
                }
                Ok(json!(frames))
            }
            Command::Close => {
                self.board.finish()?;
                Ok(Value::Null)
            }
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    let mut worker = None;
    let mut finished = false;
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let line = line?;
        let request = serde_json::from_str::<Command>(&line).map_err(|e| e.to_string());
        let close = matches!(request, Ok(Command::Close));
        let result = request.and_then(|command| {
            if let Some(worker) = worker.as_mut() {
                Worker::command(worker, command)
            } else if let Command::Create { config } = command {
                let created = Worker::new(*config)?;
                let state = created.state();
                worker = Some(created);
                Ok(state)
            } else {
                Err("worker has no board".into())
            }
        });
        let reply = match result {
            Ok(result) => json!({"ok":true,"result":result}),
            Err(error) => json!({"ok":false,"error":error}),
        };
        serde_json::to_writer(&mut output, &reply)?;
        writeln!(output)?;
        output.flush()?;
        if close {
            finished = true;
            break;
        }
    }
    if !finished && let Some(mut worker) = worker {
        worker.board.finish().map_err(io::Error::other)?;
    }
    Ok(())
}
