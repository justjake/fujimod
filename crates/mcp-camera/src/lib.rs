//! The MCP camera module: boot emulated cameras, keep them running between
//! tool calls, drive their controls, look at their screens and memory,
//! checkpoint them and profile what their processors execute.
//!
//! A session is not `Send`, so each camera lives on its own thread. Tools send
//! that thread a closure and await its reply.
#![forbid(unsafe_code)]

mod camera;
mod profile;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};

use base64::Engine as _;
use fujimod_mcp_core::{Module, Routed};
use rmcp::{
    ErrorData,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use camera::{Camera, OpenRequest};
use profile::Profile;

const INSTRUCTIONS: &str = "Emulated cameras that stay running between calls. Open one with \
camera_open (it boots to REC from the library's boot cache when one matches, or restores a \
checkpoint file), list its controls with camera_controls, then operate them with camera_press, \
camera_turn or camera_set and read the reply's screen and state. Save named checkpoints with \
camera_checkpoint and return to them with camera_restore \
instead of booting again. camera_show opens a camera in the desktop emulator window, where it \
runs live. To find what code runs, call camera_profile_start, act, then \
camera_profile_collect; compare two collections with camera_profile_diff.";

type Job = Box<dyn FnOnce(&mut Camera) + Send>;

struct Handle {
    jobs: mpsc::Sender<Job>,
    opened: Value,
}

/// State shared by the camera tools.
pub struct Cameras {
    cameras: Mutex<BTreeMap<String, Handle>>,
    profiles: Mutex<BTreeMap<String, Arc<Profile>>>,
    output: PathBuf,
    next_camera: Mutex<u64>,
    next_profile: Mutex<u64>,
}

/// The camera module. `output` receives profiles and other large results.
pub fn module(output: PathBuf) -> Arc<dyn Module> {
    let state = Cameras {
        cameras: Mutex::new(BTreeMap::new()),
        profiles: Mutex::new(BTreeMap::new()),
        output,
        next_camera: Mutex::new(1),
        next_profile: Mutex::new(1),
    };
    Arc::new(Routed::new(
        "camera",
        INSTRUCTIONS,
        state,
        Cameras::tool_router(),
    ))
}

fn failed(message: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        message.into(),
    )]))
}

fn reply(summary: Value, png: Option<Vec<u8>>) -> Result<CallToolResult, ErrorData> {
    let mut content = vec![ContentBlock::text(
        serde_json::to_string_pretty(&summary).unwrap_or_else(|error| error.to_string()),
    )];
    if let Some(png) = png {
        content.push(ContentBlock::image(
            base64::engine::general_purpose::STANDARD.encode(png),
            "image/png",
        ));
    }
    Ok(CallToolResult::success(content))
}

fn parse_address(text: &str) -> Result<u64, String> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|_| format!("invalid address {text}"))
}

