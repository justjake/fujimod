# Share links and direct WebRTC

Each USB camera's card in Dev Tools has a **Sharing** Off/On switch. On creates
a capability URL with `#?remotecontrol=<ticket>`. **Copy link** copies it; on
touch devices **Share link** opens the native share sheet. While shared the card
displays:

**Anyone with a share link will be able to control your USB device.**

Opening the URL loads Dev Tools, selects **Shared camera**, discovers the single
exported device and opens its selected interface automatically. It does not
open a USB picker or fall back to a local device. The owner records **Remote
controller connected** and **Remote controller disconnected** in the connection
log. Discovery connections can be brief; opening a protocol session establishes
its own connection.

## Native tools

The complete browser URL is also the connection setting for any native FujiMod
Rust or Python tool using normal discovery:

```sh
export FUJIMOD_USB_PROXY_URL='https://your-site/dev-tools/#?remotecontrol=...'
# Run the intended FujiMod command normally. No separate token is required.
```

Explicit Rust/Python/JavaScript/Pyodide discovery accepts the same URL:
`discover_proxy(link, "", filter)` / `usb.discoverProxy(link, "", filter)`.
The ordinary Connection and Session APIs execute their existing codecs and
tracing through the selected transport. No firmware command is implemented by
the rendezvous service or WebRTC layer.

## Connection path

