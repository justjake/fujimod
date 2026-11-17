# MULTI-SHOT STACK merge

`fujimod-multishot` merges handheld bursts of X-Trans RAFs. A capture plan is
three nested loops (burst frames, exposure steps, focus steps); the merge is a
list of stages, each reducing one loop. This crate implements the
**resolution** stage, handheld multi-frame super-resolution after Wronski et
al., "Handheld Multi-Frame Super-Resolution" (2019), and the plain robust
average that falls out of it at scale 1. Exposure (HDR) and focus stages are
declared in `plan.rs` so plans can be validated; they are refused at run time.

## Pipeline

1. **RAF reader** (`raf.rs`): the RAF header's offset table, the metadata
   records (active area 0x110/0x111, X-Trans layout 0x131, as-shot white
   balance 0x2ff0) and the raw section's Fuji directory (plane size 0xf001/2,
   bit depth 0xf003, data offset and length 0xf007/8, black levels 0xf00a).
   Exposure time, f-number, ISO, bias, focal length and capture time come
   from the embedded JPEG's EXIF. Compressed planes (lossless or lossy) are
   refused with a message naming the setting to change. Rows are read on
   demand, so no frame is held whole.
2. **Alignment** (`align.rs`, `fit.rs`): every 3x3 block of the X-Trans grid
   holds 2R 5G 2B sites, so block means form a CFA-phase-free luma plane at
   one third resolution. A global translation search on the coarsest pyramid
   level seeds per-tile matching: box pyramids, zero-mean normalized
   correlation squared, bounded symmetric search, texture, correlation and
   competing-peak tests, quadratic sub-pixel peaks and a reverse closure
   check. Tiles feed a deterministic consensus fit (similarity by default,
   affine available) with inlier, residual, 4x4 coverage, determinant and
   corner-motion gates. A rejected frame is reported and left out, never
   merged with a guessed transform. The fit is refined by Huber-weighted
   Gauss-Newton registration (affine plus gain and offset, with step
   halving), first on block luma, then on full-resolution green using the
   reference's real green sites. The previous frame's transform is the prior
   for the next.
3. **Resolution merge** (`merge.rs`): every unclipped CFA site of every
   accepted frame is mapped into reference coordinates and splatted, per
   color, onto an output grid 1.5x (default) or 2x finer than the sensor. The
   kernel follows the reference's local structure tensor: narrow across
   edges, long along them, wide in flat areas. A robustness weight per frame
   and reference block compares the frame with the reference against a noise
   model measured from the burst itself (signal-dependent block-mean
   variance, lower envelope over flat blocks) plus a texture allowance, then
   takes the 3x3 minimum, so moving subjects are rejected instead of ghosted.
   A weak prior from the reference's block means fills sparse output
   pixels. The output is full-color linear camera RGB with no demosaic.
4. **Output** (`dng.rs`, `preview.rs`): a three-sample LinearRaw DNG, 16
   bits per sample, written one strip per merge strip after a header that
   already holds every strip offset; plus an sRGB PNG preview. The DNG uses
   the as-shot neutral and an sRGB-primaries color matrix (no calibrated
   X-H2S matrix yet), so color is approximate; resolution and noise are
   measured on the linear data.
5. **Measurement** (`measure.rs`, `demosaic.rs`): ISO 12233 slanted-edge MTF
   and flat-patch noise of the merge, of the reference alone through the same
   merge, and of the reference demosaiced (green by normalized convolution,
   red and blue by color differences) and upscaled bicubically to the same
   grid.

## Command line

```sh
cargo run --release -p fujimod-multishot -- info FRAME.RAF
cargo run --release -p fujimod-multishot -- look FRAME.RAF --out look.png [--region X,Y,W,H]
cargo run --release -p fujimod-multishot -- merge --out OUT [--scale 1.5] [--region X,Y,W,H] FRAMES/*.RAF
cargo run --release -p fujimod-multishot -- measure --out OUT --edge X,Y,W,H --flat X,Y,W,H FRAMES/*.RAF
cargo run --release -p fujimod-multishot -- align-jpeg --out OUT FRAMES/*.JPG
cargo run --release -p fujimod-multishot -- synth --out OUT [--width W --height H --frames N]
```

