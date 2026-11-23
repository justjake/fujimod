//! Cumulative host-side instruction and callback counters for performance reports.
use super::super::Board;
use fujimod_emulator_cpu::Model;
use serde_json::{Value, json};

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
                json!({
                    "core": core,
                    "inline_instructions": blocks.map_or(0, |blocks| blocks.inline.entries),
                    "callbacks": {
                        "instruction": blocks.map_or(0, |blocks| blocks.instruction_callbacks),
                        "block": blocks.map_or(0, |blocks| blocks.block_callbacks),
                        "restart": blocks.map_or(0, |blocks| blocks.restart_callbacks),
                        "store": blocks.map_or(0, |blocks| blocks.store_callbacks),
                        "mmio": blocks.map_or(0, |blocks| blocks.mmio_callbacks),
                    },
                })
            })
            .collect::<Vec<_>>();
        json!({
            "interactive_host_duration_ns": self.interactive_host_duration_ns.get(),
            "cortex_m4_by_core": cortex_m4_by_core,
        })
    }
}