WebRTC carries USB requests and completions on a reliable ordered data channel.
The native implementation uses [str0m](https://docs.rs/str0m/); Wasm uses the
browser's RTCPeerConnection through the same crate's bundled JavaScript module.
Messages are postcard values with inline payload bytes, pipelined as described in
[PIPELINE.md](PIPELINE.md). A bounded framing layer splits them into 16,000-byte chunks. Transfer capacities and timeout fencing remain the USB proxy's existing
8 MiB and 120-second limits.

WebSockets provide rendezvous, not NAT traversal. The signaling endpoint is
`/usb-proxy/signal` on the Rust development service or the production
[Cloudflare Worker](../../apps/usb-sharing/README.md). It routes encrypted SDP between
an owner room and its clients. It never receives USB requests or payloads.
Rooms are [persistent](#persistent-links-and-the-native-owner): turning sharing
off revokes the link; the owner leaving keeps it.

Production remains static JavaScript on GitHub Pages. The Worker provides
rendezvous and temporary TURN credentials independently of the static host;
Cloudflare DNS is not required. Set the GitHub repository variable
`FUJIMOD_SIGNAL_URL` to the deployed Worker WebSocket endpoint before releasing
the Pages app. An unconfigured production build disables Share.

For the local Vite development and preview servers, `/usb-proxy/` forwards to
the local Rust service. Static deployments must set `FUJIMOD_SIGNAL_URL` at
build time to a reachable `wss://.../usb-proxy/signal` endpoint. Viewers must be able to reach the signaling endpoint;
a link containing a private tailnet endpoint is limited to that tailnet.

Tickets use independent 256-bit random room IDs and capability keys. The key
stays in the fragment, outside ordinary HTTP requests and access logs. SDP is
AES-256-GCM encrypted with a fresh 96-bit nonce and room/client-bound additional
data. The data channel's DTLS connection is negotiated through that authenticated
exchange. The rendezvous server cannot substitute a peer certificate or read
USB traffic. Possessing the URL authorizes control of the selected device.

The default tickets include Cloudflare's
[public STUN service](https://developers.cloudflare.com/realtime/sfu/get-started/connection-patterns/).
STUN discovers mapped addresses; it does not relay USB data. A configured
production Worker adds temporary Cloudflare TURN credentials to its ready frames,
keeping both the master API token and the temporary credentials out of share URLs.
Browsers prefer direct connections and can fall back to TURN. Native peers allocate
a UDP TURN relay themselves when the join reply carries TURN credentials; native
TURN over TCP or TLS is not implemented. `CandidatePolicy::RelayOnly` (the probe's
`--relay-only` flag) advertises only that relay, so a connection never sends from a
local address. The development Rust service issues no TURN credentials unless a test
configures them. Direct connections work where ICE
finds a viable path, including ordinary LAN and reachable tailnet addresses;
restrictive NATs/firewalls can prevent a connection. See the
[WebRTC connection guide](https://webrtc.org/getting-started/peer-connections)
for the distinction between signaling, STUN and TURN. A one-way URL with
automatic connection still needs rendezvous; eliminating that service requires
an out-of-band offer/answer exchange or an already reachable peer endpoint.

## Ownership and failure

Rust owns each browser USB device for the page's lifetime, whether it is shared
or not. Repeated discovery, direct local sessions and the browser's Rust/Wasm
exporter refer to that same nusb owner. JavaScript routes network messages;
it never opens, claims, releases or closes a second copy of the physical device.
The `/usb-proxy/` relay page also runs this Rust exporter.

Local sessions and remote controllers are clients of one owner. One logical
session can hold the owner's lock at a time. Local sessions acquire it directly;
remote proxy Open/Close operations acquire/release the same lock. Physical
configuration and interface claims persist between logical sessions. A successful
FF80 Session.close sends C4 and releases the logical claim; only device shutdown
awaits physical interface release. No export handoff occurs.

Sharing is an independent network attachment. Starting it does not close a local
session. Stopping it detaches peers and leaves the physical connection available
for local sessions. A controller holding the session lock can refuse another
client's acquisition. Abandoning an active session or an uncertain USB completion
fences that owner until explicitly reconnected; no timeout retries a camera
command, resets USB, changes camera modes, or certifies firmware execution stopped.

The JavaScript binding exposes per-device state and ordered events rather than
a global camera slot. See the [binding API](../../bindings/javascript/README.md#browser-device-ownership).

## Checks

Run Rust protocol, encrypted signaling and relay tests with
`cargo test -p fujimod-usb-proxy --features relay` and exporter tests with
`node --test bindings/javascript/tests/ownership.test.ts` after building the production binding.

`FUJIMOD_WEB_URL=<named HTTPS app URL> node crates/usb-proxy/web/peer.browser.ts`
uses a scripted WebUSB device with the real WebRTC/Rust path to test multi-megabyte
transfers, a Pyodide controller, exclusive ownership and fencing. No physical USB is accessed.

`FUJIMOD_WEB_URL=<named HTTPS app URL> node src/dev-tools/sharing.browser.ts`
from `apps/web/` boots the configured stock-firmware simulator, checks the real
Share popup and clipboard, opens the link in a separate browser context,
executes a firmware console command remotely, exercises native Python with the
URL as its only USB environment setting on a separate cold simulator boot, and
checks link revocation for both runs. Proof
images and results go to ignored `apps/web/build/sharing-browser/`.

Worker runtime and forced-relay integration checks are described in
[`apps/usb-sharing`](../../apps/usb-sharing/README.md#test). The Workers runtime
checks use a mocked Cloudflare credentials API; live Cloudflare deployment and
account credentials are separate from those offline checks.

## Sharing status in Dev Tools

Sharing is displayed independently of the device connection state.

- **Camera card:** while shared, the card shows the link with its copy or share
  button, the warning above, the latest battery level a controller reported
  with its age, and each connected controller with its connect reason. The
  camera strip tags the camera **Shared · N**. Sharing **Off** revokes the link
  and ends remote sessions but keeps the local camera connection; in local dev,
  auto-share then stays off until the camera reconnects.
- **Strip:** only while at least one remote controller is connected, one slim
  strip at the top of the page, with no dialog, toast or dismiss button: "Your
  camera is being used remotely. Keep this tab in front and your computer on to
  stay connected." If a controller disconnects while the tab is hidden, the strip
  turns amber on return and says when it happened, until a controller connects
  again.
- **Wake lock:** held while any camera on this page is shared, so viewers can
  reach it; re-acquired when the tab becomes visible again (browsers drop it
  while the tab is hidden); released when sharing stops.
- **Leaving the page:** closing or reloading the page leaves the room without
  ending it and closes the camera session. Controllers see the camera offline.
  When the page next finds the same camera (FF80 product and serial), it shares
  it again on the same link without being asked.

## Camera disconnects and battery

Agreed design, 2026-10-18.

- **A share outlives its camera.** The share link (room and key) lasts until
  sharing is turned off. When the camera disconnects, the owner stays in the room
  and reports the device offline with a reason (`battery`, `unplugged`,
  `not responding`). The camera card keeps the link and its Copy button while
  the camera is away. When a previously approved camera reconnects, the page
  opens it without a picker if the browser allows, otherwise one click on
  **Reconnect camera** opens the picker. Either way the camera returns to its
  existing device owner, matched by USB vendor, product and serial, never as a
  new camera. Before re-exporting, with no controller attached, the page reads
  the camera's FF80 identity and re-shares on the same link only if its product
  and serial match the shared camera's; a failed read re-shares on the USB
  match alone. Local-dev registrations stay, marked offline, so the queue keeps
  the same link.
- **Battery is read by controllers, not by the sharing page.** FF80 has no
  battery command; battery comes from firmware-specific RAM (X70 1.10:
  `tools/x70_battery.py`). Controller tools read it when a session opens,
  between jobs and periodically during long jobs, and send it to the owner as a
  status message. The owner shows the latest reading with its age and includes
  it in the device list every controller sees. The owner's own console reads
  battery only when it is not sharing.
- **Battery death is a normal disconnect.** The queue starts no job at or below
  PREEND ("waiting for charge"). A disconnect after a PREEND or END reading is
  classified as battery: no alarm, no failure counted, job keeps its place.
- **Unexpected disconnects alert the owner.** The page alerts for what it sees
  (camera unplugged or dead with battery fine); a controller sends an
  attention message for what only it sees (camera present but not
  answering). The alert is a modal that shakes on arrival with a chime, says
  what happened and what to do, updates instead of stacking, and closes itself
  when the problem is fixed; Close dismisses it until the next disconnect. The
  chime is unlocked by the owner's click on Sharing On, Connect camera or
  Reconnect camera. An exhausted battery shows the alert without the chime or
  shake. No system notifications.

### Tool progress and owner countdowns

A claimed controller can send `owner_status { message }` or
`owner_attention { message }` requests on the existing authenticated connection.
Messages contain 1–512 UTF-8 bytes. Neither operation reaches the USB device.
The owner logs status as Info, including expected refusals to continue.
Attention shows a banner and speaks ten through one using browser
`speechSynthesis`. Its ordinary correlated reply is sent after the ten-second
countdown completes; controllers must await it before starting the owner step.
Stopping sharing, losing the camera or disconnecting the controller cancels the
countdown. Missing owner handlers reject the request rather than claiming success.
These operations are separate from battery reports and urgent camera alerts.

Rust USB sessions expose `owner_status(message)` and `owner_attention(message)`,
also available on native Python sessions. Local USB status does nothing; local
USB attention fails because it has no owner page.

## Persistent links and the native owner

Agreed design, 2026-10-30. A camera on a computer's local USB can be shared
without a browser, on a link made before sharing starts that survives camera
restarts and owner restarts.

### Rooms

- **Owner key.** A persistent link has a second secret, the 256-bit owner key,
  kept only in the owner's ticket file and never in the link. The owner joins
  with `{type: "join", role: "host", room, owner_key}`. The first such join
  stores SHA-256 of the key in the room; later owner joins must present a key
  with the same hash. An owner join without `owner_key` to a persistent room,
  or with one to an ordinary room, is refused.
- **Owner away.** When a persistent room's owner disconnects (closed socket,
  error or the 24-hour TURN lifetime), the room is not revoked. Its clients
  get `{type: "host_offline"}`; established WebRTC connections are left alone.
  A client joining while the owner is away is refused with
  `Camera share offline`. A rejoining owner gets fresh TURN credentials.
- **End.** The owner sends `{type: "end"}` to revoke the room, as closing a
  browser share does. A persistent room with no owner for `SHARE_IDLE_SECONDS`
  (default 604800, 7 days) is revoked by its alarm.
- **Creating a link.** Creating a ticket joins once as owner and leaves, so the
  room exists, offline, before any camera is shared. Opening the link then
  shows the camera as offline rather than missing.
- **Browser owners.** Dev Tools shares on persistent links too. The page keeps
  each camera's link and owner key in local storage, keyed by the camera's FF80
  product and serial, and joins again with them after a reload or a dropped
  signaling connection, every 5 seconds until it succeeds or the room refuses
  it for good (ended, wrong owner key). Sharing **Off** sends `end` (joining once
  first if the owner is away) and forgets the stored link; the next **On** makes
  a new link.

### Controllers

`Camera share offline` is a waiting state, not a failure. Dev Tools shows
**Waiting for shared camera** and retries the join every 5 seconds until it
connects, the room ends (`Camera sharing ended`) or the viewer leaves. Native
discovery reports a share that is offline as no device, so tools wait as they
do for an unplugged camera.

### Native owner

`fujimod-share-owner`, built from `crates/usb`:

```sh
fujimod-share-owner create --signal wss://share.fujimod.com/usb-proxy/signal TICKET.json
fujimod-share-owner serve TICKET.json [--device SELECTOR] [--attention-command CMD...]
fujimod-share-owner end TICKET.json
```

- `create` writes a mode-0600 ticket `{ "link": "...", "owner_key": "..." }`
  and prints the link. The link is an ordinary Dev Tools share URL
  (`https://fujimod.com/dev-tools/#?remotecontrol=...`, or `--site`).
- `serve` opens the selected local camera through `DeviceOwners`, joins as
  owner and serves controllers until interrupted. It uses the same Rust USB
  execution as the browser (`fujimod_usb::owner::Exporter`) and the same
  pipeline rules ([PIPELINE.md](PIPELINE.md)). When the camera disconnects it
  stays in the room and lists the device offline with its reason; when the same
  body returns (`DeviceOwners` identity), it shares it again. When its
  signaling connection drops, it rejoins with the owner key, retrying every
  5 seconds. SIGINT or SIGTERM leaves the room without ending it.
- Controller notices are printed one per line: connects, disconnects with
  their reasons, `owner_status` messages and battery reports. `owner_attention`
  runs `--attention-command` with the message as its last argument and replies
  when it exits 0; without the option, attention is refused.
- `end` joins with the owner key and revokes the room. It is refused while an
  owner is serving it.

The native owner gathers the same candidates as any peer: host, STUN-mapped and,
when the join reply carries TURN credentials, a UDP relay.

### Code design

- `apps/usb-sharing/worker.ts`: `ShareRoom` stores `owner_hash` and
  `persistent` in Durable Object storage. `remove()` revokes only ordinary
  rooms; a persistent owner's removal sends `host_offline` to clients and
  schedules the idle alarm. An `end` frame from the owner revokes. The Rust
  development service (`src/signaling.rs`) implements the same rules, keeping
  persistent rooms in memory.
- `crates/usb-proxy/src/share.rs`: `OwnerTicket { link, owner_key }` with
  `create(signal, site)`, `read(path)` and `write(path)` (mode 0600), and
  `ticket()` giving the link's `Ticket`.
- `crates/usb-proxy/src/pipeline.rs`: `Pipeline`, the Rust form of
  `web/pipeline.ts` over `protocol::Message`, with the same tests.
- `crates/usb-proxy/src/peer/native.rs`: candidate gathering and the
  str0m/data-channel pump are shared functions used by both the controller
  (`PeerSocket::connect`, offerer) and the owner (answerer).
- `crates/usb-proxy/src/peer/host.rs` (native only): `HostRoom::join(&OwnerTicket)`
  holds the signaling socket and yields `HostEvent::Controller(id, PeerSocket)`
  and `HostEvent::Left(id)`; each controller has its own `Rtc`, sockets and I/O
  thread. `HostRoom::end()` sends `end`.
- `crates/usb/src/share_owner.rs` (native only): `serve(ticket, selector,
  hooks)` runs `DeviceOwners` hotplug, one `Exporter`, and per controller the
  protocol loop of `web/peer.ts`'s `ShareHost` (hello with the link key,
  `Ready`, notices, `Pipeline`, `owner_status`/`owner_attention` through
  `OwnerHooks`, stop after a USB error, `abandon` on disconnect). It rejoins the
  room when signaling drops. `src/bin/share_owner.rs` is the CLI.
