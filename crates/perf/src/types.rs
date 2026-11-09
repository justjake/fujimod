//! Public vocabulary shared by the enabled and compiled-out implementations.
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitKind {
    Debounce,
    InputGate,
    OsdFrame,
    RecReturn,
    CardLoad,
    Shutter,
    Other,
}

impl WaitKind {
    pub const ALL: [WaitKind; 7] = [
        WaitKind::Debounce,
        WaitKind::InputGate,
        WaitKind::OsdFrame,
        WaitKind::RecReturn,
        WaitKind::CardLoad,
        WaitKind::Shutter,
        WaitKind::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            WaitKind::Debounce => "debounce",
            WaitKind::InputGate => "input_gate",
            WaitKind::OsdFrame => "osd_frame",
            WaitKind::RecReturn => "rec_return",
            WaitKind::CardLoad => "card_load",
            WaitKind::Shutter => "shutter",
            WaitKind::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name() == value)
            .ok_or_else(|| format!("unknown wait kind {value:?}"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpanKind {
    Guide,
    Setup,
    Boot,
    Restore,
    Checkpoint,
    Prerequisite,
    Wait(WaitKind),
    Capture,
    Input,
    Assertion,
    Oracle,
    Media,
    Grading,
}

impl SpanKind {
    pub const COUNT: usize = 19;

    pub fn name(self) -> &'static str {
        match self {
            SpanKind::Guide => "guide",
            SpanKind::Setup => "setup",
            SpanKind::Boot => "boot",
            SpanKind::Restore => "restore",
            SpanKind::Checkpoint => "checkpoint",
            SpanKind::Prerequisite => "prerequisite",
            SpanKind::Wait(_) => "wait",
            SpanKind::Capture => "capture",
            SpanKind::Input => "input",
            SpanKind::Assertion => "assertion",
            SpanKind::Oracle => "oracle",
            SpanKind::Media => "media",
            SpanKind::Grading => "grading",
        }
    }

    pub fn wait(self) -> Option<WaitKind> {
        match self {
            SpanKind::Wait(kind) => Some(kind),
            _ => None,
        }
    }

    pub fn parse(kind: &str, wait: Option<&str>) -> Result<Self, String> {
        let parsed = match kind {
            "guide" => SpanKind::Guide,
            "setup" => SpanKind::Setup,
            "boot" => SpanKind::Boot,
            "restore" => SpanKind::Restore,
            "checkpoint" => SpanKind::Checkpoint,
            "prerequisite" => SpanKind::Prerequisite,
            "wait" => SpanKind::Wait(WaitKind::parse(
                wait.ok_or("wait spans require a wait_kind")?,
            )?),
            "capture" => SpanKind::Capture,
            "input" => SpanKind::Input,
            "assertion" => SpanKind::Assertion,
            "oracle" => SpanKind::Oracle,
            "media" => SpanKind::Media,
            "grading" => SpanKind::Grading,
            other => return Err(format!("unknown span kind {other:?}")),
        };
        if parsed.wait().is_none() && wait.is_some() {
            return Err("wait_kind is only valid for wait spans".into());
        }
        Ok(parsed)
    }

    pub fn index(self) -> usize {
        match self {
            SpanKind::Guide => 0,
            SpanKind::Setup => 1,
            SpanKind::Boot => 2,
            SpanKind::Restore => 3,
            SpanKind::Checkpoint => 4,
            SpanKind::Prerequisite => 5,
            SpanKind::Wait(kind) => 6 + kind as usize,
            SpanKind::Capture => 13,
            SpanKind::Input => 14,
            SpanKind::Assertion => 15,
            SpanKind::Oracle => 16,
            SpanKind::Media => 17,
            SpanKind::Grading => 18,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DevicePhase {
    AdvanceRead,
    AdvanceWrite,
    Event,
    Deadline,
    Dependency,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineEndReason {
    QuantumOrInstructionBudget,
    DeviceDeadline,
    WatchOrObserverStop,
    OtherExit,
}

impl EngineEndReason {
    pub const ALL: [EngineEndReason; 4] = [
        EngineEndReason::QuantumOrInstructionBudget,
        EngineEndReason::DeviceDeadline,
        EngineEndReason::WatchOrObserverStop,
        EngineEndReason::OtherExit,
    ];

    pub fn name(self) -> &'static str {
        match self {
            EngineEndReason::QuantumOrInstructionBudget => "quantum_or_instruction_budget",
            EngineEndReason::DeviceDeadline => "device_deadline",
            EngineEndReason::WatchOrObserverStop => "watch_or_observer_stop",
            EngineEndReason::OtherExit => "other_exit",
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            EngineEndReason::QuantumOrInstructionBudget => 0,
            EngineEndReason::DeviceDeadline => 1,
            EngineEndReason::WatchOrObserverStop => 2,
            EngineEndReason::OtherExit => 3,
        }
    }
}

impl DevicePhase {
    pub const ALL: [DevicePhase; 5] = [
        DevicePhase::AdvanceRead,
        DevicePhase::AdvanceWrite,
        DevicePhase::Event,
        DevicePhase::Deadline,
        DevicePhase::Dependency,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DevicePhase::AdvanceRead => "advance-read",
            DevicePhase::AdvanceWrite => "advance-write",
            DevicePhase::Event => "event",
            DevicePhase::Deadline => "deadline",
            DevicePhase::Dependency => "dependency",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ByteCounter {
    CardFileRead,
    HostJpegDecodeIn,
    HostJpegDecodeOut,
    GuestToPythonBytes,
    PythonToGuestBytes,
    PngEncodeOut,
    CheckpointPagesCopied,
    HashedBytes,
}

impl ByteCounter {
    pub const ALL: [ByteCounter; 8] = [
        ByteCounter::CardFileRead,
        ByteCounter::HostJpegDecodeIn,
        ByteCounter::HostJpegDecodeOut,
        ByteCounter::GuestToPythonBytes,
        ByteCounter::PythonToGuestBytes,
        ByteCounter::PngEncodeOut,
        ByteCounter::CheckpointPagesCopied,
        ByteCounter::HashedBytes,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ByteCounter::CardFileRead => "card-file-read",
            ByteCounter::HostJpegDecodeIn => "host-jpeg-decode-in",
            ByteCounter::HostJpegDecodeOut => "host-jpeg-decode-out",
            ByteCounter::GuestToPythonBytes => "guest-to-python-bytes",
            ByteCounter::PythonToGuestBytes => "python-to-guest-bytes",
            ByteCounter::PngEncodeOut => "png-encode-out",
            ByteCounter::CheckpointPagesCopied => "checkpoint-pages-copied",
            ByteCounter::HashedBytes => "hashed-bytes",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|counter| counter.name() == value)
            .ok_or_else(|| format!("unknown byte counter {value:?}"))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GuestProgress {
    pub instructions: u64,
    pub ticks: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceContext {
    pub index: u16,
    pub phase: DevicePhase,
}

impl GuestProgress {
    pub fn delta(self, earlier: GuestProgress) -> GuestProgress {
        GuestProgress {
            instructions: self.instructions.saturating_sub(earlier.instructions),
            ticks: self.ticks.saturating_sub(earlier.ticks),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Accumulator {
    pub calls: u64,
    pub raw: u64,
    pub worker_calls: u64,
    pub worker_raw: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpanSummary {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<String>,
    pub count: u64,
    pub elapsed_ns: u64,
    pub thread_cpu_ns: u64,
    pub instructions: u64,
    pub ticks: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceSummary {
    pub device: u16,
    pub kind: String,
    pub phase: String,
    pub calls: u64,
    pub elapsed_ns: u64,
    pub worker_calls: u64,
    pub worker_elapsed_ns: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineSummary {
    pub core: u8,
    pub calls: u64,
    pub elapsed_ns: u64,
    pub worker_calls: u64,
    pub worker_elapsed_ns: u64,
    pub runs: u64,
    pub instructions: u64,
    pub end_reasons: BTreeMap<String, u64>,
    pub instruction_histogram: Vec<InstructionHistogramBucket>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InstructionHistogramBucket {
    pub lower_bound: u64,
    pub upper_bound: u64,
    pub runs: u64,
    pub instructions: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CoreCounters {
    pub core: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub delta: BTreeMap<String, Option<u64>>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MemorySummary {
    pub resident_bytes: Option<u64>,
    pub footprint_bytes: Option<u64>,
    pub process_peak_rss_bytes: Option<u64>,
    pub sampled_run_peak_rss_bytes: Option<u64>,
    pub sampled_run_peak_footprint_bytes: Option<u64>,
    pub lifetime_peak_footprint_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GuideSummary {
    pub schema: u32,
    pub guide: String,
    pub clock: String,
    pub elapsed_ns: u64,
    pub process_cpu_seconds: Option<f64>,
    pub thread_cpu_seconds: Option<f64>,
    pub spans: Vec<SpanSummary>,
    pub waits: Vec<WaitSummary>,
    pub devices: Vec<DeviceSummary>,
    pub engines: Vec<EngineSummary>,
    pub bytes: BTreeMap<String, u64>,
    pub counters: Vec<CoreCounters>,
    pub memory: MemorySummary,
}

impl GuideSummary {
    /// An empty summary for runs that started no ledger.
    pub fn empty() -> Self {
        Self {
            schema: 1,
            guide: String::new(),
            clock: "quanta-raw".into(),
            elapsed_ns: 0,
            process_cpu_seconds: None,
            thread_cpu_seconds: None,
            spans: Vec::new(),
            waits: Vec::new(),
            devices: Vec::new(),
            engines: Vec::new(),
            bytes: BTreeMap::new(),
            counters: Vec::new(),
            memory: MemorySummary::default(),
        }
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("guide summary serializes")
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct WaitSummary {
    pub description: String,
    pub predicate_evaluations: u64,
    pub continuations: u64,
    pub instructions: u64,
    pub ticks: u64,
    pub thread_cpu_ns: u64,
    pub stops: BTreeMap<String, u64>,
}
