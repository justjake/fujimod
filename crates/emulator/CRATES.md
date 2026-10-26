# Emulator crate layout

Cargo recompiles every crate downstream of a change. The goal is that an edit to
one device model rebuilds that model's crate and the final crates that link it:
under a second for the model's unit tests and two to three seconds for the
release Python extension, with emulation no slower than before.

## Target graph

From the bottom of the graph to the top:

1. `fujimod-emulator-cpu` and its backends.
2. `fujimod-emulator-devices`, the device interface. It holds the `Device`
   trait, bus, `Cx`/`Host`, graph, checkpoint, fingerprint, record, signals,
   work, map, math and the test host. It also holds everything that crosses a
   model boundary:
   - the capability traits sessions and bindings call on devices
     (`capability`);
   - handles that models share with each other and with sessions (pins and
     pads, pulse outputs, card contacts, encoder inputs, lens bridge registers,
     the lens focus, aperture and focus-axis position, the EXR II interrupt
     outputs and their routes, HDMI sinks, XP5 pins and panel clocks, scanout
     frames);
   - the NAND and SD card media that plans, sessions and controllers exchange.

   It contains no device implementation. Two crates sit in the same layer:
   `fujimod-emulator-storage` (host-side FAT services over card media) and the
   image interface `fujimod-emulator-imaging`: scene,
   sensor and image frames and their pixels, the handles and option and spec
   types plans name, image layout, statistics frames, exposure and optics
   declarations, film and packed color, and the EXR II ISP interface. Its
   types are concrete with private fields, except the sensor images and
   exposures an image frame carries, which are `SensorImage` and
   `SensorExposure` trait objects; `SensorDecoders` decodes their checkpoints
   and RAF files. The implementation is two model crates:
   `fujimod-emulator-image-formation` (exposure sampling, optical projection,
   radiometric rendering) and `fujimod-emulator-image-processing` (analysis
   laws, RAW development, resampling, separable filters, RAF reading, the
   scene's optical blur, the clocked frame source and the ISP parameter
   bank). The EXR II ISP input crate registers the frame source and parameter
   bank constructors and the sensor decoders.
   `fujimod-emulator-exr2-resources` holds the EXR II device wires, media and
   inputs, and the image part behind its `ImageResources` trait;
   `fujimod-emulator-exr2-images` implements the image part: the image wires
   (frame period, image grid, exposure, ISP parameters, memory outputs,
   streams, output channels, phase edges) and the image, statistics and
   sensor inputs, and installs the preparation that builds a run's image
   inputs. Only the models that read images, the EXR II board and the
   bindings that inspect image inputs name it, so an image interface edit
   does not rebuild the resources, the plan, the session, preparation, the
   boot crate or the models that name no image type.
3. Plan crates: each family's checked board description and construction
   interface, such as `fujimod-emulator-exr2-plan`. A plan carries each model's
   options as JSON for the model crate to decode, so an option change in a
   model crate never rebuilds the plan. Its `construction` module holds the
   build context, the wiring helpers and the registry of constructors by plan
   model kind. A plan depends on the device interface and the EXR II
   resources, not on the image interface.
4. Model crates, one per subsystem and about 4,000 lines or fewer, for
   example `fujimod-emulator-exr2-i2c`, `-exr2-display`, `-exr2-gpu`,
   `-parts-lens`. A model crate depends on the interface, its family's plan
   and, only to share behavior, on another model crate. Each family model
   crate's `construct::register(&mut Constructors)` adds the constructors of
   its models; a shared part is wired by the family crate that attaches it
   (the EXR II serial crate wires the lens unit, for example).
5. Board crates: validation, wiring, resources and DMA consolidation.
   `fujimod-emulator-preparation` sits with them: it builds a run's declared
   inputs (NAND media from persistent inputs and settings layouts, cards and
   card files, and through the image part's installed preparation its
   images, statistics and sensors) into the plan's resources, so a settings
   edit rebuilds neither the storage interface nor the emulator. They
   construct nodes through the constructor registry and depend on the interface
   and plan crates only.
