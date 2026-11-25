# Opening a camera: design (2026-11-25)

Status: agreed with the owner; step 1 in progress.

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
| `AutomaticBoot` | FujiMod from the SD card, started by the automatic-boot installer | As `Stock`, with the installer applied | The FujiMod SD card | Nothing |

The `boot.sd` controls are the body's controls document entry, the same one
the website reads (X70: command control press and movie-record button). The
builder holds them through the session's contact driver from power-on and
releases them when the catalog says the boot ROM has sampled them. Callers
never name pins.

`AutomaticBoot` internal flash is made once per firmware and SD card: the
builder writes the stage-1 loader the card carries into the body's NAND code
space, points the boot block's reset literal at it, and keeps the resulting
flash in the firmware library. Later opens restore that flash and start from
it with no buttons held.

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
clock, extra SD card files, a checkpoint to restore.

## Results

Opening returns when the camera reaches REC or first-start setup. Anything
else is a failure that names how far the boot got:

- `Stock`: boot ROM, firmware loaded, REC.
- `SdCard` and `AutomaticBoot`: boot ROM, SD loader, FujiMod applied in RAM,
  REC, release confirmed.

## Boot cache

Each opened boot is saved as a portable checkpoint keyed by the firmware,
the boot, the SD card contents and the builder's inputs. A later open with
the same key restores it in about a second instead of booting.

## Callers

- MCP `camera_open` takes `firmware`, `boot` and options. The raw `gpio`,
  `keys` and overlay options go.
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
