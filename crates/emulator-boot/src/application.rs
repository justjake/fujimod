//! Application-profile startup plans with caller-selected firmware and media.
use crate::{graph::ProcessorStart, xpp};
use fujimod_catalog_configuration::{RunEngine, RunInputs};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::bus::Mode;
use fujimod_emulator_cpu::Register;
use fujimod_emulator_devices::graphics::scanout::Frame;
use fujimod_emulator_session::Session;
use fujimod_emulator_xp4_board as xp4;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::{Path, PathBuf}};

/// A checked emulation plan and explicit update file.
pub struct Startup {
    pub update: PathBuf,
    pub plan: PathBuf,
    pub discovery: bool,
    pub quantum: Option<usize>,
    pub card_image: Option<PathBuf>,
    pub card_files: Vec<(String, PathBuf)>,
}

/// Application startup facts. File references are relative to the plan's directory.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    configuration_id: String,
    flash: Option<PathBuf>,
    #[serde(default)]
    images: Vec<Image>,
    #[serde(default)]
    card_files: BTreeMap<String, PathBuf>,
    board: Plan,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Image {
    address: u64,
    file: PathBuf,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Plan {
    Xp4 { definition: Value },
    Xpp {
        registers: Vec<Vec<(Register, u64)>>,
        #[serde(default)]
        sub_cpu_registers: Vec<(u8, u8)>,
        #[serde(default)]
        assets: BTreeMap<String, PathBuf>,
    },
}

pub enum Board {
    Xp4(Box<xp4::Session>),
    Xpp(Box<Session>),
}

impl crate::host::Memory for Board {
    fn read(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        match self {
            Self::Xp4(board) => board.read(usize::try_from(address).map_err(|_| "physical address exceeds host size")?, bytes),
            Self::Xpp(session) => session.board.inspect_physical(address, bytes),
        }
    }

    fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        match self {
            Self::Xp4(board) => board.write_patch(usize::try_from(address).map_err(|_| "physical address exceeds host size")?, bytes),
            Self::Xpp(session) => session.board.write_physical(address, bytes),
        }
    }
}

impl crate::mod_loader::Machine for Board {
    fn arm32_core_count(&self) -> usize {
        match self {
            Self::Xp4(board) => board.instructions().len(),
            Self::Xpp(session) => crate::mod_loader::BoardMachine(&session.board).arm32_core_count(),
        }
    }

    fn arm32_translate(&self, core: usize, address: u64, access: fujimod_emulator_cpu::Access) -> Result<u64, String> {
        match self {
            Self::Xp4(board) => board.translate(core, address, access),
            Self::Xpp(session) => crate::mod_loader::BoardMachine(&session.board).arm32_translate(core, address, access),
        }
    }

    fn arm32_mmu_enabled(&self, core: usize) -> Result<bool, String> {
        match self {
            Self::Xp4(board) => {
                let sctlr = fujimod_emulator_cpu::arm::Coprocessor {
                    cp: 15, crn: 1, crm: 0, opc1: 0, opc2: 0, wide: false, secure: 0,
                };
                Ok(board.coprocessor(core, sctlr, None)? & 1 != 0)
            }
            Self::Xpp(session) => crate::mod_loader::BoardMachine(&session.board).arm32_mmu_enabled(core),
        }
    }
}

impl Board {
    pub fn gpio_input(&mut self, port: u8, pin: u8, high: bool) -> Result<(), String> {
        match self {
            Self::Xp4(board) => board.set_gpio_input(usize::from(port), usize::from(pin), high),
            Self::Xpp(session) => fujimod_emulator_xpp::board::gpio_input(session, port, pin, high),
        }
    }

    pub fn set_scene(&mut self, scene: &Value) -> Result<(), String> {
        match self {
            Self::Xp4(board) => board.set_scene(scene),
            Self::Xpp(session) => session.board.set_scene(scene),
        }
    }
}

pub struct Started {
    pub board: Board,
    pub quantum: usize,
    pub ticks_per_second: u64,
    pub engine: RunEngine,
    snapshot_identity: Option<String>,
}

impl Started {
    pub fn progress(&self) -> (Vec<u64>, u64) {
        match &self.board {
            Board::Xp4(board) => (board.instructions(), board.now()),
            Board::Xpp(session) => (session.board.instruction_counts(), session.board.now()),
        }
    }

    pub fn load_snapshot(&mut self, path: &Path) -> Result<(), String> {
        match &mut self.board {
            Board::Xp4(board) => {
                let identity = self.snapshot_identity.as_deref().ok_or("XP4 startup identity is absent")?;
                let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
                board.restore_snapshot(identity, &bytes)
            }
            Board::Xpp(session) => crate::load_snapshot(session, path),
        }
    }

