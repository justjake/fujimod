# fujimod-send

Sends files from one computer to another over a WebRTC data channel. A share-link
room on the signaling service ([SHARING.md](../usb-proxy/SHARING.md)) carries only
the encrypted connection setup; file bytes go peer to peer.

```sh
# Sending computer: prints the receive command with its link, then waits
fujimod-send send inputs.tar.xz outputs.tar.xz

# Receiving computer
fujimod-send receive -o downloads 'https://fujimod.com/dev-tools/#?remotecontrol=…'
fujimod-send receive -o - 'LINK' > file.bin    # exactly one file, to standard output
```

The sender revokes the link when the transfer ends, whether it succeeded or not.
`--signal URL` selects another signaling service (default
`wss://share.fujimod.com/usb-proxy/signal`).

## Direct only by default

Both sides use `CandidatePolicy::DirectOnly`: no TURN relay is allocated and the
peer's relayed candidates are dropped, so nothing is billed per byte. When no
direct path exists the connection fails. Passing `--allow-relay-costs-money` on
both sides permits a Cloudflare TURN relay, which is billed per byte of egress.

## Exchange

The transfer is the photo sync exchange in
[`protocol/photo-sync.proto`](../../protocol/photo-sync.proto), with the sender in
the camera's role:

1. `Hello` with capabilities `files` and `sha256`; the receiver answers `Welcome`.
2. One `ManifestBatch`: each file by its final path component and size. Names
   must be unique.
3. `Wants`: every offered file, with the length of an existing `NAME.part` in
   the output directory as its resume offset.
4. `ChunkHeader` frames of up to 1 MiB with raw bytes appended, then `FileEnd`
   with the whole file's CRC32 and SHA-256.
5. The receiver checks length and both digests, renames `NAME.part` to `NAME`
   and answers `Committed`. A mismatch deletes the partial file and fails.

The receiver refuses names containing path separators and never overwrites an
existing file.

## Checks

`cargo test -p fujimod-file-send` runs the exchange over an in-memory pipe:
several files, resume, a corrupt partial file and refused names.
`cargo test -p fujimod-webrtc --features signaling direct_only` checks that a
direct-only connection never contacts TURN.
