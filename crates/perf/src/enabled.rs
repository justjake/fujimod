//! The recording implementation. Cold paths mutate globals; hot paths only
//! read two clocks and increment relaxed atomics.
use crate::types::*;
#[cfg(not(target_arch = "wasm32"))]
use fastrace::collector::{Reporter, SpanRecord};
#[cfg(not(target_arch = "wasm32"))]
use fastrace::local::{LocalParentGuard, LocalSpan};
#[cfg(not(target_arch = "wasm32"))]
use fastrace::prelude::{Span, SpanContext};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const MAX_DEVICES: usize = 128;
const MAX_CORES: usize = 16;
const PHASE_COUNT: usize = 5;
const BYTE_COUNT: usize = 8;
const HISTOGRAM_BUCKETS: usize = 65;

struct CellCounts {
    calls: AtomicU64,
    raw: AtomicU64,
    worker_calls: AtomicU64,
    worker_raw: AtomicU64,
}

impl CellCounts {
    const fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            raw: AtomicU64::new(0),
            worker_calls: AtomicU64::new(0),
            worker_raw: AtomicU64::new(0),
        }
    }

    fn reset(&self) {
        self.calls.store(0, Ordering::Relaxed);
        self.raw.store(0, Ordering::Relaxed);
        self.worker_calls.store(0, Ordering::Relaxed);
        self.worker_raw.store(0, Ordering::Relaxed);
    }

    fn read(&self) -> Accumulator {
        Accumulator {
            calls: self.calls.load(Ordering::Relaxed),
            raw: self.raw.load(Ordering::Relaxed),
            worker_calls: self.worker_calls.load(Ordering::Relaxed),
            worker_raw: self.worker_raw.load(Ordering::Relaxed),
        }
    }
}

struct SpanCounts {
    count: AtomicU64,
    raw: AtomicU64,
    thread_cpu: AtomicU64,
    instructions: AtomicU64,
    ticks: AtomicU64,
}

struct HistogramCounts {
    runs: AtomicU64,
    instructions: AtomicU64,
}

impl HistogramCounts {
    const fn new() -> Self {
        Self {
            runs: AtomicU64::new(0),
            instructions: AtomicU64::new(0),
        }
    }
}

struct EngineRunCounts {
    runs: AtomicU64,
    instructions: AtomicU64,
    end_reasons: [AtomicU64; 4],
    instruction_histogram: [HistogramCounts; HISTOGRAM_BUCKETS],
}

impl EngineRunCounts {
    const fn new() -> Self {
        Self {
            runs: AtomicU64::new(0),
            instructions: AtomicU64::new(0),
            end_reasons: [const { AtomicU64::new(0) }; 4],
            instruction_histogram: [const { HistogramCounts::new() }; HISTOGRAM_BUCKETS],
        }
    }

    fn reset(&self) {
        self.runs.store(0, Ordering::Relaxed);
        self.instructions.store(0, Ordering::Relaxed);
        for reason in &self.end_reasons {
            reason.store(0, Ordering::Relaxed);
        }
        for bucket in &self.instruction_histogram {
            bucket.runs.store(0, Ordering::Relaxed);
            bucket.instructions.store(0, Ordering::Relaxed);
        }
    }
}

fn histogram_index(instructions: u64) -> usize {
    if instructions == 0 {
        0
    } else {
        (u64::BITS - instructions.leading_zeros()) as usize
    }
}

fn histogram_bounds(index: usize) -> (u64, u64) {
    if index == 0 {
        return (0, 0);
    }
    let lower_bound = 1u64 << (index - 1);
    let upper_bound = if index == u64::BITS as usize {
        u64::MAX
    } else {
        (1u64 << index) - 1
    };
    (lower_bound, upper_bound)
}

impl SpanCounts {
    const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            raw: AtomicU64::new(0),
            thread_cpu: AtomicU64::new(0),
            instructions: AtomicU64::new(0),
            ticks: AtomicU64::new(0),
        }
    }
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static GUIDE: Mutex<Option<String>> = Mutex::new(None);
static START_RAW: AtomicU64 = AtomicU64::new(0);
static START_PROCESS: Mutex<Option<f64>> = Mutex::new(None);
static START_THREAD: Mutex<Option<f64>> = Mutex::new(None);
static SPANS: [SpanCounts; SpanKind::COUNT] =
    [const { SpanCounts::new() }; SpanKind::COUNT];
static DEVICES: [[CellCounts; PHASE_COUNT]; MAX_DEVICES] =
    [const { [const { CellCounts::new() }; PHASE_COUNT] }; MAX_DEVICES];
static ENGINES: [CellCounts; MAX_CORES] = [const { CellCounts::new() }; MAX_CORES];
static ENGINE_RUNS: [EngineRunCounts; MAX_CORES] =
    [const { EngineRunCounts::new() }; MAX_CORES];
static BYTES: [AtomicU64; BYTE_COUNT] = [const { AtomicU64::new(0) }; BYTE_COUNT];
static DEVICE_WORK: [AtomicU64; DeviceWorkCounter::COUNT] =
    [const { AtomicU64::new(0) }; DeviceWorkCounter::COUNT];
static WAITS: Mutex<WaitLedger> = Mutex::new(WaitLedger::new());

struct WaitLedger {
    descriptions: BTreeMap<String, usize>,
    waits: Vec<WaitCounts>,
}

impl WaitLedger {
    const fn new() -> Self {
        Self { descriptions: BTreeMap::new(), waits: Vec::new() }
    }
}

#[derive(Default)]
struct WaitCounts {
    description: String,
    predicate_evaluations: u64,
    continuations: u64,
    instructions: u64,
    ticks: u64,
    thread_cpu_ns: u64,
    stops: BTreeMap<String, u64>,
    device_stops: BTreeMap<String, u64>,
}
static DEVICE_NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static COUNTER_BASELINE: Mutex<Option<Vec<Result<BTreeMap<String, u64>, String>>>> =
    Mutex::new(None);
