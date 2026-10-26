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
system-register access, native page-table translation, and instruction hooks.
ARM32 translation filters and block accounting are not advertised for that
target. Opaque in-process snapshots work; durable AArch64 snapshot export and
architectural interrupt delivery are still unsupported by this raw backend.

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