6. `fujimod-emulator`: execution and scheduling. It depends on the CPU and
   device interface crates only. `fujimod-emulator-session` sits above it: a
   session holds a board with its EXR II resources, inputs, media, history and
   checkpoints, and reaches devices through capability traits and image inputs
   through `ImageResources`, never through concrete model or image types.
   `fujimod-emulator-boot` sits above both: it prepares runs from the reference
   catalog and run configurations, assembles the CPU graph and the X70 plan,
   and opens sessions.
7. Binding libraries (the Python binding crates, the JavaScript bindings)
   over the emulator and the interface. The Python bindings are one crate per
   area, each depending only on what it binds: `fujimod-python-base` (the
   errors, values, discovery and call snapshots, the XP4 backend hook the
   emulator bindings share, and the board session hook the remote bindings
   borrow a Python board's session through), `-catalog` (the reference catalog and corpus),
   `-service` (App80 service definitions, registered in the catalog module),
   `-usb` (USB sessions, tracing, workflows and firmware files), `-app80` (App80
   mailboxes and service commands, registered in the USB module), `-usb-base`
   (the USB exceptions and error conversion, and the session hooks through
   which the App80 and remote bindings lease or run operations on a USB
   session without depending on the USB bindings), `-recording`, `-remote` and
   `-emulator` (boards, run configurations, input assets and settings
   headers). Binding functions that call one family's model code live in that
   family's binding crate (`fujimod-python-graphics` for the graphics data
   path, EXR II GPU and scanout, `-xp4` for XP4 sessions, `-host-image` for
   host image algorithms), which sits with the final crates and registers its
   functions on the module the final crate builds.
8. Final crates of a few lines each: the Python module (`_native`), the CLI
   and worker binaries, and the JavaScript module. Each links everything,
   registers constructors, board builders and family bindings, then starts.
   `fujimod_emulator_models::register()` registers every model crate's
   constructors, so final crates make one call for them.

With this graph, a model edit rebuilds the model crate, the models
registration crate and the final crates. An interface edit rebuilds
everything, so the interface holds declarations and small shared handles, not
behavior that changes often.

## Rules

- Below the final layer, only model crates depend on model crates.
- Emulation speed does not rely on LTO. Small functions called across crates
  on hot paths (bus access, `WordMap`, `Cx`/`Host`, interrupt lines, handle
  accessors) are `#[inline]` or generic. Every step compares host
  instructions retired for the X70 boot and the stock button scenario before
  and after; an increase is fixed before the step lands.
- Tests stay with the code they test. Tests that need models from several
  crates live in the board crate, or in a test crate above the models. Test
  support shared by several model crates goes in the interface's test host or,
  when it installs process-wide state such as a global allocator, in a
  test-only crate (`fujimod-emulator-exr2-isp-testing`).
- Every crate depends on `fujimod-workspace-hack` (`cargo hakari manage-deps`).
- No re-exports, aliases or shims are left for old paths; callers change in the
  same step.
- `crates/crate-graph` checks these layers in the workspace tests: every crate
  has a layer in its `layers.json`, only model and final crates depend on model
  crates, a model crate has at most 4,000 lines outside its tests, the
  interface implements no device, and a library sets `test = false` exactly
  when it has no unit tests, so `cargo test --workspace` links no empty test
  harness and skips no test. The violations that remain until the steps
  below are done are listed there; an entry that is no longer true fails, so
  the list only shrinks.
- An interface edit rebuilds every crate above it, about 100 units: rebuilding
  all workspace tests then takes 18 to 20 seconds on a 10-core machine. A model
  edit rebuilds 6 or 7 crates (about 7 seconds), and either edit republishes
  the Python extension in 2 to 7 seconds. Merging crates to shorten the
  interface case would slow these, so the interface keeps only what several
  models share.

## Capabilities

