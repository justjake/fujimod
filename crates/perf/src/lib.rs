//! Process-wide performance ledger for walkthrough and emulator runs.
//!
//! Three tiers keep the cost of always-on recording negligible: phase spans,
//! per-device and per-engine elapsed accumulators, and integer byte/counter
//! totals. Nothing here participates in modeled state or guest decisions.
//!
//! Native builds record by default. Browser builds compile the ledger only
//! with the `perf` feature, which local development builds pass and published
//! builds never pass. `fujimod_perf_off` exists only for an overhead
//! comparison build.

mod types;
pub use types::*;

#[cfg(all(
    not(fujimod_perf_off),
    any(not(target_arch = "wasm32"), feature = "perf")
))]
mod enabled;
#[cfg(all(
    not(fujimod_perf_off),
    any(not(target_arch = "wasm32"), feature = "perf")
))]
pub use enabled::*;

#[cfg(not(all(
    not(fujimod_perf_off),
    any(not(target_arch = "wasm32"), feature = "perf")
)))]
mod disabled;
#[cfg(not(all(
    not(fujimod_perf_off),
    any(not(target_arch = "wasm32"), feature = "perf")
)))]
pub use disabled::*;
