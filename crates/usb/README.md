# FujiMod USB

Async Rust access to Fujifilm's reverse-engineered ff80 service/debug interface.
This first implementation provides nusb discovery and explicit interface
selection, C0/C2/C4 session lifecycle, D0/D1 command exchanges, information
selectors, configuration commands, debug events and console polling, log reads,
diagnostic queries, capabilities metadata, E0/E1/E8 data transfers and composed memory and diagnostic-data range operations.

`ff80/commands/` encodes one firmware request at a time. `operations/` composes commands. nusb handles platform USB access; `emulator` accepts caller-supplied endpoints below the same protocol implementation.
Body/firmware definitions supply memory limits explicitly. The session's
advertised packet size, USB endpoint packet size and the host reply budget
are separate quantities. The library does not enable debug access or change
service mode to make a command work.

```rust,no_run
use std::{num::NonZeroU32, time::Duration};
use fujimod_usb::{Result, connection::{Connection, InterfaceSelection, TransferOptions},
    ff80::commands::info::InfoSelector};

async fn identify(selected: &fujimod_usb::connection::Device, interface: InterfaceSelection) -> Result<String> {
    let connection = Connection::open(selected, interface, TransferOptions {
        timeout: Duration::from_secs(5),
        max_reply_bytes: NonZeroU32::new(1024).unwrap(),
    }).await?;
    let mut session = connection.begin_session(None).await?;
    let product = session.device_info(InfoSelector::Product).await?;
    session.close().await?;
    Ok(product.text)
}
```

The caller chooses the device and configuration/interface/alternate setting.
Opening configures that selected USB interface if necessary, without detaching
kernel drivers or sending camera reset requests. Descriptor inspection requires
exactly one bulk endpoint in each direction; ambiguous layouts are rejected.
Native enumeration and claiming use nusb's `smol` support for blocking OS work;
USB transfers remain asynchronous. No particular executor is required.

Cancellation, timeout, transfer failure or malformed replies leave the session
in `CompletionUnknown`. Further exchanges, including close, are rejected.
Dropping releases host resources; it does not promise that camera execution
stopped. A validated camera error is returned as `DeviceStatus`, including both
raw status values, rather than interpreted as a request to wait or retry.
This conservative initial core has no recovery/reopen automation.

Use `ReadMemory` and `WriteMemory` for individual requests. Their constructors
validate the explicit firmware limits and the 32-bit address extent. Use
`operations::memory::MemoryReader` for a range: each `next_chunk().await` issues
one request and returns one buffer. It bounds each command by firmware limits,
the negotiated packet limit and the host reply budget, and retains the number
of successfully returned bytes. Previously returned buffers remain with the
caller if a later chunk fails.

`Session::sequence` accepts up to 512 `ff80::sequence::SequenceCommand` values:
bounded memory reads (optionally checking expected bytes), writes, and
diagnostic queries with explicit accepted status pairs. Every reply must have
its exact length, command/parameter echoes and expected status or data before
the next command runs. Over a device share these checks run at the exporter,
so a dependent sequence waits once. A failed condition discards later work and
leaves the session in `CompletionUnknown`. Local USB enforces the same checks.
The sequence is not atomic against device tasks and performs no retries or
cleanup. It validates all command limits before its first USB exchange.

Use `operations::data::DataReader` to download a CPU address range through E1.
Supply a `DataWindow` with the diagnostic pointer, allowed address aperture,
per-command capacity and `OffsetMode::Checked` or `OffsetMode::Wrapping` from
the selected firmware's definition. Each pull issues one command bounded by
capacity and host reply budget; the negotiated debug packet size does not limit
bulk data commands. Querying and validating the diagnostic pointer remains a
separate caller policy. No setup command, retry or E8 is sent implicitly.
`PartialRead.data` includes valid fragments from the failed USB completion as
well as earlier fragments in that command. `bytes_completed()` counts only
chunks already returned successfully.

