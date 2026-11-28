# Opening a camera: design (2026-11-25)

Status: agreed with the owner. The layered design below replaces the single
builder; the steps at the end move the code to it.

## Goal

A person or an agent opens an emulated camera by naming a firmware and how it
starts. Everything else comes from the catalog. The website, the desktop app,
the MCP camera module, the `fujimod-emulator` CLI and the Python binding all
open cameras the same way, so a camera boots and behaves the same in each of
them.

Two layers do this. The emulator layer knows cameras: firmware, internal
flash, SD card and the controls held at power-on. The FujiMod layer above it
knows FujiMod: its SD card, its SD card boot and its autostart.

## Emulator layer

Opening a camera takes:

- **Firmware**: the stock update and the boot ROM, as files or bytes. Each is
  hashed once and must match the catalog's entry for the firmware.
- **Internal flash**: stock, or a saved flash image.
- **SD card**: name and bytes pairs, or none. Options: present or absent,
  write protection, the host's data-transfer time and a declared FAT
  geometry (sector count, label, partition start, sectors per cluster).
- **Power-on combination**: none, or the name of a combination in the body's
  controls document (`sd`, `firmware_update`, `service`). The catalog says
  which controls each holds and when the boot ROM has sampled them; the
  emulator holds and releases them through the session's contact driver.
  Callers never name pins.

Options: execution policy (`interactive` or `exact`), engine, language,
clock, a checkpoint to restore, whether to use the boot cache, and where to
stop: at REC or first-start setup, or before the first instruction with the
combination held (a later call releases it and connects the controls the
loaded firmware expects).

An opened camera can power off and on again keeping its internal flash and
SD card, optionally with a combination held or another SD card inserted. A
stock start can instead place a caller-verified image at address zero and
start the first core at the native loader.

Opening writes nothing to disk. The emulator reads only the inputs it is
given; the boot cache is the one exception and is optional.

A failed open names how far the boot got: boot ROM, firmware loaded, REC.

## FujiMod layer

A small crate above the emulator turns FujiMod presets into emulator inputs:

| Preset       | Internal flash                                  | SD card                        | Combination |
| ------------ | ----------------------------------------------- | ------------------------------ | ----------- |
| Stock        | Stock                                           | None, or files the caller adds | None        |
| SD card boot | Stock                                           | The FujiMod SD card            | `sd`        |
| Autostart    | Stock with the internal-flash stage 1 installed | The FujiMod SD card            | None        |

- The FujiMod SD card is a folder or files the caller gives, or the
  published release SD card for the firmware. A release SD card is
  downloaded once into the state directory, and every file's SHA-256 is
  checked on download and before reuse. The startup files derived from the
  stock update are built from it, as `fujimod-sd-card sd-boot-v2-sd-card`
  builds them.
- Autostart flash is made once per firmware and SD card the way an owner
  makes it: an SD card boot, then FUJIMOD SET-UP > BOOT SETUP > AUTOMATIC,
  the installer backs the internal data up to the SD card and rewrites the
  stage-1 blocks, and the camera powers off. The resulting flash is kept in
  the state directory; later opens start from it with nothing held. The
  installer run uses stock-style menus, which expose the mod's pages to
  presses.
- After an SD card boot or autostart reaches REC, the layer confirms FujiMod
  is in RAM and that the release was confirmed. A failure names the stage:
  boot ROM, SD loader, FujiMod applied, REC, release confirmed.

The MCP camera module, the CLI, the desktop app and the Python binding call
this layer. Anything that wants a camera without FujiMod calls the emulator
layer directly.

## Files

### Firmware library (read-only)

Callers may name a firmware by id instead of passing files. The library is the
directory named by `FUJIMOD_FIRMWARE_LIBRARY`, or the current directory when
it is unset. It holds source records at `firmware/<model>/<version>/firmware.json`:

