# FujiMod JavaScript bindings

This package provides `import { initialize, usb } from 'fujimod'` for browser
service/debug tools. Its Rust binding crate belongs to the root Cargo workspace;
commands, framing, range operations and transfer logging run in `crates/usb`.
There is one JavaScript package and one `usb` namespace. Firmware/body maps and
application policies are supplied by callers.

## Build and test

Use the repository's Rust and Node toolchains (`mise install`). Install the
Wasm standard library and the wasm-bindgen CLI matching `Cargo.lock`:

```sh
rustup target add wasm32-unknown-unknown
cd bindings/javascript
npm ci
npm run setup:tooling
npm run build
npm test
```

`setup:tooling` installs the official prebuilt CLI matching `Cargo.lock`, checking
the release checksum. Alternatively use `cargo install wasm-bindgen-cli` with
that exact version and `--locked --root target/tooling` from the repository root.

`WASM_BINDGEN` can select an explicitly installed CLI executable. By default,
the build script uses `target/tooling/bin/wasm-bindgen` from the repository root.
`pkg/`, `dist/` and `node_modules/` are generated and ignored. The package is not
yet published to npm; these commands do not publish it or change `apps/web`.

Tests run compiled Wasm in Node. Instrumented tests exercise the real protocol
core with scripted endpoints. Production tests exclude that instrumentation
and exercise nusb's WebUSB backend against a fake `navigator.usb` device,
including descriptors, interface/alternate selection, endpoint numbers, data
transfers, picker cancellation and a stalled control transfer. No test opens
hardware. An additional headless Chromium check exercises the production package
in a real browser against that same fake USB API:

```sh
npx playwright install chromium
npm run test:browser
```

`PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` can select an explicitly installed Chromium
binary. Permission UI and physical camera integration remain unvalidated.

For worker CPU profiling and boot timing, see
[browser performance](../../crates/emulator/BROWSER_PERFORMANCE.md).

## Emulator runtime

The `fujimod/emulator` entry loads the emulator module and exports its `Camera`
class and types. The Rust crate in `emulator/` declares that interface once:
wasm-bindgen generates the class and its glue from `#[wasm_bindgen]` items, and
tsify generates the TypeScript types of the serde values they exchange. Emscripten
links the crate with Unicorn and runs wasm-bindgen on the result
(`-sWASM_BINDGEN`). A loaded module is configured once with its execution policy,
JIT tier, and DMP renderer preference. Configuration resolves after any requested
WebGPU device is ready, then the module constructs cameras with `Camera.fromGraph`,
`fromPrepared`, `fromResolved` or `fromSnapshot`. Failures throw `Error`; once the guest has
failed, their name is `GuestExecutionError`.

The build writes the generated declarations to `emulator/camera.generated.ts`,
which is checked in so TypeScript consumers type-check without an Emscripten
build. Commit it with any change to the Rust interface; never edit it.

A staged package contains the generated `camera.mjs` and `camera.wasm` beside
that entry, so a browser can load them with `loadBundledEmulatorModule()`. The
Node-only `fujimod/emulator/node` entry loads an explicit module path and its
adjacent Wasm file with `loadNodeModule()`, or opens a camera with `openCamera()`,
`openPreparedCamera()` or `openSnapshot()`. Node filesystem APIs are kept out of
the browser entry.

Build and stage the runtime with the pinned Mise toolchain:

```sh
mise install
npm --prefix bindings/javascript run setup:tooling
mise run javascript-emulator-build
```

The builder defaults to ignored `target/emulator/` output and stages runtime
assets under `bindings/javascript/dist/emulator/`. Use
`npm run build:emulator -- --out DIR` to choose another output directory. The
generated `build.json` records the toolchain and artifact hashes. Runtime input
manifests remain the caller's responsibility. Guest conformance scripts can run against a built module with
`npm run test:emulator:guest` and `npm run test:emulator:devices`; each accepts
an optional path to `camera.mjs`.

