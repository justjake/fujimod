# FujiMod Python package

This directory holds the `fujimod` Python package. Its PyO3 bindings are
crates by area, which compile in parallel: `catalog/`, `usb/`, `recording/`,
`remote/`, `emulator/` (boards and run configurations), `host-image/`,
`graphics/` and `xp4/`, over the shared errors and values in `base/`.
`module/` links them into `fujimod._native`. The USB implementation lives in
the workspace's `crates/usb/`. Python source lives in `python/fujimod/`. Native Python loads the compiled
`fujimod._native` extension; Pyodide uses the same public `fujimod.usb`
namespace through the [WebUSB adapter](PYODIDE.md).

Build and install the extension from the repository root:

```sh
uv run --locked python scripts/python_extension.py
```

It compiles `fujimod._native` with Cargo's release profile, checks that it
loads, replaces `python/fujimod/_native.abi3.so` and puts `python/` on the
environment's path. Rerun it after Rust changes.

## Native execution

The native extension also exposes `fujimod.emulator`. For a resolved graph, build a
`Definition` from its execution graph, device plan and identified assets, then
pass the matching bytes to `make_board`. Declared images are loaded into mapped
memory before execution.

For a catalog runtime, `Board.from_inputs` takes explicit paths to the update
file, real boot ROM and selected Unicorn library. Optional NAND and settings
files are separate keyword arguments. Body and firmware identifiers select
the runtime facts; they never select or derive an input path:

```python
from pathlib import Path

from fujimod.emulator import Board

with Board.from_inputs(
    configuration_id="x70-110-rec",
    overlays=("x70-110-rec-usb",),
    update=Path("/inputs/update.dat"),
    boot_rom=Path("/inputs/boot.rom"),
    unicorn=Path("/libraries/libunicorn.dylib"),
    nand=Path("/inputs/nand.bin"),
    settings=Path("/inputs/settings.bin"),
) as board:
    report = board.boot_to_rec()
```

Omit `nand` or `settings` when those files are not supplied. The required
configuration ID selects its body, firmware and execution policy; optional
overlay IDs select declared changes in order. Input paths remain
caller-selected.
The board owns its native execution session. It exposes processor, memory,
checkpoint and state operations, and supports `with` or an explicit `close()`.
Closing is safe to repeat; methods that need an active session raise
`RuntimeError` after close.
`board.session` provides typed ADC, GPIO, logical-input, physical-control and
display capabilities over that same native owner. Use `set_adc_samples`,
`gpio_input`, `input_level`, `set_position` and `rotate` to select external
inputs; use the corresponding state methods to inspect their modeled results.
`session.scanout` returns a `ScanoutFrame` with raw bytes, optional RGB pixels,
dimensions and state metadata without advancing guest time. `session.screenshot`
returns a PNG for catalog runtimes after a display frame is configured.
`session.set_image_rgb` supplies a caller-owned RGB image to a named scene input;
`board.read_physical_many` observes several physical memory spans at the same
stopped execution point.
Graph-backed boards expose a typed `execution_policy` property and
`set_execution_policy`, which accepts `"exact"` or `"interactive"`. `exact`
preserves requested instruction boundaries; `interactive` allows
backend-defined block-boundary overshoot. Set the policy explicitly when
needed; construction does not start guest execution or override the native
default. Policy changes require a stopped board.
`boot_to_rec` returns a typed `BootReport`. Its `status` is `"rec"`,
`"instruction-limit"` or `"stopped"`; the report also includes instruction and
guest-time counters, observed mode and menu values, setup status and presses,
plus an optional failure message.

The checked identity catalog is available through
`fujimod.emulator.catalog`. `resolve_reference(body_id, firmware_id)` returns a
`ResolvedSelection` containing the resolved `Soc`, `Body`, `Firmware` and
`Part` records; `resolve_firmware(firmware_id)` derives the referenced body
and SoC. `resolve_configuration(configuration_id)` returns the validated run
configuration with its resolved hardware reference. The generated `TypedDict`
models mirror the Rust catalog schemas and provide field-level editor and
type-checker support. They describe identity records, not local file locations.

The update and ROM files are checked against the selected runtime's recorded
size and SHA-256. A settings file is a 16-byte persistent packet with its
checksum and marker. NAND accepts an emulator-exported committed image or a
complete raw medium whose firmware prefix matches the selected update.

The default USB API is awaitable and runs directly on `asyncio`:

