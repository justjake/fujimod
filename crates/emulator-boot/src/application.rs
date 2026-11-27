//! Application-profile startup plans with caller-selected firmware and media.
use crate::host::Memory;
use crate::{graph::ProcessorStart, xpp};
use fujimod_catalog_configuration::{RunEngine, RunInputs};
use fujimod_catalog_controls::facts::ConfigurationLocation;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator::board::bus::Mode;
use fujimod_emulator_controls::Calibration;
use fujimod_emulator_cpu::Register;
use fujimod_emulator_devices::graphics::scanout::Frame;
use fujimod_emulator_session::Session;
use fujimod_emulator_xp4_board as xp4;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// A checked emulation plan and explicit update file.
pub struct Startup {
    pub update: crate::FirmwareFile,
    pub plan: PathBuf,
    pub discovery: bool,
    pub quantum: Option<usize>,
    pub card_image: Option<PathBuf>,
    pub card_files: Vec<(String, PathBuf)>,
    /// Settings to place in the plan's NOR journal before boot (XP4); empty places nothing.
    pub camera_settings: BTreeMap<String, String>,
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
    /// Each core's reset registers and the entry labels the firmware image
    /// adds; the board itself comes from the catalog.
    Xp4 {
        registers: Vec<Vec<(Register, u64)>>,
        #[serde(default)]
        labels: Vec<u64>,
    },
    Xpp {
        registers: Vec<Vec<(Register, u64)>>,
        #[serde(default)]
        sub_cpu_registers: Vec<(u8, u8)>,
        #[serde(default)]
        assets: BTreeMap<String, PathBuf>,
    },
}

/// An application-profile board: a shared session for every processor.
pub enum Board {
    Shared {
        session: Box<Session>,
        xp4_inputs: Option<xp4::shared::Inputs>,
    },
}

impl crate::host::Memory for Board {
    fn read(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        match self {
            Self::Shared { session, .. } => session.board.inspect_physical(address, bytes),
        }
    }

    fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        match self {
            Self::Shared { session, .. } => session.board.write_physical(address, bytes),
        }
    }
}

impl crate::mod_loader::Machine for Board {
    fn arm32_core_count(&self) -> usize {
        match self {
            Self::Shared { session, .. } => {
                crate::mod_loader::BoardMachine(&session.board).arm32_core_count()
            }
        }
    }

    fn arm32_translate(
        &self,
        core: usize,
        address: u64,
        access: fujimod_emulator_cpu::Access,
    ) -> Result<u64, String> {
        match self {
            Self::Shared { session, .. } => crate::mod_loader::BoardMachine(&session.board)
                .arm32_translate(core, address, access),
        }
    }

    fn arm32_mmu_enabled(&self, core: usize) -> Result<bool, String> {
        match self {
            Self::Shared { session, .. } => {
                crate::mod_loader::BoardMachine(&session.board).arm32_mmu_enabled(core)
            }
        }
    }
}

/// The electrical input a named key drives on an application-profile board.
pub enum KeyWire {
    Digital(fujimod_catalog_wiring::DigitalKey),
    Analog(fujimod_catalog_wiring::AnalogKey),
    /// One step of a command dial; a press turns it, a release does nothing.
    Dial(fujimod_catalog_wiring::DialStep),
}

impl KeyWire {
    /// The direct GPIO contact, single ADC band or command-dial step the catalog
    /// declares for a key.
    pub fn resolve(
        selection: &fujimod_catalog::reference::ResolvedSelection,
        name: &str,
    ) -> Result<Self, String> {
        if let Some(key) = fujimod_catalog_wiring::digital_key(selection, name)? {
            return Ok(Self::Digital(key));
        }
        if let Some(key) = fujimod_catalog_wiring::analog_key(selection, name)? {
            return Ok(Self::Analog(key));
        }
        fujimod_catalog_wiring::dial_step(selection, name)?
            .map(Self::Dial)
            .ok_or_else(|| format!("{name} has no declared GPIO contact, ADC band or dial step"))
    }
}

