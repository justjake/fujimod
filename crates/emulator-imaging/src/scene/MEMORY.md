# Scene memory

A RAF scene must fit in a phone browser and in several concurrent walkthrough
workers. Every rule here keeps output bit-identical: the same linear values,
binned means, projections and fingerprints as the full representation.

## Sensor radiance: the RAF's own counts

`Radiance` stores one little-endian `u16` count per site (`raw - black`,
clamped at zero) and the RAF's normalization `norm` (`f64`). A site's radiance
is `(f64::from(count) / norm) as f32`, exactly what `compute_radiance` produced
before, so values are unchanged at 2 bytes per site instead of 4.

- `RafScene` carries `counts` and `norm` instead of a `Vec<f32>`; readers use a
  `radiance(index)` accessor (`raf_to_radiance_rgb`, the Python RAF binding,
  tests).
- Checkpoints store the count bytes as the frame's blob and `norm` in the
  frame's metadata (`PixelKind::Mosaic`). A restore shares the blob, so the frame
  keeps its pixel identity (`Frame::same_pixels`).
- `Frame::new` validates `norm` finite and positive. Tests build mosaic frames
  from counts and a norm.

## No full-resolution running sums

`Scene` keeps row prefix sums (`[u32; 3]` per pixel) only for scenes it builds
small: RGB images and binned scenes. A full-resolution mosaic scene stores
nothing beyond the frame's `MosaicSites`:

- `linear(x, y)` computes the site's 16-bit value from `MosaicSites::site` and
  the frame's maximum, with the current rounding.
- Row means (`horizontal`) take a row's prefix sums computed on demand. Callers
  that sweep a row ask for the row once (`Scene::row`, borrowed for stored
  prefixes, computed for mosaic scenes) and evaluate every column from it.
- `binned(dimensions)` reads source rows through the same row accessor in one
  pass, holding at most the rows a bin overlaps.

The scene cache keeps the current frame's full scene, now about the size of its
`MosaicSites` cell means. Binned scenes keep their prefix sums. No cache needs a
pixel-count bound for the full scene; the existing bounds on binned and readout
caches stay as they are.

## No retained file bytes

Input history records the installed `Arc<Frame>` for a RAF instead of the RAF
file bytes, so history and the live sensor input share one radiance buffer.
Callers drop RAF bytes after `sensor_raf` parses them (Python `load_scene`, the
browser worker).
