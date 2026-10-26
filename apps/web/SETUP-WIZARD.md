# Service-mode setup wizard (design, 2026-10-18)

An illustrated, animated walkthrough that takes a camera owner through:

1. **Enter service mode.** Plug USB into the computer and camera, switch the
   camera off (arrow along the switch, switch moves ON → OFF), zoom out to show
   the buttons to hold, then switch on while holding them (the reverse arrow,
   OFF → ON). USB goes in first: the X70 checks the USB, UP and full-shutter key
   states at power-on.
2. **Connect over WebUSB.** "Camera is in service mode and connected", then
   "Connect camera over USB". An arrow points to where Chrome shows its device
   chooser, captioned "Select your camera here. It will not be modified until
   you approve."
3. **Debug mode**, through the connected session.
4. **Leave service mode** by removing the battery from the grip, then starting
   normally.

## Decisions (owner, 2026-10-18)

- **One 2D illustrated style for every camera.** Each body is a drawing with
  the same named parts (power switch, shutter, D-pad keys, USB port, battery
  door), so the animation code is shared and a new camera needs a drawing and
  its step data. 3D models (devices/x70/x70.glb) can come later where one
  exists; they are not the basis.
- **Cameras: X70, X-M1 and X-E2S.** Entry button combinations and exits that are
  not reviewed for a body are shown as provisional.
- **Exit:** as soon as the camera is up in service mode, arm the exit guard on
  bodies where we know how (X70: `a3=1`),
  because it is harmless and adds protection. The wizard always teaches
  the battery removal, the exit that works on every body, including those
  where no guard is known.
- **Animation library: anime.js 4.5.0 (MIT).** The owner prefers open source:
  GSAP and Remotion were set aside for their licences, Rive for its account-bound
  pipeline, and dotLottie (open) because per-camera baked files suit one shared
  script worse than animating our own SVG.

## How it is built (`src/setup/`, route `/setup/`)

- `cameras.ts`: each body as measurements and named parts (top plate, back, base,
  USB side), plus whether its entry was tried and whether an exit guard is known.
- `scene.ts`: draws a camera from those parts as the back plus an oblique top plate
  or base. Everything the picture can show is one state object (zoom, tilt, switch
  angle, arrows, cable, battery door); `draw` renders it each frame.
- `steps.ts`: one anime.js timeline per step, moving that state from wherever the
  previous step left it, so transitions are continuous. Captions start at
  timeline labels and are read from the playhead, so seeking (screenshot checks,
  reduced motion) keeps captions right. `window.setupSeek(ms)` jumps for checks.
- **Exit order taught:** battery out with the cable still in, then unplug, switch
  to OFF, battery back, switch on without holding buttons. Unplugging first is
  avoided because USB removal is itself a service-mode event on these bodies.
- **Connect:** the USB package loads when the step opens, so the button goes
  straight to Chrome's chooser; a dimmed overlay points up at it. Chrome shows the
  chooser as a dialog at the top centre of the page (owner's screenshot, 2026-10-19).
- **Not built yet:** arming the exit guard and turning on debug mode from the
  browser. The page says so; both need reviewed camera operations in the Rust core.