The interface's `capability` module declares what sessions do with devices:
`AnalogInputs`, `SerialStatus`, `TouchPanel`, `HdmiOutput`, `UsbPort`,
`NandStore`, `CardSlot` and `ScanoutSource`. `Device` has a default accessor
pair per capability (`card_slot()`, `card_slot_mut()`, ...) returning `None`;
a model that offers the capability returns itself. The board's
`inspect_capability`, `edit_capability` and `capability_input` find a node's
capability, and `device_save` reads any device's diagnostic state. Sessions no
longer downcast to models; generic `inspect_device::<T>` remains for tests and
final crates. `FrameSource` configures the clocked frame source's period and
statistics cadence (`StatisticsCadence`, beside it in `capability`). A session
reads RAF input through the sensor decoders the constructor registry holds.

## Board code outside the emulator crate

- XP4 boards live in `fujimod-emulator-xp4-board`, and their Python bindings
  in `fujimod-python-xp4`; the emulator bindings reach an XP4 session through
  `Xp4Backend` in `fujimod-python-base`, whose factory the `_native` module
  registers.
- XP5 boot lives in `fujimod-emulator-xp5-board`: `boot::open` returns the
  engine session with the `session::Inputs` that drive its keys and GPIO
  pins, so the engine session holds no XP5 state.
- These two board crates sit above the emulator, since they open sessions on
  it. Their device assembly still names models. The XP5 resources hold model
  types (mailbox pins and channel, flash storage, display configuration and
  its pixel order) and some kinds are assembled in groups (imaging, general
  DMA, clock banks); the XP4 session drives its ADC, GPIO and USB models by
  type. Each needs those handles in the interface, registered group
  assemblers and capabilities before its assembly can move to a registry.
- X70 boot policy (`boot/x70.rs`) stays only if it names plan and interface
  types alone; otherwise it moves to the EXR II board crate.
- The GIC is a model in `fujimod-emulator-ip-arm` that the A-profile runner
  reaches through `gic::InterruptController` in the interface; board builders
  construct it. The processor parts the runners drive directly live beside
  them: the generic timer and Cortex-A53 implementation registers in
  `board::aprofile`, the ARMv7-M NVIC in `architecture::mprofile`. The
  emulator depends on no model crate.

## Steps

Each step keeps every build and test working, lands as its own commit, and
records the rebuild costs below before and after.

1. Done: final crates. The Python module and the binaries have their own crates.
2. Done: EXR II plans. `fujimod-emulator-exr2-plan` holds plans and resources.
   `fujimod-emulator-exr2-board` constructs devices and is reached through
   `register_board_builder()`.
3. Model crates: done. XP4 and XP5 by subsystem (`-xp4-{storage,display,io}`,
   `-xp5-{imaging,storage,display,io}`), IP (`-ip-arm`, `-ip-dmp`, `-ip`),
   graphics, storage, parts by function (`-parts-{lens,sensor,controls,io}`),
   the EXR II subsystems
   (`-exr2-{display,i2c,gpu,audio,serial,codecs,storage,companion,system,io,boot-serial}`),
   the EXR II ISP (`-exr2-isp-{input,face,statistics,output}`), the image
   codecs (`-image-codecs`) and the sensor input models the XP4 and XP5 boards
   share (`-sensor-input`). Imaging and scene left the devices crate as the
   image layer; X70 REC live view was bit-identical before and after.
4. Plan options, handles and the constructor registry: done for EXR II. Plans
   carry model options as JSON, the handles their resources share are
   interface types, and every EXR II model is built by the constructor its
   crate registers, so neither the plan nor the board depends on a model
   crate. Remaining: XP5 and XP4 device assembly (see above).
5. Capabilities and interrupt delivery: done. The EXR II sessions reach devices
   through capabilities, XP4 boards and XP5 boot left the emulator, and the GIC
   is reached through its interface trait, so neither the emulator nor
   the Python binding libraries depend on a model crate.
6. Bindings: done. The Python binding libraries depend on no model crate;
   XP4 sessions are bound in `fujimod-python-xp4`. The former single
   `fujimod-python` crate is split by area into crates that compile in
   parallel, so a catalog edit no longer waits for every binding to rebuild
   after the emulator.
