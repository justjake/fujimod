# Browser engine

The web app has one engine: the standalone Rust WebAssembly module, owned by a
module worker. `createEngine(onError)` exposes the typed Engine API. All CPU,
device, control, card, scene and history operations are methods of the generated
`Camera` binding.
The production input table comes from the native `state.inputs` result.

`wasm.json` selects the module, binary, checked graph, guest tick rate, initial
run boundary and LCD output. The DAT is supplied separately and verified by the
module. A failed construction or unsupported native operation is an error.
`dmpCpuRenderer: true` selects the CPU DMP renderer. When it is `false`, the
worker loads `dmp_webgpu.js` and `dmp_webgpu_bg.wasm` beside the Emscripten
module. The Rust GPU renderer receives command lists and synchronous guest RAM
reads through the worker bridge; guest execution stays paused until its async
WebGPU readback is returned. When omitted, the worker selects the CPU renderer.

The worker paces bounded runs against the guest clock. It presents borrowed
RGBA through OffscreenCanvas and transferred ImageBitmap, or leases a shared
Wasm view until the synchronous frame listener returns. Metrics count native
source events separately from delivered LCD frames. Host inspection never
advances the guest.

In-memory history IDs belong to their session. The worker retains eight explicit checkpoints
at two guest-second intervals; Undo uses native rollback, falling back to a
retained checkpoint when the finer native history has expired. CBOR history is
opaque to JavaScript. Checkpoints are distinct from durable camera saves;
portable media CBOR contains committed SD/NAND bytes, power state and persistent
physical controls, with a checked reset image applied before boot. Cold power/restart
expires volatile checkpoints. Held boot contacts apply before the first instruction
and release through the native queue after the declared boot interval. Card-file
restarts merge into existing media; formatting replaces its filesystem.

Development proof routes are opt-in. `/__wasm-proof` runs generated ARM fixtures.
`/__wasm-camera` reads explicitly configured local artifacts. `/__wasm-lifecycle`
executes the owned save/reload, shutter/Playback and cold media workflow.
These routes are development-only. The ordinary app supports deterministic
development scenarios through `?scenario=x70-lcd` with a configured local DAT.

Downloadable reports identify large buffers by hash and a bounded preview while
retaining small register arrays and scalar diagnostics. They also include a
`snapshot.emusnap` archive: the complete stopped machine, shared device
resources, media, queued inputs, power state, presentation frames, and exact
constructor document and binary inputs. After a fault the exporter chooses the
newest healthy retained point, falling back to its reset point; `report.snapshot`
records the chosen boundary and rejected candidates. Export does not rewind the
live session.

`Engine.exportSnapshot()` exports the current point. A fresh engine loads the
bytes with `loadSnapshot(bytes, config, progress)`. The configuration supplies the
runtime bundle and presentation options; construction reads its graph and firmware
inputs from the archive. Loading defaults to paused replay mode. Select
`execution.mode: 'live'` to resume pacing. No cold boot, media replacement or scene
input is applied while loading. The runtime validates the archive schema, binary
hashes, execution topology, processor model, clocks and device configuration.

The JavaScript binding exposes `Camera.fromSnapshot(bytes)` and
`Camera.exportSnapshot()`. Node callers can use `openSnapshot(modulePath, snapshotPath)`.
The development route `/__wasm-snapshot` verifies a report archive against fresh
worker restoration and continued execution. Its browser check downloads the file,
closes the source page, opens `/__wasm-snapshot?load`, and loads the file from disk
in Chromium and WebKit. Firmware is not fetched during that load.

The camera browser check requires `FUJIMOD_WASM_EXPECT_EXECUTION`, a JSON
execution receipt for the identical manifest, DAT, boot tick and LCD output,
and `FUJIMOD_WASM_DAT_SHA256`. It compares per-processor counts and every
initial displayed pixel before testing controls, scene, media and history.
The receipt carries `created`, `error`, `status.ticks`, `status.cores`,
`status.failed`, and the frame's `width`, `height` and RGBA `sha256`.