```python
from fujimod import usb


async def identify(device, interface, firmware_limits):
    session = await usb.Session.open(device, interface, firmware_limits)
    try:
        return await session.device_info(usb.InfoSelector.Product)
    finally:
        if session.state == usb.SessionState.Ready:
            await session.close()


async def download(session, address, length, consume):
    async with usb.memory.read_chunks(session, address, length) as reader:
        async for chunk in reader:
            await consume(chunk)
```

`await usb.discover(filter)` inventories devices; `await usb.interfaces(device)`
inspects bulk interface selections. Direct session commands, `usb.memory` range
operations, `usb.data.upload` and `usb.app80.exchange` are awaitable. Chunk
factories acquire ownership synchronously and return async iterators. Use
`async with` or `await reader.aclose()` when ending iteration early.

Use `usb.CommandSequence()` and `await session.execute_sequence(sequence)` for
bounded dependent reads, writes and diagnostic queries. `read(address, length,
expected=None)`, `write(address, data)` and `query(tag, statuses=None)` return
indices into the resulting list of byte strings; write results are empty.
`statuses` is a list of accepted `(primary, secondary)` pairs, defaulting to
`[(0, 0)]`. Reads check their full expected data when supplied. Guards run
beside USB even over a share; a failed guard prevents later commands and fences
the session. Native and Pyodide use the same Rust implementation.

The [mod installation helpers](../../crates/mod-install/README.md) add
`usb.InstallQueries(callback, stock, output, original)` for checked borrowed
callback batches. `call(callback, tag, statuses, output)` returns query/output
indices; `finish()` returns a `CommandSequence`. Use
`usb.CommandSequence.verified_writes(address, data, limits, packet_bytes)` for
bounded write/readback batches. `usb.OwnedUpload(allocation_address,
allocation_size, address, data, piece_bytes, alignment)` validates padding inside
an owned allocation; `await session.upload_owned(image, pointer, original,
capacity_address, capacity)` borrows, uploads and restores its stock destination.
`await session.verify_owned(image, window)` checks every image byte before the
caller publishes hooks or runs initialization. Addresses and ownership must be
qualified by the caller; the helpers never infer them from a camera name.

Python cancellation drops the actual Rust operation. An interrupted exchange
restores a session fenced as `CompletionUnknown`; it sends no cleanup or retry.
A pending operation reports `InFlight`, and another operation fails with
`SessionBusyError`. A pending chunk cancellation closes its reader and restores
the fenced session. `aclose()` can cancel a pending chunk and awaits release of
ownership; `session.abandon()` cancels pending host work and leaves `Abandoned`.
Neither operation can stop camera execution. Creating an unawaited coroutine
sends nothing; discarding or canceling it before dispatch leaves the session
available.

The native binding uses PyO3's `experimental-async` coroutine support and nusb's
wakeups directly, without a background Python worker or an added Rust runtime.
I/O polling releases the GIL. These native coroutines work with `await`,
`asyncio.create_task`, cancellation and separate event loops. The Pyodide wheel
uses an async Python/JavaScript adapter over the same Rust core.

Discover devices with `await usb.discover(usb.DeviceFilter.Diagnostic)` and
inspect interface selections with `await usb.interfaces(device)`. The caller
selects the device and interface explicitly. `DeviceFilter.Fujifilm` includes
other Fujifilm USB modes for inventory, without opening a protocol session.

All wire exchanges run through the async Rust implementation. Commands accept
enums for selectors, views, modes and key states. Numbers represent addresses,
lengths and raw device fields. Research queries require
`usb.DiagnosticTag.research(value)` explicitly.

`Session.read_memory` and `Session.write_memory` issue one camera request each.
`usb.memory.read_range` and `usb.memory.write_range` use the library's
composed operations. `usb.memory.read_chunks` returns an async context-managed
iterator that exclusively owns the session until iteration finishes or closes.
Each yielded buffer can be saved immediately; failures retain the prior chunks.
Configuration writes
are distinct from persistent saves. `diagnostic_query`, `prepare_data_read`,
`prepare_data_write`, `read_data`, `write_data` and `empty_response` expose separate
stages without implicit preparation, firmware retries or cleanup.

Camera errors have a separate exception type from protocol/transport failures.
Device-command exceptions retain `command`, `primary` and `secondary` fields;
failed transfers retain available partial bytes. Diagnostic replies expose a
`CommandStatus` enum alongside raw fields, and console polls expose a
`ConsoleProgress` enum, preserving unknown progress values. No integer return
value is interpreted as an instruction to wait.

