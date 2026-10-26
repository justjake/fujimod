# Camera workflows: design (agreed with the owner, 2026-10-19)

Camera procedures (service-mode checks, the factory-reset guard, debug mode on
and off, bulk-read setup, mod install) are shared Rust code, so Dev Tools,
the setup pages, Python tools and any other Rust caller run the same checks.
Today several exist only as Python scripts in the x70view repository
(`tools/x70_service_exit.py`, `tools/x70_persist_debug.py`,
`tools/fujicam/bulk_policy.py`).

## Layering

**`fujimod-usb` operations: generic mechanisms.** Anything driven only by
addresses, bytes and limits the caller supplies, with no firmware knowledge:

- Verified writes: write, read back, compare; optional check through an alias
  view. Moves here from `fujimod-mod-install` (`verified_writes`).
- Borrowed callback queries: swap a callback pointer, run a diagnostic query,
  check the reply against accepted status pairs, restore the original once.
  Moves here from `fujimod-mod-install` (`BorrowedQueries`).
- Region checks: read a range, compare with expected bytes or a SHA-256.
- Bulk/slow probe comparison: for a `DataWindow` and probe addresses, read
  each both ways and fail on any mismatch.
- Configuration byte read/write with readback, for live and stored views.

**`fujimod-workflows`: camera and firmware policy.** Replaces
`fujimod-mod-install`; contains no protocol code. It combines `fujimod-usb`
operations with firmware data:

- Firmware tables selected by firmware id (never by model name): addresses,
  expected stock bytes or hashes, write allowlists, cache-helper addresses, the
  bulk window definition, the X70 1.10 range-save adapter.
- Workflows, one module each: `service_mode` (mode check, reset guard),
  `debug_mode` (enter: store `f7=1` keeping stored `a3=0`, read back; leave:
  store `f7=0`), `bulk` (window qualification), `install` (today's
  mod-install policy).
- Ordering and safety rules: check service mode first; arm the guard first;
  verify stock code regions before any write; writes only from the allowlist,
  each read back; at most one storage-write attempt, never retried; callbacks
  restored on every exit path; when completion is uncertain, stop and leave the
  camera state as found.
- Workflows report typed progress events; the Dev Tools log shows them as they
  happen and a CLI can print them.

## Bindings and callers

The JavaScript and Python bindings expose the workflows as they expose
`fujimod-mod-install` today. Per the no-aliases rule the rename updates every
caller and binding in one change. The x70view Python tools become thin callers
of the workflows (or are removed once their callers move).

## Validation

Workflows are tested against the protocol fixtures and the emulator before any
camera run. The X70 1.10 debug-mode facts come from the hardware-validated
records in `research/persistent-debug-save` and `research/service-exit`.
Battery removal as the service-mode exit has not yet been exercised on a camera.
