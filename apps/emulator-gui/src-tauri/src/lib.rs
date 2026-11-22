use fujimod_catalog_configuration::{RunInputs, RunStart};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_gui::Xp5Session;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Configuration {
    configuration: String,
    body: String,
    model: String,
    firmware: String,
    update_sha256: String,
    update_bytes: u64,
}

impl Configuration {
    fn from_resolved(configuration: &ResolvedConfiguration) -> Result<Self, String> {
        let run = &configuration.configuration;
        let update = configuration
            .reference
            .firmware
            .update
            .as_ref()
            .ok_or("selected firmware has no update image")?;
        Ok(Self {
            configuration: run.id.clone(),
            body: run.body_id.clone(),
            model: configuration.reference.body.id.to_uppercase(),
            firmware: run.firmware_id.clone(),
            update_sha256: update.sha256.clone(),
            update_bytes: update.size,
        })
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchRequest {
    configuration: String,
    update: String,
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

struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<serde_json::Value, String>>>,
}

struct AppState {
    running: Mutex<Option<Running>>,
    command: Mutex<Option<LaunchRequest>>,
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
fn note(message: String) {
    log_session(format!("desktop: {message}"));
}

#[tauri::command]
fn configurations() -> Vec<Configuration> {
    fujimod_catalog_configuration::RunConfiguration::bundled_ids()
        .filter_map(|id| {
            let configuration = ResolvedConfiguration::bundled(id).ok()?;
            let run = &configuration.configuration;
            if !matches!(run.start, RunStart::Loader | RunStart::Rom)
                || !matches!(run.inputs, Some(RunInputs::Xp5(_)))
                || run
                    .preset
                    .as_ref()
                    .and_then(|preset| preset.firmware_profile_id.as_ref())
                    .is_none()
            {
                return None;
            }
            Configuration::from_resolved(&configuration).ok()
        })
        .collect()
}

#[tauri::command]
async fn choose_update() -> Option<String> {
    rfd::AsyncFileDialog::new()
        .add_filter("Fujifilm firmware", &["DAT"])
        .pick_file()
        .await
        .map(|file| file.path().display().to_string())
}

fn checked_update(
    configuration: &ResolvedConfiguration,
    update: &str,
) -> Result<PathBuf, String> {
    let camera = Configuration::from_resolved(configuration)?;
    let update = PathBuf::from(update);
    let metadata = std::fs::metadata(&update)
        .map_err(|error| format!("{}: {error}", update.display()))?;
    if metadata.len() != camera.update_bytes {
        return Err(format!(
            "{} is not the firmware for {}.",
            update.display(),
            camera.model
        ));
    }
    let shipped = std::fs::read(&update)
        .map_err(|error| format!("{}: {error}", update.display()))?;
    use sha2::{Digest, Sha256};
    if format!("{:x}", Sha256::digest(&shipped)) != camera.update_sha256 {
        return Err(format!(
            "{} is not the firmware for {}.",
            update.display(),
            camera.model
        ));
    }
    Ok(update)
}

fn launch_camera(request: LaunchRequest) -> Result<(LaunchedCamera, Running), String> {
    let mut resolved = ResolvedConfiguration::bundled(&request.configuration)?;
    if let Some(engine) = request.engine {
        resolved.override_engine(engine)?;
    }
    if !matches!(resolved.configuration.start, RunStart::Loader | RunStart::Rom)
        || !matches!(resolved.configuration.inputs, Some(RunInputs::Xp5(_)))
    {
        return Err("This camera cannot start from the firmware file alone.".into());
    }
    let camera = Configuration::from_resolved(&resolved)?;
    let update = checked_update(&resolved, &request.update)?;
    let output = output_directory(&camera.configuration)?;
    let socket = fujimod_emulator_gui::bind(std::net::Ipv4Addr::LOCALHOST.into())?;
    let endpoint = socket.endpoint().url.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = thread::Builder::new()
        .name("fujimod-emulator-session".into())
        .spawn(move || {
            log_session(format!(
                "session: opening camera engine={:?}",
                resolved.configuration.execution.map(|execution| execution.engine)
            ));
            let run = fujimod_emulator_boot::xp5::Run {
                configuration: resolved.clone(),
                load_snapshot: None,
                save_snapshot: None,
                update,
                manifest: None,
                watch_pcs: None,
                pc_trace: None,
                screenshot: None,
                viewer_port: None,
                dump: None,
                hash: None,
                boot_rom: None,
                card_image: None,
                card_files: Vec::new(),
                keys: None,
                gpio: None,
                pokes: Vec::new(),
                press: None,
                discovery: false,
                stubs: Vec::new(),
                patch_file: None,
                mods: Vec::new(),
                scene: None,
                quantum: None,
                seconds: None,
                instruction_limit: None,
                wall_seconds: None,
                status_every: std::time::Duration::from_secs(30),
                execution_policy: fujimod_emulator_cpu::ExecutionPolicy::Interactive,
                output,
            };
            let opened = fujimod_emulator_boot::xp5::open_gui(run)
                .inspect_err(|error| log_session(format!("session: {error}")))?;
            log_session("session: camera opened");
            let fujimod_emulator_boot::Opened::Xp5(started) = opened else {
                return Err("This run did not open an XP5 camera.".into());
            };
            let (session, inputs, ticks_per_second, quantum, _) = started.into_gui_parts();
            socket
                .serve(
                    Xp5Session {
                        configuration: resolved,
                        session,
                        inputs,
                        ticks_per_second,
                        quantum,
                    },
                    Default::default(),
                )
                .run_until(&thread_stop)
                .inspect_err(|error| log_session(format!("session: {error}")))
        })
        .map_err(|error| error.to_string())?;
    let launched = LaunchedCamera {
        id: endpoint.clone(),
        configuration: camera.configuration,
        body: camera.body,
        model: camera.model,
        firmware: camera.firmware,
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
}

fn window_launch() -> Result<WindowLaunch, String> {
    let mut launch = WindowLaunch::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let (key, inline) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(key, value)| (key, Some(value)));
        match key {
            "--configuration" => {
                launch.command.get_or_insert_with(LaunchRequest::default).configuration =
                    inline.map(str::to_owned).or_else(|| arguments.next()).ok_or("--configuration needs a value")?;
            }
            "--update" => {
                launch.command.get_or_insert_with(LaunchRequest::default).update =
                    inline.map(str::to_owned).or_else(|| arguments.next()).ok_or("--update needs a value")?;
            }
            "--engine" => {
                let value = inline
                    .map(str::to_owned)
                    .or_else(|| arguments.next())
                    .ok_or("--engine needs a value")?;
                launch.command.get_or_insert_with(LaunchRequest::default).engine =
                    Some(value.parse().map_err(|_| "unknown --engine value")?);
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if let Some(command) = &launch.command
        && (command.configuration.is_empty() || command.update.is_empty())
    {
        return Err("--configuration and --update are given together".into());
    }
    Ok(launch)
}

#[tauri::command]
fn launch_connection(state: tauri::State<'_, AppState>) -> Option<LaunchRequest> {
    state.command.lock().ok()?.take()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let connection = window_launch()?;
    tauri::Builder::default()
        .manage(AppState {
            running: Mutex::new(None),
    command: Mutex::new(connection.command),
        })
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let window = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("FujiMod Emulator")
            .inner_size(1280.0, 900.0)
            .min_inner_size(720.0, 600.0)
            .visible(false)
            .build()?;
            window.show()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            note,
            configurations,
            choose_update,
            launch,
            launch_connection,
            close
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}
