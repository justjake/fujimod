//! One emulated camera, owned by its thread.
use std::{collections::BTreeMap, path::PathBuf};

use fujimod_camera::{
    CardSource, Library, Preset, Request as CameraRequest, Stage, StateDir, open as open_camera,
    open_unbooted,
};
use fujimod_catalog_remote::Facts;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot as boot;
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_session::{Session, checkpoint::Snapshot};
use fujimod_remote::{BoardRemote, Control, operations};
use serde_json::{Value, json};

use crate::{OpenBoot, OpenParams};

/// The `Send` part of an open request, moved to the camera's thread.
pub struct OpenRequest {
    firmware: String,
    boot: OpenBoot,
    sd_card: Option<PathBuf>,
    snapshot: Option<PathBuf>,
    execution_policy: Option<String>,
    boot_cache: bool,
}

impl From<OpenParams> for OpenRequest {
    fn from(p: OpenParams) -> Self {
        Self {
            firmware: p.firmware,
            boot: p.boot,
            sd_card: p.sd_card.map(Into::into),
            snapshot: p.snapshot.map(Into::into),
            execution_policy: p.execution_policy,
            boot_cache: p.boot_cache.unwrap_or(true),
        }
    }
}

impl OpenRequest {
    fn camera_request(&self) -> Result<CameraRequest, String> {
        let policy = match &self.execution_policy {
            Some(policy) => ExecutionPolicy::parse(policy)?,
            None => ExecutionPolicy::Exact,
        };
        Ok(CameraRequest {
            firmware: self.firmware.clone(),
            files: fujimod_camera::FirmwareInput::Library,
            overlays: Vec::new(),
            preset: match self.boot {
                OpenBoot::Stock => Preset::Stock,
                OpenBoot::SdCard => Preset::SdCard,
                OpenBoot::Autostart => Preset::Autostart,
            },
            sd_card: self
                .sd_card
                .as_ref()
                .map(|directory| CardSource::Directory(directory.clone())),
            policy,
            engine: None,
            // A named checkpoint is an explicit restore; the boot cache would
            // otherwise replace it.
            cache: if self.boot_cache && self.snapshot.is_none() {
                fujimod_camera::BootCache::Reuse
            } else {
                fujimod_camera::BootCache::Bypass
            },
            run_to_rec: true,
            card: Default::default(),
            nand_image: None,
            face_result_profile: None,
            camera_settings: Default::default(),
        })
    }

