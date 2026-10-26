# Camera remote control

`fujimod-remote` resolves immutable menu, option and physical-control references
against catalog facts and live memory. Navigation and selection use native
inputs. Reads return `RemoteReadResult` values without advancing a board.

`Remote` has asynchronous, local-future primitives: `set_input`, `read`,
`read_memory`, `readiness`, `frame` and `wait`. The `operations` module composes
navigation, selection, presses, turns, predicate waits and screenshots over
those primitives. Predicate waits retain the store/device reads made by their
predicate. Callers supply absolute deadlines in the connection's clock and own
their assertions.

A read's `State` is the UI mode, menu, node, cursor and current row key.
`readiness()` separately decides whether the firmware accepts a new input: no
pending mode transition, busy UI category, pending input, alert timer or busy
mode register, and every input task receiving. It returns the pending checks
and its own read set, so a wait on menu state never wakes for readiness reads.
`operations::wait_ready` waits for it; Python exposes `Camera.readiness()` and
`Camera.wait_until_ready(deadline=...)`.

Build `Facts` with `Facts::from_selection(&selection, manifest)`. A reference is
resolved on every use; duplicate or missing rows and option names produce an
error with the candidates found. Stock menu IDs are root indices or catalog
roles. Use `via` to identify a shared submenu occurrence. Mod rows and stable
option identities come from the installed build's menu manifest.

`BoardRemote::new(session, facts, options)` borrows or owns an in-process
`Session`. Its checked physical panel and contact acknowledgment driver must
already be connected. Continuations use exact store/device watches and yield
between bounded instruction windows. Checkpoints, restoring, scene input and
explicit clock advancement are board-only methods. A press of a dial or ring
whose motion has its own contacts drives its `{id}.press` actuator. A turn of
an encoder rotates it; a turn of a control whose motion is two contacts presses
and releases its `{id}.increase` or `{id}.decrease` contact once per step, each
edge accepted by the scanner as a button's, and each later press waiting for
input readiness. Frame observations use the selected scanout and a completed-frame counter from the board's device graph.

`Ff80Remote::new(session, facts, options)` borrows or owns an existing USB
session. Options supply firmware transport limits and polling cadence. It uses
range reads and explicit debug command bindings; native scanner names are never
converted into debug command names. An uncertain or failed command is propagated
without replay. Its screenshot completion witness is a newly published
framebuffer pointer, so an unchanged pointer can reach the deadline; passive
`frame()` can still capture the current buffer. Physical identity, permission
and session entry/exit belong to the caller.

Python exposes the same futures through `fujimod.remote.Camera` and
`camera.blocking`. `menu(id, key=..., via=...)`, `option(item, name)` and
`control(id)` create references without a connection. Async operations yield to
Python's event loop on the owning thread. Overlapping operations refuse, and
canceling an operation releases its camera lease. A USB camera leases its
existing session until `camera.close()`.

```python
from fujimod.remote import menu, option

quality = menu("shooting", key=61)
await camera.navigate(quality, deadline=deadline)
await camera.select(option(quality, "NORMAL"), deadline=deadline)
current = camera.blocking.read(quality)
capture = await camera.screenshot(deadline=deadline)
capture.frame.save("quality.png")
```

The `testing` module supplies a synthetic transport for binding tests. Unit and
USB replay tests use synthetic facts and memory only.

`Camera.set_input(control(...), "stage", stage="half_press", deadline=...)`
selects a shutter stage; `"released"` releases all shutter contacts.
`"position", position=...` selects a catalog detent without inventing a release
notification. The board acknowledges the selected physical pose; ff80 resolves
all debug bindings before sending any part of the action and never retries it.

`InputState::TouchStart { x, y }` puts one finger on a touch control at
firmware logical coordinates, `TouchMove { x, y }` moves it and `TouchEnd`
lifts it. The catalog refuses a point outside the touch protocol located for
the control's connection. Only `TouchStart` waits for input readiness. The
board accepts each step once the firmware's touch task has decoded a report
showing it, read from the decoded report the catalog locates: one contact at
the point, or no contact after `TouchEnd`. `operations::tap` and
`operations::drag` compose these steps with no fixed durations. ff80 refuses
touch. Python exposes `touch_start`, `touch_move`, `touch_end`, `tap` and
`drag` on `Camera` and `camera.blocking`.

`Connection::observe` installs a standing observer that runs before every
wait, including those inside an input's readiness and acceptance. The read set
it returns joins that wait, so state a caller tracks (display waits, passive
probes, recording samples) is observed at every continuation boundary.
Python passes it as `Camera.from_board(..., observer=...)`, returning
`{"stores": [(lo, hi)], "devices": [name]}`. `camera.blocking.set_input`
mirrors the async input call.

For asynchronous application observations, `Camera.wait_for(predicate,
reads=reads, condition=description, deadline=deadline)` returns the predicate's
truthy result. The predicate records its memory ranges and device IDs in the
provided `reads` dictionary on each evaluation. The operation clears that set
before each observation and passes it to the remote `wait` primitive when the
predicate remains false. Application exceptions and cancellation propagate
unchanged. A deadline bounds waiting; it does not prove that a device operation
stopped or authorize cleanup.

`RemoteReader.read_memory(address, size)` reads a caller-selected physical
address range and returns its bytes with the backend read set. A Rust
`wait_until` predicate may combine these byte reads with semantic reference
reads; the operation tracks every range and waits on the resulting read set
between predicate evaluations. This leaves byte interpretation and any
firmware-specific meaning with the caller.

`Camera.wait_until_memory(predicate, ranges, deadline=..., condition=...)`
provides the same tracked loop to Python. Each range is a `(physical_address,
size)` pair. The synchronous predicate receives the latest `(address, bytes)`
pairs; Rust repeats the reads, invokes the predicate and waits on the backend
read set between checks.

`Camera.wait_until_stable_memory(ranges, stable_spans=..., equals=...,
stable_comparisons=..., deadline=..., condition=...)` handles the common byte-only
case without calling Python for every sample. The caller supplies the ranges to
read, the byte spans that must remain unchanged, and any exact byte values that
must match. `stable_comparisons` counts consecutive equal comparisons after the
first matching sample. Rust owns the read and wait loop; it assigns no meaning
to the supplied addresses or bytes.

`Camera.from_ff80(..., operation_scoped=True)` borrows the existing session for
each remote operation and returns it before the next application observation.
Its absolute clock is Python's `time.monotonic_ns()`. This permits existing
session commands between operations while preserving refusal of overlapping
USB exchanges. The default connection retains its session lease until close.

Optional `Facts.input_tasks` supplies a checked task-table layout, required
entry identities and receive state, plus mode-qualified busy registers. Input
readiness resolves live task pointers on each observation, refuses missing or
ambiguous tasks, and retains all table, descriptor and receive-state reads in
the continuation's read set. Firmware-specific task identities belong to the
caller supplying these facts.
