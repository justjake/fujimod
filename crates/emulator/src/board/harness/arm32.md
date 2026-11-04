# Memory and device policy for isolated A32 callers

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

**Verified (source):** The XP4 physical host calls this check before its RAM copy.
Native guard admission refuses attached models unless an explicit
`device_policy` JSON document selects `mmio`, `model_readable` and
`model_writable` policies. Unknown JSON fields and invalid ranges are rejected
before the guard becomes active. Model RAM policies narrow the caller's RAM
permissions; MMIO ranges must lie outside all RAM backing and inside windows
owned by the attached models or the board's GIC. Discovery, timers and active
observers remain excluded. Unguarded device behavior remains unchanged.

**Verified (source):** Each MMIO entry specifies half-open `start`/`end`, exact
`width` (1, 2 or 4 bytes) and separate `read`/`write` permissions. Entries are
aligned and disjoint. CPU instruction preflight admits plain single
LDR/STR, LDRB/STRB and LDRH/STRH for MMIO, while atomic and multiple accesses remain
excluded. A second check before dispatch verifies core, address, extent,
alignment, width, direction, stopped state and record bound. A denied read
cannot consume model data and a denied write cannot alter model state through
this dispatch. Admitted MMIO accesses and model RAM writes have separate bounded
observations; policies cannot change after execution/access begins.

**Verified (source):** Guarded execution keeps MMU off, little-endian A32 and
IRQ/FIQ masked. Device events run at ordinary guest instruction ticks. A model
RAM denial during event dispatch stops the CPU before another instruction.
Allowed DMA writes may yield the CPU for code-cache invalidation; callers
resume the same bounded session rather than treating that yield as completion.

**Verified (test scope):** Unit controls cover default denial, policy widening, executable exclusion,
direction, cross-boundary and overflowing transactions, sticky failure, record
limits and access after the return sentinel. These controls establish the
policy primitive. The focused suite passes 13 tests.

**Verified (integration controls):** Three XP4 tests qualify model-owned
register reads/writes, default refusal without a policy, rejection of unowned
or RAM-overlapping MMIO, and a patterned 32-byte NOR/DMA transfer. The transfer
preserves a trailing 32-byte canary. Restricting the destination policy to 16
bytes rejects that complete 32-byte host write before any destination byte
changes. The guest resumes across a cache-invalidation yield in the admitted
case. Five existing physical-memory tests also pass; the XP4 Python binding
compile check passes. These are synthetic model controls, not hardware tests.

**Unknown:** Firmware-specific initialization, ownership, controller completion,
recovery and physical cache coherence remain to be qualified by callers. The
model transfer does not measure physical timing or memory geometry. Hardware
cache behavior and atomicity of a larger device operation composed of several
separately guarded host writes do not follow from the single-write control.
