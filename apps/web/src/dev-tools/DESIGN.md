# Dev Tools design

## Cameras and ownership

Dev Tools holds any number of cameras at once, keyed by a stable device id. A
simulated camera is one more entry. Rust owns each camera's USB device for as
long as it is present: local tools and remote controllers are both clients of
that one owner, and nothing else opens, claims or closes the device.

Per camera:

| Field       | Values                                                                                                                                                                                               |
| ----------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `presence`  | `present`, or `gone` after unplugging. A gone camera keeps its log context and returns when the same camera is plugged in again.                                                                     |
| `identity`  | Unknown until first read, then vendor, product, serial and version.                                                                                                                                  |
| `session`   | `idle`, or `held` by a local action (named by its tool) or a remote peer. A session is short-lived: acquire, open ff80, run its operations, close, and release once the interface is fully released. |
| `sharing`   | `off`, or `on` with its link and connected peers. Independent of every other field; peers are only possible session holders.                                                                         |
| `uncertain` | Set when a session ends without a known completion, such as a remote vanishing mid-session. Only that camera needs reopening.                                                                        |

Per page:

- `selected`: the camera the tools act on. The header lists every camera and
  switches between them.
- `log`: the single console log.

Everything else is derived. There is no page-level connected, sharing or
blocked state.

- A tool or Console command is enabled when the selected camera is present,
  idle and not uncertain.
- While a remote holds the session, tools show who is using the camera. While a
  local tool holds it, that tool shows it is running.
- A share link names one camera. Stopping a share removes its peers and changes
  nothing else.

## Log

There is one log, the console. Every entry carries its camera and session
holder, and filters by layer and by camera.

- The transport logs; tools do not. Each ff80 exchange is one FF80 entry
  carrying its meaning (operation, address, length), with the raw bytes in its
  expandable detail.
- The Connection layer holds connection events only (camera present or gone,
  sharing started or stopped, errors) and filters like every other layer.

## Firmware identity

Camera details resolve the service internal version through the catalog's
firmware version map and display the public release when known. An internal
version below that product's highest known version gets an official update
link. Unmapped lower versions say they appear outdated; newer or unparseable
versions do not get that inference. Simulated cameras display their version
without a hardware update prompt.

## Files

Binary data stays binary. Descriptive fields (identity, geometry, times) may
be JSON; bytes read from a camera are stored as binary files, never in JSON as
number arrays, hex or base64.

Tools do not offer their own downloads. Everything a tool produces is added to
one Files pane pinned to the bottom of the tools column, which stays in place
while the user moves between tools. The user collects files across tools and
shares them once.

- Files are grouped by camera: one folder per camera, named for its model and
  serial (`X70 59012345`), holding a flat list of `type-name.ext` files:
  `flash.bin`, `memory-dram.bin`, `memory-boot-rom.bin`,
  `capture-still-shooting-dram.bin`, `capture-af-s-half-press.jpg`,
  `mapping-shutter-dial-candidates.bin`, `configuration.bin`,
  `log.jsonl`.
- Each row shows an icon, name and size, a download arrow and a remove button.
  The camera folder has the same two actions for all of its files.
- **Download all** saves one deflate-compressed zip with a folder per camera
  and a `manifest.json` describing every file (identity, addresses, sizes,
  SHA-256, tool metadata). There is never a zip inside the list.
- **Contribute** (upload icon) opens the Contribute dialog.
- The list keeps a fixed height and scrolls; adding a file never moves the
  tool above it. The newest file is briefly tinted.
- `log.jsonl` is the camera's log entries as JSON Lines (one entry per line)
  with no payload bytes. It covers every log layer, not only console commands.
  Each entry keeps its time, layer, level, direction, decoded message, detail
  and sizes. The transport keeps at most 256 bytes of each exchange's payload
  for the console's hex view; the full data lives only in the tool's own file.
  Removing `log.jsonl` drops the entries logged so far from the file;
  it returns when new entries arrive.
- Files live in browser memory as Blobs until removed or the page closes.

The same file list component renders the emulator's SD card, the Files pane,
the Contribute dialog and the bug report page.

### Contribute dialog

The dialog opens on every Contribute click. It shows the disclosure, the same
file list read-only, a **Message (optional)** field, and **Cancel** and
**Upload**. The footer is one fixed-height row, status on the left and buttons
on the right, so the progress bar, the result and an error (clamped to two
lines) appear without reflowing anything. On success, Close replaces Cancel
and Upload, and the files stay in the pane. A failed upload keeps the message;
Upload retries the same contribution.

Uploads are private and open no GitHub issue. Each file is gzip-compressed in
the browser and uploaded as its own object through R2 multipart parts, so file
size is not limited by the request size. Objects are stored as
`models/<model>/cameras/<serial>/<contribution>/<file>.gz`, with the
contribution's `manifest.json` and `message.json` under
`contributions/<contribution>/`. The service is described in
`apps/usb-sharing/CONTRIBUTIONS.md`.

The bug report page lists its report's files with the same component and offers
Download all and Contribute. Its dialog adds the delete-from-camera choice and
says that the message is posted publicly on GitHub; its upload remains the
issue-report upload.

### Camera mode captures

A camera mode capture re-reads everything except frozen regions. The boot ROM
and other constant regions are read once per camera, by Dump memory. There is no
quick scope.

The browser entry point is `pageCameraModel()` in `usb-camera-model.ts`. It pins
one model to the browser page, outside component ownership, so component and
module hot swaps keep every Rust device owner and session alive. Server renders
create independent models. The model owns connection, peer and FF80 log entries;
Rust supplies decoded traffic, including console command and output bytes, tagged
with the device and session holder. Empty, incomplete console polls wait 50 ms
before returning so callers can poll without a busy loop.

Development and local-preview pages start `startDevShare(model, options)` once.
Each present USB camera gets its own share and registry WebSocket through
`model.setSharing`; remote cameras are never registered as local hardware.
Identity, presence and battery status update that camera's registration. Stopping
a share suspends automatic sharing for that camera until it reconnects. A shared
camera keeps its offline registration while unplugged. Page unload withdraws all
registrations synchronously. These attachments never change device connectivity
or take the local session lock.

## Workbench layout

Until a camera is known, Dev Tools shows the start screen: how to enter service
mode, Connect camera, and Simulated camera, which lists every camera saved in the
emulator and starts the chosen one as a simulated camera over simulated USB. It
stays unavailable until the build can run one, and offers the emulator when no
camera is saved. Once a camera is known:

- A camera strip lists every camera with its state and whether it is shared,
  plus Connect camera.
- The selected camera's details, at the top of the tools column, show its identity (read automatically when it
  appears), its state, the service-mode caution, and its sharing switch.
- Tools start with the numbered path in `GETTING-STARTED.md` (back up flash,
  dump camera state, enable debug mode, restart, states to capture), followed
  by the other tools.
- The tools column is 420 px wide, with the Files pane pinned at its bottom.
- The console log fills the right side. Its camera column and camera filters
  appear only once more than one camera is known.

Controls are neutral and compact; green marks state and the one primary
action, amber marks writes.
