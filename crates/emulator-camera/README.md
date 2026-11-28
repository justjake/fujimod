# Opening an emulated camera

Every front door opens a camera from the same camera description: the MCP
server's `camera_open`, Python's `Board.open_camera`, the command line's
`fujimod-emulator boot --camera` and `open --camera`, and the desktop app.
This crate turns the description into a camera; `fujimod-camera` adds the
FujiMod boots on top.

## Four layers

A camera is assembled from four layers, each with its own home:

| Layer | What it says | Where it lives |
|---|---|---|
| Processor | Hardware every body with this processor shares: cores, buses, blocks, the boot ROM's behavior | `catalog/soc/<processor>` |
| Body | The hardware one camera body contains, the same every time it is opened: its device list, wiring, controls, storage slots and resting inputs | `catalog/bodies/<body>/body.json` |
| Firmware | Facts about the program: its identity, how it starts, the addresses located in it and the records generated from it. Never hardware | `catalog/firmware/<body>/<version>/` |
| Session | The choices of one open: settings, cards, scene, date, engine, how it starts | the camera description |

Nothing maps a firmware to the hardware it runs on. The firmware names its
body, and the body's description is the hardware.

## The camera description

```json
{
  "body": "xh2s",
  "firmware": "7.30",
  "settings": {"language": "en", "setup": "done"},
  "sd_card": {"files": {"DCIM/100_FUJI/DSCF0001.JPG": {"source": "photo.jpg"}}},
  "date": "2026-11-28T12:30:00",
  "engine": "unicorn"
}
```

`catalog/schemas/emulator-camera.generated.schema.json` is its schema. Only `body` is required:

- `firmware` is a version (`7.30`), a firmware identity (`xh2s-730`) or
  `none` (an empty flash, the boot ROM alone). Omitted, it is the newest
  firmware the catalog has for the body. A firmware of another body is
  refused: `xh2-530 is X-H2 firmware, not X-H2S`.
- `settings` names camera settings, which the firmware's settings record
  writes before the first instruction: the EXR II power-on settings, the XP5
  user profile or the XP4 journal.
- `sd_card`, `sd_card_2` and `cfexpress_card` are the cards in the body's
  storage slots (`storage_slots` in `body.json`). Each is `present`,
  `write_protected` and one of `files` (by full path on the card), `directory`
  or `image`. An omitted `sd_card` is one blank card. A slot the body lacks is
  refused, naming the body's slots.
- `date`, `scene`, `engine`, `execution_policy`, `boot` and the rest default
  to the firmware's run.

The command line's `boot` also takes `--actions`, a list of timed inputs named
as the MCP server's input tools name them
(`catalog/schemas/emulator-actions.generated.schema.json`), and plain flags
for the run's limits and outputs.

## How a camera resolves

`plan` resolves a description, as the builder's `Request`, without reading a
firmware byte: the body and
firmware choice select a firmware, the firmware's run configuration is
resolved, and the configuration's processor decides the startup path (EXR II,
XP5, or the XP4 and X-Processor Pro application start) and whether the start
reads the boot ROM. Each refusal names its gap, for example
`body x100t has no device list yet`. The catalog test `catalog_plans` plans
every bundled firmware and lists the gap of each one that does not open yet,
so a body falling off the shared path fails there.

`open` then reads the firmware's files, builds the board, puts the cards in
their slots and runs the camera to REC. A firmware that declares no REC
readiness facts is built and returned before its first instruction, saying
so; the caller runs it.

## The canonical run configuration, for now

Until the run configurations are sorted into body, firmware and session
facts, each firmware has exactly one canonical run configuration,
`configurations/emulator/<firmware-id>/run.json`, and the builder uses only
that one. Other run configurations and overlays of a firmware remain for the
tests and tools that still use them.

Later steps move what a run configuration still holds to its layer:

- hardware (device options, hardware-input profiles, body profiles) into the
  body's one description;
- startup facts (start kind, startup registers, application startup plans)
  into the firmware record;
- session choices (card, settings, scene, date, engine) into camera
  defaults.

When a run configuration holds nothing that a layer does not, it goes away.
