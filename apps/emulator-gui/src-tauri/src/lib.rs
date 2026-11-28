use fujimod_camera::{self as camera, Library, LibraryFirmware, StateDir};
use fujimod_catalog_configuration::{RunInputs, RunStart};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_boot::xp5::RunHost;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchRequest {
    /// Catalog body identity.
    body: String,
    /// The body's firmware: a version, a firmware identity or `none`; the
    /// body's newest when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    firmware: Option<String>,
    update: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    boot_rom: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    camera: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    application_plan: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    load_snapshot: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    engine: Option<fujimod_catalog_configuration::RunEngine>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchedCamera {
    id: String,
    configuration: String,
    body: String,
    model: String,
    firmware: String,
    endpoint: String,
}

/// One firmware the desktop launcher can open.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LauncherCamera {
    firmware: String,
    body: String,
    model: String,
    version: String,
}

/// A camera the desktop app opens itself through the camera builder.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenCameraRequest {
    body: String,
    firmware: String,
    boot: LauncherBoot,
    #[serde(default)]
    sd_card: Option<PathBuf>,
    /// Boot without the library's saved boot, and save none.
    #[serde(default)]
    skip_boot_cache: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum LauncherBoot {
    Stock,
    SdCard,
    Autostart,
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<serde_json::Value, String>>>,
}

struct AppState {
    running: Mutex<Option<Running>>,
    command: Mutex<Option<LaunchRequest>>,
    endpoint: Mutex<Option<String>>,
}