impl Cameras {
    /// Run `work` on the camera's thread and wait for its result.
    async fn on<T: Send + 'static>(
        &self,
        camera: &str,
        work: impl FnOnce(&mut Camera) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let jobs = self
            .cameras
            .lock()
            .map_err(|_| "camera table is poisoned")?
            .get(camera)
            .map(|handle| handle.jobs.clone())
            .ok_or_else(|| format!("no camera named {camera}; see camera_list"))?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        jobs.send(Box::new(move |camera: &mut Camera| {
            let _ = sender.send(work(camera));
        }))
        .map_err(|_| format!("camera {camera} has stopped"))?;
        receiver
            .await
            .map_err(|_| format!("camera {camera} stopped while working"))?
    }

    fn next(counter: &Mutex<u64>, prefix: &str) -> String {
        let mut next = counter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let name = format!("{prefix}-{next}");
        *next += 1;
        name
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenParams {
    /// Name to address this camera by in later calls. Defaults to camera-N.
    pub name: Option<String>,
    /// The camera to open: the same camera description the Python binding and
    /// the command line take. `body` is required; `firmware` defaults to the
    /// body's newest. Firmware files come from `FUJIMOD_FIRMWARE_LIBRARY`, or
    /// the current directory when it is unset; relative paths resolve from the
    /// server's current directory.
    pub camera: fujimod_camera::CameraDescription,
}

#[derive(Deserialize, JsonSchema)]
pub struct CameraParams {
    /// Camera name from camera_open or camera_list.
    pub camera: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CheckpointParams {
    /// Camera name.
    pub camera: String,
    /// Name to restore it by.
    pub name: String,
    /// Also write the checkpoint to this file, which camera_open can restore.
    pub file: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RestoreParams {
    /// Camera name.
    pub camera: String,
    /// Checkpoint name from camera_checkpoint.
    pub name: Option<String>,
    /// Or a checkpoint file written by a camera built from the same inputs.
    pub file: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RunParams {
    /// Camera name.
    pub camera: String,
    /// Guest seconds to run. Ignored with `until_rec`.
    pub seconds: Option<f64>,
    /// Run until the camera reports REC, within `instruction_limit`.
    #[serde(default)]
    pub until_rec: bool,
    /// Guest instruction budget for `until_rec`. Defaults to two billion.
    pub instruction_limit: Option<u64>,
    /// Return the screen after running. Defaults to true.
    pub screen: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PressParams {
    /// Camera name.
    pub camera: String,
    /// Control id from camera_controls, such as `menu_ok_button`, or a pad
    /// direction such as `selector.up`.
    pub control: String,
    /// Guest milliseconds to keep running after the firmware registers the
    /// release. Defaults to 500.
    pub settle_ms: Option<u64>,
    /// Return the screen afterwards. Defaults to true.
    pub screen: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct TurnParams {
    /// Camera name.
    pub camera: String,
    /// Rotary control id from camera_controls.
    pub control: String,
    /// Detents to turn; negative turns the other way.
    pub steps: i32,
    /// Guest milliseconds to keep running after the firmware registers the
    /// steps. Defaults to 500.
    pub settle_ms: Option<u64>,
    /// Return the screen afterwards. Defaults to true.
    pub screen: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SetParams {
    /// Camera name.
    pub camera: String,
    /// Dial, switch, ring or lever id from camera_controls.
    pub control: String,
    /// Position id from camera_controls, such as A or 1/500.
    pub position: String,
    /// Guest milliseconds to keep running after the firmware registers the
    /// position. Defaults to 500.
    pub settle_ms: Option<u64>,
    /// Return the screen afterwards. Defaults to true.
    pub screen: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MemoryParams {
    /// Camera name.
    pub camera: String,
    /// Physical address, decimal or `0x` hexadecimal.
    pub address: String,
    /// Bytes to read, at most 16 MiB.
    pub size: u64,
    /// Write the bytes to this file. Without it at most 4096 bytes are returned
    /// as a hex dump.
    pub file: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProfileParams {
    /// Profile id from camera_profile_collect.
    pub profile: String,
    /// Blocks to list. Defaults to 30.
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DiffParams {
    /// Baseline profile id.
    pub before: String,
    /// Profile id to compare with the baseline.
    pub after: String,
    /// Blocks to list. Defaults to 30.
    pub limit: Option<usize>,
}

#[tool_router]
impl Cameras {
    #[tool(
        description = "Open an emulated camera. `camera` is the camera description: `body` (a \
catalog body such as `xh2s`), optionally `firmware` (a version such as `7.30`, a firmware id, \
or `none`; the body's newest when omitted), `settings`, `sd_card`/`sd_card_2`/`cfexpress_card` \
(each `{present, write_protected, files | directory | image}`), `date`, `scene`, `engine` and \
`boot`: `stock`, `sd-card` (FujiMod from the SD card, or the current release card when it \
names no files) or `autostart`. It boots to REC (or restores `snapshot`) and stays running for \
later calls; a firmware without REC readiness facts is returned before its first instruction, \
saying so. Returns its name, state and screen."
    )]
    async fn camera_open(
        &self,
        Parameters(params): Parameters<OpenParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = match &params.name {
            Some(name) => name.clone(),
            None => Self::next(&self.next_camera, "camera"),
        };
        let request = OpenRequest::from(params);
        let (jobs, receive) = mpsc::channel::<Job>();
        // Reserve the name before booting, so a concurrent open of the same
        // name refuses instead of replacing this camera.
        match self.cameras.lock() {
            Ok(mut cameras) if !cameras.contains_key(&name) => {
                cameras.insert(
                    name.clone(),
                    Handle {
                        jobs: jobs.clone(),
                        opened: json!({ "camera": name, "opening": true }),
                    },
                );
            }
            Ok(_) => return failed(format!("a camera named {name} is already open")),
            Err(_) => return failed("camera table is poisoned"),
        }
        let release = |cameras: &Mutex<BTreeMap<String, Handle>>| {
            if let Ok(mut cameras) = cameras.lock() {
                cameras.remove(&name);
            }
        };
        let (opened, wait) = tokio::sync::oneshot::channel();
        let thread_name = name.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("camera {thread_name}"))
            .spawn(move || {
                let mut camera = match Camera::open(request) {
                    Ok((camera, summary)) => {
                        let _ = opened.send(Ok(summary));
                        camera
                    }
                    Err(error) => {
                        let _ = opened.send(Err(error));
                        return;
                    }
                };
                loop {
                    // A shown camera runs live between jobs; otherwise it waits.
                    let job = if camera.shown() {
                        match receive.try_recv() {
                            Ok(job) => job,
                            Err(mpsc::TryRecvError::Empty) => {
                                camera.serve_window();
                                continue;
                            }
                            Err(mpsc::TryRecvError::Disconnected) => break,
                        }
                    } else {
                        match receive.recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        }
                    };
                    job(&mut camera);
                }
            });
        if let Err(error) = spawned {
            release(&self.cameras);
            return failed(format!("cannot start the camera thread: {error}"));
        }
        let (summary, png) = match wait.await {
            Ok(Ok(opened)) => opened,
            Ok(Err(error)) => {
                release(&self.cameras);
                return failed(error);
            }
            Err(_) => {
                release(&self.cameras);
                return failed("the camera thread stopped while opening");
            }
        };
        let mut summary = summary;
        summary["camera"] = json!(name);
        if let Ok(mut cameras) = self.cameras.lock()
            && let Some(handle) = cameras.get_mut(&name)
        {
            handle.opened = summary.clone();
        }
        reply(summary, png)
    }

    #[tool(description = "List open cameras and how each was opened.")]
    async fn camera_list(&self) -> Result<CallToolResult, ErrorData> {
        let cameras = match self.cameras.lock() {
            Ok(cameras) => cameras
                .iter()
                .map(|(name, handle)| (name.clone(), handle.opened.clone()))
                .collect::<BTreeMap<_, _>>(),
            Err(_) => return failed("camera table is poisoned"),
        };
        reply(json!({ "cameras": cameras }), None)
    }

    #[tool(
        description = "Close a camera and release its memory. Its in-memory checkpoints are lost; \
checkpoint files remain."
    )]
    async fn camera_close(
        &self,
        Parameters(CameraParams { camera }): Parameters<CameraParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .cameras
            .lock()
            .map(|mut cameras| cameras.remove(&camera))
        {
            Ok(Some(_)) => reply(json!({ "closed": camera }), None),
            Ok(None) => failed(format!("no camera named {camera}")),
            Err(_) => failed("camera table is poisoned"),
        }
    }

    #[tool(
        description = "List the camera's controls: ids for camera_press and camera_turn, their \
kinds, positions, and why any control is unavailable."
    )]
    async fn camera_controls(
        &self,
        Parameters(CameraParams { camera }): Parameters<CameraParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.on(&camera, |camera| camera.controls()).await {
            Ok(controls) => reply(json!({ "controls": controls }), None),
            Err(error) => failed(error),
        }
    }

    #[tool(
        description = "Save the whole machine under a name, optionally also to a file, so \
experiments can return to this exact point with camera_restore."
    )]
    async fn camera_checkpoint(
        &self,
        Parameters(params): Parameters<CheckpointParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let CheckpointParams { camera, name, file } = params;
        let result = self
            .on(&camera, move |camera| {
                camera.checkpoint(&name, file.as_deref())
            })
            .await;
        match result {
            Ok(summary) => reply(summary, None),
            Err(error) => failed(error),
        }
    }

    #[tool(
        description = "Return a camera to a named checkpoint or a checkpoint file. Returns the \
restored state and screen."
    )]
    async fn camera_restore(
        &self,
        Parameters(params): Parameters<RestoreParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let RestoreParams { camera, name, file } = params;
        let result = self
            .on(&camera, move |camera| {
                camera.restore(name.as_deref(), file.as_deref())?;
                camera.observe(true)
            })
            .await;
        match result {
            Ok((summary, png)) => reply(summary, png),
            Err(error) => failed(error),
        }
    }

    #[tool(
        description = "Advance guest time by `seconds`, or until the camera reaches REC. Returns \
the state and screen."
    )]
    async fn camera_run(
        &self,
        Parameters(params): Parameters<RunParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let RunParams {
            camera,
            seconds,
            until_rec,
            instruction_limit,
            screen,
        } = params;
        let result = self
            .on(&camera, move |camera| {
                if until_rec {
                    camera.run_to_rec(instruction_limit.unwrap_or(2_000_000_000))?;
                } else {
                    let seconds = seconds.ok_or("give seconds or until_rec")?;
                    camera.run_for(seconds)?;
                }
                camera.observe(screen.unwrap_or(true))
            })
            .await;
        match result {
            Ok((summary, png)) => reply(summary, png),
            Err(error) => failed(error),
        }
    }

    #[tool(
        description = "Press and release a control. Waits until the firmware is ready for input \
and confirms its key scanner registered the press and the release, keeps running for \
`settle_ms`, and returns the new state and screen."
    )]
    async fn camera_press(
        &self,
        Parameters(params): Parameters<PressParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let PressParams {
            camera,
            control,
            settle_ms,
            screen,
        } = params;
        let result = self
            .on(&camera, move |camera| {
                camera.press(&control, settle_ms.unwrap_or(500))?;
                camera.observe(screen.unwrap_or(true))
            })
            .await;
        match result {
            Ok((summary, png)) => reply(summary, png),
            Err(error) => failed(error),
        }
    }

    #[tool(
        description = "Move a dial, switch, ring or lever to a named position through the \
contact driver. Waits until the firmware's own observation of the control shows the new pose, \
keeps running for `settle_ms`, and returns the new state and screen. Buttons press with \
camera_press; rotary detents turn with camera_turn."
    )]
    async fn camera_set(
        &self,
        Parameters(params): Parameters<SetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let SetParams {
            camera,
            control,
            position,
            settle_ms,
            screen,
        } = params;
        let result = self
            .on(&camera, move |camera| {
                let set = camera.set(&control, &position, settle_ms.unwrap_or(500))?;
                let (mut summary, png) = camera.observe(screen.unwrap_or(true))?;
                if let Some(object) = summary.as_object_mut() {
                    object.insert("set".into(), set);
                }
                Ok::<(Value, Option<Vec<u8>>), String>((summary, png))
            })
            .await;
        match result {
            Ok((summary, png)) => reply(summary, png),
            Err(error) => failed(error),
        }
    }

    #[tool(
        description = "Turn a rotary control by detents, keep running for `settle_ms`, and \
return the new state and screen."
    )]
    async fn camera_turn(
        &self,
        Parameters(params): Parameters<TurnParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let TurnParams {
            camera,
            control,
            steps,
            settle_ms,
            screen,
        } = params;
        let result = self
            .on(&camera, move |camera| {
                camera.turn(&control, steps, settle_ms.unwrap_or(500))?;
                camera.observe(screen.unwrap_or(true))
            })
            .await;
        match result {
            Ok((summary, png)) => reply(summary, png),
            Err(error) => failed(error),
        }
    }

    #[tool(
        description = "Show the camera in the desktop emulator window. While the window is open the \
camera runs live between tool calls and the window's controls operate it."
    )]
    async fn camera_show(
        &self,
        Parameters(params): Parameters<CameraParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.on(&params.camera, |camera| camera.show()).await {
            Ok(summary) => reply(summary, None),
            Err(error) => failed(error),
        }
    }

    #[tool(description = "The camera's current screen and state, without advancing guest time.")]
    async fn camera_screen(
        &self,
        Parameters(CameraParams { camera }): Parameters<CameraParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.on(&camera, |camera| camera.observe(true)).await {
            Ok((summary, png)) => reply(summary, png),
            Err(error) => failed(error),
        }
    }

    #[tool(
        description = "The camera's UI and boot state words, guest time and instruction counts, \
without a screen and without advancing guest time."
    )]
    async fn camera_state(
        &self,
        Parameters(CameraParams { camera }): Parameters<CameraParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.on(&camera, |camera| camera.observe(false)).await {
            Ok((summary, _)) => reply(summary, None),
            Err(error) => failed(error),
        }
    }

    #[tool(description = "Read physical memory without advancing guest time.")]
    async fn camera_read_memory(
        &self,
        Parameters(params): Parameters<MemoryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let MemoryParams {
            camera,
            address,
            size,
            file,
        } = params;
        let address = match parse_address(&address) {
            Ok(address) => address,
            Err(error) => return failed(error),
        };
        if size == 0 || size > 16 << 20 {
            return failed("size must be 1 byte to 16 MiB");
        }
        if file.is_none() && size > 4096 {
            return failed("reads over 4096 bytes need a file");
        }
        let result = self
            .on(&camera, move |camera| {
                camera.read_memory(address, size as usize)
            })
            .await;
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) => return failed(error),
        };
        if let Some(file) = file {
            if let Err(error) = std::fs::write(&file, &bytes) {
                return failed(format!("{file}: {error}"));
            }
            return reply(
                json!({ "address": format!("{address:#x}"), "size": size, "file": file }),
                None,
            );
        }
        let dump = bytes
            .chunks(16)
            .enumerate()
            .map(|(row, chunk)| {
                let hex = chunk
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{:#010x}  {hex}", address + row as u64 * 16)
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(CallToolResult::success(vec![ContentBlock::text(dump)]))
    }

    #[tool(
        description = "Start counting the instructions every processor executes, by basic block. \
Act on the camera, then call camera_profile_collect."
    )]
    async fn camera_profile_start(
        &self,
        Parameters(CameraParams { camera }): Parameters<CameraParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.on(&camera, |camera| camera.profile_start()).await {
            Ok(()) => reply(json!({ "camera": camera, "profiling": true }), None),
            Err(error) => failed(error),
        }
    }

    #[tool(
        description = "Stop the camera's profile and summarize it: blocks ranked by instructions \
executed. The full profile is written to a file; the returned id works with camera_profile_diff."
    )]
    async fn camera_profile_collect(
        &self,
        Parameters(CameraParams { camera }): Parameters<CameraParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let raw = match self.on(&camera, |camera| camera.profile_collect()).await {
            Ok(raw) => raw,
            Err(error) => return failed(error),
        };
        let id = Self::next(&self.next_profile, "profile");
        let path = self.output.join(format!("{id}.json"));
        let profile = match Profile::new(&id, &camera, raw, &path) {
            Ok(profile) => Arc::new(profile),
            Err(error) => return failed(error),
        };
        let summary = profile.summary(30);
        if let Ok(mut profiles) = self.profiles.lock() {
            profiles.insert(id, profile);
        }
        reply(summary, None)
    }

    #[tool(description = "Summarize a collected profile again with a different number of blocks.")]
    async fn camera_profile_show(
        &self,
        Parameters(ProfileParams { profile, limit }): Parameters<ProfileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .profiles
            .lock()
            .ok()
            .and_then(|profiles| profiles.get(&profile).cloned())
        {
            Some(found) => reply(found.summary(limit.unwrap_or(30)), None),
            None => failed(format!("no profile {profile}")),
        }
    }

    #[tool(
        description = "Compare two profiles, for example the same run with and without a key \
press, or stock firmware against a mod. Lists the blocks whose executed instructions changed most."
    )]
    async fn camera_profile_diff(
        &self,
        Parameters(DiffParams {
            before,
            after,
            limit,
        }): Parameters<DiffParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (before, after) = {
            let Ok(profiles) = self.profiles.lock() else {
                return failed("profile table is poisoned");
            };
            match (
                profiles.get(&before).cloned(),
                profiles.get(&after).cloned(),
            ) {
                (Some(a), Some(b)) => (a, b),
                (None, _) => return failed(format!("no profile {before}")),
                (_, None) => return failed(format!("no profile {after}")),
            }
        };
        reply(profile::diff(&before, &after, limit.unwrap_or(30)), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_set_takes_a_control_and_a_position() {
        let tools = module(std::env::temp_dir()).tools();
        let tool = tools
            .iter()
            .find(|tool| tool.name == "camera_set")
            .expect("camera_set is declared");
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        let mut required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect();
        required.sort_unstable();
        assert_eq!(required, ["camera", "control", "position"]);
    }

    #[test]
    fn camera_open_takes_a_name_and_the_open_request() {
        let tools = module(std::env::temp_dir()).tools();
        let tool = tools
            .iter()
            .find(|tool| tool.name == "camera_open")
            .expect("camera_open is declared");
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        let mut names: Vec<_> = properties.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["camera", "name"]);
        let required: Vec<_> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert_eq!(required, ["camera"]);
        assert_eq!(schema["additionalProperties"], json!(false));
        let request = &schema["$defs"]["CameraDescription"];
        assert_eq!(request["required"], json!(["body"]));
        let fields = request["properties"].as_object().unwrap();
        for field in [
            "body",
            "firmware",
            "settings",
            "sd_card",
            "sd_card_2",
            "cfexpress_card",
            "date",
            "scene",
            "engine",
            "boot",
            "snapshot",
        ] {
            assert!(fields.contains_key(field), "{field} is not exposed");
        }
        for removed in [
            "configuration",
            "update",
            "sd_card_files",
            "camera_settings",
        ] {
            assert!(!fields.contains_key(removed), "{removed} is still exposed");
        }
    }
}
