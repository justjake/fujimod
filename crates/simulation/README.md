# Camera environment simulation

Shared Rust/Bevy 0.20.0 world, physical camera rig and observer rendering.
The web app currently uses LCD-only rendering. This standalone crate retains
camera model rendering, picking, dial pivots and firmware LCD textures.
Environments live beside the implementation in
`environments/`; native and browser rendering consume the same prepared asset.
There are no device-specific quality variants.

## Interaction and time

The player is 1.75 m tall, with eyes at 1.64 m, and holds the physical camera
at 1.45 m, 0.40 m forward and 0.055 m to the right when looking level.
The head glances down in Walk mode to frame the camera slightly right of center,
with its bottom about 5% above the viewport edge. Framing follows the viewport
aspect ratio and remains stable while looking up/down or turning. The camera
is a physical world object; entering Inspect moves only the observer.
In Walk mode, WASD moves at 1.4 m/s and captured mouse movement turns the player
and camera. Right click enters Inspect, releases the mouse and moves the
observer close to the camera's rear glass. Dragging orbits the observer; the
physical camera pose and sensor image stay fixed. Right click returns to Walk.
Browser hosts can issue the same navigation commands.
Focus loss clears movement. A spatial index of the static scan supplies ground following, wall checks and floor-gap boundaries, independently of rendering visibility. The controller is in Rust; browser event capture
and DOM controls belong to the embedding host.

Interactive movement uses real elapsed time, independently of guest execution
speed. `Simulation::advance(seconds)` provides explicit time for scenarios.
Native rendering can use Bevy's `TimeUpdateStrategy::ManualDuration`, as in
`examples/render_scenario.rs`. Observer animation never advances firmware.
`Bridge::snapshot` and `Bridge::take_sensor` expose output and never advance the
world. GPU readback completion is asynchronous; a frame retains its render
request's world time, sequence and physical camera pose.

The observer sees world layer 0 and camera-body layer 1. The sensor sees only
layer 0. It renders a 768×512 image using physical focal length/sensor size and
Bevy's depth-of-field effect on native renderers. Both browser backends omit the depth-of-field post-process. Focus/aperture are explicit scenario inputs through
`Command::Optics`. Physical body width and initial optics come from the selected
body definition. The generic headless default is a declared 50 mm / 24 mm sensor-height
profile; it is not a camera calibration. These are declared approximations, not recovered
actuator calibration. No firmware PC, private RAM or software mode determines
world behavior. No autofocus decision is supplied by the environment.

The web firmware engine runs in a separate Pyodide worker. A host can submit
sensor frames through its RGB image-input adapter. The world and renderer
share one Bevy world; firmware execution remains a separate runtime. Native callers
can consume `SensorFrame` directly. Interactive sensor submission renders again when the world, physical pose or
optics change. The emulator retains that source between updates. Native scenario
rendering samples each requested interval, holding scenario time fixed until
GPU completion. Submission is bounded to one pending readback and one latest
completed frame; display rendering is independent.

## Assets

Crouch End Station is installed for standalone rendering. The same
GLB is used on every device and by native scenarios. It contains 347,393 faces,
512,089 vertices and a 2048 px photographic atlas (20,055,032 bytes). The scan has
incomplete geometry outside its captured coverage; walking stops at floor gaps.
Source units are treated as meters, with explicit placement and a reviewed
street spawn. This is not a surveyed physical calibration.

To prepare a fresh authorized Sketchfab download, extract its glTF archive and
run the pinned offline asset tool:

```sh
npm ci --prefix crates/simulation/tools
node crates/simulation/tools/prepare-photogrammetry.ts SOURCE/scene.gltf FRESH_RUNTIME.glb
python3 crates/simulation/environments/crouch-end-station/prepare.py DOWNLOAD.zip \
  --runtime-scene FRESH_RUNTIME.glb --output FRESH_ENVIRONMENT \
  --scale 1 --spawn -5 0 0 --translation 0 -6.27 0
```

The tool preserves texture coordinates during geometry reduction, resizes the
photographic atlas once for all targets, and retains baked lighting through
unlit materials. Normals stay in the asset to prevent loader-side vertex expansion. Review orientation/yaw and place the camera inside the scan
before installing the result. The checked manifest records hashes and geometry
budgets; attribution records the modifications.

Download Crouch End Station's glTF archive through the authorized Sketchfab
Download flow. Download requires a signed-in account; the simulator never
scrapes the viewer or includes account credentials. See its attribution and
`prepare.py --help`. Preparation creates a fresh staging directory and records
source/output SHA-256s. Review scale, orientation and spawn before installing
`environment.json` and `assets/` into that environment's directory. The scene
must have meaningful meter scale before distance/focus tests are meaningful.

The glTF loader imports scene 0 through `WorldAssetRoot` and configures spawned
entities after `WorldInstanceReady`. Imported cameras/lights are disabled.
Photogrammetry can select unlit materials to preserve photographed lighting;
the camera body uses its PBR materials. LCD geometry gets independent UVs and
an unlit texture. No shader is needed for the initial optics implementation.

The web app does not package these assets. Use the environment manifest with
the standalone native renderer.
Large curated GLBs use Git LFS. Source downloads, conversion intermediates and
scenario output stay in ignored build/output directories.

## Build and checks

The simulation is its own Cargo workspace, with its own lockfile and target
directory, so Bevy's dependency features never reach the emulator's builds.
From the FujiMod repository, use its Mise toolchain:

