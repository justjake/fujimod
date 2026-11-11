# Release downloads (design)

Agreed 2026-10-26. Release metadata supplies download availability at runtime.

## Releases without a website deployment

The website reads the current release at runtime instead of bundling
`artifacts/patches.json` at deploy time. GitHub Releases stays the only place
releases live.

Browser JavaScript can read GitHub's release list
(`api.github.com/repos/justjake/fujimod/releases` sends
`access-control-allow-origin: *`, 60 unauthenticated requests per hour per
visitor), but not release files: every download path ends at
`release-assets.githubusercontent.com`, which sends no CORS header. The
`fujimod-web` Worker therefore adds a stateless pass-through:

- `/releases/current` returns `{tag, name, published_at, prerelease}` for the
  newest published release that has a `patches.json` (pre-releases included),
  selected by publication time across all release-list pages.
- `/releases/<tag>/<file>` fetches that release file from GitHub server-side.
  Both routes accept GET and HEAD. No GitHub token or browser credentials are
  forwarded. Tags use up to 96 ASCII letters, digits, periods, underscores or
  hyphens, starting with a letter or digit; filenames use the same characters.
- The `RELEASES` KV namespace keeps the current release and each release file.
  `/releases/current` answers from it and refreshes it from GitHub in the
  background; release files fall back to it when GitHub fails.
- Cloudflare's edge cache keeps current metadata for five minutes and release
  files for one year. Development routes read `FUJIMOD_PATCHES` with no cache.
  The browser still verifies
  every file's size and SHA-256 as it does today.

## Release contents

`patches.json` schema 2 has, per firmware entry, two download switches, both false
unless the release sets them:

- `sd_boot_v1`: offer FujiMod SD boot v1 for this body.
- `sd_boot_v1_installable`: offer FujiMod installable (SD boot files plus the update DAT).
  Accepted only when the release contains that body's update DAT output and the
  entry's `update_filename`, the name the body's updater looks for
  (`FPUPDATE.DAT`, `FWUP0009.DAT`, …).

Each firmware entry lists the mods built into its release in `mods`; the
SD card download orders its cameras by that count.

An optional `card` list names the files the release offers for the SD card.
Each entry has the card `path` in upper-case 8.3 names
(`FUJIMOD/RECIPES/AGFAOPT.JSN`), the release `asset` that holds it, its
`sha256` and `bytes`, and optionally `title` and `author`. Entries with a
`processor` (a catalog processor name such as `EXR Processor II`) are that
processor's SD boot files at the card root (`FFBOOT.BIN`, `FFSYS.BIN`,
`FM_EXR2.BIN`) and are written only for its cameras; the others are optional
files the user may leave out. Local development adds a card file directory
(`card.json` and its assets) to the configured release with
`FUJIMOD_CARD_FILES`.

An optional `notes.md` (one per release, not per body) is referenced from
`patches.json` by filename. Without it no notes are shown. The
site renders it with a restricted Markdown renderer: headings, lists,
emphasis, code and links (opening in a new tab); no raw HTML or images.

## Browser data API

`loadRelease(baseUrl)` in `src/patch/release.ts` returns a `LoadedRelease` or
`undefined` when no current release exists. Upstream errors throw. The loader
fetches release metadata and Markdown text; firmware patches are downloaded
and verified when used.

`LoadedRelease` includes all validated `PatchIndex` fields plus `tag`, `name`,
`published_at`, `prerelease`, the absolute `assetBase` URL and optional
`notesText` (Markdown text). `notes` retains the index's filename.
The home controller exposes this value as `state.release`. To find download
availability, find the entry in `state.release.firmwares` by the chosen DAT's
SHA-256 and read `entry.downloads.sd_boot_v1` or `entry.downloads.sd_boot_v1_installable`.
The validator fills missing switches with `false`; malformed values fail.
Use `notesText` with the restricted renderer described above. Resolve patch
filenames against `assetBase`, including when passing it to the patch worker.

## Emulator section

Desktop: two columns inside the existing emulator panel; the inner launcher
card is removed. Phone: one column directly on the page.

**Left: the release panel.** One rounded panel headed with the release name
and date contains the rendered notes (tall on desktop, scrolling) and four
rows styled like the camera list rows (icon, title, subtitle, chevron):

| Row                            | Subtitle with a chosen DAT              | Unavailable                         |
| ------------------------------ | --------------------------------------- | ----------------------------------- |
| Download FujiMod installable   | `X70 • 1.10 • Includes SD boot version` | _Not available for this camera yet_ |
| Download FujiMod SD boot v1       | `X70 • 1.10 base firmware`              | _Not available for this camera yet_ |
| Run FujiMod in your browser    | `X70 • 1.10 with FujiMod 0.0.1`         |                                     |
| Run unmodified in your browser | `X70 • 1.10 official firmware`          |                                     |

