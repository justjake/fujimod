//! Sampled service time for selected M4 board callbacks.
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc, time::Instant};

pub(super) const SERVICE_TIME_SAMPLE_DENOMINATOR: u64 = 1024;

#[derive(Clone, Copy)]
pub(super) enum CallbackKind {
    Block,
    Store,
    Mmio,
}

/// Named consumers of one block callback, timed separately while the
/// `FUJIMOD_BLOCK_PHASES` diagnostic is set. The engine cannot run any of them
/// inline, so the split decides which to batch and which to keep.
#[derive(Clone, Copy)]
pub(super) enum BlockPhase {
    Settle,
    Due,
    EnterCode,
    SettlePins,
    Scan,
    WindowRoom,
    Controller,
    SyncStores,
    Arms,
}

impl BlockPhase {
    pub(super) const COUNT: usize = 9;
    pub(super) const ALL: [Self; Self::COUNT] = [
        Self::Settle,
        Self::Due,
        Self::EnterCode,
        Self::SettlePins,
        Self::Scan,
        Self::WindowRoom,
        Self::Controller,
        Self::SyncStores,
        Self::Arms,
    ];
    fn index(self) -> usize {
        match self {
            Self::Settle => 0,
            Self::Due => 1,
            Self::EnterCode => 2,
            Self::SettlePins => 3,
            Self::Scan => 4,
            Self::WindowRoom => 5,
            Self::Controller => 6,
            Self::SyncStores => 7,
            Self::Arms => 8,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Settle => "settle",
            Self::Due => "due",
            Self::EnterCode => "enter_code",
            Self::SettlePins => "settle_pins",
            Self::Scan => "scan",
            Self::WindowRoom => "window_room",
            Self::Controller => "controller",
            Self::SyncStores => "sync_stores",
            Self::Arms => "arms",
        }
    }
}

