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

## Transport changes

Native WebRTC binds a socket to each advertised interface address and sends
from the source chosen by ICE. A received packet's socket supplies its actual
destination; no socket is created to guess a route for each incoming datagram.
Receive buffers are reused across cancelable waits. Ready network input and
due RTC timers run before additional application writes.
Each host UDP socket requests a 4 MiB receive queue, covering SCTP's 1 MiB
receive window plus DTLS overhead and scheduling bursts. The OS may clamp that
request to its configured limit.

The native SCTP queue is bounded to 512 KiB instead of str0m's 128 KiB default.
Host output is paced in 8 KiB bursts at 50 MB/s. Enlarging the queue without
pacing caused large-transfer stalls in Chromium; pacing leaves headroom above
the target while avoiding receive-queue flooding. This is a transport ceiling,
not a guarantee of USB-device bandwidth. USB operations remain ordered and
serialized, with the existing ownership, completion-unknown and no-retry rules.

The Python extension builds with the release profile
(`scripts/python_extension.py`), so ordinary tools use optimized transport code.

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
