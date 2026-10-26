//! Browser bindings, declared once in Rust. wasm-bindgen generates the
//! JavaScript glue and tsify the TypeScript types; there is no hand-written ABI.
//! One module instance runs on one worker; calls are serialized by JavaScript.
mod camera;
mod commands;
mod display;
mod fingerprint;
mod history;
pub mod jit;
pub mod lifecycle;
pub mod results;
mod session;
mod snapshot;
use fujimod_emulator_boot::RecReport;
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_session::card::{CardState, CardTree};
use jit::{JitSelection, JitStatistics};
use lifecycle::BootInput;
use results::{
    CardFile, Checkpoint, DeviceStates, DeviceWorkers, ErrorReport, Fingerprint, InputLog, Point,
    Preview, Profile, SavedMedia, SessionSnapshot, SessionState, TouchPhase, UsbStatus,
};
use serde::{Deserialize, Serialize};
pub use session::Session;
use std::{cell::Cell, sync::Arc};
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;

thread_local! { static POLICY: Cell<Option<ExecutionPolicy>> = const { Cell::new(None) }; }

/// Thrown to JavaScript as an `Error`. Its name is `GuestExecutionError` once
/// the guest has failed; the session then only reports diagnostics.
pub struct Failure {
    message: String,
    guest: bool,
}
impl Failure {
    fn host(message: impl ToString) -> Self {
        Self {
            message: message.to_string(),
            guest: false,
        }
    }
}
impl From<Failure> for JsValue {
    fn from(failure: Failure) -> Self {
        let error = js_sys::Error::new(&failure.message);
        if failure.guest {
            error.set_name("GuestExecutionError");
        }
        error.into()
    }
}

#[derive(Clone, Copy, Deserialize, Tsify)]
#[serde(rename_all = "lowercase")]
#[tsify(hashmap_as_object, missing_as_null)]
pub enum ExecutionPolicyName {
    /// Permit block-boundary overshoot for interactive runs (the default).
    Interactive,
    /// Stop at requested instruction boundaries, as native qualification does.
    Exact,
}
#[derive(Deserialize, Tsify)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct ModuleOptions {
    #[serde(default)]
    #[tsify(optional)]
    pub execution_policy: Option<ExecutionPolicyName>,
    #[serde(default)]
    #[tsify(optional)]
    pub tcg_jit: Option<JitSelection>,
}
/// Select the board/CPU execution policy and browser JIT tier, once, before any camera.
#[wasm_bindgen]
pub fn configure(options: Ts<ModuleOptions>) -> Result<(), Failure> {
    let options = options.to_rust().map_err(Failure::host)?;
    if POLICY.get().is_some() {
        return Err(Failure::host("the emulator module is already configured"));
    }
    jit::setup(jit::selection(options.tcg_jit).map_err(Failure::host)?).map_err(Failure::host)?;
    POLICY.set(Some(
        match options
            .execution_policy
            .unwrap_or(ExecutionPolicyName::Interactive)
        {
            ExecutionPolicyName::Interactive => ExecutionPolicy::Interactive,
            ExecutionPolicyName::Exact => ExecutionPolicy::Exact,
        },
    ));
    Ok(())
}
fn policy() -> Result<ExecutionPolicy, Failure> {
    POLICY
        .get()
        .ok_or_else(|| Failure::host("configure the emulator module before creating cameras"))
}
#[wasm_bindgen(js_name = jitStatistics)]
pub fn jit_statistics() -> Result<Ts<JitStatistics>, Failure> {
    jit::statistics().into_ts().map_err(Failure::host)
}
/// Current WebAssembly linear memory size in bytes.
#[wasm_bindgen(js_name = memoryBytes)]
pub fn memory_bytes() -> f64 {
    #[cfg(target_family = "wasm")]
    {
        (core::arch::wasm32::memory_size::<0>() * 65536) as f64
    }
    #[cfg(not(target_family = "wasm"))]
    {
        0.
    }
}

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct Status {
    #[wasm_bindgen(readonly)]
    pub ticks: u64,
    #[wasm_bindgen(readonly)]
    pub instructions: u64,
    #[wasm_bindgen(readonly)]
    pub failed: bool,
}
/// A presented frame. `rgba` views tightly packed RGBA8888 pixels, top row
/// first, in WebAssembly memory; reacquire it after any call that may grow memory.
#[wasm_bindgen]
pub struct Frame {
    frame: Arc<fujimod_emulator_devices::graphics::scanout::Frame>,
}
#[wasm_bindgen]
impl Frame {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.frame.width
    }
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.frame.height
    }
    #[wasm_bindgen(getter)]
    pub fn output(&self) -> usize {
        self.frame.output
    }
    /// Presentation change count, not a hardware counter.
    #[wasm_bindgen(getter)]
    pub fn sequence(&self) -> u64 {
        self.frame.sequence
    }
    #[wasm_bindgen(getter)]
    pub fn rgba(&self) -> js_sys::Uint8Array {
        // SAFETY: the view borrows pixels this object keeps alive; JavaScript
        // must not retain it past `free` or a memory growth.
        unsafe { js_sys::Uint8Array::view(&self.frame.rgba) }
    }
}