impl CallbackKind {
    fn index(self) -> usize {
        match self {
            Self::Block => 0,
            Self::Store => 1,
            Self::Mmio => 2,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Store => "store",
            Self::Mmio => "mmio",
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Totals {
    sequence: u64,
    samples: u64,
    sampled_service_ns: u64,
}

pub(super) struct Statistics {
    by_core: Vec<[Totals; 3]>,
    /// Per-core, per-phase cumulative block-callback time and call counts.
    /// The call table stays `None` unless `FUJIMOD_BLOCK_PHASES` asks for the
    /// split; the environment is read once per process.
    block_phases: Vec<[u64; BlockPhase::COUNT]>,
    block_phase_calls: Option<Vec<[u64; BlockPhase::COUNT]>>,
    #[cfg(test)]
    sample_every_callback: bool,
}

pub(super) type SharedStatistics = Rc<RefCell<Statistics>>;

pub(super) fn new(cores: usize) -> SharedStatistics {
    Rc::new(RefCell::new(Statistics {
        by_core: vec![[Totals::default(); 3]; cores],
        block_phases: vec![[0; BlockPhase::COUNT]; cores],
        block_phase_calls: None,
        #[cfg(test)]
        sample_every_callback: false,
    }))
}

#[cfg(test)]
pub(super) fn new_for_test(cores: usize) -> SharedStatistics {
    Rc::new(RefCell::new(Statistics {
        by_core: vec![[Totals::default(); 3]; cores],
        block_phases: vec![[0; BlockPhase::COUNT]; cores],
        block_phase_calls: None,
        sample_every_callback: true,
    }))
}

/// Per-callback accumulator for the block-phase diagnostic. It lives on the
/// callback's own stack and reaches the shared totals once, when it drops, so
/// the measurement costs one clock read per phase and one borrow per callback
/// instead of a borrow per phase.
pub(super) struct PhaseCapture {
    statistics: Option<SharedStatistics>,
    core: usize,
    service: [u64; BlockPhase::COUNT],
    calls: [u64; BlockPhase::COUNT],
}

impl PhaseCapture {
    /// With the diagnostic off the capture is inert and every `span` runs its
    /// closure untouched. The environment is read once per process: a lookup
    /// per callback cost about a tenth of the M4's host time.
    pub(super) fn new(statistics: Option<&SharedStatistics>, core: usize) -> Self {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let enabled = statistics.is_some()
            && *ENABLED.get_or_init(|| {
                std::env::var_os("FUJIMOD_BLOCK_PHASES").is_some_and(|v| v != "0")
            });
        Self {
            statistics: enabled.then(|| statistics.cloned()).flatten(),
            core,
            service: [0; BlockPhase::COUNT],
            calls: [0; BlockPhase::COUNT],
        }
    }
    /// Run `f`, charging its wall time to `phase`.
    pub(super) fn span<T>(&mut self, phase: BlockPhase, f: impl FnOnce() -> T) -> T {
        if self.statistics.is_none() {
            return f();
        }
        let started = Instant::now();
        let value = f();
        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let index = phase.index();
        self.service[index] = self.service[index].saturating_add(elapsed);
        self.calls[index] = self.calls[index].saturating_add(1);
        value
    }
}

impl Drop for PhaseCapture {
    fn drop(&mut self) {
        let Some(statistics) = self.statistics.as_ref() else {
            return;
        };
        let mut s = statistics.borrow_mut();
        let cores = s.by_core.len();
        if s.block_phase_calls.is_none() {
            s.block_phase_calls = Some(vec![[0; BlockPhase::COUNT]; cores]);
        }
        let Statistics {
            block_phases,
            block_phase_calls,
            ..
        } = &mut *s;
        let calls = block_phase_calls
            .as_mut()
            .expect("the phase call table was just initialized");
        for (index, service) in self.service.iter().enumerate() {
            let phase = &mut block_phases[self.core][index];
            *phase = phase.saturating_add(*service);
            let count = &mut calls[self.core][index];
            *count = count.saturating_add(self.calls[index]);
        }
    }
}

pub(super) struct SampledCallback {
    statistics: SharedStatistics,
    core: usize,
    kind: CallbackKind,
    started: Instant,
}

impl Drop for SampledCallback {
    fn drop(&mut self) {
        let elapsed_ns = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let mut statistics = self.statistics.borrow_mut();
        let totals = &mut statistics.by_core[self.core][self.kind.index()];
        totals.samples = totals.samples.saturating_add(1);
        totals.sampled_service_ns = totals.sampled_service_ns.saturating_add(elapsed_ns);
    }
}

fn sample_this_callback(core: usize, kind: CallbackKind, sequence: u64) -> bool {
    // Hash each core/kind occurrence independently so busy kinds do not line
    // up on a fixed global stride.
    let mut value = (core as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (kind.index() as u64).wrapping_mul(0xbf58476d1ce4e5b9)
        ^ sequence.wrapping_mul(0x94d049bb133111eb);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^= value >> 31;
    value.is_multiple_of(SERVICE_TIME_SAMPLE_DENOMINATOR)
}

pub(super) fn count(
    statistics: &SharedStatistics,
    core: usize,
    kind: CallbackKind,
) -> Option<SampledCallback> {
    let should_sample = {
        let mut statistics = statistics.borrow_mut();
        let sample_every_callback = {
            #[cfg(test)]
            {
                statistics.sample_every_callback
            }
            #[cfg(not(test))]
            {
                false
            }
        };
        let totals = &mut statistics.by_core[core][kind.index()];
        totals.sequence = totals.sequence.wrapping_add(1);
        sample_every_callback || sample_this_callback(core, kind, totals.sequence)
    };
    should_sample.then(|| SampledCallback {
        statistics: statistics.clone(),
        core,
        kind,
        started: Instant::now(),
    })
}

pub(super) fn json(statistics: &SharedStatistics, core: usize) -> Value {
    let totals = statistics.borrow();
    let by_kind = &totals.by_core[core];
    let mut value = json!({
        "sampling_probability": {
            "numerator": 1,
            "denominator": SERVICE_TIME_SAMPLE_DENOMINATOR,
        },
    });
    let object = value
        .as_object_mut()
        .expect("timing metrics are a JSON object");
    for kind in [CallbackKind::Block, CallbackKind::Store, CallbackKind::Mmio] {
        let metrics = by_kind[kind.index()];
        object.insert(
            kind.name().into(),
            json!({
                "samples": metrics.samples,
                "sampled_service_ns": metrics.sampled_service_ns,
            }),
        );
    }
    if let Some(calls) = totals.block_phase_calls.as_ref() {
        let mut phases = serde_json::Map::new();
        for phase in BlockPhase::ALL {
            let index = phase.index();
            phases.insert(
                phase.name().into(),
                json!({
                    "calls": calls[core][index],
                    "service_ns": totals.block_phases[core][index],
                }),
            );
        }
        object.insert("block_phases".into(), Value::Object(phases));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::{CallbackKind, count, json, new_for_test};

    #[test]
    fn sampled_callback_durations_are_cumulative_by_kind() {
        let statistics = new_for_test(1);
        for kind in [CallbackKind::Block, CallbackKind::Store, CallbackKind::Mmio] {
            drop(count(&statistics, 0, kind));
        }
        let snapshot = json(&statistics, 0);
        assert_eq!(snapshot["sampling_probability"]["denominator"], 1024);
        for name in ["block", "store", "mmio"] {
            assert_eq!(snapshot[name]["samples"], 1);
            assert!(snapshot[name]["sampled_service_ns"].as_u64().is_some());
        }
    }
}
