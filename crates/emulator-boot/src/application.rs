//! Application-profile startup plans with caller-selected firmware and media.
use crate::{graph::ProcessorStart, xpp};
use fujimod_catalog_configuration::{RunEngine, RunInputs};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::bus::Mode;
use fujimod_emulator_cpu::Register;
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
    Xp4 { definition: Box<xp4::Definition> },
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

pub struct Started {
    pub board: Board,
    pub quantum: usize,
    pub ticks_per_second: u64,
    pub engine: RunEngine,
}

impl Started {
    /// Drive a stopped application-profile board by its declared quantum.
    pub fn run(&mut self, instruction_limit: u64, duration: Option<std::time::Duration>) -> Result<Value, String> {
        let (instructions, ticks, state) = match &mut self.board {
            Board::Xp4(board) => {
                if duration.is_some() {
                    return Err("XP4 currently requires an instruction limit".into());
                }
                board.run_board(instruction_limit, self.quantum)?;
                let state = serde_json::to_value(board.state(false)).map_err(|error| error.to_string())?;
                (board.instructions(), board.now(), state)
            }
            Board::Xpp(session) => {
                let limits = crate::host::Limits::new(session, Some(instruction_limit), duration, self.ticks_per_second)?;
                limits.step(session, self.quantum, None, None)?;
                (session.board.instruction_counts(), session.board.now(), session.board.state())
            }
        };
        Ok(json!({"engine":self.engine,"instructions":instructions,"now":ticks,"state":state}))
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
    let files = named(directory, document.card_files)?;
    let execution = resolved.configuration.execution.as_ref();
    let engine = execution.map_or(RunEngine::Unicorn, |execution| execution.engine);
    let quantum = startup.quantum.or_else(|| execution.map(|run| run.quantum)).unwrap_or(1000);
    if quantum == 0 {
        return Err("processor quantum must be positive".into());
    }
    let mode = if startup.discovery { Mode::Discovery } else { Mode::Strict };
    let body = &resolved.reference;
    let board = match (body.soc.id.as_str(), document.board) {
        ("xp4", Plan::Xp4 { mut definition }) => {
            if engine != RunEngine::Unicorn {
                return Err("XP4 application processors require Unicorn".into());
            }
            if definition.cores != body.soc.processors.len() {
                return Err("XP4 startup plan core count differs from the selected SoC".into());
            }
            definition.discovery = startup.discovery;
            let board = xp4::Session::new(*definition, flash, files.into_iter().collect())?;
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
            install_card_files(&mut devices, &mut assets, files)?;
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
            let board = fujimod_emulator_xpp::board::build(assembly.execution, spec, &assets, mode, engine)?
                .into_session(definition)?;
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
    Ok(Started { board, quantum, ticks_per_second, engine })
}

fn install_card_files(
    devices: &mut Value, assets: &mut BTreeMap<String, Vec<u8>>, files: BTreeMap<String, Vec<u8>>,
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
