# Camera workflows

`fujimod-workflows` holds camera procedures composed from `fujimod-usb`
operations (design: [`../WORKFLOWS.md`](../WORKFLOWS.md)). Its Rust
implementation is shared by native Python and JavaScript/WebUSB, including
Pyodide, over local USB or USB sharing.

## `debug_mode`: persistent debug access

`debug_mode::enter` / `leave` store the selected camera definition's debug
byte as 1 / 0. Both arm the service reset guard, verify code and storage
geometry, capture the logical storage banks and configuration RAM, and check
the borrowed adapter with a no-write probe before calling the one-byte range
writer once. Readback must differ only at the debug byte; stored reset guard,
settings/profile payloads, scratch and callbacks must remain unchanged.

Successful cleanup leaves debug access off and the reset guard armed in RAM.
Failure restores the original debug flag when completion is known, preserves
unexpected callback/scratch values, and fences failed cleanup. No writer is
retried. Python exposes `Session.enter_debug_mode` / `leave_debug_mode` with
the same camera table and firmware identity arguments as `arm_reset_guard`;
they return `(already_persisted, progress_lines)`.

## `install`: debug mod installation

Installation also uses two generic `fujimod-usb` operations:
`operations::verified::verified_writes` (bounded write/readback batches whose
guards run at the USB owner) and `operations::borrowed::BorrowedQueries`
(borrowed callback words, explicit accepted statuses, one restore per batch).

- `OwnedUpload` checks that the image and its padded bulk extent fit inside the
  supplied owned allocation. `upload` checks the original destination pointer
  and capacity, borrows the pointer, sends E0 pieces, checks completion and
  restores the pointer. `verify` compares every image byte through an explicit
  qualified E1 window and fences the session on mismatch.

The caller qualifies firmware instructions and address views, establishes actual
allocation ownership, authorizes callback targets, maintains caches and verifies
the image before publishing hooks or executing initializers. The crate does not
infer any of these from a model name. Guards are not atomic against device tasks.
Errors and cancellation never retry, repair pointers, free allocations or reset
the device. Retain ownership records and memory when completion is uncertain.

```rust,no_run
use fujimod_workflows::install::{OwnedUpload, UploadDestination};
use fujimod_usb::{Result, ff80::{Session, commands::memory::{DebugMemoryLimits,
    MemoryRange}}, operations::data::DataWindow};
use std::num::NonZeroU32;

async fn transfer(session: &mut Session, allocation: MemoryRange, image: Vec<u8>,
    destination: UploadDestination, limits: DebugMemoryLimits, readback: DataWindow,
    piece_bytes: NonZeroU32, alignment: NonZeroU32) -> Result<()> {
    let upload = OwnedUpload::new(allocation, allocation.address(), image,
                                  piece_bytes, alignment)?;
    upload.upload(session, destination, limits).await?;
    upload.verify(session, readback).await?;
    Ok(())
}
```

Bindings expose `usb.InstallQueries`, `usb.OwnedUpload`,
`usb.CommandSequence.verified_writes` / `verifiedWrites`, and
`session.upload_owned` / `uploadOwned`, `verify_owned` / `verifyOwned`.
Firmware and allocation policy stays with the caller in each language.

Run `cargo test -p fujimod-workflows` for range, padding, packet/batch bounds,
pointer guards, bulk completion, cancellation and full readback checks through
the ff80 core. These checks do not measure physical camera throughput.

## Optional `service-capture` capability

Enable the `service-capture` Cargo feature for `service_capture`. Supply a
facts document following [its JSON Schema](schema/service-capture.schema.json).
`Facts::review` prints the plan and every missing requirement; `validate`
refuses incomplete facts. The offline example performs no device discovery:

```sh
cargo run -p fujimod-workflows --features service-capture \
  --example service_capture_plan -- /path/to/facts.json
cargo test -p fujimod-workflows --features service-capture
```

