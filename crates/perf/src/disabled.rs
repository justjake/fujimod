//! Compiled-out ledger. Every call retains the public signature so call sites
//! stay identical between recorded and overhead-comparison builds.
use crate::types::*;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

pub fn worker_thread() {}

pub fn capture_context() -> Option<DeviceContext> {
    None
}

pub fn run_on_worker<R>(_context: Option<DeviceContext>, job: impl FnOnce() -> R) -> R {
    job()
}

pub fn register_devices(_names: &[&'static str]) {}

pub fn begin_guide(_id: &str) {}

pub fn end_guide() -> GuideSummary {
    GuideSummary::empty()
}

pub fn set_cpu_counters(_counters: Vec<Result<BTreeMap<String, u64>, String>>) {}

pub fn begin_wait(_description: &str) -> usize {
    usize::MAX
}

pub fn record_wait_evaluation(_index: usize) {}

pub fn record_wait_stop(_index: usize, _cause: &str, _device: Option<&str>) {}

pub struct SpanGuard;

impl SpanGuard {
    pub fn guest(&mut self, _start: GuestProgress, _end: GuestProgress) {}
}

pub fn span(_kind: SpanKind) -> SpanGuard {
    SpanGuard
}

pub fn begin_span(_kind: SpanKind, _guest: GuestProgress, _wait_counter: Option<usize>) -> bool {
    false
}

pub fn end_span(_guest: GuestProgress, _properties: &[(&str, u64)]) {}

pub struct AccumulatorGuard;

pub fn device(_index: u16, _kind: &'static str, _phase: DevicePhase) -> AccumulatorGuard {
    AccumulatorGuard
}

pub struct EngineRunGuard;

impl EngineRunGuard {
    pub fn stop(&mut self) {}

    pub fn set_instructions(&mut self, _instructions: u64) {}

    pub fn finish(self, _reason: EngineEndReason) {}

    pub fn cancel(self) {}
}

pub fn engine_run(_core: u8, _worker: bool) -> EngineRunGuard {
    EngineRunGuard
}

pub fn add_bytes(_counter: ByteCounter, _bytes: u64) {}

pub fn write_chrome_trace(path: &Path) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, b"{\"displayTimeUnit\":\"ns\",\"traceEvents\":[]}")
}
