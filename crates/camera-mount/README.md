# Camera mount

`fujimod-mount` exposes one connected camera through macOS FSKit:

- `sd/`: the camera’s SD0 namespace, with regular-file reads, writes, creation,
  truncation and deletion.
- `memory`: a read-only file with a 4 GiB logical size. File offsets are camera
  addresses. Only ranges admitted by the selected firmware’s readable memory
  map can be read; holes return `EIO`.
- `regions.json`: the readable ranges, with names, addresses and sizes.

There is no `debug/` wrapper. Memory reads are live, not a snapshot. The adapter
requests that FSKit inhibit kernel offloaded I/O; applications should use
`pread` or seek/read and handle I/O errors. A regular address-indexed file is
the POSIX interface here: no privileged character-device driver is needed.
The current adapter does not provide a custom `mmap` interface.

## Run

Install and enable the signed [FSKitBridge 0.2.0 application](https://github.com/debox-network/FSKitBridge/releases/tag/v0.2.0),
following its extension setup instructions. macOS 15.4 or newer is required.
Building requires Rust and `protoc` (`brew install protobuf`).

The camera must already provide an available ff80 debug service. Select a
firmware with a card capability in the bundled catalog. The command verifies
camera identity and the selected code checks before starting the mount.

```sh
mkdir -p /tmp/fuji
cargo run -p fujimod-camera-mount --bin fujimod-mount -- \
  --firmware x100v-301 /tmp/fuji
```

Use `--device` (or `FUJIMOD_USB_DEVICE`) to select a bus, bus/port key or serial
when more than one camera is attached. `--read-limit` and `--write-limit` set
ff80 command payload limits; both default to 256 bytes. These limits do not
limit qualified E0/E1 bulk transfers. Ctrl-C synchronizes and
unmounts; keep the process running while using the volume.

SD paths currently use 8.3 names and fewer than 48 bytes including drive and
separators; regular files are limited to 4 GiB minus one byte. Existing
directories can be traversed. Directory creation/removal,
rename, extended attributes, links, ownership and permission changes are not
implemented. Applications that save through a temporary file and rename will
receive an unsupported-operation error. Capacity statistics are unavailable.

The X70 installed file service requires descriptor ABI 3 for mounted access.
An older service is rejected during the root-directory preflight. The stock
SDK backend needs no uploaded executable code.

## Architecture and performance

The namespace and policy live in Rust independently of the platform adapter.
The worker holds device descriptors while idle. Each bounded SD or memory
operation acquires a fresh ff80 session, completes its callbacks, closes all
file handles, releases its allocation, restores borrowed controls, and closes
the session before replying. Other tools can use the camera between operations;
there is no mount-lifetime session or idle USB claim. Synchronize and unmount
need no new session because writes are already committed before replying.

The installed-service descriptor address is cached as a discovery hint. Each
transaction revalidates its descriptor, pointers and ownership; a missing hint
triggers rediscovery. Firmware identity and stock code checks are repeated in
the session that uses them. Regular-file I/O transfers bounded ranges rather
than downloading and replacing whole files. Extending a file zero-fills its gap.

Read transport selection compares CPU command count with qualified E1 setup,
probes and transfers. It supports every published bulk source family: query,
32-bit pattern publication and 64-bit zero-base publication. The latter two
borrow and restore the located source words without running pattern generation
or filling diagnostic scratch. DMA rounding must remain inside the admitted
aperture. Small control reads use CPU commands when cheaper. Stock SDK file
buffers use their checked E1 source pair for larger reads.

Owned SD buffers use E0 uploads when a checked upload destination contract is
published and complete padded pieces fit the buffer and registered capacity.
The uploader guards and restores the destination pointer before the file
operation continues, including completed transport refusals. Currently the
X70 card profile publishes this contract. Cameras without a qualified E0
contract use CPU writes; an unavailable fast path is never guessed.
`--read-limit` and `--write-limit` apply to CPU commands, not E0/E1 pieces.
These choices minimize known protocol work; actual end-to-end throughput has
not yet been benchmarked on a mounted camera.

Memory contents are never cached. Directory snapshots are cached for 250 ms,
invalidated by mutations, and carry generation cookies for enumeration.
Unknown completion fences the mount, prevents reacquisition and retains any
reservation that an outstanding storage callback might still use. It never
closes or reclaims an uncertain camera session.

The macOS adapter pins [fskit-rs 0.2.0](https://docs.rs/fskit-rs/0.2.0/fskit_rs/).
It delegates Apple extension packaging and the FSKit/XPC lifecycle to
FSKitBridge while keeping filesystem behavior in Rust. `objc2-fs-kit` supplies
lower-level bindings but would still require an extension and lifecycle host.
The higher-level `fsk` crate currently targets macOS 26+, so it does not cover
macOS 15 hosts. The established [fuser](https://github.com/cberner/fuser) crate
is another option through [macFUSE 5’s FSKit backend](https://github.com/macfuse/macfuse/wiki/FUSE-Backends);
it requires macFUSE’s libraries at build time and its extension at runtime. The bridge adds IPC and copies; actual throughput needs to be
measured on a camera before choosing further batching or a direct host.

## Checks

```sh
cargo test -p fujimod-camera-mount -p fujimod-usb -p fujimod-workflows --lib --bins
```

The namespace tests exercise read-only memory, address bounds, live reads,
random writes, zero-filled gaps, truncation and metadata invalidation. USB
protocol tests check owned-buffer uploads and uncertain-completion fencing.
Transaction tests require ff80 close before each reply and verify no session
acquisition on idle synchronization.
A live OS mount additionally requires the enabled signed extension and camera.
