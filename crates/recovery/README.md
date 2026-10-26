# SD recovery cards

`fujimod-recovery` constructs SD startup files from an official update supplied
by the caller. It checks the complete DAT identity, decoded source hashes,
initialized mappings, BSS, expected old words, header policy, and file names.
The official DAT is included unchanged at its required card-root name.

## Scope and card contract

SD boot has two purposes: recover a camera after a bad firmware flash by
booting the card and flashing new firmware, and dual-boot another operating
system that loads and saves settings normally.

A card holds the files the boot ROM requires plus one OS image. The OS image
is everything the firmware update installs, unpacked with overlays applied.
The first stage loads it into RAM and runs it. Wherever the stock firmware
would read bytes the update installed into internal storage, it reads them
from the OS image instead. Per-camera storage (settings, calibration) stays in
internal storage and works normally. Startup never reads the update DAT on the
card, so that DAT can be any firmware the updater flashes.

File names follow the Fujifilm service manuals' SD_BOOT packages:

| Processor | Files | Roles |
| --- | --- | --- |
| EXR, EXR Pro | `ffsys.bin`, `ffboot.bin`, `ffuser.bin` | ROM settings header; small loader (the ROM reads only 16 bits of its length); OS image |
| EXR II, X-Processor Pro | `ffsys.bin`, `ffboot.bin` | ROM settings header; OS image |
| X-Processor 4 | `ffinit.bin`, `ffboot.bin` | ROM-run first stage that sets up DRAM; OS image |
| X-Processor 5 | `marble_init.bin`, `ffboot.bin` | ROM-run first stage that sets up DRAM; OS image |

Booting with zeroed or scrambled internal storage is out of scope.

The processor selects the packaging mechanism. EXR II uses the stock NAND
header fragments with SD power hold and a preloaded RAM image. XP4 uses raw
FFINIT with the update's DDR setup and relocated tables, the Topaz filesystem
ABI, and a cold preloaded RAM image. Reset mode remains cold; three checked
comparisons select the preloaded table, images, and worker bypass.

EXR and EXR Pro use EXR's 512-byte `ffsys.bin` header and a small
`ffboot.bin`. That SD loader copies itself to SRAM and uses the ROM's u16 read
ABI to load `ffuser.bin` in chunks, checking file length, actual counts and
CRC32 before entering the stock RAM image. The extra file avoids the ROM's
16-bit truncation of the initial boot-file length. Updater code proofs are
checked against `ffuser.bin`. The runtime indexed reader opens this same file
and reads the firmware namespace appended after its initialized RAM image.

EXR II's remaining preloaded section-5 read uses the source bytes appended to
`ffboot.bin`. Construction checks the section selector, callback-free call,
16 KiB or 128 KiB block geometry, section descriptor, and code reservation before
redirecting that call. The stock indexed reader and per-camera storage calls
retain their original code.

X-Processor Pro uses Agate's 2 KiB DDR configuration as `ffsys.bin` and the
complete RAM image as `ffboot.bin`. The locator follows the update's single
or initial/trained DDR-copy helper to choose the header. Three checked
comparisons select the stock preloaded table, header and image paths while
retaining cold reset mode. Its plain and progress read paths share a checked
read-only descriptor reader. Calls to that reader fetch update-installed
addresses from the firmware namespace appended to `ffboot.bin`, using Agate
ROM file I/O. Other addresses delegate to the unchanged stock reader. Program
and erase paths retain their stock code. Both families use rooted section tables and checked
source mappings; unrecognized table or update-device layouts refuse.

XP5 emits raw `marble_init.bin` with the update's setup at byte zero. Its
entry loader preserves registers and stack, applies the checked header carried
on the card, and restores the setup entry. Device bases, flash pins, SD
routing/power/clock operations, and status-lamp fields are catalog inputs with
checked source spans. The loader makes no ROM calls.

Startup and recovery use this same entry loader, then load FAT32 `ffboot.bin`
after stock DDR setup and check its exact length and CRC32 before continuing
stock startup. Stock setup retains internal-storage reads that are deferred
for a later decision: X100VI prefix/register tables at `0xf0011e00`,
`0xf0012090`, `0xf00124e0/24f0` and DDR tables near
`0xf001c8ec..0xf001cf7c`; serial-storage parameters (X100VI `0xf0027100`, X-M5 `0xf0017040`); PHY IMEM/DMEM at
`0xf0080000+0x10000` and `0xf0090000+0x10000` or
`0xf00a0000+0x6120` and `0xf00a6120+0x10000`; the DDR layout
marker at `0xf7fffffc` and retained-state ranges at `0xf7ce0000` and
`0xf7fe0000`; and the reset-image DMA copy from `0xf0040000` (X100VI
`0x14000` bytes, X-M5 `0x19000` bytes). Missing device or
setup facts produce specific construction refusals. `Recipe::build_stage_one`
uses the same loader with internal-flash initialization, read-timing sampling,
and the original internal-flash continuation; it produces only
`marble_init.bin`.