The final module is optimized with the pinned SDK's Binaryen, using WebAssembly
GC support to refine indirect-call types. The optimizer validates the module
and preserves its import/export interface before replacing the linked file.
`build.json` records the optimizer version, flags, sizes and hashes. Runtime
browsers must support WebAssembly GC, shared memory and exception handling.

Run the emulator in a dedicated browser worker. The runtime uses shared Wasm
memory and two persistent device threads; serve the page and runtime assets with
`Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp`. The generated module is also the
pthread worker entry, so keep its URL reachable alongside the Wasm binary.

Module loading defaults to `executionPolicy: 'interactive'`, the same board/CPU
policy used by native clients. Use `{ executionPolicy: 'exact' }` for native
fingerprint comparisons and deterministic replay. A runtime manifest can select
the same policy with `executionPolicy`. The policy is selected before the reset
checkpoint, so power cycling preserves it. Interactive Undo restores a stopped
checkpoint and may replay approximately.

Device jobs own frozen inputs and staging bytes. They cannot access the bus,
live guest RAM, processor state or clocks. Only the owner publishes their output;
checkpointing materializes pending results without publishing device effects.
Still color development, warp resampling and JPEG work use this executor;
Exact mode executes the same arithmetic inline.

The [mod installation helpers](../../crates/mod-install/README.md) expose
`usb.InstallQueries` for guarded borrowed callback batches,
`usb.CommandSequence.verifiedWrites` for packet-sized write/readback batches,
and `usb.OwnedUpload` for padded images within caller-owned allocations.
`session.uploadOwned` borrows and restores the qualified stock destination;
`session.verifyOwned` compares the complete image through a qualified bulk
window. Callers provide firmware addresses, ownership and accepted status pairs,
then publish hooks or run initialization only after successful verification.

## Connect and identify

Call `await initialize()` once before enabling the GUI's connect action. It
loads the adjacent Wasm asset. Host the package/assets over HTTPS, such as
GitHub Pages, or localhost, in a browser supporting WebUSB. From the user's
connect action, call `usb.requestDevice(usb.DeviceFilter.Diagnostic)` to open the
browser picker. Cancellation returns `undefined`. `usb.discover(filter)` lists
already granted devices without opening a picker.

Inspect `device.interfaces()` and explicitly choose a configuration, interface
and alternate setting. Select limits from the body's firmware definition. The
library has no default camera, interface or firmware map.

```ts
import { initialize, usb, type Device } from 'fujimod';
await initialize();

async function identify(
  device: Device,
  selected: InstanceType<typeof usb.InterfaceSelection>,
  limits: InstanceType<typeof usb.FirmwareLimits>,
) {
  const options = new usb.TransferOptions(5000, 1024 * 1024);
  const session = await usb.Session.open(device, selected, limits, options);
  try {
    const reply = await session.deviceInfo(usb.InfoSelector.Product);
    return reply.text;
  } finally {
    if (session.state === usb.SessionState.Ready) await session.close();
    else session.abandon();
  }
}
```

`TransferOptions` specifies a per-transfer timeout in milliseconds and a host
reply budget. `FirmwareLimits` requires memory-read, memory-write, configuration
and console limits, in that order. Addresses, sizes and raw protocol fields are
checked integers; strings, fractions, negative values, overflow and nonfinite
numbers are rejected. Choices use generated TypeScript enums, with runtime
validation of their number/type/domain. Data arguments require `Uint8Array`;
bytes are copied across the Wasm boundary rather than exposing a borrowed view.

## Commands and composed operations

Memory of any size goes through `session.memory(firmwareId)`. It qualifies
the firmware's stock bulk path once; `read` uses it for anything larger than one
packet, `write` splits larger writes into packet-sized commands, and `close`
restores any bulk source override. `readSinglePacket` and `writeSinglePacket`
send exactly one memory command of at most `packetBytes` (256 on the X70) and
refuse anything longer.

