# FujiMod MCP server: design (draft, 2026-11-18)

Status: design only. Nothing here is implemented. Decisions marked "agreed"
were settled with the owner in conversation; everything else is a proposal.

## Goal

Give agents a fast way to analyze code running in a camera. Today an agent
that cannot tell which code is responsible for a behavior rebuilds all mods
with a small change, boots the emulator, looks, and starts again. Each loop
pays for a build and a boot.

The server keeps virtual cameras alive between tool calls, so an agent boots
once and then runs many experiments from checkpoints: press inputs, look at
the screen, watch memory, profile execution, patch RAM, compare two cameras.
The same control and observation tools work on a physical camera over USB.

## Agreed decisions

- One server binary (the hub) with tool groups (modules) that can be active
  or not.
- The camera module comes first. A Ghidra module that proxies GhidraMCP
  comes later.
- General primitives, not shrink-wrapped workflow tools. Workflows such as
  "find what code handles this button" or "compare stock with FujiMod" are
  recipes in skills, built from the primitives.
- No batch tool that runs a list of steps in one call.
- "Boot a build" means the SD boot path: the card built from the image,
  booted through the boot ROM.
- The work happens on a branch of this repository.

## Crates

| Crate | Directory | Purpose |
|---|---|---|
| `fujimod-mcp-core` | `crates/mcp-core` | The module trait, tool-name rules, result handles. No camera or protocol dependencies beyond `rmcp` types. |
| `fujimod-mcp-camera` | `crates/mcp-camera` | The camera module: tools over `fujimod-remote` and the emulator. |
| `fujimod-mcp` | `crates/mcp` | The hub binary: selects modules, serves stdio. |
| `fujimod-mcp-ghidra` | `crates/mcp-ghidra` | Later: the GhidraMCP proxy module. |

One crate per module follows [the emulator crate layout](emulator/CRATES.md):
an edit to one module rebuilds that module and the binary.

## Hub

The hub uses `rmcp`, the official Rust MCP SDK (3.11.0 when this was
written). Its minimum Rust version is 1.88, the workspace's. It builds on
tokio, and the lockfile already holds tokio, axum, hyper, tower and
schemars. Adding it requires `mise run rust-hack`.

- The hub implements `rmcp::ServerHandler` once. It owns the tool table and
  passes each call to the module that registered the tool.
- A module implements a small object-safe trait: list its tools, call one.
  A module written with the `rmcp` `#[tool]` macros plugs in through an
  adapter holding the module's state and its typed `ToolRouter`.
- Activation has three levels. Cargo features on the binary choose which
  modules are compiled in. A `--modules` argument chooses which are active
  at startup; this is the main switch. A core tool can enable or disable a
  module at runtime, which sends `notifications/tools/list_changed`; client
  support for that notification is uneven, so nothing may depend on it.
- Tool names carry the module prefix: `camera_press`, `ghidra_decompile`.
- Transport is stdio first. The hub is `Clone` over shared state so it can
  also be mounted as a Streamable HTTP service later, when several agents
  need to share one camera.

A throwaway prototype (not in this repository) confirmed against `rmcp`
3.11.0 that modules with different state types compose behind one trait,
that tools of an inactive module are refused, that enabling a module at
runtime notifies the client, and that an upstream server's tools can be
re-exported, hidden or replaced. It used a fake upstream and a stand-in for
the emulator session.

## Camera module

### What it builds on

[`fujimod-remote`](remote/src/lib.rs) already drives an emulator session and
a USB camera through one interface:

- `Remote` and `RemoteReader` (`remote/src/protocol.rs`): read a menu or
  control reference, read memory, take a frame, check input readiness, set
  an input, wait on a read set.
- `BoardRemote` (`remote/src/board.rs`) implements them for an emulator
  session, and adds checkpoint and restore. `Ff80Remote`
  (`remote/src/ff80.rs`) implements them for a camera over USB.
- `operations` (`remote/src/operations.rs`) holds `press`, `turn`, `tap`,
  `drag`, `navigate`, `select`, `screenshot`, `wait_until` and `wait_ready`,
  written once over `Remote`.
- A captured `fujimod_recording::Frame` encodes itself as PNG.

The emulator board already has bounded observation
(`emulator/src/board/observation.rs`, `emulator-cpu/src/lib.rs`): memory
watches with task context, function watches, instruction points, PC traces,
bus transaction watches, and a guest execution profile with exact
instruction counts per block.

The module adds no second control path. Where a tool needs something these
layers lack, the layer gains it.

### Cameras and threads

A camera is one opened target: an emulator session or a USB session. Every
tool takes a camera id, so several cameras at once is the ordinary case.

`Remote` futures have no `Send` bound and `Session` holds `RefCell` state,
so each camera lives on its own OS thread with a current-thread executor.
A tool sends the camera an action over a channel and awaits the reply. The
hub and other modules stay `Send + Sync`; the `rmcp` `local` feature is not
used.

### Primitives

Names are provisional. "USB" says whether the primitive works on a physical
camera.

