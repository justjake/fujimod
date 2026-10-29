//! OpenVG-style vector GPU shell. Logical offsets double on the host bus.
//! Pattern fill controls use explicit field masks; copy pixels retain tile
//! bytes, and clear lists use decoded rectangles. All UI pixel, AA, stroke,
//! snapshot and throughput laws
//! remain declared software assumptions, not measured hardware semantics.
//! Command validation, records and completion tokens live here.
pub mod construct;
pub mod recording;
pub mod replay;
#[cfg(not(target_os = "emscripten"))]
mod ui_paint;
#[cfg(not(target_os = "emscripten"))]
use fujimod_emulator_gpu::CoverageKernel;
use fujimod_emulator_graphics::{batch, clear, init, regs::Registers, render, stream};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
mod device;
pub use device::{Options, VectorGpu, Wiring};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Profile {
    #[serde(rename = "vg-32bpp-untile-unrotated")]
    GpuCopyProfile,
    #[serde(rename = "stock-vg-clear-only")]
    GpuClearProfile,
    #[serde(rename = "declared-vg-ui-primitives-unmeasured")]
    GpuUiProfile,
}
impl Profile {
    pub const fn name(self) -> &'static str {
        match self {
            Self::GpuCopyProfile => COPY_PROFILE,
            Self::GpuClearProfile => CLEAR_PROFILE,
            Self::GpuUiProfile => UI_PROFILE,
        }
    }
    fn value(self) -> Value {
        Value::constant(self.name())
    }
}
use fujimod_emulator_devices::record::Result;
use fujimod_emulator_devices::{Action, Record, Unsupported, Value, WordMap};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
#[cfg(not(target_os = "emscripten"))]
use std::sync::OnceLock;

pub const BASE: u64 = 0xfff78000;
pub const SIZE: u64 = 0x5000;
pub const IRQ: u8 = 0x3c;
pub const FILL_STATUS: u32 = 1 << 26;
pub const COPY_STATUS: u32 = 1 << 30;
pub const CLEAR_STATUS: u32 = 1 << 31;
pub const FILL_CONTROL: u32 = 0x0f100201;
/// Tile bytes are retained unchanged by the optional untile profile.
pub const COPY_PROFILE: &str = "vg-32bpp-untile-unrotated";
pub const CLEAR_PROFILE: &str = "stock-vg-clear-only";
pub const UI_PROFILE: &str = "declared-vg-ui-primitives-unmeasured";
#[cfg(target_os = "emscripten")]
static WEBGPU_BRIDGE_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Select asynchronous host WebGPU rendering for vector-device submissions.
pub fn configure_webgpu_bridge(enabled: bool) {
    #[cfg(target_os = "emscripten")]
    WEBGPU_BRIDGE_ENABLED.store(enabled, std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(target_os = "emscripten"))]
    let _ = enabled;
}

pub(crate) fn webgpu_bridge_enabled() -> bool {
    #[cfg(target_os = "emscripten")]
    {
        WEBGPU_BRIDGE_ENABLED.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(not(target_os = "emscripten"))]
    {
        false
    }
}
const COMMAND: [u32; 6] = [0x10d4, 0x10dc, 0x10e4, 0x10ec, 0x20e0, 0x20e8];
const COPY: [u32; 8] = [0xc00, 0xc04, 0xc08, 0xc0c, 0xc10, 0xc20, 0xc24, 0xc28];
// Stock GL submits its shader/uniform command list through logical register 0x20f0.
const GLES_SUBMIT: u32 = 0x20f0;
const TRIGGERS: [u32; 3] = [0x10d0, 0x10e0, GLES_SUBMIT];
pub type DmaOutput = Vec<(u64, Arc<Vec<u8>>)>;
pub type RenderedBatch = (DmaOutput, u64);
pub type Fields = Vec<(&'static str, Value)>;
pub type JobFields = Vec<(Cow<'static, str>, Value)>;

/// Pending GPU DMA is immutable checkpoint data. Encode each payload as a
/// binary string so CBOR snapshots do not serialize every pixel byte as an
/// integer; JSON fingerprints retain their usual byte-array representation.
mod dma_output_bytes {
    use super::DmaOutput;
    use serde::{Deserialize, Serializer, de::Deserializer, ser::SerializeSeq};
    use std::sync::Arc;

    struct Bytes<'a>(&'a [u8]);

    impl serde::Serialize for Bytes<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_bytes(self.0)
        }
    }

    pub fn serialize<S: Serializer>(output: &DmaOutput, serializer: S) -> Result<S::Ok, S::Error> {
        let mut entries = serializer.serialize_seq(Some(output.len()))?;
        for (address, bytes) in output {
            entries.serialize_element(&(*address, Bytes(bytes.as_slice())))?;
        }
        entries.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DmaOutput, D::Error> {
        Ok(Vec::<(u64, Vec<u8>)>::deserialize(deserializer)?
            .into_iter()
            .map(|(address, bytes)| (address, Arc::new(bytes)))
            .collect())
    }
}
/// Owned receipt state. The binding retains Python job-view identity by id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub revision: u64,
    pub fields: JobFields,
    #[serde(skip, default = "record_receipts_default")]
    record_receipts: bool,
}
fn record_receipts_default() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum PendingWebGpu {
    Gles {
        id: u64,
        cpu: i64,
        irq_cpu: i64,
        command_address: u64,
        command_bytes: u64,
        job: Job,
    },
    Ui {
        id: u64,
        cpu: i64,
        irq_cpu: i64,
        draw_count: usize,
        filled_paths: usize,
        stroked_paths: usize,
        sampled_images: usize,
        linear_gradients: usize,
        radial_gradients: usize,
        job: Job,
    },
}
fn fail<T>(s: impl Into<String>) -> Result<T> {
    Err(Unsupported(s.into()))
}
fn hex(n: impl Into<u128>) -> Value {
    Value::hex(n.into())
}
fn num(n: impl Into<i128>) -> Value {
    Value::int(n)
}
fn text(s: &str) -> Value {
    Value::str(s)
}
fn set(fields: &mut JobFields, k: &'static str, v: Value) {
    if let Some((_, old)) = fields.iter_mut().find(|(key, _)| *key == k) {
        *old = v;
    } else {
        fields.push((Cow::Borrowed(k), v));
    }
}
fn update(j: &mut Job, fields: Fields) {
    if !j.record_receipts {
        return;
    }
    for (k, v) in fields {
        set(&mut j.fields, k, v);
    }
    j.revision += 1;
}
fn dict(fields: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Dict(
        fields
            .into_iter()
            .map(|(k, v)| (Value::Str(k), v))
            .collect(),
    )
}
fn dget<'a>(d: &'a Value, k: &str) -> Option<&'a Value> {
    match d {
        Value::Dict(d) => d.iter().find(|(key, _)| key.is_key(k)).map(|(_, v)| v),
        _ => None,
    }
}
fn dget_mut<'a>(d: &'a mut Value, k: &str) -> Option<&'a mut Value> {
    match d {
        Value::Dict(d) => d.iter_mut().find(|(key, _)| key.is_key(k)).map(|(_, v)| v),
        _ => None,
    }
}
fn dset(d: &mut Value, k: &str, v: Value) {
    if let Value::Dict(d) = d {
        if let Some((_, old)) = d.iter_mut().find(|(key, _)| key.is_key(k)) {
            *old = v;
        } else {
            d.push((Value::Str(k.to_owned()), v));
        }
    }
}
fn append_capture_stream(job: &mut Job, name: &'static str, stream: Value) {
    if let Some((_, Value::Dict(streams))) = job.fields.iter_mut().find(|(k, _)| *k == "streams") {
        streams.push((Value::constant(name), stream));
    }
}
fn log(actions: &mut Vec<Action>, name: &'static str, j: &Job) {
    if !j.record_receipts {
        return;
    }
    actions.push(Action::Log(Record::new(name, j.fields.clone())));
}

fn bank(m: &WordMap) -> Registers {
    let mut r = Registers::new();
    for (k, v) in m.items() {
        r.set(k, v as u64);
    }
    r
}
fn replace(m: &mut WordMap, r: &Registers) {
    *m = r.iter().map(|(k, v)| (k, v as u32)).collect();
}
fn merge(m: &mut WordMap, r: &Registers) {
    for (k, v) in r.iter() {
        m.insert(k, v as u32);
    }
}