/// The configuration block the firmware running on a board has loaded.
struct LoadedConfiguration<'a> {
    board: &'a Board,
    base: u64,
}

impl<'a> LoadedConfiguration<'a> {
    fn new(board: &'a Board, location: ConfigurationLocation) -> Result<Self, String> {
        let base = match location {
            ConfigurationLocation::Direct { address } => address,
            ConfigurationLocation::Pointer { address } => {
                let mut word = [0; 4];
                board.read(address, &mut word)?;
                u64::from(u32::from_le_bytes(word))
            }
        };
        if base == 0 {
            return Err("the firmware has not loaded its configuration yet".into());
        }
        Ok(Self { board, base })
    }
}

impl Calibration for LoadedConfiguration<'_> {
    fn bytes(&self, offset: u32, length: usize) -> Option<Vec<u8>> {
        let mut bytes = vec![0; length];
        self.board
            .read(self.base.checked_add(u64::from(offset))?, &mut bytes)
            .ok()?;
        Some(bytes)
    }
}

impl Board {
    pub fn gpio_input(&mut self, port: u8, pin: u8, high: bool) -> Result<(), String> {
        match self {
            Self::Shared { session, .. } => {
                fujimod_emulator_xpp::board::gpio_input(session, port, pin, high)
            }
        }
    }

    pub fn adc_input(&mut self, channel: u8, sample: u16) -> Result<(), String> {
        match self {
            Self::Shared { session, .. } => {
                use fujimod_emulator_devices::capability::AnalogInputs;
                let node = session
                    .node("xp4-adc")
                    .or_else(|_| session.node("xpp-adc"))
                    .map_err(|_| "XP4 ADC inputs are not wired")?;
                session
                    .board
                    .edit_capability::<dyn AnalogInputs, _>(node, |adc| {
                        adc.set_sample(usize::from(channel), sample)
                    })
            }
        }
    }

    /// Turn a command dial the board declares by `edges` quadrature edges.
    pub fn turn_dial(&mut self, name: &str, edges: i32) -> Result<(), String> {
        match self {
            Self::Shared {
                session,
                xp4_inputs,
            } => xp4_inputs
                .as_ref()
                .ok_or("X-Processor Pro command dials are not wired")?
                .turn_dial(session, name, edges),
        }
    }

    /// Present a key pressed or released. ADC bands whose bounds live in the
    /// firmware's configuration are read from the block the firmware has loaded.
    pub fn key_input(&mut self, wire: &KeyWire, pressed: bool) -> Result<(), String> {
        match wire {
            KeyWire::Digital(key) => self.gpio_input(key.port, key.pin, key.active == pressed),
            KeyWire::Analog(key) => {
                let calibration = key
                    .calibration
                    .map(|location| LoadedConfiguration::new(self, location))
                    .transpose()?;
                let (down, up) = fujimod_emulator_controls::adc_levels(
                    key,
                    calibration.as_ref().map(|c| c as &dyn Calibration),
                )?;
                self.adc_input(key.channel, if pressed { down } else { up })
            }
            // One firmware step is one edge, as the X-T3 discovery inputs declare.
            KeyWire::Dial(step) if pressed => {
                self.turn_dial(&step.control, i32::from(step.direction))
            }
            KeyWire::Dial(_) => Ok(()),
        }
    }

    pub fn set_scene(&mut self, scene: &Value) -> Result<(), String> {
        match self {
            Self::Shared { session, .. } => session.board.set_scene(scene),
        }
    }
}

pub struct Started {
    pub board: Board,
    /// The `--card-image` file and what it held, to write the changed sectors back.
    card_image: Option<CardImage>,
    pub quantum: usize,
    pub ticks_per_second: u64,
    pub engine: RunEngine,
    /// The journal copies the requested camera settings changed; null without a request.
    pub camera_settings: Value,
}

impl crate::host::Execution for Started {
    fn progress(&self) -> (Vec<u64>, u64) {
        Started::progress(self)
    }