7. Final link: measured, no change. Relinking the extension module takes
   2.7–3.3 s end to end with the default Apple linker, 3.4–4.2 s with
   `-no_deduplicate` and 3.4–10 s with `lld` (load average 14), so the default
   stays. Release builds carry no debug info.
8. Image interface and implementation: done. The image layer split into the
   interface and its two implementation crates, so the emulator, the plan and
   the Python binding libraries no longer rebuild after an optics, rendering,
   RAW or RAF edit. X70 REC live view stayed bit-identical, with identical
   guest instruction counts, through two seconds after REC.
9. Catalog and its bindings: done. The catalog split by domain into
   `fujimod-catalog-{content,service,construction,controls,remote,settings,bulk}` (content and service split again in step 12)
   below `fujimod-catalog`, the reference catalog of SoCs, parts, bodies and
   firmware and their hardware profiles. `fujimod-catalog-configuration`
   holds the run configurations above it and `fujimod-catalog-schemas` the
   schema generator. USB, the workflows and the USB bindings depend on the
   service crate only, and the EXR II plan and models on the construction
   crate. The service definition bindings (`fujimod-python-service`) and the
   ff80 session lease (now in `fujimod-python-usb-base`) left the catalog and USB
   bindings. The catalog-driven boot moved from the emulator into
   `fujimod-emulator-boot`, and the run input preparation from the emulator
   and the storage interface into `fujimod-emulator-preparation`, so the
   emulator depends on no catalog crate except the content and construction
   crates and no settings crate. The X70 boot to REC ran the same 48,023,300
   guest instructions after both moves, with the same host instructions
   retired. The remote bindings borrow a board's session through
   `fujimod-python-base`, so they no longer wait for the emulator bindings.
10. Feature splits, run inputs and the final link: done. The `typescript` and
    `testing` features are gone, so `cargo test -p`, the workspace, the
    Python extension and the JavaScript builds share one copy of each crate
    under them; the catalog crates keep their TypeScript declarations off
    Emscripten, and the JavaScript synthetic USB endpoints are the test-only
    `fujimod-javascript-testing` crate. `fujimod-catalog-run-inputs` holds
    the run options, device profiles and native, firmware and hardware
    inputs beside the reference catalog. `fujimod-python-boot` installs the
    catalog boot behind a hook in `fujimod-python-base`, so the Python
    emulator bindings no longer wait for the boot crate. Release builds no
    longer strip the extension, which took 0.6 s of every rebuild; the dist
    profile still strips published artifacts.
11. Simulation workspace, EXR II resources and build scripts: done. The Bevy
    simulation is its own Cargo workspace with its own lockfile and target
    directory, so its features no longer widen the shared third-party crates
    and `cargo test -p` reuses the workspace build's crates. The plan's
    resources, their checkpoint and the `wire!` macro are
    `fujimod-emulator-exr2-resources`, and the image interface keeps the
    sensor decoders, so the emulator no longer depends on the plan. Release
    builds optimize the workspace build scripts, whose binding generator
    parses the App80 and service sources on every USB or service edit. The X70
    boot to REC ran the same 48,023,300 guest instructions with the same host
    instructions retired.
12. App80, catalog identity and progress: done. App80 is `fujimod-usb-app80`,
    firmware identities and camera definitions are `fujimod-catalog-camera`,
    and the Python App80 bindings are `fujimod-python-app80`, so a service
    definition edit no longer reaches `fujimod-usb` or the USB bindings. The
    identifier checks are `fujimod-catalog-identity` and the content-addressed
    assets with their file provider are `fujimod-catalog-assets`, so the
    emulator depends on no catalog crate. The EXR II plan carries the video
    encoder options as JSON and no longer depends on the construction
    catalog. Bring-up progress documents moved to `catalog/progress`, outside
    the tree the catalog's build script watches, so regenerating them no
    longer rebuilds the catalog chain.