static COUNTERS: Mutex<Vec<Result<BTreeMap<String, u64>, String>>> = Mutex::new(Vec::new());
static RESIDENT: AtomicU64 = AtomicU64::new(0);
static FOOTPRINT: AtomicU64 = AtomicU64::new(0);
static SAMPLED_PEAK_RSS: AtomicU64 = AtomicU64::new(0);
static SAMPLED_PEAK_FOOTPRINT: AtomicU64 = AtomicU64::new(0);
static LIFETIME_FOOTPRINT: AtomicU64 = AtomicU64::new(0);
#[cfg(not(target_arch = "wasm32"))]
static ROOT: Mutex<Option<Span>> = Mutex::new(None);
#[cfg(not(target_arch = "wasm32"))]
static REPORTS: Mutex<Vec<SpanRecord>> = Mutex::new(Vec::new());
#[cfg(not(target_arch = "wasm32"))]
static LAST_TRACE: Mutex<Vec<SpanRecord>> = Mutex::new(Vec::new());

thread_local! {
    #[cfg(not(target_arch = "wasm32"))]
    static LOCAL_PARENT: RefCell<Option<LocalParentGuard>> = const { RefCell::new(None) };
    static IS_WORKER: Cell<bool> = const { Cell::new(false) };
    static DEVICE_CONTEXT: Cell<Option<DeviceContext>> = const { Cell::new(None) };
    static PENDING_SPANS: RefCell<Vec<PendingSpan>> = const { RefCell::new(Vec::new()) };
}

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SpanHandle;

#[cfg(not(target_arch = "wasm32"))]
type SpanHandle = LocalSpan;

fn local_span(name: String) -> SpanHandle {
    #[cfg(not(target_arch = "wasm32"))]
    {
        LocalSpan::enter_with_local_parent(name)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        SpanHandle
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clock() -> &'static quanta::Clock {
    static CLOCK: OnceLock<quanta::Clock> = OnceLock::new();
    CLOCK.get_or_init(quanta::Clock::new)
}

#[cfg(not(target_arch = "wasm32"))]
fn raw_now() -> u64 {
    clock().raw()
}

#[cfg(not(target_arch = "wasm32"))]
fn scale(raw: u64) -> u64 {
    if raw == 0 {
        0
    } else {
        clock().delta_as_nanos(0, raw)
    }
}

#[cfg(target_arch = "wasm32")]
fn raw_now() -> u64 {
    // Emscripten's monotonic browser clock, already in nanoseconds.
    unsafe { (emscripten_get_now() * 1_000_000.0) as u64 }
}

#[cfg(target_arch = "wasm32")]
fn scale(raw: u64) -> u64 {
    raw
}

#[cfg(target_arch = "wasm32")]
unsafe extern "C" {
    fn emscripten_get_now() -> f64;
}

fn elapsed_raw(start: u64) -> u64 {
    raw_now().saturating_sub(start)
}

fn is_worker() -> bool {
    IS_WORKER.with(Cell::get)
}

/// Mark the calling thread as device or raster worker.
pub fn worker_thread() {
    IS_WORKER.with(|worker| worker.set(true));
}

pub fn capture_context() -> Option<DeviceContext> {
    DEVICE_CONTEXT.with(Cell::get)
}

/// Run one device or raster job, attributing its worker time to the device
/// accumulator active when the job was submitted.
pub fn run_on_worker<R>(context: Option<DeviceContext>, job: impl FnOnce() -> R) -> R {
    worker_thread();
    let previous = DEVICE_CONTEXT.with(|slot| slot.replace(context));
    let started = context.filter(|_| ACTIVE.load(Ordering::Relaxed)).map(|_| raw_now());
    let result = job();
    if let (Some(context), Some(started)) = (context, started) {
        let elapsed = elapsed_raw(started);
        let cell = &DEVICES[context.index as usize][context.phase as usize];
        cell.worker_calls.fetch_add(1, Ordering::Relaxed);
        cell.worker_raw.fetch_add(elapsed, Ordering::Relaxed);
    }
    DEVICE_CONTEXT.with(|slot| slot.set(previous));
    result
}

/// Register one board's device kinds once, at construction.
pub fn register_devices(names: &[&'static str]) {
    let mut registered = DEVICE_NAMES.lock().unwrap_or_else(|e| e.into_inner());
    registered.clear();
    registered.extend(names.iter().take(MAX_DEVICES).map(|name| (*name).to_owned()));
}

#[cfg(not(target_arch = "wasm32"))]
struct TraceReporter;

#[cfg(not(target_arch = "wasm32"))]
impl Reporter for TraceReporter {
    fn report(&mut self, spans: Vec<SpanRecord>) {
        REPORTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(spans);
    }
}

fn reset_ledger() {
    *WAITS.lock().unwrap_or_else(|e| e.into_inner()) = WaitLedger::new();
    for span in &SPANS {
        span.count.store(0, Ordering::Relaxed);
        span.raw.store(0, Ordering::Relaxed);
        span.thread_cpu.store(0, Ordering::Relaxed);
        span.instructions.store(0, Ordering::Relaxed);
        span.ticks.store(0, Ordering::Relaxed);
    }
    for device in &DEVICES {
        for cell in device {
            cell.reset();
        }
    }
    for engine in &ENGINES {
        engine.reset();
    }
    for engine in &ENGINE_RUNS {
        engine.reset();
    }
    for counter in &BYTES {
        counter.store(0, Ordering::Relaxed);
    }
    for counter in &DEVICE_WORK {
        counter.store(0, Ordering::Relaxed);
    }
    RESIDENT.store(0, Ordering::Relaxed);
    FOOTPRINT.store(0, Ordering::Relaxed);
    SAMPLED_PEAK_RSS.store(0, Ordering::Relaxed);
    SAMPLED_PEAK_FOOTPRINT.store(0, Ordering::Relaxed);
    LIFETIME_FOOTPRINT.store(0, Ordering::Relaxed);
    *COUNTER_BASELINE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    COUNTERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

#[cfg(not(target_arch = "wasm32"))]
fn install_reporter() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        fastrace::set_reporter(TraceReporter, fastrace::collector::Config::default());
    });
}

