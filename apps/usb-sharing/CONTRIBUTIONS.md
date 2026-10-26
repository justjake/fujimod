# Private contributions

The browser explicitly uploads contributions to `https://share.fujimod.com`.
No GitHub request is made. The existing private `ISSUE_REPORTS` R2 bucket holds
individual gzip files and contribution metadata. Writes use the shared bucket
helpers to index each completed object in `REPORTS_INDEX` (D1) as a
`contribution`; completion retries repair an interrupted index write. The API exposes receipts,
never object downloads. Requests use `credentials: 'omit'`.

The `CONTRIBUTIONS` Durable Object coordinates intake and a separate global
budget of 50 new contributions per UTC day. Creation shares issue reports'
per-IP limit of five requests per minute (`ISSUE_LIMIT`). Remaining contribution
requests have their own per-IP limit of 60 per minute (`CONTRIBUTION_LIMIT`).
Every route checks `Origin` against `ALLOWED_ORIGINS`, including preflight;
allowed methods are GET, POST and PUT. No cookies or GitHub token are required.

## Protocol

Choose a UUID and retain it with the original files for retries. All routes
start with `/contributions/<id>`. JSON requests use `Content-Type:
application/json`; responses are JSON with `Cache-Control: no-store`.

1. `POST <base>` creates the contribution with
   `{ files: [{ model, serial, name, bytes }] }`. `bytes` is the uncompressed
   size. It returns `{ id, files, finished }`. Repeating identical creation
   returns current status without using another daily slot. Changed file
   identities or sizes return 409. There must be 1–256 distinct files.
2. `GET <base>` returns that status. Each file includes
   its object `key`, `complete`, and `parts: [{ partNumber, etag, bytes }]`.
3. `POST /files/<zero-based-index>` starts an R2 multipart upload, or returns
   the existing file status. Its `uploadId` is retained durably.
4. `PUT /files/<index>/parts/<number>` sends raw compressed bytes. Numbers are
   1–10000. Each request is bounded to 64 MiB, including streamed requests
   without Content-Length. Repeating a saved part returns its receipt. Parts
   are immutable: resume with the same input and compression boundaries.
5. `POST /files/<index>/complete` with `{ parts: <count> }` requires contiguous
   parts. Every part except the last must have the same size, at least 5 MiB.
   The final part may be smaller. Completion retries recover from R2 even if
   its earlier acknowledgment was lost.
6. `POST /finish` with `{ manifest, message }` requires all files completed.
   JSON bodies are bounded to 1 MiB and messages to 12000 characters. The first
   finish payload is retained before writing both metadata objects, so retries
   finish that same payload. Success returns `{ id }`; finished retries do too.

Model, serial and name are at most 200 characters each; characters outside
`[A-Za-z0-9._-]` become underscores. Empty components and `.`/`..` are rejected.
Sanitized duplicate keys are rejected. Objects have these paths:

- `models/<model>/cameras/<serial>/<id>/<name>.gz`
- `contributions/<id>/manifest.json` (the submitted manifest)
- `contributions/<id>/message.json` (`{ message }`)

The client compresses each Blob as a stream and uploads 8 MiB parts sequentially,
retaining only a part for retries. Resuming re-reads an unfinished file to rebuild
its gzip stream but skips saved parts; completed files need no further reads.
Progress counts uncompressed bytes read or already completed. Abort stops reads
and requests. Four attempts per part precede the displayed retry error.

The UUID is the unguessable resume capability: retain it locally and do not
publish it. Abandoned contributions and multipart uploads remain private;
configure R2 lifecycle cleanup for abandoned multipart uploads. An interruption
between multipart creation and saving its ID can leave an orphan upload.

## Local checks

`npm --prefix apps/usb-sharing test` bundles and runs the real Worker with local
Miniflare Durable Objects and R2, without cloud credentials. Run
`npm --prefix apps/usb-sharing run check` for Worker/test type checks and a
Wrangler deployment dry run. Browser protocol tests live beside `contribute.ts`.
