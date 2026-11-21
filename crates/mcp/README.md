# fujimod-mcp

An MCP server for experimenting on emulated cameras. Cameras stay running
between tool calls, so an agent boots once, checkpoints, and runs many
experiments from that point. Design and status: [../MCP.md](../MCP.md).

```sh
cargo build --release -p fujimod-mcp
target/release/fujimod-mcp [--modules camera] [--output DIR]
```

All compiled-in modules are enabled by default, including `camera`. Omit
`--modules` to include new modules automatically as they are added. Pass
`--modules camera` only to restrict the server to that module.

The server speaks MCP over standard input and output. `--output` receives
large results such as profiles; it defaults to a directory under the
system temporary directory. Register it with an MCP client as a stdio
server, for example in a project `.mcp.json`:

```json
{
  "mcpServers": {
    "fujimod": { "command": "target/release/fujimod-mcp" }
  }
}
```

The UI paint uses a host GPU through wgpu. On a machine without one,
install a software Vulkan driver (Mesa's lavapipe).

## Camera module

`camera_open` takes a run configuration id, its overlays and the firmware
files the configuration names: the update file, the boot ROM and, when its
settings name a camera, the directory of that camera's NAND sections. It
boots to REC, or restores a checkpoint file written by a camera built from
the same inputs.

To SD boot an X70 1.10 from a FujiMod SD card directory:

```json
{
  "configuration": "x70-110-rec",
  "overlays": ["x70-110-browser-camera"],
  "update": "FPUPDATE.DAT",
  "boot_rom": "bootrom.bin",
  "camera_sections": "catalog/bodies/x70/cameras/reference",
  "sd_card": "path/to/sd-card",
  "gpio": "0:10=0,0:11=0",
  "keys": "t=0.3:-controls-gpio-0-10;t=0.3:-controls-gpio-0-11"
}
```

`gpio` holds the two SD boot buttons from power-on and `keys` releases them
0.3 s into the boot.

| Tool | Does |
|---|---|
| `camera_open`, `camera_list`, `camera_close` | Open, list and close cameras |
| `camera_checkpoint`, `camera_restore` | Save and return to named checkpoints, optionally files |
| `camera_run` | Advance guest seconds, or until REC |
| `camera_controls` | Control ids, kinds and positions |
| `camera_press`, `camera_turn` | Press a control or turn a rotary control, each edge confirmed by the firmware's key scanner, then settle |
| `camera_screen`, `camera_state` | Screen and state without advancing time |
| `camera_read_memory` | Physical memory as a hex dump or file |
| `camera_profile_start`, `camera_profile_collect`, `camera_profile_show`, `camera_profile_diff` | Instructions executed per basic block, and what changed between two runs |

Every camera runs on its own thread; calls to different cameras proceed in
parallel. Open cameras with `execution_policy` `exact` when their runs will
be compared, so a restored checkpoint replays the same way.

Controls connect when first used, once the firmware has loaded its
configuration: `fujimod-emulator-controls` builds the camera's control
wiring and input confirmation from the catalog and installs them. A camera
opened from a checkpoint file adopts the ones saved with it.

To find the code that handles a key: checkpoint, profile an idle run of the
same length as the press, restore, profile the press, and diff the two.