#[cfg(target_arch = "wasm32")]
fn install_reporter() {}

pub fn begin_guide(id: &str) {
    reset_ledger();
    *GUIDE.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_owned());
    START_RAW.store(raw_now(), Ordering::Relaxed);
    *START_PROCESS.lock().unwrap_or_else(|e| e.into_inner()) = process_cpu_seconds();
    *START_THREAD.lock().unwrap_or_else(|e| e.into_inner()) = thread_cpu_seconds();
    ACTIVE.store(true, Ordering::Relaxed);
    sample_memory();
    install_reporter();
    #[cfg(not(target_arch = "wasm32"))]
    {
        REPORTS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let root = Span::root(id.to_owned(), SpanContext::random());
        let guard = root.set_local_parent();
        *ROOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(root);
        LOCAL_PARENT.with(|slot| {
            *slot.borrow_mut() = Some(guard);
        });
    }
    begin_span(SpanKind::Guide, GuestProgress::default(), None);
}

/// Count a predicate evaluation for an interned wait description.
pub fn begin_wait(description: &str) -> usize {
    if !ACTIVE.load(Ordering::Relaxed) {
        return usize::MAX;
    }
    let mut ledger = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = ledger.descriptions.get(description) {
        return *index;
    }
    let index = ledger.waits.len();
    ledger.waits.push(WaitCounts {
        description: description.to_owned(),
        ..WaitCounts::default()
    });
    ledger.descriptions.insert(description.to_owned(), index);
    index
}

pub fn record_wait_evaluation(index: usize) {
    if !ACTIVE.load(Ordering::Relaxed) || index == usize::MAX { return; }
    let mut ledger = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(wait) = ledger.waits.get_mut(index) {
        wait.predicate_evaluations += 1;
    }
}

pub fn record_wait_stop(index: usize, cause: &str, device: Option<&str>) {
    if !ACTIVE.load(Ordering::Relaxed) || index == usize::MAX { return; }
    let mut ledger = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(wait) = ledger.waits.get_mut(index) else { return; };
    wait.continuations += 1;
    if let Some(device) = device {
        if let Some(count) = wait.device_stops.get_mut(device) {
            *count += 1;
        } else {
            wait.device_stops.insert(device.to_owned(), 1);
        }
    } else {
        if let Some(count) = wait.stops.get_mut(cause) {
            *count += 1;
        } else {
            wait.stops.insert(cause.to_owned(), 1);
        }
    }
}

pub fn end_guide() -> GuideSummary {
    if !ACTIVE.swap(false, Ordering::Relaxed) {
        return GuideSummary::empty();
    }
    let ended = raw_now();
    sample_memory();
    let process_cpu = (*START_PROCESS.lock().unwrap_or_else(|e| e.into_inner()))
        .zip(process_cpu_seconds())
        .map(|(start, end)| (end - start).max(0.0));
    let thread_cpu = (*START_THREAD.lock().unwrap_or_else(|e| e.into_inner()))
        .zip(thread_cpu_seconds())
        .map(|(start, end)| (end - start).max(0.0));
    let guide = GUIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap_or_default();
    let spans = (0..SpanKind::COUNT)
        .filter_map(|index| {
            let counts = &SPANS[index];
            let count = counts.count.load(Ordering::Relaxed);
            if count == 0 {
                return None;
            }
            let (kind, wait) = span_labels(index);
            Some(SpanSummary {
                kind: kind.to_owned(),
                wait: wait.map(str::to_owned),
                count,
                elapsed_ns: scale(counts.raw.load(Ordering::Relaxed)),
                thread_cpu_ns: counts.thread_cpu.load(Ordering::Relaxed),
                instructions: counts.instructions.load(Ordering::Relaxed),
                ticks: counts.ticks.load(Ordering::Relaxed),
            })
        })
        .collect();
    let waits = WAITS.lock().unwrap_or_else(|e| e.into_inner()).waits.iter()
        .map(|counts| {
            let mut stops = counts.stops.clone();
            for (device, count) in &counts.device_stops {
                stops.insert(format!("device:{device}"), *count);
            }
            WaitSummary {
                description: counts.description.clone(),
                predicate_evaluations: counts.predicate_evaluations,
                continuations: counts.continuations,
                instructions: counts.instructions,
                ticks: counts.ticks,
                thread_cpu_ns: counts.thread_cpu_ns,
                stops,
            }
        }).collect();
    let names = DEVICE_NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let mut devices = Vec::new();
    for (index, phases) in DEVICES.iter().enumerate() {
        for (phase, cell) in phases.iter().enumerate() {
            let values = cell.read();
            if values.calls == 0 && values.worker_calls == 0 {
                continue;
            }
            devices.push(DeviceSummary {
                device: index as u16,
                kind: names
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("device-{index}")),
                phase: DevicePhase::ALL[phase].name().to_owned(),
                calls: values.calls,
                elapsed_ns: scale(values.raw),
                worker_calls: values.worker_calls,
                worker_elapsed_ns: scale(values.worker_raw),
            });
        }
    }
    drop(names);
    let engines = ENGINES
        .iter()
        .enumerate()
        .filter_map(|(core, cell)| {
            let values = cell.read();
            let run_counts = &ENGINE_RUNS[core];
            let runs = run_counts.runs.load(Ordering::Relaxed);
            (values.calls != 0 || values.worker_calls != 0 || runs != 0).then(|| {
                let end_reasons = EngineEndReason::ALL
                    .into_iter()
                    .map(|reason| {
                        (
                            reason.name().to_owned(),
                            run_counts.end_reasons[reason.index()].load(Ordering::Relaxed),
                        )
                    })
                    .collect();
                let instruction_histogram = run_counts
                    .instruction_histogram
                    .iter()
                    .enumerate()
                    .filter_map(|(index, bucket)| {
                        let runs = bucket.runs.load(Ordering::Relaxed);
                        (runs != 0).then(|| {
                            let (lower_bound, upper_bound) = histogram_bounds(index);
                            InstructionHistogramBucket {
                                lower_bound,
                                upper_bound,
                                runs,
                                instructions: bucket.instructions.load(Ordering::Relaxed),
                            }
                        })
                    })
                    .collect();
                EngineSummary {
                    core: core as u8,
                    calls: values.calls,
                    elapsed_ns: scale(values.raw),
                    worker_calls: values.worker_calls,
                    worker_elapsed_ns: scale(values.worker_raw),
                    runs,
                    instructions: run_counts.instructions.load(Ordering::Relaxed),
                    end_reasons,
                    instruction_histogram,
                }
            })
        })
        .collect();
    let bytes = ByteCounter::ALL
        .iter()
        .zip(BYTES.iter())
        .map(|(counter, value)| {
            (
                counter.name().to_owned(),
                value.load(Ordering::Relaxed),
            )
        })
        .collect();
    let device_work = DeviceWorkCounter::ALL
        .iter()
        .map(|counter| {
            (
                counter.name().to_owned(),
                DEVICE_WORK[counter.index()].load(Ordering::Relaxed),
            )
        })
        .collect();
    let counters = counter_deltas();
    let memory = memory_summary();

    #[cfg(not(target_arch = "wasm32"))]
    {
        let root = ROOT.lock().unwrap_or_else(|e| e.into_inner()).take();
        LOCAL_PARENT.with(|slot| {
            slot.borrow_mut().take();
        });
        drop(root);
        fastrace::flush();
        *LAST_TRACE.lock().unwrap_or_else(|e| e.into_inner()) =
            std::mem::take(&mut *REPORTS.lock().unwrap_or_else(|e| e.into_inner()));
    }

    GuideSummary {
        schema: 1,
        guide,
        clock: "quanta-raw".into(),
        elapsed_ns: scale(ended.saturating_sub(START_RAW.load(Ordering::Relaxed))),
        process_cpu_seconds: process_cpu,
        thread_cpu_seconds: thread_cpu,
        spans,
        waits,
        devices,
        engines,
        bytes,
        device_work,
        counters,
        memory,
    }
}