    pub fn save_snapshot(&self, path: &Path) -> Result<(), String> {
        match &self.board {
            Board::Xp4(board) => {
                let identity = self.snapshot_identity.as_deref().ok_or("XP4 startup identity is absent")?;
                let bytes = board.export_snapshot(identity)?;
                std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
            }
            Board::Xpp(session) => crate::save_snapshot(session, path),
        }
    }

    pub fn display_frames(&self) -> Vec<Frame> {
        match &self.board {
            Board::Xp4(board) => board.scanout_frames().into_iter().map(|frame| Frame {
                output: frame.output,
                sequence: frame.sequence,
                width: frame.width,
                height: frame.height,
                rgba: frame.rgba,
            }).collect(),
            Board::Xpp(session) => session.board.display_frames(),
        }
    }

    /// Drive each scheduler window to the next requested host observation or stop.
    pub fn run_with(
        &mut self,
        instruction_limit: Option<u64>,
        duration: Option<std::time::Duration>,
        mut observe: impl FnMut(&mut Self) -> Result<Option<u64>, String>,
    ) -> Result<Value, String> {
        if instruction_limit.is_none() && duration.is_none() {
            return Err("application startup requires an instruction or time limit".into());
        }
        let (counts, now) = self.progress();
        let limits = crate::host::Limits::from_boundary(now, &counts, instruction_limit, duration, self.ticks_per_second)?;
        loop {
            let until = observe(self)?;
            let (before_counts, before_tick) = self.progress();
            let total = before_counts.iter().copied().sum();
            if limits.reached_at(before_tick, total) {
                break;
            }
            let advanced = match &mut self.board {
                Board::Xp4(board) => {
                    let remaining = limits.instructions - total;
                    let until = limits.tick.into_iter().chain(until).min();
                    board.run_board_window(remaining.min(self.quantum as u64), self.quantum, until)?;
                    board.now() != before_tick || board.instructions() != before_counts
                }
                Board::Xpp(session) => limits.step(session, self.quantum, None, until)?,
            };
            if !advanced {
                break;
            }
        }
        let (instructions, now) = self.progress();
        let state = match &self.board {
            Board::Xp4(board) => serde_json::to_value(board.state(false)).map_err(|error| error.to_string())?,
            Board::Xpp(session) => session.board.state(),
        };
        let stubs = match &self.board {
            Board::Xp4(board) => board.discovery_callee_stubs(),
            Board::Xpp(session) => session.board.discovery_callee_stubs(),
        };
        Ok(json!({"engine":self.engine,"instructions":instructions,"now":now,"state":state,"discovery_callee_stubs":stubs}))
    }
}

fn bytes(directory: &Path, path: &Path) -> Result<Vec<u8>, String> {
    let path = directory.join(path);
    let size = std::fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?.len();
    if size > 512 << 20 {
        return Err(format!("{} exceeds the startup asset size bound", path.display()));
    }
    std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))
}

fn named(
    directory: &Path, inputs: BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    inputs.into_iter()
        .map(|(name, path)| Ok((name, bytes(directory, &path)?)))
        .collect()
}

