# USB proxy

`fujimod-usb-proxy` forwards USB operations from a Rust controller (native or
`wasm32-unknown-unknown`) to a WebUSB browser. [Share links](SHARING.md) use
direct WebRTC with encrypted rendezvous. Explicit WebSocket connections use
the relay transport described below. FujiMod's existing
Rust USB sessions, native Python bindings and Pyodide bindings select this
transport using remote discovery. The camera protocol still executes in
`fujimod-usb`; this crate knows no Fujifilm commands, firmware or memory maps.
It is a userspace transport, not a virtual OS USB device or an interception of
arbitrary calls to the external `nusb` crate.

```text
Rust / Python / Pyodide controller
  → WebSocket relay on the development machine
  → Rust/Wasm exporter in the laptop browser
  → browser-permitted USB devices
```

## Relay and browser

Run from the workspace root with a random shared token of at least 32 characters:

```sh
export FUJIMOD_USB_PROXY_TOKEN="$(openssl rand -hex 32)"
cargo run -p fujimod-usb-proxy --features relay --bin fujimod-usb-relay -- 127.0.0.1:5202
```

Expose this loopback service through HTTPS/WSS, for example with Tailscale Serve.
Open `/usb-proxy/` in a WebUSB-capable browser on the laptop, enter the same
token, Connect, then Add USB device. Previously permitted connected devices
are listed on connection. Refresh updates the list; Disconnect releases host
claims. The browser must remain open and awake. Permissions belong to that
browser profile and origin. Chrome may also request local-network permission
for a browser controller connecting to a private relay. HTTPS does not bypass browser/OS protected-interface
restrictions. Control, bulk and interrupt transfers are supported; isochronous
transfers, kernel-driver detach, USB reset and virtual host topology are not.

Both roles authenticate in the first WebSocket frame, with a ten-second
handshake deadline. Tokens never enter query strings or server logs. The relay
accepts same-origin browser WebSockets. For controllers hosted on another origin,
set `FUJIMOD_USB_PROXY_ORIGINS` to an explicit comma-separated list of origins
(e.g. `https://your-host.ts.net:8794`); other browser origins are rejected. Use a private network and TLS; anyone
with relay access and its shared token can operate the exported devices. The
browser explicitly grants device permissions; the relay does not enumerate USB.
There is no firmware setup, debug enable, camera reset or command replay.

## FujiMod controllers

Rust uses ordinary connections and sessions after explicit remote discovery:

```rust,no_run
use fujimod_usb::connection::{discover_proxy, interfaces, Connection, DeviceFilter, TransferOptions};
use std::{num::NonZeroU32, time::Duration};
# async fn example(url: &str, token: &str) -> fujimod_usb::Result<()> {
let devices = discover_proxy(url, token, DeviceFilter::Diagnostic).await?;
let device = devices.first().expect("choose a remote device");
let selections = interfaces(device).await?;
let connection = Connection::open(device, selections[0], TransferOptions {
    timeout: Duration::from_secs(5), max_reply_bytes: NonZeroU32::new(8388608).unwrap(),
}).await?;
let mut session = connection.begin_session(None).await?;
let _ = session.ping().await?;
session.close().await?;
# Ok(()) }
```

Native Python and Pyodide use the same async interface:

```python
from fujimod import usb

devices = await usb.discover_proxy(
    "wss://your-host.ts.net:8795/usb-proxy/ws",
    token,
    usb.DeviceFilter.Diagnostic,
)
choices = await usb.interfaces(devices[0])
# Select the intended device/interface, then use usb.Session.open as usual.
```

Native Python also offers `await fujimod.usb.discover_proxy(url, token,
filter)`. JavaScript offers `usb.discoverProxy(url, token, filter)`; the returned
Device works with the existing Session API. Pyodide must initialize the FujiMod
JavaScript bridge as documented in `bindings/python/README.md`.

