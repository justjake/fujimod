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
    dispatch_samples: u64,
    sampled_before_dispatch_ns: u64,
    sampled_dispatch_ns: u64,
    sampled_after_dispatch_ns: u64,
}

pub(super) struct Statistics {
    by_core: Vec<[Totals; 3]>,
    #[cfg(test)]
    sample_every_callback: bool,
}

pub(super) type SharedStatistics = Rc<RefCell<Statistics>>;

pub(super) fn new(cores: usize) -> SharedStatistics {
    Rc::new(RefCell::new(Statistics {
        by_core: vec![[Totals::default(); 3]; cores],
        #[cfg(test)]
        sample_every_callback: false,
    }))
}

#[cfg(test)]
pub(super) fn new_for_test(cores: usize) -> SharedStatistics {
    Rc::new(RefCell::new(Statistics {
        by_core: vec![[Totals::default(); 3]; cores],
        sample_every_callback: true,
    }))
}

pub(super) struct SampledCallback {
    statistics: SharedStatistics,
    core: usize,
    kind: CallbackKind,
    started: Instant,
    dispatch: Option<(Instant, Option<Instant>)>,
}

impl SampledCallback {
    pub fn begin_dispatch(&mut self) {
        self.dispatch = Some((Instant::now(), None));
    }
    pub fn end_dispatch(&mut self) {
        if let Some((_, end)) = &mut self.dispatch {
            *end = Some(Instant::now());
        }
    }
}

fn elapsed_ns(from: Instant, to: Instant) -> u64 {
    u64::try_from(to.duration_since(from).as_nanos()).unwrap_or(u64::MAX)
}

impl Drop for SampledCallback {
    fn drop(&mut self) {
        let finished = Instant::now();
        let mut statistics = self.statistics.borrow_mut();
        let totals = &mut statistics.by_core[self.core][self.kind.index()];
        totals.samples = totals.samples.saturating_add(1);
        totals.sampled_service_ns = totals
            .sampled_service_ns
            .saturating_add(elapsed_ns(self.started, finished));
        if let Some((begin, Some(end))) = self.dispatch {
            totals.dispatch_samples = totals.dispatch_samples.saturating_add(1);
            totals.sampled_before_dispatch_ns = totals
                .sampled_before_dispatch_ns
                .saturating_add(elapsed_ns(self.started, begin));
            totals.sampled_dispatch_ns = totals
                .sampled_dispatch_ns
                .saturating_add(elapsed_ns(begin, end));
            totals.sampled_after_dispatch_ns = totals
                .sampled_after_dispatch_ns
                .saturating_add(elapsed_ns(end, finished));
        }
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
        dispatch: None,
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
        let mut entry = json!({
            "samples": metrics.samples,
            "sampled_service_ns": metrics.sampled_service_ns,
        });
        if matches!(kind, CallbackKind::Mmio) {
            entry["dispatch_samples"] = json!(metrics.dispatch_samples);
            entry["sampled_before_dispatch_ns"] = json!(metrics.sampled_before_dispatch_ns);
            entry["sampled_dispatch_ns"] = json!(metrics.sampled_dispatch_ns);
            entry["sampled_after_dispatch_ns"] = json!(metrics.sampled_after_dispatch_ns);
        }
        object.insert(kind.name().into(), entry);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::{CallbackKind, count, json, new_for_test};

    #[test]
    fn sampled_callback_durations_are_cumulative_by_kind() {
        let statistics = new_for_test(1);
        for kind in [CallbackKind::Block, CallbackKind::Store] {
            drop(count(&statistics, 0, kind));
        }
        let mut sample = count(&statistics, 0, CallbackKind::Mmio).unwrap();
        sample.begin_dispatch();
        sample.end_dispatch();
        drop(sample);
        let snapshot = json(&statistics, 0);
        assert_eq!(snapshot["sampling_probability"]["denominator"], 1024);
        for name in ["block", "store", "mmio"] {
            assert_eq!(snapshot[name]["samples"], 1);
            assert!(snapshot[name]["sampled_service_ns"].as_u64().is_some());
        }
        let mmio = &snapshot["mmio"];
        assert_eq!(mmio["dispatch_samples"], 1);
        let sum = ["sampled_before_dispatch_ns", "sampled_dispatch_ns", "sampled_after_dispatch_ns"]
            .iter()
            .map(|key| mmio[key].as_u64().unwrap())
            .sum::<u64>();
        assert_eq!(mmio["sampled_service_ns"], sum);
    }
}