pub(crate) fn open(resolved: &ResolvedConfiguration, startup: Startup) -> Result<Started, String> {
    let update = bytes(Path::new(""), &startup.update)?;
    let identity = resolved.reference.firmware.update.as_ref().ok_or("firmware has no update identity")?;
    if update.len() as u64 != identity.size || format!("{:x}", Sha256::digest(&update)) != identity.sha256 {
        return Err("application update differs from the catalog identity".into());
    }
    let document: Document = serde_json::from_slice(&bytes(Path::new(""), &startup.plan)?)
        .map_err(|error| format!("{}: {error}", startup.plan.display()))?;
    if document.configuration_id != resolved.configuration.id {
        return Err("application startup plan selects another run configuration".into());
    }
    let directory = startup.plan.parent().unwrap_or_else(|| Path::new(""));
    let flash = document.flash.as_deref().map(|path| bytes(directory, path)).transpose()?;
    let mut files = named(directory, document.card_files)?;
    for (name, bytes) in crate::media::read_files(&startup.card_files)? {
        if files.insert(name.clone(), bytes).is_some() {
            return Err(format!("SD card file {name} supplied twice"));
        }
    }
    let card_image = startup.card_image.as_deref().map(crate::media::read_image).transpose()?;
    let execution = resolved.configuration.execution.as_ref();
    let engine = execution.map_or(RunEngine::Unicorn, |execution| execution.engine);
    let quantum = startup.quantum.or_else(|| execution.map(|run| run.quantum)).unwrap_or(1000);
    if quantum == 0 {
        return Err("processor quantum must be positive".into());
    }
    let mode = if startup.discovery { Mode::Discovery } else { Mode::Strict };
    let body = &resolved.reference;
    let mut snapshot_identity = None;
    let board = match (body.soc.id.as_str(), document.board) {
        ("xp4", Plan::Xp4 { definition }) => {
            let geometry = json!({
                "configuration": resolved.configuration, "definition": definition,
                "images": document.images.iter().map(|image| (image.address, &image.sha256)).collect::<Vec<_>>(),
                "flash": flash.as_ref().map(|bytes| format!("{:x}", Sha256::digest(bytes))),
                "sd_card_files": files.iter().map(|(name, bytes)| (name, format!("{:x}", Sha256::digest(bytes)))).collect::<BTreeMap<_, _>>(),
            });
            snapshot_identity = Some(format!("{:x}", Sha256::digest(
                serde_json::to_vec(&geometry).map_err(|error| error.to_string())?
            )));
            let mut definition: xp4::Definition = serde_json::from_value(definition)
                .map_err(|error| format!("XP4 board definition: {error}"))?;
            fujimod_emulator::board::factory::validate_engine(engine, [fujimod_emulator_cpu::Model::CortexA7])?;
            if definition.cores != body.soc.processors.len() {
                return Err("XP4 startup plan core count differs from the selected SoC".into());
            }
            definition.discovery = startup.discovery;
            let files = files.into_iter().collect::<Vec<_>>();
            let board = match card_image {
                Some(medium) => xp4::Session::new_with_media(definition, flash, Vec::new(),
                    vec![(0, fujimod_emulator_session::card::merge_files(medium, &files)?)])?,
                None => xp4::Session::new(definition, flash, files)?,
            };
            Board::Xp4(Box::new(board))
        }
        ("xpp", Plan::Xpp { registers, sub_cpu_registers, assets }) => {
            if registers.len() != body.soc.processors.len() {
                return Err("X-Processor Pro startup plan core count differs from the selected SoC".into());
            }
            let starts = body.soc.processors.iter().zip(registers).map(|(processor, registers)| {
                ProcessorStart { processor_id: processor.id.clone(), released: true, registers }
            }).collect::<Vec<_>>();
            let mut assets = named(directory, assets)?;
            let assembly = xpp::assemble(resolved, &xpp::Inputs {
                starts: &starts, sub_cpu_registers: &sub_cpu_registers, assets: &assets, sensor_image: None,
            })?;
            assets.insert(assembly.flash_asset.into(), flash.ok_or("X-Processor Pro requires flash backing")?);
            let mut devices = assembly.devices;
            let mut files = files.into_iter().collect::<Vec<_>>();
            if card_image.is_none() {
                install_card_files(&mut devices, &mut assets, std::mem::take(&mut files))?;
            }
            let definition = json!({
                "configuration_id": resolved.configuration.id,
                "firmware_id": resolved.configuration.firmware_id,
                "engine": engine,
                "quantum": quantum,
                "mode": mode.name(),
                "execution": crate::graph::value(
                    &resolved.reference, &assembly.execution,
                    fujimod_catalog_configuration::RunInstructionRate {
                        numerator: assembly.rate.0,
                        denominator: assembly.rate.1,
                    },
                ),
                "devices": &devices,
            });
            let spec = serde_json::from_value(devices).map_err(|error| error.to_string())?;
            let mut board = fujimod_emulator_xpp::board::build(assembly.execution, spec, &assets, mode, engine)?
                .into_session(definition)?;
            if let Some(medium) = card_image {
                let slot = board.contacts.keys().next().cloned().ok_or("board has no modeled SD card slot")?;
                board.install_card_files(&slot, Some(medium), &files)?;
            }
            Board::Xpp(Box::new(board))
        }
        _ => return Err("application startup plan does not match the selected processor".into()),
    };
    for image in document.images {
        let data = bytes(directory, &image.file)?;
        if format!("{:x}", Sha256::digest(&data)) != image.sha256 {
            return Err(format!("{} differs from its declared SHA-256", image.file.display()));
        }
        match &board {
            Board::Xp4(board) => board.write(usize::try_from(image.address).map_err(|_| "RAM image address exceeds host size")?, &data)?,
            Board::Xpp(session) => session.board.write_memory(0, image.address, &data)?,
        }
    }
    let ticks_per_second = match resolved.configuration.inputs.as_ref() {
        Some(RunInputs::Xp4(inputs)) => inputs.ticks_per_second,
        _ => 54_000_000,
    };
    Ok(Started { board, quantum, ticks_per_second, engine, snapshot_identity })
}

fn install_card_files(
    devices: &mut Value, assets: &mut BTreeMap<String, Vec<u8>>, files: Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let host = devices["nodes"].as_array_mut().into_iter().flatten()
        .find(|node| node["kind"] == "sd")
        .ok_or("X-Processor Pro startup has no modeled SD card host")?;
    let mut entries = BTreeMap::new();
    for (index, (name, bytes)) in files.into_iter().enumerate() {
        let key = format!("startup-sd-file-{index}");
        if assets.insert(key.clone(), bytes).is_some() {
            return Err("SD card file asset identity collides with another input".into());
        }
        entries.insert(name, key);
    }
    host["files"] = json!(entries);
    Ok(())
}