`operations::data::upload` owns the complete setup/E0/E8 sequence. Supply the
setup mode and E0 piece size explicitly. It checks the final receive budget
before setup, stops on a framed setup error, and sends each piece and E8 once.
Its `UploadReply` preserves the raw setup fields and complete acknowledgment (4 or 32 bytes).
Padding, destination-buffer ownership and readback/retry policy stay above this
composition. The debug memory packet limit does not constrain E0 data pieces.

`Session::empty_response()` preserves E8's four-byte or 32-byte raw reply. Uploads
validate the longer reply's setup command echo, reserved word and status fields.
Neither form is a readiness poll or a wait-for-execution API.

The core is tested on native macOS and as Wasm in a JavaScript runtime using
scripted exchanges. nusb's WebUSB backend compiles for `wasm32-unknown-unknown`.
The [JavaScript bindings](../../bindings/javascript/README.md) expose that
backend; physical browser/camera integration remains unvalidated. No hardware
access is part of the test suite. Native Python bindings live
in `bindings/python/` at the workspace root, with an awaitable API. All Rust public I/O is async and the
internal test interface does not require `Send` futures.

Protocol references are `fftlib.py` and `ffjlib.py` from
[tiredboffin/fffw](https://github.com/tiredboffin/fffw/tree/bedc091e0b54a1a34aaf6929dd08e1db36d13b08/ff80).
The upstream MIT notice is retained in [LICENSE.fffw](LICENSE.fffw).

Run `cargo test --workspace --all-features` from the repository root.

The `testing` module supplies strict exchange scripts and synchronous `testing::Peer` completions through the same session implementation. Python's tests use them to verify streaming and ownership across the language boundary. Production emulator integrations use the endpoint API below.

## Emulated endpoints

Implement `emulator::EndpointProvider` to advertise `InterfaceDescriptor` values and claim a caller-selected configuration/interface/alternate setting. Each descriptor lists typed endpoint addresses, transfer kinds and nonzero packet sizes. `Connection::open_emulated` rejects missing or duplicate selections and requires exactly one bulk endpoint in each direction before claiming. It never enumerates devices or falls back to physical USB or a proxy.

```rust,no_run
use std::{num::NonZeroU32, time::Duration};
use fujimod_usb::{
    Error, Result,
    connection::{Connection, TransferOptions},
    emulator::EndpointProvider,
    ff80::commands::info::InfoSelector,
};

async fn identify_emulated(provider: &mut impl EndpointProvider) -> Result<String> {
    let selection = provider.interfaces().first().ok_or(Error::InvalidInterface)?.selection;
    let connection = Connection::open_emulated(provider, selection, TransferOptions {
        timeout: Duration::from_secs(1),
        max_reply_bytes: NonZeroU32::new(1024).unwrap(),
    }).await?;
    let mut session = connection.begin_session(None).await?;
    let product = session.device_info(InfoSelector::Product).await?;
    session.close().await?;
    Ok(product.text)
}
```

`claim` returns an owned `Box<dyn Endpoints>`. The connection, then the session, exclusively owns that claim. `Endpoints` receives raw vendor control OUT requests and bulk IN/OUT buffers, not decoded service commands. The existing ff80 layer still supplies framing, codecs, reply validation, traces and `SessionState`. Bulk buffers have the exact requested length; packet sizes are descriptor metadata, not an instruction to round emulated buffers. IN buffers are bounded by the session's host reply budget; OUT buffers borrow the existing encoded request.

Return `Completion { transferred, status }` for each transfer, including partial byte counts on errors and `TransferError::Cancelled` for an explicit cancellation completion. Only the reported IN prefix is returned. Error prefixes are preserved in `Error::Transfer.partial`. Short control/OUT transfers fail; short IN replies go through the existing ff80 decoder. Counts exceeding the supplied buffer capacity fail as `Error::EmulatorCompletion`.

The host applies `TransferOptions.timeout` separately to claim, each transfer and release. Transfer deadlines return `Error::Timeout`; resource failures and deadlines return `Error::EmulatorResource` and `Error::EmulatorResourceTimeout` with a `ResourceOperation`. Pending futures must yield to the executor. A synchronous callback that does not return cannot be interrupted by a future deadline. Native `EndpointFuture` values must be `Send`; Wasm futures need not be `Send`. The owned endpoint object remains `Send + Sync`.

Dropping an in-flight command future, a transfer failure or malformed reply fences the session as `CompletionUnknown`. No later command or protocol close is sent. Dropping a connection/session must release its claim through the endpoint object's `Drop` without protocol traffic. After a successful ff80 close, the host also awaits `Endpoints::release`; `Drop` must still release resources if that future fails, times out or is cancelled. Claim-future cancellation must release resources acquired before completion. Cancellation stops host waiting and does not certify that emulated device work stopped.

## Additional APIs

App80 transport is the separate [`fujimod-usb-app80`](../usb-app80/src/lib.rs)
crate; command codecs and value types live in
[`fujimod-usb-app80-types`](../usb-app80-types/src/lib.rs), below USB.
Its wire codec has `CommandCode`, bounded `Request` and exact `Reply`.
Identifiers and argument meanings come from the selected firmware's service
table; the codec does not equate command bytes, ff80 status words or firmware
callback dispositions.

Its `operations::Mailbox` composes the ff80 carrier. Supply the selected
firmware's page, command-byte offset and configuration limit. `exchange()`
stages 32 argument bytes, publishes the 33-byte request, awaits that write's
ff80 reply, then reads the mailbox once. The complete operation's host budget
is checked before the first write. Firmware asynchronous completion is handled
by the ff80 exchange; there is no invented mailbox busy bit, automatic poll,
retry, reset or cleanup. A failed or canceled USB exchange fences the session.
Command-specific result decoding and named body APIs belong above this carrier.

`trace::Trace` optionally records the ff80 USB transfers beginning with session
open. Pass a clone to `Connection::begin_session(Some(trace.clone()))` and drain
batches independently of the session. Records retain timestamps, correlation
IDs, endpoint addresses, submitted control/OUT bytes, received IN bytes and
partial failure data. Host future cancellation is recorded without implying
that the camera stopped executing. A USB completion does not certify a valid
firmware reply; protocol decoding still determines session state.

The queue bounds both event count and retained payload bytes. It discards old
records (or an oversized record whole) and reports exact loss counts with each
batch; it never truncates payloads silently or changes USB outcomes. Recording
calls no user code. File writing, trace-loss policy and GUI presentation belong
to callers. With tracing disabled, transfer payloads are not copied for logging.

`Session::prepare_capabilities()` sends one `0x580000` command and returns
`CapabilitiesReply { byte_length, raw }`. A later E1 `read_data` retrieves the
table; that read remains a separate action. Length comes from the firmware and
is not fixed to one body's table size. The library preserves metadata fields
without treating the table as host timeouts or inferring firmware-specific
record meanings. A nonzero device status is a completed `DeviceStatus` error;
a malformed header fences the session.

The [service command catalog](PROTOCOL.md) lists direct APIs, their effects,
and unclassified operations. `firmware_download_status()` reads state once;
`clear_firmware_download_status()` explicitly mutates it. Both retain text and
raw bytes alongside a `FirmwareDownloadState` enum. Writing is application
state, never an implicit instruction to poll, wait or retry.

Use `ff80::commands::research::{CommandCode, Request}` and
`Session::research_command` for unclassified requests. The explicit reply
capacity is checked before sending. The result validates the common header,
preserves raw status/body bytes and performs no command-specific interpretation.
It is distinct from App80's one-byte identifier and mailbox codec.

## Wasm core checks

The workspace's `.cargo/config.toml` enables the WebUSB APIs required by nusb.
Install the Wasm standard library and a wasm-bindgen test runner matching the
`wasm-bindgen` version in `Cargo.lock` (currently `0.2.129`):

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked --root target/tooling
PATH="$PWD/target/tooling/bin:$PATH" cargo test -p fujimod-usb -p fujimod-usb-app80 -p fujimod-usb-app80-types \
  --target wasm32-unknown-unknown --lib --test wasm_core
```

Run from the workspace root with Node on `PATH`, or use `mise run rust-wasm-test`
after installing the runner. The version match is required by wasm-bindgen's
binary format. These tests execute lifecycle, tracing, cancellation and timeout
code as Wasm without device discovery. They do not claim browser permission or
physical USB validation.

Control and bulk transfers share a host deadline. This covers WebUSB, where
nusb cannot apply a control-transfer timeout itself. On expiration the session
is fenced; the camera may still be processing the transfer. Trace timestamps
use the JavaScript wall clock on Wasm and preserve the same epoch/type as native
builds, without calling the unsupported Wasm standard-library wall clock.

Browser bindings expose asynchronous sessions, owned chunk iterators, structured
errors, explicit abandonment and pull-based tracing through one `usb` namespace.
They reside in `bindings/javascript/`, separately from the implementation crate
and `apps/web`. See their README for the static-host build and mock-WebUSB tests.

## Firmware-qualified App80 controls

`fujimod::catalog_service::Definition` loads the selected firmware's service
data against a `fujimod::catalog_firmware_identity::FirmwareIdentity`.
`fujimod_usb_app80_types::catalog::Catalog` resolves its ordered mode rules and
typed codecs; `fujimod_usb_app80::operations::Service` binds those codecs to
that definition's ff80 mailbox. The resulting `Invocation` keeps its request
and carrier together.
Preparation does no I/O and never changes the camera's service mode.

```rust,ignore
use fujimod_usb_app80::operations::Service;
use fujimod_usb_app80_types::catalog::{Command, PinBank};

let identity = FirmwareIdentity::new(firmware_id, firmware_sha256)?;
let definition = Definition::from_json(&definition_json, &identity)?;
let service = Service::new(definition)?;
let request = service.prepare(
    ServiceModes::new(current_service_flags)?,
    Command::ReadPins { bank: PinBank::new(0)? },
)?;
let reply = request.execute(&mut session).await?;
```

These internal service flags are distinct from camera UI or shooting modes.
Missing, ambiguous, disabled or unreviewed entries fail selection. Command
identifiers, slot positions and mailbox locations come from data, with no
fallback to another body. Typed inputs cover pin reads/writes, channel access,
profile-byte bank reads/writes/reset, download status, configuration copy/save,
and service word access. Peripheral codecs add register controls, I²C, accumulator
start/stop and an opaque eight-byte report. Remaining entries retain explicit
research envelopes.
These are wire codecs; a firmware handler may ignore lower-level hardware
errors, and `NoPayload` does not certify a physical effect.

Typed results preserve the raw mailbox alongside `ReplyValue`. A save result
of 1 is `SaveStatus::Failed`, never a host wait instruction; unknown result
values remain `Unknown`. The save output starts at an unknown sentinel. Word
read/write results distinguish values, firmware rejection and unknown status.
The reviewed word codec encodes big-endian address/value bytes and checks its
known alignment/address windows before I/O. Research replies stay opaque.
A wrong command echo in a typed nested reply fences the session, preventing
further requests or implicit close. No polling, retry or cleanup is added.

Run the read-only catalog inspector with an explicit identity:

```sh
cargo run -p fujimod-usb --example service_catalog -- definition.json firmware-id SHA256
```

It prints metadata and performs no USB discovery. The native
[Python bindings](../../bindings/python/README.md#firmware-selected-service-controls)
expose these definitions, enum commands and typed replies through async and
awaitable adapters. The [JavaScript package](../../bindings/javascript/README.md#firmware-selected-service-controls)
exposes the same core through typed factories and discriminated reply unions.
Pyodide exposes the same Python catalog interface through that JavaScript
module, retaining Rust's selection, request/reply codecs and session ownership.

I²C uses checked `I2cLength`/`I2cPayload` values of 1–10 bytes and enum ports.
The protocol's port 3 is represented only by `I2cAction::WriteSingle`; it cannot
be selected for a read or a variable-length write. Read replies expose the
requested bytes while retaining the full raw mailbox. Port names denote protocol
routes, not USB buses or chip identities; some studied routes call stubs.
`AccumulatorAction::StopAndRead` stops accumulation and returns the low 16 bits
of three counters as big-endian halfwords. It is not an observational snapshot.

Register controls expose the established write/forward/pair/no-op modes. The
studied read modes call stubs that leave output uninitialized; they stay behind
the explicit research API rather than returning invented register values.
`PeripheralReport` preserves eight opaque bytes without assigning hardware
meaning to flags derived from unreviewed or stubbed callees.

Media codecs cover display pin/option requests, sound clips/tracks, HDMI packets
and triplets, parameter-service toggling, and charging control/status. Display
options are distinct from camera UI modes and service-dispatch flags. Sound
requests acknowledge queue submission; they do not wait for playback to finish.
Track/operation enums choose canonical wire codes without exposing equivalent
numeric aliases.

`HdmiPacket` owns exactly 16 bytes and `HdmiValue` is a checked 12-bit quantity.
The codec handles the packet's reversed byte order. Studied HDMI callees are
stubs, so these requests do not establish that an output was enabled.
`ChargeAction::EnableAndSetFlag` is one firmware command that first enables the
control and then sets the flag. Charge output starts at an unknown sentinel;
failure, presence, reported register-state groups and unknown bytes have
different enum types. A value of 1 never instructs the host to poll or wait.

Reply decoding exhaustively matches the supported commands and subactions.
Adding a new response-bearing command requires explicit decoding rather than
silently falling through to `NoPayload`.

Audio capture uses a checked `AudioCount` (0–10 firmware chunks) and an
`AudioRoute` request choice. Playback selects both interleaved lanes or zeroes
one lane in the firmware's copied capture buffer. Neither command uploads or
downloads samples: buffer addresses and any transfer workflow belong in the
selected firmware's definitions and higher-level operations. Both return an
`AudioResult`; an early failure may leave the output sentinel unchanged, which
remains `Unknown(255)` rather than success.

`StorageErase` distinguishes the firmware's five erase/reset selectors. These
can clear persistent regions, diagnostics and profiles; they are individual
firmware commands, not a host reset workflow. `StorageEraseResult` retains
success, failure and unknown output. `StorageStatus` preserves the encoded
numbering classification: `Zero` can come from a success or error path, so it
does not establish success or a physical storage condition. `FormatStorage`
formats the firmware-selected drive; `DisplaySetup` runs the firmware's display
setup sequence. Both have no payload result. They neither supply nor imply a
host cleanup/retry sequence.

`NandReadBlocks { spare, block_group }` requests eight firmware NAND blocks
beginning at `block_group * 8`. `NandSpare` selects whether spare bytes are
included. The command populates the firmware's selected RAM buffer; it does not
download it. `NandReadResult` keeps success, the shared rejection/wait-error
code, other errors and unknown bytes separate from host completion.

`WlanControl { enabled }` requests the WLAN control line action. The studied enable
path also increments a firmware counter, so repeating it is not an idempotent
state assignment. `RadioAction::Primary` separates `RadioMode`, checked
`RadioChannel` (0–14), checked `RadioPower` (0–127) and an option flag. The option
occupies the high power-field bit. Firmware transforms the remaining power
field in mode 1 and preserves the raw byte in mode 0; these are encoded values,
not dBm. `Secondary` has only mode and channel. The firmware can shut down LAN
or restart the driver internally, and ignores some lower-level return values;
`NoPayload` does not certify that a radio setting took effect.

`Option41` returns `Option41State` for the firmware getter's zero/nonzero result,
not a drive-presence assertion. `SignalStrength { address }` copies an exact
six-byte address and returns an opaque `raw_value`; firmware state selects the
peer or connected-station path. Some failure paths leave output unchanged or
copy an uninitialized firmware byte, so this is not a validated signal
measurement. `Iperf` uses the command text already stored in camera
configuration. Its opaque `raw_result` may be observed before completion,
because firmware signals the service result before running the test. Reading
or waiting for a later result is an explicit higher-level operation.

`Wps { action }` separates `Stop`, `Start { target }`, `Connect` and
`PublishProgress`. `WpsTarget` is broadcast or a `WpsPeerAddress`: six bytes with
a nonzero first byte. Firmware treats a zero first byte as broadcast and ignores
the remaining bytes, so the exact-peer type rejects that ambiguous input.
Start uses the camera's configured WPS mode/PIN and may later write credential
configuration through its callback. WPS also installs a shared connection
callback that uses the current station-key table; it does not initialize that
table. The library adds no preceding station command or camera-state setup.
`WpsCallbackResult` is a low-byte snapshot;
unknown 255 may still be the initial sentinel. `WpsProgress` decodes the published
phase/connection pair and preserves other bytes. Publishing also writes a
firmware configuration key; it is an explicit mutating command, with no host
poll or assertion that asynchronous camera work has finished.

`Station { action }` exposes stop, start using stored credentials and scan with
`StationScanMode::Mode0/Mode1`. Both scan selectors invoke the same firmware
path. Connection and scan replies preserve opaque callback-result bytes; scan
also preserves the low count byte. The command can wait within firmware for
callbacks or network readiness. Host deadlines still fence uncertain USB work;
there is no implicit retry, credential setup or scan-result download.

`NetworkReconfigure { action }` stops the server, reads the six-byte network
configuration snapshot, or applies the firmware's AP request. `Apply` takes a
byte-sized number (formatted by firmware into its three-byte SSID), checked
`RadioChannel` 0–14, `AccessPointRadioMode::Mode2/Mode3`,
`AccessPointSecurity::Option0/Option2/Option3` and an exact two-byte `CountryBytes`.
The radio choices encode 0/1 and select lower mode 2/3. Other security fields
require legacy-key data that this command cannot provide. Key material, beacon
interval and other options come from firmware state; the library does not build
credentials or a connection workflow. Apply can acknowledge before completing
its work, so its six output bytes remain a snapshot. Unchanged output retains
the initial `ff` bytes.

The service transfer controls are `TransferPrepare`,
`TransferSelectOperation { operation }`, `TransferReady`,
`TransferSelectChannel { channel }` and `TransferStart`. They invoke firmware
control requests; they do not compose USB payload transfer or a setup sequence.
`TransferOperation` names lower operations 0/1/2/3/6, with canonical selectors
0/1/2/3/255. `TransferChannel` names operations 1/3/4 with selectors 0/1/2.
Preparation output distinguishes success 0, preparation error 2 and source status
7 (encoded 3). The other results distinguish 0/1 and preserve unknown bytes.
An operation-selection callback error can leave output unchanged; unknown 255
is data rather than a host completion instruction. Studied transfer callees can
be stubs, so an encoded success does not establish a physical transfer or device
readiness. The host adds no retry, poll or release operation.

Transfer data commands use their own types. `TransferWordAddress` checks
four-byte alignment within the transfer engine's 64 KiB word space; it cannot
substitute for CPU-memory `WordAddress`. Read/write words use big-endian address
and value fields, and expose payload only for result byte 0. `TransferPairFirst`
checks the first pair field's nine-bit range; the second input is 16 bits, while
successful output contains only two bytes. Pair replies retain callback error,
rejection, wait error and unknown status as separate variants.

`TransferHalfwordAddress` forwards the full 32-bit address without inventing an
alignment rule. `TransferPayloadByte` forwards one opaque byte to the byte,
mode-byte and bitfield commands. Their constructors are infallible in Rust;
language bindings still check integer widths. Bitfields take two 16-bit masks,
which firmware expands into separate bit arrays. Byte commands return two raw
bytes. Halfword and option-word replies are explicitly `raw_value` snapshots:
studied lower helpers return success without populating their output buffer.
Other studied transfer helpers pass null callback data, which can cause invalid
firmware memory accesses. These codecs describe the requests and returned bytes;
they do not establish that the current camera state or lower implementation can
perform the operation.

`TransferOption` selects the eight mode/bank combinations. Selected-word commands
use distinct enums for lower request kinds 9–14, 7/8 and 16/17; their values are
16-bit quantities. The stock kind-16/17 command can omit result signaling on its
success path. `TransferLoadMode` selects the firmware's setup sequence, run
sequence or kind-3 request with one/zero. These are single service commands;
firmware performs their internal sequences. The host awaits normal ff80
completion and adds no polling, inferred success, retries or setup/repair.

Configuration service commands use `ConfigurationBank::Bank0/Bank1`,
`ConfigurationMode::Mode0/Mode1/Mode2/Mode3` and explicit `Toggle` fields.
`ReadConfigurationFlags` submits the flags request before reading two bank
snapshots; it is not an observational settings read. `ApplyConfigurationFlags`
only submits flags. The eight returned `raw_data` bytes preserve published
slot order: buffer bytes 1,0,3,2 for the odd bank, then the even bank.
`SelectConfigurationMode` asks firmware to reset its lower mode before applying
its selector. `PackConfigurationOptions` forwards two flag fields and full-byte
upper/lower quantities, which firmware packs at bits 10 and 2 respectively.
`ReadConfigurationOptions` returns the four published bytes as a big-endian
`raw_value`. Studied lower configuration helpers return zero without filling
read buffers, so none of these snapshots certify valid settings. These remain
single firmware commands; the host adds no setup, polling or persistence policy.

`BatteryControl` selects `BatteryAction::ResetPending` or `CallbackWait`.
Reset forwards canonical zero for a byte the reviewed lower helper ignores;
the other action invokes the firmware's battery callback and waits internally.
The handler discards the wait result and publishes no measurement or status.
`InitializeDisplay` submits selector zero and lets firmware use its existing
operation, timing and buffer state. It does not choose an output mode or prepare
that state on the host.

`DisplayPattern` takes `DisplayPatternAction::ResetPreset`, `Bars`, or
`Solid { repeated, middle, low }`. Solid packs the repeated byte into both high
bytes and retains the other two byte fields; the names make no RGB/YUV claim.
Bars draws into firmware-selected buffers, applies the layout and waits for
firmware events. These three codecs expose no result payload. A completed
request does not verify a visible display, and no setup or polling is implicit.

`AdjustmentConfiguration` reads the firmware's configured flag or applies an
`AdjustmentProfile::Profile0`–`Profile6` for a checked `AdjustmentDuration` of
1–15360 seconds. The duration encodes as minutes plus seconds; the maximum uses
255 minutes and 60 seconds. Apply changes service flags before running the
firmware's configuration sequence and uses other configuration already on the
camera. Read status returns configured/not-configured/unknown explicitly.
`SelectAdjustmentMode` requests internal `AdjustmentMode::Mode8/Mode16`, encoding
1/0 respectively. Firmware requires the opposite current mode, changes its flags
before submitting the request, and can fail afterward. No host retry, mode repair
or inferred rollback is performed; these are not camera UI or shooting modes.

`LensSelect` takes `LensTarget::Initialize` or a checked `LensSelectionIndex`.
Indices encode as index plus one; 255 overflows the field and 160 aliases the
lower initialization sentinel, so neither is accepted as an index. The firmware
checks its own available-count limit. `LensSelectionRange::NotRejected` reports
only absence of range rejection, not successful movement. Two additional raw
bytes remain in the reply. The otherwise-unused initialization parameter byte
is canonical zero.

`LensOperation2` and `LensOperation3` invoke the corresponding firmware lens
requests and retain a raw byte initialized to zero by firmware; it is not a
callback-success code. `LensOperation4` and `LensOperation5` have no payload.
These handlers wait for firmware events, but some discard the callback's status.
The host awaits ordinary completion without treating any reply byte as an
instruction to poll, retry or repair the lens state.

`LensOperation6` submits the remaining opaque lens operation. The explicit
`PublishLensCompletionParameter` command writes two configuration bytes: a flag
for the firmware parameter `0x80` and an encoded byte. Other parameter values
publish `(parameter+1)&255`; the `0x80` case publishes 255. The typed reply keeps
the flag and encoded byte separate instead of guessing an index or wait state.

`LensBoundedPosition { position, value }` forwards two byte-sized quantities.
Firmware checks its own position limit. `LensPositionRange::OutOfRange` represents
the published rejection; `NotRejected` does not certify successful movement.
`LensFixedPosition` submits the firmware's fixed request and returns its raw
output byte. `InvokePrimaryParameterCallback` and
`InvokeSecondaryParameterCallback` forward a byte the handler reads, then retain
a raw output snapshot. Studied lower routines ignore that parameter and invoke
the callback; these APIs make no promise of changing a camera setting.

`PrepareAdjustmentOutput` and `PrepareAlternateAdjustmentOutput` are distinct
firmware commands with the same studied internal request sequence. They return
no payload and do not prepare a host bulk read. `QueryAdjustmentOption` takes an
opaque `AdjustmentOptionByte` and returns a raw halfword plus explicit reported
success/failure/unknown status, including raw data on failure. The studied helper
invokes its callback with null data, which the callback dereferences; a reported
success is not proof of a valid physical read. All commands await ordinary
carrier completion without implicit polling, retries or callback repair.

Typed service controls also cover signed lens-position requests, capture/sample
requests, exposure packets and firmware pipeline/mode commands. `SetLensPosition`
accepts an `i16`; `ReadLensParameter` retains the notifier's low 16 bits without
claiming they are a measured position. Pipeline actions encode one firmware
command even when that handler runs an internal sequence. Mode transitions and
completion remain properties of the selected firmware definition.

`Service::prepare_code` selects an explicit `CommandCode` while checking its
codec and active modes. Use it when several identifiers implement the same
operation; ordinary `prepare` rejects that ambiguity. Device-9 reads/writes
accept a two-byte prefix and 0–29 data bytes. Their reviewed layout extends the
mailbox by one byte, and the carrier stages and reads the complete layout.
Payload and reply budgets are checked before any USB traffic.

Constant firmware handlers have explicit `ReportServiceSuccess`,
`ReportServiceFailure` and `ReturnWithoutSignal` requests. Select their identifiers
with `prepare_code` when needed. A handler's C return value is separate from its
completion signal: some error handlers return zero, and no-signal handlers may
leave the request awaiting completion until its deadline. Firmware failures
remain transport errors; the library never turns these values into host retries.

`ReadScan` accepts `ScanSelection::new(index, observed_count)` using a prior
station-scan count. This checks the observation, while the camera can replace
its list before execution. The reply preserves 32 name bytes, with unused tail
bytes initialized to zero. A full-length name makes the reviewed firmware write
its terminator through the table's extra key (configuration key zero on the
studied layout); the host does not add a compensating write.

## Remote USB

The [USB proxy crate](../usb-proxy/README.md) exports browser-permitted devices
through a relay. `connection::discover_proxy(url, token, filter)` returns the
same local-or-remote `connection::Device` used by `interfaces` and
`Connection::open`. Both bindings expose this discovery API, including Pyodide.
Native callers may instead set `FUJIMOD_USB_PROXY_URL` and
`FUJIMOD_USB_PROXY_TOKEN` for existing discovery calls. Remote failure never
falls back to a local device. ff80 commands, trace records and completion
fencing execute in this core for either transport.