Session methods such as `readSinglePacket`, `readConfiguration`,
`diagnosticQuery`, `submitConsole` and `pollConsole` each perform one command.
`configurationGeometry`, `prepareCapabilities` and the two firmware-download
status methods retain typed metadata and raw replies. `readLog` is consuming
retrieval: firmware can advance its log cursor or clear its overrun state.
`clearFirmwareDownloadStatus` is an explicit mutation, separate from reading
state or identifying the camera. Application states such as Writing or console
Pending never cause implicit waits, retries or cleanup.

Higher-level operations live in their own groups:

- `usb.data.readChunks(session, window, address, length)` streams E1 ranges as
  an async iterator with `bytesCompleted` and an explicit `close()`;
  `DataWindow` requires an explicit base, aperture, capacity and offset mode.
- `usb.data.upload(session, bytes, pieceBytes, mode)` owns setup, E0 pieces and
  E8 as one operation, preserving raw setup fields and final acknowledgment.
- `usb.app80` supplies distinct one-byte command IDs, requests and an explicit
  mailbox carrier. Codes and result meanings come from the selected firmware.
- `usb.research` supplies distinct ff80 command IDs and unclassified request
  bodies with an explicit reply capacity. Only the common header is validated.

```ts
const memory = await session.memory('x70-110');
try {
  const bytes = await memory.read(address, length);
  await sink.write(bytes);
} finally {
  await memory.close();
}
```

A range owns its session between pulls and never reads ahead. Finishing, closing
or breaking a `for await` loop releases that ownership without sending C4.
Completed chunks stay with the caller on failure. A failed bulk command's valid
partial data remains in `error.partial`. A currently pending pull must finish
before `close()`; explicitly abandon its session to stop host processing.

The [command catalog](../../crates/usb/PROTOCOL.md) defines effects and the
boundary between direct mechanisms, nested protocols and compositions. Debug
activation, safe exit, callbacks, allocation, persistent settings and mod
installation belong in firmware-qualified application code.

## Ownership, cancellation and errors

Each session operation reserves its session before its Promise is returned. Overlapping
commands reject with `ErrorKind.Busy`; a range reports `SessionState.ReadingRange`
until it releases ownership. Timeout, transfer failure and malformed replies
leave `CompletionUnknown`. Further commands cannot consume an unfinished reply.

`close()` consumes the session on success or failure. `Closed` requires a
received two-byte C4 reply; those bytes remain opaque. A failed close leaves
`CompletionUnknown` and cannot be retried. It does not certify camera shutdown.

JavaScript Promises have no implicit cancellation. `session.abandon()` explicitly
releases idle host resources, cancels a pending host operation or range pull, and
makes the session terminal (`Abandoned`). It sends no further USB command or
C4. The browser may already have submitted a transfer, and camera execution may
continue. Dropping or ignoring a Promise alone does not cancel its operation.

Errors are ordinary `Error` objects with a typed `kind`. Use `usb.isError(error)`
to narrow a TypeScript `unknown` to `UsbError`. Transfer errors retain stage,
request kind when applicable, backend failure kind/code, counts and partial
bytes. Protocol errors retain a `ProtocolFailureKind` and available numeric
fields. Device errors retain command/primary/secondary values. Research replies
and diagnostic results retain a `CommandStatus` enum alongside their raw fields.
No integer is interpreted as an instruction to halt or wait.

## Raw logging

Create `new usb.tracing.Trace(maxEvents, maxBytes)` and call
`options.setTrace(trace)` before opening a session. `trace.drain()` works
independently of session ownership, including during a pending command and
after opening fails. Each immutable event retains typed stage/phase/request
kind, raw bytes, endpoint and counts. Correlation/sequence IDs and exact loss
counts are JavaScript `bigint`; timestamps are Unix milliseconds.

