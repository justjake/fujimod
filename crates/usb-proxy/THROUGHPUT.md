# USB proxy throughput

Run from the workspace root, with the declared Node/Rust tools and Playwright
Chromium installed:

```sh
node crates/usb-proxy/web/throughput.browser.ts
```

The runner builds the release relay and browser modules, starts isolated local
HTTP/signaling services and Chromium, and runs the native Rust controller against
a synthetic shared device. It does not discover or open physical USB devices,
register development shares, restart an installed service, or use saved share
links. The fixture rejects corrupted uploads; Rust verifies every downloaded
byte and every completion count.

Each direction uses 1 MiB and 8 MiB transfers, one full-size warmup followed by
eight measured transfers. MB/s means decimal megabytes per second. Timing
includes the complete request/reply exchange, binary framing, WebRTC/DTLS/SCTP,
browser dispatch and upload verification. Download verification and preparing
the upload input are outside the timer. Directions run sequentially. The default
gate requires **20 MB/s for all four cases**; it fails when a case misses the
target. Results, including failures, go to ignored
`target/usb-proxy-throughput/latest.json`, including each individual transfer's
duration so retransmission stalls remain visible.

For diagnosis, `USB_PROXY_MIN_MBPS=0` reports rates without a speed gate;
`USB_PROXY_BENCH_ITERATIONS` changes the repeat count and
`USB_PROXY_BENCH_PROFILE=dev` measures an unoptimized native controller.
`USB_PROXY_BENCH_RELAY_PORT` overrides the isolated signaling port (15202).
The benchmark never automatically retries a failed transfer or speed check.

## Transport

Native peers run webrtc-rs (`src/peer/native.rs`) on the smol runtime, with
one UDP socket per local IPv4 interface. Relay-only peers bind only the
address that routes to the TURN server. Each socket requests 4 MiB send and
receive queues, covering SCTP's 1 MiB receive window plus DTLS overhead and
scheduling bursts; the OS may clamp that request. A data channel holds at most
4 MiB handed to SCTP but not yet acknowledged; `send` waits above that.

Connections that may reach a browser (`Pacing::Browser`) send in 8 KiB bursts
at 50 MB/s (`src/peer/runtime.rs`). Without pacing, native-to-Chromium uploads
stalled on retransmission timeouts; pacing leaves headroom above the target
while avoiding receive-queue flooding. Native-to-native connections, such as
`fujimod-send`, use `Pacing::Unpaced`. This is a transport ceiling, not a
guarantee of USB-device bandwidth. USB operations remain ordered and
serialized, with the existing ownership, completion-unknown and no-retry rules.

The Python extension builds with the release profile
(`scripts/python_extension.py`), so ordinary tools use optimized transport code.

## Native-to-native benchmark

```sh
cargo run --release -p fujimod-usb-proxy --features relay --bin fujimod-webrtc-bench -- 1024 1024 1
```

The arguments are MiB to send, message KiB and connection count. It starts an
in-process signaling service, connects unpaced direct-only peers on this host
and prints decimal MB/s and process CPU time. It touches no USB device.

## Measurement limits

The initial same-host release baseline measured 0.95/7.73 MB/s for 1 MiB
upload/download and 9.13/6.11 MB/s for 8 MiB upload/download (one measured
transfer per case). The final implementation measured **29.30/30.19 MB/s** for
1 MiB and **29.77/30.05 MB/s** for 8 MiB, with eight measured transfers per case
on the same macOS host. All four default speed gates passed; all measured
8 MiB transfers took 0.276–0.289 seconds. These samples have different repeat
counts and shared-host load; they establish neither a 100-fold gain nor a precise speedup
ratio. The checked-in runner is the reproducible acceptance test.

Same-host Chromium results do not measure physical USB, a second computer,
gigabit Ethernet, simultaneous upload/download, TURN, or high-latency paths.
Those require their own measurements; a camera's protocol, transfer size and
USB controller can limit end-to-end speed below transport capacity.
