//! One emulated camera, owned by its thread.
use std::{collections::BTreeMap, path::PathBuf};

use fujimod_catalog_remote::{Facts, Selection};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot::{self as boot, host};
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_session::{Session, checkpoint::Snapshot};
use fujimod_remote::{BoardRemote, Control, board::Options as RemoteOptions, operations};
use serde_json::{Value, json};

use crate::OpenParams;

/// The `Send` part of an open request, moved to the camera's thread.
pub struct OpenRequest {
    configuration: String,
    overlays: Vec<String>,
    update: PathBuf,
    boot_rom: Option<PathBuf>,
    application_plan: Option<PathBuf>,
    camera_sections: Option<PathBuf>,
    nand: Option<PathBuf>,
    settings: Option<PathBuf>,
    sd_card: Option<PathBuf>,
    gpio: Option<String>,
    keys: Option<String>,
    snapshot: Option<PathBuf>,
    execution_policy: Option<String>,
    instruction_limit: u64,
}

impl From<OpenParams> for OpenRequest {
    fn from(p: OpenParams) -> Self {
        Self {
            configuration: p.configuration,
            overlays: p.overlays,
            update: p.update.into(),
            boot_rom: p.boot_rom.map(Into::into),
            application_plan: p.application_plan.map(Into::into),
            camera_sections: p.camera_sections.map(Into::into),
            nand: p.nand.map(Into::into),
            settings: p.settings.map(Into::into),
            sd_card: p.sd_card.map(Into::into),
            gpio: p.gpio,
            keys: p.keys,
            snapshot: p.snapshot.map(Into::into),
            execution_policy: p.execution_policy,
            instruction_limit: p.instruction_limit.unwrap_or(2_000_000_000),
        }
    }
}

pub struct Camera {
    session: Session,
    resolved: ResolvedConfiguration,
    quantum: usize,
    ticks_per_second: u64,
    checkpoints: BTreeMap<String, Snapshot>,
    profiling: bool,
    /// Firmware facts `fujimod-remote` reads to deliver and confirm inputs,
    /// or why the catalog has none for this firmware.
    facts: Result<Facts, String>,
    /// The last LCD image the camera showed.
    latest: Option<fujimod_emulator_devices::graphics::scanout::Frame>,
    /// The desktop window showing this camera; the guest runs live while it is open.
    shown: Option<Shown>,
}

struct Shown {
    viewer: fujimod_emulator_gui::Viewer,
    window: fujimod_emulator_gui::launch::Window,
    /// Why live running stopped; the window still shows the last image.
    stopped: Option<String>,
    /// When the LCD was last read for the window.
    read_at: std::time::Instant,
}

/// Shortest wall time between LCD reads for the window, about 30 images a second.
const WINDOW_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

/// The window's commands and images reach the camera between guest slices.
impl fujimod_emulator_gui::Run for Camera {
    fn session(&self) -> &Session {
        &self.session
    }
    fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }
    fn ticks_per_second(&self) -> u64 {
        self.ticks_per_second
    }
    fn step(&mut self) -> Result<(), String> {
        // About one LCD frame of guest time per slice keeps the window responsive.
        let until = self.session.board.now() + (self.ticks_per_second / 60).max(1);
        self.session.run_window(u64::MAX, self.quantum, None, Some(until))
    }
    fn settle(&mut self) -> Result<(), String> {
        let until = self.session.board.now() + (self.ticks_per_second / 1000).max(1);
        self.session.run_window(u64::MAX, self.quantum, None, Some(until))
    }
    fn frames(
        &mut self,
    ) -> Result<Vec<fujimod_emulator_devices::graphics::scanout::Frame>, String> {
        Ok(self.collect_frames())
    }
    fn finish(self) -> Result<Value, String> {
        Ok(Value::Null)
    }
}

/// The catalog facts `fujimod-remote` needs for a resolved selection.
fn remote_facts(resolved: &ResolvedConfiguration) -> Result<Facts, String> {
    let selection = &resolved.reference;
    let input = Selection {
        firmware_id: &selection.firmware.id,
        body_id: &selection.body.id,
        firmware_sha256: selection
            .firmware
            .update
            .as_ref()
            .map(|content| content.sha256.as_str()),
        remote_facts: selection
            .firmware
            .generated_facts
            .as_ref()
            .and_then(|facts| facts.remote.as_ref()),
        physical_controls: selection.physical_controls.as_ref(),
        control_ids: selection.control_ids.as_ref(),
        connections: selection
            .body
            .connections
            .iter()
            .map(|connection| connection.id.as_str())
            .collect(),
    };
    Facts::from_selection(&input, None)
}