The recorder bounds event count and retained payload bytes. It reports loss
without truncating payloads silently or changing USB outcomes. Consumers choose
when to drain and whether losing trace data should stop their application.

Python in Pyodide can use this same initialized module. Call
`registerPyodide(pyodide)` and install the browser wheel from
[`bindings/python`](../python/PYODIDE.md). `session.cancelPending()` and chunk
iterators' `cancelPending()` cancel host work without abandoning the session.
Await the pending Promise before reusing ownership; the session is fenced if
an exchange was incomplete. Canceling before dispatch leaves it ready. Host
cancellation rejects with `ErrorKind.Canceled`; it never stops camera code.
`abandon()` remains the distinct terminal action.

## Firmware-selected service controls

`catalog` loads the shared firmware data. `usb.app80` exposes typed command factories
and executes their invocations through the same Rust codecs as native Python:

```ts
import { catalog, usb } from 'fujimod';

const identity = new catalog.FirmwareIdentity(firmwareId, firmwareSha256);
const definition = catalog.service.Definition.fromJson(definitionJson, identity);
const service = new usb.app80.Service(definition);
const invocation = service.prepare(
  new catalog.service.ServiceModes(currentServiceFlags),
  usb.app80.Command.readPins(new usb.app80.PinBank(0)),
);
const reply = await usb.app80.execute(session, invocation);
const value = reply.value;
if (value.kind === usb.app80.ReplyKind.Pins) {
  console.log(value.levels);
}
```

The identity qualifies the loaded data; the caller still identifies the camera
and checks execution eligibility. Preparation performs no USB I/O or mode change.
Command codes, argument positions, mode rules and mailbox come from the selected
definition. An invocation keeps the request and mailbox together. Commands and
profile actions have named factories rather than raw selector constructors.

Definitions preserve ordered entries, duplicate codes, mode availability and
transitions. Handler and parameter-table addresses use `bigint`. Metadata and
reply values use immutable unions tagged by enums; TypeScript narrows payload
fields using their `kind`. Raw mailbox getters return independent `Uint8Array`
copies. `invocation.decode(raw)` supports offline decoding without a session.

`catalog.isError(error)` exposes a typed `DefinitionFailure` in `error.reason`;
`usb.app80.isError(error)` similarly exposes `CommandFailure`. Unsupported,
ambiguous, disabled or unreviewed controls do not fall back to another body.
Research remains explicit through `service.prepareResearch(code, modes, bytes)`.

Save failure and unknown status values remain data. Word results distinguish a
returned value, rejection, confirmation and unknown status. A malformed typed
reply fences the session; an interrupted publication follows normal cancellation
and ownership rules. There is no implicit poll, retry or close. The Node and
Chromium production tests execute this path through nusb and fake WebUSB.

Peripheral factories include `Command.registerControl(RegisterAction.powerWrite(...))`,
`Command.i2c(I2cAction.read(I2cPort.Port0, command, new I2cLength(length)))`,
`Command.accumulator(AccumulatorAction.StopAndRead)` and `Command.peripheralReport()`
under `usb.app80`. `new I2cPayload(bytes)` owns a checked 1–10 byte copy; the
write-only route uses `I2cAction.writeSingle(command, value)`. Result variants
carry copied bytes or an immutable tuple of three low counter halfwords.

Media factories add `Command.displayOption`, `displayMode`, `sound`, `hdmi`,
`parameterService`, `chargeControl` and `chargeStatus`. Inputs use the corresponding
action factories, `Toggle`, `DisplayMode`, sound enums and `ChargeQuery`.
`SoundClip`, `HdmiPacket` and `HdmiValue` validate the clip index, exact packet
length and 12-bit values in Rust. Charge reply unions use different status enums
for applying control, presence and reported register-state groups; unknown bytes
retain `rawStatus`. No charge value starts an implicit poll.