An incomplete exchange leaves the session in `CompletionUnknown`, where further
I/O is rejected. `close()` consumes the session on both success and failure:
`Closed` means its two-byte C4 reply was received; a failed or malformed close
leaves `CompletionUnknown` and cannot be retried. The two reply bytes remain
opaque and do not certify a camera power cycle. Releasing Python objects does
not send a camera reset, mode transition or rollback. The consuming workflow owns firmware policy and cleanup
when completion and ownership are known. When a completed command started
application work whose completion or ownership is uncertain, `session.abandon()`
releases host resources without issuing C4 or cleanup. Its terminal state is
`Abandoned`, and all further I/O is rejected. Abandoning cannot stop firmware
execution. Close or cancel an owned range reader with `await reader.aclose()`.

Python protocol/ownership tests use `fujimod.usb.testing`; build the extension
with `scripts/python_extension.py`, then run `pytest bindings/python/tests`.
These tests replay scripted USB exchanges through the production Rust session,
including streaming, ownership, protocol errors and disconnects. No test opens a
camera. Tests also cover
asyncio wakeups, cancellation during I/O, concurrent calls and async iterators.

Use `async with usb.data.read_chunks(session, window, address, length)` for a streamed
E1 memory range. Construct `usb.DataWindow(base, aperture_address,
aperture_length, capacity, usb.OffsetMode.Checked)` from the selected firmware
map; use `OffsetMode.Wrapping` only where firmware's pointer addition is known
to wrap. The reader bounds commands by capacity and the host reply budget and
owns the session until iteration finishes or its context closes, just like the
memory reader. It sends no setup, retry or finalization commands. On failure,
`TransferError.partial` retains the failed command's valid bytes, including any
data delivered with the failed USB completion; previously yielded chunks remain
with the caller. The session then rejects further requests.

`session.empty_response()` sends one E8 and returns its complete four-byte or 32-byte reply; those
bytes do not indicate camera readiness or execution completion.

App80 wire types live in `fujimod.usb.app80`. `CommandCode(value)` is a distinct
immutable identifier from a selected firmware's service table. Construct
`Request(code, arguments)` with at most 32 argument bytes; input is zero padded
and never truncated. `Mailbox(page=..., offset=..., configuration_limit=...)`
requires explicit firmware values. Call
`await usb.app80.exchange(session, mailbox, request)` to stage, publish and read
one result through Rust; it returns the exact 33 mailbox bytes. It holds session
ownership for the complete operation, checks both mailbox and session limits
before sending, and does no polling or retries. The publish write awaits its
ff80 completion; mailbox bytes are not interpreted as a host wait instruction.

For raw transfer logging, create `tracing.Trace(max_events=..., max_bytes=...)`
from `fujimod.usb.tracing` and pass it as `trace=` when opening a
session. `trace.drain()` returns a batch of immutable events plus
`dropped_events`/`dropped_bytes`. Events expose typed `stage`, `phase`,
`request_kind` and `failure_kind`, raw `data`, correlation IDs, endpoint and
transfer counts, and a Unix timestamp. Drain after each operation for complete
logs, including after an exception or failed session open. The recorder remains
usable while a range reader owns the session. USB work never invokes Python
callbacks. Callers choose whether trace loss stops their capture.

Emulator integration tests can use `await testing.peer_session(peer, limits, ep_in=...,
ep_out=...)` instead of a fixed transcript. The peer's `control(envelope: bytes)`
and `write(data: bytes)` return `None`; `read(maximum: int)` returns bytes. These
callbacks advance the virtual endpoint model synchronously and must not reenter
the session or its range readers. Callback errors retain their original Python
exception and fence the session. All framing, validation and compositions still
run in Rust; no USB discovery or physical connection is involved. This injection
path is absent from regular builds. Production USB and trace recording never
call Python callbacks.

`await usb.data.upload(session, data, piece_bytes=..., mode=usb.DataWriteMode.Preserve)`
holds exclusive session ownership across the setup, every E0 piece and final E8. It returns
an immutable `UploadReply` with `setup`, `acknowledgment` and `bytes_written`.
Mode and piece size are required inputs. The operation sends data unchanged,
without padding, readback, retries or destination-buffer policy. A refused setup
is a structured `DeviceCommandError`; a failed exchange fences the session.
A four-byte acknowledgment remains opaque. A 32-byte reply is validated against
the upload setup command and status fields. Neither certifies application completion.

`session.prepare_capabilities()` returns immutable `usb.CapabilitiesReply`
metadata with `byte_length` and `raw`. It does not download or parse the table.
Call `read_data(0, byte_length)` separately when the length is nonzero and within
the chosen host budget; interpret those bytes using the selected firmware.

