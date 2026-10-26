# Live view cost

Target: REC live view at three times camera speed, 0.33 CPU seconds per
emulated second, with bit-identical output. Measured 2026-10-23 on the X70
with the default RAF scene (release extension, `rec_profile.py`, 2 emulated
seconds, 10 frames): 0.93 CPU s per emulated second, of which the statistics
producer (`isp-block18000`) takes 90 ms per frame and each of the two image
output channels about 30 ms.

The scene is static, yet every frame repeats the full image path. The hot
frames are `ExposedFrame::sample`, `Scene::linear`, `Scene::row`,
`Scene::binned`, `readout::horizontal_row`, `encode_response_coordinates` and
`radiometric::Image::color`.

## Approach

1. **Find what changes per frame.** `radiometric::Renderer::capture` reuses the previous
   `Image` (and its cached renders and measurements) when the frame,
   exposure, aperture, focus shift, white-balance gain and film words are
   unchanged. Record which of these differ between consecutive live-view
   frames.
2. **Keep the expensive work independent of what changes.** Optics are
   linear in radiance: scenes, binned scenes and projected linear values do
   not depend on exposure, so they are cached across exposure changes and
   only the response encoding is redone. Anything that depends only on the
   scene, focus and geometry is computed once per change of those inputs.
3. **No uncached paths.** `bin_exposure` builds and bins a new `Scene` on
   every call; it uses the projection's scene and binned caches like
   `prepare_binned_windows`.
4. **Share one frame's work among consumers.** The producer and both output
   channels read the same captured image; a projection or render one of them
   prepares is reused by the others when their geometry matches.

Every change keeps output bit-identical; tests compare against the uncached
path. Cache entries are keyed by their inputs, with no size constants to tune.
