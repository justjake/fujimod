# Driving a camera from any host

Every host (the CLI, the MCP camera module, the browser binding, the desktop
window) opens, runs, operates and watches a camera the same way, whatever its
processor. Hosts never match on the processor generation.

## One opened camera

`fujimod_emulator_boot::open` returns one `Camera`:

- `session`: the `fujimod_emulator_session::Session` holding the board,
  physical controls, contact confirmation, media and checkpoints.
- Startup facts a run needs afterwards (card images written back at the end of
  a loader start, the RAM read limit, ticks per second, quantum).

There is no `Opened` enum and no `into_exr2`. Generation-specific startup
(the XP5 boot ROM hand-off, card images, EXR II plans) happens inside `open`
and inside board construction, selected by the configuration's checked graph.

XP4 runs on its own cluster type, not on `Session`. Until XP4 is ported onto
the shared board, `open` refuses XP4 configurations for hosts that need a
`Session`; the CLI's XP4 application path stays as it is.

## One input path

A session's physical controls are its `SwitchPanel` (`session.controls`) and
its contact confirmation (`session.contact_driver`), compiled from the
catalog by `fujimod_emulator_controls`. Hosts press, release, turn, move and
touch controls by catalog control id through the session; the control
catalog a frontend lays out is the driver's `ControlInput` table.

XP5 bodies use the same path. Their GPIO, ADC and serial status owners are
named device nodes in the session definition, and their catalog wiring
compiles to switch-panel circuits like any other body. A position whose
firmware scanner records are not yet located has no confirmation checks; the
transition completes when its contacts are applied. The XP5 key-input models
and `Inputs::key`/`Inputs::set_position` are removed once nothing calls them.

## One run loop

`fujimod_emulator_boot::Runner` advances a `Camera` one bounded slice at a
time: apply host actions that are due (queued inputs, timed GPIO levels,
memory pokes, patches, watch arming), run the session until the slice ends or
the next due action, then hand new console lines and LCD frames to the host.
The CLI boot commands, the MCP camera module and the desktop host all drive
this one loop; no host keeps its own copy.

## Hosts

- The desktop host serves any `Camera` on a loopback WebSocket using the
  protocol in `crates/emulator-gui`. It reads the control catalog and
  positions from the session and sends inputs to it; it has no
  processor-specific control code. Socket reads never block guest execution,
  and an LCD frame is encoded only after it is known to differ from the last
  one sent.
- The MCP camera module opens every configuration `open` accepts.