fn whole(value: f64, name: &str) -> Result<u64, Failure> {
    if value.fract() != 0. || !(0. ..=9007199254740991.).contains(&value) {
        return Err(Failure::host(format!(
            "{name} must be a safe nonnegative integer"
        )));
    }
    Ok(value as u64)
}
fn many<T: Tsify + serde::de::DeserializeOwned>(values: Vec<Ts<T>>) -> Result<Vec<T>, Failure>
where
    <T as Tsify>::JsType: Clone,
{
    values
        .into_iter()
        .map(|v| v.to_rust().map_err(Failure::host))
        .collect()
}

/// One emulated camera body.
#[wasm_bindgen]
pub struct Camera {
    session: Session,
}
impl Camera {
    fn new(session: Result<Session, String>) -> Result<Self, Failure> {
        Ok(Self {
            session: session.map_err(Failure::host)?,
        })
    }
    fn with<T>(&mut self, f: impl FnOnce(&mut Session) -> Result<T, String>) -> Result<T, Failure> {
        f(&mut self.session).map_err(|message| Failure {
            message,
            guest: self.session.failed,
        })
    }
    fn with_progress<T>(
        &mut self,
        progress: Option<js_sys::Function>,
        run: impl FnOnce(&mut Session) -> Result<T, String>,
    ) -> Result<T, Failure> {
        self.session
            .core
            .board
            .set_progress_observer(progress.map(|progress| {
                Box::new(move |instructions: u64| {
                    // Advisory host display only; a detached listener cannot change guest execution.
                    let _ = progress.call1(&JsValue::NULL, &JsValue::from_f64(instructions as f64));
                }) as Box<dyn FnMut(u64)>
            }));
        let result = self.with(run);
        self.session.core.board.set_progress_observer(None);
        result
    }
    fn typed<T: Tsify + Serialize>(
        &mut self,
        f: impl FnOnce(&mut Session) -> Result<T, String>,
    ) -> Result<Ts<T>, Failure> {
        self.with(f)?.into_ts().map_err(Failure::host)
    }
}
#[wasm_bindgen]
impl Camera {
    /// Construct from a typed processor/body/scenario document and verified firmware.
    #[wasm_bindgen(js_name = fromResolved)]
    pub fn from_resolved(config: &str, firmware: &[u8]) -> Result<Camera, Failure> {
        Self::new(Session::create(config.as_bytes(), firmware, policy()?))
    }
    /// Construct an EXR II camera from its device graph document and DAT.
    #[wasm_bindgen(js_name = fromGraph)]
    pub fn from_graph(config: &str, dat: &[u8]) -> Result<Camera, Failure> {
        Self::new(Session::create_camera(config.as_bytes(), dat, policy()?))
    }
    /// Construct from a native prepared run. `request` is the JSON text of
    /// `{document, clockSeconds2000?, preferredLanguages?, camera?}`, where
    /// `document` is the exact JSON text of `{schema, prepared, browser}` and
    /// `camera` lists `[section, length]` pairs of the camera's compressed NAND
    /// sections, packed in that order in `camera`.
    #[wasm_bindgen(js_name = fromPrepared)]
    pub fn from_prepared(
        request: &str,
        update: Vec<u8>,
        boot_rom: Vec<u8>,
        nand: Option<Vec<u8>>,
        settings: Option<Vec<u8>>,
        camera: Option<Vec<u8>>,
    ) -> Result<Camera, Failure> {
        if boot_rom.is_empty() {
            return Err(Failure::host("prepared session requires a boot ROM"));
        }
        Self::new(Session::create_prepared(
            request.as_bytes(),
            update,
            boot_rom,
            nand,
            settings,
            camera.as_deref().unwrap_or_default(),
            policy()?,
        ))
    }
    /// Reconstruct a running session from `exportSnapshot` or `errorReport` bytes.
    #[wasm_bindgen(js_name = fromSnapshot)]
    pub fn from_snapshot(bytes: &[u8]) -> Result<Camera, Failure> {
        policy()?;
        Self::new(Session::load_snapshot(bytes))
    }
    /// Finish device work and release the session.
    pub fn close(mut self) -> Result<(), Failure> {
        self.with(|s| s.core.board.finish())
    }
    #[wasm_bindgen(js_name = runUntil)]
    pub fn run_until(
        &mut self,
        tick: u64,
        #[wasm_bindgen(unchecked_optional_param_type = "(instructions: number) => void")] progress: Option<
            js_sys::Function,
        >,
    ) -> Result<(), Failure> {
        let result = self.with_progress(progress, |s| s.run(tick));
        #[cfg(target_os = "emscripten")]
        fujimod_emulator_cpu_unicorn::jit::emscripten::flush();
        result.map_err(|error| Failure {
            message: error.message,
            guest: true,
        })
    }
    /// Queue a physical key edge at guest tick `at` (resolved bodies).
    pub fn input(&mut self, key: &str, pressed: bool, at: u64) -> Result<(), Failure> {
        self.with(|s| s.input(key, pressed, at))
    }
    pub fn status(&self) -> Status {
        self.session.status()
    }
    /// The latest presented frame of `output`, or `undefined` before the first.
    pub fn frame(&mut self, output: usize) -> Result<Option<Frame>, Failure> {
        self.with(|s| Ok(s.frame(output)?.map(|frame| Frame { frame })))
    }
    /// Show RGB888 pixels, tightly packed and top row first, as the scene.
    #[wasm_bindgen(js_name = setScene)]
    pub fn set_scene(&mut self, width: u32, height: u32, rgb: Vec<u8>) -> Result<(), Failure> {
        self.with(|s| s.set_scene(Some((width, height, rgb))))
    }
    /// Restore the analytic scene.
    #[wasm_bindgen(js_name = clearScene)]
    pub fn clear_scene(&mut self) -> Result<(), Failure> {
        self.with(|s| s.set_scene(None))
    }
    /// Use a RAF's mosaic as the sensor scene.
    #[wasm_bindgen(js_name = sceneRaf)]
    pub fn scene_raf(&mut self, raf: &[u8]) -> Result<(), Failure> {
        self.with(|s| s.scene_raf(raf))
    }
    pub fn state(&mut self) -> Result<Ts<SessionState>, Failure> {
        self.typed(|s| s.session_state())
    }
    #[wasm_bindgen(js_name = usbStatus)]
    pub fn usb_status(&mut self, node: &str) -> Result<Ts<UsbStatus>, Failure> {
        self.typed(|s| s.usb_status(node))
    }
    #[wasm_bindgen(js_name = usbReset)]
    pub fn usb_reset(&mut self, node: &str) -> Result<(), Failure> {
        self.with(|s| s.usb_reset(node))
    }
    #[wasm_bindgen(js_name = usbSetup)]
    pub fn usb_setup(&mut self, node: &str, packet: &[u8]) -> Result<(), Failure> {
        self.with(|s| s.usb_setup(node, packet))
    }
    #[wasm_bindgen(js_name = usbOut)]
    pub fn usb_out(&mut self, node: &str, endpoint: u8, packet: &[u8]) -> Result<(), Failure> {
        self.with(|s| s.usb_out(node, endpoint, packet))
    }
    #[wasm_bindgen(js_name = usbIn)]
    pub fn usb_in(&mut self, node: &str, endpoint: u8, size: usize) -> Result<Vec<u8>, Failure> {
        self.with(|s| s.usb_in(node, endpoint, size))
    }
    #[wasm_bindgen(js_name = usbInToken)]
    pub fn usb_in_token(&mut self, node: &str, endpoint: u8) -> Result<(), Failure> {
        self.with(|s| s.usb_in_token(node, endpoint))
    }
    #[wasm_bindgen(js_name = cardState)]
    pub fn card_state(&mut self) -> Result<Ts<CardState>, Failure> {
        self.typed(|s| s.card_state())
    }
    #[wasm_bindgen(js_name = cardTree)]
    pub fn card_tree(&mut self, revision: Option<f64>) -> Result<Ts<CardTree>, Failure> {
        let revision = revision.map(|r| whole(r, "revision")).transpose()?;
        self.typed(|s| s.card_tree(revision))
    }
    /// Read a file from the card; `maxBytes` defaults to 64 MiB.
    #[wasm_bindgen(js_name = cardRead)]
    pub fn card_read(
        &mut self,
        path: &str,
        revision: Option<f64>,
        max_bytes: Option<usize>,
    ) -> Result<Vec<u8>, Failure> {
        let revision = revision.map(|r| whole(r, "revision")).transpose()?;
        self.with(|s| s.card_read(path, revision, max_bytes))
    }
    #[wasm_bindgen(js_name = cardInsert)]
    pub fn card_insert(&mut self, present: bool) -> Result<Ts<SessionState>, Failure> {
        self.typed(|s| s.card_insert(present))
    }
    #[wasm_bindgen(js_name = cardFormat)]
    pub fn card_format(&mut self) -> Result<Ts<SessionState>, Failure> {
        self.typed(|s| s.card_format())
    }
    #[wasm_bindgen(js_name = setPosition)]
    pub fn set_position(
        &mut self,
        id: String,
        position: String,
    ) -> Result<Ts<SessionState>, Failure> {
        self.typed(|s| s.set_position(id, position))
    }
    pub fn press(
        &mut self,
        id: String,
        stage: Option<String>,
    ) -> Result<Ts<SessionState>, Failure> {
        self.typed(|s| s.press(id, stage))
    }
    /// Release power-on holds directly through the declared physical contacts.
    #[wasm_bindgen(js_name = releaseBootInputs)]
    pub fn release_boot_inputs(
        &mut self,
        inputs: Vec<Ts<BootInput>>,
    ) -> Result<Ts<SessionState>, Failure> {
        let inputs = many(inputs)?;
        self.typed(|s| s.release_boot_inputs(&inputs))
    }
    pub fn release(&mut self, id: String) -> Result<Ts<SessionState>, Failure> {
        self.typed(|s| s.release(id))
    }
    pub fn rotate(&mut self, id: &str, steps: i32) -> Result<Ts<SessionState>, Failure> {
        self.typed(|s| s.rotate(id, steps))
    }
    pub fn touch(
        &mut self,
        id: &str,
        phase: Ts<TouchPhase>,
        x: u16,
        y: u16,
    ) -> Result<Ts<SessionState>, Failure> {
        let phase = phase.to_rust().map_err(Failure::host)?;
        self.typed(|s| s.touch(id, phase, x, y))
    }
    pub fn power(&mut self, on: bool) -> Result<Ts<SessionState>, Failure> {
        self.typed(|s| s.set_power(on))
    }
    /// Power off and on again, holding `inputs` during the boot.
    #[wasm_bindgen(js_name = powerCycle)]
    pub fn power_cycle(&mut self, inputs: Vec<Ts<BootInput>>) -> Result<Ts<SessionState>, Failure> {
        let inputs = many(inputs)?;
        self.typed(|s| s.cycle_power(&inputs))
    }
    /// Replace the card with `files`, packed contiguously into `upload`, and boot.
    #[wasm_bindgen(js_name = replaceCard)]
    pub fn replace_card(
        &mut self,
        files: Vec<Ts<CardFile>>,
        upload: &[u8],
        inputs: Vec<Ts<BootInput>>,
    ) -> Result<Ts<SessionState>, Failure> {
        let (files, inputs) = (many(files)?, many(inputs)?);
        self.typed(|s| s.replace_card(files, upload, &inputs))
    }
    /// Export committed storage; restore it with `restoreDurable`.
    pub fn save(&mut self) -> Result<Ts<SavedMedia>, Failure> {
        self.typed(|s| s.save_durable())
    }
    #[wasm_bindgen(js_name = restoreDurable)]
    pub fn restore_durable(&mut self, durable: &[u8]) -> Result<Ts<SessionState>, Failure> {
        self.typed(|s| s.load_durable(durable))
    }
    pub fn checkpoint(&mut self) -> Result<Ts<Checkpoint>, Failure> {
        self.typed(|s| s.checkpoint())
    }
    pub fn restore(&mut self, id: &str) -> Result<Ts<Point>, Failure> {
        self.typed(|s| s.restore_checkpoint(id))
    }
    pub fn undo(&mut self) -> Result<Ts<Point>, Failure> {
        self.typed(|s| s.undo())
    }
    pub fn rollback(&mut self, tick: u64) -> Result<Ts<Point>, Failure> {
        self.typed(|s| s.rollback(tick))
    }
    pub fn history(&mut self) -> Result<Ts<InputLog>, Failure> {
        self.typed(|s| s.history_log())
    }
    pub fn replay(&mut self, log: &[u8]) -> Result<Ts<Point>, Failure> {
        self.typed(|s| s.replay(log))
    }
    pub fn fingerprint(&mut self) -> Result<Ts<Fingerprint>, Failure> {
        self.typed(|s| s.fingerprint())
    }
    /// Start the declared preview clock.
    pub fn preview(&mut self) -> Result<Ts<Preview>, Failure> {
        self.typed(|s| s.preview(true))
    }
    #[wasm_bindgen(js_name = previewState)]
    pub fn preview_state(&mut self) -> Result<Ts<Preview>, Failure> {
        self.typed(|s| s.preview(false))
    }
    /// Set the stopped host worker preference (0 or 2) and report it.
    #[wasm_bindgen(js_name = deviceWorkers)]
    pub fn device_workers(&mut self, count: Option<usize>) -> Result<Ts<DeviceWorkers>, Failure> {
        self.typed(|s| s.device_workers(count))
    }
    #[wasm_bindgen(js_name = deviceStates)]
    pub fn device_states(&mut self, kind: &str) -> Result<Ts<DeviceStates>, Failure> {
        self.typed(|s| s.device_states(kind))
    }
    #[wasm_bindgen(js_name = memoryRead)]
    pub fn memory_read(&mut self, address: u64, size: usize) -> Result<Vec<u8>, Failure> {
        self.with(|s| s.memory_read(address, size))
    }
    #[wasm_bindgen(js_name = watchPcs)]
    pub fn watch_pcs(&mut self, addresses: Vec<u32>) -> Result<(), Failure> {
        let addresses: Vec<u64> = addresses.into_iter().map(u64::from).collect();
        self.with(|s| s.watch_pcs(&addresses))
    }
    pub fn profile(&mut self, limit: Option<usize>) -> Result<Ts<Profile>, Failure> {
        self.typed(|s| s.profile(limit))
    }
    #[wasm_bindgen(js_name = errorReport)]
    pub fn error_report(&mut self) -> Result<Ts<ErrorReport>, Failure> {
        self.typed(|s| s.error_report())
    }
    #[wasm_bindgen(js_name = exportSnapshot)]
    pub fn export_snapshot(&mut self) -> Result<Ts<SessionSnapshot>, Failure> {
        self.typed(|s| s.export_snapshot())
    }
    /// Native first-start and REC observation, bounded by an instruction budget.
    #[wasm_bindgen(js_name = bootToRec)]
    pub fn boot_to_rec(
        &mut self,
        limit: u64,
        #[wasm_bindgen(unchecked_optional_param_type = "(instructions: number) => void")] progress: Option<
            js_sys::Function,
        >,
    ) -> Result<Ts<RecReport>, Failure> {
        self.with_progress(progress, |s| s.boot_to_rec(limit))?
            .into_ts()
            .map_err(Failure::host)
    }
}