    fn boot_name(&self) -> &'static str {
        match self.boot {
            OpenBoot::Stock => "stock",
            OpenBoot::SdCard => "sd-card",
            OpenBoot::Autostart => "autostart",
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
    fn step(&mut self) -> Result<(), String> {
        // About one LCD frame of guest time per slice keeps the window responsive.
        let until = self.session.board.now() + (self.ticks_per_second / 60).max(1);
        self.session
            .run_window(u64::MAX, self.quantum, None, Some(until))
    }
    fn settle(&mut self) -> Result<(), String> {
        let until = self.session.board.now() + (self.ticks_per_second / 1000).max(1);
        self.session
            .run_window(u64::MAX, self.quantum, None, Some(until))
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
impl Camera {
    pub fn open(request: OpenRequest) -> Result<(Self, (Value, Option<Vec<u8>>)), String> {
        let library = Library::from_env().map_err(|error| error.to_string())?;
        let state = StateDir::from_env();
        let started = std::time::Instant::now();
        let camera_request = request.camera_request()?;
        let policy = camera_request.policy;
        let opened = match &request.snapshot {
            Some(snapshot) => {
                let mut opened = open_unbooted(&library, &state, camera_request)
                    .map_err(|error| error.to_string())?;
                boot::load_snapshot(&mut opened.camera.camera.session, snapshot)?;
                opened.camera.stage = match boot::status(&opened.camera.camera.session) {
                    Ok(report) if report.status == "rec" => Stage::Rec,
                    _ => Stage::FirmwareLoaded,
                };
                opened
            }
            None => {
                open_camera(&library, &state, camera_request).map_err(|error| error.to_string())?
            }
        };
        let quantum = opened.camera.camera.quantum;
        let ticks_per_second = opened.camera.camera.ticks_per_second;
        let facts = fujimod_emulator_camera::remote_facts(&opened.resolved);
        let stage = opened.stage;
        let release = opened.release.clone();
        let opened_cached = opened.cached;
        let emulator_opened = opened.camera;
        let mut camera = Self {
            session: emulator_opened.camera.session,
            resolved: emulator_opened.resolved,
            quantum,
            ticks_per_second,
            checkpoints: BTreeMap::new(),
            profiling: false,
            facts,
            shown: None,
            latest: None,
        };
        // Connect the controls now, so every checkpoint carries them and
        // restores into this camera. A boot that stopped before the firmware
        // loaded its configuration leaves them to connect on first use.
        let controls = camera.connect().err();
        let (mut summary, png) = camera.observe(true)?;
        if let Some(error) = controls {
            summary["controls_error"] = json!(error);
        }
        summary["opened"] = json!({
            "firmware": request.firmware.clone(),
            "boot": request.boot_name(),
            "cached": opened_cached,
            "sd_card": request
                .sd_card
                .as_ref()
                .map(|path| path.display().to_string())
                .or(release),
            "snapshot": request.snapshot.as_ref().map(|path| path.display().to_string()),
            "execution_policy": policy.name(),
            "stage": stage.name(),
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
        let rgb: Vec<u8> = frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
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
        let options = fujimod_emulator_camera::remote_options(&self.session, self.quantum)?;
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

    /// Move a dial, switch, ring or lever to a named position through the
    /// contact driver, and wait until the firmware's own observation of the
    /// control shows the new pose.
    pub fn set(&mut self, control: &str, position: &str, settle_ms: u64) -> Result<Value, String> {
        if settle_ms > 600_000 {
            return Err("settle_ms must be at most 600000".into());
        }
        self.connect()?;
        let driver = self
            .session
            .contact_driver
            .as_ref()
            .ok_or("physical input automation is not configured")?;
        let actuator = driver
            .spec
            .actuators
            .iter()
            .find(|actuator| actuator.id == control);
        let actuator = match actuator {
            Some(actuator) if actuator.momentary => {
                return Err(format!(
                    "{control} is a button; press it with camera_press or turn it with camera_turn"
                ));
            }
            Some(actuator) => actuator,
            None => {
                let controls: Vec<&str> = driver
                    .spec
                    .actuators
                    .iter()
                    .filter(|actuator| !actuator.momentary)
                    .map(|actuator| actuator.id.as_str())
                    .collect();
                return Err(format!(
                    "no physical control {control}; persistent controls: {controls:?}"
                ));
            }
        };
        let pose = actuator
            .states
            .iter()
            .find(|pose| pose.position == position)
            .ok_or_else(|| {
                let positions: Vec<&str> = actuator
                    .states
                    .iter()
                    .map(|pose| pose.position.as_str())
                    .collect();
                format!("{control} has no position {position}; positions: {positions:?}")
            })?;
        if pose.ready.is_empty() || pose.expected.is_empty() {
            return Err(format!(
                "the firmware has no observation of {control} at {position}, so the move cannot be confirmed"
            ));
        }
        let before = self
            .session
            .control_state()?
            .positions
            .get(control)
            .cloned();
        let started = self.session.board.now();
        let deadline = started + 5 * self.ticks_per_second;
        self.session.schedule_input(
            started,
            fujimod_emulator_session::contact_driver::Scheduled::Position {
                id: control.into(),
                position: position.into(),
            },
        )?;
        if let Some(driver) = self.session.contact_driver.as_mut() {
            driver.set_watched(true);
        }
        let waited = self.wait_contact_input(deadline);
        if let Some(driver) = self.session.contact_driver.as_mut() {
            driver.set_watched(false);
        }
        waited?;
        self.run_for(settle_ms as f64 / 1000.0)?;
        let state = self.session.control_state()?;
        let panel = state.positions.get(control).cloned();
        if panel.as_deref() != Some(position) {
            return Err(format!("{control} settled at {panel:?}, not {position}"));
        }
        Ok(json!({
            "control": control,
            "position": position,
            "from": before,
            "confirmed": true,
            "input_queue": state.input_queue,
        }))
    }

    /// Run until the contact driver has acknowledged every queued move.
    fn wait_contact_input(&mut self, deadline: u64) -> Result<(), String> {
        loop {
            let Some(driver) = self.session.contact_driver.as_ref() else {
                return Ok(());
            };
            if !driver.busy() {
                return Ok(());
            }
            let poll = driver.spec.poll_ticks.max(1);
            if self.session.board.now() >= deadline {
                return Err(format!(
                    "timed out waiting for the firmware to register the position; queue: {}",
                    serde_json::to_string(&driver.state()).unwrap_or_default()
                ));
            }
            let end = (self.session.board.now() + poll).min(deadline);
            let before = self.progress();
            self.session
                .run_window(u64::MAX, self.quantum, None, Some(end))?;
            if self.progress() == before {
                let failure = boot::status(&self.session)
                    .ok()
                    .and_then(|report| report.failure)
                    .unwrap_or_else(|| "no guest progress".into());
                return Err(format!("the camera stopped: {failure}"));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn x70_is_openable() -> bool {
        Library::from_env()
            .is_ok_and(|library| library.firmwares().iter().any(|row| row.id == "x70-110"))
    }

    fn request(boot: OpenBoot, sd_card: Option<PathBuf>) -> OpenRequest {
        OpenRequest {
            firmware: "x70-110".into(),
            boot,
            sd_card,
            snapshot: None,
            execution_policy: None,
            boot_cache: false,
        }
    }

    #[test]
    fn a_stock_boot_with_a_card_directory_adds_files() {
        let built = request(OpenBoot::Stock, Some(PathBuf::from("/tmp/example-card")))
            .camera_request()
            .unwrap();
        assert_eq!(built.preset, Preset::Stock);
        assert!(matches!(
            built.sd_card,
            Some(CardSource::Directory(ref path)) if path == Path::new("/tmp/example-card")
        ));
        assert_eq!(built.policy, ExecutionPolicy::Exact);
    }

    #[test]
    fn an_sd_card_boot_without_a_directory_uses_the_release() {
        let built = request(OpenBoot::SdCard, None).camera_request().unwrap();
        assert_eq!(built.preset, Preset::SdCard);
        assert!(built.sd_card.is_none());
    }

    #[test]
    fn an_sd_card_boot_uses_the_directory() {
        let directory = PathBuf::from("/tmp/example-card");
        let built = request(OpenBoot::SdCard, Some(directory.clone()))
            .camera_request()
            .unwrap();
        assert_eq!(built.preset, Preset::SdCard);
        assert!(matches!(
            built.sd_card,
            Some(CardSource::Directory(path)) if path == directory
        ));
    }

    #[test]
    fn an_autostart_without_a_directory_uses_the_release() {
        let built = request(OpenBoot::Autostart, None).camera_request().unwrap();
        assert_eq!(built.preset, Preset::Autostart);
        assert!(built.sd_card.is_none());
    }

    #[test]
    fn an_autostart_uses_the_directory() {
        let directory = PathBuf::from("/tmp/example-card");
        let built = request(OpenBoot::Autostart, Some(directory.clone()))
            .camera_request()
            .unwrap();
        assert_eq!(built.preset, Preset::Autostart);
        assert!(matches!(
            built.sd_card,
            Some(CardSource::Directory(path)) if path == directory
        ));
    }

    #[test]
    fn the_execution_policy_is_parsed() {
        let mut request = request(OpenBoot::Stock, None);
        request.execution_policy = Some("interactive".into());
        assert_eq!(
            request.camera_request().unwrap().policy,
            ExecutionPolicy::Interactive
        );
        request.execution_policy = Some("fast".into());
        assert!(
            request
                .camera_request()
                .err()
                .unwrap()
                .contains("unknown CPU execution policy")
        );
    }

    #[test]
    fn a_camera_opens_and_restores_through_the_builder() {
        if !x70_is_openable() {
            eprintln!("skipped: the library holds no openable x70-110");
            return;
        }
        let started = std::time::Instant::now();
        let (mut camera, (summary, _)) = Camera::open(request(OpenBoot::Stock, None)).unwrap();
        assert_eq!(summary["opened"]["stage"], "REC");
        assert_eq!(summary["opened"]["boot"], "stock");
        assert_eq!(summary["opened"]["firmware"], "x70-110");
        assert_eq!(summary["opened"]["execution_policy"], "exact");
        let directory =
            std::env::temp_dir().join(format!("fujimod-mcp-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("rec.snapshot");
        camera
            .checkpoint("rec", Some(file.to_str().unwrap()))
            .unwrap();
        drop(camera);

        let mut restored = request(OpenBoot::Stock, None);
        restored.snapshot = Some(file.clone());
        let (restored, (summary, _)) = Camera::open(restored).unwrap();
        assert_eq!(summary["opened"]["stage"], "REC");
        assert_eq!(summary["opened"]["snapshot"], file.display().to_string());
        assert_eq!(boot::status(&restored.session).unwrap().status, "rec");
        std::fs::remove_dir_all(&directory).unwrap();
        eprintln!("open and restore in {:?}", started.elapsed());
    }

    #[test]
    fn the_x70_aperture_ring_confirms_the_position_it_moves_to() {
        if !x70_is_openable() {
            eprintln!("skipped: the library holds no openable x70-110");
            return;
        }
        let (mut camera, _) = Camera::open(request(OpenBoot::Stock, None)).unwrap();
        let started = std::time::Instant::now();
        let moved = camera.set("aperture_ring", "5.6", 100).unwrap();
        assert_eq!(moved["control"], "aperture_ring");
        assert_eq!(moved["position"], "5.6");
        assert_eq!(moved["confirmed"], true);
        assert_eq!(moved["from"], "A");
        let state = camera.session.control_state().unwrap();
        assert_eq!(
            state.positions.get("aperture_ring").map(String::as_str),
            Some("5.6")
        );
        // The firmware ran on with the ring at its new position.
        assert_eq!(boot::status(&camera.session).unwrap().status, "rec");
        // The same ring returns to A, and the button and unknown positions
        // are refused.
        let back = camera.set("aperture_ring", "A", 100).unwrap();
        assert_eq!(back["from"], "5.6");
        assert!(
            camera
                .set("menu_ok_button", "pressed", 0)
                .err()
                .unwrap()
                .contains("button")
        );
        assert!(
            camera
                .set("aperture_ring", "f/5.6", 0)
                .err()
                .unwrap()
                .contains("positions")
        );
        eprintln!("aperture ring moves in {:?}", started.elapsed());
    }
}
