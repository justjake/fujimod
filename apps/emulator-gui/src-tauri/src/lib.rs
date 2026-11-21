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

#[derive(Deserialize)]
struct LaunchRequest {
    configuration: String,
    update: String,
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
    standalone: bool,
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<serde_json::Value, String>>>,
}

#[derive(Default)]
struct AppState {
    running: Mutex<Option<Running>>,
    launch: Mutex<Option<LaunchedCamera>>,
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
fn configurations() -> Vec<Configuration> {
    fujimod_catalog_configuration::RunConfiguration::bundled_ids()
        .filter_map(|id| {
            let configuration = ResolvedConfiguration::bundled(id).ok()?;
            let run = &configuration.configuration;
            if !matches!(run.start, RunStart::Loader | RunStart::Rom)
                || !matches!(run.inputs, Some(RunInputs::Xp5(_)))
                || run.preset.as_ref().and_then(|preset| preset.firmware_profile_id.as_ref()).is_none()
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

fn launch_camera(request: LaunchRequest) -> Result<(LaunchedCamera, Running), String> {
    let resolved = ResolvedConfiguration::bundled(&request.configuration)?;
    if !matches!(resolved.configuration.start, RunStart::Loader | RunStart::Rom)
        || !matches!(resolved.configuration.inputs, Some(RunInputs::Xp5(_)))
    {
        return Err("This camera cannot start from the firmware file alone.".into());
    }
    let configuration = Configuration::from_resolved(&resolved)?;
    let update = PathBuf::from(request.update);
    let metadata = std::fs::metadata(&update).map_err(|error| format!("{}: {error}", update.display()))?;
    if metadata.len() != configuration.update_bytes {
        return Err(format!(
            "{} is not the firmware for {}.",
            update.display(),
            configuration.model
        ));
    }
    let shipped = std::fs::read(&update).map_err(|error| format!("{}: {error}", update.display()))?;
    use sha2::{Digest, Sha256};
    if format!("{:x}", Sha256::digest(&shipped)) != configuration.update_sha256 {
        return Err(format!(
            "{} is not the firmware for {}.",
            update.display(),
            configuration.model
        ));
    }
    let output = output_directory(&configuration.configuration)?;
    let socket = fujimod_emulator_gui::bind(std::net::Ipv4Addr::LOCALHOST.into())?;
    let endpoint = socket.endpoint().url.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = thread::Builder::new()
        .name("fujimod-emulator-session".into())
        .spawn(move || {
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
            let opened = fujimod_emulator_boot::xp5::open_gui(run)?;
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
        })
        .map_err(|error| error.to_string())?;
    let launched = LaunchedCamera {
        id: endpoint.clone(),
        configuration: configuration.configuration.clone(),
        body: configuration.body,
        model: configuration.model,
        firmware: configuration.firmware,
        endpoint,
        standalone: true,
    };
    Ok((launched, Running { stop, thread: Some(thread) }))
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
    if let Ok(mut pending) = state.launch.lock() {
        *pending = Some(launched.clone());
    }
    Ok(launched)
}

#[tauri::command]
fn launch_command(state: tauri::State<'_, AppState>) -> Option<LaunchedCamera> {
    let mut pending = state.launch.lock().ok()?;
    if pending.is_none() {
        let (configuration, update) = std::env::args().skip(1).find_map(|argument| {
            let launch = argument.strip_prefix("--launch=")?;
            launch.split_once(',').map(|(configuration, update)| (configuration.to_owned(), update.to_owned()))
        })?;
        let camera = match launch_camera(LaunchRequest { configuration, update }) {
            Ok((camera, running)) => {
                *state.running.lock().ok()? = Some(running);
                camera
            }
            Err(error) => {
                eprintln!("fujimod-emulator-gui: {error}");
                return None;
            }
        };
        return Some(camera);
    }
    pending.take()
}

/// A CLI-started window receives its complete camera in the page URL.
#[tauri::command]
fn launch_cli() -> Option<LaunchedCamera> {
    None
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            configurations,
            choose_update,
            launch,
            launch_command,
            launch_cli,
            close
        ])
        .run(tauri::generate_context!())
        .expect("error while running FujiMod Emulator");
}
