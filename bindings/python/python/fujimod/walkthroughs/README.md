# Python walkthrough recording

Walkthrough authoring requires Python 3.14. Install FujiMod with its
`walkthroughs` extra. Screenshot acceptance does not require ffmpeg. Pass
`video=True` to `Guide` when recording docs media; that mode requires ffmpeg on
PATH. Other FujiMod APIs retain their
existing Python requirement.
The library has no camera, firmware or repository dependency.

```python
from fujimod.walkthroughs import open_menu, ref, walkthrough

page = ref(menu="settings_item")
option = ref(menu="option_item")
settings = open_menu(page, title="Settings")


@walkthrough("Use a feature", start_from=settings)
def example(camera, guide):
    with guide.step(t"Choose {option.path(relative_to=page)}"):
        camera.select_menu_item(option)
    camera.open_menu_item(option)
    with guide.step("Example option"):
        assert camera.current_option() == "example"
```

A normal walkthrough can also be a `start_from` prerequisite. Branches start
independently from their parent's completed state. A host must reject cycles
and unresolved prerequisites before execution. The current runner replays
prerequisites in a fresh camera, runs their assertions and settling, then
starts the child's recording. Replay contributes no child steps, input records
or coverage. Failed prerequisites prevent the child from executing. Checkpoint
caching is not part of the authoring API.

`open_menu(item, title=..., start_from=None)` creates an ordinary walkthrough
with one illustrated instruction: `Choose Menu > Item > Submenu`. The screenshot
shows the destination highlighted, then the guide activates it. Its adapter implements
`menu_reference(item)`, `select_menu_item(item)` and `open_menu_item(item)`. Factory
identity comes from the module variable name; decorated functions use their
function name. Filenames and containing directories provide the remaining
identity and ownership. There are no explicit IDs or mod lists.

`discover(path)` imports one file or all public Python files in a directory in
one fresh package. Several walkthroughs may share a file; relative imports can
reference prerequisites in other files. Discovery reloads local source instead
of accepting stale bytecode, and imported guides keep their original ownership.

Construct a fresh camera adapter per segment, then call
`Guide(camera, fresh_output, identity=identity, title=title).run(function,
prerequisites=[(parent_identity, parent_function), ...])` with prerequisites in
root-to-parent order. The host owns build and execution/source identity. Python
assertions must be enabled. The optional `navigation_hold=.12` and
`action_hold=.45` constructor arguments set recording-only result holds.

A camera adapter implements:

- `start_recording(guide)`: attach observation to existing execution/input paths.
- `begin_segment()`: reset the local guest clock and supply the first frame after replay.
- `capture()`: settle and return a clean `fujimod.recording.Frame`.
- `stop_recording()`: remove recording hooks, including on failure.

It calls `guide.frame(frame, seconds)` at monotonically increasing guest times,
`guide.input(name, pressed, seconds)` for real input edges (returns an index),
`guide.touch(phase, x, y, seconds)` for each touch phase (`start`, `move`,
`end`) at a position in the recorded frames' coordinates (returns an index),
and `guide.interaction(surface, item, input_index=..., **evidence)` for observed
interactions. Frame sampling never drives device state. Only non-fixture press
inputs or touch starts can anchor coverage. Report inputs carry `kind`
(`control` or `touch`); a touch also records its `phase`, `x` and `y`, with
`pressed` true at the start, false at the lift and null while moving. The
videos draw a translucent circle at a finger while it is down, following its
moves; a tap ends in a ripple and a longer contact fades where it lifts. The
clean video shows touches too, so a tap is always visible. Screenshots stay unmarked. Input badges push onto a bottom-right stack, newest
at the bottom, retaining up to five presses. Repeated presses have separate
entries; each remains visible for five presentation seconds after release,
fading only during the final 0.4 seconds. New presses push the oldest entries
off the top when the stack is full.
Input edges flush the prior interval before changing the stack.

Adapters wrap completed user actions in `with guide.action(navigation=False):`.
The adapter executes only as far as real input/render readiness requires; it
must not add fixed settling delays for the video. On successful exit the
recorder captures the ready result and adds its configured presentation hold:
0.12 seconds for navigation, 0.45 seconds for feature actions. Neither holds nor
extra captures run during prerequisite replay. The adapter marks navigation
intent explicitly; a direction key can also operate a feature.

