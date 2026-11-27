# fujimod-mcp

An MCP server for experimenting on emulated cameras. Cameras stay running
between tool calls, so an agent boots once, checkpoints, and runs many
experiments from that point. Design and status: [../MCP.md](../MCP.md).

```sh
cargo build --release -p fujimod-mcp
target/release/fujimod-mcp [--modules camera] [--output DIR]
```

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

`camera_open` takes a catalog firmware id and a boot. The firmware update,
boot ROM and reference camera sections come from its source record under
`FUJIMOD_FIRMWARE_LIBRARY`, or the current directory when it is unset. It
restores the library's saved boot for the same
inputs when it holds one, boots to REC otherwise, or restores a checkpoint
file written by a camera built from the same inputs. `boot_cache: false` boots
from scratch and saves nothing.

To SD boot an X70 1.10 from its current release card:

```json
{
  "firmware": "x70-110",
  "boot": "sd-card",
  "execution_policy": "interactive"
}
```

Give `sd_card` to boot a FujiMod tree from a directory instead:

```json
{
  "firmware": "x70-110",
  "boot": "sd-card",
  "sd_card": "path/to/sd-card",
  "execution_policy": "interactive"
}
```

For autostart, let the card's installer write internal flash once:

```json
{
  "firmware": "x70-110",
  "boot": "autostart",
  "execution_policy": "interactive"
}
```

The builder holds the body's SD boot controls from power-on and releases
them at REC; an autostart holds nothing and starts from the kept flash.
The first autostart open boots the card, chooses AUTOMATIC on its FUJIMOD
SET-UP BOOT SETUP page, and keeps the stage-1 blocks the card's installer
wrote.
It downloads a release card once into the state directory and checks every
file's SHA-256 before booting. A `stock` boot takes no `sd_card`, or uses
its files as extra files on the otherwise empty card. `execution_policy`
defaults to `exact`.

| Tool                                                                                           | Does                                                                                                                                                       |
| ---------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `camera_open`, `camera_list`, `camera_close`                                                   | Open, list and close cameras                                                                                                                               |
| `camera_checkpoint`, `camera_restore`                                                          | Save and return to named checkpoints, optionally files                                                                                                     |
| `camera_run`                                                                                   | Advance guest seconds, or until REC                                                                                                                        |
| `camera_controls`                                                                              | Control ids, kinds and positions                                                                                                                           |
| `camera_press`, `camera_turn`                                                                  | Press a control or turn a rotary control, each edge confirmed by the firmware's key scanner, then settle                                                   |
| `camera_set`                                                                                   | Move a dial, switch, ring or lever to a named position through the contact driver, confirmed by the firmware's own observation of the control, then settle |
| `camera_screen`, `camera_state`                                                                | Screen and state without advancing time                                                                                                                    |
| `camera_read_memory`                                                                           | Physical memory as a hex dump or file                                                                                                                      |
| `camera_profile_start`, `camera_profile_collect`, `camera_profile_show`, `camera_profile_diff` | Instructions executed per basic block, and what changed between two runs                                                                                   |

Every camera runs on its own thread; calls to different cameras proceed in
parallel. Open cameras with `execution_policy` `exact` when their runs will
be compared, so a restored checkpoint replays the same way.

Controls connect when first used, once the firmware has loaded its
configuration: `fujimod-emulator-controls` builds the camera's control
wiring and input confirmation from the catalog and installs them. A camera
opened from a checkpoint file adopts the ones saved with it.

To find the code that handles a key: checkpoint, profile an idle run of the
same length as the press, restore, profile the press, and diff the two.
