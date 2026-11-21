# fujimod-emulator-imaging

The image layer: scene frames, optics, exposure, radiometry, analysis, film and
the EXR II ISP interface.

## Host scene for the XP5 (X-T5) emulator: `XT5_SCENE`

The XP5 board does not model the sensor readout or the develop blocks. The
`XT5_SCENE` environment variable puts a host picture into the live-view output
(MOVDC) and into every JPEG encode, so live view and still captures show it.
It is a host input: it is not part of the run definition, checkpoints or the
wiring hash, and a snapshot loads with or without it.

| Value | Scene |
| --- | --- |
| `bars` | Colour bars with a moving marker |
| `PATH.bmp` | An uncompressed 24- or 32-bit BMP |
| `video:avf:N[@WxH[@FPS]]` | AVFoundation video device `N`: webcam, OBS Virtual Camera, iPhone Continuity Camera, or `Capture screen N` |
| `video:PATH_OR_URL[@WxH[@FPS]]` | A video file (looped, paced in real time) or a stream URL |
| `video:lavfi:GRAPH[@WxH[@FPS]]` | An ffmpeg lavfi source, for example `testsrc2=size=640x480:rate=30` |
| `fifo:PATH@WxH` | Raw `rgb24` frames (`W*H*3` bytes each, top row first) from any producer |

List the AVFoundation devices with:

```sh
ffmpeg -f avfoundation -list_devices true -i ""
```

`video:` runs `ffmpeg` as a child process (`XT5_FFMPEG` if set, otherwise
`ffmpeg` on `PATH`, then `/opt/homebrew/bin/ffmpeg` and
`/usr/local/bin/ffmpeg`). ffmpeg scales the input to cover `W x H` keeping its
aspect ratio, crops the centre, and writes raw RGB frames into a pipe. The
default is `1080x720` (3:2, like the X-T5 sensor) at 30 fps. Devices open
with `-framerate FPS -pixel_format nv12`; pick an `@FPS` the device offers.
The terminal that starts the emulator needs macOS camera (or screen
recording) permission.

A background thread keeps only the newest complete frame; the emulator never
waits for it. Each live-view frame and each JPEG encode takes the newest frame
at that moment and scales it bilinearly to the programmed size, so a still is
a snapshot of the feed (the main image and its thumbnails are separate encodes
and can come from adjacent frames). Until the first frame arrives, or when
ffmpeg fails before one, the scene shows colour bars and the reason is logged
once. ffmpeg messages appear on standard error prefixed with
`XT5_SCENE ffmpeg:`.

Examples, with `fe` the X-T5 launcher
(`fujimod-emulator boot --configuration xt5-431 --update FWUP0030.DAT`):

```sh
# MacBook camera (device 1)
XT5_SCENE=video:avf:1@1080x720@30 fe --output run-cam --seconds 30 --viewer-port 8090
# A video file, looped
XT5_SCENE=video:$HOME/clips/street.mp4@1080x720 fe --output run-clip --seconds 30
# Any producer through a FIFO
mkfifo /tmp/xt5.rgb
ffmpeg -re -f lavfi -i testsrc2=size=640x480:rate=30 -vf scale=1080:720 \
  -pix_fmt rgb24 -f rawvideo - > /tmp/xt5.rgb &
XT5_SCENE=fifo:/tmp/xt5.rgb@1080x720 fe --output run-fifo --seconds 30
```

Check a source without the emulator (prints the frame count and writes the
still the JPEG shortcut would encode as a PPM):

```sh
XT5_SCENE='video:lavfi:testsrc2=size=640x480:rate=30@320x240' \
  cargo run -p fujimod-emulator-imaging --example scene_probe -- 3 /tmp/scene.ppm
```

The ffmpeg path test runs with
`cargo test -p fujimod-emulator-imaging -- --ignored ffmpeg_lavfi`.