| Group | Tool | Does | USB |
|---|---|---|---|
| Cameras | `camera_open` | Boot an SD card (stock or a build), open a checkpoint file, or attach to a USB camera | see open questions |
| | `camera_list` | Open cameras, their kind, firmware and checkpoints | yes |
| | `camera_checkpoint` | Save the whole machine under a name, optionally to a file | no |
| | `camera_restore` | Return to a named checkpoint | no |
| | `camera_fork` | Open a second camera from a checkpoint | no |
| | `camera_close` | Release the camera | yes |
| Time | `camera_run` | Advance until a condition or a deadline and report which stopped it | waits only |
| Inputs | `camera_press`, `camera_turn`, `camera_touch` | Operate a physical control | yes |
| | `camera_set_input` | Drive a hardware input level (GPIO, ADC, card) | no |
| Observe | `camera_screen` | The current frame as an image | yes |
| | `camera_state` | UI mode, menu node, cursor, a menu or control reference | yes |
| | `camera_read_memory` | Bytes at an address or symbol | yes |
| | `camera_read_registers` | CPU registers of a stopped core | no |
| | `camera_devices` | Device state: GPIO, serial, HDMI, card, scanout | no |
| Probes | `probe_arm` | Start a probe: memory access, calls, execution profile, PC trace, bus traffic | no |
| | `probe_collect` | Stop it and return its records | no |
| Mutate | `camera_write_memory` | Write bytes, including code, and invalidate translations | to be decided |
| | `camera_write_registers` | Write CPU registers of a stopped core | no |
| Helpers | `symbols_resolve` | Name to address and back, from the build's symbols | n/a |
| | `result_diff` | Compare two results of the same kind | n/a |

`camera_run` conditions: frame completed, memory range changed, address
reached, input accepted, UI ready, guest deadline.

### Checkpoint files

Agreed: checkpoints can be saved to a file and a camera can be opened from
one, so a boot is paid for once per build, not once per server process.

A portable snapshot (`emulator-session/src/checkpoint/portable.rs`) carries
the session definition: the run configuration id, overlays, quantum, clock,
languages, device graph and input identities. It does not carry the
firmware update or boot ROM, and today it only restores into a session the
caller has already built from the same inputs
(`fujimod_emulator_boot::load_snapshot`).

`camera_open` from a file hides that step. It reads the definition from the
snapshot header, builds the unexecuted board that definition names, then
imports and restores the snapshot. The caller supplies the firmware files
the header identifies, or a directory to find them in by hash. Whether the
SD card and NAND contents travel in the snapshot is not yet checked.

A snapshot written by a board built another way, such as the Python board
factory in the private repository, has a different definition and is
refused.

### Rules

- An input tool returns an observation: the new screen, the UI state and
  what changed since the previous observation. The common case is one call.
- Probes stay armed across calls. An agent arms one, makes any number of
  input and run calls, then collects.
- Results are handles. A probe collection or observation gets an id; the
  reply carries a bounded, ranked summary and the full record goes to a
  file. `result_diff` and later queries take ids.
- Addresses are reported with names. Every address in a result is resolved
  against the build's linked symbols and the firmware's known functions.
- `result_diff` is what makes the primitives general. Two execution
  profiles from one checkpoint, with and without an input, show what ran
  because of the input. Two profiles from a stock camera and a FujiMod
  camera show where time went. Two device states show what hardware
  changed. Two screens show a pixel difference.
- A primitive that a target cannot support refuses with that reason. It
  never degrades silently.
- Runs that will be compared use the exact execution policy, so a restored
  checkpoint replays identically.

### Changes to existing crates

- The Python binding has a string-to-operation dispatcher
  (`bindings/python/remote/src/lib.rs`, `Camera.operation`). It moves into
  `fujimod-remote` as a serializable action type and one function that runs
  an action on a `Remote`. The binding and the module both call it.
- The emulator-only operations (checkpoint, restore, probes, device state,
  register access, hardware inputs) go behind one small trait that
  `BoardRemote` implements, so the shared tools never branch on the target
  kind.
- `Remote` has no memory write. If `camera_write_memory` should work over
  USB, the trait gains one and both backends implement it.

## Recipes

Recipes live as skills beside the tools, each a short list of primitive
calls an agent can adapt. The first ones:

- Find the code that handles an input: checkpoint, profile an idle run,
  restore, profile the same run with the input, diff.
- Find what writes a value: arm a memory-access probe on its range, do the
  action, collect.
- Compare stock with FujiMod: open both, bring each to the same UI state,
  run the same inputs, diff the timings and the profiles.

## Ghidra module (later)

At startup the module connects to an upstream GhidraMCP as an `rmcp`
client, lists its tools and treats each one of three ways: re-export it
under the `ghidra_` prefix, hide it, or replace it with a tool that wraps
the upstream call. An allowlist decides; nothing is passed through by
default. The upstream runs as a stdio child process first, which avoids
adding `reqwest`.

## Open questions

1. How a Rust server gets an SD-booted session. `fujimod-emulator-boot`
   prepares and opens sessions and saves snapshots in Rust. The SD boot the
   owner means, with its cached post-boot snapshot, is driven today by
   Python in the private repository. Either the Rust path gains the SD card
   boot, or `camera_open` accepts a snapshot the existing tooling produced.
2. How a build is made. The build tools live outside this workspace. The
   likely answer is that `camera_open` takes a built image or card
   directory and the agent runs the build itself.
3. Opening a physical camera. Camera runs go through a queue elsewhere, and
   an interactive USB session opened by an agent may not fit it.
4. Whether guest time is a useful performance measure beyond instruction
   counts. This design assumes instruction counts and guest ticks are
   reliable for comparison and that host time is not.
5. Whether a rebuilt mod can be loaded into a running session. The RAM mod
   loader in `fujimod-emulator-boot` attaches hooks once during boot.
6. Whether memory writes over USB belong in this module at all.

## First slice

1. `fujimod-mcp-core` and the `fujimod-mcp` binary serving stdio with the
   camera module.
2. Cameras: open by SD boot, checkpoint, restore, close.
3. Inputs and observation: press, turn, screen, UI state, memory read,
   symbol lookup.
4. One probe end to end, the execution profile, with `result_diff`.

That is enough to try one real "why did stock do A when we did B" question
with two cameras and judge whether the loop is faster.