fn span_labels(index: usize) -> (&'static str, Option<&'static str>) {
    let kind = match index {
        0 => SpanKind::Guide,
        1 => SpanKind::Setup,
        2 => SpanKind::Boot,
        3 => SpanKind::Restore,
        4 => SpanKind::Checkpoint,
        5 => SpanKind::Prerequisite,
        6..=12 => SpanKind::Wait(WaitKind::ALL[index - 6]),
        13 => SpanKind::Capture,
        14 => SpanKind::Input,
        15 => SpanKind::Assertion,
        16 => SpanKind::Oracle,
        17 => SpanKind::Media,
        _ => SpanKind::Grading,
    };
    (kind.name(), kind.wait().map(WaitKind::name))
}

fn counter_deltas() -> Vec<CoreCounters> {
    let baseline = COUNTER_BASELINE.lock().unwrap_or_else(|e| e.into_inner());
    let current = COUNTERS.lock().unwrap_or_else(|e| e.into_inner());
    current
        .iter()
        .enumerate()
        .map(|(core, values)| match values {
            Ok(values) => {
                let before = baseline
                    .as_ref()
                    .and_then(|baseline| baseline.get(core))
                    .and_then(|value| value.as_ref().ok());
                CoreCounters {
                    core,
                    delta: values
                        .iter()
                        .map(|(name, value)| {
                            let delta = before
                                .and_then(|before| before.get(name))
                                .and_then(|before| value.checked_sub(*before));
                            (name.clone(), delta)
                        })
                        .collect(),
                    error: None,
                }
            }
            Err(error) => CoreCounters {
                core,
                delta: BTreeMap::new(),
                error: Some(error.clone()),
            },
        })
        .collect()
}

/// Snapshot existing CPU and R-profile exit counters. The first snapshot after
/// `begin_guide` is the baseline; later snapshots are the guide's end value.
pub fn set_cpu_counters(counters: Vec<Result<BTreeMap<String, u64>, String>>) {
    let converted = counters
        .into_iter()
        .map(|values| values.map(|values| values.into_iter().collect()))
        .collect::<Vec<_>>();
    let mut baseline = COUNTER_BASELINE.lock().unwrap_or_else(|e| e.into_inner());
    if baseline.is_none() {
        *baseline = Some(converted.clone());
    }
    drop(baseline);
    *COUNTERS.lock().unwrap_or_else(|e| e.into_inner()) = converted;
}

pub struct SpanGuard {
    index: usize,
    start_raw: u64,
    start_thread_ns: u64,
    guest: Option<GuestProgress>,
    span: Option<SpanHandle>,
}

impl SpanGuard {
    pub fn guest(&mut self, start: GuestProgress, end: GuestProgress) {
        self.guest = Some(end.delta(start));
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if self.start_raw == 0 {
            return;
        }
        let elapsed = elapsed_raw(self.start_raw);
        let thread_cpu_ns = thread_cpu_ns_now().saturating_sub(self.start_thread_ns);
        let guest = self.guest.unwrap_or_default();
        #[cfg(not(target_arch = "wasm32"))]
        if self.span.is_some() {
            LocalSpan::add_properties(|| [("thread_cpu_ns".to_owned(), thread_cpu_ns.to_string())]);
        }
        record_span(self.index, elapsed, thread_cpu_ns, guest);
        self.span.take();
    }
}

pub fn span(kind: SpanKind) -> SpanGuard {
    if !ACTIVE.load(Ordering::Relaxed) {
        return SpanGuard {
            index: kind.index(),
            start_raw: 0,
            start_thread_ns: 0,
            guest: None,
            span: None,
        };
    }
    let local = local_span(kind.name().to_owned());
    SpanGuard {
        index: kind.index(),
        start_raw: raw_now(),
        start_thread_ns: thread_cpu_ns_now(),
        guest: None,
        span: Some(local),
    }
}

struct PendingSpan {
    index: usize,
    wait_counter: Option<usize>,
    start_raw: u64,
    start_thread_ns: u64,
    guest_start: GuestProgress,
    span: SpanHandle,
}

