# Fujimod

See [browser boot profiling](../../crates/emulator/BROWSER_PERFORMANCE.md) for
worker CPU captures, Wasm symbols, instruction rates and repeatable timing.

This app lives in `apps/web/` of the FujiMod repository. Run the commands
below from that directory.

Try camera firmware and build SD boot files in your browser. Firmware is
identified by its complete SHA-256 and never uploaded. The web app is developed
on review branches; the Pages workflow deploys approved main builds only.

## Rendering and static hosting

`@solidjs/vite-plugin` runs in native start mode with `ssr: true`. The plugin
owns production entries and hydration; `src/App.tsx` selects the pages
and `src/Document.tsx` owns their HTML document and metadata. Browser-only emulator, USB and LCD work starts after hydration. Engine/input protocols
remain separate from the Solid components that display their state. The
[emulator UI package](../../packages/emulator-ui/README.md) owns the reusable
session and workspace; `src/home/controller.ts` owns homepage policy.

`npm run build` renders `/`, `/dev-tools/` and `/bug/` through start mode's
production request handler. Deploy **`dist/client/`**, which contains the full
page HTML, CSS and browser assets; static hosting needs no running SSR server.
`dist/server/` is a build-time renderer. `npm run build:local-preview` uses
`dist-local-preview/{client,server}`. Both build commands fail if a page cannot
be prerendered. The initial camera catalog and release metadata enter the build
from the installed public artifacts; the browser refreshes them after mounting.
Firmware selection and execution remain browser operations.

`npm run test:prerender` checks the production static files in Chromium and
WebKit, with and without JavaScript. It asserts page content, hydration of the
existing homepage nodes, Features and Feedback, and the report page's
absence of development reporting hooks. The web workflow runs this after build.
For local development, the generated document loads simulator reporting and
browser diagnostics only on the homepage and Dev Tools; `/bug/` stays local.

