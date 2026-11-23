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

The switch panel drives devices only through device capabilities:
`GpioInputs` for GPIO pads, `AnalogInputs` for converter channels and
`SerialStatus` for serial status registers. It never reaches into a
processor's resource tables.

XP5 bodies use the same path. A body whose catalog record declares
`ladders` (`catalog/bodies/<body>/controls.json`) gets its panel from
`fujimod_emulator_xp5_board::panel`: each ladder wire becomes one circuit
over the controls that read it, with a case for every declared band, and the
board table in `ladder_controls` adds what the catalog schema cannot yet hold
(contact sets, position dials, shutter pads, command dials, push pads). ADC
ladders drive the ADC's `AnalogInputs`; the power controller's ladder
registers drive its `SerialStatus` registers; shutter and push pads drive
`GpioInputs`. A quadrature command dial is an encoder node turned through
`RotaryInput`; a counting dial is a power-controller register. The panel is
installed with `Session::install_panel`, the same call the browser and
Python bindings use for a caller-described board.

XP5 key scripts (`--press`, `--keys`) and the frame viewer name physical keys
(`MENU`, `STICK_UP`, `REAR+`); `Inputs::key` maps each to its catalog control
and schedules it on the session. There is no second key model.

A position without located firmware scanner records has no confirmation
checks; its transition completes when its contacts are applied.

## One run loop

`fujimod_emulator_boot::Runner` advances a `Camera` one bounded slice at a
time: apply host actions that are due (queued inputs, timed GPIO levels,
memory pokes, patches, watch arming), run the session until the slice ends or
the next due action, then hand new console lines and LCD frames to the host.
The CLI boot commands, the MCP camera module and the desktop host all drive
this one loop; no host keeps its own copy.

## Hosts

- The viewer owns a share room (`crates/webrtc`): the same sealed
  `#?remotecontrol=` rooms, `HostRoom` and `PeerSocket` that camera sharing
  uses, so Dev Tools controls an emulated camera exactly as it controls a
  real one. Other devices join through the room's share link. The desktop
  window joins a loopback room on the same machine, with the same protocol
  and no signaling service.
- The LCD is one outgoing video track on the controller's `PeerSocket`:
  H.264 Baseline from the workspace's `openh264`, with zero latency and no
  B-frames. Video is sent only over a direct candidate pair; a controller
  reached through a TURN relay gets controls without video. The control
  catalog, positions, inputs, touches and replies share one ordered data
  channel. The viewer has no processor-specific code.
- Every joined controller may send input. A button is pressed while any
  controller holds it and released when the last one lets go; a dial or
  switch takes the position any controller set most recently.
- Frames flow without backlog: the display's publication counter skips
  unchanged front buffers, a one-slot mailbox replaces frames not yet sent,
  and the encoder runs only when the sender asks for the next frame. Socket
  and network work never blocks guest execution, which runs on absolute
  virtual-time deadlines paced to wall time; a late slice catches up and
  never skips camera time.
- The MCP camera module opens every configuration `open` accepts, and
  `camera_show` puts any open camera in the desktop window.
- `fujimod_emulator_gui::Run` is what a window needs from a host: the
  session, one guest slice, new LCD frames and a final report. `Live` runs any
  `Camera`; the XP5 `RunHost` adds its startup actions. `fujimod-emulator
  --gui` uses `Live` for every processor except XP5 boots, which use
  `RunHost`.