13. Sessions, run configuration resolution and TypeScript derives: done. The
    emulator's sessions are `fujimod-emulator-session`, above the execution
    core, so an image interface edit no longer rebuilds the emulator. Run
    configurations no longer depend on the reference catalog:
    `fujimod-catalog-resolution` resolves them, applies their overlays and
    checks every identity they use against the selected hardware, so a
    catalog edit reaches the boot crate through a 0.1 s crate instead of the
    configuration crate. `HardwareSelection` moved to the construction crate.
    The catalog crates derive their TypeScript declarations only on wasm32
    outside Emscripten, the target the JavaScript bindings build for, so
    native builds no longer expand them; the reference catalog builds in
    1.1 s instead of 1.5 s. The X70 boot to REC ran the same 48,023,300
    guest instructions with the same host instructions retired, and the
    JavaScript declarations, `camera.generated.ts`, the JSON Schemas and the
    Python types are unchanged.
14. App80 session hooks and the module's catalog boot: done. The session
    lease crate became `fujimod-python-usb-base`, which holds the
    `fujimod.usb` exception types, USB error conversion and the session hooks
    the USB bindings install when they register. The App80 bindings lease and
    run operations through it, so they no longer wait for the USB bindings.
    The extension module installs the catalog boot itself, so
    `fujimod-python-boot` is gone and the module links right after the boot
    crate.
15. Image handles behind the resources' image part: done. The image wires
    (frame period, image grid, exposure, ISP parameters, memory outputs,
    streams, output channels, phase edges) and the image, statistics and
    sensor inputs are `fujimod-emulator-exr2-images`, which implements the
    `ImageResources` trait of `fujimod-emulator-exr2-resources`. Sessions,
    their history and checkpoints reach the image part only through the
    trait, and input logs carry RAF frames in the image part's encoding. The
    lens focus, aperture and focus-axis position, and the EXR II interrupt
    outputs and routes, moved to the device interface, and the EXR II plan
    carries every image model's options as JSON. The resources, the plan,
    the session, the remote crates and the models that name no image type
    no longer rebuild after an image interface edit. Portable checkpoints
    keep their format: a checkpoint written before restores after and the
    reverse, with identical fingerprints. The X70 boot to REC and two
    seconds of live view gave identical screenshots, ticks and instruction
    counts at every quarter second, and the boot to REC ran the same
    48,023,300 guest instructions with the same host instructions retired.
    The image part also installs the preparation that builds a run's image,
    statistics and sensor inputs from their JSON documents, so preparation,
    the boot crate and the emulator bindings name no image type either.
16. Firmware identities: done. `FirmwareIdentity`, `Digest` and
    `IdentityError` are `fujimod-catalog-firmware-identity`, below the camera
    and service definitions, so a camera definition edit no longer reaches
    the service definitions, App80 or the App80 bindings, and their generated
    service values rebuild only when the service definitions or App80 change.
17. App80 command types and Python values: `fujimod-usb-app80-types` holds
    command identifiers, requests and replies, the firmware codec catalog and
    its value types below USB. `fujimod-usb-app80` holds the ff80 mailbox and
    session operations. Generated Python command values moved to
    `fujimod-python-app80-values`, whose domain dependencies are App80 types
    and `fujimod-python-service`; `fujimod-python-app80` registers transport
    operations. JavaScript and Python callers use the types crate directly.
    The generated Python and TypeScript value declarations retain their public
    names, and the Python namespace remains `fujimod.usb.app80`.
18. Reference catalog documents: `fujimod-catalog` stores physical controls,
    firmware control IDs, generated input facts and remote layouts as JSON
    values. The controls and remote crates decode and validate those documents
    at their use boundaries; remote facts are assembled from a resolved
    selection in the remote crate. The reference catalog no longer depends on
    either decoder crate, so edits there skip the catalog and resolution. Boot
    names the controls and remote fact types it decodes directly.

## Rebuild costs