pub fn begin_span(kind: SpanKind, guest: GuestProgress, wait_counter: Option<usize>) -> bool {
    if !ACTIVE.load(Ordering::Relaxed) {
        return false;
    }
    debug_assert!(wait_counter.is_none() || kind.wait().is_some());
    let name = match kind.wait() {
        Some(wait) => format!("wait.{}", wait.name()),
        None => kind.name().to_owned(),
    };
    let span = local_span(name);
    PENDING_SPANS.with(|pending| {
        pending.borrow_mut().push(PendingSpan {
            index: kind.index(),
            wait_counter,
            start_raw: raw_now(),
            start_thread_ns: thread_cpu_ns_now(),
            guest_start: guest,
            span,
        });
    });
    true
}

/// Close the innermost span opened by `begin_span`. Optional integer
/// properties become fastrace properties for the trace only.
pub fn end_span(guest: GuestProgress, properties: &[(&str, u64)]) {
    let pending = PENDING_SPANS.with(|pending| pending.borrow_mut().pop());
    let Some(pending) = pending else {
        return;
    };
    let _ = &pending.span;
    let thread_cpu_ns = thread_cpu_ns_now().saturating_sub(pending.start_thread_ns);
    #[cfg(not(target_arch = "wasm32"))]
    LocalSpan::add_properties(|| {
        properties
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.to_string()))
            .chain(std::iter::once((
                "thread_cpu_ns".to_owned(),
                thread_cpu_ns.to_string(),
            )))
    });
    let elapsed = elapsed_raw(pending.start_raw);
    let guest = guest.delta(pending.guest_start);
    record_span(pending.index, elapsed, thread_cpu_ns, guest);
    if let Some(index) = pending.wait_counter {
        record_wait_span(index, guest, thread_cpu_ns);
    }
    drop(pending);
}

fn record_wait_span(index: usize, guest: GuestProgress, thread_cpu_ns: u64) {
    if !ACTIVE.load(Ordering::Relaxed) || index == usize::MAX {
        return;
    }
    let mut ledger = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(wait) = ledger.waits.get_mut(index) else {
        return;
    };
    wait.instructions = wait.instructions.saturating_add(guest.instructions);
    wait.ticks = wait.ticks.saturating_add(guest.ticks);
    wait.thread_cpu_ns = wait.thread_cpu_ns.saturating_add(thread_cpu_ns);
}

fn record_span(index: usize, elapsed_raw: u64, thread_cpu_ns: u64, guest: GuestProgress) {
    let counts = &SPANS[index];
    counts.count.fetch_add(1, Ordering::Relaxed);
    counts.raw.fetch_add(elapsed_raw, Ordering::Relaxed);
    counts
        .thread_cpu
        .fetch_add(thread_cpu_ns, Ordering::Relaxed);
    counts
        .instructions
        .fetch_add(guest.instructions, Ordering::Relaxed);
    counts.ticks.fetch_add(guest.ticks, Ordering::Relaxed);
    if matches!(index, 0 | 2 | 3 | 16) {
        sample_memory();
    }
}

pub struct AccumulatorGuard {
    cell: Option<&'static CellCounts>,
    start_raw: u64,
    worker: bool,
    previous: Option<DeviceContext>,
}

pub struct EngineRunGuard {
    core: usize,
    start_raw: u64,
    elapsed_raw: Option<u64>,
    instructions: u64,
    worker: bool,
    recording: bool,
    finished: bool,
}

impl EngineRunGuard {
    pub fn stop(&mut self) {
        if self.recording && self.elapsed_raw.is_none() {
            self.elapsed_raw = Some(elapsed_raw(self.start_raw));
        }
    }

    pub fn set_instructions(&mut self, instructions: u64) {
        self.instructions = instructions;
    }

    pub fn finish(mut self, reason: EngineEndReason) {
        self.record(reason);
    }

    pub fn cancel(mut self) {
        self.finished = true;
    }

    fn record(&mut self, reason: EngineEndReason) {
        if self.recording && !self.finished {
            self.stop();
            let elapsed = self.elapsed_raw.unwrap_or(0);
            let cell = &ENGINES[self.core];
            if self.worker {
                cell.worker_calls.fetch_add(1, Ordering::Relaxed);
                cell.worker_raw.fetch_add(elapsed, Ordering::Relaxed);
            } else {
                cell.calls.fetch_add(1, Ordering::Relaxed);
                cell.raw.fetch_add(elapsed, Ordering::Relaxed);
            }

            let counts = &ENGINE_RUNS[self.core];
            counts.runs.fetch_add(1, Ordering::Relaxed);
            counts.instructions.fetch_add(self.instructions, Ordering::Relaxed);
            counts.end_reasons[reason.index()].fetch_add(1, Ordering::Relaxed);
            let bucket = &counts.instruction_histogram[histogram_index(self.instructions)];
            bucket.runs.fetch_add(1, Ordering::Relaxed);
            bucket.instructions.fetch_add(self.instructions, Ordering::Relaxed);
        }
        self.finished = true;
    }
}

impl Drop for EngineRunGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.record(EngineEndReason::OtherExit);
        }
    }
}

impl Drop for AccumulatorGuard {
    fn drop(&mut self) {
        if let Some(cell) = self.cell {
            let elapsed = elapsed_raw(self.start_raw);
            if self.worker {
                cell.worker_calls.fetch_add(1, Ordering::Relaxed);
                cell.worker_raw.fetch_add(elapsed, Ordering::Relaxed);
            } else {
                cell.calls.fetch_add(1, Ordering::Relaxed);
                cell.raw.fetch_add(elapsed, Ordering::Relaxed);
            }
        }
        if self.previous.is_some() || self.cell.is_some() {
            DEVICE_CONTEXT.with(|slot| slot.set(self.previous));
        }
    }
}