`guide.step(title, text='', hold=1.5)` captures on successful block exit, then
holds that observation without advancing guest time. For menu navigation, use
`Choose Menu > Item > Submenu`, matching the displayed labels. Select/highlight
the destination inside the block, then activate it and assert success after the
block. Screenshots explaining a setting or its effect show that resulting state.
Use physical-button instructions only when needed, such as `Press MENU/OK to apply`.
This follows the [Apple Style Guide's menu convention](https://help.apple.com/pdf/applestyleguide/en_US/apple-style-guide.pdf).

`guide.pause(seconds=.35)`
shows a settled action result without creating a documentation step. Explicit
holds are never compressed. Unchanged frames without active/fading inputs keep
at most 0.4 seconds of guest waiting; transitions and input intervals remain.
Input records retain their actual guest times and presentation timestamps.
Neither video pacing nor holds change emulator execution or CPU speed.

Presentation panels cover part of the video while keeping the camera canvas at
its original size. The camera adapter supplies observed snapshots and frames:

```python
card = guide.show_card(snapshot)
assert card.visible
assert card.label == "(untitled)"
assert "/DCIM/100_FUJI/DSCF0001.JPG" in card.paths

# After the camera has formatted the card, reveal its observed new contents.
card.format(formatted_snapshot)
assert not card.paths
card.hide()

display = guide.show_display(hdmi_frame, label="HDMI", width=.6)
display.update(next_hdmi_frame)
assert display.visible and display.label == "HDMI"
display.hide()
```

`CardSnapshot` contains `label`, `rows` and `highlight`. Each of at most six
`CardRow` entries contains `name`, `path`, `depth`, `directory` and `bytes`.
Adapters prune the tree before presenting it. Card handles expose immutable
snapshot copies, the displayed label and paths for assertions. An empty label
is displayed as `(untitled)`. `guide.format_card(snapshot)` and
`guide.hide_card()` operate on the guide's current card too.

Cards slide in with cubic easing. Formatting rapidly shakes the old card before
the new label and files appear. `show_display` uses the same rounded panel style
for HDMI, EVF or a labeled reference image. Its width is a fraction of the main
canvas, from `.25` to `.75`. Show/hide animations add half a second of
presentation time; frame updates add no holds or guest time. These overlays
appear in both videos, and their events are recorded under `overlays` in the
report. Native screenshots and option-coverage evidence remain clean.

Use `guide.pause(1.5)` after opening a menu before navigating within it. The
shared `open_menu` starter adds this pause automatically, after activation.

A zero hold adds no presentation time. The final frame is retained. Failed steps
and fixtures fail the run even if author code catches their exception.
`guide.fixture(description)` records setup interventions and suppresses coverage.

`guide.json` schema 3 contains identity, title, prerequisite results, pass/fail,
numbered steps, screenshot hashes, local video timestamps, input edges,
interactions, fixtures, presentation timing and artifact hashes. `poster.png`
is the segment's actual starting frame, captured after prerequisite replay.
That state includes inputs after the parent's last illustrated step; its last
screenshot may still show a highlighted destination before activation.
`presentation.duration_seconds` includes the complete video, including those
trailing inputs, and can extend beyond the last step's `video_end`.
The host records `start_from` as the parent identity so consumers can link guides.
Failed runs retain diagnostics and never qualify coverage. The Rust recorder
owns screenshot encoding, frame reuse, badges and optional 30 fps video.
`media` identifies `screenshots` or `video`; only video reports contain
`video.mp4` and `video-clean.mp4`. Websites request video recordings explicitly.
Image assertions can call `camera.capture().png()`; video sampling and holds
pass native frames directly without PNG encoding or decoding.

Step titles and descriptions accept ordinary strings or standard Python
template strings (`t"Choose {item}"`). Templates retain the interpolated menu
reference; f-strings eagerly stringify it and are refused by reference objects.
Normal Python interpolation and formatting still work for other values.

`ref(menu=symbol_or_code, key=None, via=None)` identifies a menu entry. The
adapter owns firmware-specific symbol/code lookup for **stock and mod menus**.
Use `key=` to identify a particular row within a stock menu. Shared submenu
descriptors can specify their opening row with `via=ref(menu=parent, key=...)`.
Missing references, ambiguous openers, cycles and unrelated ancestors fail.

`{item}` renders icon and English name; `{item.name}` and `{item.icon}` select
either presentation while retaining the entry identity. `{item.path()}` renders
the full menu path; `{item.path(relative_to=parent)}` omits that ancestor and
everything above it. Paths contain ordered menu tokens and have one standard
separator. Entries without artwork render their names.

The adapter's `menu_reference(reference)` returns `MenuEntry(identity, name,
icon=None, ancestors=())` from `fujimod.walkthroughs.text`. `icon` is PNG bytes;
ancestors are resolved entries from root to immediate parent. Resolution must
be observational and contributes no interaction or coverage.

Each step's `title` and `text` are JSON token arrays: `text` tokens hold literal
text, `menu` tokens hold identity, English name, display mode and optional icon,
and `menu_path` tokens hold ordered menu entries. Icon descriptors contain a
relative `icons/<sha256>.png` asset and its hash; assets are bundled with the
report and verified like screenshots. Renderers use these resolved values,
without firmware access or evaluation of Python expressions.