`availability(firmware_id)` returns `can_build`, the exact missing facts, output
file names, and the catalog's SD-boot and firmware-update button combinations.
A recovery build requires complete facts, including version-independent
acceptance by the stock updater. Located stock getters and producers supply the
SD-running firmware's own public version; the stock comparison accepts equality,
so the paired official DAT needs no version change. The recipe pins those
code spans and verifies them against the constructed RAM image. `Recipe::build_startup` constructs boot files
separately for execution and transfer tests.

JavaScript uses `recovery.availability(firmwareId)` and
`recovery.build(firmwareId, datBytes)`. Call `card.takeFiles()` to obtain file
objects with `name`, `size`, and `bytes: Uint8Array`. Free the file objects and
card after copying their contents to downloads or an archive. Construction has
no filesystem, USB, camera, or network access.

The native CLI exposes `list`, `build FIRMWARE DAT NEW_DIRECTORY`, and
`startup FIRMWARE DAT NEW_DIRECTORY`, and `stage-one FIRMWARE DAT NEW_DIRECTORY`.

`rescue-baseline FIRMWARE DAT NEW_DIRECTORY` reconstructs the original XP4 cold
pair before firmware-reader redirection. `Recipe::build_rescue_baseline` exposes
the same construction for regression comparisons.

`card-startup FIRMWARE DAT STORAGE_IMAGE NEW_DIRECTORY` accepts an explicit
unit storage image for recipes with retained setup data. It checks the storage
size and complete update pages, carries the retained cache in SRAM, and appends
the storage namespace to the OS file. It defaults to zero success hold. The
optional runtime binding initializes the card and serves checked descriptor
reads from that namespace; it uses the card’s retained training data and
does not initialize the serial NOR controller.

`startup` and `stage-one` accept `--success-hold-seconds N` (default `0`).
After sampling and header operations succeed, a nonzero value holds steady
green using a counted loop before continuing. Each second requests 2,048 units
of 1,000,000 four-NOP iterations, allowing up to two billion iterations per
second; the startup clock can make the hold longer. This early hold calls no
stock code and requires no initialized timer. Zero skips the hold. The library
exposes the same setting through `StartupOptions` and the corresponding `_with_options` methods.

The XP4 compiled SD loader is `assets/xp4-sd-loader.generated.bin`; its companion
`assets/xp4-sd-loader.generated.json` describes segment offsets, symbols and ARM
relocations. The crate loads these inputs with `include_bytes!` and
`include_str!`. They contain empty camera-data placeholders. Runtime relocation
inserts the supplied update's setup tables and updates absolute references,
transfer size, CRC32, workspace and filesystem calls.

XP4 startup recipes can bind firmware reads to an immutable namespace embedded
in `ffboot.bin`. The supplied update provides those bytes at card construction
time. Startup reads only `ffinit.bin` and `ffboot.bin`; the update DAT on the
card can be replaced or removed independently. The builder checks the original
reader and primary-loader call identities, reserves its code and backing ranges,
and preserves the conditional primary-core initialization call. Out-of-range
reads return errors. Per-unit storage APIs retain their stock behavior.

The XP5 compiled SD loader is `assets/xp5-sd-loader.generated.bin`; its companion
`assets/xp5-sd-loader.generated.json` gives the image size, symbol offsets, the
pre-setup stack bound and the two relocations against stock setup routines. The
build script turns that layout into constants and the crate loads the image with
`include_bytes!`. The image contains empty camera-data placeholders; card
construction needs only the official DAT and the compiled catalog recipes.

The EXR compiled SD loader is `assets/exr-sd-loader.generated.bin`; its
companion `assets/exr-sd-loader.generated.json` gives the startup loader and
firmware-reader segments, configuration offsets, symbols, and SRAM/stack
bounds. The crate loads these inputs with `include_bytes!` and `include_str!`.
Card construction fills the empty configuration fields with the ROM file API,
transfer sizes, and CRC32.

`cargo run -p fujimod-recovery --example locate_rom_sd -- exr UPDATE.DAT`
emits freshly located startup plans and header facts from the supplied update.
Use `exrpro` or `xpp` for the other processor families.
