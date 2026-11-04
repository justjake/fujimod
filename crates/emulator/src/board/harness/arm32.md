# Model memory policy for isolated A32 callers

**Verified (source):** `Guard::with_model_memory` narrows model-initiated reads and writes to subsets
of the caller's existing readable and writable RAM spans. Default model access
is denied. This policy does not admit CPU MMIO or enable attached models.

**Verified (source):** `Guard::model_access` checks a complete transaction before the host copies
bytes. It checks direction, full extent, overflow, stopped state and a bounded
model-write record count. A denied transaction retains a guard failure, so a
model cannot ignore the returned error and make a later allowed access. Model
writes record `(address, byte_count)` separately from CPU stores; the counts
describe admitted transactions, not their payload or proof of device success.
CPU-store and model-write records each have the configured `store_limit` bound.

**Verified (source):** The XP4 physical host calls this check before its RAM copy. Its ordinary
unguarded device behavior remains unchanged. Guard snapshots and the Python
observation include model policies and admitted writes. The native admission
rule still refuses device-backed guarded callers. Device-aware MMIO policies,
width checks, event/IRQ behavior and end-to-end device tests must be implemented
before that rule can be relaxed.

**Verified (test scope):** Unit controls cover default denial, policy widening, executable exclusion,
direction, cross-boundary and overflowing transactions, sticky failure, record
limits and access after the return sentinel. These controls establish the
policy primitive; they do not demonstrate a device transfer or hardware state.

**Unknown:** End-to-end device-backed routine execution remains unqualified.
In particular, the policy primitive does not admit CPU MMIO or prove model
completion, cleanup, cache coherence or any physical memory geometry.