Rows are always shown on desktop; an unavailable row is dimmed without a
chevron. Before any firmware is chosen, every row is disabled with a
descriptive subtitle (Includes SD boot version / Dual boot from an SD card /
FujiMod on an emulated camera / Official firmware, emulated) and a tinted
banner above them reads "Add base firmware to continue" with an arrow toward
Base firmware. On a phone, the banner and rows are hidden until firmware is
chosen.

Run rows resume the most recently used saved camera made from the selected
firmware with the same FujiMod setting (`CameraRecord.modEnabled`), or start a
new one. When such a camera exists, the row's subtitle ends with a
"Start fresh" link that starts a new camera instead. The separate Resume/New
camera lists and the pop-up choice dialog are removed.

**Right, top: Base firmware.** One component, `FirmwarePicker`, shared with
the Recovery page: saved firmware rows, the drop zone and file input, adding
to the shared firmware library and selecting. Heading "Base firmware", description
"FujiMod is assembled in your browser by patching official firmware you
provide". Saved firmware uses Recovery's radio rows with the subtitle
"Saved in your browser • Remove". Below the group, a separate rounded drop
zone with a dashed stroke: "Add official firmware", "Drop a Fujifilm .DAT file
here, or choose a file". With no saved firmware it is the only element and is
tinted blue. The most recently added firmware is selected automatically.

**Right, below: Get from fujifilm.com.** The existing search and camera list,
filling the remaining height with a minimum of 4.5 rows. The body badge reads
"Moddable" and sits beside the camera name instead of in its own grid column,
so processor names stay on one line (this also fixes Recovery's list).

## Download modals

Each download row opens a modal built from the same step components as the
Recovery page (numbered steps, file list, `PowerComboKeys` from the body's
boot data):

Before each SD boot or firmware-update button combination, switch the camera
OFF, then remove and reinsert the battery. Show the hand-drawn battery-cycle
SVG beside this instruction.

- **FujiMod SD boot v1:** 1. Download the SD card files, with the disclaimer
  below and a blue button. 2. Copy them to a FAT32 SD card (`ffsys.bin`,
  `ffboot.bin`). 3. Boot from the SD card with the `sd` combination only.
  Hold the buttons only while turning the power switch on; once the switch is
  in the ON position, release them.
- **FujiMod installable:** 1. Download the files, orange button. 2. Copy all files including the update DAT. 3. Try it from the SD card first
  (`sd` combination) and shoot normally until satisfied. 4. Install it: turn
  the camera OFF, remove and reinsert the battery, then power on holding the
  `firmware_update` combination. Release the buttons once the switch is in the
  ON position. 5. Optional: SET-UP → DEVTOOLS → BUILD OVERLAY → OFF.

Disclaimer, in bold above each download button:

> _No custom firmware is completely safe_. FujiMod is tested in our emulator
> and spot-checked on real camera hardware before each release, and we believe
> SD card dual boot is very low risk. We still do not guarantee this software
> for any purpose expressed or implied.

Followed by: "[Recovery tools](/recovery/) are available in the very unlikely
event this bricks your camera."

## SD card download

The homepage section after the feature catalog builds one card for any set of
release cameras. A floating checklist lists every release camera by mod count;
cameras whose root boot files differ from a chosen camera's are disabled. The
section is drawn as an SD card holding the card's file tree, with notes above
the files they describe. Optional card files and `FUJIMOD/.git` (Track
changes with Git) have include boxes; excluded files are dimmed.

Choosing a camera loads `/ota/<FIRMWARE>/latest.json`, the camera update
release whose store files fill `FUJIMOD/SYSTEM/STORE/`. The download writes
the processor's boot files, the chosen card files, each camera's first-install
`A.JSN` (generation 1) and `STATE.JSN`, and every store file once, checks each
fetched file's size and SHA-256, and zips the card's root as
`fujimod-sd-card-<release>.zip`. With Git tracking, `FUJIMOD/.git` holds one
commit of every file under `FUJIMOD/` and an index, so `git status` starts
clean.

## Download filenames

Archives use `fujimod-{sd-card-recovery,green-led,modified-sd-boot-v1,modified-installable}-{model}-{firmware}-build-{build}.zip`.
The model is lowercase with hyphens instead of spaces; firmware versions retain
periods, and the build is the website's eight-character Git commit SHA.
For example: `fujimod-sd-card-recovery-x-t1-5.52-build-1234abcd.zip`.
Camera-required filenames inside each archive stay intact. Modified installable
archives include the SD boot files and the body's `update_filename` DAT.
