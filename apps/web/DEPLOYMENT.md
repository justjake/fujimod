# Production hosting

`fujimod.com` and `www.fujimod.com` serve the static website through the
`fujimod-web` Cloudflare Worker. The separate [USB sharing Worker](../usb-sharing/README.md)
serves `share.fujimod.com`; its signaling endpoint is
`wss://share.fujimod.com/usb-proxy/signal`.

## Deploy

Website deployment is independent of mod publication. GitHub Releases contain
`patches.json`, mod patches, and optional `notes.md`. Generated emulator runtimes, the camera
catalog, and feature media are uploaded directly to Cloudflare with the website.
Generated assets stay outside Git.

From `apps/web`, use the locally generated asset directories:

```sh
FUJIMOD_SIGNAL_URL=wss://share.fujimod.com/usb-proxy/signal npm run deploy -- \
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
prereleases, through unauthenticated API requests. `/releases/current` returns
`{tag, name, published_at, prerelease}` with a five-minute edge cache.
`/releases/<tag>/<file>` proxies GitHub asset bytes with a one-year edge cache.
Only successful responses are cached; browser consumers still verify file sizes
and SHA-256. Development and preview servers expose the same routes from
`FUJIMOD_PATCHES` without caching.

Verify both HTTPS hostnames, `/dev-tools/`, camera launch controls, an actual
emulator boot, and feature media after deployment. Firmware DATs are selected
locally in the browser and are not website assets.

## DNS and sharing

Gandi is the registrar. Cloudflare hosts DNS, delegated to `ali.ns.cloudflare.com`
and `rudy.ns.cloudflare.com`. Mail records remain in the zone. Worker custom
domains manage the website records and HTTPS certificates. GitHub Pages is disabled.

The sharing Worker currently provides STUN only. TURN usage monitoring and a
budget cutoff are required before enabling relay credentials; its issuance quota
is not a spending cap.