fn accumulator(cell: &'static CellCounts, context: Option<DeviceContext>) -> AccumulatorGuard {
    if !ACTIVE.load(Ordering::Relaxed) {
        return AccumulatorGuard {
            cell: None,
            start_raw: 0,
            worker: false,
            previous: None,
        };
    }
    let worker = is_worker();
    let covered = worker && context.is_some_and(|ctx| capture_context() == Some(ctx));
    let previous = context.map(|ctx| DEVICE_CONTEXT.with(|slot| slot.replace(Some(ctx))));
    AccumulatorGuard {
        cell: (!covered).then_some(cell),
        start_raw: raw_now(),
        worker,
        previous: previous.flatten(),
    }
}

pub fn device(index: u16, _kind: &'static str, phase: DevicePhase) -> AccumulatorGuard {
    if index as usize >= MAX_DEVICES {
        return AccumulatorGuard {
            cell: None,
            start_raw: 0,
            worker: false,
            previous: None,
        };
    }
    let context = DeviceContext { index, phase };
    accumulator(
        &DEVICES[index as usize][phase as usize],
        Some(context),
    )
}

pub fn engine_run(core: u8, worker: bool) -> EngineRunGuard {
    let core = core as usize;
    let recording = ACTIVE.load(Ordering::Relaxed) && core < MAX_CORES;
    EngineRunGuard {
        core: core.min(MAX_CORES - 1),
        start_raw: if recording { raw_now() } else { 0 },
        elapsed_raw: None,
        instructions: 0,
        worker,
        recording,
        finished: !recording,
    }
}

pub fn add_bytes(counter: ByteCounter, bytes: u64) {
    if ACTIVE.load(Ordering::Relaxed) && bytes != 0 {
        BYTES[counter as usize].fetch_add(bytes, Ordering::Relaxed);
    }
}

pub fn add_work(counter: DeviceWorkCounter, value: u64) {
    if value != 0 && ACTIVE.load(Ordering::Relaxed) {
        DEVICE_WORK[counter.index()].fetch_add(value, Ordering::Relaxed);
    }
}

pub struct WorkTimer {
    counter: DeviceWorkCounter,
    started: Option<u64>,
}

pub fn work_timer(counter: DeviceWorkCounter) -> WorkTimer {
    WorkTimer {
        counter,
        started: ACTIVE.load(Ordering::Relaxed).then(raw_now),
    }
}

impl Drop for WorkTimer {
    fn drop(&mut self) {
        if let Some(started) = self.started
            && ACTIVE.load(Ordering::Relaxed)
        {
            DEVICE_WORK[self.counter.index()]
                .fetch_add(scale(elapsed_raw(started)), Ordering::Relaxed);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn write_chrome_trace(path: &Path) -> io::Result<()> {
    fastrace::flush();
    let records = LAST_TRACE.lock().unwrap_or_else(|e| e.into_inner());
    let events = records
        .iter()
        .map(|record| {
            let mut args = serde_json::Map::new();
            for (key, value) in &record.properties {
                let key = key.clone().into_owned();
                let value = value.clone().into_owned();
                let value = if key == "thread_cpu_ns" {
                    value
                        .parse::<u64>()
                        .map(serde_json::Value::from)
                        .unwrap_or_else(|_| value.into())
                } else {
                    value.into()
                };
                args.insert(key, value);
            }
            args.insert(
                "span_id".into(),
                format!("{:x}", record.span_id.0).into(),
            );
            args.insert(
                "parent_id".into(),
                format!("{:x}", record.parent_id.0).into(),
            );
            serde_json::json!({
                "name": record.name,
                "cat": "fujimod",
                "ph": "X",
                "ts": record.begin_time_unix_ns / 1_000,
                "dur": record.duration_ns / 1_000,
                "pid": 1,
                "tid": 1,
                "args": args,
            })
        })
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "displayTimeUnit": "ns",
        "traceEvents": events,
    });
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec(&payload).map_err(io::Error::other)?,
    )?;
    std::fs::rename(temporary, path)
}

#[cfg(target_arch = "wasm32")]
pub fn write_chrome_trace(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        b"{\"displayTimeUnit\":\"ns\",\"traceEvents\":[]}",
    )
}

fn memory_summary() -> MemorySummary {
    let fields = sample_memory();
    MemorySummary {
        resident_bytes: fields.resident,
        footprint_bytes: fields.footprint,
        process_peak_rss_bytes: peak_rss(),
        sampled_run_peak_rss_bytes: (SAMPLED_PEAK_RSS.load(Ordering::Relaxed) != 0)
            .then(|| SAMPLED_PEAK_RSS.load(Ordering::Relaxed)),
        sampled_run_peak_footprint_bytes: (SAMPLED_PEAK_FOOTPRINT.load(Ordering::Relaxed) != 0)
            .then(|| SAMPLED_PEAK_FOOTPRINT.load(Ordering::Relaxed)),
        lifetime_peak_footprint_bytes: fields.lifetime_footprint,
    }
}

#[derive(Default)]
struct MemoryFields {
    resident: Option<u64>,
    footprint: Option<u64>,
    lifetime_footprint: Option<u64>,
}

fn sample_memory() -> MemoryFields {
    let fields = platform_memory();
    if let Some(resident) = fields.resident {
        RESIDENT.store(resident, Ordering::Relaxed);
        SAMPLED_PEAK_RSS.fetch_max(resident, Ordering::Relaxed);
    }
    if let Some(footprint) = fields.footprint {
        FOOTPRINT.store(footprint, Ordering::Relaxed);
        SAMPLED_PEAK_FOOTPRINT.fetch_max(footprint, Ordering::Relaxed);
    }
    if let Some(lifetime) = fields.lifetime_footprint {
        LIFETIME_FOOTPRINT.fetch_max(lifetime, Ordering::Relaxed);
    }
    fields
}