    fn run_window(
        &mut self,
        end: u64,
        quantum: usize,
        pause_after: Option<u64>,
        until: Option<u64>,
    ) -> Result<(), String> {
        match &mut self.board {
            Board::Shared { session, .. } => session.run_window(end, quantum, pause_after, until),
        }
    }
}

impl Started {
    pub fn progress(&self) -> (Vec<u64>, u64) {
        match &self.board {
            Board::Shared { session, .. } => {
                (session.board.instruction_counts(), session.board.now())
            }
        }
    }

    pub fn load_snapshot(&mut self, path: &Path) -> Result<(), String> {
        match &mut self.board {
            Board::Shared { session, .. } => crate::load_snapshot(session, path),
        }
    }

    pub fn save_snapshot(&self, path: &Path) -> Result<(), String> {
        match &self.board {
            Board::Shared { session, .. } => crate::save_snapshot(session, path),
        }
    }

    /// Write the sectors of the card that differ from the `--card-image` file back into it,
    /// as XP5 loader starts do. Returns the number of sectors written, or `None` without an image.
    pub fn write_back_card(&self) -> Result<Option<usize>, String> {
        self.card_image
            .as_ref()
            .map(|image| image.write_back(&self.board))
            .transpose()
    }

    pub fn display_frames(&self) -> Vec<Frame> {
        match &self.board {
            Board::Shared { session, .. } => session.board.display_frames(),
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
        let limits =
            crate::host::Limits::new(self, instruction_limit, duration, self.ticks_per_second)?;
        limits.drive(
            self,
            self.quantum,
            |board| observe(board).map(std::ops::ControlFlow::Continue),
            |_, _| Ok(()),
        )?;
        let (instructions, now) = self.progress();
        let state = match &self.board {
            Board::Shared { session, .. } => session.board.state(),
        };
        let stubs = match &self.board {
            Board::Shared { session, .. } => session.board.discovery_callee_stubs(),
        };
        Ok(
            json!({"engine":self.engine,"instructions":instructions,"now":now,"state":state,"discovery_callee_stubs":stubs}),
        )
    }
}

fn bytes(directory: &Path, path: &Path) -> Result<Vec<u8>, String> {
    let path = directory.join(path);
    let size = std::fs::metadata(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .len();
    if size > 512 << 20 {
        return Err(format!(
            "{} exceeds the startup asset size bound",
            path.display()
        ));
    }
    std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))
}

fn named(
    directory: &Path,
    inputs: BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    inputs
        .into_iter()
        .map(|(name, path)| Ok((name, bytes(directory, &path)?)))
        .collect()
}