/// Every file under `directory`, named by its path relative to it.
fn sd_card_files(directory: &PathBuf) -> Result<Vec<(String, PathBuf)>, String> {
    let mut files = Vec::new();
    let mut pending = vec![directory.clone()];
    while let Some(next) = pending.pop() {
        let entries =
            std::fs::read_dir(&next).map_err(|error| format!("{}: {error}", next.display()))?;
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let name = path
                    .strip_prefix(directory)
                    .map_err(|error| error.to_string())?
                    .to_str()
                    .ok_or("SD card file name is not UTF-8")?
                    .replace('\\', "/");
                files.push((name, path));
            }
        }
    }
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "SD card directory {} is empty",
            directory.display()
        ));
    }
    Ok(files)
}

impl Camera {
    pub fn open(request: OpenRequest) -> Result<(Self, (Value, Option<Vec<u8>>)), String> {
        let resolve =
            || ResolvedConfiguration::with_overlays(&request.configuration, &request.overlays);
        let sd_files = request.sd_card.as_ref().map(sd_card_files).transpose()?;
        let opened = boot::open_request(
            resolve()?,
            boot::Request {
                update: request.update.clone(),
                boot_rom: request.boot_rom.clone(),
                nand: request.nand.clone(),
                settings: request.settings.clone(),
                camera: request.camera_sections.clone(),
                application_plan: request.application_plan.clone(),
                card_files: sd_files.unwrap_or_default(),
                discovery: false,
            },
        )?;
        let quantum = opened.quantum;
        let ticks_per_second = opened.ticks_per_second;
        let mut session = opened.session;
        let resolved = resolve()?;
        if let Some(policy) = &request.execution_policy {
            session
                .board
                .set_execution_policy(ExecutionPolicy::parse(policy)?)?;
        }
        let facts = remote_facts(&resolved);
        let mut camera = Self {
            session,
            resolved,
            quantum,
            ticks_per_second,
            checkpoints: BTreeMap::new(),
            profiling: false,
            facts,
            shown: None,
            latest: None,
        };
        let started = std::time::Instant::now();
        if let Some(snapshot) = &request.snapshot {
            if request.gpio.is_some() || request.keys.is_some() {
                return Err("a restored camera takes no boot gpio or keys".into());
            }
            boot::load_snapshot(&mut camera.session, snapshot)?;
        } else if boot::scanout_node(&camera.session).is_err() {
            // The emulator recognizes no REC screen for this camera: it opens
            // at power-on and camera_run advances it.
            if request.gpio.is_some() || request.keys.is_some() {
                return Err("boot gpio and keys need a camera whose REC screen the emulator recognizes".into());
            }
        } else {
            let edges = host::parse_keys(request.keys.as_deref(), None)?;
            let automatic_setup = edges.is_empty();
            let mut keys = fujimod_emulator_controls::Keys::new(
                &camera.resolved.reference,
                edges.into_iter().map(|edge| {
                    (
                        host::duration_ticks(edge.delay, ticks_per_second),
                        edge.name,
                        edge.pressed,
                    )
                }),
                ticks_per_second,
            )?;
            if let Some(levels) = &request.gpio {
                for level in levels.split(',') {
                    let (port, pin, high) = host::parse_gpio(level.trim())?;
                    fujimod_emulator_controls::set_gpio(&mut camera.session, port, pin, high)?;
                }
            }
            let start = camera.session.board.now();
            boot::to_rec_with_actions(
                &mut camera.session,
                request.instruction_limit,
                None,
                automatic_setup,
                |session| keys.advance(session, start),
                |_, _| Ok(()),
            )?;
        }
        // Connect the controls now, so every checkpoint carries them and
        // restores into this camera. A boot that stopped before the firmware
        // loaded its configuration leaves them to connect on first use.
        let controls = camera.connect().err();
        let (mut summary, png) = camera.observe(true)?;
        if let Some(error) = controls {
            summary["controls_error"] = json!(error);
        }
        summary["opened"] = json!({
            "configuration": request.configuration,
            "overlays": request.overlays,
            "sd_card": request.sd_card.map(|path| path.display().to_string()),
            "snapshot": request.snapshot.map(|path| path.display().to_string()),
            "wall_seconds": started.elapsed().as_secs_f64(),
        });
        Ok((camera, (summary, png)))
    }

