# Program-independent emulation

Peripherals, scheduling and the CPU backend must not infer software intent by
inspecting the program being emulated. An explicit profile, exact firmware
hash, a declared synthetic scene, strict admission checks and a
firmware-specific namespace do not exempt this rule.

## Allowed interfaces

Devices consume their own register accesses, modeled hardware peers, explicit
external inputs, virtual time and DMA buffers addressed through their hardware
interface. A descriptor fetched from a peripheral's descriptor-address register
is a hardware input. A driver request fetched from a known software global,
CPU argument or inferred stack frame is not.

The CPU backend decodes instructions to execute the ISA and handles actual
architectural traps. It may also fast-forward a loop exactly:

- From the instructions it is executing, it proves that the loop makes no
  stores and no side-effecting accesses, and that each iteration's effect
  follows only from the registers, flags and inputs it reads. The inputs are
  RAM and device registers whose device declares the value stable until its
  next deadline.
- It then advances directly to the first point where the loop exits or an
  input can change: an interrupt, a device deadline, or a DMA or peer write
  into the loop's read set.
- It charges the instructions and virtual time that execution would have
  taken, and leaves the identical architectural state at that exact
  instruction.

Counted delay loops and loops polling a flag both qualify. The proof uses no
firmware addresses, task names or RTOS structures, and a regression test
compares results with the fast-forward disabled.

Recognizing particular code is not a proof. Guessing idle execution from
nearby opcodes, repeated PCs, task names or scheduling structures is
forbidden. So is matching a known routine (a library function, a driver, a
firmware hash or its location) and substituting a computed result or state for
executing it.

Debugger observations, checkpoint integrity and asset verification remain
observational. Explicit, isolated `DriverHarness` substitutions remain reported
test interventions, not ordinary device behavior.

IRQ register values may be retained and validated as hardware state. A device
must not require a particular firmware handler address. Opaque program uploads
are bytes transferred through the interface: their hash may be reported, but
recognizing a known hash or its location cannot authorize an effect.

## Device scheduling

Devices declare `next_event_tick(now)` as the exact guest virtual tick of their
next self-initiated effect, or `None` when there is none. `tick(board)`
performs that effect when it is due; an early tick is a no-op. Device behavior
depends on guest virtual time and guest accesses, never on how often the
scheduler calls a tick.

Do not invent polling deadlines for conditions changed only by the guest.
Evaluate those conditions lazily on guest-driven device accesses or peer reads,
and only when their consequences are observed through that device. If a
consequence affects another device, declare the causing native model in
`native_dependencies` so its access wakes the dependent device, and return
`now` when that effect is ready. Periodic deadlines represent actual hardware
clocks, such as frame edges, sampling or motor steps.

Host reads never change modeled state. Snapshots, summaries, checkpoint
capture, fingerprints and tool inspection are observational. Do not put state
changes in property getters; perform them explicitly on a guest-driven access,
a due tick or a completion.

## When the camera disagrees

When the camera breaks something the emulator accepted, the same change models
it. The emulator either reproduces the camera behavior or refuses the
configuration with an error naming the observation. Either way, the change adds
a regression test citing the camera run.

## Discovery mode

The one exception is an explicitly selected discovery bus mode
(`Mode::Discovery`). It records each unmodeled access and continues, and its
runs never count as boots (`counts_as_boot` is false). Strict execution
enables none of its interventions.