```json
{
  "id": "xe3-132",
  "source": {
    "path": "firmware/x-e3/1.32/original/FWUP0014.DAT",
    "size": 107104384,
    "sha256": "..."
  },
  "boot_rom": {
    "content": { "size": 65536, "sha256": "..." },
    "source": {
      "path": "firmware/x-processor-pro/boot-rom/rom-fff00000.bin.zst",
      "size": 13152,
      "sha256": "...",
      "compression": "zstd",
      "original": { "size": 65536, "sha256": "..." }
    }
  }
}
```

- Paths are relative to the library root.
- A `source` with `"compression": "zstd"` names the stored file; `original`
  names the decoded bytes. Both identities are checked.
- A boot ROM capture may be longer than the ROM. The ROM is the capture's
  first `content.size` bytes, and `content.sha256` identifies it.
- Opening a firmware reads the record whose `id` matches. Its update must
  match the catalog's update hash. The boot ROM is any record's `boot_rom`
  whose `content.sha256` is the hash the catalog names for that firmware, so
  bodies that share a ROM keep one copy.
- Files are read and checked once, then decoded in memory. The library is
  never written.

The emulator reads zstd-compressed update and boot ROM files wherever it takes
them, including explicit `--update` and `--boot-rom` paths, by their zstd
magic number.

### State directory

Everything FujiMod writes and does not commit lives in one state directory,
named by `FUJIMOD_STATE_DIR`, or `/tmp/fujimod` when unset:

- `sd-cards/<release>/<firmware>/`: downloaded release SD cards.
- `autostart/<firmware>/<sd-card-key>.flash`: kept autostart flash.
- `boot-cache/`: saved boots.

Everything in it can be made again: a release SD card downloads again and an
autostart installs again.

## Boot cache

Each boot that reaches REC is saved as a portable checkpoint keyed by the
firmware files, the resolved configuration, the internal flash, the SD card's
files and options, the combination, the execution policy, the engine and the
builder's version. A later open with the same key restores it in about 0.4 s
instead of booting (about 1.2 s on the X70). A saved boot is used only when
it restores into a freshly built session that still reports REC.

- The cache holds at most 1 GiB, or `FUJIMOD_BOOT_CACHE_MAX` bytes. Saving
  past the limit deletes the least recently used saves first.
- Saves are compressed.
- A save is written to a temporary file and renamed into place. Opening the
  cache deletes leftover temporary files and saves from other builder
  versions.
- Any open can skip the cache: MCP `boot_cache: false`, CLI
  `"boot_cache": false`, the desktop launcher's Boot from scratch.
- Tests of the cache itself use a scratch state directory under `/tmp`.

## Callers

- MCP `camera_open` takes the camera description (`body`, optionally `firmware`,
  `boot`: `stock`, `sd-card` or `autostart`, cards and options; see
  [emulator-camera](emulator-camera/README.md)). `camera_set` moves a dial,
  switch, ring or lever to a catalog position and returns once the firmware
  observes the new pose.
- The desktop launcher lists the library's cameras and opens Stock, SD card
  or Autostart; `--endpoint` still connects to a session another host serves.
- The CLI `open --camera CAMERA [--gui]` takes the same description as a
  file, standard input or inline JSON.
- The Python binding's `Board.open_camera` takes the same description as
  keyword arguments; a power-on hold, a RAM image or a flash image opens the emulator
  layer.
- The website keeps its interface and calls the same layers inside its Wasm
  module.

## Steps

1. Split the builder: the emulator layer takes files, flash, SD card files
   and a combination name; the FujiMod layer holds the presets, the release
   SD card download, the autostart install and the FujiMod checks. Rename
   automatic boot to autostart everywhere.
2. The library reads source records from the current directory or
   `FUJIMOD_FIRMWARE_LIBRARY`, checks each file once and decodes zstd in
   memory; it is never written. The state directory holds downloads,
   autostart flash and the boot cache.
3. The boot cache's size limit, compression and cleanup.
4. Every caller passes inputs directly and writes nothing to disk to start a
   camera.
5. Hosts that build boards another way move to these layers, one body at a
   time.
