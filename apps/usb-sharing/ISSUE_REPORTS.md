# Camera issue reports

`POST /issue-reports` accepts an explicitly selected TAR, its QR descriptor and a
reporter's description. The browser never sends the URL fragment during page load.
The description becomes public in `justjake/fujimod`. Issues carry the labels `bug`,
`from-camera` and the camera's label, such as `x70` ("Fujifilm X70 (EXR
Processor 2)"). The Worker creates a camera's label with that description the first
time the camera reports. `camera-labels.generated.json` lists every known
firmware's label, generated from the catalog and `processor-names.json` by
`pnpm run generate`. The archive is stored in
R2 and a separate read-only share URL is returned to the uploader. Anyone with
that URL can download the report. No GitHub account or credential is needed in the browser.

Intake validates the complete TAR against the descriptor, limits uploads to 8 MiB
and 64 files, and accepts only configured origins. The per-IP limit is five
requests per minute; a Durable Object serializes submissions and caps intake at
50 reports per UTC day. Origin filtering is not authentication. These limits
bound initial abuse; monitor the budget before increasing it.

The client keeps a random idempotency key across retries. R2 receives
`<id>/report.tar` and `<id>/metadata.json` before GitHub is contacted. A successful
response is a durable storage acknowledgment, so the browser can then request
camera deletion. An ambiguous GitHub response returns 202 with the report ID,
retains the archive and never automatically repeats the GitHub request. Retrieve
that ID from the private bucket to reconcile it manually. Receipts also include
`shareUrl`; see [Shared files](SHARED_FILES.md). Records and archives persist until maintainers delete
them; there is no automatic retention policy yet.

## Index

Every object written to the bucket also has a row in the `fujimod-reports` D1
database (`objects` table: key, kind, item ID, state, size, content type,
summary, metadata JSON, start and stored times). The row is written as
`uploading` before any bytes go to R2 and marked `stored`, with its size, when
the upload finishes, so abandoned uploads stay visible. [`bucket.ts`](bucket.ts)
is the only module that writes the bucket; `bucket.test.ts` fails on any other
direct write. Query it with the Cloudflare CLI:

```sh
cf d1 query 73658419-4432-430f-9ac8-53fa5a87d19b \
  --sql "SELECT started_at, state, kind, id, summary FROM objects ORDER BY started_at DESC LIMIT 20"
```

Schema changes go in `migrations/`; apply them with
`node ../../scripts/cloudflare/wrangler.ts d1 migrations apply fujimod-reports --remote`.

## Deployment

Use a Cloudflare credential authorized for this Worker, Durable Objects and R2.
Create `fujimod-issue-reports` as a private R2 bucket (do not enable r2.dev/public
access). Configure `GITHUB_ISSUES_TOKEN` with Issues: write access to
`justjake/fujimod` under the account that should author the issues, then deploy:

```sh
pnpm exec wrangler r2 bucket create fujimod-issue-reports
pnpm exec wrangler secret put GITHUB_ISSUES_TOKEN
pnpm run deploy
```

Never reuse a developer's local GitHub credential implicitly. The token stays in
Worker secrets. The Wrangler v2 migration adds the `IssueReports` Durable Object;
existing USB sharing objects and TURN secrets remain intact.

Maintainers can retrieve a report with their Cloudflare credentials:

```sh
pnpm exec wrangler r2 object get fujimod-issue-reports/REPORT_ID/report.tar --remote --file report.tar
```

Run `node --test worker.test.ts issue-reports.test.ts`. Tests use a real local
Worker/R2/Durable Object runtime and intercept GitHub; no actual issues are posted.