Wall seconds after one edit: the release Python extension
(`scripts/python_extension.py`), and the edited crate's unit tests
(`cargo test -p` with `--no-run`). "Before step 2" was measured on a loaded
machine. "Before steps 6 and 8" precedes the binding split of step 6 and the
image split of step 8: the catalog and image rows were measured in the same
checkout as the next column, at load average 5–11, and the other rows
repeat the measurements before them (load average 4–8). "Before step 9"
precedes the catalog split, when every catalog row was one crate edit.
"Before step 12" repeats the "now" column of step 11, measured at load average
4.7–5.7, and "Before step 13" the "now" column of step 12, measured at load
average 3.1–5.4, "Before step 14" the "now" column of step 13, measured
at load average 5.0–5.7, and "Before step 16" the "now" column of step 14,
measured at load average 3.5–4.6. The "now" columns were measured after
step 16 at load average 4.1–4.9, the lowest available, two edits per row; a
catalog row edits the file named, otherwise the crate's `lib.rs`. Other
builds ran throughout, so a row varies by up to 1 s between measurements;
four unit-test rows that caught a load spike repeat the run before, at load
average 4.9–8.0.

After step 17, the four requested release-extension rows were each measured
with two edits using `touch <file> && cargo build --release --locked
--manifest-path bindings/python/module/Cargo.toml --lib --timings`. The load
figures below are the one-minute load average immediately before each build;
times are the two samples in seconds. The catalog and plan source boundaries
were not changed in step 17.

| Edit | Before step 17 (s; load) | After step 17 (s; load) |
|---|---|---|
| USB core (`crates/usb/src/lib.rs`) | 3.16, 2.97; 3.37, 3.26 | 2.03, 2.06; 2.89, 3.06 |
| Catalog controls (`crates/catalog-controls/src/lib.rs`) | 3.12, 2.85; 3.26, 3.24 | 3.44, 2.92; 2.75, 3.49 |
| Catalog identity (`crates/catalog-identity/src/lib.rs`) | 3.16, 3.16; 3.24, 4.26 | 3.15, 3.16; 3.49, 4.09 |
| EXR II plan (`crates/emulator-exr2-plan/src/lib.rs`) | 2.83, 2.66; 4.08, 4.08 | 2.82, 2.82; 3.92, 3.92 |

