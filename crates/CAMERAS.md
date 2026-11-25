# Opening a camera: design (2026-11-25)

Status: agreed with the owner; steps 1-4 done, step 5 next.

## Goal

A person or an agent opens an emulated camera by naming a firmware and a
boot. Everything else comes from the catalog. The website, the desktop app,
the MCP camera module, the `fujimod-emulator` CLI and the Python binding all
open cameras through one builder, so a camera boots and behaves the same in
each of them.

```text
open(firmware = "x70-110", boot = Stock)
open(firmware = "x70-110", boot = SdCard { card: Release })
open(firmware = "x70-110", boot = AutomaticBoot { card: Release })
```

## Boots

| Boot | What runs | Internal flash | SD card | Held at power-on |
|---|---|---|---|---|
| `Stock` | The installed firmware | The catalog's reference camera settings | Empty, or files the caller adds | Nothing |
| `SdCard` | FujiMod from the SD card | As `Stock` | The FujiMod SD card | The catalog's `boot.sd` controls |
| `AutomaticBoot` | FujiMod from the SD card, started by the card's AUTOMATIC boot | As `Stock`, with the card's AUTOMATIC boot installed | The FujiMod SD card | Nothing |

The `boot.sd` controls are the body's controls document entry, the same one
the website reads (X70: command control press and movie-record button). The
builder holds them through the session's contact driver from power-on and
releases them when the catalog says the boot ROM has sampled them. Callers
never name pins.

`AutomaticBoot` internal flash is made once per firmware and SD card: the
builder boots the card with its `boot.sd` holds, opens FUJIMOD SET-UP's BOOT
SETUP page and chooses AUTOMATIC, the same presses a person makes. The card's
installer backs the internal data up to the SD card and rewrites the stage-1
blocks the boot ROM loads. The builder powers the camera off and keeps those
blocks in the firmware library under the firmware and the card's contents.
Later opens program them back into a stock boot and start from internal flash
with no buttons held.

The installer run boots the card with stock-style menus, which is how the
stock menu descriptors expose the mod's pages; every other boot uses the card
as the caller gave it.

### SD card sources

- `Release`: the published release SD card for the firmware, as the website
  downloads it. It is fetched once into the firmware library under
  `sd-cards/<release>/<firmware>`, with every file's SHA-256 checked on the
  fetch and again before reuse.
- `Directory(path)`: a folder holding an SD card tree, such as a local mod
  build's SD card.
- `Files(list)`: name and bytes pairs, for the browser.

The SD card's startup files that derive from the stock update are built by
the builder from the firmware library, as `fujimod-sd-card
sd-boot-v2-sd-card` builds them, so a caller supplies only the FujiMod tree.

## Firmware library

Callers name firmware by id. The builder finds the stock update, the boot
ROM and the reference camera's settings in a firmware library: a directory
of files found by content hash, named by `FUJIMOD_FIRMWARE_LIBRARY`. A file
whose hash the catalog does not list is refused. The browser supplies the
same files in memory.

## The builder

One crate owns turning `(firmware, boot, options)` into a running session.
It holds what the browser binding builds today and native hosts lack:

- the virtual SD card, formatted and filled;
- boot holds and their release, and power cycles;
- the sensor scene input (RGB and RAF) and LCD preview cadence;
- the generated controls and contact driver;
- the durable storage identity.

These are derived from the resolved run configuration and catalog, never
from a host-written document. The browser binding, the desktop app, the
MCP camera module and the CLI call the builder; the Python binding reaches
it through its existing Rust bindings.

Options: execution policy (`interactive` or `exact`), engine, language,
clock, extra SD card files, the SD card's presence and write protection, the
host's SD data-transfer time, a checkpoint to restore, whether to use the
boot cache, and whether to stop before the first instruction with the boot
holds applied.

## Results

Opening returns when the camera reaches REC or first-start setup. Anything
else is a failure that names how far the boot got:

- `Stock`: boot ROM, firmware loaded, REC.
- `SdCard` and `AutomaticBoot`: boot ROM, SD loader, FujiMod applied in RAM,
  REC, release confirmed.

An open can also stop before the first instruction: the builder builds the
session and applies the boot holds, the caller runs it, and a builder call
releases the holds and connects the controls the loaded firmware expects.

An open keeps its media available to the caller: `Opened::power_on` powers the
camera off and on again with its internal flash and SD card, optionally holding
the catalog's SD-boot controls or replacing the card with a fresh one from the
boot source.

## Boot cache

Each opened boot is saved as a portable checkpoint keyed by the firmware
update, the resolved configuration, the boot, the SD card's files, the
execution policy, the engine and the builder's version. A later open with
the same key restores it in about a second instead of booting.

Saved boots live in `boot-cache/<firmware>/` in the firmware library, one per
firmware, boot, policy and engine: saving a boot with another card or another
builder replaces the slot's earlier entry. A saved boot is reused only when
its checkpoint restores into a freshly built session and still reports REC;
anything else boots the camera and replaces the entry. A snapshot is the whole
machine, about 100 MB for an X70 at REC, so a library holds one per slot.

Every caller can boot without it: MCP `camera_open` takes `boot_cache: false`,
the CLI open takes `--no-boot-cache`, and the desktop launcher offers Boot
from scratch. A bypass reads no saved boot and saves none.

## Callers

- MCP `camera_open` takes `firmware`, `boot` and options. The raw `gpio`,
  `keys` and overlay options go. `camera_set` moves a dial, switch, ring or
  lever to a catalog position through the contact driver and returns once the
  firmware's own observation of the control shows the new pose.
- The desktop launcher lists the firmware library's cameras and opens Stock,
  SD card or automatic boots; `--endpoint` still connects to a session
  another host serves.
- The CLI `open` takes `--firmware ID --boot stock|sd-card|automatic
  [--sd-card DIR] [--execution-policy interactive|exact] [--gui]`.
- The website keeps its interface and calls the builder inside its Wasm
  module.

## Steps

1. Move the browser binding's construction into the builder with the three
   boots and the firmware library. A test boots the current X70 release SD
   card to REC and presses a button.
2. Switch the MCP camera module, the desktop app and the CLI to it.
3. `AutomaticBoot`, built from the installer once and kept.
4. The boot cache, and an MCP tool that sets dial, switch, ring and lever
   positions.
5. Hosts that build boards another way move to the builder, one body at a
   time.
