//! Cumulative host-side instruction and callback counters for performance reports.
use super::super::Board;
use fujimod_emulator_cpu::Model;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RegisterAccessKey {
    pub core: usize,
    pub address: u64,
    pub write: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegisterAccessTotals {
    pub calls: u64,
    pub service_samples: u64,
    pub sampled_service_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PublishedFastReadMappingKey {
    pub core: usize,
    pub mapping_base: u64,
}

#[derive(Clone, Debug, Default)]
pub struct RegisterAccessSnapshot {
    pub service_sampling_denominator: u64,
    pub totals: BTreeMap<RegisterAccessKey, RegisterAccessTotals>,
    pub published_fast_read_hits: BTreeMap<PublishedFastReadMappingKey, u64>,
}

impl Board {
    /// Cumulative execution observations, separate from modeled board state.
    pub fn execution_metrics(&self) -> Value {
        let state = self.state.borrow();
        let cortex_m4_by_core = self
            .cpus
            .iter()
            .enumerate()
            .filter(|(_, cpu)| cpu.model() == Model::CortexM4)
            .map(|(core, _)| {
                let blocks = state.blocks.get(core).and_then(Option::as_ref);
                let mut row = json!({
                    "core": core,
                    "inline_instructions": blocks.map_or(0, |blocks| blocks.inline.entries),
                    "callbacks": {
                        "instruction": blocks.map_or(0, |blocks| blocks.instruction_callbacks),
                        "block": blocks.map_or(0, |blocks| blocks.block_callbacks),
                        "restart": blocks.map_or(0, |blocks| blocks.restart_callbacks),
                        "store": blocks.map_or(0, |blocks| blocks.store_callbacks),
                        "mmio": blocks.map_or(0, |blocks| blocks.mmio_callbacks),
                    },
                });
                if let Some(counters) = state
                    .store_observer
                    .as_ref()
                    .and_then(|counters| counters.get(core))
                {
                    row.as_object_mut()
                        .expect("M4 metrics row is a JSON object")
                        .insert("store_observer".into(), counters.json());
                }
                if let Some(statistics) = &state.callback_service_time {
                    row.as_object_mut()
                        .expect("M4 metrics row is a JSON object")
                        .insert(
                            "callback_service_time".into(),
                            super::super::callback_timing::json(statistics, core),
                        );
                }
                row
            })
            .collect::<Vec<_>>();
        json!({
            "interactive_host_duration_ns": self.interactive_host_duration_ns.get(),
            "settled_mmio_reuses": state.settled.mmio_reuses,
            "private_interrupt_reuses": state.settled.private_interrupt_reuses,
            "bounded_cohort_segments": self.bounded_segments,
            "local_grant_completions": self.local_grant_completions,
            "cohort_pause_barriers": self.cohort_pause_barriers,
            "inline_cohort_runs": self.inline_cohort_runs,
            "inline_cohort_callbacks": self.inline_cohort_callbacks,
            "inline_cohort_instructions": self.inline_cohort_instructions,
            "inline_cohort_dispatches": self.inline_cohort_dispatches,
            "inline_cohort_entries_before_park": self.inline_cohort_entries_before_park,
            "withheld_lane_grants": self.withheld_lane_grants,
            "segments_reported_over_lead": self.segments_over_lead,
            "cortex_m4_by_core": cortex_m4_by_core,
        })
    }

    /// Exact cumulative access totals, enabled only by the access histogram diagnostic.
    pub fn register_access_snapshot(&self) -> Option<RegisterAccessSnapshot> {
        let mut totals = BTreeMap::new();
        for (core, address, write, calls, service_samples, sampled_service_ns) in
            super::super::access_histogram::snapshot(self.access_histogram.as_ref()?)
        {
            totals.insert(
                RegisterAccessKey {
                    core,
                    address,
                    write,
                },
                RegisterAccessTotals {
                    calls,
                    service_samples,
                    sampled_service_ns,
                },
            );
        }
        let mut published_fast_read_hits = BTreeMap::new();
        for binding in &self.bindings {
            let hits = binding.published_hits.get();
            if hits != 0 {
                *published_fast_read_hits
                    .entry(PublishedFastReadMappingKey {
                        core: binding.core,
                        mapping_base: binding.base,
                    })
                    .or_default() += hits;
            }
        }
        Some(RegisterAccessSnapshot {
            service_sampling_denominator:
                super::super::access_histogram::SERVICE_TIME_SAMPLE_DENOMINATOR,
            totals,
            published_fast_read_hits,
        })
    }
}