pub fn open(resolved: &ResolvedConfiguration, startup: Startup) -> Result<Started, String> {
    let update = startup.update.read(crate::firmware_file::MAX_BYTES)?;
    let identity = resolved
        .reference
        .firmware
        .update
        .as_ref()
        .ok_or("firmware has no update identity")?;
    if update.len() as u64 != identity.size
        || format!("{:x}", Sha256::digest(&update)) != identity.sha256
    {
        return Err("application update differs from the catalog identity".into());
    }
    let document: Document = serde_json::from_slice(&bytes(Path::new(""), &startup.plan)?)
        .map_err(|error| format!("{}: {error}", startup.plan.display()))?;
    if document.configuration_id != resolved.configuration.id {
        return Err("application startup plan selects another run configuration".into());
    }
    let directory = startup.plan.parent().unwrap_or_else(|| Path::new(""));
    let mut flash = document
        .flash
        .as_deref()
        .map(|path| bytes(directory, path))
        .transpose()?;
    let camera_settings = if startup.camera_settings.is_empty() {
        Value::Null
    } else {
        if resolved.reference.soc.id != "xp4" {
            return Err("camera settings need an XP4 application-profile plan".into());
        }
        let nor = flash
            .as_mut()
            .ok_or("camera settings need the plan's flash")?;
        crate::journal_settings::place(resolved, nor, &startup.camera_settings)?
    };
    let mut files = named(directory, document.card_files)?;
    for (name, bytes) in crate::media::read_files(&startup.card_files)? {
        if files.insert(name.clone(), bytes).is_some() {
            return Err(format!("SD card file {name} supplied twice"));
        }
    }
    let card_image = startup
        .card_image
        .map(|path| {
            let original = crate::media::read_image(&path)?;
            Ok::<_, String>(CardImage { path, original })
        })
        .transpose()?;
    let execution = resolved.configuration.execution.as_ref();
    let engine = execution.map_or(RunEngine::Unicorn, |execution| execution.engine);
    let quantum = startup
        .quantum
        .or_else(|| execution.map(|run| run.quantum))
        .unwrap_or(1000);
    if quantum == 0 {
        return Err("processor quantum must be positive".into());
    }
    let mode = if startup.discovery {
        Mode::Discovery
    } else {
        Mode::Strict
    };
    let body = &resolved.reference;
    let board = match (body.soc.id.as_str(), document.board) {
        ("xp4", Plan::Xp4 { registers, labels }) => {
            fujimod_emulator::board::factory::validate_engine(
                engine,
                [fujimod_emulator_cpu::Model::CortexA7],
            )?;
            let files = files.into_iter().collect::<Vec<_>>();
            let (card_files, media) = match &card_image {
                Some(image) => (
                    Vec::new(),
                    vec![(
                        0,
                        fujimod_emulator_session::card::merge_files(
                            image.original.clone(),
                            &files,
                        )?,
                    )],
                ),
                None => (files, Vec::new()),
            };
            let (session, xp4_inputs) = xp4_session(
                resolved,
                Xp4Inputs {
                    registers,
                    labels: &labels,
                    flash,
                    card_files,
                    media,
                    mode,
                    engine,
                    quantum,
                },
            )?;
            Board::Shared {
                session: Box::new(session),
                xp4_inputs: Some(xp4_inputs),
            }
        }
        (
            "xpp",
            Plan::Xpp {
                registers,
                sub_cpu_registers,
                assets,
            },
        ) => {
            if registers.len() != body.soc.processors.len() {
                return Err(
                    "X-Processor Pro startup plan core count differs from the selected SoC".into(),
                );
            }
            let starts = body
                .soc
                .processors
                .iter()
                .zip(registers)
                .map(|(processor, registers)| ProcessorStart {
                    processor_id: processor.id.clone(),
                    released: true,
                    parked_until_sgi: false,
                    registers,
                })
                .collect::<Vec<_>>();
            let mut assets = named(directory, assets)?;
            let assembly = xpp::assemble(
                resolved,
                &xpp::Inputs {
                    starts: &starts,
                    sub_cpu_registers: &sub_cpu_registers,
                    assets: &assets,
                    sensor_image: None,
                },
            )?;
            assets.insert(
                assembly.flash_asset.into(),
                flash.ok_or("X-Processor Pro requires flash backing")?,
            );
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
            let mut board = fujimod_emulator_xpp::board::build(
                assembly.execution,
                spec,
                &assets,
                mode,
                engine,
            )?
            .into_session(definition)?;
            if let Some(image) = &card_image {
                let slot = board
                    .contacts
                    .keys()
                    .next()
                    .cloned()
                    .ok_or("board has no modeled SD card slot")?;
                board.install_card_files(&slot, Some(image.original.clone()), &files)?;
            }
            Board::Shared {
                session: Box::new(board),
                xp4_inputs: None,
            }
        }
        _ => return Err("application startup plan does not match the selected processor".into()),
    };
    for image in document.images {
        let data = bytes(directory, &image.file)?;
        if format!("{:x}", Sha256::digest(&data)) != image.sha256 {
            return Err(format!(
                "{} differs from its declared SHA-256",
                image.file.display()
            ));
        }
        match &board {
            Board::Shared { session, .. } => session.board.write_memory(0, image.address, &data)?,
        }
    }
    let ticks_per_second = match resolved.configuration.inputs.as_ref() {
        Some(RunInputs::Xp4(inputs)) => inputs.ticks_per_second,
        _ => 54_000_000,
    };
    Ok(Started {
        board,
        card_image,
        quantum,
        ticks_per_second,
        engine,
        camera_settings,
    })
}