    /// Open the desktop window on this camera. Until it closes, the guest
    /// runs live between tool calls and the window's input reaches it.
    pub fn show(&mut self) -> Result<Value, String> {
        if self.shown.is_some() {
            return Err("this camera is already shown".into());
        }
        self.connect()?;
        let room = fujimod_emulator_gui::Room::loopback()?;
        let endpoint = room.endpoint().url.clone();
        let mut window = fujimod_emulator_gui::launch::connect(&endpoint)?;
        // The window has 30 seconds to join before the request gives up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let joined = room.join(&mut || std::time::Instant::now() >= deadline || !window.open());
        let viewer = match joined {
            Ok(viewer) => viewer,
            Err(error) => {
                window.close();
                return Err(error);
            }
        };
        self.shown = Some(Shown {
            viewer,
            window,
            stopped: None,
            read_at: std::time::Instant::now(),
        });
        Ok(json!({ "endpoint": endpoint }))
    }

    /// The last published LCD image as PNG.
    fn latest_png(&self) -> Option<Vec<u8>> {
        let frame = self.latest.as_ref()?;
        let rgb: Vec<u8> = frame.rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        fujimod_recording::Frame::rgb(frame.width, frame.height, rgb)
            .ok()?
            .png()
            .ok()
            .map(|png| png.to_vec())
    }

    /// Keep the newest LCD image the camera published.
    fn collect_frames(&mut self) -> Vec<fujimod_emulator_devices::graphics::scanout::Frame> {
        let frames = boot::lcd_frames(&mut self.session).unwrap_or_default();
        if let Some(frame) = frames.iter().max_by_key(|frame| frame.sequence) {
            self.latest = Some(frame.clone());
        }
        frames
    }

    pub fn shown(&self) -> bool {
        self.shown.is_some()
    }