`session.firmware_download_status()` reads firmware application state once.
`session.clear_firmware_download_status()` is a separate mutating command. Their
`FirmwareDownloadReply` exposes a `FirmwareDownloadState` enum, `failure_code`
when known, and exact `text`/`raw` fields. Writing does not trigger polling.
Clearing a status field does not stop an active firmware write.

Unclassified ff80 requests use `fujimod.usb.research`: construct
`Request(CommandCode(value), body, reply_capacity=...)`, then call
`session.research_command(request)`. The body starts after the 16-byte common
header and needs at least 16 bytes. The reply exposes typed status, primary and
secondary values, a distinct command identifier, body and raw bytes. Only the
common header is validated; the selected firmware supplies other semantics.
There is no implicit padding, polling, data transfer, retry or cleanup. The
[command catalog](../../crates/usb/PROTOCOL.md) describes effects and limits.

## Firmware-selected service controls

Both native Python and Pyodide expose the shared JSON loader through `fujimod.catalog`.
Definitions and command codecs execute in Rust; Python selects typed inputs:

```python
from fujimod import catalog, usb
from fujimod.catalog import service as data

identity = catalog.FirmwareIdentity(firmware_id, firmware_sha256)
definition = data.Definition.from_json(definition_json, identity)
service = usb.app80.Service(definition)
invocation = service.prepare(
    data.ServiceModes(current_service_flags),
    usb.app80.Command.ReadPins(bank=usb.app80.PinBank(0)),
)
reply = await usb.app80.execute(session, invocation)
if isinstance(reply.value, usb.app80.ReplyValue.Pins):
    levels = reply.value.levels
```

`invocation.raw` and `invocation.decode(raw)` support offline inspection; neither
performs USB I/O. `Definition.entries` preserves firmware order and typed codec,
availability and transition metadata. The identity is supplied explicitly and
qualifies the data, not the connected camera or its execution eligibility.

`Command`, `ProfileAction`, `ReplyValue`, `SaveStatus` and word results are enums
with payload variants. Construct `Command.ProfileByte(action=ProfileAction.Read(...))`
or `ProfileAction.Write(...)`; reset is the explicit `ProfileAction.Reset()`.
Quantities use checked `PinBank`, `ProfileIndex` and `WordAddress` types. Selection
errors raise `app80.CommandError` with a typed `CommandFailure` in `.reason`.
Definition errors similarly expose `catalog.DefinitionFailure` through
`catalog.DefinitionError.reason`.
Missing, ambiguous and unreviewed controls do not fall back to another body.

The returned `CommandReply` keeps all 33 raw bytes. Save failure and unknown
result bytes remain data, and no result starts host polling or retry. A malformed
typed reply fences the session. Cancellation and session ownership follow the
same rules as other async compositions. Research invocations remain explicit
through `service.prepare_research(code, modes, arguments)`.

Pyodide uses the same imports and variant constructors through the initialized
JavaScript module. Frozen Python variants own Rust command handles or retain
decoded result values; they do not implement encoding, selection or completion
rules. Shared buffer, handle, error and async adapters live at the package root.

Peripheral commands use `Command.RegisterControl(action=RegisterAction.PowerWrite(...))`,
`Command.I2c(action=I2cAction.Read(port=I2cPort.Port0, command=..., length=I2cLength(...)))`,
`Command.Accumulator(action=AccumulatorAction.StopAndRead)` and
`Command.PeripheralReport()`. `I2cPayload(bytes)` owns a checked 1–10 byte copy.
The write-only route has the distinct `I2cAction.WriteSingle(command=..., value=...)`
variant. Replies are `ReplyValue.I2cData`, `Accumulated` or `PeripheralReport`.
Accumulator values are three low halfwords; report bytes remain opaque.

Media variants include `Command.DisplayOption`, `DisplayMode`, `Sound`, `Hdmi`,
`ParameterService`, `ChargeControl` and `ChargeStatus`. Their inputs use
`DisplayOptionAction`, `SoundAction`, `HdmiAction`, `ChargeAction`, `ChargeQuery`
and distinct enum choices. `SoundClip` checks indices 0–22; `HdmiPacket` copies
exactly 16 bytes; `HdmiValue` checks the 12-bit range. Replies distinguish
`ReplyValue.ChargeApplied`, `ChargePresence` and `ChargeState`, each with its
own status enum and explicit `Unknown(raw_status=...)` variant.