`Plan::recognize_normal` checks the supplied USB descriptor and returns the
located physical boot inputs. The operator enters service mode and reconnects.
On that service session, `prepare` checks descriptor IDs, exact product and
internal firmware version before changing configuration RAM. It enables only
the located read-gate byte if its current value is zero, then checks stock code.
Bulk mode additionally checks the stock callback, source and positive capacity,
compares small bulk/slow reads, and hashes a larger mapped-code probe. Zero
capacity is refused. Slow mode is explicit and uses the debug memory reader.
There is no automatic fallback between them.

The returned `CaptureReader` borrows the session throughout the streaming read
and retains delivered-byte progress. Callers write each returned chunk to their
chosen output. Facts must supply the readable RAM range separately from the
transfer capacity. This crate contains no model table, density assumption,
storage save, callback replacement or physical-input simulation.

After success, failure or cancellation, remove the battery, disconnect USB,
then reinsert the battery before normal startup. The workflow issues no shutdown,
reset or cleanup command and leaves the volatile read gate for battery removal
to discard. A protocol error retains the USB core's completion fence; no transfer
or write is retried. Live use requires caller authorization and qualified facts.

## Service preparation and captures

`service_mode::prepare_service_session` performs the reset-guard checks and
leaves debug access enabled in configuration RAM until restart. It reports both
original flags and never stores a setting. `arm_reset_guard` retains its existing
behavior of leaving debug access disabled after success.

`state::dump` reads the selected catalog state regions through the qualified
bulk reader. Constant regions must agree across two reads; all-zero and all-FF
constants are omitted as masked. Each delivered region carries a SHA-256.
Its byte-progress callback advances during each bulk transfer, including both
reads of constant regions and excluding path qualification.

`flash::backup` uses the catalog flash contract and a separately built
flash-reader payload. `xp5-cmd-callback` uses a temporary command callback and
zero executable reservation. `exr2-query-heap` uses a stock query callback and
an allocation returned by stock `malloc`. Both verify
the stock reader and transport instructions, translation state, driver state,
payload upload, request/result ABI and update-backed control before delivering
backup chunks. Camera-only chunks are delivered first; the returned SHA-256
covers flash-offset order. Cleanup verifies ownership and restores the callback,
executable reservation, source, scratch and original debug gate. Unknown
completion sends no further commands. A sink error stops capture and runs the
same cleanup.
`flash::Callbacks` supplies chunk delivery and byte progress separately.
Progress starts at zero, advances after each delivered chunk and excludes the
control read. Progress callback errors follow the same cleanup path as sink
errors. JavaScript's final `ByteProgress` argument is optional for both capture
methods.

The EXR II payload reads corrected physical NAND main-area pages, one stock
page-reader call at a time, under the located driver lock. It supports 512-byte
and 2 KiB page contracts. It does not initialize or shut down the high-level
driver and sends no FTL, journal, program or erase request. It preserves the
located DMA and geometry state, restores query outputs after every callback,
and frees the allocation and restores the compiled bootstrap only after known
completion and verified ownership.

JavaScript exposes `Session.prepareServiceSession`, `backupFlash` and
`dumpState`; each holds the session lease for the complete workflow, including
chunk callbacks and cleanup. `flash_reader_plan` validates a built artifact
against the bundled catalog without opening USB.

The `artifact-build` feature adds `flash::build`: native, offline verification
of compiled reader artifacts, initialized update sources, callback BSS, cache
instructions, executable translations, and control hashes. The
`flash_reader_build` example accepts an explicit staging recipe with source
files and a compiler command, then writes separate binary and JSON artifacts.
It replaces the runtime catalog only after every declared reader validates.
EXR II preparation also requires an explicit function index, checks its
allocator/configuration candidates against initialized update bytes, locates
CPU0 cache helpers by immutable fingerprints, and rejects absolute references
into the compiled entrypoints' link image.
