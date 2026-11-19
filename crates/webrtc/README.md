# fujimod-webrtc

WebRTC data channels between peers that meet in a share-link room.

A share link carries a `share::Ticket` in its URL fragment: the signaling
service URL, a room id and a link key. The room owner joins with
`HostRoom::join` and answers each controller with a `PeerSocket`; a controller
connects with `PeerSocket::connect`. Offers and answers are sealed with the
link key (AES-256-GCM, bound to the room and peer), so the signaling service
relays them without reading them. `signaling` (feature `signaling`) is the
development signaling service; the deployed one implements the same rules.

`CandidatePolicy` chooses which paths ICE may use: `Any`, `RelayOnly` (TURN
only) or `DirectOnly` (no TURN allocation, and the peer's relayed candidates
are dropped). A `PeerSocket` carries ordered messages up to `MAX_MESSAGE`
bytes, split into 16000-byte data channel messages.

## Transport

Native peers run webrtc-rs (`src/native.rs`) on the smol runtime, with
one UDP socket per local IPv4 interface. Relay-only peers bind only the
address that routes to the TURN server. Each socket requests 4 MiB send and
receive queues, covering SCTP's 1 MiB receive window plus DTLS overhead and
scheduling bursts; the OS may clamp that request. A data channel holds at most
4 MiB handed to SCTP but not yet acknowledged; `send` waits above that.

Connections that may reach a browser (`Pacing::Browser`) send in 8 KiB bursts
at 50 MB/s (`src/runtime.rs`). Without pacing, native-to-Chromium uploads
stalled on retransmission timeouts; pacing leaves headroom above the target
while avoiding receive-queue flooding. Native-to-native connections, such as
`fujimod-send`, use `Pacing::Unpaced`.

## Native-to-native benchmark

```sh
cargo run --release -p fujimod-webrtc --features signaling --bin fujimod-webrtc-bench -- 1024 1024 1
```

The arguments are MiB to send, message KiB and connection count. It starts an
in-process signaling service, connects unpaced direct-only peers on this host
and prints decimal MB/s and process CPU time.

## Checks

```sh
cargo test -p fujimod-webrtc --features signaling
```

The tests run signaling and a TURN server in-process. `tests/relay_live.rs` and
`relay_only_data_channel_through_live_cloudflare_turn` are ignored by default;
they contact a live service.