`Command.AudioCapture(count=AudioCount(n), route=AudioRoute.Request82)` checks
0–10 firmware chunks. `Command.AudioPlayback(mode=AudioPlayback.AllLanes)` also
supports `ZeroSecondLane` and `ZeroFirstLane`. These commands operate on camera
buffers; sample transfer stays separate. `ReplyValue.AudioCaptured` and
`AudioPlayed` carry `AudioResult` variants.

`Command.EraseStorage(action=StorageErase.ResetProfiles)` selects one of five
firmware erase/reset controls and returns `ReplyValue.StorageErased` with a
`StorageEraseResult`. `Command.StorageStatus()` returns
`ReplyValue.StorageStatus`: `StorageStatus.Zero` preserves an ambiguous result,
while `Value4`, `Value8`, `Value16`, `Value32` and `Unknown` preserve other encoded
classifications. `Command.FormatStorage()` formats the firmware-selected drive;
`Command.DisplaySetup()` runs its display setup. Both have `NoPayload` results.
All are single firmware commands selected from the supplied definition.

`Command.NandReadBlocks(spare=NandSpare.Include, block_group=n)` reads eight
firmware NAND blocks into its RAM buffer, without downloading them.
`ReplyValue.NandRead.status` is a `NandReadResult` variant. `Command.Option41()`
returns `ReplyValue.Option41.state`, a zero/nonzero `Option41State` rather than
a drive-presence claim. `Command.WlanControl(enabled=Toggle.Enabled)` requests
the control action; the enable path increments a firmware counter.

`Command.RadioControl(action=RadioAction.Primary(...))` takes `RadioMode`,
`RadioChannel` (0–14), `RadioPower` (encoded 0–127) and `Toggle` option.
`RadioAction.Secondary` takes only mode/channel. `Command.SignalStrength` takes
`PeerAddress(data)` (exactly six copied bytes); its `raw_value` is opaque and
can reflect unchanged or uninitialized firmware output. `Command.Iperf()` uses
stored command text and returns an opaque `raw_result` snapshot; the service
can acknowledge before the test runs. Neither observation implies completion,
validated signal units or a host poll.

Service enums, checked quantities, command/action factories, structured replies
and public registrations come from the [shared Rust generator](../generator/README.md).
Builds regenerate them automatically; validation remains in the core.

WPS and station controls use `Command.Wps(action=...)` and
`Command.Station(action=...)`. Their action enums are `WpsAction` and
`StationAction`. WPS starts with `WpsTarget.Broadcast()` or
`WpsTarget.Peer(address=WpsPeerAddress(data))`; exact peers reject a zero first
byte because firmware treats it as broadcast. `PublishProgress()` writes camera
configuration as well as returning a `WpsProgress` snapshot. Station
`StartStored()` uses existing credentials; `Scan(mode=StationScanMode.Mode0)`
and `Mode1` invoke the same studied path. Result snapshots never trigger an
implicit poll. See the [Rust command semantics](../../crates/usb/README.md).

`Command.NetworkReconfigure(action=...)` accepts
`NetworkReconfigureAction.StopServer()`, `ReadConfiguration()`, or
`Apply(number=..., channel=RadioChannel(...), radio=AccessPointRadioMode.Mode2,
security=AccessPointSecurity.Option0, country=CountryBytes(b"US"))`.
Its configuration data is an opaque six-byte snapshot. Transfer preparation,
operation/channel selection, readiness and start are separate `Command` variants;
selectors are `TransferOperation` and `TransferChannel` enums. `TransferResult`
and `TransferPrepareResult` preserve unknown bytes without requesting a poll.

Transfer word and halfword requests use distinct `TransferWordAddress` and
`TransferHalfwordAddress` handles. `TransferReadWord`/`TransferWriteWord` retain
word-result variants; failed reads do not expose a value. Pair requests use
`TransferPairFirst`, with separate result variants. `TransferPayloadByte` owns an
opaque byte for byte/mode-byte/bitfield commands. `TransferOption` and the three
selected-word kind enums replace numeric selectors. Halfword/option results are
raw snapshots, and some stock transfer callbacks receive null data; see the
[Rust command limits](../../crates/usb/README.md) before using those operations.

Trace batches expose `events` for USB and `protocol_events` for FF80/App80.
Both collections use one bounded queue and shared sequence numbers; merge by
`sequence` to inspect nesting. Protocol records include layer, command, exchange,
phase and exact payload bytes. Discard counters include all layers. The native
and Pyodide adapters expose the same observations.
