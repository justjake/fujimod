# Dev Tools: getting a camera ready for development

Agreed with the owner on 2026-10-25. This records the design; implementation
follows it. It replaces the Enter Debug Mode wizard: Dev Tools itself is the
path, a short list of tools done in order. There is no separate wizard. The
camera operations (service-mode check, reset guard, flash backup, state dump,
debug-mode save and rollback) live in a Rust workflow crate beside
`fujimod-usb`, shared by every caller. Python tools that Dev Tools needs move
to Rust.

## Start screen

Dev Tools opens on a service-mode entry guide, Connect camera, and a simulated
camera choice among the emulator's saved cameras (see DESIGN.md). The guide
covers the power-on chord, the OFF-switch warning and the battery-removal exit.
In service mode, do not use the OFF switch: shutdown factory-resets the camera.
Restart by removing the battery. After connecting, Dev Tools confirms service
mode, arms the RAM reset guard and enables debug access in RAM, which the
flash backup and state dumps need. Both stay set until the camera restarts;
neither is stored. It reads both original values first and records them with
the state dump.

## The path

Once a camera is connected in service mode, the tools column starts with a
numbered list. Each item shows its state (to do, running, done, skipped):

1. **Back up flash.** Copies the camera's whole internal flash through the
   firmware's own reader. It reads and never erases or programs. A small
   **Skip** link sits beside it.
2. **Dump camera state.** RAM, the boot ROM and any other device state that is
   safe to read, as one bundle.
3. **Enable debug mode.** Locked until step 1 is done or skipped. Persists the
   debug setting and reads it back.
4. **Restart and reconnect.** Remove the battery, reinsert it, switch on
   normally and reconnect. Plug USB in twice for file transfer.
5. **States to capture.** The camera states we want, each with a capture
   button. Each capture is a full camera state dump.

Completing or skipping step 1 is saved in local storage under the camera's
serial number. That serial then has every Dev Tools tool unlocked, including on
later visits.

A user who does not want debug mode stops after step 2.

Every step offers its download as soon as it finishes, so a later failure does
not lose earlier results. Bytes stay binary; each download carries a manifest
of identity, addresses, sizes and hashes. Results never leave the browser
unless the user downloads or shares them.

Each step forms its command from the camera's firmware facts in the catalog and
verifies the reply. A step whose facts the firmware lacks (no flash reader, no
boot ROM location, no transport for its processor generation) is disabled and
says which fact is missing.

## Flash and camera state

"Flash" names the internal storage on every body, whatever the part: serial
NAND, serial NOR or raw NAND. The flash backup replaces the configuration dump
on the path; the stored configuration is part of flash. Dump configuration
stays available as an ordinary tool.

The camera state dump replaces the RAM dump. It covers DRAM, the boot ROM and
the other readable device state the catalog lists for the body. The boot ROM is
the same across a processor family, so a dump whose hash matches a known boot
ROM is labeled as already known. A boot ROM that reads as all zero or all
`0xff` is masked, not dumped. Each region is read twice where the catalog marks
it as constant, and the reads must match.

## Other tools

Outside the numbered list, as ordinary tools:

- **Recovery SD image.** Builds an SD recovery image from a firmware update
  the user provides. It reuses firmware files already dropped into the
  emulator, which share browser storage.
- **Capture buttons**, Console, Dump configuration, Disable debug mode, Share
  camera and Bug report.

These are needed rarely, usually once per body, and the page does not promote
them.

## Disable debug mode

Restores the stored debug setting to 0. The rollback has not been validated on
hardware.
