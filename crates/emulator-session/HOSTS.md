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

XP5 bodies use the same path. Getting there takes four changes:

1. **Wires from the body's ladders.** For most X-T5 controls the firmware's
   located inputs name a key but carry no electrical predicate; the bands are
   in the body's `ladders` table (`catalog/bodies/xt5/controls.json`), which
   today only the XP5 key model reads. `fujimod_catalog_wiring::locate`
   builds each such input's wire from its ladder rung: an ADC rung becomes an
   ADC band on its channel, and a GPIO connection becomes a GPIO wire.
2. **The power controller's ladder registers are analog inputs.** Registers
   52 and 53 of the X-T5 power controller report 10-bit levels of two button
   ladders. The XP5 SPI controller offers them as `AnalogInputs` channels of
   its power-controller part, and the panel's analog target names its
   converter node, so a body can have more than one analog owner.
3. **XP5 devices accept panel signals.** The XP5 ADC implements
   `AnalogInputs`; the XP5 GPIO controller implements `GpioInputs`. The XP5
   session definition lists its device nodes by kind and name, which is what
   `fujimod_emulator_controls::Nodes` reads.
4. **No second key model.** Once the panel drives every XP5 control, the
   ladder and resistor key models, `Inputs::key` and `Inputs::set_position`
   are deleted, and key scripts (`--keys`) name catalog control positions.

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

- The desktop host serves any `Camera` on a loopback WebSocket using the
  protocol in `crates/emulator-gui`. It reads the control catalog and
  positions from the session and sends inputs to it; it has no
  processor-specific control code. Socket reads never block guest execution,
  and an LCD frame is encoded only after it is known to differ from the last
  one sent.
- The MCP camera module opens every configuration `open` accepts, and
  `camera_show` puts any open camera in the desktop window.
- `fujimod_emulator_gui::Run` is what a window needs from a host: the
  session, one guest slice, new LCD frames and a final report. `Live` runs any
  `Camera`; the XP5 `RunHost` adds its startup actions. `fujimod-emulator
  --gui` uses `Live` for every processor except XP5 boots, which use
  `RunHost`.

## Remaining

- Delete the XP5 ladder and resistor key models (`resistor_keys`, the X100VI
  key inputs and the wasm `Body::Xp5` path) once every XP5 body's controls
  compile to a panel.
