# Camera bug reports

`/bug#?report=<base64url(JSON)>` is a Solid entry point. The fragment is decoded
in the browser; it is never sent as part of an HTTP request. `link.schema.json`
is also the source for the camera's committed, generated C codec. Inline
`archive` bytes are base64url TAR data. Otherwise the page retrieves the named
TAR over the existing diagnostic USB memory protocol, or accepts a local file.
All paths validate the complete byte length, CRC32 and TAR headers before
reporting. The two acquisition paths remain visible side by side, including
platform-specific SD-card directions. After capture, the TAR's files replace the
initial data categories, listed with the shared file list (each file downloads
on its own). Nothing is sent automatically.

**Download all** saves the TAR (local gzip/ZIP). **Contribute** opens the shared
Contribute dialog, which says the message is posted publicly on GitHub. Its
Upload sends the archive and message to the sharing Worker's `/issue-reports`
endpoint. The archive stays private in R2; a server-side token creates the
public GitHub issue without requiring a reporter account. Provisioning, request
limits and reconciliation are documented in `apps/usb-sharing/ISSUE_REPORTS.md`.

**Delete from camera after uploading**, in the dialog, is checked by default and
shown only for a live USB-acquired report. A matching durable storage acknowledgment must
arrive before deletion. Failed uploads retain the file; repeated upload attempts
reuse the same idempotency key. Pending GitHub creation still has a durable
archive receipt and report ID.

**Open Dev Tools** switches to the existing Solid component in place, handing
it the selected device and live session. Its opt-in `connectOnMount` starts
that adapter; it does not open another device chooser or USB session.

## Shared session-file access

The expanded `files` address identifies `x70_file_api` from the session-io library,
the same file operations used by the RAW/photo-session tools. The previous
report-specific channel, commands and nonce are removed. `DeviceFiles` is the
reusable browser reader; `bug/transfer.ts` supplies X70 1.10 address bounds and
uncached RAM mapping, then validates the complete report TAR. Its memory reads
and writes use the established diagnostic USB SDK.

The 64-byte discovery record advertises a build tag, capacity and pointers to
the shared request/state/status/path/buffer fields. Open/read/close operations
are 1/2/3, failed-stream close is 4, existing-file rewrite/write are 5/6, and
remove is 7. The browser validates all pointers and detects another operation
intervening. A timeout never frees or reuses a pending camera buffer.

The scanner wakes the existing report worker only for pending file requests.
That adapter services the common entry independently of the UI task and refuses
service/JIG mode. No stock callback-table redirection, console command or camera
mode change is needed. The QR screen's temporary USB flag and Dev Tools handoff
are unchanged. After restart of the same build, reopen the report screen or
enable USB DEBUGGING to retrieve an earlier saved report.

Deletion consent and read/CRC verification live in the report client. The general
file service handles ordinary paths and is also used by the other session tools.
Tests cover the actual stock-written TAR with UI task 1 suspended, plus existing
RAW rewrite/read callers; physical USB timing is not qualified by emulator tests.

## Browser integration test

Pass the output directory from the successful camera walkthrough. This uses
its actual QR URL and stock-written TAR, checks phone/desktop layout, opens the
file locally, and compares the downloaded archive byte-for-byte. Upload is
intercepted and its exact TAR digest is checked. No outgoing write is allowed
before the explicit upload click; no real issue is submitted. Set `FUJIMOD_BROWSER=webkit` for the Safari engine.

```sh
FUJIMOD_WEB_URL=https://your-host.ts.net/ node src/bug/browser.ts PATH_TO_GUIDE
```

Screenshots and results are written to ignored `apps/web/build/bug-report/`.

## Compact QR descriptor

The URL fragment contains base64url-encoded, minified JSON generated from
`link.schema.json`. Its keys are `v` (format/firmware version, currently X70 1.10),
`b` (eight hexadecimal build digits), `r` (seven-digit report ID), `n` (TAR bytes),
`c` (CRC32), `f` (shared file discovery address), and optional `a` (inline TAR).
The page reconstructs the fixed FUJIMOD/REPORTS directory and R<id>.TAR filename;
the shared file service selects the mounted drive. Expanded upload metadata uses
descriptive names. The full commit and firmware identity remain in the TAR's
`manifest.json`.