| Edit | Extension before step 2 | Before steps 6 and 8 | Before step 9 | Before step 12 | Before step 13 | Before step 14 | Before step 16 | Extension now | Unit tests now | Target |
|---|---|---|---|---|---|---|---|---|---|---|
| EXR II I2C controller | 23–26 | 1.4 | 1.4 | 1.0–1.1 | 1.0 | 0.9–1.0 | 1.0–1.1 | 1.0–1.1 | 0.9 | 2–3; under 1 |
| EXR II ISP image input | 15–19 | 1.6 | 1.4 | 1.0–1.3 | 0.9–1.0 | 0.9–1.0 | 1.0–1.1 | 1.1 | 1.1 | 2–3; under 1 |
| Lens part | 23–26 | 2.0 | 1.8 | 1.4–1.6 | 1.4 | 1.3–1.5 | 1.7–1.8 | 1.4–1.7 | 0.9 | 2–3; under 1 |
| XP4 I/O | | 2.0–2.1 | 1.6 | 1.3–1.4 | 1.3 | 1.2–1.3 | 1.2–1.4 | 1.3–1.5 | 0.9 | 2–3; under 1 |
| XP5 I/O | | not linked | not linked | not linked | not linked | not linked | not linked | not linked | 0.8 | under 1 |
| Arm interrupt controller | | 3.3–3.5 | 1.5 | 1.2 | 1.1–1.2 | 1.1 | 1.2–1.4 | 1.2–1.4 | 0.8 | 2–3; under 1 |
| Image formation | | 6.5–7.8 | 2.3–4.6 | 1.5–1.7 | 1.5 | 1.4–1.5 | 1.5–1.6 | 1.5–1.6 | 0.8 | 2–3; under 1 |
| Image processing | | 6.5–7.8 | 2.1–2.2 | 1.4 | 1.3 | 1.2–1.3 | 1.3–1.4 | 1.3 | 0.8 | 2–3; under 1 |
| Image interface | 23–26 | 6.5–7.8 | 4.7–5.2 | 3.4–3.9 | 3.2–3.7 | 3.1–3.5 | 3.2–3.7 | 2.0–2.3 | 1.0 | 2–3 |
| EXR II plan | | 12.7–17 | 6.0–7.7 | 2.4–2.5 | 2.2–2.3 | 2.2–2.3 | 2.5 | 2.5–2.7 | 2.8 | 0.9 | 2–3 |
| Boot assembly | | | | 1.3 | 1.3 | 1.2 | 1.3–1.4 | 1.2–1.3 | 1.5 | 2–3 |
| Run input preparation | | | | 1.9–2.4 | 1.4–1.7 | 1.4–1.5 | 1.5–2.0 | 1.4–1.6 | 0.8 | 2–3 |
| Emulator sessions | | | | | | 1.7–1.9 | 1.8–2.2 | 1.8–1.9 | 1.0 | 2–3 |
| EXR II image part | | | | | | | | 1.7–1.9 | 1.2 | 2–3 |
| USB core (`fujimod-usb`) | | | | | 3.4–5.5 | 3.2–3.3 | 2.8–2.9 | 2.8–2.9 | 2.0–2.1 | 1.1 | 2–3 |
| Run configuration (`lib.rs`) | | | 8.1–9.6 | 1.7–1.8 | 1.7 | 1.7 | 1.8 | 1.6–1.8 | 1.0 | 2–3 |
| Run configuration resolution | | | | | | 1.4 | 1.4–1.5 | 1.4 | 0.7 | 2–3 |
| Run inputs (`options.rs`) | | | 8.1–9.6 | 2.8–3.2 | 2.5–2.9 | 2.1–2.2 | 2.2–2.3 | 2.1–2.3 | 1.0 | 2–3 |
| Reference catalog (`reference.rs`) | | 9.7–10.6 | 8.1–9.6 | 3.5–4.4 | 3.2–3.3 | 2.5–2.8 | 2.7–2.9 | 3.0–4.1 | 1.9 | 2–3 |
| Hardware profiles (`profiles.rs`) | | | 8.1–9.6 | 3.2 | 3.1–3.2 | 2.5 | 2.6–2.7 | 2.5–2.8 | 1.5 | 2–3 |
| Catalog identity (was in content) | | | 8.1–9.6 | 4.5–5.6 | 3.9–5.0 | 3.3–3.5 | 3.1–3.4 | 3.1–3.4 | 3.1–3.2 | 0.6 | 2–3 |
| Catalog assets (was in content) | | | 8.1–9.6 | 4.5–5.6 | 1.4 | 1.4 | 1.5 | 1.5–1.8 | 0.7 | 2–3 |
| Catalog camera (was in service) | | | 8.1–9.6 | 3.5–3.8 | 3.0 | 3.0 | 2.7–3.2 | 1.6–1.8 | 0.7 | 2–3 |
| Catalog firmware identity | | | | | | | | 3.2–3.4 | 0.6 | 2–3 |
| Catalog service | | | 8.1–9.6 | 3.5–3.8 | 2.5–2.6 | 2.5 | 2.6–2.7 | 2.6–2.8 | 0.7 | 2–3 |
| Catalog construction | | | 8.1–9.6 | 3.9–4.2 | 3.3–3.4 | 2.8–3.0 | 2.9 | 2.8–2.9 | 0.7 | 2–3 |
| Catalog controls (`lib.rs`) | | | 8.1–9.6 | 3.7 | 3.6 | 2.8 | 2.9 | 2.9 | 2.9–3.4 | 0.9 | 2–3 |
| Catalog remote | | | 8.1–9.6 | 3.3–3.4 | 3.3 | 2.6 | 2.7–2.9 | 2.6–2.7 | 0.7 | 2–3 |
| Catalog settings | | | 8.1–9.6 | 3.2–3.3 | 3.2–3.3 | 2.6 | 2.7 | 2.6 | 0.7 | 2–3 |
| Catalog bulk | | | 8.1–9.6 | 3.3–3.5 | 3.3–3.4 | 3.1 | 2.7 | 2.5–2.6 | 0.6 | 2–3 |