`info` prints geometry, CFA layout, levels, exposure and per-tile-position
means of the frame center (sites of one color should agree, which checks the
CFA phase). `look` writes a block preview (one pixel per 3x3 sites) for
choosing regions. `merge` writes `alignment.json`, `merged.dng`,
`preview.png` and `report.json`; `--alignment` reuses an alignment, and
`--region` merges one reference region quickly. `measure` writes PNG crops of
each variant and `measure.json`. `align-jpeg` registers camera JPEGs with
the same matcher, fit and refinement when no RAW exists. `synth` writes
uncompressed RAFs of the synthetic chart for end-to-end checks.

## Measured results

### Synthetic bursts (`tests/synthetic.rs`, `cargo test --release`)

A 600x420-site chart (slanted edge, Siemens star, fine random color cells, a
flat patch and an oblique grating) is seen through known handheld motion
(translations within ±3 sites, rotations within ±0.11°), a Gaussian lens
(σ = 0.5 site) and each site's full aperture, sampled through the X-Trans
CFA with shot and read noise. The truth is what a full-color sensor at the
reference position would record, on the output grid. Twelve frames, scale
1.5, default parameters:

| Measure | Merge | Single frame, same merge | Demosaic + bicubic | Truth |
| --- | --- | --- | --- | --- |
| Slanted-edge MTF50 (cycles per site) | **0.298** | 0.238 | 0.273 | 0.329 |
| Fine-detail RMSE against truth | **0.0142** | | 0.0352 | 0 |
| Flat-patch luma noise (relative std) | **0.0015** | | 0.0073 | |

The merge resolves a neutral edge 9% better than demosaicing and upscaling
the best single frame (25% better than the single frame through the same
kernels), recovers fine color detail with 2.5x lower error and has 4.8x lower
noise. Demosaicing a neutral edge is the single frame's best case; the gain
on colored and aliasing-prone detail is the RMSE column.

- **Negative control:** eight identical frames merge to the single-frame
  result (largest pixel difference 1.8e-7; equal MTF50). Registration of
  identical frames returns the identity within 1e-3 sites.
- **Alignment:** recovered transforms are within 0.087 sites of the truth
  at the image corners over six frames.
- **Moving subject:** a disc moving 6 sites per frame leaves an error of
  0.021 against the reference scene with robustness and 0.069 without.
- **Noise model:** the measured green block-mean variance at 0.3 is 0.53x the
  rendered value (the flat-block selection biases it low).

### Full size

Twenty 6240x4160 synthetic RAFs (point-sampled chart): alignment 116 s,
merge 92 s on 8 threads into a 9360x6240 DNG. With 48-row strips the
**peak strip memory is 115.5 MiB**: each of 8 workers holds a 10.8 MB
accumulator (24 bytes per output pixel) and one frame's row band (about 1 MB),
plus kernel, block and output strips. Alignment peaks at about 470 MiB,
dominated by full-resolution green planes for the final refinement; the
process maximum RSS was 558 MB. At this size the chart's only fine texture is
the random-color panel, which aliases at block resolution; 7 of 20 frames
were rejected by the inlier gates, as designed, rather than merged.

### Real X-H2S data

Uncompressed handheld RAFs are still needed for a real-scene resolution
measurement; the first RAW sets were compressed and are refused. Camera
JPEGs of handheld brackets and bursts registered with `align-jpeg` show:

- frame-to-frame hand motion up to about 90 pixels and 0.18° of roll, with
  scale within 0.2%; the global translation search keeps such motion inside
  the tile search;
- AE bracket (3 frames): both non-reference frames register (470 and 401
  inliers; gain and offset absorb the exposure change);
- burst with shallow depth of field (10 frames): 6 of 9 register; 3 fail the
  8-of-16 grid-coverage gate because only the in-focus subject has texture;
- focus bracket (19 frames): frames register while the reference's texture
  stays sharp, then fail as defocus removes it. A focus stage must chain
  adjacent focus steps and check drift against the reference.

## Limits and next steps

- Compressed RAFs (lossless and lossy) are refused.
- The DNG color matrix is a placeholder.
- One global transform per frame; local residual motion is rejected by
  robustness rather than aligned. A per-tile residual field would keep more
  samples in scenes with parallax.
- Coverage and inlier gates are caller policy; shallow depth-of-field bursts
  need a policy that accepts a well-supported subject region.
- Exposure (HDR) and focus stages are declared in `plan.rs` and not built.
