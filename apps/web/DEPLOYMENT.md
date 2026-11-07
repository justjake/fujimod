# Production hosting

`fujimod.com` and `www.fujimod.com` serve the static website through the
`fujimod-web` Cloudflare Worker. The separate [USB sharing Worker](../usb-sharing/README.md)
serves `share.fujimod.com`; its signaling endpoint is
`wss://share.fujimod.com/usb-proxy/signal`.

## Deploy

Website deployment is independent of mod publication. GitHub Releases contain
`patches.json`, mod patches, optional `notes.md`, and camera update assets.
Generated emulator runtimes, the camera catalog, and feature media are uploaded
directly to Cloudflare with the website.
Generated assets stay outside Git.

From `apps/web`, use the locally generated asset directories:

```sh
FUJIMOD_SIGNAL_URL=wss://share.fujimod.com/usb-proxy/signal pnpm run deploy -- \
  --runtime /path/to/runtime \
  --media /path/to/feature-media
```

Deploys authenticate with the Cloudflare CLI's sign-in (`cf auth login`) through
[`scripts/cloudflare/wrangler.ts`](../../scripts/cloudflare/wrangler.ts); a
`CLOUDFLARE_API_TOKEN` in the environment takes precedence. No Wrangler login is needed. Install the USB sharing
Worker's dependencies for its pinned Wrangler CLI. The runtime directory contains
`catalog.json` and one qualified bundle per advertised emulator, including
the standalone Wasm module, its loader, cold inputs, and license notices. The media directory contains `media.json` and the
content-addressed MP4 and WebP files it names, keyed by feature card. Each card
shows its newest recording, even when the card's text changed after it was recorded;
staging leaves out recordings of cards the homepage no longer has. Locally, set
`FUJIMOD_MEDIA_ROOT` to a recording workspace to preview its frontier, falling
back to its current promotion. Media and recording outputs are served directly
at workspace-relative URLs. `FUJIMOD_MEDIA` selects a standalone media directory
at `/artifacts/docs/`; production builds read this explicit directory.

The command validates and stages these assets, runs application checks and tests,
builds the website, and deploys `dist/client` using `wrangler.jsonc`. Staging fails
before replacing installed assets if a required file or qualification is missing.
Use `--prepare-only` to stage assets without building or deploying. The generated
`site-assets.json` records file sizes and SHA-256 hashes for deployment verification.
A mod release never triggers or supplies a website deployment. The Worker
selects the newest published GitHub release with `patches.json`, including
prereleases, through GitHub API requests. `/releases/current` returns
`{tag, name, published_at, prerelease}` with a five-minute edge cache. The
`RELEASES` KV namespace (`fujimod-releases`) holds the current release, every
release file served, and the OTA manifest and store copies used when GitHub
fails. `/releases/current` answers from KV and refreshes it
from GitHub in the background; release files fall back to KV when GitHub fails.
GitHub's unauthenticated limit is shared by every Worker on an egress address,
so only the first lookup after creating the namespace waits on GitHub.
Set the `GITHUB_TOKEN` Worker secret to authenticate these API lookups with a
token; it increases the shared limit and is never sent to asset downloads.
Set it once from `apps/web` with
`node ../../scripts/cloudflare/wrangler.ts secret put GITHUB_TOKEN`.
`/releases/<tag>/<file>` proxies GitHub asset bytes with a one-year edge cache.
Only successful responses are cached; browser consumers still verify file sizes
and SHA-256. Development and preview servers expose the same routes from
`FUJIMOD_PATCHES` without caching.

## OTA updates

Camera updates use the same release. `GET /ota/<firmware-id>/latest.json`
answers with the release manifest and `GET /ota/store/<XXXXXXXX.YYY>` with the
store file of that content-addressed name. Both come from the newest published
release that carries `patches.json`. `/ota/timezone` answers with
`{"timezone": <request.cf.timezone>}` for clock setup.

`fujimod-release-manifest` writes a release directory containing `latest.json`
and one file per distinct SHA-256. Upload it with the manifest renamed to
`<firmware-id>-latest.json`, because GitHub release assets have flat names:

```sh
gh release upload <tag> \
  release/latest.json#x70-110-latest.json \
  release/[0-9A-F]*.???
```

