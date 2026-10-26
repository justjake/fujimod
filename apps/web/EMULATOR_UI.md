# Emulator UI

The emulator UI (viewer, control panel, card browser, fault and report UI,
status) is one package used by every frontend: the site's homepage, Dev Tools
and a small standalone frontend served by a native emulator process.

## Boundaries

- The UI drives an emulator only through `Engine`
  (`packages/emulator-ui/src/engine/api.ts`).
- `WasmEngine` runs the emulator in a browser worker. `RemoteEngine` reaches a
  native emulator process. Both send the same messages: the worker protocol
  becomes the wire protocol, defined once and used by the worker, the remote
  engine and the native process.
- A session controller owns one running emulator: boot, inputs, card, faults,
  checkpoints and Undo, using only `Engine`. The homepage adds its own parts
  around it: saved cameras, catalog, firmware choice and site pages.
- Control layouts are data (`DevicePanel`). The same renderer draws them in
  every frontend.

## Homepage emulator presentation

- Idle: the launcher (`src/cameras/Launcher.tsx`) offers Resume and New for
  saved cameras, the add-camera steps, and the supported list with a download
  button linking each body's Fujifilm DAT.
- Loading: one list of technically named steps, each with the same bar and no
  rate or ETA text: Verifying firmware, Runtime manifest, Camera emulator,
  Runtime loader, Patching firmware (FujiMod boots only), Building camera,
  Booting camera. Rows exist before they start; a waiting row has an empty bar
  and no label. A step without a measured size uses an indeterminate bar.
- Running: a short bar under the display holds the camera name, a firmware
  picker (FujiMod or Unmodded), smoothed speed, webcam input and Feedback.
  There is no overflow menu. Save & Exit sits in the Emulator heading row.
- The emulator section keeps one set of colors, type sizes and controls; Dev
  Tools and the rest of the site are outside this scope.

## Transport

WebRTC only. A native emulator process serves the UI bundle over HTTP, and the
same server exchanges the WebRTC offer and answer. Remote connections use the
existing share-link signaling. Control and card messages use a data channel;
the LCD is a video track. The native and browser peers are the ones in
`crates/usb-proxy`.

## Control layouts

A layout file is a versioned `DevicePanel` naming its body. Presets ship per
body; bodies without one get a layout generated from the emulator's input
list. Import checks every bound control against that list and reports unknown
ones. The editor edits the same tree in place on the live panel and exports
it as JSON. User layouts are stored with the saved camera.

### Default layouts

`defaultLayout(inputs, controls)` builds a panel from supported session inputs and
hand-authored body facts. The catalog owns every control's printed text, icon,
and short name. Mechanisms determine D-pad and command grouping; input support
and positions determine what is interactive. Explicit device panels retain their
node layout.

The panel, power-on combinations, Recovery, and service-mode guide share the
same drawing components. See [Control markings and mini controls](../../packages/emulator-ui/CONTROL_MARKINGS.md)
for the catalog rules, mapping, shared faces, and checks.

### Editor

- Entry: **Edit layout** in the viewer's ⋯ menu. The panel shows no editor
  controls until then.
- While editing, the viewer is not sticky on narrow screens and the panel
  scrolls into view. An editor bar at the top of the panel holds Export,
  Import and Done, and stays visible while the panel scrolls.
- Tapping any control (dial, segment, key, shutter, D-pad, command control)
  selects it and does not reach the emulator. The inspector under the editor
  bar edits its caption, presentation (`compatibleVariants`) and bound input.
  D-pads and command controls show one input picker per slot. The input picker
  lists every input; unsupported ones are disabled.
- Unknown ids are listed after an import, and for the active layout when it
  has any.

### Code

- `ui/layout.ts`: the `DevicePanel` types, import/export and checks, plus pure
  tree functions `nodeAt(panel, path)` and `replaceAt(panel, path, node)` with
  `type NodePath = readonly number[]`.
- `ui/default-layout.ts`: `defaultLayout`, `compatibleVariants`, and
  `groupInputs(inputs, controls): InputGroup[]`, where an
  `InputGroup` is a pad, a command control or a single input.
- `ui/layout-editor.ts`: `LayoutEditor` owns the edited panel and selection:
  `editing`, `selected`, `begin()`, `end()`, `select(path)`,
  `update(path, patch)`, `bind(path, slot, inputId)`, `importJson(text,
  inputs)`, `exportJson()` and an `onChange(panel)` listener. Editing state
  lives only here; `ControlSession` keeps input and rendering state.
- `ui/LayoutInspector.tsx`: the editor bar and inspector.
- `ui/controls.tsx`: the one renderer. `Node` takes an optional editor and its
  path for selection; nothing else differs while editing.
- The homepage controller creates one `LayoutEditor` per active camera and
  saves its changes to the camera store.

### Checks

- `default-layout.test.ts` runs `defaultLayout` over a fixture of every catalog
  body's input list. It checks that no unsupported, touch or latch input
  appears, that each D-pad and command group becomes one control, that face
  limits hold, and the D-pad centers on the X20 (`selector_button.press`) and
  X70 (`menu_ok_button`).
- A phone-sized (390×844) browser check on the X-A2 and X70 enters through the
  ⋯ menu, selects a key, the D-pad and a dial, edits a caption, exports,
  imports and reloads.

## Order

1. Complete: split `src/home/runtime.ts` into the Engine-only session controller
   (`packages/emulator-ui/src/session/controller.ts`) and homepage code
   (`src/home/controller.ts`). The old runtime module is deleted.
2. Complete: move the UI into [`@fujimod/emulator-ui`](../../packages/emulator-ui/README.md).
   The homepage imports its workspace, session, viewer, controls, status,
   faults and reports. The Wasm engine implementation stays in the web app.
3. Layout schema version, import/export and binding checks are complete. Next:
   the default layouts, editor and checks described above.
4. `RemoteEngine` over WebRTC, and the native process serving the UI.

## Refactor rules

Each step is a real change of structure, checked in review:

- Code moves; it is not copied. Old modules are deleted in the same commit,
  with no re-exports, wrappers or aliases left behind, and every importer
  updated.
- One implementation per concept: one session controller, one protocol
  definition, one layout renderer. A second frontend reuses them; it does not
  get its own variant.
- No behavior changes ride along with a move. Existing unit and browser checks
  pass unchanged, except for import paths.
- Each step's commit message lists the deleted modules. Review checks that no
  remaining file imports them and that no moved function exists in two places.
