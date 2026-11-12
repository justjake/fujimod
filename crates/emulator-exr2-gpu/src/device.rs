//! Runtime connection for the vector engine: physical DMA, routed IRQs and
//! exact completion deadlines. The peripheral has no processor capability.
use super::{CommandProcessor, GlesLatency, IRQ, Inputs, Profile, recording};
use fujimod_emulator_devices::{Action, Cx, Device, Unsupported, WordMap, bus::HostRequest};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub ram_bytes: u64,
    pub max_fill_bytes: u64,
    pub max_copy_bytes: u64,
    pub max_command_capture_bytes: u64,
    pub max_clear_bytes: u64,
    pub bytes_per_tick: u64,
    pub copy_bytes_per_tick: u64,
    /// 3D list latency terms; see `GlesLatency` and `X70_GLES_LATENCY`.
    pub gles_base_ticks: u64,
    pub gles_draw_ticks: u64,
    pub gles_filled_pixel_microticks: u64,
    pub gles_blended_pixel_microticks: u64,
    pub copy_profile: Option<Profile>,
    pub clear_profile: Option<Profile>,
}
pub struct Wiring {
    pub controllers: Vec<(usize, fujimod_emulator_devices::exr2::irq::Inputs)>,
}
pub struct VectorGpu {
    publication_generation: u64,
    state: CommandProcessor,
    controllers: [Option<fujimod_emulator_devices::exr2::irq::Inputs>; 2],
    due: BTreeMap<u64, u64>,
    due_order: BTreeSet<(u64, u64)>,
    recording: Option<recording::Recorder>,
    next_host_request: u64,
    host_request: Option<HostRequest>,
    host_request_published: bool,
}
impl VectorGpu {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        let mut controllers: [Option<fujimod_emulator_devices::exr2::irq::Inputs>; 2] =
            Default::default();
        for (core, inputs) in wiring.controllers {
            if core >= 2 || controllers[core].is_some() {
                return Err("invalid vector engine interrupt wiring".into());
            }
            controllers[core] = Some(inputs);
        }
        let capture_limit = if options.clear_profile == Some(Profile::GpuUiProfile) {
            0x80000
        } else {
            0x10000
        };
        if controllers.iter().any(Option::is_none)
            || options.ram_bytes == 0
            || options.ram_bytes > 0x40000000
            || [
                options.max_fill_bytes,
                options.max_copy_bytes,
                options.max_clear_bytes,
            ]
            .iter()
            .any(|&n| n == 0 || n > options.ram_bytes)
            || options.bytes_per_tick == 0
            || options.copy_bytes_per_tick == 0
            || options.gles_base_ticks == 0
            || !(8..=capture_limit).contains(&options.max_command_capture_bytes)
            || options.max_command_capture_bytes & 7 != 0
            || !matches!(options.copy_profile, None | Some(Profile::GpuCopyProfile))
            || !matches!(
                options.clear_profile,
                None | Some(Profile::GpuClearProfile | Profile::GpuUiProfile)
            )
        {
            return Err("invalid vector engine configuration, bounds or IRQ connections".into());
        }
        let state = CommandProcessor {
            ram_bytes: options.ram_bytes,
            max_fill_bytes: options.max_fill_bytes,
            max_copy_bytes: options.max_copy_bytes,
            max_command_capture_bytes: options.max_command_capture_bytes,
            max_clear_bytes: options.max_clear_bytes,
            bytes_per_tick: options.bytes_per_tick,
            copy_bytes_per_tick: options.copy_bytes_per_tick,
            gles_latency: super::GlesLatency {
                base_ticks: options.gles_base_ticks,
                draw_ticks: options.gles_draw_ticks,
                filled_pixel_microticks: options.gles_filled_pixel_microticks,
                blended_pixel_microticks: options.gles_blended_pixel_microticks,
            },
            copy_profile: options.copy_profile,
            clear_profile: options.clear_profile,
            ..CommandProcessor::default()
        };
        let recording = std::env::var_os("FUJIMOD_VECTOR_GPU_RECORD")
            .map(|path| {
                recording::Recorder::create(Path::new(&path), &recording::Header::from(&state))
                    .map_err(|error| format!("could not start vector GPU recording: {error}"))
            })
            .transpose()?;
        Ok(Self {
            publication_generation: 0,
            state,
            controllers,
            due: BTreeMap::new(),
            due_order: BTreeSet::new(),
            recording,
            next_host_request: 0,
            host_request: None,
            host_request_published: false,
        })
    }
    fn actions(&mut self, cx: &mut Cx<'_>, actions: &mut Vec<Action>) -> Result<(), String> {
        for action in actions.drain(..) {
            match action {
                Action::Log(record) | Action::Record(record) => cx.record(record),
                Action::QueueIrq(core, line) => cx.raise_irq(core.into(), line.into())?,
                Action::Schedule { delay, token } => {
                    let at = cx
                        .now()
                        .checked_add(delay)
                        .ok_or("vector engine completion time overflow")?;
                    if delay == 0 || self.due.contains_key(&token) {
                        return Err("invalid vector engine completion token".into());
                    }
                    self.due.insert(token, at);
                    self.due_order.insert((at, token));
                }
            }
        }
        Ok(())
    }
    fn settle(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let now = cx.now();
        let mut actions = Vec::new();
        while let Some(&(at, token)) = self.due_order.first() {
            if at > now {
                break;
            }
            {
                let outputs = self.state.output(token).map_err(|e| e.0)?;
                for (address, bytes) in outputs {
                    cx.ram_write(*address, bytes)?;
                }
                if let Some(recording) = &mut self.recording {
                    recording.completion(now, token, outputs).map_err(|error| {
                        format!("could not write vector GPU recording: {error}")
                    })?;
                }
            }
            self.state
                .complete(token, now, &mut actions)
                .map_err(|e| e.0)?;
            self.due.remove(&token);
            self.due_order.remove(&(at, token));
            self.actions(cx, &mut actions)?;
        }
        Ok(())
    }
}
struct Connected<'a, 'b> {
    cx: &'a mut Cx<'b>,
    next_host_request: &'a mut u64,
    host_request: &'a mut Option<HostRequest>,
    controllers: &'a [Option<fujimod_emulator_devices::exr2::irq::Inputs>; 2],
    dma_failure: Option<String>,
    capture: bool,
    memory_reads: Vec<recording::MemoryRead>,
    target_samples: Vec<recording::TargetSample>,
}
impl Inputs for Connected<'_, '_> {
    fn has_reader(&self) -> bool {
        true
    }
    fn has_irq(&self) -> bool {
        true
    }
    fn routed(&self) -> bool {
        true
    }
    fn read(&mut self, cpu: i64, address: u64, length: u64) -> Result<Vec<u8>, String> {
        let size = match usize::try_from(length) {
            Ok(size) => size,
            Err(_) => {
                let error = "vector DMA length exceeds host address space".to_owned();
                self.dma_failure = Some(error.clone());
                if self.capture {
                    self.memory_reads.push(recording::MemoryRead {
                        cpu,
                        address,
                        length,
                        result: Err(error.clone()),
                    });
                }
                return Err(error);
            }
        };
        let result = match self.cx.ram_bytes(address, size) {
            Ok(bytes) => Ok(bytes),
            Err(error) => {
                self.dma_failure = Some(error.clone());
                Err(error)
            }
        };
        if self.capture {
            self.memory_reads.push(recording::MemoryRead {
                cpu,
                address,
                length,
                result: result.clone(),
            });
        }
        result
    }
    fn submit_webgpu_request(
        &mut self,
        kind: &str,
        payload: Vec<u8>,
    ) -> fujimod_emulator_devices::record::Result<Option<u64>> {
        if !super::webgpu_bridge_enabled() {
            return Ok(None);
        }
        if self.host_request.is_some() {
            return Err(Unsupported(
                "Vector GPU already has an asynchronous request".into(),
            ));
        }
        self.cx.request_pause().map_err(Unsupported)?;
        let id = *self.next_host_request;
        *self.next_host_request = id
            .checked_add(1)
            .ok_or_else(|| Unsupported("Vector WebGPU request ID exhausted".into()))?;
        *self.host_request = Some(HostRequest {
            id,
            kind: kind.to_owned(),
            payload,
        });
        Ok(Some(id))
    }
    fn targets(&mut self) -> fujimod_emulator_devices::record::Result<(usize, i64)> {
        let mut count = 0;
        let mut selected = -1;
        for (core, pins) in self.controllers.iter().enumerate() {
            if pins
                .as_ref()
                .is_some_and(|pins| pins.enabled() & (1 << IRQ) != 0)
            {
                selected = core as i64;
                count += 1;
            }
        }
        let result = Ok((count, selected));
        if self.capture {
            self.target_samples
                .push(recording::TargetSample::Routed { count, selected });
        }
        result
    }
}
impl VectorGpu {
    fn read_published(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let now = cx.now();
        let cpu = cx.cpu as i64;
        if let Err(error) = self.settle(cx) {
            cx.fail(error);
            return 0;
        }
        let result = self
            .state
            .read(address, size.into())
            .map_err(|error| error.0);
        if let Some(recording) = &mut self.recording
            && let Err(error) = recording.register_read(&recording::RegisterRead {
                now,
                cpu,
                address,
                size,
                result: result.clone(),
            })
        {
            cx.fail(format!("could not write vector GPU recording: {error}"));
            return 0;
        }
        match result {
            Ok(value) => value.into(),
            Err(error) => {
                cx.unsupported(error);
                0
            }
        }
    }
}
impl VectorGpu {
    fn write_published(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let now = cx.now();
        let cpu = cx.cpu as i64;
        if let Err(error) = self.settle(cx) {
            cx.fail(error);
            return;
        }
        let Ok(value) = u32::try_from(value) else {
            if let Err(error) = record_write(
                &mut self.recording,
                now,
                cpu,
                address,
                size,
                value,
                Err("vector GPU word exceeds 32 bits".into()),
                Vec::new(),
                Vec::new(),
            ) {
                cx.fail(error);
                return;
            }
            cx.unsupported("vector GPU word exceeds 32 bits");
            return;
        };
        if size != 4 {
            if let Err(error) = record_write(
                &mut self.recording,
                now,
                cpu,
                address,
                size,
                u64::from(value),
                Err("vector GPU requires aligned word accesses".into()),
                Vec::new(),
                Vec::new(),
            ) {
                cx.fail(error);
                return;
            }
            cx.unsupported("vector GPU requires aligned word accesses");
            return;
        }
        let mut host = Connected {
            cx,
            next_host_request: &mut self.next_host_request,
            host_request: &mut self.host_request,
            controllers: &self.controllers,
            dma_failure: None,
            capture: self.recording.is_some(),
            memory_reads: Vec::new(),
            target_samples: Vec::new(),
        };
        let mut actions = Vec::new();
        let result = self
            .state
            .write_native(cpu, address, value, &mut host, &mut actions);
        let dma_failure = host.dma_failure.take();
        let memory_reads = std::mem::take(&mut host.memory_reads);
        let target_samples = std::mem::take(&mut host.target_samples);
        drop(host);
        let access_result = result.as_ref().map(|_| ()).map_err(|error| error.0.clone());
        if let Err(error) = record_write(
            &mut self.recording,
            now,
            cpu,
            address,
            size,
            u64::from(value),
            access_result,
            memory_reads,
            target_samples,
        ) {
            cx.fail(error);
            return;
        }
        if let Some(error) = dma_failure {
            cx.fail(error);
            return;
        }
        // A rejected hardware command does not acquire a completion or DMA.
        // Preserve any diagnostic records emitted before the refusal.
        if let Err(error) = self.actions(cx, &mut actions) {
            cx.fail(error);
            return;
        }
        if let Err(Unsupported(reason)) = result {
            cx.unsupported(reason);
        }
    }
}
impl VectorGpu {
    fn advance_published(&mut self, cx: &mut Cx<'_>) {
        if self
            .next_deadline(cx.now())
            .is_some_and(|at| at <= cx.now())
            && let Err(error) = self.settle(cx)
        {
            cx.fail(error);
        }
    }
}
impl VectorGpu {
    fn complete_host_request_published(
        &mut self,
        cx: &mut Cx<'_>,
        id: u64,
        result: &[u8],
    ) -> Result<(), String> {
        if self
            .host_request
            .as_ref()
            .is_none_or(|request| request.id != id || !self.host_request_published)
        {
            return Err("Vector WebGPU result has no matching published request".into());
        }
        let mut actions = Vec::new();
        self.state
            .complete_webgpu_request(id, result, &mut actions)
            .map_err(|error| error.0)?;
        self.host_request = None;
        self.host_request_published = false;
        self.actions(cx, &mut actions)
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    state: CommandProcessor,
    due: BTreeMap<u64, u64>,
    due_order: BTreeSet<(u64, u64)>,
}
#[derive(PartialEq, serde::Serialize)]
struct PublishedJob {
    id: u64,
    revision: u64,
    fields: super::JobFields,
}
#[derive(PartialEq, serde::Serialize)]
struct Publication {
    gles: fujimod_emulator_ip_dmp::gles::Publication,
    last_job: Option<PublishedJob>,
    pending_completions: usize,
    #[serde(skip)]
    deadlines: BTreeMap<u64, u64>,
    cpu_raster_calls: u64,
    cpu_vector_paint_calls: u64,
    gpu_gles_lists: u64,
    gpu_vector_paint_calls: u64,
    regs: WordMap,
    path: WordMap,
    fill: WordMap,
    written: BTreeSet<u32>,
    clear_ack_offsets: BTreeSet<u32>,
    command_pending: Option<(i64, Vec<u32>)>,
    busy: bool,
    jobs_started: i64,
    jobs_completed: i64,
    bytes_completed: i64,
    irqs_raised: i64,
    acknowledgments: i64,
    ram_bytes: u64,
    max_fill_bytes: u64,
    max_copy_bytes: u64,
    max_command_capture_bytes: u64,
    max_clear_bytes: u64,
    bytes_per_tick: u64,
    copy_bytes_per_tick: u64,
    gles_latency: GlesLatency,
    gles_latency_ticks: u64,
    gles_byte_rate_ticks: u64,
    copy_profile: Option<Profile>,
    clear_profile: Option<Profile>,
    next_token: u64,
    next_job: u64,
}
impl VectorGpu {
    fn publication_state(&self) -> Publication {
        let s = &self.state;
        Publication {
            gles: s.gles.publication_state(),
            last_job: s.last_job.as_ref().map(|job| PublishedJob {
                id: job.id,
                revision: job.revision,
                fields: job.fields.clone(),
            }),
            pending_completions: s.completions.len(),
            deadlines: self.due.clone(),
            cpu_raster_calls: s.cpu_raster_calls,
            cpu_vector_paint_calls: s.cpu_vector_paint_calls,
            gpu_gles_lists: s.gpu_gles_lists,
            gpu_vector_paint_calls: s.gpu_vector_paint_calls,
            regs: s.regs.clone(),
            path: s.path.clone(),
            fill: s.fill.clone(),
            written: s.written.clone(),
            clear_ack_offsets: s.clear_ack_offsets.clone(),
            command_pending: s.command_pending.clone(),
            busy: s.busy,
            jobs_started: s.jobs_started,
            jobs_completed: s.jobs_completed,
            bytes_completed: s.bytes_completed,
            irqs_raised: s.irqs_raised,
            acknowledgments: s.acknowledgments,
            ram_bytes: s.ram_bytes,
            max_fill_bytes: s.max_fill_bytes,
            max_copy_bytes: s.max_copy_bytes,
            max_command_capture_bytes: s.max_command_capture_bytes,
            max_clear_bytes: s.max_clear_bytes,
            bytes_per_tick: s.bytes_per_tick,
            copy_bytes_per_tick: s.copy_bytes_per_tick,
            gles_latency: s.gles_latency,
            gles_latency_ticks: s.gles_latency_ticks,
            gles_byte_rate_ticks: s.gles_byte_rate_ticks,
            copy_profile: s.copy_profile,
            clear_profile: s.clear_profile,
            next_token: s.next_token,
            next_job: s.next_job,
        }
    }
}

impl Device for VectorGpu {
    fn publication_generation(&self) -> Option<u64> {
        Some(self.publication_generation)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        #[derive(serde::Serialize)]
        struct Snapshot<'a> {
            state: &'a CommandProcessor,
            due: &'a BTreeMap<u64, u64>,
            due_order: &'a BTreeSet<(u64, u64)>,
        }
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &Snapshot {
                state: &self.state,
                due: &self.due,
                due_order: &self.due_order,
            },
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let _: Saved = saved.decode(self.name())?;
        if !saved.blobs().is_empty() {
            return Err("device snapshot blob lengths differ".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let saved: Saved = saved.decode(self.name())?;
        let before = self.publication_state();
        self.state = saved.state;
        self.due = saved.due;
        self.due_order = saved.due_order;
        if self.publication_state() != before {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
        Ok(())
    }
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"engine":self.state,"deadlines":self.due,"deadline_order":self.due_order}),
        )
    }

    fn name(&self) -> &'static str {
        "vector-gpu"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if self
            .next_deadline(cx.now())
            .is_none_or(|due| due > cx.now())
        {
            return self.read_published(cx, address, size);
        }
        let before = self.publication_state();
        let result = self.read_published(cx, address, size);
        if self.publication_state() != before {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
        result
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let before = self.publication_state();
        self.write_published(cx, address, size, value);
        if self.publication_state() != before {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.state.read(address, size.into()).is_ok()
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.due_order.first().map(|(at, _)| *at)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self
            .next_deadline(cx.now())
            .is_none_or(|due| due > cx.now())
        {
            return;
        }
        let before = self.publication_state();
        self.advance_published(cx);
        if self.publication_state() != before {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
    }
    fn take_host_request(&mut self) -> Option<HostRequest> {
        if self.host_request_published {
            return None;
        }
        let request = self.host_request.clone()?;
        self.host_request_published = true;
        Some(request)
    }
    fn has_host_request(&self) -> bool {
        self.host_request.is_some()
    }
    fn complete_host_request(
        &mut self,
        cx: &mut Cx<'_>,
        id: u64,
        result: &[u8],
    ) -> Result<(), String> {
        let before = self.publication_state();
        let result = self.complete_host_request_published(cx, id, result);
        if self.publication_state() != before {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
        result
    }
    fn save(&self) -> serde_json::Value {
        // Publication excludes pending DMA pixels, retained by exact checkpoints.
        serde_json::json!({"engine":self.publication_state(),"deadlines":self.due})
    }
}

fn record_write(
    recording: &mut Option<recording::Recorder>,
    now: u64,
    cpu: i64,
    address: u64,
    size: u8,
    value: u64,
    result: std::result::Result<(), String>,
    memory_reads: Vec<recording::MemoryRead>,
    target_samples: Vec<recording::TargetSample>,
) -> Result<(), String> {
    if let Some(recording) = recording {
        recording
            .register_write(&recording::RegisterWrite {
                now,
                cpu,
                address,
                size,
                value,
                result,
                memory_reads,
                target_samples,
            })
            .map_err(|error| format!("could not write vector GPU recording: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