    /// One live slice for the window: its input, a guest slice, its image.
    pub fn serve_window(&mut self) {
        use fujimod_emulator_gui::Run as _;
        let Some(mut shown) = self.shown.take() else {
            return;
        };
        let open = match shown.viewer.service(self) {
            Ok(()) => shown.window.open(),
            Err(error) => {
                eprintln!("camera window: {error}");
                false
            }
        };
        if !open {
            shown.window.close();
            return;
        }
        if shown.stopped.is_none() {
            let before = self.progress();
            let read = shown.read_at.elapsed() >= WINDOW_FRAME_INTERVAL;
            let shown_frames = self
                .step()
                .and_then(|()| if read { self.frames() } else { Ok(Vec::new()) });
            if read {
                shown.read_at = std::time::Instant::now();
            }
            match shown_frames {
                Ok(frames) => {
                    if let Err(error) = shown.viewer.show(&frames) {
                        eprintln!("camera window: {error}");
                    }
                    if self.progress() == before {
                        shown.stopped = Some("no guest progress".into());
                    }
                }
                Err(error) => shown.stopped = Some(error),
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        self.shown = Some(shown);
    }

    /// Boot progress, UI state and guest time; the screen when asked.
    pub fn observe(&self, screen: bool) -> Result<(Value, Option<Vec<u8>>), String> {
        let mut summary = match boot::status(&self.session) {
            Ok(report) => serde_json::to_value(&report).map_err(|error| error.to_string())?,
            Err(error) => json!({ "status_error": error }),
        };
        if let Some(object) = summary.as_object_mut() {
            object.remove("discovery_callee_stubs");
            object.remove("translation_cache_size_bytes");
            object.remove("setup_presses");
        }
        let now = self.session.board.now();
        summary["guest_seconds"] = json!(now as f64 / self.ticks_per_second as f64);
        summary["instructions_per_core"] = json!(self.session.board.instruction_counts());
        summary["profiling"] = json!(self.profiling);
        let png = if screen {
            match boot::screenshot(&self.session).or_else(|error| self.latest_png().ok_or(error)) {
                Ok(png) => Some(png),
                Err(error) => {
                    summary["screen_error"] = json!(error);
                    None
                }
            }
        } else {
            None
        };
        Ok((summary, png))
    }

    /// Install the control wiring and input confirmation once the firmware
    /// has loaded its configuration.
    fn connect(&mut self) -> Result<(), String> {
        fujimod_emulator_controls::connect(
            &mut self.session,
            &self.resolved.reference,
            self.ticks_per_second,
        )
        .map(|_| ())
    }

    /// The controls `press` and `turn` accept, with their positions.
    pub fn controls(&mut self) -> Result<Value, String> {
        self.connect()?;
        let state = self.session.control_state()?;
        serde_json::to_value(&state.inputs).map_err(|error| error.to_string())
    }

    fn progress(&self) -> (Vec<u64>, u64) {
        (
            self.session.board.instruction_counts(),
            self.session.board.now(),
        )
    }

    /// Run to an absolute guest tick.
    fn run_to(&mut self, until: u64) -> Result<(), String> {
        while self.session.board.now() < until {
            let before = self.progress();
            self.session
                .run_window(u64::MAX, self.quantum, None, Some(until))?;
            if self.progress() == before {
                let failure = boot::status(&self.session)
                    .ok()
                    .and_then(|report| report.failure)
                    .unwrap_or_else(|| "no guest progress".into());
                return Err(format!("the camera stopped: {failure}"));
            }
        }
        self.collect_frames();
        Ok(())
    }

    /// Run for guest seconds.
    pub fn run_for(&mut self, seconds: f64) -> Result<(), String> {
        if !(seconds.is_finite() && (0.0..=3600.0).contains(&seconds)) {
            return Err("seconds must be between 0 and 3600".into());
        }
        let end = self.session.board.now() + (seconds * self.ticks_per_second as f64) as u64;
        self.run_to(end)
    }

    pub fn run_to_rec(&mut self, instruction_limit: u64) -> Result<(), String> {
        boot::to_rec_with_actions(
            &mut self.session,
            instruction_limit,
            None,
            false,
            |_| Ok(None),
            |_, _| Ok(()),
        )
        .map(|_| ())
    }

    /// Deliver one operation through `fujimod-remote`, which waits until the
    /// firmware accepts input and confirms the scanner registered it.
    fn operate(
        &mut self,
        operation: impl for<'r> FnOnce(
            &'r mut BoardRemote<&mut Session>,
            u64,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), String>> + 'r>,
        >,
    ) -> Result<(), String> {
        let facts = self.facts.clone()?;
        self.connect()?;
        let options = RemoteOptions {
            quantum: self.quantum,
            window_instructions: 100_000,
            scanout_node: boot::scanout_node(&self.session)?,
            output: "lcd".into(),
            // The EXR II display-timing model counts completed LCD frames here.
            frame_counter_kind: "display-timing".into(),
            frame_counter_pointer: "/0/banks/1/frames".into(),
        };
        let deadline = self.session.board.now() + 5 * self.ticks_per_second;
        let mut remote = BoardRemote::new(&mut self.session, facts, options)?;
        futures_lite::future::block_on(operation(&mut remote, deadline))
    }

    pub fn press(&mut self, control: &str, settle_ms: u64) -> Result<(), String> {
        if settle_ms > 600_000 {
            return Err("settle_ms must be at most 600000".into());
        }
        let control = Control::new(control);
        self.operate(move |remote, deadline| {
            Box::pin(async move { operations::press(remote, &control, deadline).await })
        })?;
        self.run_for(settle_ms as f64 / 1000.0)
    }

    pub fn turn(&mut self, control: &str, steps: i32, settle_ms: u64) -> Result<(), String> {
        if settle_ms > 600_000 || steps.unsigned_abs() > 100 {
            return Err("steps must be within 100 and settle_ms at most 600000".into());
        }
        let control = Control::new(control);
        self.operate(move |remote, deadline| {
            Box::pin(async move { operations::turn(remote, &control, steps, deadline).await })
        })?;
        self.run_for(settle_ms as f64 / 1000.0)
    }

    pub fn checkpoint(&mut self, name: &str, file: Option<&str>) -> Result<Value, String> {
        if self.profiling {
            return Err("collect the active profile before taking a checkpoint".into());
        }
        let snapshot = self.session.checkpoint(None)?;
        if let Some(file) = file {
            boot::save_snapshot(&self.session, std::path::Path::new(file))?;
        }
        let summary = json!({
            "checkpoint": name,
            "file": file,
            "guest_seconds": snapshot.now() as f64 / self.ticks_per_second as f64,
            "instructions_per_core": snapshot.instruction_counts(),
        });
        self.checkpoints.insert(name.into(), snapshot);
        Ok(summary)
    }

    pub fn restore(&mut self, name: Option<&str>, file: Option<&str>) -> Result<(), String> {
        if self.profiling {
            return Err("collect the active profile before restoring".into());
        }
        match (name, file) {
            (Some(name), None) => {
                let snapshot = self
                    .checkpoints
                    .get(name)
                    .ok_or_else(|| format!("no checkpoint named {name}"))?;
                self.session.restore(snapshot)
            }
            (None, Some(file)) => {
                boot::load_snapshot(&mut self.session, std::path::Path::new(file))
            }
            _ => Err("give exactly one of name or file".into()),
        }
    }

    pub fn read_memory(&self, address: u64, size: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0; size];
        self.session.board.read_physical(address, &mut bytes)?;
        Ok(bytes)
    }

    pub fn profile_start(&mut self) -> Result<(), String> {
        self.session.board.start_guest_profile()?;
        self.profiling = true;
        Ok(())
    }

    pub fn profile_collect(&mut self) -> Result<Value, String> {
        if !self.profiling {
            return Err("no profile is active; call camera_profile_start first".into());
        }
        self.profiling = false;
        Ok(self.session.board.guest_profile())
    }
}
