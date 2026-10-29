# Unicorn CPU backend

Its `sys/` crate, `fujimod-unicorn-sys`, builds the pinned Unicorn source as a
static ARM/AArch64 archive during Cargo builds and links the archive into each
executable or extension. It has no features, so the archive is compiled once
per profile. There is no runtime Unicorn library selection. Initialize the
source submodule once in a fresh checkout:

```sh
git submodule update --init -- crates/emulator-cpu-unicorn/vendor/unicorn
```

The sys crate's build script configures CMake and builds the archive automatically.
The build requires CMake 3.24 or newer and a native C compiler. Configuration
starts with a fresh CMake cache so compiler changes cannot discard the ARM and
AArch64 selection. The build refuses a cache that selects other architectures.

`Model::ArmV8A` opens the Cortex-A53 target with native EL3/EL2/EL1 execution,
system-register access, native page-table translation, instruction hooks,
translation filters, and block accounting. Instruction-budgeted AArch64 runs
count translated instruction entries per block, shortening the final block to
preserve exact limits. Instruction observers retain every entry; helper-driven
store restarts return the unentered block tail to the budget. A caller that
installs a translation filter owns accounting explicitly.

The exact accounting ring packs a 32-bit guest address and instruction count.
Blocks outside that address range return to the owner's block hook rather than
truncating their addresses; coarse accounting supports full 64-bit addresses.
Opaque in-process snapshots work; durable AArch64 snapshot export and
architectural interrupt delivery are still unsupported by this raw backend.

`cargo run --release -p fujimod-emulator-cpu-unicorn --example span -- INPUT.json`
measures a caller-supplied AArch64 span from verified images and explicit startup
registers. The input supplies `model`, `memory` (address/size pairs), `images`
(address/path/SHA-256 triples), `registers`, `begin`, `until`, and `repeats`.
It calibrates instruction entries, interleaves budgeted/unbudgeted samples and
a legacy per-instruction counting reference, and checks matching final integer
registers and RAM. Only execution is timed; mapping and verification are outside
the samples.

## A-profile board execution

The board factory wraps AArch64 Unicorn processors in `aprofile::Core`.
Ordinary instructions advance exact budgets in native translated blocks;
framework register accesses and WFE/WFI/SEV instructions exit before entry so
the board supplies their values, effects, and guest time. Device accesses
expose the entered instruction count and can shorten the current deadline.
Interrupts enter at the next exact instruction boundary, including after
mask changes and exception returns. The adapter shares code-write coherence,
accounting, and observer indexing with the R-profile adapter through the
`memory`, `accounting`, and `observers` modules. Instruction points and completed
store observers retain exact entry and completion boundaries.

## Exact loop execution

Exact board runs use `LoopMode::Accelerate` by default. Select
`LoopMode::Measure` to count eligible instructions while executing them, or
`LoopMode::Disabled` for the ordinary backend comparison.
The board grants each exact run a stable window ending at its first device,
DMA, interrupt or runnable-peer boundary. A standalone CPU owner must grant
that window explicitly; it expires when the run returns. Measurement executes
every instruction and counts qualifying taken iterations. Acceleration charges
the same instructions and leaves partial iterations to ordinary execution.

The conservative A32 proof accepts direct backward loops of at most sixteen
instructions, unconditional integer operations, aligned RAM/declared pure
status loads, fixed architectural states and unit-step counters compared with
an invariant immediate or register. It rebuilds the proof from fetch-checked
bytes, checks data permissions, preserves NZCV, and stops before counter wrap.
Stores, atomics, system operations, conditional bodies, Thumb and unsupported
algebra continue through the CPU normally. Instruction/block observers inside
a loop, read observers, discovery, transaction recording and watched device
waits disable acceleration. Loop counters and worklist visit stamps are host
bookkeeping; pending work, deadlines and all modeled device state remain in
the machine fingerprint.
