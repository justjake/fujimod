# Native recording

`fujimod-recording` owns RGB captures, PNG screenshots, input badges, presentation
timing and optional annotated/clean video encoders. It is independent of camera
transport and execution backends. Camera sessions import declared BGRA buffers;
emulator sessions supply composed RGB from native scanout. Both use the same
immutable `Frame` and `Recorder`.

PNG encoding is lazy and shared by clones of a frame. Video consumes RGB
directly, fits a changed image once to its initial canvas, and reuses clean and
annotated buffers until pixels or badge appearance change. The two ffmpeg
processes consume that same rendering. Screenshots retain native resolution and
never include input badges. Badges and touch circles are anti-aliased vector
paths drawn with tiny-skia; badge text is Inter SemiBold (`assets/fonts`, SIL
Open Font License) outlined through ab_glyph with the font's advances and
kerning. Catalog control IDs, icons, colors and geometry come from
[`catalog/input-presentation.json`](../../catalog/input-presentation.json),
shared with the web overlay.

Screenshots are the default. Creating a screenshot recorder never starts or
checks ffmpeg. Only `video=true` requires that executable. Timestamped input and
frame events are monotonic; holds add presentation time without advancing any
source clock. Unchanged idle intervals retain at most 0.4 seconds. Released
badges remain for five presentation seconds, fading in the last 0.4 seconds.
Touches (`Recorder::touch`, phases start, move and end, in the recorded frames'
coordinates) appear in both videos, never in screenshots: a translucent circle
at the finger while it is down, popping in as it lands and following moves. A
tap (a contact under `tap_ms`) ends in a ripple, a ring that grows from the
circle and fades as the circle goes; a longer contact's circle fades where the
finger lifts. Every touch shows as down for at least `minimum_ms`. Geometry and
timing are the `touch` entry of
[`catalog/input-presentation.json`](../../catalog/input-presentation.json).
LCD frames are sampled at most 15 times a second; video runs at the recorder's
frame rate (30 fps for walkthroughs), so badges and touches animate smoothly.
The final observation is retained and finalization is idempotent. Dropping a
recorder reaps its child encoders, including on failure.

SD cards and secondary displays overlay the original canvas without resizing
the camera image. Both use cubic slide motion. SD presentation accepts a volume
label and at most six directory rows; an empty label renders as `(untitled)`.
Formatting shakes the old card before the caller publishes its new snapshot.
Secondary displays use a rounded rectangle with a small caller-supplied label,
so HDMI, EVF and reference images share one style. These panels appear in both
videos; screenshots retain their original pixels.

`cache::CaptureCache` is an observational rendering memo. A source provides its
register/configuration key and a read function. Reuse requires the same key and
identical bytes at **every** previously addressed span. A changed register,
buffer or palette causes a new rendering. Refused captures never enter the
cache. Emulator sessions keep this host cache outside checkpoints and complete
state fingerprints; it neither drives execution nor acknowledges hardware.

Python exposes `fujimod.recording.Frame`, `Recorder` and `capture_scanout` through
the native binding. Guide code receives native frames; only an explicit image
consumer calls `Frame.png()`. `Frame.save()` and `Recorder.screenshot()` write
screenshots and return their SHA-256. Importing PNG is supported for image files
and test inputs; the native scanout/video path never uses PNG intermediates.

Run `cargo test -p fujimod-recording` and
`cargo clippy -p fujimod-recording --all-targets -- -D warnings` from the
workspace. Encoder integration checks run when ffmpeg is available. Tests
cover raw pixel parity, source invalidation, source-read failures, timing,
independent badge releases, touch indicators, clean/annotated output and
final-frame retention.