fn log_session(line: impl AsRef<str>) {
    let path = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches")))
        .unwrap_or_else(std::env::temp_dir)
        .join("fujimod/emulator-gui.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{}", line.as_ref());
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[tauri::command]
async fn choose_update() -> Option<String> {
    rfd::AsyncFileDialog::new()
        .add_filter("Fujifilm firmware", &["DAT"])
        .pick_file()
        .await
        .map(|file| file.path().display().to_string())
}

/// The model name a launched camera shows.
fn model(resolved: &ResolvedConfiguration) -> String {
    resolved.reference.body.id.to_uppercase()
}

fn checked_update(resolved: &ResolvedConfiguration, update: &str) -> Result<PathBuf, String> {
    let identity = resolved
        .reference
        .firmware
        .update
        .as_ref()
        .ok_or("selected firmware has no update image")?;
    let update = PathBuf::from(update);
    let metadata =
        std::fs::metadata(&update).map_err(|error| format!("{}: {error}", update.display()))?;
    let refused = || {
        format!(
            "{} is not the firmware for {}.",
            update.display(),
            model(resolved)
        )
    };
    if metadata.len() != identity.size {
        return Err(refused());
    }
    let shipped =
        std::fs::read(&update).map_err(|error| format!("{}: {error}", update.display()))?;
    use sha2::{Digest, Sha256};
    if format!("{:x}", Sha256::digest(&shipped)) != identity.sha256 {
        return Err(refused());
    }
    Ok(update)
}

fn launch_camera(request: LaunchRequest) -> Result<(LaunchedCamera, Running), String> {
    let library = Library::without_directory().map_err(|error| error.to_string())?;
    let firmware = library
        .select(
            &request.body,
            &camera::FirmwareChoice::parse(request.firmware.as_deref()),
        )
        .map_err(|error| error.to_string())?;
    let mut resolved = library
        .resolved(&firmware, &[])
        .map_err(|error| error.to_string())?;
    if let Some(engine) = request.engine {
        resolved.override_engine(engine)?;
    }
    if !matches!(
        resolved.start,
        RunStart::Loader | RunStart::Rom
    ) {
        return Err("This camera cannot start from the firmware file alone.".into());
    }
    let update = checked_update(&resolved, &request.update)?;
    let output = output_directory(&resolved.configuration.id)?;
    let launched_body = resolved.reference.body.id.clone();
    let launched_model = model(&resolved);
    let launched_firmware = resolved.reference.firmware.id.clone();
    let launched_configuration = resolved.configuration.id.clone();
    let room = fujimod_emulator_gui::Room::loopback()?;
    let endpoint = room.endpoint().url.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = thread::Builder::new()
        .name("fujimod-emulator-session".into())
        .spawn(move || {
            log_session(format!(
                "session: opening camera engine={:?}",
                resolved
                    .configuration
                    .execution
                    .map(|execution| execution.engine)
            ));
            if !matches!(resolved.inputs, Some(RunInputs::Xp5(_))) {
                let selection = resolved.reference.clone();
                let boot_request = fujimod_emulator_boot::Request {
                    update: fujimod_emulator_boot::FirmwareFile::Path(update),
                    boot_rom: request
                        .boot_rom
                        .map(fujimod_emulator_boot::FirmwareFile::Path),
                    nand: None,
                    settings: None,
                    camera: request.camera,
                    application_plan: request.application_plan,
                    cards: Default::default(),
                    discovery: false,
                    sd_card_transfer_ticks: None,
                    card_layout: None,
                    card_initialization_fault: None,
                    camera_settings: Default::default(),
                    clock_seconds_2000: None,
                    scene: None,
                };
                let mut camera = fujimod_emulator_boot::open_request(resolved, boot_request)
                    .inspect_err(|error| log_session(format!("session: {error}")))?;
                if let Some(path) = request.load_snapshot {
                    fujimod_emulator_boot::load_snapshot(&mut camera.session, &path)
                        .inspect_err(|error| log_session(format!("session: {error}")))?;
                }
                log_session("session: camera opened");
                return fujimod_emulator_gui::serve(
                    fujimod_emulator_gui::Live::new(camera, selection),
                    room,
                    &mut || !thread_stop.load(std::sync::atomic::Ordering::Acquire),
                )
                .inspect_err(|error| log_session(format!("session: {error}")));
            }
            let run = fujimod_emulator_boot::xp5::Run {
                load_snapshot: request.load_snapshot,
                boot_rom: request
                    .boot_rom
                    .map(fujimod_emulator_boot::FirmwareFile::Path),
                ..fujimod_emulator_boot::xp5::Run::new(
                    resolved.clone(),
                    update,
                    fujimod_emulator_cpu::ExecutionPolicy::Interactive,
                    output,
                )
            };
            let run =
                RunHost::open(run).inspect_err(|error| log_session(format!("session: {error}")))?;
            log_session("session: camera opened");
            fujimod_emulator_gui::serve(run, room, &mut || {
                !thread_stop.load(std::sync::atomic::Ordering::Acquire)
            })
            .inspect_err(|error| log_session(format!("session: {error}")))
        })
        .map_err(|error| error.to_string())?;
    let launched = LaunchedCamera {
        id: endpoint.clone(),
        configuration: launched_configuration,
        body: launched_body,
        model: launched_model,
        firmware: launched_firmware,
        endpoint,
    };
    Ok((
        launched,
        Running {
            stop,
            thread: Some(thread),
        },
    ))
}

/// Launcher rows for firmwares the library holds.
fn launcher_rows(firmwares: Vec<LibraryFirmware>) -> Vec<LauncherCamera> {
    firmwares
        .into_iter()
        .map(|firmware| LauncherCamera {
            model: firmware.body_id.to_uppercase(),
            firmware: firmware.id,
            body: firmware.body_id,
            version: firmware.version,
        })
        .collect()
}

#[tauri::command]
fn launcher_cameras() -> Result<Vec<LauncherCamera>, String> {
    let library = Library::from_env().map_err(|error| error.to_string())?;
    Ok(launcher_rows(library.firmwares()))
}

#[tauri::command]
async fn choose_sd_card() -> Option<String> {
    rfd::AsyncFileDialog::new()
        .pick_folder()
        .await
        .map(|folder| folder.path().display().to_string())
}

/// Open a camera through the builder and serve the opened session live.
fn open_with_builder(request: OpenCameraRequest) -> Result<(LaunchedCamera, Running), String> {
    let library = Library::from_env().map_err(|error| error.to_string())?;
    let mut camera_request = camera::EmulatorRequestBase::new(
        request.body.clone(),
        camera::FirmwareChoice::parse(Some(request.firmware.as_str())),
    );
    let resolved = camera::plan(&library, &camera_request)
        .map_err(|error| error.to_string())?
        .resolved;
    let preset = match request.boot {
        LauncherBoot::Stock => camera::Preset::Stock,
        LauncherBoot::SdCard => camera::Preset::SdCard,
        LauncherBoot::Autostart => camera::Preset::Autostart,
    };
    let sd_card = request
        .sd_card
        .as_deref()
        .map(|directory| {
            camera::directory_files(directory)
                .map(camera::Card::with_files)
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let body = resolved.reference.body.id.clone();
    let model = body.to_uppercase();
    let configuration = resolved.configuration.id.clone();
    let firmware = resolved.reference.firmware.id.clone();
    let engine = resolved
        .configuration
        .execution
        .map(|execution| execution.engine);
    let selection = resolved.reference.clone();
    camera_request.cards.sd_card = sd_card;
    camera_request.policy = fujimod_emulator_cpu::ExecutionPolicy::Interactive;
    camera_request.cache = if request.skip_boot_cache {
        camera::BootCache::Bypass
    } else {
        camera::BootCache::Reuse
    };
    let camera_request = camera::Request {
        camera: camera_request,
        preset,
    };
    let state = StateDir::from_env();
    let room = fujimod_emulator_gui::Room::loopback()?;
    let endpoint = room.endpoint().url.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let (report, received) = std::sync::mpsc::channel::<Result<(), String>>();
    let thread = thread::Builder::new()
        .name("fujimod-emulator-session".into())
        .spawn(move || {
            log_session(format!("session: opening camera engine={:?}", engine));
            let opened = match camera::open(&library, &state, camera_request) {
                Ok(opened) => {
                    let _ = report.send(Ok(()));
                    opened
                }
                Err(error) => {
                    log_session(format!("session: {error}"));
                    let error = error.to_string();
                    let _ = report.send(Err(error.clone()));
                    return Err(error);
                }
            };
            log_session(format!("session: camera opened cached={}", opened.cached));
            fujimod_emulator_gui::serve(
                fujimod_emulator_gui::Live::new(opened.into_camera(), selection),
                room,
                &mut || !thread_stop.load(Ordering::Acquire),
            )
            .inspect_err(|error| log_session(format!("session: {error}")))
        })
        .map_err(|error| error.to_string())?;
    let running = Running {
        stop,
        thread: Some(thread),
    };
    match received.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            drop(running);
            return Err(error);
        }
        Err(_) => {
            drop(running);
            return Err("The camera thread stopped before the boot finished.".into());
        }
    }
    let launched = LaunchedCamera {
        id: endpoint.clone(),
        configuration,
        body,
        model,
        firmware,
        endpoint,
    };
    Ok((launched, running))
}

/// Accept a session that another process serves in a loopback share room.
fn checked_endpoint(endpoint: &str) -> Result<(), String> {
    let ticket = fujimod_webrtc::share::Ticket::from_url(endpoint)
        .map_err(|error| error.to_string())?
        .ok_or("a session endpoint must be a share link")?;
    let signal = url::Url::parse(&ticket.signal).map_err(|error| error.to_string())?;
    if signal.scheme() != "ws" || signal.host_str() != Some("127.0.0.1") {
        return Err("a session endpoint must be a loopback share room".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_endpoint_is_a_loopback_share_room() {
        let room = fujimod_webrtc::LoopbackRoom::start("http://localhost").unwrap();
        assert!(checked_endpoint(room.link()).is_ok());
        assert!(checked_endpoint("ws://127.0.0.1:48123/emulator").is_err());
        assert!(checked_endpoint("https://example.com/").is_err());
    }

    #[test]
    fn launcher_rows_name_each_held_firmware() {
        let rows = launcher_rows(vec![
            LibraryFirmware {
                id: "x70-110".into(),
                body_id: "x70".into(),
                version: "1.10".into(),
            },
            LibraryFirmware {
                id: "x100vi-131".into(),
                body_id: "x100vi".into(),
                version: "1.31".into(),
            },
        ]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].firmware, "x70-110");
        assert_eq!(rows[0].body, "x70");
        assert_eq!(rows[0].model, "X70");
        assert_eq!(rows[0].version, "1.10");
        assert_eq!(rows[1].firmware, "x100vi-131");
        assert_eq!(rows[1].model, "X100VI");
    }

    #[test]
    fn the_launcher_lists_firmwares_the_library_holds() {
        let library =
            Library::from_env().unwrap_or_else(|error| panic!("firmware library: {error}"));
        let rows = launcher_rows(library.firmwares());
        if rows.is_empty() {
            eprintln!("skipped: the library holds no openable firmware");
            return;
        }
        assert!(!rows.is_empty());
        for row in &rows {
            assert_eq!(row.model, row.body.to_uppercase());
            assert!(library.resolved(&row.firmware, &[]).is_ok());
        }
    }
}

#[tauri::command]
async fn connect(
    endpoint: String,
    state: tauri::State<'_, AppState>,
) -> Result<LaunchedCamera, String> {
    checked_endpoint(&endpoint)?;
    let previous = state
        .running
        .lock()
        .map_err(|_| "emulator state is poisoned".to_owned())?
        .take();
    drop(previous);
    // The catalog event supplies the camera identity once the webview joins the session.
    Ok(LaunchedCamera {
        id: endpoint.clone(),
        configuration: String::new(),
        body: String::new(),
        model: String::new(),
        firmware: String::new(),
        endpoint,
    })
}

#[tauri::command]
async fn launch(
    request: LaunchRequest,
    state: tauri::State<'_, AppState>,
) -> Result<LaunchedCamera, String> {
    let (launched, running) = launch_camera(request)?;
    let previous = state
        .running
        .lock()
        .map_err(|_| "emulator state is poisoned".to_owned())?
        .replace(running);
    drop(previous);
    Ok(launched)
}

#[tauri::command]
async fn open_camera(
    request: OpenCameraRequest,
    state: tauri::State<'_, AppState>,
) -> Result<LaunchedCamera, String> {
    let (launched, running) = open_with_builder(request)?;
    let previous = state
        .running
        .lock()
        .map_err(|_| "emulator state is poisoned".to_owned())?
        .replace(running);
    drop(previous);
    Ok(launched)
}

#[tauri::command]
fn close(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let running = state
        .running
        .lock()
        .map_err(|_| "emulator state is poisoned".to_owned())?
        .take();
    drop(running);
    Ok(())
}

fn output_directory(configuration: &str) -> Result<PathBuf, String> {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches")))
        .ok_or("cannot resolve the application cache directory")?
        .join("fujimod/emulator")
        .join(configuration);
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    Ok(root)
}

#[derive(Default)]
struct WindowLaunch {
    command: Option<LaunchRequest>,
    endpoint: Option<String>,
}

fn window_launch() -> Result<WindowLaunch, String> {
    let mut launch = WindowLaunch::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let (key, inline) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(key, value)| (key, Some(value)));
        match key {
            "--body" => {
                launch
                    .command
                    .get_or_insert_with(LaunchRequest::default)
                    .body = inline
                    .map(str::to_owned)
                    .or_else(|| arguments.next())
                    .ok_or("--body needs a value")?;
            }
            "--firmware" => {
                launch
                    .command
                    .get_or_insert_with(LaunchRequest::default)
                    .firmware = Some(
                    inline
                        .map(str::to_owned)
                        .or_else(|| arguments.next())
                        .ok_or("--firmware needs a value")?,
                );
            }
            "--update" => {
                launch
                    .command
                    .get_or_insert_with(LaunchRequest::default)
                    .update = inline
                    .map(str::to_owned)
                    .or_else(|| arguments.next())
                    .ok_or("--update needs a value")?;
            }
            "--boot-rom" | "--camera" | "--application-plan" | "--load-snapshot" => {
                let value = inline
                    .map(str::to_owned)
                    .or_else(|| arguments.next())
                    .ok_or_else(|| format!("{key} needs a value"))?;
                let command = launch.command.get_or_insert_with(LaunchRequest::default);
                let path = Some(PathBuf::from(value));
                match key {
                    "--boot-rom" => command.boot_rom = path,
                    "--camera" => command.camera = path,
                    "--application-plan" => command.application_plan = path,
                    _ => command.load_snapshot = path,
                }
            }
            "--engine" => {
                let value = inline
                    .map(str::to_owned)
                    .or_else(|| arguments.next())
                    .ok_or("--engine needs a value")?;
                launch
                    .command
                    .get_or_insert_with(LaunchRequest::default)
                    .engine = Some(value.parse().map_err(|_| "unknown --engine value")?);
            }
            "--endpoint" => {
                launch.endpoint = Some(
                    inline
                        .map(str::to_owned)
                        .or_else(|| arguments.next())
                        .ok_or("--endpoint needs a value")?,
                );
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if let Some(command) = &launch.command
        && (command.body.is_empty() || command.update.is_empty())
    {
        return Err("--body and --update are given together".into());
    }
    Ok(launch)
}

#[tauri::command]
fn connection(state: tauri::State<'_, AppState>) -> Option<String> {
    state.endpoint.lock().ok()?.take()
}

#[tauri::command]
fn launch_connection(state: tauri::State<'_, AppState>) -> Option<LaunchRequest> {
    state.command.lock().ok()?.take()
}

#[cfg(target_os = "macos")]
mod window_shape;

/// Width of the controls panel beside the camera screen, in points.
const PANEL: f64 = 360.0;
/// Width over height of the camera screen.
const SCREEN: f64 = 3.0 / 2.0;

/// Content width of the camera window for a content height.
fn window_width(height: f64) -> f64 {
    height * SCREEN + PANEL
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let startup = window_launch()?;
    fujimod_emulator_exr2_board::register_board_builder();
    fujimod_emulator_models::register();
    tauri::Builder::default()
        .manage(AppState {
            running: Mutex::new(None),
            command: Mutex::new(startup.command),
            endpoint: Mutex::new(startup.endpoint),
        })
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let window = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Fujimod Emulator")
            .inner_size(window_width(720.0), 720.0)
            .min_inner_size(window_width(540.0), 540.0)
            .visible(false)
            .build()?;
            #[cfg(target_os = "macos")]
            window_shape::constrain(window.ns_window()?)?;
            window.show()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            choose_update,
            launch,
            launcher_cameras,
            choose_sd_card,
            open_camera,
            connect,
            connection,
            launch_connection,
            close
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}