/// The caller-owned inputs of an XP4 board: firmware-side reset state,
/// entry labels, flash contents and SD card media.
struct Xp4Inputs<'a> {
    registers: Vec<Vec<(Register, u64)>>,
    labels: &'a [u64],
    flash: Option<Vec<u8>>,
    card_files: Vec<(String, Vec<u8>)>,
    media: Vec<(usize, fujimod_emulator_devices::parts::sd_card::Card)>,
    mode: Mode,
    engine: RunEngine,
    quantum: usize,
}

/// An XP4 run on the shared board. The board comes from the catalog; the
/// declared instruction rate is each core's.
fn xp4_session(
    resolved: &ResolvedConfiguration,
    inputs: Xp4Inputs<'_>,
) -> Result<(Session, xp4::shared::Inputs), String> {
    let processors = &resolved.reference.soc.processors;
    if inputs.registers.len() != processors.len() {
        return Err("XP4 startup plan core count differs from the selected SoC".into());
    }
    let starts: Vec<ProcessorStart> = processors
        .iter()
        .zip(&inputs.registers)
        .map(|(processor, registers)| ProcessorStart {
            processor_id: processor.id.clone(),
            released: true,
            parked_until_sgi: false,
            registers: registers.clone(),
        })
        .collect();
    let definition = xp4::catalog::definition(resolved, inputs.registers, inputs.labels)?;
    let rate: (u64, u64) = serde_json::from_value(definition["instructions_per_tick"].clone())
        .map_err(|error| format!("XP4 instruction rate: {error}"))?;
    let rate = fujimod_catalog_configuration::RunInstructionRate {
        numerator: rate.0,
        denominator: rate.1,
    };
    let execution = crate::graph::assemble_with_starts(
        &resolved.reference,
        resolved.dram_address_space_limit_bytes()?,
        rate,
        None,
        &starts,
    )?;
    let record = json!({
        "configuration_id": resolved.configuration.id,
        "firmware_id": resolved.configuration.firmware_id,
        "engine": inputs.engine,
        "quantum": inputs.quantum,
        "mode": inputs.mode.name(),
        "execution": crate::graph::value(&resolved.reference, &execution, rate),
        "devices": &definition,
    });
    let definition = serde_json::from_value(definition)
        .map_err(|error| format!("XP4 board definition: {error}"))?;
    xp4::shared::build(
        execution,
        definition,
        inputs.flash,
        inputs.card_files,
        inputs.media,
        inputs.mode,
        inputs.engine,
        record,
        xp4::shared::DeviceSpace::Declared,
    )
}

/// A `--card-image` file and the medium read from it, before the caller's files were merged.
struct CardImage {
    path: PathBuf,
    original: fujimod_emulator_devices::parts::sd_card::Card,
}

impl CardImage {
    fn write_back(&self, board: &Board) -> Result<usize, String> {
        let current = match board {
            Board::Shared { session, .. } => match session.contacts.keys().next() {
                Some(name) => session.card_medium(name)?,
                None => xp4::shared::card_medium(session, 0)?,
            },
        };
        self.write_back_medium(&current)
    }

    fn write_back_medium(
        &self,
        current: &fujimod_emulator_devices::parts::sd_card::Card,
    ) -> Result<usize, String> {
        let written = crate::media::write_back(&self.path, &self.original, current)?;
        eprintln!(
            "FUJIMOD card image {}: {written} changed sectors written back",
            self.path.display()
        );
        Ok(written)
    }
}

