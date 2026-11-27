//! Opt-in, cumulative timing and execution snapshots for XP5 boots.
use fujimod_emulator::board::Board;
use fujimod_emulator::board::observation::RegisterAccessSnapshot;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const PHASE_FILE: &str = "phase-timing.jsonl";
static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(0);

pub(super) struct PhaseRecorder {
    output: Option<BufWriter<File>>,
    path: PathBuf,
    run_id: String,
    wall_start: Instant,
    guest_start: u64,
    ticks_per_second: u64,
    configuration: String,
    firmware: String,
    execution_policy: String,
    backend: Value,
    previous_register_access: Option<RegisterAccessSnapshot>,
    _perf_scope: Option<fujimod_perf::GuideScope>,
}

impl PhaseRecorder {
    pub(super) fn open(
        run_directory: &Path,
        wall_start: Instant,
        guest_start: u64,
        ticks_per_second: u64,
        configuration: &str,
        firmware: &str,
        execution_policy: &str,
        backend: Value,
    ) -> Result<Self, String> {
        let path = run_directory.join(PHASE_FILE);
        let attribute = std::env::var_os("FUJIMOD_EXECUTION_COUNTERS").is_some();
        let output = if attribute || std::env::var_os("FUJIMOD_PHASE_TIMING").is_some() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            Some(BufWriter::new(file))
        } else {
            None
        };
        let perf_scope = attribute.then(|| fujimod_perf::guide_scope("xp5-boot"));
        let wall_start_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let run_sequence = NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            output,
            path,
            run_id: format!("{}-{wall_start_unix_ns}-{run_sequence}", std::process::id()),
            wall_start,
            guest_start,
            ticks_per_second,
            configuration: configuration.to_owned(),
            firmware: firmware.to_owned(),
            execution_policy: execution_policy.to_owned(),
            backend,
            previous_register_access: None,
            _perf_scope: perf_scope,
        })
    }

    pub(super) fn record(
        &mut self,
        event: &str,
        board: &Board,
        frame: Option<(usize, u64, u64)>,
    ) -> Result<(), String> {
        self.record_event(event, board, frame, None)
    }

    pub(super) fn record_end(
        &mut self,
        board: &Board,
        success: bool,
        stop_reason: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.record_event("end", board, None, Some((success, stop_reason, error)))
    }

    fn record_event(
        &mut self,
        event: &str,
        board: &Board,
        frame: Option<(usize, u64, u64)>,
        outcome: Option<(bool, &str, Option<&str>)>,
    ) -> Result<(), String> {
        if self.output.is_none() {
            return Ok(());
        }

        let wall_elapsed_ns =
            u64::try_from(self.wall_start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let guest_tick = board.now();
        let register_access_delta = board
            .register_access_snapshot()
            .map(|snapshot| self.register_access_delta(snapshot));
        let engine_exit_counters_by_core = board
            .exit_counters()
            .into_iter()
            .enumerate()
            .map(|(core, counters)| match counters {
                Ok(counters) => json!({"core": core, "counters": counters}),
                Err(error) => json!({"core": core, "error": error}),
            })
            .collect::<Vec<_>>();
        let mut record = json!({
            "run_id": self.run_id,
            "event": event,
            "wall_elapsed_ns": wall_elapsed_ns,
            "guest_tick": guest_tick,
            "guest_ticks_elapsed": guest_tick.saturating_sub(self.guest_start),
            "ticks_per_second": self.ticks_per_second,
            "configuration": self.configuration,
            "firmware": self.firmware,
            "execution_policy": self.execution_policy,
            "backend": self.backend,
            "perf": fujimod_perf::guide_performance_snapshot(),
            "instructions_by_core": board.instruction_counts(),
            "engine_exit_counters_by_core": engine_exit_counters_by_core,
            "mmio_callbacks_by_core": board.mmio_trap_callback_counts(),
            "board_execution": board.execution_metrics(),
        });
        if let Some(register_access_delta) = register_access_delta {
            record
                .as_object_mut()
                .expect("phase record is a JSON object")
                .insert("register_access_delta".into(), register_access_delta);
        }
        if let Some((bank, archive_sequence, modeled_sequence)) = frame {
            let fields = record
                .as_object_mut()
                .expect("phase record is a JSON object");
            fields.insert("frame_bank".into(), json!(bank));
            fields.insert("frame_sequence".into(), json!(archive_sequence));
            fields.insert("modeled_frame_sequence".into(), json!(modeled_sequence));
        }
        if let Some((success, stop_reason, error)) = outcome {
            let fields = record
                .as_object_mut()
                .expect("phase record is a JSON object");
            fields.insert("success".into(), json!(success));
            fields.insert("stop_reason".into(), json!(stop_reason));
            if let Some(error) = error {
                fields.insert("error".into(), json!(error));
            }
        }
        let output = self.output.as_mut().expect("phase output was checked");
        writeln!(output, "{record}")
            .and_then(|()| output.flush())
            .map_err(|error| format!("{}: {error}", self.path.display()))
    }

    fn register_access_delta(&mut self, current: RegisterAccessSnapshot) -> Value {
        let Some(previous) = self.previous_register_access.take() else {
            let service_sampling_denominator = current.service_sampling_denominator;
            self.previous_register_access = Some(current);
            return json!({
                "service_sampling_probability": {
                    "numerator": 1,
                    "denominator": service_sampling_denominator,
                },
                "accesses": [],
                "published_fast_read_hits_subset_by_mapping": [],
            });
        };
        let accesses = current
            .totals
            .iter()
            .filter_map(|(key, totals)| {
                let old = previous.totals.get(key).copied().unwrap_or_default();
                let calls = totals.calls.saturating_sub(old.calls);
                let service_samples = totals.service_samples.saturating_sub(old.service_samples);
                let sampled_service_ns = totals
                    .sampled_service_ns
                    .saturating_sub(old.sampled_service_ns);
                (calls != 0 || service_samples != 0 || sampled_service_ns != 0).then(|| {
                    json!({
                        "core": key.core,
                        "address": key.address,
                        "operation": if key.write { "write" } else { "read" },
                        "calls": calls,
                        "service_samples": service_samples,
                        "sampled_service_ns": sampled_service_ns,
                    })
                })
            })
            .collect::<Vec<_>>();
        let published_fast_read_hits_subset_by_mapping = current
            .published_fast_read_hits
            .iter()
            .filter_map(|(key, &hits)| {
                let old = previous
                    .published_fast_read_hits
                    .get(key)
                    .copied()
                    .unwrap_or(0);
                let count = hits.saturating_sub(old);
                (count != 0).then(|| {
                    json!({
                        "core": key.core,
                        "mapping_base": key.mapping_base,
                        "count": count,
                    })
                })
            })
            .collect::<Vec<_>>();
        let delta = json!({
            "service_sampling_probability": {
                "numerator": 1,
                "denominator": current.service_sampling_denominator,
            },
            "accesses": accesses,
            "published_fast_read_hits_subset_by_mapping":
                published_fast_read_hits_subset_by_mapping,
        });
        self.previous_register_access = Some(current);
        delta
    }
}
