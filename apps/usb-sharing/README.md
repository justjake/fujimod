# USB sharing Worker

Production rendezvous and temporary Cloudflare TURN credentials for the static
[web application](../web/README.md). This Worker is available for testing at
`https://share.fujimod.com`. The website is intentionally unpublished.
See the [hosting configuration](../web/DEPLOYMENT.md).

Each share room is a hibernating SQLite Durable Object. It routes opaque,
authenticated-encrypted SDP and assigns controller IDs. USB requests and payloads
travel on WebRTC, either directly or through TURN; they never pass through this
Worker. Closing the owner revokes the room and attempts to revoke its temporary
TURN credentials. A room cannot reopen after revocation. Rooms expire after the
configured TURN credential lifetime, including when TURN is not enabled.

## Deploy

Use a Cloudflare account with Workers and SQLite Durable Objects available.
From this directory:

```sh
npm ci
cf auth login
npm test
npm run check
npm run deploy
```

`npm run deploy` uses the Cloudflare CLI's sign-in (see
[`scripts/cloudflare/wrangler.ts`](../../scripts/cloudflare/wrangler.ts)). It fails
until R2 is enabled for the account, because `wrangler.jsonc` binds the private
[issue-report](ISSUE_REPORTS.md) bucket.

The production endpoint is `wss://share.fujimod.com/usb-proxy/signal`.
The Cloudflare release workflow passes this public URL to the static build.
Rebuild/release the site after changing it; no secret is included in the app.
Production builds without a configured service disable Share. Local development and local-preview
builds continue to use the existing local Rust signaling service by default.

Set `ALLOWED_ORIGINS` in `wrangler.jsonc` to the app's exact HTTPS origin(s),
comma-separated. Production allows `https://fujimod.com` and
`https://www.fujimod.com`. Native controllers send no
Origin and are accepted. Origin filtering is browser policy, not authentication.
Room IDs and the URL fragment's separate encryption key remain the share
capability; the Worker never receives that encryption key.

## Website error logs

`POST /error-logs` receives errors from visitors' browsers on the website:
uncaught errors, unhandled rejections, `console.error` calls and emulator faults
([client](../web/src/error-log.ts)). The Worker validates each record (allowed
origin, at most 16 KiB, 30 per minute per IP) and writes it to Workers Logs with
`console.log`; nothing is stored elsewhere. Observability is enabled in
`wrangler.jsonc`.

In the dashboard, open Workers & Pages › `fujimod-usb-sharing` › Logs and filter
on `source = website-client`. Each entry describes one browser, not the Worker:

- `message`: `[browser 3f2a9c10 · Safari 27.0.1 · iOS 390×844] error on
  /dev-tools/: TypeError: …`. The bracketed ID is the first eight characters of
  the page-load session.
- `client`: `session` (one per page load), `page` (origin and path only),
  `build`, `userAgent`, `country`, `reportedAt`, `secondsSinceLoad` and
  `device`: browser, OS, mobile, screen size, pixel ratio and orientation,
  viewport, touch points, CPU cores, memory (Chromium), language, time zone,
  color scheme, home-screen install, network type, cross-origin isolation, and
  WebUSB and WebGPU availability.
- `error`: `kind` (`error`, `rejection`, `console` or `fault`), the full
  `message`, `stack` and, for emulator faults, the `fault` record.

A page sends at most 20 distinct errors. Workers Logs keeps entries for the
plan's retention period only.

## Enable Cloudflare TURN

Create a Realtime TURN key in the Cloudflare dashboard, then store its key ID and
API token as Worker secrets using the interactive prompts:

```sh
npx wrangler secret put TURN_KEY_ID
npx wrangler secret put TURN_API_TOKEN
```

Do not put either value in Git, static build variables, browser code, or share
URLs. The Worker calls the Cloudflare credentials API once per room and sends
only the resulting temporary `iceServers` credentials to its connected peers.
Both secrets absent means STUN-only; partial configuration and issuance failures
reject sharing rather than silently pretending TURN is available.

Browser owners and browser controllers use TURN over UDP, TCP or TLS as offered
by Cloudflare, with direct ICE candidates still preferred. Native Rust/Python
controllers currently use outbound UDP and can connect to the browser owner's
TURN relay. They do not yet allocate their own TURN-over-TCP/TLS connection, so
a native controller on a network blocking all UDP still cannot connect. The
Worker does not add WebSocket USB fallback or resend USB commands.

Configuration defaults:

| Setting                  | Default          | Meaning                                                                    |
| ------------------------ | ---------------- | -------------------------------------------------------------------------- |
| `TURN_TTL_SECONDS`       | 86400            | Maximum share-room lifetime and temporary credential TTL, 60–86400 seconds |
| `TURN_DAILY_ISSUE_LIMIT` | 50               | Global credential issuance reservations per UTC day; failures count        |
| `CONNECTIONS` rate limit | 60/minute/IP     | WebSocket upgrade requests                                                 |
| Room sockets             | 17               | One owner and up to 16 controllers, including pending handshakes           |
| Signaling frames         | 60/minute/socket | Bounded encrypted connection setup, maximum 64 KiB                         |

The issuance quota is **not a bandwidth or billing cap**. Public clients receive
temporary relay access, so configure the quota for the intended audience and
monitor Cloudflare TURN usage. Credentials are shared by the room's participants
and expire even if immediate revocation fails. The Worker deliberately does not
log URLs, SDP, credentials or API error bodies. Worker/Durable Object requests
and storage have their own Cloudflare pricing, separate from TURN egress.

## Test

`npm test` executes the actual Worker under Miniflare/workerd, including room
ownership, opaque routing, revocation, temporary credential delivery, server
secret isolation, issuance quotas and failed configuration. The Cloudflare API
is mocked; these checks incur no Cloudflare charges. `npm run check` bundles a
deployment without uploading it.

The browser integration uses the built JS and native Python bindings and a named
HTTPS development preview. From this directory:

```sh
FUJIMOD_WEB_URL=<named HTTPS URL> PYTHON=<Python with FujiMod installed> node peer.browser.ts
```

It runs a local Workers runtime, exports a scripted WebUSB device, transfers 2 MiB in
both directions through native Python, connects a browser controller, and checks
exclusive ownership. It never accesses physical USB. To force TURN, run a local
TURN server listening on loopback with test-only credentials and loopback
peers allowed. Bind its **relay** socket to a routable local interface address,
so it can send to every advertised local ICE candidate (a loopback-only relay
cannot reach the machine's LAN/tailnet addresses). Then set `TURN_TEST_URL`,
`TURN_TEST_USER`, and `TURN_TEST_PASSWORD`.
The test mocks Cloudflare credential issuance with those test credentials and
asserts that the selected WebRTC candidate is a relay. It does not verify the
live Cloudflare account or cross-network NAT behavior.

References: [TURN credentials](https://developers.cloudflare.com/realtime/turn/generate-credentials/),
[WebSocket hibernation](https://developers.cloudflare.com/durable-objects/best-practices/websockets/),
[TURN pricing](https://developers.cloudflare.com/realtime/sfu/platform/pricing/).