```sh
cargo test --locked --manifest-path crates/simulation/Cargo.toml --lib
cargo check --locked --manifest-path crates/simulation/Cargo.toml --examples
npm ci --prefix crates/simulation/tools
node crates/simulation/build-web.ts
python3 crates/simulation/environments/crouch-end-station/test_prepare.py
```

The web app currently uses LCD-only rendering and does not build or load this crate.
Build its standalone Wasm modules explicitly with the command above. Generated
bindings and Wasm live under `bindings/javascript/pkg/simulation/`, never in
source control. The generated entry points are `fujimod/simulation/webgl2` and
`fujimod/simulation/webgpu`. The default build
command produces both variants; pass `webgl2` or `webgpu` to build just one.
The Cargo features have the same names and are mutually exclusive on Wasm.
Both browser backends use the same resolution, assets, lighting and effects.
Every browser build uses size optimization, thin LTO, and Binaryen. The script
caches generated bindings and optimization until the compiled Wasm changes.
Only the rendering features in use are enabled; text, animation, gizmos and
post-processing bundles are excluded from the browser build. Only the command
and scenario structs enable serialization. The optimized Wasm has a 64 MiB
build budget to catch accidental feature/bundle growth. The observer uses a small prefiltered studio reflection map, directional fill
and rim lights, HDR tone mapping, FXAA without MSAA, and no shadow maps. Bevy's
`MainPassResolutionOverride` limits the costly main passes to one pixel per CSS
pixel even when Safari/Winit allocates a larger backing canvas. Picking and DOM
projections explicitly map between the camera viewport and CSS coordinates.
Buttons retain a 28 px touch snapping radius (8 px mouse) with occlusion checks.
Dial decals are authored as part members and rotate around the same pivot;
drag direction follows the projected rotational tangent at the grab point. The LCD
upload reuses its dimensions and reads the existing frame buffer directly.
LCD-only mode disables the observer camera and its texture uploads. Imported texture pixels are released after GPU upload; meshes retain CPU data for picking and navigation.

After installing an environment, render a native scenario without a window:

```sh
cargo run --manifest-path crates/simulation/Cargo.toml --example render_scenario -- \
  crates/simulation/environments/crouch-end-station/environment.json \
  target/simulation-runs/a-fresh-run apps/web/devices/x70/parts.json
```

GPU image comparisons should use tolerances across adapters; exact pose and
command replay tests do not depend on GPU pixels.

## Implementation references

These are the version-matched upstream patterns used here, rather than older
Bevy tutorials or custom equivalents:

- [Bevy 0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
- [glTF loading](https://github.com/bevyengine/bevy/blob/v0.20.0/examples/gltf/load_gltf.rs)
- [glTF material overrides](https://github.com/bevyengine/bevy/blob/v0.20.0/examples/gltf/edit_material_on_gltf.rs)
- [Render targets](https://github.com/bevyengine/bevy/blob/v0.20.0/examples/3d/render_to_texture.rs)
- [GPU readback](https://github.com/bevyengine/bevy/blob/v0.20.0/examples/shader/gpu_readback.rs)
- [Externally driven rendering](https://github.com/bevyengine/bevy/blob/v0.20.0/examples/app/externally_driven_headless_renderer.rs)
- [Depth of field](https://github.com/bevyengine/bevy/blob/v0.20.0/examples/3d/depth_of_field.rs)
- [glTF texture/mipmap preparation](https://github.com/bevyengine/bevy/blob/v0.20.0/examples/large_scenes/mipmap_generator/README.md)
- [Sketchfab Download API](https://sketchfab.com/developers/download-api/downloading-models)

The browser uploads the camera before starting the environment load, so both
GLBs and their decoded textures do not peak together in an unshrinkable Wasm heap.
The X70 body uses a separately simplified GLB (4,631,148 bytes, 118,704 vertices);
all authored control nodes, materials, dial labels and pivots are retained.
`tools/prepare-camera.ts SOURCE.glb FRESH_RUNTIME.glb` reproduces that reduction
from the original asset (SHA-256 `1e6c2c6a30c47a9b30540682db5457c4fb51a7a7d9ffcc3a09d8c2f15c72306e`,
the version tracked before this optimization). Both tools use the same pinned
`tools/package-lock.json`.

On the development preview (2026-10-17, desktop Chromium, WebGL2, station plus
X70), the renderer's linear-memory high-water mark fell from 329,711,616 to
165,281,792 bytes (about 50%). This measures the unshrinkable Wasm heap, not
Safari process RSS or GPU allocations. The separate Pyodide emulator still
reserves 941,621,248 bytes, including guest RAM; these asset changes do not halve
the combined tab. Development diagnostics now record emulator memory at each
boot stage, including before a camera becomes ready. Phone stability still
requires verification on the actual device.

Startup checkpoint export now writes compressed blobs in 64 KiB input chunks
to a MEMFS file; the worker reads that file directly. It no longer retains the
whole ZIP as Python bytes before copying it into JavaScript. A desktop Safari
cold boot measured 818,151,424 bytes before checkpoint export and 880,345,088
after it, compared with the prior 941,621,248-byte completion measurement.
These remain linear-memory measurements and do not establish an iPhone memory
limit or confirm a memory kill. Boot diagnostics distinguish snapshot capture,
archive export and persistence. Download diagnostics record the filename;
automatic crash reports continue to upload in the background.