fn install_card_files(
    devices: &mut Value,
    assets: &mut BTreeMap<String, Vec<u8>>,
    files: Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let host = devices["nodes"]
        .as_array_mut()
        .into_iter()
        .flatten()
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

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_catalog::reference::{ReferenceCatalog, ResolvedSelection};
    use fujimod_emulator_devices::parts::sd_card::{Card, Options, SECTOR};
    use std::{
        fs::File,
        io::{Read, Seek, SeekFrom, Write},
    };

    fn sector_of(file: &mut File, lba: u64) -> Vec<u8> {
        let mut sector = vec![0; SECTOR];
        file.seek(SeekFrom::Start(lba * SECTOR as u64)).unwrap();
        file.read_exact(&mut sector).unwrap();
        sector
    }

    #[test]
    fn the_card_image_takes_the_sectors_the_xp4_card_changed() {
        // The smallest SDHC card: 2 GiB, written sparse.
        let sectors = 0x40_0000u32;
        let length = u64::from(sectors) * SECTOR as u64;
        let path =
            std::env::temp_dir().join(format!("application-card-{}.img", std::process::id()));
        let mut file = File::create(&path).unwrap();
        file.set_len(length).unwrap();
        let mut original = Card::new(Options {
            sectors,
            max_dirty_sectors: 65536,
        })
        .unwrap();
        for (lba, fill) in [(10u64, 0x11u8), (30, 0x44)] {
            file.seek(SeekFrom::Start(lba * SECTOR as u64)).unwrap();
            file.write_all(&[fill; SECTOR]).unwrap();
            original
                .write_bytes(lba * SECTOR as u64, &[fill; SECTOR])
                .unwrap();
        }
        drop(file);
        // What the camera left on the card: one sector rewritten, one written, one erased.
        let mut current = original.clone();
        current
            .write_bytes(10 * SECTOR as u64, &[0x22; SECTOR])
            .unwrap();
        current
            .write_bytes(20 * SECTOR as u64, &[0x33; SECTOR])
            .unwrap();
        current
            .write_bytes(30 * SECTOR as u64, &[0; SECTOR])
            .unwrap();
        let card = CardImage {
            path: path.clone(),
            original,
        };
        assert_eq!(card.write_back_medium(&current).unwrap(), 3);
        let mut file = File::open(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), length);
        for (lba, fill) in [(9, 0), (10, 0x22), (20, 0x33), (30, 0), (31, 0)] {
            assert_eq!(sector_of(&mut file, lba), [fill; SECTOR], "sector {lba}");
        }
        std::fs::remove_file(&path).unwrap();
    }

    fn selection(id: &str) -> ResolvedSelection {
        ReferenceCatalog::bundled()
            .unwrap()
            .resolve_firmware(id)
            .unwrap()
    }

    #[test]
    fn command_dial_steps_resolve_after_gpio_contacts_and_adc_bands() {
        let xt3 = selection("xt3-511");
        assert!(matches!(
            KeyWire::resolve(&xt3, "CMD_RIGHT"),
            Ok(KeyWire::Dial(step)) if step.control == "rear_command_dial" && step.direction == 1
        ));
        assert!(matches!(
            KeyWire::resolve(&xt3, "CMD2_L"),
            Ok(KeyWire::Dial(step)) if step.control == "front_command_dial" && step.direction == -1
        ));
        // Keys that already resolved keep their wires.
        assert!(matches!(
            KeyWire::resolve(&xt3, "xt3-fn1-button"),
            Ok(KeyWire::Digital(_))
        ));
        assert!(matches!(
            KeyWire::resolve(&selection("x70-110"), "EXE_M"),
            Ok(KeyWire::Analog(_))
        ));
        // An unknown name is still refused.
        let error = KeyWire::resolve(&xt3, "CMD_NOWHERE").err().unwrap();
        assert_eq!(
            error,
            "CMD_NOWHERE has no declared GPIO contact, ADC band or dial step"
        );
    }

    #[test]
    fn command_dial_step_schedules_a_shared_xp4_countdown_edge() {
        let definition = serde_json::from_value(json!({
            "ram_size": 65536,
            "cores": 1,
            "discovery": false,
            "cbar": 0x2000_0000,
            "countdown_timers": [0xfffc_1000u32, []],
            "countdown_dials": [{
                "name": "rear_command_dial",
                "channel": 2,
                "phase_ticks": 10,
                "rest_phase": 0
            }]
        }))
        .unwrap();
        let (session, inputs) = xp4::shared::from_definition(
            definition,
            None,
            Vec::new(),
            Vec::new(),
            RunEngine::Unicorn,
        )
        .unwrap();
        let mut board = Board::Shared {
            session: Box::new(session),
            xp4_inputs: Some(inputs),
        };
        let wire = KeyWire::resolve(&selection("xt3-511"), "CMD_RIGHT").unwrap();
        board.key_input(&wire, false).unwrap();
        let Board::Shared { session, .. } = &board;
        assert_eq!(session.board.next_device_deadline(), None);
        board.key_input(&wire, true).unwrap();
        let Board::Shared { session, .. } = &board;
        assert_eq!(session.board.next_device_deadline(), Some(10));
    }
}