Development rendering uses `src/development/` entries. Each lazy page's preload
and hydration import share a URL derived from its source content and server
session. Safari can retain preloaded responses without revalidating `no-cache`
([WebKit 193533](https://bugs.webkit.org/show_bug.cgi?id=193533)); reusing an
unversioned page URL after editing its HTML structure can therefore stop
hydration. Development HTML and modules use `no-store`; production keeps the
generated entries and hashed build assets. `node scripts/test-development.ts`
checks mobile Chromium and WebKit against a running development server, including
a retained stale unversioned page response, repeated visits, and working inputs.
Set `FUJIMOD_WEB_URL` to check another development endpoint.

## Dev Tools

`/dev-tools/` is a Solid 2 USB console using the public `fujimod` JavaScript/Wasm
package and nusb's WebUSB backend. Connect opens the browser's FF80 device
picker. A single bulk interface is selected automatically; multiple interfaces
require a choice. WebUSB needs a supporting browser and HTTPS.

The connection target selects Camera · USB or Simulator. Simulator runs the actual
firmware in a dedicated worker. Its WebUSB device forwards descriptor, control
and bulk transactions to the emulator's USB controller; nusb and the Rust FF80
core process the same bytes as a hardware connection. The page's physical
`navigator.usb` stays intact. Firmware is selected by its full DAT hash and must
have a registered `emulate.usb` runtime and checked scenario. Locally configured
DATs can be used directly; otherwise Connect simulator opens a DAT chooser.

Commands are printable ASCII with a selectable line ending. Responses and
connection events share one log pane. Pending responses are polled for up to
ten seconds, then wait for an explicit Read response. Unknown progress or a
failed transfer blocks further commands. Disconnect closes a healthy idle session. During an interrupted or pending
operation it abandons host ownership without issuing a camera close or mode request.

Layer filters (Console, App80, FF80, USB) and severity filters affect display,
including previously captured entries. Rust records protocol boundaries and raw
USB transfers with shared sequence numbers. Expand a row for bytes and exchange
IDs; Save exports the captured log. The Rust queue and browser log are bounded,
and discarded entries are reported. Long rows scroll inside the log on mobile.

```sh
npm run build
npm run test:dev-tools:browser
# Or check an existing server:
FUJIMOD_WEB_URL=https://your-host.ts.net/ npm run test:dev-tools:browser
```

This checks production Wasm against a simulated WebUSB device in desktop/mobile
Chromium, plus the unsupported-browser state in WebKit. It never opens hardware.

The firmware integration check uses that same simulator connection, with physical
USB forbidden in the page. It cold-boots firmware and checks the stock console:

```sh
FUJIMOD_WEB_URL=https://your-host.ts.net/ node src/dev-tools/browser-emulator.ts
```

## Camera viewer

The TypeScript web app uses SolidJS 2 and a 2D canvas for the camera LCD.
The web app does not build, load or ship the standalone Bevy simulation, its
Wasm modules, camera model or environment assets.
Vite Plus provides the development server, builds, Oxlint, Oxfmt and Vitest.
Solid components own the homepage, camera controls, SD browser, viewer overlays, dialogs and status displays. Drop a
DAT anywhere on the viewer, or choose it there. The whole-file SHA-256 selects
`public/catalog.json`: firmware ID, body, model, optional engine artifact and
optional device controls. Unrecognized files stay on the neutral camera silhouette;
recognized bodies without installed capabilities show unsupported. Patching
and emulation are independent capabilities. Release-only patch entries also
work without a web app source change.

The homepage opens **Your cameras** without loading emulator code, creating its
worker or downloading its runtime. The engine is imported only after launching
supplied firmware or opening a saved camera. Loading shows each runtime asset's
status and consumed bytes, with percentage, transfer rate and estimated remaining
time when a decoded total is available. Completed assets stay visible during
initialization. Waiting for headers, module imports and guest setup use
indeterminate progress. The overlay clears when the camera is ready or loading
fails. `src/home/lazy-loading.browser.ts` holds the real engine import and Wasm
binary download to check deferred loading in Chromium and WebKit.
`src/home/downloads.browser.ts` checks measured transfer progress during a
bandwidth-limited cold boot and the deterministic preview route.

Firmwares declaring catalog `emulate.sceneInput` expose the webcam button beside
Viewer settings. It requests video only and sends bounded RGB frames through
`Engine.setScene`, waiting for each submission before capturing another. Disabling
it restores the default scene. Reset, restart, faults, page hide and unload stop
capture and release all tracks. `src/home/webcam.browser.ts` checks the virtual
Chromium camera against actual guest LCD pixels, then verifies capture stops and
the default scene returns. Both browser checks take `FUJIMOD_WEB_URL` and
`FUJIMOD_TEST_DAT`.

Uploading a supported DAT creates a separate virtual camera record. Its firmware
stays in IndexedDB. The camera binding supports live execution, same-owner Undo and portable durable media
saves. Each reopen cold boots committed storage and persistent physical controls. Save failures
are shown in the library and preserve any previous successful transaction.

`@fujimod/emulator-ui/engine/api` is the web app's emulator boundary. The standalone Rust Wasm
worker owns execution, pacing, request ordering and native history. Rendering
and panel code contain no register addresses or firmware bindings. Inputs,
labels, support and positions come from native `state.inputs`. Layout, printed
faces and gestures belong to `devices/x70/panel.json`; icons and badge styling
belong to `packages/emulator-ui/src/feedback/presentation.json`. The X70 panel uses semantic body
control IDs throughout. A firmware catalog supplies the checked Wasm artifact
bundle; there is one engine implementation.

`@fujimod/emulator-ui/viewer/lcd` composes actual guest LCD frames and fault feedback. Native
card state and FAT files supply the SD browser. Unsupported lifecycle operations
return their native error. Scene input and preview timing are applied through
the graph's declared native policies.

### Default scene

With no selected scene the camera sees the Elogio del Horizonte in Gijón,
photographed by the project's author with an X70.

- **Original:** `src/engine/assets/default-scene.raf.zst`, the camera's RAF
  compressed losslessly with Zstandard (Git LFS). It is never edited.
- **Derived file:** `scripts/assets/default-scene.ts` decompresses it to
  `build/assets/default-scene.raf` (ignored), skipping the work when the
  output's SHA-256 already matches the recorded original. Native runs and tests
  load it through `fujimod.scenes.default_scene_raf()`, which runs the script
  when the file is missing or stale. The same script writes
  `build/assets/default-scene.raf.gzipdata`, which the website serves (Cloudflare
  assets are limited to 25 MiB; the RAF is 32 MiB) and the browser decompresses
  with `DecompressionStream`.
- **Sensor input:** the RAF reaches the sensor through the RAF input (per-site
  linear radiance on the sensor's color filter pattern, normalized by the
  RAF's recorded exposure), not the RGB input. The browser `Scene` is a union
  of an RGB image (selected images, webcam) and a RAF; the worker's `scene`
  action passes RAF bytes to the Wasm camera's RAF input.
- `default-scene.png`, the camera's JPEG with the deepest shadows lifted,
  remains only for bodies whose sensors have not been checked against an X70
  RAF.

The model is **CAMERA ( Fujifilm X70 )** by
[McCLOUD2101](https://sketchfab.com/McCLOUD2101),
[original model](https://sketchfab.com/3d-models/camera-fujifilm-x70-5fd32c3c004f4fe4aecaaab8e2b89783),
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
The idle body preview derives from this model. The Git LFS model is retained in
`crates/simulation/assets/x70.glb` for standalone simulation work.
`src/home/viewport-controls.browser.ts` exercises real LCD frames and menu input,
both firmware modes, responsive layouts and the absence of 3D asset requests in
Chromium and WebKit. Its deterministic development preview is `?scenario=x70-lcd#workspace`.

Every advertised camera requires a prepared graph and a passing browser workflow.
“Try emulator” selects a configured local DAT or opens the DAT chooser.

### Develop

Use the Node version in the repository's `mise.toml` (`mise install`),
`git lfs install --local`, `git lfs pull`, then:

```sh
rustup target add wasm32-unknown-unknown
npm --prefix ../.. ci
npm --prefix ../../packages/emulator-ui ci
npm --prefix ../../bindings/javascript ci
npm --prefix ../../bindings/javascript run setup:tooling
npm ci
npm run dev -- --port 5190
```

The web commands build the local Rust USB bindings before starting. The tooling
setup downloads the official wasm-bindgen CLI matching `Cargo.lock` and verifies
its published SHA-256. `WASM_BINDGEN` can instead select an installed CLI.

Vite binds to `127.0.0.1`. To use existing research artifacts locally, put
absolute paths in ignored `.env.local`:

```text
FUJIMOD_RUNTIME=/path/to/staged/runtime
FUJIMOD_PATCHES=/path/to/patch-release/artifacts
FUJIMOD_DAT=/path/to/local/FPUPDATE.DAT
```

The staged runtime directory contains `catalog.json` and firmware-ID folders
with `camera.wasm`, `camera.mjs`, `wasm.json` and
`runtime-NOTICES.txt`. A release preparation stages this tree into
`public/runtime/`, which Vite also serves directly without a mount. These mounts exist only in
the dev and preview servers. Setting `FUJIMOD_DAT` enables automatic local boot; without it,
the viewer remains a DAT drop target. Public builds omit the automatic DAT fetch. The local DAT is served from its
opt-in mount and is never copied into any build artifact.

```sh
npm run check
npm test
npm run build
FUJIMOD_WEB_URL=https://your-host.ts.net:port node tests/wasm.browser.ts
FUJIMOD_WEB_URL=https://your-host.ts.net:port node tests/wasm-camera.browser.ts
node tests/speed.browser.ts --url 'https://your-host.ts.net:port/?scenario=x70-lcd#workspace' \
  --engine chromium --output build/fresh-speed-chromium
```

`wasm.browser.ts` runs generated ARM instructions, DAT verification, fault,
card and scene contracts in Chromium and WebKit. `wasm-camera.browser.ts` uses
explicitly configured local camera inputs to assert real REC pixels, physical
MENU, a shutter dial change, scene pixels, card contacts and native Undo.
`speed.browser.ts` measures 30 wall seconds after 25 guest seconds of warmup,
with native history enabled. It requires 0.8× guest/wall speed and at least
9.5 source and presented LCD frames per guest second. Run engines sequentially
on a quiet host. A missing operation or failed native postcondition fails the gate.

Set `FUJIMOD_WASM_CAMERA` to the prepared `wasm.json` and `FUJIMOD_WASM_DAT` to
the matching DAT for the development camera test route. Those files are served
only from their configured paths and are never copied into the public build.
The ordinary `?scenario=x70-lcd#workspace` development route loads the configured
local DAT into live REC with deterministic RTC input and normal Engine calls.
It executes the real guest; no screen state or peripheral result is synthesized.

Tests stay next to the code they cover. `npm test` runs the emulator UI package
tests and then the web app tests. `.test.tsx` tests use jsdom, and `.test.ts`
tests use Node. Import test APIs from `vite-plus/test`; call Solid 2 `flush()`
before asserting batched UI changes. Use `npm run format -- path/to/file.tsx`
for selected files. The runtime and DOM package are pinned to Solid 2.0.0-rc.15.
See [the engine contract](src/engine/WASM.md) for transport and history behavior.

### Local production preview

```sh
npm run build:local-preview
npm run preview -- --mode local-preview --port 5190 --strictPort
```

This serves `dist-local-preview/` and enables the configured local DAT. Public
builds omit automatic DAT loading. The server binds to `127.0.0.1` and accepts
`*.ts.net` Host headers for Tailscale Serve. Restart after environment changes.
Native same-owner Undo and durable media restoration use the camera binding. Durable
saves contain committed SD/NAND media and physical controls; volatile execution
state starts fresh at each cold boot.

### Website assets and deployment

The Web checks workflow runs lint, type checks, tests and the public build on
web changes in pull requests and on main. It does not deploy.

`npm run deploy -- --runtime DIRECTORY --patches DIRECTORY --media DIRECTORY`
validates locally generated assets, runs checks and tests, builds the website,
and uploads it directly to Cloudflare. Runtime bundles and feature media remain
in ignored generated directories. GitHub Releases contain mod patches and their
index; publishing a mod does not deploy the website.

`scripts/stage-site.ts` requires a complete qualified runtime for every advertised
emulator, verifies mod patches, and records asset hashes. A failed preparation
preserves installed assets. See [DEPLOYMENT.md](DEPLOYMENT.md) for the full command.
Neither a firmware DAT nor a boot ROM is a website build input.

## JavaScript API

```js
import { patch } from './src/patch/patcher.ts';
const { ffboot, ffsys, metadata } = await patch(baseStockDatFile, compiledPatch);
```

Inputs are a File, Blob, ArrayBuffer or Uint8Array. Outputs are Uint8Arrays.
This API does no network or filesystem I/O. It verifies the stock input,
command stream and both final files with SHA-256. The page additionally checks
the downloaded patch against the index. Work runs in a module worker; the
page provides a ZIP download and separate download links for both binaries.

## Patch format: FJMODP01

All integers are unsigned little-endian. Eight ASCII magic bytes `FJMODP01`,
then a u32 JSON metadata length, then UTF-8 JSON, then a binary command stream.
Metadata declares schema 1, source byte count and SHA-256, output sizes and
SHA-256 (`outputs.ffboot`, `outputs.ffsys`), command count and stream SHA-256.
It also identifies the firmware, release and included mods.

Each command has a 20-byte header:

| Byte | Field                                                  |
| ---- | ------------------------------------------------------ |
| 0    | Opcode: 1 COPY, 2 FUJI_LZ, 3 WRITE                     |
| 1    | Output: 0 ffboot, 1 ffsys                              |
| 2    | Flags: bit 0 XOR source with 0xff; all other bits zero |
| 3    | Reserved, zero                                         |
| 4    | Destination offset, u32                                |
| 8    | Output byte count, u32                                 |
| 12   | DAT source offset, u32 (zero for WRITE)                |
| 16   | DAT source byte count, u32 (zero for WRITE)            |

COPY reads DAT bytes, optionally XOR-decoding them. FUJI_LZ uses Fuji's
method-4 stream (not the LZ4 codec): a five-u32 header then literal runs and
11-bit-distance/4-bit-length matches, including overlapping matches. WRITE
is followed immediately by its literal bytes, with flags zero. Outputs start
zero-filled; commands execute in order, so later WRITE operations replace
reconstructed stock bytes. Stock firmware comes from the supplied DAT;
patch literals contain compiled changes and short gaps coalesced between them.

Both parsers reject invalid commands, bounds, malformed streams, wrong inputs,
trailing bytes and mismatched output hashes. Versioned magic permits future
format changes while existing pages handle any firmware represented by these
operations. A genuinely new codec would require an interpreter update.

`patches.json` has schema 1, a release tag, `patches: {SHA256: filename}` and a
`firmwares` array. Each entry supplies model/version, source hash/size, Fuji
download page and original release date, patch hash/size, included mods and model-specific boot instructions. Patch filenames are simple
local filenames. Pages serves index and patches from the same origin.

`report.json` includes the user's comment and the complete issue URL and prefilled
fields (including labels, firmware ID, version and simulator version). The ZIP
and the GitHub link use the same submission object; GitHub indexes the capture.

In published builds, report delivery saves the latest report ZIP and its issue
URL in the browser's IndexedDB before starting the download and opening GitHub. “Download saved
report” recovers those same bytes after navigation or a reload. If a popup is
blocked, the dialog also retains an ordinary “Open GitHub” link. Reports stay
local to that browser and origin; clearing site data removes the saved copy.
The published-delivery regression removes development upload hooks, replaces
GitHub navigation and makes `window.open` navigate the current tab; the fault
comes from real replay execution:

```sh
python tests/feedback.py --url 'https://your-host.ts.net:port/?scenario=x70-movie' \
  --output build/fresh-feedback-proof
```

See [the simulation crate](../../crates/simulation/README.md) for environment
assets, native scenario rendering, input semantics and the current runtime boundary.

## Browser bug reports during development

The development server saves feedback and detected simulator, worker and JavaScript
faults from Emulator and Dev Tools under `browser-bug-reports/<report-id>/`. `metadata.json`
is written first; `report.zip` adds guest state, runtime identity, recent inputs
and logs. The ZIP’s `report.json` is saved alongside it for direct inspection.
Dev Tools includes binary USB traces. Unavailable snapshots are recorded
explicitly. Inspection snapshots preserve the captured boundary and are not resume
points. Release builds keep the browser download and GitHub issue workflow.