#[cfg(target_os = "macos")]
fn platform_memory() -> MemoryFields {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
        )
    };
    if result != 0 {
        return MemoryFields::default();
    }
    MemoryFields {
        resident: Some(info.ri_resident_size),
        footprint: Some(info.ri_phys_footprint),
        lifetime_footprint: Some(info.ri_lifetime_max_phys_footprint),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_memory() -> MemoryFields {
    let resident = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|contents| {
            contents
                .split_whitespace()
                .nth(1)
                .and_then(|pages| pages.parse::<u64>().ok())
        })
        .map(|pages| pages * page_size());
    MemoryFields {
        resident,
        footprint: None,
        lifetime_footprint: None,
    }
}

#[cfg(not(unix))]
fn platform_memory() -> MemoryFields {
    MemoryFields::default()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn page_size() -> u64 {
    static SIZE: OnceLock<u64> = OnceLock::new();
    *SIZE.get_or_init(|| unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 })
}

#[cfg(unix)]
fn peak_rss() -> Option<u64> {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    let bytes = usage.ru_maxrss as u64;
    #[cfg(not(target_os = "macos"))]
    let bytes = (usage.ru_maxrss as u64) * 1024;
    Some(bytes)
}

#[cfg(not(unix))]
fn peak_rss() -> Option<u64> {
    None
}

#[cfg(unix)]
fn thread_cpu_seconds() -> Option<f64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0 {
        return None;
    }
    Some(time.tv_sec as f64 + time.tv_nsec as f64 / 1e9)
}

#[cfg(unix)]
fn thread_cpu_ns_now() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0 {
        return 0;
    }
    (time.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(time.tv_nsec as u64)
}

#[cfg(not(unix))]
fn thread_cpu_seconds() -> Option<f64> {
    None
}

#[cfg(not(unix))]
fn thread_cpu_ns_now() -> u64 {
    0
}

#[cfg(unix)]
fn process_cpu_seconds() -> Option<f64> {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1e6;
    Some(seconds(usage.ru_utime) + seconds(usage.ru_stime))
}

