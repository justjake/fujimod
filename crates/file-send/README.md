# fujimod-send

Moves files between computers over a WebRTC data channel. A share-link room
on the signaling service ([SHARING.md](../usb-proxy/SHARING.md)) carries only
the encrypted connection setup; file bytes go peer to peer.

```sh
# One-shot transfer
fujimod-send send inputs.tar.xz outputs.tar.xz
fujimod-send receive -o downloads 'https://fujimod.com/dev-tools/#?remotecontrol=…'
fujimod-send receive -o - 'LINK' > file.bin    # exactly one file, to standard output

# Serve a directory until this process stops
fujimod-send serve --ticket share.json ./path/to/dir

# Keep a receiver copy in sync while both commands run
fujimod-send sync -o ./copy 'https://fujimod.com/dev-tools/#?remotecontrol=…'
```

`serve` prints the `sync` command with its link. It accepts every receiver
that connects, including several at once, and sends a new complete snapshot
when the directory changes. `--ticket FILE` keeps the same link across server
restarts; without it the link belongs to this process. A running server keeps
the offered directory readable by anyone with the link until Ctrl-C stops it.

`sync` mirrors the sender's tree: new files and directories appear, changed
files are replaced, and every path in the managed destination that is not in
the latest snapshot is deleted. It is a strict mirror, including files added to
the destination by hand. It retries the connection after a dropped transfer.
Resuming an interrupted file uses a hashed entry under `.fujimod-sync-parts/`
and a digest sidecar holding the source SHA-256, so a partial file is resumed
only when it belongs to the same source bytes. Source paths using the reserved
`.fujimod-sync-parts/` namespace are refused.

The receiver refuses to manage a non-empty directory unless it already contains
the `.fujimod-sync` marker created by an earlier sync. Use a new directory, or
pass `--force` once to adopt an existing one. Symlinks in the served tree are
skipped; the receiver replaces symlinks it finds in managed paths.

## Direct only by default

Both sides use `CandidatePolicy::DirectOnly`: no TURN relay is allocated and the
peer's relayed candidates are dropped, so nothing is billed per byte. When no
direct path exists the connection fails. Passing `--allow-relay-costs-money` on
both sides permits a Cloudflare TURN relay, which is billed per byte of egress.

## Exchange

The transfer is the photo sync exchange in
[`protocol/photo-sync.proto`](../../protocol/photo-sync.proto). One-shot send
uses `Hello` with capabilities `files` and `sha256`, one `ManifestBatch`, then
`ChunkHeader` frames and a `FileEnd` for every `Wants` entry. The receiver
answers `Committed` after checking CRC32 and SHA-256 and moving the file into
place. The sender waits for each file's commit before starting the next one.

Directory sync adds three protobuf fields without a new message or a version
bump:

- `ManifestEntry.directory` marks an empty directory.
- `ManifestEntry.sha256` carries the whole-file digest, letting a receiver skip
  files it already has and detect replacements.
- `ManifestBatch.complete` marks a full snapshot. A sync receiver removes paths
  in its managed destination that this one does not list. One-shot senders
  leave it false, so older receivers ignore the field.

The `sync` capability in `Hello` tells the client it can wait for later
snapshots. Each snapshot is an ordinary manifest/wants/chunks/committed round
on the same data channel; the server offers the next one only when its scan sees
a change.

## Browser receivers

The printed link is the existing Dev Tools capability URL. It is currently a
rendezvous ticket for `fujimod-send sync`; a browser page that speaks these
frames is not built yet.

## Checks

`cargo test -p fujimod-file-send` runs the exchange over an in-memory pipe:
several files, resume, a corrupt partial file, refused names, a nested directory
tree, later snapshots on one connection, removals and the managed-directory
guard. `cargo test -p fujimod-webrtc --features signaling direct_only` checks
that a direct-only connection never contacts TURN.
