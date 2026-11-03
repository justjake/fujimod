# Camera mount

`fujimod-mount` exposes one connected camera through macOS FSKit:

- `sd/`: the camera’s SD0 namespace, with regular-file reads, writes, creation,
  truncation and deletion.
- `memory`: a read-only file with a 4 GiB logical size. File offsets are camera
  addresses. Only ranges admitted by the selected firmware’s readable memory
  map can be read; holes return `EIO`.
- `flash`: a read-only file spanning the selected internal-storage window when
  a prepared reader artifact is supplied. Offsets are logical storage bytes;
  NAND readers return corrected main-area bytes.
- `regions.json`: the readable ranges, with names, addresses and sizes.

There is no `debug/` wrapper. Memory reads are live, not a snapshot. The adapter
requests that FSKit inhibit kernel offloaded I/O; applications should use
`pread` or seek/read and handle I/O errors. A regular address-indexed file is
the POSIX interface here: no privileged character-device driver is needed.
The current adapter does not provide a custom `mmap` interface.

## Run

macOS 15.4 or newer is required. On first use the command installs and registers
the signed [FSKitBridge 0.1.0 application](https://github.com/debox-network/FSKitBridge/releases/tag/v0.1.0),
checking the pinned archive hash and the application signatures. macOS requires
enabling `fskitbridge` once under System Settings → General → Login Items &
Extensions → File System Extensions.
The command checks embedded provisioning-profile expiration before opening USB.
The pinned 0.1.0 host has a profile valid through October 25, 2043 and uses the
same wire protocol as the Rust crate. The 0.2.0 host archive has an expired
profile and cannot launch; rebooting does not repair that profile.
Building requires Rust, `protoc`, Clang with ARM support, and LLD
(`brew install protobuf llvm`). `CLANG` and `LD_LLD` select build tools.
The camera helper is compiled into the mount executable; mounting runs no compiler.

The camera must already provide an available ff80 debug service. Select a
firmware with readable memory facts in the bundled catalog. The `sd/` subtree
is present when that firmware also has a card capability. The command verifies
camera identity and the selected code checks before starting the mount.

```sh
mkdir -p /tmp/fuji
cargo run -p fujimod-camera-mount --bin fujimod-mount -- \
  --firmware x100v-301 /tmp/fuji
```

Add `--flash-reader /path/to/reader.json` (or set `FUJIMOD_FLASH_READER`) to
expose internal storage. The metadata and separate image are the prepared
[`flash::Artifact`](../workflows/README.md) output. Loading verifies firmware,
image hashes, stock-reader contracts and bounds before opening USB. Each file
read checks a control page, reads only the pages containing the requested
range, restores temporary RAM and callbacks, and releases its session.
It borrows E1 publication state without generating or restoring diagnostic scratch.

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

For a firmware with an owned query-service contract, the command allocates and
loads its bundled file helper automatically. No preinstalled camera mod or
separate helper artifact is needed. The helper negotiates a buffer up to 8 MiB with the stock allocator, reducing
its request after confirmed allocation refusals. It combines open/seek/read/close
for ranges fitting that buffer. The helper runs stock asynchronous storage
operations, uses its own buffer and directory cursor, and installs no hooks or
background timer. The stock SDK backend uses existing camera code.

`--benchmark` mounts the volume, measures uncached POSIX memory and existing SD
file reads at several request sizes, and unmounts. It reports decimal MB/s and
does not write SD files. `FUJIMOD_MOUNT_TRACE=1` logs backend request sizes,
session acquisition time, and operation time to stderr.
`--benchmark-usb` measures the same bounded camera backend directly, without
starting FSKit, and labels its output `usb-backend`. This isolates transport
and storage costs while diagnosing the OS adapter.

## Architecture and performance

The namespace and policy live in Rust independently of the platform adapter.
The worker holds device descriptors and any owned file-helper allocation while idle.
Writes, small control operations and flash reads use one ff80 session per
operation. Consecutive SD and memory reads of at least 64 KiB may reuse one
qualified session. A burst stops at 64 MiB or two seconds between operations;
an in-flight bounded operation completes before release. The worker restores
publication state and closes ff80 after 20 ms without another read. It blocks
without polling after releasing the lease. File handles close before each reply,
and writes are committed before replying. Synchronization flushes a pending
read lease. Unmount releases the helper allocation and reports cleanup errors.
Flash reads yield the file helper allocation first, then recreate it on the next
SD operation. Other tools can acquire ff80 between bursts and while idle.

The installed-service descriptor address is cached as a discovery hint. Each
transaction revalidates its descriptor, pointers and ownership; a missing hint
triggers rediscovery. Firmware identity and stock code checks are repeated in
each session that uses them; a bounded burst shares that qualification. Nearby code checks share qualified bulk reads when
that costs fewer commands; CPU fallback reads only the checked spans. Each
individual digest is still verified. Regular-file I/O transfers bounded ranges rather
than downloading and replacing whole files. Extending a file zero-fills its gap.

Read transport selection compares CPU command count with qualified E1 setup,
probes and transfers. It supports every published bulk source family: query,
32-bit pattern publication and 64-bit zero-base publication. The latter two
borrow and restore the located source words without running pattern generation
or filling diagnostic scratch. DMA rounding must remain inside the admitted
aperture. Preserving pattern sources use the live registered capacity up to
the host transfer budget; the 64 KiB generated pattern size does not cap E1.
The adapter keeps requests together up to 64 MiB for memory and 8 MiB for SD per transaction rather than
reopening and requalifying every 128 KiB. Small control reads use CPU commands when cheaper. Stock SDK file
buffers use their checked E1 source pair for larger reads.

Owned SD buffers use E0 uploads when a checked upload destination contract is
published in the shared bulk capability and complete padded pieces fit the
buffer and registered capacity.
Cached SDK buffers are cleaned before DMA and synchronized before file I/O.
The uploader guards and restores the destination pointer before the file
operation continues, including completed transport refusals. The catalog
publishes destination contracts for all 25 firmwares with bulk reads, across
32-bit EXR II, 32-bit XP4 and 64-bit XP5. Uploads preserve the full pointer width.
A setup branch with configuration effects requires its published RAM-only
precondition; otherwise that operation uses CPU writes. Cameras without a
located upload contract use CPU writes.
`--read-limit` and `--write-limit` apply to CPU commands, not E0/E1 pieces.
Bulk DRAM throughput does not establish SD, internal-flash or CPU-only memory
throughput. Those include file callbacks, storage-driver work or CPU copies.
No mounted-camera benchmark establishes a universal 30+ MB/s rate.

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