/// The runtime supplies snapshot reads and configured IRQ banks.
/// Missing callbacks are explicit fixtures, checked when their effects occur.
pub trait Inputs {
    fn has_reader(&self) -> bool;
    fn has_irq(&self) -> bool;
    fn routed(&self) -> bool;
    fn read(&mut self, cpu: i64, address: u64, length: u64)
    -> std::result::Result<Vec<u8>, String>;
    fn submit_webgpu_request(&mut self, _kind: &str, _payload: Vec<u8>) -> Result<Option<u64>> {
        Ok(None)
    }
    fn targets(&mut self) -> Result<(usize, i64)>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebGpuUiRequest {
    pub cpu: i64,
    pub draws: Vec<batch::Draw>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebGpuUiResult {
    pub output: Vec<(u64, Vec<u8>)>,
    pub touched_bytes: u64,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum CompletionEvent {
    Fill,
    Copy,
    Clear,
    Batch,
    Gles,
}
impl CompletionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Fill => "Graphics fill completed",
            Self::Copy => "Graphics software-profile copy completed",
            Self::Clear => "Graphics clear-only completed",
            Self::Batch => "Graphics declared UI batch completed",
            Self::Gles => "DMP 3D command batch completed",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Completion {
    pub cpu: i64,
    pub irq_cpu: i64,
    pub status: u32,
    pub bytes: u64,
    #[serde(with = "dma_output_bytes")]
    pub output: DmaOutput,
    pub job: Job,
    pub event: CompletionEvent,
    pub ui: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandProcessor {
    #[serde(default)]
    pub gles: fujimod_emulator_ip_dmp::gles::Engine,
    #[serde(default)]
    pending_webgpu: Option<PendingWebGpu>,
    #[serde(default)]
    cpu_raster_calls: u64,
    #[serde(default)]
    cpu_vector_paint_calls: u64,
    #[serde(default)]
    gpu_gles_lists: u64,
    #[serde(default)]
    gpu_vector_paint_calls: u64,
    pub regs: WordMap,
    pub path: WordMap,
    pub fill: WordMap,
    pub written: BTreeSet<u32>,
    pub clear_ack_offsets: BTreeSet<u32>,
    pub command_pending: Option<(i64, Vec<u32>)>,
    pub busy: bool,
    pub jobs_started: i64,
    pub jobs_completed: i64,
    pub bytes_completed: i64,
    pub irqs_raised: i64,
    pub acknowledgments: i64,
    pub last_job: Option<Job>,
    pub ram_bytes: u64,
    pub max_fill_bytes: u64,
    pub max_copy_bytes: u64,
    pub max_command_capture_bytes: u64,
    pub max_clear_bytes: u64,
    pub bytes_per_tick: u64,
    pub copy_profile: Option<Profile>,
    pub clear_profile: Option<Profile>,
    pub completions: BTreeMap<u64, Completion>,
    pub next_token: u64,
    pub next_job: u64,
}
impl Default for CommandProcessor {
    fn default() -> Self {
        Self {
            gles: Default::default(),
            pending_webgpu: None,
            cpu_raster_calls: 0,
            cpu_vector_paint_calls: 0,
            gpu_gles_lists: 0,
            gpu_vector_paint_calls: 0,
            regs: WordMap::default(),
            path: WordMap::default(),
            fill: WordMap::default(),
            written: BTreeSet::new(),
            clear_ack_offsets: BTreeSet::new(),
            command_pending: None,
            busy: false,
            jobs_started: 0,
            jobs_completed: 0,
            bytes_completed: 0,
            irqs_raised: 0,
            acknowledgments: 0,
            last_job: None,
            ram_bytes: 0x08000000,
            max_fill_bytes: 0x04000000,
            max_copy_bytes: 0x04000000,
            max_command_capture_bytes: 0x10000,
            max_clear_bytes: 0x04000000,
            bytes_per_tick: 16,
            copy_profile: None,
            clear_profile: None,
            completions: BTreeMap::new(),
            next_token: 0,
            next_job: 0,
        }
    }
}
fn init_values(offset: u32) -> Option<&'static [u32]> {
    match offset {
        0x1800 | 0x1000 | 0x1020 => Some(&[0]),
        0x1880 | 0x10a0 => Some(&[0x12345678]),
        0x18c0 => Some(&[0xfffffff0, 0xffffffff]),
        0xc1c => Some(&[0x8000]),
        0x10c0 => Some(&[0xffffffff]),
        0x10c4 => Some(&[0xffffffff, 0xfffffff0]),
        0x1248 => Some(&[1]),
        _ => None,
    }
}
fn setup(offset: u32) -> Option<&'static [u32]> {
    match offset {
        0x1204 => Some(&[0, 0x10000]),
        0x12b4 => Some(&[0]),
        0x1234 => Some(&[0xffff]),
        0x128c => Some(&[0x10000]),
        _ => None,
    }
}
pub fn contains(address: u64, size: u32) -> bool {
    address >= BASE
        && address
            .checked_add(size as u64)
            .is_some_and(|e| e <= BASE + SIZE)
}
pub fn offset(address: u64, size: u32) -> Result<u32> {
    let physical = address.wrapping_sub(BASE);
    let logical = (physical / 2) as u32;
    if !contains(address, size)
        || size != 4
        || physical & 7 != 0
        || !(init_values(logical).is_some()
            || setup(logical).is_some()
            || COMMAND.contains(&logical)
            || COPY.contains(&logical)
            || TRIGGERS.contains(&logical)
            || [0x10, 0x14, 0x18, 0x1c, 0x34, 0xc18, 0x105c].contains(&logical))
    {
        return fail(format!(
            "Unknown graphics-accelerator register/access {address:#x}/{size}"
        ));
    }
    Ok(logical)
}
impl CommandProcessor {
    pub fn new_job(&mut self, fields: Fields) -> Job {
        self.new_job_with_receipts(true, || fields)
    }
    fn new_job_with_receipts(
        &mut self,
        record_receipts: bool,
        fields: impl FnOnce() -> Fields,
    ) -> Job {
        let id = self.next_job;
        self.next_job += 1;
        Job {
            id,
            revision: 0,
            fields: if record_receipts {
                fields()
                    .into_iter()
                    .map(|(k, v)| (Cow::Borrowed(k), v))
                    .collect()
            } else {
                Vec::new()
            },
            record_receipts,
        }
    }
    pub fn read(&self, address: u64, size: u32) -> Result<u32> {
        let o = offset(address, size)?;
        if o == 0x105c && self.clear_profile.is_none() {
            return fail(
                "Graphics command-list post-wait status is unresolved without clear profile",
            );
        }
        Ok(self.regs.get_or_zero(o))
    }
    pub fn needs_inputs(address: u64, value: u32) -> bool {
        offset(address, 4)
            .is_ok_and(|o| TRIGGERS.contains(&o) || ([0x1c, 0xc18].contains(&o) && value != 0))
    }
    fn status(&self) -> u32 {
        self.regs.get_or_zero(0x34)
    }
    fn ui(&self) -> bool {
        self.clear_profile == Some(Profile::GpuUiProfile)
    }
    fn delay(&self, bytes: u64) -> u64 {
        2.max(bytes.div_ceil(self.bytes_per_tick))
    }
    fn irq_cpu(&self, cpu: i64, kind: &str, host: &mut dyn Inputs) -> Result<i64> {
        if !host.routed() {
            return Ok(cpu);
        }
        let t = host.targets()?;
        if t.0 != 1 || ![0, 1].contains(&t.1) {
            return fail(format!(
                "Graphics {kind} IRQ has missing or ambiguous configured bank"
            ));
        }
        Ok(t.1)
    }
    fn store(&mut self, o: u32, v: u32) {
        self.regs.insert(o, v);
        self.written.insert(o);
    }
    pub fn write(
        &mut self,
        cpu: i64,
        address: u64,
        size: u32,
        value: u32,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        self.write_with_receipts(cpu, address, size, value, host, actions, true)
    }
    fn write_with_receipts(
        &mut self,
        cpu: i64,
        address: u64,
        size: u32,
        value: u32,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
        record_receipts: bool,
    ) -> Result<()> {
        let o = offset(address, size)?;
        if o == 0x34 || o == 0x105c {
            return fail("Graphics completion status is read-only");
        }
        if o == GLES_SUBMIT {
            return self.start_gles(cpu, value, host, actions, record_receipts);
        }
        if TRIGGERS.contains(&o) {
            self.store(o, value);
            let mut capture = self.capture(cpu, o, value, true, record_receipts, host, actions);
            if self.clear_profile.is_none() {
                return fail("Graphics command-list rasterization is not modeled");
            }
            if let Err(error) =
                self.start_commands(cpu, o, value, &mut capture, host, actions, record_receipts)
            {
                if record_receipts {
                    capture.hex_words();
                    update(&mut capture.job, vec![("rejection", text(&error.0))]);
                    log(
                        actions,
                        if self.ui() {
                            "Graphics UI command rejected"
                        } else {
                            "Graphics clear-only command rejected"
                        },
                        &capture.job,
                    );
                }
                if record_receipts {
                    self.last_job = Some(capture.job);
                }
                return Err(error);
            }
            if record_receipts {
                self.last_job = Some(capture.job);
            }
            return Ok(());
        }
        if o == 0xc18 {
            if value != 0 {
                if self.copy_profile.is_none() && !self.native_pixmap_descriptor() {
                    self.store(o, value);
                    return fail("Graphics image copy/untile is not modeled");
                }
                return self.start_copy(cpu, value, host, actions, record_receipts);
            }
            if self.busy {
                return fail("Graphics copy abort semantics are not modeled");
            }
            if self.status() & COPY_STATUS != 0 {
                self.regs.insert(0x34, self.status() & !COPY_STATUS);
                self.acknowledgments += 1;
                if record_receipts {
                    actions.push(Action::Log(Record::new(
                        "Graphics copy status acknowledged",
                        [("cpu", num(cpu))],
                    )));
                }
            }
        } else if o == 0x1c {
            if value != 0 {
                return self.start_fill(cpu, value, host, actions, record_receipts);
            }
            if self.busy {
                return fail("Graphics fill abort semantics are not modeled");
            }
            if self.status() & FILL_STATUS != 0 {
                self.regs.insert(0x34, self.status() & !FILL_STATUS);
                self.acknowledgments += 1;
                if record_receipts {
                    actions.push(Action::Log(Record::new(
                        "Graphics fill status acknowledged",
                        [("cpu", num(cpu))],
                    )));
                }
            }
        } else if let Some(values) = init_values(o) {
            if !values.contains(&value) {
                return fail(format!("Unknown graphics initialization {o:#x}={value:#x}"));
            }
            if [0x1800, 0x1020].contains(&o) && self.status() & CLEAR_STATUS != 0 {
                if self.busy {
                    return fail("Graphics clear abort semantics are not modeled");
                }
                self.clear_ack_offsets.insert(o);
                if self.clear_ack_offsets == BTreeSet::from([0x1800, 0x1020]) {
                    self.regs.insert(0x34, self.status() & !CLEAR_STATUS);
                    self.acknowledgments += 1;
                    if record_receipts {
                        actions.push(Action::Log(Record::new(
                            "Graphics clear bit31 acknowledged by both actual ISR stores",
                            [("cpu", num(cpu))],
                        )));
                    }
                }
            }
        } else if let Some(values) = setup(o) {
            if !values.contains(&value) {
                return fail(format!(
                    "Unknown graphics command-list setup {o:#x}={value:#x}"
                ));
            }
        } else if [0x10, 0x14, 0x18].contains(&o) && self.busy {
            return fail("Graphics fill descriptor changed while busy");
        } else if COPY.contains(&o) && self.busy {
            return fail("Graphics copy descriptor changed while busy");
        } else if COMMAND.contains(&o)
            && (self.busy || (self.command_pending.is_some() && [0x10d4, 0x10dc].contains(&o)))
        {
            return fail("Graphics command-list descriptor changed while pending/busy");
        }
        self.store(o, value);
        Ok(())
    }
    /// Plan a native submission without publishing a rejected trigger or
    /// capture. A refusal is retried on the event path, which publishes the
    /// operation record there. Descriptor and scalar accesses need no
    /// speculative model clone, lock, or scratch allocation.
    pub fn write_native(
        &mut self,
        cpu: i64,
        address: u64,
        value: u32,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        self.write_native_with_receipts(cpu, address, value, host, actions, true)
    }
    pub(crate) fn write_native_for_replay(
        &mut self,
        cpu: i64,
        address: u64,
        value: u32,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        self.write_native_with_receipts(cpu, address, value, host, actions, false)
    }
    fn write_native_with_receipts(
        &mut self,
        cpu: i64,
        address: u64,
        value: u32,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
        record_receipts: bool,
    ) -> Result<()> {
        let o = offset(address, 4)?;
        if o == GLES_SUBMIT {
            return self.start_gles(cpu, value, host, actions, record_receipts);
        }
        if TRIGGERS.contains(&o) {
            let mut capture = self.capture(cpu, o, value, false, record_receipts, host, actions);
            if self.clear_profile.is_none() {
                return fail("Graphics command-list rasterization is not modeled");
            }
            self.start_commands(cpu, o, value, &mut capture, host, actions, record_receipts)?;
            self.store(o, value);
            if record_receipts {
                self.last_job = Some(capture.job);
            }
            return Ok(());
        }
        if o == 0xc18 && value != 0 {
            if self.copy_profile.is_none() && !self.native_pixmap_descriptor() {
                return fail("Graphics image copy/untile is not modeled");
            }
            return self.start_copy(cpu, value, host, actions, record_receipts);
        }
        self.write_with_receipts(cpu, address, 4, value, host, actions, record_receipts)
    }

    fn schedule(&mut self, completion: Completion, delay: u64, actions: &mut Vec<Action>) {
        let token = self.next_token;
        self.next_token += 1;
        self.completions.insert(token, completion);
        actions.push(Action::Schedule { delay, token });
    }
    /// DMA must succeed before this method commits status and logs/raises IRQ.
    /// The runtime applies output() first, then complete(), preserving the
    /// memory-write-before-state order, including callback failures.
    pub fn output(&self, token: u64) -> Result<(i64, DmaOutput)> {
        let c = self
            .completions
            .get(&token)
            .ok_or_else(|| Unsupported("Unknown graphics completion token".into()))?;
        Ok((c.cpu, c.output.clone()))
    }
    pub fn complete_webgpu_request(
        &mut self,
        id: u64,
        result: &[u8],
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        let pending = self
            .pending_webgpu
            .as_ref()
            .cloned()
            .ok_or_else(|| Unsupported("Vector WebGPU result has no pending request".into()))?;
        match pending {
            PendingWebGpu::Gles {
                id: expected,
                cpu,
                irq_cpu,
                command_address,
                command_bytes,
                mut job,
            } => {
                if id != expected {
                    return fail("Vector WebGPU result does not match the pending GLES list");
                }
                let snapshots = self.gles.apply_webgpu_result(result).map_err(Unsupported)?;
                let output: DmaOutput = self
                    .gles
                    .written_ranges()
                    .iter()
                    .map(|&(address, length)| {
                        let (base, snapshot) = snapshots
                            .iter()
                            .find(|(base, bytes)| {
                                *base <= address
                                    && address + length as u64 <= *base + bytes.len() as u64
                            })
                            .ok_or_else(|| Unsupported("DMP write range outside target snapshot".into()))?;
                        let offset = (address - base) as usize;
                        Ok((address, Arc::new(snapshot[offset..offset + length].to_vec())))
                    })
                    .collect::<Result<_>>()?;
                let bytes = output.iter().map(|(_, data)| data.len() as u64).sum::<u64>();
                if job.record_receipts {
                    update(&mut job, vec![("bytes", num(bytes))]);
                }
                self.pending_webgpu = None;
                self.last_job = if job.record_receipts { Some(job.clone()) } else { None };
                self.schedule(
                    Completion {
                        cpu,
                        irq_cpu,
                        status: CLEAR_STATUS,
                        bytes,
                        output,
                        job,
                        event: CompletionEvent::Gles,
                        ui: false,
                    },
                    self.delay(command_bytes + bytes),
                    actions,
                );
                let _ = command_address;
            }
            PendingWebGpu::Ui {
                id: expected,
                cpu,
                irq_cpu,
                draw_count,
                filled_paths,
                stroked_paths,
                sampled_images,
                linear_gradients,
                radial_gradients,
                mut job,
            } => {
                if id != expected {
                    return fail("Vector WebGPU result does not match the pending UI batch");
                }
                let rendered: WebGpuUiResult = ciborium::de::from_reader(std::io::Cursor::new(result))
                    .map_err(|error| Unsupported(format!("Decode vector WebGPU result: {error}")))?;
                let output: DmaOutput = rendered
                    .output
                    .into_iter()
                    .map(|(address, bytes)| (address, Arc::new(bytes)))
                    .collect();
                let bytes = output.iter().map(|(_, data)| data.len() as u64).sum::<u64>();
                if bytes > self.max_clear_bytes || rendered.touched_bytes > self.max_clear_bytes {
                    return fail("Vector WebGPU output exceeds configured bounds");
                }
                let delay = self.delay(rendered.touched_bytes);
                if job.record_receipts {
                    update(
                        &mut job,
                        vec![
                            ("operation", Value::constant("ui-primitives")),
                            ("profile", self.clear_profile.unwrap().value()),
                            ("accepted", Value::Bool(true)),
                            ("draw_count", num(draw_count as u64)),
                            ("filled_paths", num(filled_paths as u64)),
                            ("stroked_paths", num(stroked_paths as u64)),
                            ("sampled_images", num(sampled_images as u64)),
                            ("linear_gradients", num(linear_gradients as u64)),
                            (
                                "destinations",
                                Value::List(
                                    output
                                        .iter()
                                        .map(|(address, data)| {
                                            dict([
                                                ("address".into(), hex(*address)),
                                                ("bytes".into(), num(data.len() as u64)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                            ("changed_bytes", num(bytes)),
                            ("covered_bytes_in_draw_order", num(rendered.touched_bytes)),
                            ("irq_cpu", num(irq_cpu)),
                            ("latency_ticks", num(delay)),
                            (
                                "pixel_semantics",
                                Value::constant(
                                    "declared software RGBA8 SRC/SRC_OVER, affine paths, regular-grid AA, centerline strokes, nearest Morton images, linear ramp interpolation; hardware pixels unmeasured",
                                ),
                            ),
                            (
                                "completion_semantics",
                                Value::constant(
                                    "atomic validated whole batch before bit31/physicalIRQ3c; throughput timing unmeasured",
                                ),
                            ),
                        ],
                    );
                    if radial_gradients != 0 {
                        update(
                            &mut job,
                            vec![
                                ("radial_gradients", num(radial_gradients as u64)),
                                (
                                    "pixel_semantics",
                                    Value::constant(
                                        "declared software RGBA8 SRC/SRC_OVER, affine paths, regular-grid AA, centerline strokes, nearest Morton images, linear/radial ramp interpolation; hardware pixels unmeasured",
                                    ),
                                ),
                            ],
                        );
                    }
                    log(actions, "Graphics declared UI batch started", &job);
                }
                self.pending_webgpu = None;
                self.last_job = if job.record_receipts { Some(job.clone()) } else { None };
                self.schedule(
                    Completion {
                        cpu,
                        irq_cpu,
                        status: CLEAR_STATUS,
                        bytes,
                        output,
                        job,
                        event: CompletionEvent::Batch,
                        ui: true,
                    },
                    delay,
                    actions,
                );
            }
        }
        Ok(())
    }
    pub fn complete(&mut self, token: u64, _now: u64, actions: &mut Vec<Action>) -> Result<Job> {
        let mut c = self
            .completions
            .remove(&token)
            .ok_or_else(|| Unsupported("Unknown graphics completion token".into()))?;
        self.busy = false;
        self.jobs_completed += 1;
        self.bytes_completed += c.bytes as i64;
        if c.job.record_receipts {
            update(&mut c.job, vec![("completed", Value::Bool(true))]);
        }
        self.regs.insert(0x34, self.status() | c.status);
        if c.job.record_receipts && self.last_job.as_ref().is_some_and(|job| job.id == c.job.id) {
            self.last_job = Some(c.job.clone());
        }
        log(actions, c.event.name(), &c.job);
        self.irqs_raised += 1;
        actions.push(Action::QueueIrq(c.irq_cpu as u8, IRQ));
        Ok(c.job)
    }
    /// Commit replay state after the recorder's independent output digest check.
    /// Replay discards diagnostic records, so it does not build another receipt.
    pub(crate) fn complete_for_replay(&mut self, token: u64, _now: u64) -> Result<()> {
        let c = self
            .completions
            .remove(&token)
            .ok_or_else(|| Unsupported("Unknown graphics completion token".into()))?;
        self.busy = false;
        self.jobs_completed += 1;
        self.bytes_completed += c.bytes as i64;
        self.regs.insert(0x34, self.status() | c.status);
        self.irqs_raised += 1;
        Ok(())
    }
    fn start_gles(
        &mut self,
        cpu: i64,
        trigger: u32,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
        receipts: bool,
    ) -> Result<()> {
        if trigger != 1 || self.busy || self.command_pending.is_some() || self.status() != 0 {
            return fail("DMP 3D submission requires idle engine and acknowledged completion");
        }
        if ![0x20e0, 0x20e8].iter().all(|r| self.written.contains(r))
            || !host.has_reader()
            || !host.has_irq()
        {
            return fail("DMP 3D submission needs command descriptor, DMA and IRQ routing");
        }
        let length = u64::from(self.regs.get_or_zero(0x20e0)) * 8;
        let address = (u64::from(self.regs.get_or_zero(0x20e8)) * 8) & !0x40000000;
        if length == 0 || length > 1024 * 1024 || address + length > self.ram_bytes {
            return fail("DMP 3D command range exceeds bound");
        }
        let irq_cpu = self.irq_cpu(cpu, "3D", host)?;
        let bytes = host.read(cpu, address, length).map_err(Unsupported)?;
        if bytes.len() as u64 != length {
            return fail("DMP short 3D command DMA");
        }
        let words: Vec<_> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        if webgpu_bridge_enabled() {
            let webgpu_request = self
                .gles
                .encode_webgpu_request(&words)
                .map_err(Unsupported)?;
            if let Some(id) =
                host.submit_webgpu_request("vector-gles-webgpu", webgpu_request)?
            {
            let job = self.new_job_with_receipts(receipts, || {
                vec![
                    ("operation", Value::constant("3D")),
                    ("command_address", hex(address)),
                    ("command_bytes", num(length)),
                    ("bytes", num(0)),
                    ("completed", Value::Bool(false)),
                ]
            });
            self.last_job = if receipts { Some(job.clone()) } else { None };
            self.busy = true;
            self.jobs_started += 1;
            self.clear_ack_offsets.clear();
            self.store(GLES_SUBMIT, trigger);
            self.gpu_gles_lists += 1;
            self.pending_webgpu = Some(PendingWebGpu::Gles {
                id,
                cpu,
                irq_cpu,
                command_address: address,
                command_bytes: length,
                job,
            });
            return Ok(());
            }
        }
        self.cpu_raster_calls += 1;
        let ram_bytes = self.ram_bytes;
        let output = self
            .gles
            .execute(&words, &mut |address, length| {
                let address = address & !0x40000000;
                if address
                    .checked_add(length as u64)
                    .is_none_or(|end| end > ram_bytes)
                {
                    return Err(format!("DMP 3D DMA outside RAM {address:#x}+{length:#x}"));
                }
                host.read(cpu, address, length as u64)
            })
            .map_err(Unsupported)?;
        // Raster snapshots include untouched destination pixels. Commit only
        // actual color/depth writes so concurrent accesses elsewhere survive.
        let output: Vec<(u64, Vec<u8>)> = self
            .gles
            .written_ranges()
            .iter()
            .map(|&(address, length)| {
                let (base, snapshot) = output
                    .iter()
                    .find(|(base, bytes)| {
                        *base <= address && address + length as u64 <= *base + bytes.len() as u64
                    })
                    .ok_or_else(|| Unsupported("DMP write range outside target snapshot".into()))?;
                let offset = (address - base) as usize;
                Ok((address, snapshot[offset..offset + length].to_vec()))
            })
            .collect::<Result<_>>()?;
        let count = output.iter().map(|(_, b)| b.len() as u64).sum::<u64>();
        let job = self.new_job_with_receipts(receipts, || {
            vec![
                ("operation", Value::constant("3D")),
                ("command_address", hex(address)),
                ("command_bytes", num(length)),
                ("bytes", num(count)),
                ("completed", Value::Bool(false)),
            ]
        });
        self.last_job = if receipts { Some(job.clone()) } else { None };
        self.busy = true;
        self.jobs_started += 1;
        self.clear_ack_offsets.clear();
        self.store(GLES_SUBMIT, trigger);
        self.schedule(
            Completion {
                cpu,
                irq_cpu,
                status: CLEAR_STATUS,
                bytes: count,
                output: output.into_iter().map(|(a, b)| (a, Arc::new(b))).collect(),
                job,
                event: CompletionEvent::Gles,
                ui: false,
            },
            self.delay(length + count),
            actions,
        );
        Ok(())
    }
    fn start_fill(
        &mut self,
        cpu: i64,
        control: u32,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
        record_receipts: bool,
    ) -> Result<()> {
        let format =
            fujimod_emulator_ip_dmp::fill::Control::decode(control).map_err(Unsupported)?;
        if self.busy
            || self.command_pending.is_some()
            || self.status() & (FILL_STATUS | COPY_STATUS | CLEAR_STATUS) != 0
        {
            return fail("Graphics fill requires idle and acknowledged previous completion");
        }
        if ![0x10, 0x14, 0x18].iter().all(|o| self.written.contains(o)) {
            return fail("Graphics fill descriptor is incomplete");
        }
        if !host.has_irq() {
            return fail("Graphics fill completion IRQ routing is not configured");
        }
        let irq_cpu = self.irq_cpu(cpu, "fill", host)?;
        let (start, end, word) = (
            self.regs.get_or_zero(0x10),
            self.regs.get_or_zero(0x14),
            self.regs.get_or_zero(0x18),
        );
        let alias = start & 0xc0000000;
        if ![0, 0x40000000].contains(&alias) || end & 0xc0000000 != alias {
            return fail("Graphics fill uses an unresolved RAM address/alias");
        }
        let destination = (start - alias) as u64;
        let endpoint = (end - alias) as u64;
        if start & 0x7f != 0
            || endpoint <= destination
            || (endpoint - destination) & 0xf != 0
            || endpoint - destination > self.max_fill_bytes
            || endpoint > self.ram_bytes
        {
            return fail("Graphics fill range/alignment exceeds evidenced bounds");
        }
        let count = endpoint - destination;
        let pattern = word.to_le_bytes();
        let mut payload = if format.preserves_destination() {
            if !host.has_reader() {
                return fail("Graphics masked fill requires destination memory");
            }
            host.read(cpu, destination, count).map_err(Unsupported)?
        } else {
            vec![0; count as usize]
        };
        if payload.len() as u64 != count {
            return fail("Graphics fill destination read is incomplete");
        }
        format.apply(&mut payload, word).map_err(Unsupported)?;
        let delay = self.delay(count);
        self.busy = true;
        self.store(0x1c, control);
        self.jobs_started += 1;
        let j = self.new_job_with_receipts(record_receipts, || {
            vec![
                ("cpu", num(cpu)),
                ("start_register", hex(start)),
                ("end_exclusive_register", hex(end)),
                ("destination", hex(destination)),
                ("bytes", num(count)),
                ("pattern_word", hex(word)),
                (
                    "pattern_little_endian",
                    text(
                        &pattern
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>(),
                    ),
                ),
                ("control", hex(control)),
                ("irq_cpu", num(irq_cpu)),
                (
                    "irq_routing",
                    Value::constant(if host.routed() {
                        "configured bank"
                    } else {
                        "submitting CPU fixture"
                    }),
                ),
                ("latency_ticks", num(delay)),
                ("completed", Value::Bool(false)),
            ]
        });
        if record_receipts {
            self.last_job = Some(j.clone());
        }
        log(actions, "Graphics fill started", &j);
        // The complete validated output becomes visible at the DMA deadline.
        self.schedule(
            Completion {
                cpu,
                irq_cpu,
                status: FILL_STATUS,
                bytes: count,
                output: vec![(destination, Arc::new(payload))],
                job: j,
                event: CompletionEvent::Fill,
                ui: false,
            },
            delay,
            actions,
        );
        Ok(())
    }
    fn native_pixmap_descriptor(&self) -> bool {
        self.regs.get_or_zero(0xc10) == 8
            || (self.regs.get_or_zero(0xc10) == 1
                && self.regs.get_or_zero(0xc0c) > 0
                && self.regs.get_or_zero(0xc0c) >> 16 == 0)
    }
    fn start_copy(
        &mut self,
        cpu: i64,
        trigger: u32,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
        record_receipts: bool,
    ) -> Result<()> {
        if trigger != 1 {
            return fail(format!("Unknown graphics copy trigger {trigger:#x}"));
        }
        if self.busy
            || self.command_pending.is_some()
            || self.status() & (FILL_STATUS | COPY_STATUS | CLEAR_STATUS) != 0
        {
            return fail("Graphics copy requires idle and acknowledged previous completion");
        }
        if ![0xc00, 0xc04, 0xc08, 0xc0c, 0xc10]
            .iter()
            .all(|o| self.written.contains(o))
        {
            return fail("Graphics copy descriptor is incomplete");
        }
        let control = self.regs.get_or_zero(0xc10);
        if control == 8 {
            return self.start_tile_copy(cpu, trigger, host, actions, record_receipts);
        }
        if control != 1 {
            return fail(format!(
                "Unknown graphics software-profile copy control {control:#x}"
            ));
        }
        let dims = self.regs.get_or_zero(0xc08);
        let (w, h) = (dims & 0xffff, dims >> 16);
        let destination_shape = self.regs.get_or_zero(0xc0c);
        let output_pitch = if destination_shape == dims {
            w as u64 * 4
        } else if destination_shape >> 16 == 0
            && destination_shape >= w * 4
            && destination_shape & 3 == 0
        {
            u64::from(destination_shape)
        } else {
            return fail("DMP unsupported native pixmap pitch");
        };
        if w == 0 || h == 0 || w & 7 != 0 || h & 7 != 0 {
            return fail(
                "Graphics software-profile copy requires identical positive8-aligned dimensions",
            );
        }
        let count = w as u64 * h as u64 * 4;
        let destination_count = output_pitch * u64::from(h);
        if count > self.max_copy_bytes || destination_count > self.max_copy_bytes {
            return fail("Graphics software-profile copy exceeds byte bound");
        }
        let (sreg, dreg) = (self.regs.get_or_zero(0xc00), self.regs.get_or_zero(0xc04));
        let mut addresses = Vec::new();
        for (a, length) in [(sreg, count), (dreg, destination_count)] {
            let alias = a & 0xc0000000;
            if ![0, 0x40000000].contains(&alias) {
                return fail("Graphics software-profile copy has unresolved RAM alias");
            }
            let p = (a - alias) as u64;
            if p & 3 != 0 || p + length > self.ram_bytes {
                return fail("Graphics software-profile copy range/alignment exceeds bounds");
            }
            addresses.push(p);
        }
        let (source, destination) = (addresses[0], addresses[1]);
        if source < destination + destination_count && destination < source + count {
            return fail("Graphics software-profile overlapping copy is unproved");
        }
        if !host.has_reader() || !host.has_irq() {
            return fail("Graphics software-profile copy requires memory and IRQ routing");
        }
        let irq_cpu = self.irq_cpu(cpu, "copy", host)?;
        let input = host.read(cpu, source, count).map_err(Unsupported)?;
        if input.len() as u64 != count {
            return fail("Graphics software-profile copy source read is incomplete");
        }
        let linear = render::copy_untile(&input, w as usize, h as usize);
        let payload = if output_pitch == u64::from(w) * 4 {
            linear
        } else {
            let mut bytes = host
                .read(cpu, destination, destination_count)
                .map_err(Unsupported)?;
            if bytes.len() as u64 != destination_count {
                return fail("DMP short native pixmap DMA");
            }
            for row in 0..h as usize {
                bytes[row * output_pitch as usize..row * output_pitch as usize + w as usize * 4]
                    .copy_from_slice(&linear[row * w as usize * 4..(row + 1) * w as usize * 4]);
            }
            bytes
        };
        let delay = self.delay(destination_count);
        self.busy = true;
        self.store(0xc18, trigger);
        self.jobs_started += 1;
        let profile = self
            .copy_profile
            .map_or_else(|| Value::constant("native-pixmap"), Profile::value);
        let j = self.new_job_with_receipts(record_receipts, || vec![
            ("operation", Value::constant("copy")),
            ("profile", profile),
            ("cpu", num(cpu)),
            ("source_register", hex(sreg)),
            ("destination_register", hex(dreg)),
            ("source", hex(source)),
            ("destination", hex(destination)),
            ("width", num(w)),
            ("height", num(h)),
            ("bytes", num(count)),
            ("control", hex(control)),
            ("trigger", num(trigger)),
            ("irq_cpu", num(irq_cpu)),
            (
                "irq_routing",
                Value::constant(if host.routed() {
                    "configured bank"
                } else {
                    "submitting CPU fixture"
                }),
            ),
            ("latency_ticks", num(delay)),
            ("completed", Value::Bool(false)),
            (
                "pixel_semantics",
                Value::constant("Morton 8x8 untile, bottom-up rows, bytes unchanged; direct hardware tile interpretation is unverified"),
            ),
        ]);
        if record_receipts {
            self.last_job = Some(j.clone());
        }
        if record_receipts {
            log(actions, "Graphics software-profile copy started", &j);
        }
        self.schedule(
            Completion {
                cpu,
                irq_cpu,
                status: COPY_STATUS,
                bytes: count,
                output: vec![(destination, Arc::new(payload))],
                job: j,
                event: CompletionEvent::Copy,
                ui: false,
            },
            delay,
            actions,
        );
        Ok(())
    }
    /// Copy byte chunks with independently pitched source and destination rows.
    fn start_tile_copy(
        &mut self,
        cpu: i64,
        trigger: u32,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
        record_receipts: bool,
    ) -> Result<()> {
        if ![0xc20, 0xc24, 0xc28]
            .iter()
            .all(|r| self.written.contains(r))
        {
            return fail("DMP tile copy descriptor is incomplete");
        }
        let count = u64::from(self.regs.get_or_zero(0xc20));
        if count == 0 || count > self.max_copy_bytes || count & 15 != 0 {
            return fail("DMP tile copy byte count exceeds bounds");
        }
        let descriptors = [self.regs.get_or_zero(0xc24), self.regs.get_or_zero(0xc28)];
        let mut bases = [0; 2];
        let mut chunks = [0; 2];
        let mut strides = [0; 2];
        let mut lengths = [0; 2];
        for i in 0..2 {
            let address = self.regs.get_or_zero(if i == 0 { 0xc00 } else { 0xc04 });
            if address & 0x8000000f != 0 {
                return fail("DMP tile copy address alias/alignment is unsupported");
            }
            bases[i] = u64::from(address & !0x40000000);
            chunks[i] = u64::from(descriptors[i] & 65535) * 16;
            strides[i] = chunks[i] + u64::from(descriptors[i] >> 16) * 16;
            if chunks[i] == 0 || count % chunks[i] != 0 {
                return fail("DMP tile copy has a partial chunk");
            }
            lengths[i] = (count / chunks[i] - 1) * strides[i] + chunks[i];
            if lengths[i] > self.max_copy_bytes || bases[i] + lengths[i] > self.ram_bytes {
                return fail("DMP tile copy DMA exceeds bounds");
            }
        }
        if bases[0] < bases[1] + lengths[1] && bases[1] < bases[0] + lengths[0] {
            return fail("DMP overlapping tile copy is unsupported");
        }
        let irq_cpu = self.irq_cpu(cpu, "copy", host)?;
        let source = host.read(cpu, bases[0], lengths[0]).map_err(Unsupported)?;
        let mut destination = host.read(cpu, bases[1], lengths[1]).map_err(Unsupported)?;
        if source.len() as u64 != lengths[0] || destination.len() as u64 != lengths[1] {
            return fail("DMP short tile copy DMA");
        }
        for n in 0..count {
            let src = n / chunks[0] * strides[0] + n % chunks[0];
            let dst = n / chunks[1] * strides[1] + n % chunks[1];
            destination[dst as usize] = source[src as usize];
        }
        let delay = self.delay(count);
        let job = self.new_job_with_receipts(record_receipts, || {
            vec![
                ("operation", Value::constant("tile-copy")),
                ("bytes", num(count)),
            ]
        });
        self.busy = true;
        self.jobs_started += 1;
        self.store(0xc18, trigger);
        self.schedule(
            Completion {
                cpu,
                irq_cpu,
                status: COPY_STATUS,
                bytes: count,
                output: vec![(bases[1], Arc::new(destination))],
                job,
                event: CompletionEvent::Copy,
                ui: false,
            },
            delay,
            actions,
        );
        Ok(())
    }
    fn capture(
        &mut self,
        cpu: i64,
        o: u32,
        trigger: u32,
        publish: bool,
        record_receipts: bool,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
    ) -> Capture {
        let capture_bound = self.max_command_capture_bytes;
        let mut j = self.new_job_with_receipts(record_receipts, || {
            vec![
                ("operation", Value::constant("command-list")),
                ("cpu", num(cpu)),
                (
                    "trigger_logical_offset",
                    Value::constant(match o {
                        0x10d0 => "0x10d0",
                        0x10e0 => "0x10e0",
                        _ => "0x20f0",
                    }),
                ),
                (
                    "trigger",
                    match (o, trigger) {
                        (0x10d0, 0x10001) => Value::constant("0x10001"),
                        (0x10e0 | 0x20f0, 1) => Value::constant("0x1"),
                        _ => hex(trigger),
                    },
                ),
                ("completed", Value::Bool(false)),
                ("accepted", Value::Bool(false)),
                ("streams", dict([])),
                ("capture_bound_bytes_each", num(capture_bound)),
            ]
        });
        let mut words = HashMap::new();
        let mut invalid = BTreeSet::new();
        for (name, a, c) in [("path", 0x10d4, 0x10dc), ("fill", 0x10e4, 0x10ec)] {
            let mut s = if record_receipts {
                dict([
                    (
                        "address_register_logical_offset".into(),
                        Value::constant(match a {
                            0x10d4 => "0x10d4",
                            _ => "0x10e4",
                        }),
                    ),
                    (
                        "count_register_logical_offset".into(),
                        Value::constant(match c {
                            0x10dc => "0x10dc",
                            _ => "0x10ec",
                        }),
                    ),
                ])
            } else {
                Value::None
            };
            if !self.written.contains(&a) || !self.written.contains(&c) {
                invalid.insert(name);
                dset(
                    &mut s,
                    "capture_error",
                    Value::constant("descriptor not yet written"),
                );
                if record_receipts {
                    append_capture_stream(&mut j, name, s);
                }
                continue;
            }
            let address = self.regs.get_or_zero(a);
            let count = self.regs.get_or_zero(c);
            let declared = count as u64 * 8;
            dset(&mut s, "address_register", hex(address));
            dset(&mut s, "count_register", hex(count));
            dset(&mut s, "declared_bytes", num(declared));
            let alias = address & 0xc0000000;
            let physical = (address - alias) as u64;
            if ![0, 0x40000000].contains(&alias)
                || address & 3 != 0
                || physical + declared > self.ram_bytes
            {
                invalid.insert(name);
                dset(
                    &mut s,
                    "capture_error",
                    Value::constant("unresolved RAM range/alias/alignment"),
                );
                if record_receipts {
                    append_capture_stream(&mut j, name, s);
                }
                continue;
            }
            if !host.has_reader() {
                invalid.insert(name);
                dset(
                    &mut s,
                    "capture_error",
                    Value::constant("no memory reader configured"),
                );
                if record_receipts {
                    append_capture_stream(&mut j, name, s);
                }
                continue;
            }
            let length = declared.min(self.max_command_capture_bytes);
            let payload = match host.read(cpu, physical, length) {
                Ok(p) => p,
                Err(e) => {
                    invalid.insert(name);
                    if record_receipts {
                        dset(&mut s, "capture_error", text(&e));
                    }
                    if record_receipts {
                        append_capture_stream(&mut j, name, s);
                    }
                    continue;
                }
            };
            let ints: Vec<u32> = payload
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect();
            dset(&mut s, "physical_address", hex(physical));
            dset(&mut s, "captured_bytes", num(payload.len() as u64));
            dset(
                &mut s,
                "truncated",
                Value::Bool(payload.len() as u64 != declared),
            );
            words.insert(name, ints);
            if payload.len() as u64 != declared || payload.len() as u64 != length {
                invalid.insert(name);
            }
            if payload.len() as u64 != length {
                dset(
                    &mut s,
                    "capture_error",
                    Value::constant("short memory read"),
                );
            }
            if record_receipts {
                append_capture_stream(&mut j, name, s);
            }
        }
        if publish && record_receipts {
            self.last_job = Some(j.clone());
        }
        if record_receipts {
            log(
                actions,
                "Graphics command-list payload captured before validation",
                &j,
            );
        }
        Capture {
            job: j,
            words,
            invalid,
        }
    }
    fn start_commands(
        &mut self,
        cpu: i64,
        o: u32,
        trigger: u32,
        c: &mut Capture,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
        record_receipts: bool,
    ) -> Result<()> {
        if self.busy || self.status() & (FILL_STATUS | COPY_STATUS | CLEAR_STATUS) != 0 {
            return fail("Graphics clear requires idle and acknowledged previous completion");
        }
        if !host.has_reader() || !host.has_irq() {
            return fail("Graphics clear-only requires memory and IRQ routing");
        }
        let p = bank(&self.path);
        let f = bank(&self.fill);
        if o == 0x10d0 {
            if trigger != 0x10001 || self.command_pending.is_some() {
                return fail("Graphics clear-only path trigger/order is unsupported");
            }
            let words = c.words("path")?;
            if self.ui() {
                stream::parse(&words, true, Some(&p))?;
            } else {
                clear::path_descriptor(&words, self.ram_bytes, self.max_clear_bytes)?;
            }
            self.command_pending = Some((cpu, words));
            c.compact();
            if record_receipts {
                update(
                    &mut c.job,
                    vec![
                        (
                            "stage",
                            Value::constant("path validated; awaiting separate fill stream"),
                        ),
                        ("accepted", Value::Bool(true)),
                    ],
                );
            }
            return Ok(());
        }
        if o != 0x10e0 || trigger != 1 || self.command_pending.is_none() {
            return fail("Graphics clear-only fill trigger/order is unsupported");
        }
        if self.command_pending.as_ref().unwrap().0 != cpu {
            return fail("Graphics clear-only cross-CPU stream submission is unproved");
        }
        let pw = c.words("path")?;
        let fw = c.words("fill")?;
        if pw != self.command_pending.as_ref().unwrap().1 {
            return fail("Graphics clear-only path memory changed before fill submission");
        }
        let mut parsed = match clear::parse_clear(
            &pw,
            &fw,
            self.ram_bytes,
            self.max_clear_bytes,
            Some(&p),
            Some(&f),
        ) {
            Ok(value) => Some(value),
            Err(error) if !self.ui() => return Err(error),
            Err(_) => None,
        };
        let mut draws = None;
        if parsed.is_none() {
            let (path_draws, path_bank) = stream::parse(&pw, true, Some(&p))?;
            let (fill_draws, fill_bank) = stream::parse(&fw, false, Some(&f))?;
            if path_draws.is_empty() && fill_draws.is_empty() {
                parsed = Some(init::initialize(
                    &pw,
                    &fw,
                    Some(&p),
                    Some(&f),
                    self.max_clear_bytes,
                )?);
            } else {
                draws = Some(batch::batch_parsed(
                    path_draws,
                    fill_draws,
                    path_bank,
                    fill_bank,
                    self.ram_bytes,
                    self.max_clear_bytes,
                )?);
            }
        }
        let irq_cpu = self.irq_cpu(cpu, "clear", host)?;
        if let Some((draws, banks)) = draws {
            return self.start_ui(
                (cpu, irq_cpu),
                c,
                draws.as_slice(),
                banks,
                host,
                actions,
                record_receipts,
            );
        }
        let (descriptor, (pb, fb)) = parsed.unwrap();
        let initialization =
            matches!(clear::get(&descriptor,"operation"),Some(Value::Str(s)) if s=="initialize");
        let (output, changed) = if initialization {
            (Vec::new(), 0)
        } else {
            let address = clear::int(&descriptor, "destination") as u64;
            let length = clear::int(&descriptor, "frame_bytes") as u64;
            let source = host.read(cpu, address, length).map_err(Unsupported)?;
            if source.len() as u64 != length {
                return fail("Graphics clear-only destination read is incomplete");
            }
            let r = match clear::get(&descriptor, "rectangle").unwrap() {
                Value::List(v) | Value::Tuple(v) => v
                    .iter()
                    .map(|v| if let Value::Int(n) = v { *n as i64 } else { 0 })
                    .collect::<Vec<_>>(),
                _ => unreachable!(),
            };
            let pixel = if let Some(Value::Bytes(b)) = clear::get(&descriptor, "pixel_bytes") {
                b
            } else {
                unreachable!()
            };
            let payload = clear::render_clear(
                &source,
                clear::int(&descriptor, "width") as u64,
                r.clone().try_into().unwrap(),
                pixel,
            );
            (
                vec![(address, Arc::new(payload))],
                ((r[2] - r[0]) * (r[3] - r[1]) * 4) as u64,
            )
        };
        let delay = self.delay(changed);
        self.command_pending = None;
        merge(&mut self.path, &pb);
        merge(&mut self.fill, &fb);
        self.clear_ack_offsets.clear();
        self.busy = true;
        self.jobs_started += 1;
        if self.ui() {
            c.compact();
        }
        if record_receipts {
            update(
                &mut c.job,
                vec![
                    (
                        "operation",
                        clear::get(&descriptor, "operation").unwrap().clone(),
                    ),
                    ("profile", self.clear_profile.unwrap().value()),
                    ("accepted", Value::Bool(true)),
                    (
                        "descriptor",
                        dict(
                            descriptor
                                .iter()
                                .filter(|(k, _)| *k != "pixel_bytes")
                                .map(|(k, v)| (k.to_string(), v.clone())),
                        ),
                    ),
                    ("changed_bytes", num(changed)),
                    ("irq_cpu", num(irq_cpu)),
                    ("latency_ticks", num(delay)),
                    (
                        "pixel_semantics",
                        Value::constant(if initialization {
                            "No pixel writes: state-only initialization without geometry"
                        } else {
                            "GPU profile uses a declared software layout; hardware draw behavior is unverified"
                        }),
                    ),
                    (
                        "completion_semantics",
                        Value::constant(
                            "cross-stream fence/draw and bit31 completion are explicit software-derived assumptions",
                        ),
                    ),
                ],
            );
        }
        if record_receipts {
            log(actions, "Graphics clear-only started", &c.job);
        }
        self.schedule(
            Completion {
                cpu,
                irq_cpu,
                status: CLEAR_STATUS,
                bytes: changed,
                output,
                job: c.job.clone(),
                event: CompletionEvent::Clear,
                ui: false,
            },
            delay,
            actions,
        );
        Ok(())
    }
    fn start_ui(
        &mut self,
        cpus: (i64, i64),
        c: &mut Capture,
        draws: &[batch::Draw],
        banks: batch::Banks,
        host: &mut dyn Inputs,
        actions: &mut Vec<Action>,
        record_receipts: bool,
    ) -> Result<()> {
        let (cpu, irq_cpu) = cpus;
        if webgpu_bridge_enabled() {
            let mut payload = Vec::new();
            ciborium::ser::into_writer(
                &WebGpuUiRequest {
                    cpu,
                    draws: draws.to_vec(),
                },
                &mut payload,
            )
            .map_err(|error| Unsupported(format!("Serialize vector WebGPU request: {error}")))?;
            if let Some(id) = host.submit_webgpu_request("vector-ui-webgpu", payload)? {
                let radial_gradients = draws
                    .iter()
                    .filter(|draw| draw.gradient.as_ref().is_some_and(|gradient| gradient.radial.is_some()))
                    .count();
                self.command_pending = None;
                replace(&mut self.path, &banks.path);
                replace(&mut self.fill, &banks.fill);
                self.clear_ack_offsets.clear();
                self.busy = true;
                self.jobs_started += 1;
                c.compact();
                self.last_job = if record_receipts { Some(c.job.clone()) } else { None };
                self.gpu_vector_paint_calls += 1;
                self.pending_webgpu = Some(PendingWebGpu::Ui {
                    id,
                    cpu,
                    irq_cpu,
                    draw_count: draws.len(),
                    filled_paths: draws
                        .iter()
                        .filter(|draw| !draw.stroke && draw.texture.is_none())
                        .count(),
                    stroked_paths: draws.iter().filter(|draw| draw.stroke).count(),
                    sampled_images: draws
                        .iter()
                        .filter(|draw| draw.texture.is_some())
                        .count(),
                    linear_gradients: draws
                        .iter()
                        .filter(|draw| draw.gradient.as_ref().is_some_and(|gradient| gradient.radial.is_none()))
                        .count(),
                    radial_gradients,
                    job: c.job.clone(),
                });
                return Ok(());
            }
        }
        self.cpu_vector_paint_calls += 1;
        let (output, touched) = ui_render(draws, cpu, host)?;
        let bytes = output.iter().map(|(_, p)| p.len() as u64).sum();
        // Whole-surface snapshots preserve untouched pixels for the software
        // renderer. They are host storage, not GPU work: charge the covered
        // bytes in draw order, including overlapping draws, as clear does.
        let delay = self.delay(touched);
        self.command_pending = None;
        replace(&mut self.path, &banks.path);
        replace(&mut self.fill, &banks.fill);
        self.clear_ack_offsets.clear();
        self.busy = true;
        self.jobs_started += 1;
        c.compact();
        if record_receipts {
            update(
                &mut c.job,
                vec![
                    ("operation", Value::constant("ui-primitives")),
                    ("profile", self.clear_profile.unwrap().value()),
                    ("accepted", Value::Bool(true)),
                    ("draw_count", num(draws.len() as u64)),
                    (
                        "filled_paths",
                        num(draws
                            .iter()
                            .filter(|d| !d.stroke && d.texture.is_none())
                            .count() as u64),
                    ),
                    (
                        "stroked_paths",
                        num(draws.iter().filter(|d| d.stroke).count() as u64),
                    ),
                    (
                        "sampled_images",
                        num(draws.iter().filter(|d| d.texture.is_some()).count() as u64),
                    ),
                    (
                        "linear_gradients",
                        num(draws
                            .iter()
                            .filter(|d| d.gradient.as_ref().is_some_and(|g| g.radial.is_none()))
                            .count() as u64),
                    ),
                    (
                        "destinations",
                        Value::List(
                            output
                                .iter()
                                .map(|(a, p)| {
                                    dict([
                                        ("address".into(), hex(*a)),
                                        ("bytes".into(), num(p.len() as u64)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    ("changed_bytes", num(bytes)),
                    ("covered_bytes_in_draw_order", num(touched)),
                    ("irq_cpu", num(irq_cpu)),
                    ("latency_ticks", num(delay)),
                    (
                        "pixel_semantics",
                        Value::constant(
                            "declared software RGBA8 SRC/SRC_OVER, affine paths, regular-grid AA, centerline strokes, nearest Morton images, linear ramp interpolation; hardware pixels unmeasured",
                        ),
                    ),
                    (
                        "completion_semantics",
                        Value::constant(
                            "atomic validated whole batch before bit31/physicalIRQ3c; throughput timing unmeasured",
                        ),
                    ),
                ],
            );
        }
        let radial_count = draws
            .iter()
            .filter(|d| d.gradient.as_ref().is_some_and(|g| g.radial.is_some()))
            .count();
        if record_receipts && radial_count != 0 {
            update(
                &mut c.job,
                vec![
                    ("radial_gradients", num(radial_count as u64)),
                    (
                        "pixel_semantics",
                        Value::constant(
                            "declared software RGBA8 SRC/SRC_OVER, affine paths, regular-grid AA, centerline strokes, nearest Morton images, linear/radial ramp interpolation; hardware pixels unmeasured",
                        ),
                    ),
                ],
            );
        }
        if record_receipts {
            log(actions, "Graphics declared UI batch started", &c.job);
        }
        self.schedule(
            Completion {
                cpu,
                irq_cpu,
                status: CLEAR_STATUS,
                bytes,
                output,
                job: c.job.clone(),
                event: CompletionEvent::Batch,
                ui: true,
            },
            delay,
            actions,
        );
        Ok(())
    }
}
struct Capture {
    job: Job,
    words: HashMap<&'static str, Vec<u32>>,
    invalid: BTreeSet<&'static str>,
}
impl Capture {
    fn words(&self, bank: &str) -> Result<Vec<u32>> {
        if self.invalid.contains(bank) || !self.words.contains_key(bank) {
            return fail(format!(
                "Graphics clear-only {bank} requires a complete bounded payload capture"
            ));
        }
        Ok(self.words[bank].clone())
    }
    fn hex_words(&mut self) {
        if !self.job.record_receipts {
            return;
        }
        let fields = &mut self.job.fields;
        let streams = &mut fields.iter_mut().find(|(k, _)| *k == "streams").unwrap().1;
        for (name, words) in &self.words {
            if let Some(s) = dget_mut(streams, name)
                && dget(s, "words").is_none()
                && dget(s, "word_count").is_none()
            {
                dset(
                    s,
                    "words",
                    Value::List(words.iter().map(|n| hex(*n)).collect()),
                );
            }
        }
        self.job.revision += 1;
    }
    fn compact(&mut self) {
        if !self.job.record_receipts {
            return;
        }
        let fields = &mut self.job.fields;
        let streams = &mut fields.iter_mut().find(|(k, _)| *k == "streams").unwrap().1;
        for (name, words) in &self.words {
            if let Some(Value::Dict(s)) = dget_mut(streams, name) {
                s.retain(|(k, _)| !k.is_key("words"));
                s.push((Value::constant("word_count"), num(words.len() as u64)));
            }
        }
        self.job.revision += 1;
    }
}

#[cfg(not(target_os = "emscripten"))]
fn coverage_kernel() -> Result<&'static CoverageKernel> {
    static KERNEL: OnceLock<std::result::Result<CoverageKernel, String>> = OnceLock::new();
    match KERNEL.get_or_init(|| fujimod_emulator_gpu::context().map(CoverageKernel::new)) {
        Ok(kernel) => Ok(kernel),
        Err(error) => fail(format!("Graphics UI GPU coverage {error}")),
    }
}

#[cfg(target_arch = "wasm32")]
fn ui_coverages(draws: &[batch::Draw]) -> Result<Vec<std::rc::Rc<render::Coverage>>> {
    draws
        .iter()
        .map(|draw| render::coverage(draw).map(std::rc::Rc::new))
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn ui_render(draws: &[batch::Draw], cpu: i64, host: &mut dyn Inputs) -> Result<RenderedBatch> {
    ui_paint::render(draws, cpu, host)
}

#[cfg(target_arch = "wasm32")]
pub fn ui_render(draws: &[batch::Draw], cpu: i64, host: &mut dyn Inputs) -> Result<RenderedBatch> {
    let mut targets = HashMap::new();
    let mut textures = HashMap::new();
    for d in draws {
        let (a, w, h, _) = d.target;
        if let std::collections::hash_map::Entry::Vacant(entry) = targets.entry(a) {
            let p = host
                .read(cpu, a, w as u64 * h as u64 * 4)
                .map_err(Unsupported)?;
            if p.len() as u64 != w as u64 * h as u64 * 4 {
                return fail("Graphics UI destination/texture snapshot is incomplete");
            }
            entry.insert(p);
        }
        if let Some(t) = &d.texture {
            let k = (t.source, t.width, t.height);
            if let std::collections::hash_map::Entry::Vacant(entry) = textures.entry(k) {
                let p = host
                    .read(cpu, t.source, t.width as u64 * t.height as u64 * 4)
                    .map_err(Unsupported)?;
                if p.len() as u64 != t.width as u64 * t.height as u64 * 4 {
                    return fail("Graphics UI destination/texture snapshot is incomplete");
                }
                entry.insert(p);
            }
        }
    }
    let mut coverages = ui_coverages(draws)?.into_iter();
    let (output, touched) = render::render(draws, &targets, &textures, &mut |_draw| {
        coverages
            .next()
            .ok_or_else(|| Unsupported("Graphics UI coverage batch ended early".into()))
    })?;
    if coverages.next().is_some() {
        return fail("Graphics UI coverage batch has unused draws");
    }
    let output = output
        .into_iter()
        .map(|(address, bytes)| (address, Arc::new(bytes)))
        .collect();
    Ok((output, touched))
}

/// Render one parsed vector batch with the shared WebGPU device.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub async fn render_ui_host_gpu_async(
    draws: &[batch::Draw],
    cpu: i64,
    host: &mut dyn Inputs,
) -> Result<RenderedBatch> {
    fujimod_emulator_gpu::context_async()
        .await
        .map_err(|error| Unsupported(format!("Graphics UI GPU context {error}")))?;
    ui_paint::render_async(draws, cpu, host).await
}

impl CommandProcessor {
    pub fn snapshot(&self, device: &str, routed: bool) -> Value {
        let mut registers: Vec<_> = self.regs.items().collect();
        registers.sort_by_key(|(k, _)| *k);
        let mut path: Vec<_> = self.path.items().collect();
        path.sort_by_key(|(k, _)| *k);
        let mut fill: Vec<_> = self.fill.items().collect();
        fill.sort_by_key(|(k, _)| *k);
        let j = self.last_job.as_ref().map_or(Value::None, |j| {
            dict(j.fields.iter().map(|(k, v)| (k.to_string(), v.clone())))
        });
        dict(vec![
            ("device".into(), text(device)),
            ("base".into(), hex(BASE)),
            ("logical_offset_multiplier".into(), num(2)),
            (
                "registers".into(),
                dict(
                    registers
                        .into_iter()
                        .map(|(o, v)| (format!("{:#x}", BASE + o as u64 * 2), hex(v))),
                ),
            ),
            (
                "written_logical_offsets".into(),
                Value::List(self.written.iter().map(|o| hex(*o)).collect()),
            ),
            ("busy".into(), Value::Bool(self.busy)),
            ("jobs_started".into(), num(self.jobs_started)),
            ("jobs_completed".into(), num(self.jobs_completed)),
            ("bytes_completed".into(), num(self.bytes_completed)),
            ("cpu_raster_calls".into(), num(self.cpu_raster_calls)),
            (
                "cpu_vector_paint_calls".into(),
                num(self.cpu_vector_paint_calls),
            ),
            ("gpu_gles_lists".into(), num(self.gpu_gles_lists)),
            (
                "gpu_vector_paint_calls".into(),
                num(self.gpu_vector_paint_calls),
            ),
            ("irqs_raised".into(), num(self.irqs_raised)),
            ("acknowledgments".into(), num(self.acknowledgments)),
            ("last_job".into(), j),
            ("bytes_per_tick".into(), num(self.bytes_per_tick)),
            (
                "copy_profile".into(),
                self.copy_profile.map_or(Value::None, Profile::value),
            ),
            ("max_copy_bytes".into(), num(self.max_copy_bytes)),
            (
                "max_command_capture_bytes".into(),
                num(self.max_command_capture_bytes),
            ),
            (
                "clear_profile".into(),
                self.clear_profile.map_or(Value::None, Profile::value),
            ),
            ("max_clear_bytes".into(), num(self.max_clear_bytes)),
            (
                "command_pending".into(),
                Value::Bool(self.command_pending.is_some()),
            ),
            (
                "command_register_banks".into(),
                dict([
                    (
                        "path".into(),
                        dict(path.into_iter().map(|(o, v)| (format!("{o:#x}"), hex(v)))),
                    ),
                    (
                        "fill".into(),
                        dict(fill.into_iter().map(|(o, v)| (format!("{o:#x}"), hex(v)))),
                    ),
                ]),
            ),
            (
                "irq_routing".into(),
                Value::constant(if routed {
                    "exactly one configured bank at trigger"
                } else {
                    "submitting CPU fixture"
                }),
            ),
            (
                "limits".into(),
                Value::List(vec![
                    Value::constant(
                        "Pattern DMA supports 16-bit repetition and masked 32-bit byte lanes",
                    ),
                    Value::constant(
                        "Initialization register values are explicit fixtures; physical reset values are unmeasured",
                    ),
                    Value::constant(
                        "Command-list rasterization and unknown copy controls/formats stop explicitly",
                    ),
                    Value::constant(
                        "Optional vg-32bpp-untile-unrotated profile keeps tile bytes and presents rows bottom-up; direct hardware-copy behavior is unverified",
                    ),
                    Value::constant(
                        "Copy source snapshot/atomic completion timing and dimension/alignment bounds are explicit model assumptions",
                    ),
                    Value::constant(
                        "Optional clear-only profile decodes format0/no-source/no-scissor rectangles and state-only initialization; supported register state, paired fences and RAM bounds are checked independently of writer order",
                    ),
                    Value::constant(
                        "State-only initializers contain no geometry and perform zero pixel writes; completion semantics remain inferred",
                    ),
                    Value::constant(
                        "Clear pixel packing/Morton layout derive from software; cross-stream fence/draw, bit31 completion and atomic destination snapshot are inferred",
                    ),
                    Value::constant(
                        "Optional declared UI profile parses byte-lane registers, fixed-arity geometry and paired draw fences; unknown commands, paints, formats or state reject before DMA/completion",
                    ),
                    Value::constant(
                        "UI profile supports bounded affine format0 solid/image/linear/radial-ramp fills and nondashed centerline strokes; gamma, projective, masks and nonidentity color transforms remain unsupported",
                    ),
                    Value::constant(
                        "UI straight RGBA SRC/SRC_OVER, regular1/2/4 squared AA,0.125px curve flattening, reconstructed cap/join strokes with miter limit4, nearest image sampling and transparent image border are declared unmeasured laws",
                    ),
                    Value::constant(
                        "UI whole-batch destination/input snapshots and bit31/IRQ completion timing are model assumptions; actual driver progress does not validate hardware pixel fidelity",
                    ),
                    Value::constant(
                        "Logical105c retains explicit zero reset fixture; its physical post-wait value is unmeasured, never forced ready",
                    ),
                    Value::constant(
                        "Pattern fill timing/atomic completion and IRQ bank-routing rule are model assumptions",
                    ),
                    Value::constant(
                        "Real kernel IRQ3c registration/masking and task scheduling remain the Board's responsibility",
                    ),
                ]),
            ),
        ])
    }
}

impl CommandProcessor {
    pub fn deep_copy(&self) -> Self {
        self.clone()
    }
}

#[cfg(test)]
mod send_tests {
    #[test]
    fn shared_model_and_receipts_are_send() {
        fn require_send<T: Send>() {}
        require_send::<super::CommandProcessor>();
        require_send::<fujimod_emulator_devices::Value>();
        require_send::<fujimod_emulator_devices::Record>();
        require_send::<fujimod_emulator_devices::Action>();
    }
}