`Command.audioCapture(new AudioCount(n), AudioRoute.Request82)` checks 0–10
firmware chunks; `Command.audioPlayback(AudioPlayback.AllLanes)` can instead
zero either interleaved lane. Samples stay in camera buffers; USB transfer is a
separate operation. Results use `AudioResultKind`. `Command.eraseStorage` takes
`StorageErase` and returns `StorageEraseResultKind`. These selectors can clear
persistent regions and profiles. `Command.storageStatus()` returns
`StorageStatusKind`: zero is deliberately ambiguous, and other classifications
retain their numeric names. `Command.formatStorage()` formats the selected
drive, and `Command.displaySetup()` invokes the internal display setup sequence.
These last two have no payload result. No result starts host polling or cleanup.

`Command.nandReadBlocks(NandSpare.Include, group)` reads eight firmware NAND
blocks into its RAM buffer without downloading them. The reply uses
`NandReadResultKind`; output 1 is an application result, never a host wait.
`Command.option41()` returns `Option41StateKind.Zero`/`NonZero`/`Unknown` for the
firmware getter, without asserting drive presence. `Command.wlanControl(Toggle)`
requests a control-line action whose enable path also increments a counter.

`RadioAction.primary(mode, channel, power, option)` uses `RadioMode`, checked
`RadioChannel` (0–14), checked `RadioPower` (encoded 0–127) and `Toggle`.
`secondary(mode, channel)` excludes the unused fields. Pass either to
`Command.radioControl`. `Command.signalStrength(new PeerAddress(bytes))` owns
six bytes and returns opaque `rawValue`, which can include unchanged or
uninitialized firmware output. `Command.iperf()` uses stored configuration text
and returns opaque `rawResult`; firmware service acknowledgment can precede
running the test. Neither result starts a host poll or certifies a measurement.

Service enums, checked quantities, command/action factories, structured replies
and public registrations come from the [shared Rust generator](../generator/README.md).
Builds regenerate them automatically; validation remains in the core.

WPS and station controls use `Command.wps(WpsAction.start(target))` and
`Command.station(StationAction.startStored())`. WPS targets are
`WpsTarget.broadcast()` or `WpsTarget.peer(new WpsPeerAddress(bytes))`; exact
peers reject a zero first byte. `WpsAction.publishProgress()` also writes camera
configuration. Station scans use `StationScanMode.Mode0/Mode1`, which invoke the
same studied path. Replies preserve callback snapshots without a host poll.
Generated handle types remain distinct in TypeScript, even when they expose the
same properties. See the [Rust command semantics](../../crates/usb/README.md).

`Command.networkReconfigure(NetworkReconfigureAction.readConfiguration())`
returns an opaque six-byte snapshot. AP application uses the action's `apply`
factory with number, checked channel, `AccessPointRadioMode`,
`AccessPointSecurity` and exact two-byte `CountryBytes`. The separate
`Command.transferPrepare`, `transferSelectOperation`, `transferReady`,
`transferSelectChannel` and `transferStart` factories use `TransferOperation` and
`TransferChannel` enums and return typed results with preserved unknown bytes.
They add no host poll or USB payload transfer workflow.

Transfer payload factories include `transferReadWord`, `transferWriteWord`,
`transferPair`, `transferBitfields`, byte/mode-byte operations, halfword
operations, option reads, selected-word writes and load tests. Their address,
quantity and selector types remain distinct. Halfword/option replies are raw
snapshots; some stock transfer helpers leave output uninitialized or pass null
callback data. The [Rust command limits](../../crates/usb/README.md) describe
these constraints; the host does not manufacture completion or repair state.

Protocol observations are in `trace.drain().protocolEvents`; USB observations
remain in `events`. Both share sequence numbers and one event/byte budget.
`usb.tracing.Layer` distinguishes FF80 and App80, and `phase` reports submission,
completion, failure or cancellation. Protocol records carry the actual request
and reply bytes, including rejected replies. Tracing never decides completion,
retries work or invokes user callbacks during an exchange.

