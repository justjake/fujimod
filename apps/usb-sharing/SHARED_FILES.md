# Shared files

The sharing Worker serves `/file/<R2 key>` from the `fujimod-shared-files` bucket.
Uploading an object there publishes it at that URL without a website deployment.
The bucket itself does not need a public R2 domain. There is no public upload API
or bucket listing. Administration uses the authenticated publishing command.

The viewer shows the file, its stored HTTP/custom metadata and a Download button.
Supported image and video MIME types use native media elements. `?type=download`
forces a download-only page; `?type=image` and `?type=video` cannot change the
object's media type. HTML and other non-media types remain download-only. Metadata
is escaped as text, including attributes in the initial Open Graph markup.

`/file/<key>?direct=true` streams any `image/*` or `video/*` object using its
uploaded Content-Type, including MIME parameters. `/media/<key>` serves the same
media for viewer elements and Open Graph. Both use `nosniff`, a restrictive
CSP and single byte-range support. Images and downloads use a sandbox to block
scripts when an SVG is opened directly. Video policy permits same-origin media
so browsers' native players can load the video. Non-media direct requests return 415.
`/download/<key>` always uses
an attachment disposition and `application/octet-stream`. Stored upload headers
are displayed but never replayed as response policy; media Content-Encoding is
preserved to describe the stored representation. These routes read only the
shared-files bucket, not the report/contribution bucket.

Image pages include Open Graph image metadata. For video previews in Discord,
upload a PNG poster at `<video key>.poster.png`. The page then includes
`og:type=video.other`, a direct HTTPS `og:video` URL and the poster's `og:image`.
The initial HTML contains all tags; no browser JavaScript is needed.

## Publish a file

From the repository root, after signing in with `cf auth login`:

```sh
mise run share -- /path/to/demo.mp4
mise run share -- /path/to/image.png --direct
mise run share -- /path/to/demo.mp4 --key clips/demo.mp4 --poster /path/to/poster.png
```

The command prints the public URL. It uses `file` to detect MIME types; override
with `--content-type MIME`. Default keys include the complete SHA-256 and file
name, so different bytes do not overwrite a previous share. `--key` chooses an
explicit object key and replaces any existing object there. Videos get a PNG
poster from `ffmpeg` unless `--poster` supplies one. `--direct` prints a direct
image/video URL instead of a viewer URL. Any regular file can be published;
non-media files get the metadata/download viewer.

## Contribution links

Completed contribution and issue-report uploads receive a separate read-only
capability at `/contribution/<token>`. The `publications` D1 table maps it to an
explicit list of uploaded objects. The upload/resume ID is never used as a share
token. A share URL lists its files, and each file has the same metadata/download
view. A recipient cannot upload, resume, list other submissions or select an
arbitrary object key. Downloads from these links are not cached. Deleting the
publication row revokes the link.

The website copies this URL after a successful upload and announces “Share URL
copied.” If clipboard access fails, the completed upload remains successful;
the dialog offers the URL and a Copy URL button.

## Deploy and test

Create `fujimod-shared-files` once with the authenticated Wrangler wrapper's
`r2 bucket create` command. Apply D1 migrations before deploying:

```sh
node ../../scripts/cloudflare/wrangler.ts d1 migrations apply fujimod-reports --remote
pnpm test
pnpm run test:media:browser
pnpm run check
pnpm run deploy
```

Worker tests exercise local R2 and D1, including media ranges, metadata escaping,
active-document downloads, separate private storage and read-only share links.
