# FujiMod MCP server: design (2026-11-18)

Status: the hub and the first slice of the camera module are implemented in
`crates/mcp-core`, `crates/mcp-camera` and `crates/mcp`; usage is in
[mcp/README.md](mcp/README.md). The primitives table marks what exists.
Decisions marked "agreed" were settled with the owner in conversation;
everything else is a proposal.

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
- "Boot a build" means the SD boot path: the SD card built from the image,
  booted through the boot ROM.
- The work happens on a branch of this repository.

## Crates

| Crate                | Directory           | Purpose                                                                                                    |
| -------------------- | ------------------- | ---------------------------------------------------------------------------------------------------------- |
| `fujimod-mcp-core`   | `crates/mcp-core`   | The module trait, the adapter for `rmcp` tool macros, tool-name rules and the hub. No camera dependencies. |
| `fujimod-mcp-camera` | `crates/mcp-camera` | The camera module: tools over `fujimod-emulator-camera`, `fujimod-emulator-boot` and the emulator session. |
| `fujimod-mcp`        | `crates/mcp`        | The hub binary: selects modules, serves stdio.                                                             |
| `fujimod-mcp-ghidra` | `crates/mcp-ghidra` | Later: the GhidraMCP proxy module.                                                                         |

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

Presses and turns go through `fujimod-remote`, which waits until the
firmware accepts input and confirms its key scanner registered each edge.
On an emulated camera that needs two things on the session, which two
crates build from the catalog: the control wiring (the electrical levels of
every control position, the session's `SwitchPanel`) and the input
confirmation (the scanner records each position must read ready and report,
the session's `contact_driver`).

- `fujimod-catalog-wiring` resolves every physical control against the
  firmware's located inputs and the body's wiring: concrete pins, ADC
  channels, threshold band offsets, scanner records and interfaces.
- `fujimod-emulator-controls` builds the wiring, composed across controls
  sharing a wire, and the input confirmation against the session's device
  graph, reading thresholds from the firmware's loaded configuration, and
  installs both. Its `Keys` and `set_key` drive the same wiring for timed
  key scripts, the CLI's `--keys` and `--press`, and boot's first-start
  acceptance.

On X70 1.10 every electronic control compiles, with thresholds read from
the running firmware.

### Cameras and threads

A camera is one opened target: an emulator session or a USB session. Every
tool takes a camera id, so several cameras at once is the ordinary case.

`Remote` futures have no `Send` bound and `Session` holds `RefCell` state,
so each camera lives on its own OS thread with a current-thread executor.
A tool sends the camera an action over a channel and awaits the reply. The
hub and other modules stay `Send + Sync`; the `rmcp` `local` feature is not
used.

### Primitives

"USB" says whether the primitive can work on a physical camera.

| Group   | Tool                                                                    | Does                                                                                                               | Status      | USB           |
| ------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ----------- | ------------- |
| Cameras | `camera_open`                                                           | Open a firmware with a boot from [CAMERAS.md](CAMERAS.md), or restore a checkpoint file                            | implemented | planned       |
|         | `camera_list`                                                           | Open cameras and how each was opened                                                                               | implemented | yes           |
|         | `camera_close`                                                          | Release the camera                                                                                                 | implemented | yes           |
|         | `camera_checkpoint`                                                     | Save the whole machine under a name, optionally to a file                                                          | implemented | no            |
|         | `camera_restore`                                                        | Return to a named checkpoint or a checkpoint file                                                                  | implemented | no            |
|         | `camera_fork`                                                           | Open a second camera from a checkpoint                                                                             | planned     | no            |
| Time    | `camera_run`                                                            | Advance guest seconds, or until REC                                                                                | implemented | waits only    |
| Inputs  | `camera_controls`                                                       | Control ids, kinds and positions `camera_press`, `camera_turn` and `camera_set` accept                             | implemented | yes           |
|         | `camera_press`                                                          | Press and release a control, confirmed by the firmware, settle, return state and screen                            | implemented | yes           |
|         | `camera_turn`                                                           | Turn a rotary control by detents                                                                                   | implemented | yes           |
|         | `camera_set`                                                            | Move a dial, switch, ring or lever to a named position, confirmed by the firmware, settle, return state and screen | implemented | yes           |
|         | `camera_touch`                                                          | Touch the screen                                                                                                   | planned     | yes           |
|         | `camera_set_input`                                                      | Drive a hardware input level (GPIO, ADC, SD card)                                                                  | planned     | no            |
| Observe | `camera_screen`                                                         | The current screen and state                                                                                       | implemented | yes           |
|         | `camera_state`                                                          | Boot and UI state words, guest time, instructions                                                                  | implemented | yes           |
|         | `camera_read_memory`                                                    | Physical memory as a hex dump or file                                                                              | implemented | yes           |
|         | `camera_read_registers`                                                 | CPU registers of a stopped core                                                                                    | planned     | no            |
|         | `camera_devices`                                                        | Device state: GPIO, serial, HDMI, SD card, scanout                                                                 | planned     | no            |
| Probes  | `camera_profile_start`, `camera_profile_collect`, `camera_profile_show` | Instructions executed per basic block                                                                              | implemented | no            |
|         | `camera_profile_diff`                                                   | Blocks whose instructions changed most between two profiles                                                        | implemented | no            |
|         | memory access, calls, PC trace, bus traffic                             | The board's other bounded observations                                                                             | planned     | no            |
| Mutate  | `camera_write_memory`, `camera_write_registers`                         | Write bytes (including code) or registers                                                                          | planned     | to be decided |
| Helpers | symbol names                                                            | Resolve addresses in results to names                                                                              | planned     | n/a           |

Planned `camera_run` conditions beyond time and REC: frame completed, memory
range changed, address reached, input accepted, UI ready.

### Checkpoint files

Agreed: checkpoints can be saved to a file and a camera can be opened from
one, so a boot is paid for once per build, not once per server process.

A portable snapshot (`emulator-session/src/checkpoint/portable.rs`) carries
the session definition: the run configuration id, overlays, quantum, clock,
languages, device graph and input identities. It does not carry the
firmware update or boot ROM, and it only restores into a session built from
the same inputs (`fujimod_emulator_boot::load_snapshot`). Import refuses
unless the receiving session's definition, firmware images, execution,
devices and scene all match.

`camera_open` from a file hides that step. It asks the builder for the
unexecuted board from the same firmware, boot, SD card and execution
policy (`open_unbooted`), then imports and restores the snapshot. The
builder's firmware library and resolved configuration are what make the
snapshot's definition, firmware images, devices and scene match.

Measured on X70 1.10 with the release `fujimod-emulator` CLI, on a 4-core
cloud machine with Mesa's software Vulkan driver:

- A stock boot to REC (`x70-110-rec`) ran 52 million guest instructions in
  4.2 s of wall time and wrote a 108 MB snapshot.
- A fresh process restored it in 1.4 s at the same instruction count, with a
  byte-identical screen. Through the Python binding, building the unexecuted
  board took 0.65 s and the restore 0.6 s.
- The restored camera accepted a MENU/OK press through `fujimod.remote`
  `Camera.from_board` and redrew the screen.
- A snapshot taken with SD card files attached restored into a board built
  with the same SD card files and continued running.

### Opening cameras

`camera_open` calls the shared builder in
[`fujimod-emulator-camera`](emulator-camera/src/lib.rs) with a catalog
firmware identity and one of its boots:

- `firmware`: the catalog release, such as `x70-110`.
- `boot`: `stock`, `sd-card` or `automatic-boot`. An `sd-card` boot holds
  the body's `boot.sd` controls through the session contact driver from
  power-on and releases them at REC; an `automatic-boot` holds nothing and
  drives the card's AUTOMATIC boot installer into internal flash once.
- `sd_card`: the directory for the virtual SD card. An `sd-card` or
  `automatic-boot` without it fetches the current release card for the
  firmware; for `stock` its files are added to the otherwise empty card.
- `execution_policy`: `exact` (the default) or `interactive`.
- `boot_cache`: restore a saved boot with the same inputs instead of
  booting, and save this boot; the default. `false` boots from scratch and
  saves nothing.
- `name`: the camera id later calls use.

The builder finds the firmware update, boot ROM and reference camera
sections in the directory named by `FUJIMOD_FIRMWARE_LIBRARY`, resolves
the firmware's registered run configuration, and owns the boot holds, card
media and control connection. Callers never name a configuration,
overlays, update files, boot ROMs, NAND images, GPIO levels or key
scripts.

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

1. The remote's optional input task layout and parameter-independent
   inputs. They should come from firmware locators, not written-down
   addresses.
2. How a build is made. The likely answer is that `camera_open` takes a
   built SD card directory and the agent runs the build itself.
3. Opening a physical camera: whether an agent may hold an interactive USB
   session open, or only run prepared jobs against the camera.
4. Whether guest time is a useful performance measure beyond instruction
   counts. This design assumes instruction counts and guest ticks are
   reliable for comparison and that host time is not.
5. Whether a rebuilt mod can be loaded into a running session. The RAM mod
   loader in `fujimod-emulator-boot` attaches hooks once during boot.
6. Whether memory writes over USB belong in this module at all.

## First slice (implemented)

The hub serves the camera module over stdio. An MCP client drove every
implemented tool against X70 1.10:

- A stock camera opened to REC in about 2.6 s of wall time; restoring a
  named checkpoint took 0.3 s, and opening a second camera from its
  checkpoint file 1.2 s.
- `menu_ok_button` opened the shooting menu, confirmed by the firmware's
  key scanner; the control ring turned and pad directions pressed.
- Profiles of the same guest time from one checkpoint, idle and with the
  MENU/OK press, differed by about 15 million instructions and 9,723
  blocks that only ran with the press.
- A camera opened from a checkpoint file adopted the saved camera's
  control wiring and accepted presses.
- A FujiMod SD card booted to REC, showed its welcome sheet after eight
  guest seconds, and DISP/BACK dismissed it.

## Next

1. Names for profile and memory addresses from a build's linked symbols.
2. `camera_touch` and moving positional controls.
3. Memory-access, call and PC-trace probes.
4. `camera_run` stop conditions beyond time and REC.