The store file name is the full asset name, so every release must include each
store file its manifest names. The Worker rejects a manifest whose `firmware`
field differs from the requested firmware id. Requests never forward the
client's authorization or cookie headers to GitHub.

The current-release lookup and each firmware manifest are cached at the edge
for five minutes and mirrored in KV. Camera requests inside that window answer
without contacting GitHub; an origin error serves the last copy. Manifests go
to cameras with `Cache-Control: no-store`. Their edge copies carry an internal
fetch time and stay for a year, so they remain available during an outage. The
`GITHUB_TOKEN` secret authenticates GitHub API lookups; store assets are
downloaded without it.

Store files are immutable. On a cold cache the Worker fetches the whole asset
once, never forwarding a client's `Range` header, then caches the bytes at the
edge and in the `RELEASES` KV namespace and answers ranges from that copy.

A single `bytes=` range is answered with `206` and `Content-Range`, or `416`
with `Content-Range: bytes */<size>` when unsatisfiable. Other range syntax
receives the whole file with `200`. `HEAD` answers with the same status and
headers as `GET` and no body. The Worker follows GitHub's redirect to asset
storage itself, so these routes answer directly over HTTPS without redirecting
the camera.

A published release becomes visible within five minutes. To publish
immediately, purge the cached release lookup and each changed manifest by URL:

```sh
curl -sS -X POST "https://api.cloudflare.com/client/v4/zones/$CLOUDFLARE_ZONE_ID/purge_cache" \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H 'Content-Type: application/json' \
  --data '{"files":[
    "https://fujimod.com/ota/release.json",
    "https://fujimod.com/ota/x70-110/latest.json"
  ]}'
```

`/ota/release.json` is the internal cache key for the current-release lookup
and is not a public route. The Worker reads and writes these entries through
the Cache API under those exact URLs, so a URL purge clears them in every data
center.

Verify both HTTPS hostnames, `/dev-tools/`, camera launch controls, an actual
emulator boot, `/ota/timezone`, a release manifest, and feature media after
deployment. Firmware DATs are selected locally in the browser and are not
website assets.

## Accounts

The Worker mounts Better Auth at `/api/auth/*` ahead of static assets and stores
users, sessions, linked Google accounts, devices and device codes in the
`fujimod-auth` D1 database (`AUTH_DB`). `www.fujimod.com` signs in through the
apex origin, so one Google redirect covers both hostnames.

The Google OAuth client `FujiMod Web` needs these authorized redirect URIs, and
the JavaScript origins without the path:

```text
https://fujimod.com/api/auth/callback/google
https://cozycat.tail2b5ca.ts.net:8794/api/auth/callback/google
```

The Google Auth Platform data access page must list `openid`,
`userinfo.email`, `userinfo.profile`, `photoslibrary.appendonly`,
`photoslibrary.readonly.appcreateddata` and `photospicker.mediaitems.readonly`.
The scope list lives in [`src/auth/options.ts`](src/auth/options.ts).

The Worker needs `GOOGLE_CLIENT_SECRET` and `BETTER_AUTH_SECRET` secrets;
`GOOGLE_CLIENT_ID` is a plain variable in `wrangler.jsonc`. Local development
reads them from `.dev.vars` (see `.dev.vars.example`).

Schema changes come from Drizzle: edit `src/db/`, then `pnpm run db:generate`
and apply the new migration.

```sh
pnpm run db:migrate:list
pnpm run db:migrate:remote
```

Applying migrations uses the Cloudflare CLI (`cf`), which shares this
repository's sign-in with Wrangler. `pnpm run db:migrate:local` targets the local
D1 copy instead.

## DNS and sharing

Gandi is the registrar. Cloudflare hosts DNS, delegated to `ali.ns.cloudflare.com`
and `rudy.ns.cloudflare.com`. Mail records remain in the zone. Worker custom
domains manage the website records and HTTPS certificates. GitHub Pages is disabled.

The sharing Worker issues Cloudflare TURN credentials when its `TURN_KEY_ID` and
`TURN_API_TOKEN` secrets are set. Its daily issuance quota (`TURN_DAILY_ISSUE_LIMIT`)
is not a spending cap; monitor TURN usage in Cloudflare.