#[cfg(not(unix))]
fn process_cpu_seconds() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as TestMutex;

    static TEST_LOCK: TestMutex<()> = TestMutex::new(());

    #[test]
    fn spans_nest_and_record_per_kind_guest_deltas() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_guide("spans");
        let mut outer = span(SpanKind::Guide);
        {
            let mut wait = span(SpanKind::Wait(WaitKind::Debounce));
            wait.guest(
                GuestProgress {
                    instructions: 10,
                    ticks: 100,
                },
                GuestProgress {
                    instructions: 35,
                    ticks: 160,
                },
            );
        }
        outer.guest(
            GuestProgress {
                instructions: 0,
                ticks: 0,
            },
            GuestProgress {
                instructions: 40,
                ticks: 200,
            },
        );
        drop(outer);
        let summary = end_guide();
        let guide = summary
            .spans
            .iter()
            .find(|span| span.kind == "guide")
            .unwrap();
        assert_eq!(guide.count, 1);
        assert_eq!((guide.instructions, guide.ticks), (40, 200));
        assert!(guide.thread_cpu_ns > 0);
        let wait = summary
            .spans
            .iter()
            .find(|span| span.kind == "wait")
            .unwrap();
        assert_eq!(wait.wait.as_deref(), Some("debounce"));
        assert_eq!(wait.count, 1);
        assert_eq!((wait.instructions, wait.ticks), (25, 60));
        assert!(wait.thread_cpu_ns > 0);
    }

    #[test]
    fn wait_counters_are_keyed_by_description_and_stop_cause() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_guide("wait-counters");
        let display = begin_wait("display frame");
        record_wait_evaluation(display);
        record_wait_evaluation(display);
        record_wait_stop(display, "device-publication", Some("display-timing"));
        record_wait_stop(display, "deadline", None);
        let start = GuestProgress {
            instructions: 10,
            ticks: 20,
        };
        assert!(begin_span(
            SpanKind::Wait(WaitKind::OsdFrame),
            start,
            Some(display),
        ));
        end_span(
            GuestProgress {
                instructions: 30,
                ticks: 70,
            },
            &[],
        );
        assert!(begin_span(
            SpanKind::Wait(WaitKind::OsdFrame),
            GuestProgress {
                instructions: 30,
                ticks: 70,
            },
            Some(display),
        ));
        end_span(
            GuestProgress {
                instructions: 35,
                ticks: 80,
            },
            &[],
        );
        let menu = begin_wait("menu ready");
        record_wait_evaluation(menu);
        let summary = end_guide();
        let wait = summary.waits.iter().find(|wait| wait.description == "display frame").unwrap();
        assert_eq!(wait.predicate_evaluations, 2);
        assert_eq!(wait.continuations, 2);
        assert_eq!(wait.stops.get("device:display-timing"), Some(&1));
        assert_eq!(wait.stops.get("deadline"), Some(&1));
        assert_eq!((wait.instructions, wait.ticks), (25, 60));
        let wait_span = summary
            .spans
            .iter()
            .find(|span| span.kind == "wait" && span.wait.as_deref() == Some("osd_frame"))
            .unwrap();
        assert_eq!(
            (wait.instructions, wait.ticks),
            (wait_span.instructions, wait_span.ticks)
        );
        assert_eq!(wait.thread_cpu_ns, wait_span.thread_cpu_ns);
        assert_eq!(summary.waits.iter().find(|wait| wait.description == "menu ready").unwrap().predicate_evaluations, 1);
    }

    #[test]
    fn python_spans_close_in_lifo_order_with_properties() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_guide("python");
        let start = GuestProgress {
            instructions: 7,
            ticks: 9,
        };
        assert!(begin_span(SpanKind::Wait(WaitKind::OsdFrame), start, None));
        assert!(begin_span(SpanKind::Capture, start, None));
        end_span(
            GuestProgress {
                instructions: 9,
                ticks: 11,
            },
            &[("polls", 3)],
        );
        end_span(
            GuestProgress {
                instructions: 15,
                ticks: 21,
            },
            &[("instructions", 8), ("polls", 4)],
        );
        let summary = end_guide();
        assert_eq!(
            summary
                .spans
                .iter()
                .find(|span| span.kind == "capture")
                .unwrap()
                .instructions,
            2
        );
        assert_eq!(
            summary
                .spans
                .iter()
                .find(|span| span.kind == "wait")
                .unwrap()
                .ticks,
            12
        );
    }

    #[test]
    fn accumulators_are_per_device_phase_and_engine() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_guide("accumulators");
        {
            let _read = device(3, "sd", DevicePhase::AdvanceRead);
        }
        {
            let _event = device(3, "sd", DevicePhase::Event);
        }
        {
            let mut run = engine_run(1, false);
            run.set_instructions(4);
            run.finish(EngineEndReason::QuantumOrInstructionBudget);
        }
        let summary = end_guide();
        assert_eq!(summary.devices.len(), 2);
        let read = summary
            .devices
            .iter()
            .find(|row| row.phase == "advance-read")
            .unwrap();
        assert_eq!((read.device, read.calls), (3, 1));
        assert_eq!(summary.engines[0].core, 1);
        assert_eq!(summary.engines[0].calls, 1);
        assert_eq!(summary.engines[0].runs, 1);
        assert_eq!(summary.engines[0].instructions, 4);
        assert_eq!(summary.engines[0].instruction_histogram[0].lower_bound, 4);
        assert_eq!(summary.engines[0].instruction_histogram[0].upper_bound, 7);
    }

    #[test]
    fn engine_run_histogram_and_end_reasons_cover_each_core_run() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_guide("engine-runs");
        for (instructions, reason, worker) in [
            (0, EngineEndReason::OtherExit, false),
            (1, EngineEndReason::QuantumOrInstructionBudget, false),
            (3, EngineEndReason::DeviceDeadline, true),
            (4, EngineEndReason::WatchOrObserverStop, true),
        ] {
            let mut run = engine_run(0, worker);
            run.set_instructions(instructions);
            run.finish(reason);
        }

        let value = end_guide().to_value();
        let engine = &value["engines"][0];
        assert_eq!(engine["core"], 0);
        assert_eq!(engine["calls"], 2);
        assert_eq!(engine["worker_calls"], 2);
        assert_eq!(engine["runs"], 4);
        assert_eq!(engine["instructions"], 8);
        assert_eq!(engine["end_reasons"]["other_exit"], 1);
        assert_eq!(engine["end_reasons"]["quantum_or_instruction_budget"], 1);
        assert_eq!(engine["end_reasons"]["device_deadline"], 1);
        assert_eq!(engine["end_reasons"]["watch_or_observer_stop"], 1);
        let bins = engine["instruction_histogram"].as_array().unwrap();
        assert_eq!(bins.len(), 4);
        assert_eq!(
            (
                bins[0]["lower_bound"].as_u64(),
                bins[0]["upper_bound"].as_u64()
            ),
            (Some(0), Some(0))
        );
        assert_eq!(
            (
                bins[1]["lower_bound"].as_u64(),
                bins[1]["upper_bound"].as_u64()
            ),
            (Some(1), Some(1))
        );
        assert_eq!(
            (
                bins[2]["lower_bound"].as_u64(),
                bins[2]["upper_bound"].as_u64()
            ),
            (Some(2), Some(3))
        );
        assert_eq!(
            (
                bins[3]["lower_bound"].as_u64(),
                bins[3]["upper_bound"].as_u64()
            ),
            (Some(4), Some(7))
        );
        assert_eq!(
            bins.iter()
                .map(|bin| bin["runs"].as_u64().unwrap())
                .sum::<u64>(),
            engine["runs"].as_u64().unwrap()
        );
        assert_eq!(
            bins.iter()
                .map(|bin| bin["instructions"].as_u64().unwrap())
                .sum::<u64>(),
            engine["instructions"].as_u64().unwrap()
        );
        assert_eq!(
            engine["end_reasons"]
                .as_object()
                .unwrap()
                .values()
                .map(|count| count.as_u64().unwrap())
                .sum::<u64>(),
            engine["runs"].as_u64().unwrap()
        );
    }

    #[test]
    fn worker_jobs_charge_the_submitting_device() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_guide("workers");
        let context = {
            let _device = device(2, "isp", DevicePhase::Deadline);
            capture_context()
        };
        assert!(context.is_some());
        run_on_worker(context, || {
            std::thread::sleep(std::time::Duration::from_micros(200));
        });
        let summary = end_guide();
        let row = summary
            .devices
            .iter()
            .find(|row| row.device == 2 && row.phase == "deadline")
            .unwrap();
        assert_eq!((row.calls, row.worker_calls), (1, 1));
        assert!(row.worker_elapsed_ns > 0);
    }

    #[test]
    fn summary_serializes_and_end_guide_resets_every_accumulator() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_guide("serialization");
        add_bytes(ByteCounter::HashedBytes, 4096);
        add_bytes(ByteCounter::HashedBytes, 96);
        let _device = device(1, "card", DevicePhase::Event);
        drop(_device);
        end_span(GuestProgress::default(), &[]);
        let first = end_guide().to_value();
        assert_eq!(first["bytes"]["hashed-bytes"], 4192);
        assert_eq!(first["devices"][0]["calls"], 1);
        assert_eq!(first["schema"], 1);
        assert!(first["clock"].is_string());
        assert!(first["spans"][0]["thread_cpu_ns"].is_u64());
        begin_guide("reset");
        let second = end_guide();
        assert_eq!(second.bytes["hashed-bytes"], 0);
        assert!(second.devices.is_empty());
        assert!(second.spans.is_empty());
    }

    #[test]
    fn counters_report_guide_deltas_from_existing_snapshots() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        begin_guide("counters");
    set_cpu_counters(vec![Ok(BTreeMap::from([
        ("translations".to_owned(), 40),
        ("loop_instructions".to_owned(), 900),
    ]))]);
    set_cpu_counters(vec![Ok(BTreeMap::from([
        ("translations".to_owned(), 47),
        ("loop_instructions".to_owned(), 950),
    ]))]);
        let summary = end_guide();
        assert_eq!(summary.counters[0].delta["translations"], Some(7));
        assert_eq!(summary.counters[0].delta["loop_instructions"], Some(50));
    }
}
