# Share links and direct WebRTC

Dev Tools' **Share** button lists the connected camera. Selecting it creates a
capability URL with `#?remotecontrol=<ticket>`. Touch devices invoke the native
share sheet; desktop browsers copy the URL and show a toast. The popup displays:

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
an ephemeral owner room and its clients. It never receives USB requests or
payloads. A room disappears when its owner disconnects; links are not persistent
camera identities. Stop sharing or close the owner's page to revoke the link.

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
Browsers prefer direct connections and can fall back to TURN. Native controllers
use outbound UDP, including to the browser owner's TURN relay; native TURN-over-
TCP/TLS allocation is not implemented. The development Rust service remains
STUN-only. Direct connections work where ICE
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

- **Top bar:** while the camera is shared, the status and Share button become
  one accent **Sharing** chip, with "· N connected" while remote controllers
  are attached. It opens the share dialog, which lists connected controllers
  with their connect reasons. **Stop sharing** beside it revokes the link and
  ends remote sessions but keeps the local camera connection; in local dev,
  auto-share then stays off until the camera reconnects or Share is pressed.
  **Disconnect** still ends everything.
- **Strip:** only while at least one remote controller is connected, one slim
  strip under the top bar, with no dialog, toast or dismiss button: "Your camera
  is being used remotely. Keep this tab in front and your computer on to stay
  connected." If the page was hidden during a remote session, the strip turns
  amber on return and says when the connection was interrupted.
- **Wake lock:** held only while a remote controller is connected; re-acquired
  when the tab becomes visible again; released when the last controller leaves.
- **Clean shutdown:** on page freeze or unload, stop sharing (controllers get a
  clean disconnect) and close the camera session. Switching tabs alone does not
  stop sharing.

## Camera disconnects and battery

Agreed design, 2026-10-18.

- **A share outlives its camera.** The share link (room and key) lasts as long
  as the owner's tab. When the camera disconnects, the owner stays in the room
  and reports the device offline with a reason (`battery`, `unplugged`,
  `not responding`). When a previously approved camera reconnects, the page
  opens it without a picker if the browser allows, otherwise one click on
  **Reconnect camera**. Before re-exporting, with no controller attached, it
  reads the camera's identity (FF80 product name and firmware version, plus a
  per-body identifier from firmware-specific configuration where one is
  known) and re-shares on the same link only if it matches. Local-dev
  registrations stay, marked offline, so the queue keeps the same link.
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
  when the problem is fixed; Close dismisses it early. The chime is unlocked by
  the owner's click on Share or Connect camera. No system notifications.

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