An EXR II model edit recompiles the model crate (0.2–0.3 s),
`fujimod-emulator-models` (0.1 s) and the extension module (0.5 s since step
10, of which the link is 0.4 s; 0.8–1.0 s before, when cargo also stripped the
extension). Before the registry it also recompiled the board crate (0.4–0.5 s;
2.1–2.3 s in all for the I2C controller, 2.6 s for the lens part). A lens part
edit also recompiles the EXR II serial, audio and boot serial crates, which
depend on it. The extension links no XP5 model. An Arm interrupt controller
edit recompiles the XP4 board and its Python bindings with the extension
module; before the GIC trait it also recompiled the emulator and all Python
bindings.

Before step 8 an image formation or processing edit was an image layer edit:
it rebuilt the plan, the emulator, every EXR II model and `fujimod-python`.
It now recompiles the edited crate (0.2–0.3 s; 1.4 s for the first formation
edit), the ISP, codec and sensor models that use it, the host image bindings
and the module. An image interface edit still rebuilds the plan (0.9 s), the
emulator (2.0–2.1 s), the EXR II models in parallel and the emulator bindings
(0.7 s); a plan edit adds the emulator (1.4 s now) and the emulator and remote
bindings.

Before step 9 a catalog edit rebuilt the 20.8K-line catalog (2.6–3.5 s), then
`fujimod-usb` (1.0–1.3 s), `fujimod-python-usb` (2.5–3.2 s) and the remote
bindings on its critical path, beside the plan, every EXR II model and the
emulator. After step 10 a run configuration edit rebuilds the configuration
crate (0.5–0.6 s), the boot crate (0.7–0.9 s), the boot and catalog bindings
(0.1 s and 0.3–0.4 s) and the module (0.5 s). A run input edit adds the
run-inputs crate (0.8 s) before the configuration crate. A reference catalog
or hardware profile edit adds `fujimod-catalog` instead, and a document crate
edit adds that crate (0.1–0.3 s) before it; the remote bindings build beside
the configuration and boot crates. Since step 12 an asset edit rebuilds only
the boot crate, the emulator bindings and the CLI worker beside the module; a
construction edit rebuilds the run inputs, the reference catalog, the image
codecs and the ISP output, not the plan or the other EXR II models; and a
service definition edit rebuilds the App80 crate (0.2 s), the service and
App80 bindings (0.2 s and 1.5–1.8 s) and the module only. A camera definition
edit adds the workflows and the USB bindings (0.7 s) beside the App80
bindings. A
plan edit rebuilds the EXR II models, the boot crate and the bindings that
name the plan, but since step 11 not the emulator or the remote crates. Since
step 13 an image interface edit rebuilds the EXR II resources, the session
crate (0.4 s), the plan, the EXR II models and the boot crate, but not the
emulator, and a reference catalog or hardware profile edit rebuilds the
catalog (1.1 s), the resolution crate (0.1 s), the boot crate (0.7–0.8 s),
the catalog bindings and, since step 14, the module straight after the boot
crate. Since step 14 a USB core edit reaches the App80 bindings through the
App80 crate and `fujimod-python-usb-base`, beside the USB bindings, and a
bulk edit reaches the USB bindings (0.6 s) but not the App80 bindings.

Since step 15 an image interface edit rebuilds 20 crates (6.4–7.5 s of
compilation): the image part, the models that name image types, the EXR II
board, the models registration crate, the host image, graphics and XP4
bindings and the module, but not the resources, the plan, the session,
preparation, the boot crate or the remote and emulator bindings. Since step
16 a camera definition edit rebuilds the workflows and the USB bindings
before the module, not App80.

The rows still above 3 s:

- Reference catalog, at 3.0 s for its second edit: the reference catalog
  (1.2 s), resolution, the boot crate (0.7 s) and the module, one after
  another. Its first edit (4.1 s) ran during a load spike that slowed every
  crate by about a third.
- Catalog identity: identity, controls, remote facts, the reference
  catalog (1.2–1.3 s), resolution, the boot crate (0.8 s) and the module,
  one after another.
- Catalog firmware identity: the service definitions, App80 and the App80
  bindings (1.6–2.1 s), whose generated service values rebuild with App80.
  Firmware identities and identifiers are rarely edited.