Existing native Rust/Python FujiMod callers can select the remote bus through
`FUJIMOD_USB_PROXY_URL=ws://127.0.0.1:5202/usb-proxy/ws` and
`FUJIMOD_USB_PROXY_TOKEN`. Both variables are read at discovery. A configured
proxy never silently falls back to a local camera. Explicit `discover_proxy`
works on every target without environment variables. A remote device's bus ID
is its exporter/device route; address 0 and an empty port chain indicate that
physical host topology is unavailable.

## Low-level controllers

`Client::connect(url, token)`, `devices()`, `submit(device_id, operation,
timeout)`, `expect(of, offset, bytes, mask, length, alternatives)`, `sync()` and `request(device_id,
operation, timeout)` expose the camera-independent transport. Operations are
`Open` (explicit configuration/interface/alternate), `Close` (host resources
only), `ControlIn/Out`, `TransferIn/Out` and `ClearHalt`. Endpoint addresses
include the direction bit. `Completion` preserves payload, actual count and
WebUSB status/DOMException error.

Transfers are pipelined like the Postgres extended-query protocol
([PIPELINE.md](PIPELINE.md)). `submit` and `expect` send at once without waiting;
the exporter runs operations strictly in order and streams a result for each;
`sync` waits for every outcome. A failed operation or expectation makes the
exporter discard everything after it, without touching USB, until the sync.
`request` is one operation and its sync. A controller keeps at most 4096
operations and 1 MiB of payload in flight.

Every message is one [postcard](https://postcard.jamesmunns.com/wire-format)
value: varints, length-prefixed strings and byte strings, one-varint enum tags.
Payloads are inline byte strings and never pass through JSON, UTF-8 or base64.
Both WebRTC and WebSocket transports carry the same messages; a `Hello` of another
version is rejected. `web/vectors.json`, generated from the Rust types, holds
the byte-exact vectors the TypeScript codec is tested against.

The relay reserves a device for one controller socket before forwarding Open,
and routes each controller as its own stream to the exporter, passing the
pipeline through without waiting. Other sockets cannot interleave transfers.
No transfer is retried. A USB error ends the connection after its result; an
expired deadline, canceled call or unexpected message fences the client.
Each operation's USB deadline goes unchanged to the exporter; the controller
waits that long plus 60 seconds for its result. A timed-out browser transfer
stays in its queue until the actual USB promise settles; a new request cannot
overtake it. On disconnect, host-only close is queued after existing work. If
that close is uncertain, the lease remains unavailable until the exporter
disconnects. Reload/reconnect is explicit and does not assert that the camera
stopped executing. FujiMod's own session additionally retains its
`CompletionUnknown` behavior and USB tracing.

Transfers are bounded to 8 MiB and USB budgets to 120 seconds. WebUSB cannot cancel
an in-flight transfer: a host deadline is not evidence of device completion.
There is no automatic reconnect, recovery, persistence or hardware validation.

Run `npm ci` from the repository root before building the relay or Wasm target.
Cargo type-checks the TypeScript transport, then bundles each browser entry
(`peer.ts` for the Wasm controller, `exporter.ts` for the relay page) with
Rolldown into one self-contained module in ignored `web/build/`; generated
JavaScript is never committed.

Run `cargo test -p fujimod-usb-proxy --features relay` for relay/client checks,
and `node --test bindings/javascript/tests/ownership.test.ts` (after building the binding) for exporter ownership checks.
The browser integration test is `crates/usb-proxy/web/browser.ts`; it uses the
real relay and bindings with a scripted WebUSB device. No test accesses a camera.

The browser exporter executes in Rust/Wasm on the same nusb owner used by direct
local sessions. JavaScript routes wire messages only. Build the JavaScript binding
before serving the relay page (`npm --prefix bindings/javascript run build`).
The relay serves generated SDK assets from `bindings/javascript/`; deployments
can set `FUJIMOD_USB_PROXY_BINDINGS` to that built directory.