## Browser device ownership

`usb.discover(filter)` and `usb.requestDevice(filter)` return per-device handles
backed by one page-lifetime Rust nusb owner. Discovery opens each selected owner
once; `await device.open()` is idempotent and verifies that handle is still open
and present. Rediscovery reuses its stable `device.id`. `discoverProxy` handles
also expose IDs, sessions, presence events and Rust-decoded traffic. Their IDs
are scoped to the host, credential and device route; they observe the remote
owner rather than opening a browser USB device. A read-only inventory monitor
updates remote presence. Sharing remains exclusive to locally owned devices.

- `await device.acquireSession(selection, limits, options, tool?)` acquires the owner's
  exclusive logical session lock and opens FF80. `await session.close()` sends
  C4 and releases that lock. The optional tool name tags local session and log
  events. An abandoned/uncertain session fences the owner.
- `device.share()` enables sharing and returns a Rust `Exporter` for this device.
  Pass it to ShareHost. `device.stopSharing()` disables remote access; it does
  not shut down the device or release its persistent physical interface.
- `device.status` is a `DeviceState` snapshot: `id`, `present`, `open`, `sharing`,
  `sessionOwner` (`null`, `{kind: 'local', tool?}`, or `{kind: 'remote', controller}`),
  and `fenced`. `present`, `sharing`, and `sessionOwner` also have direct getters.
- `device.watch()` returns `DeviceEvents`. `await events.next()` yields the
  initial state, then every ownership/presence/sharing transition in order.
  `events.close()` wakes pending reads; `next()` returns `undefined` once drained.
  Free the subscription handle when finished. Hotplug events come from nusb.
- `await device.close()` stops sharing, waits for a local session to finish and
  awaits physical interface release. Close or abandon active sessions first.
  `await device.open()` reopens a cleanly shut down owner without reopening USB.

Local and remote clients share the same lock. A busy acquisition refuses without
USB traffic. Sharing on/off does not acquire that lock, close FF80 or reopen USB.
Stopping a share with an unfinished remote session conservatively fences the
owner. Keep the device handle connected independently of any network share.

`Exporter.execute(controllerId, encodedExecute)` returns the encoded proxy Result.
`inventoryJson`, `busy(id)`, `owns(controllerId, id)`, `fenced(id)` and
`abandon(controllerId)` support transport routing. `Exporter.stopSharing()`
detaches its devices; `shutdown()` explicitly shuts them down. Freeing an
exporter wrapper does not close or release the page's Rust device owner.
`usb.Exporter.discover()`/`requestDevice()` select and share all permitted devices
for the `/usb-proxy/` relay page. Device IDs and events remain per-device.

`device.watchLog()` returns a `LogEvents` subscription with `next()` and `close()`.
It emits exactly one `Ff80Exchange` after each FF80 control-envelope/bulk phase,
for both direct local sessions and remote controllers. D0 and D1 are separate
exchanges; D1 carries the decoded request's address/selector context. Events
include `deviceId`, `holder`, sequence/time, operation, command/function,
address/selector/page/length, requested/transferred lengths, the 12-byte
`envelope` and raw payload `data` as `Uint8Array`, plus error/truncation fields.
Selectors and command fields contain wire values. `consoleData` contains the
submitted console command on D0 function 6 or an output chunk on D1 function 7,
with the protocol lengths checked in Rust; other events contain `null`. A model
can decode this byte slice as text without duplicating the FF80 decoder.
The observer never changes USB execution. Capture is limited to 8 MiB per
exchange; exact transferred counts and `truncated` describe a longer payload.
A subscriber exceeding 128 queued events or 16 MiB closes and rejects `next()`
after its captured events drain, reporting overflow rather than silently losing
log entries. Close and free subscriptions when finished.