#[cfg(test)]
mod shared_board_tests {
    use super::*;

    /// Boots a prepared XP4 application plan on the shared board:
    /// `FUJIMOD_XP4_PLAN` names the plan and `FUJIMOD_XP4_INSTRUCTIONS` the
    /// instruction budget. Skipped without a plan; plans hold firmware bytes.
    #[test]
    fn xp4_plan_runs_on_the_shared_board() {
        let Ok(path) = std::env::var("FUJIMOD_XP4_PLAN") else {
            return;
        };
        let count: u64 = std::env::var("FUJIMOD_XP4_INSTRUCTIONS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(1_000_000);
        let plan = PathBuf::from(path);
        let document: Document = serde_json::from_slice(&std::fs::read(&plan).unwrap()).unwrap();
        let directory = plan.parent().unwrap();
        let resolved = ResolvedConfiguration::bundled(&document.configuration_id).unwrap();
        let Plan::Xp4 { registers, labels } = document.board else {
            panic!("not an XP4 plan")
        };
        let flash = document
            .flash
            .as_deref()
            .map(|path| bytes(directory, path))
            .transpose()
            .unwrap();
        let (session, _) = xp4_session(
            &resolved,
            Xp4Inputs {
                registers,
                labels: &labels,
                flash,
                card_files: Vec::new(),
                media: Vec::new(),
                mode: Mode::Strict,
                engine: RunEngine::Unicorn,
                quantum: 1000,
            },
        )
        .unwrap();
        let mut board = session.board;
        let cores = board.instruction_counts().len();
        for image in &document.images {
            board
                .write_physical(image.address, &bytes(directory, &image.file).unwrap())
                .unwrap();
        }
        let result = board.run_for_instructions(count, 1000);
        let pcs: Vec<String> = (0..cores)
            .map(|core| {
                format!(
                    "{:#x}",
                    board.read_register(core, Register::Pc).unwrap_or(0)
                )
            })
            .collect();
        eprintln!(
            "FUJIMOD shared XP4 {}: {:?} instructions {:?} now {} pcs {:?}",
            document.configuration_id,
            result,
            board.instruction_counts(),
            board.now(),
            pcs
        );
        if let Ok(range) = std::env::var("FUJIMOD_XP4_DUMP") {
            let (address, length) = range.split_once(':').unwrap();
            let address = u64::from_str_radix(address.trim_start_matches("0x"), 16).unwrap();
            let mut bytes = vec![0; length.parse().unwrap()];
            board.read_physical(address, &mut bytes).unwrap();
            let words: Vec<String> = bytes
                .chunks(4)
                .map(|w| format!("{:08x}", u32::from_le_bytes(w.try_into().unwrap())))
                .collect();
            eprintln!("FUJIMOD shared XP4 dump {address:#x}: {}", words.join(" "));
        }
        if let Some(directory) = std::env::var_os("FUJIMOD_XP4_FRAMES") {
            for frame in board.display_frames() {
                let png =
                    crate::png_writer::rgba_png(frame.width, frame.height, &frame.rgba).unwrap();
                std::fs::write(
                    Path::new(&directory).join(format!("output{}.png", frame.output)),
                    png,
                )
                .unwrap();
            }
        }
        if std::env::var_os("FUJIMOD_XP4_STATE").is_some() {
            eprintln!("FUJIMOD shared XP4 state {}", board.state());
        }
        result.unwrap();
    }
}
