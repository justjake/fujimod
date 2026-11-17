# MULTI-SHOT STACK merge

`fujimod-multishot` merges handheld bursts of X-Trans RAFs. A capture plan is
three nested loops (burst frames, exposure steps, focus steps); the merge is a
list of stages, each reducing one loop. This crate implements the
**resolution** stage, handheld multi-frame super-resolution after Wronski et
al., "Handheld Multi-Frame Super-Resolution" (2019), and the plain robust
average that falls out of it at scale 1, and the **exposure** (HDR) stage for
exposure brackets of single frames. The focus stage, and exposure merges of
bursts per step, are declared in `plan.rs` so plans can be validated; they
are refused.

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
   accepted frame is mapped into reference coordinates and splatted onto an
   output grid 1.5x (default) or 2x finer than the sensor. Green (5 of 9
   X-Trans sites) is merged first; red and blue (2 of 9 each) are merged as
   white-balanced differences to the merged green at their own sample
   positions, so they inherit green's resolution (`--color independent`
   splats each color from its own samples instead). The
   kernel follows the reference's local structure tensor: narrow across
   edges, long along them, wide in flat areas. A robustness weight per frame
   and reference block compares the frame with the reference against a noise
   model measured from the burst itself (signal-dependent block-mean
   variance, lower envelope over flat blocks) plus a texture allowance, then
   takes the 3x3 minimum, so moving subjects are rejected instead of ghosted.
   A weak prior from the reference's block means fills sparse output
   pixels. The output is full-color linear camera RGB with no demosaic.
   With `--local` (`local.rs`), each frame also gets a residual shift field
   on 96-site tiles: a translation with gain and offset is refined by
   Gauss-Newton on full-resolution green from the global transform. A tile
   counts only when its shift has a standard error under 0.05 sites and its
   gradient tensor is not one-directional (eigenvalue ratio at least 0.3, so
   a lone edge cannot invent a shift along itself). Outliers against the 3x3
   median are dropped and empty tiles filled from neighbors.
   With `--bracket` the frames are an exposure bracket. Alignment measures
   each frame's exposure factor to the reference (`gain`: the median ratio
   of block luma over mid-tones in `[0.01, 0.5]` in both frames) and scales
   the noise-model estimate to it. The merge scales every sample by its
   frame's gain and weights it by inverse noise variance relative to a
   reference sample at the same level, `(aL + b) / (g a L + g^2 b)` from the
   burst noise model, so long exposures carry the shadows; clipped sites
   are skipped, so highlights come from the short exposures. Robustness
   compares gain-scaled frames against the noise of both. A clipped
   reference block only bounds the radiance from below, so its prior then
   barely counts. The output is scaled so that 1.0 is the shortest
   exposure's white level. Without `--reference`, a bracket (recorded
   exposures spanning more than 1/6 EV) takes its shortest exposure as the
   reference: least motion blur, no clipped highlights. Gradient energy
   would pick the brightest frame instead.
4. **Output** (`dng.rs`, `preview.rs`): a three-sample LinearRaw DNG, 16
   bits per sample, written one strip per merge strip after a header that
   already holds every strip offset; plus an sRGB PNG preview. The DNG uses
   the as-shot neutral and an sRGB-primaries color matrix (no calibrated
   X-H2S matrix yet), so color is approximate; resolution and noise are
   measured on the linear data.
5. **Measurement** (`measure.rs`, `demosaic.rs`, `markesteijn.rs`): ISO
   12233 slanted-edge MTF and flat-patch noise of the merge, of the
   reference alone through the same merge, and of the reference demosaiced
   and upscaled bicubically to the same grid, two ways: a simple demosaic
   (green by normalized convolution, red and blue by color differences) and
   Markesteijn's 3-pass X-Trans demosaic, the strongest open-source single
   frame baseline. `markesteijn.rs` is written from the algorithm as
   LibRaw's `xtrans_interpolate` (LGPL-2.1) describes it, in floating
   point; on a 500x260-site crop of a real frame it agrees with LibRaw 0.22.1
   (through rawpy, 3 passes) within 0.5% (green) to 1.6% (blue) mean
   absolute difference, correlation 0.998 to 0.9998; the simple demosaic
   differs from LibRaw by 3% to 20%.

## Command line

```sh
cargo run --release -p fujimod-multishot -- info FRAME.RAF
cargo run --release -p fujimod-multishot -- look FRAME.RAF --out look.png [--region X,Y,W,H]
cargo run --release -p fujimod-multishot -- merge --out OUT [--scale 1.5] [--region X,Y,W,H] FRAMES/*.RAF
cargo run --release -p fujimod-multishot -- measure --out OUT --edge X,Y,W,H --flat X,Y,W,H FRAMES/*.RAF
cargo run --release -p fujimod-multishot -- merge --out OUT --bracket --scale 1 BRACKET/*.RAF
cargo run --release -p fujimod-multishot -- measure-hdr --out OUT --scale 1 --dark X,Y,W,H --bright X,Y,W,H BRACKET/*.RAF
cargo run --release -p fujimod-multishot -- align-jpeg --out OUT FRAMES/*.JPG
cargo run --release -p fujimod-multishot -- synth --out OUT [--width W --height H --frames N]
```

`info` prints geometry, CFA layout, levels, exposure and per-tile-position
means of the frame center (sites of one color should agree, which checks the
CFA phase). `look` writes a block preview (one pixel per 3x3 sites) for
choosing regions. `merge` writes `alignment.json`, `merged.dng`,
`preview.png` and `report.json`; `--alignment` reuses an alignment, and
`--region` merges one reference region quickly. `measure` writes PNG crops of
each variant and `measure.json`. `measure-hdr` merges a bracket and each of
its frames alone through the same kernels over a dark flat patch, and
reports the green noise floor against each one's clip level in stops,
plus the clipped fraction of a bright region. `align-jpeg` registers camera JPEGs with
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

| Measure | Merge | Single frame, same merge | Demosaic + bicubic | Markesteijn-3 + bicubic | Truth |
| --- | --- | --- | --- | --- | --- |
| Slanted-edge MTF50 (cycles per site) | **0.314** | 0.271 | 0.276 | 0.301 | 0.329 |
| Fine-detail RMSE against truth | **0.0152** | | 0.0352 | 0.0267 | 0 |
| Flat-patch luma noise (relative std) | **0.0017** | | 0.0073 | | |

The merge resolves a neutral edge 14% better than demosaicing and upscaling
the best single frame (16% better than the single frame through the same
kernels), recovers fine color detail with 2.3x lower error and has 4.3x lower
noise. Demosaicing a neutral edge is the single frame's best case; the gain
on colored and aliasing-prone detail is the RMSE column. Against
Markesteijn's demosaic the merge is 4% sharper and has 1.8x lower detail
error.

Merging red and blue independently instead gives edge MTF50 0.298 and Siemens
star RMSE 0.0083 (green-guided 0.0069). On the chart's random color cells,
whose channels are uncorrelated (the worst case for color differences), the
independent merge is better: RMSE 0.0182 against 0.0204.

- **Negative control:** eight identical frames merge to the single-frame
  result (largest pixel difference 1.8e-7; equal MTF50). Registration of
  identical frames returns the identity within 1e-3 sites.
- **Alignment:** recovered transforms are within 0.087 sites of the truth
  at the image corners over six frames.
- **Local alignment:** with every frame's true transform pushed 0.4 sites
  off (alternating sign), edge MTF50 falls from 0.315 to 0.265; the residual
  field measures the error back and the merge recovers 0.314.
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

**Uncompressed handheld burst**: 10 frames, 1/100 s, f/1.8, ISO 800, 56 mm,
electronic shutter, close-up of a printed box. The reader takes geometry
(plane 6336x4188, active 6240x4160 at 12,13), 14-bit data, black 1024, the
X-Trans layout and the as-shot white balance (Fuji directory 0xf00e) from
the files; same-color tile positions agree within 1%, confirming the CFA
phase.

- Alignment: 9 of 10 frames accepted (one fails the 8-of-16 coverage gate),
  shifts up to 39 pixels and 0.21° of roll against the reference; 90 s.
- Merge: 9 frames into 9360x6240 in 119 s, mean robustness weights 0.82 to
  0.97, peak strip memory 115.8 MiB, process maximum RSS 580 MB.
- Noise, flat patches with a fitted plane removed: luma relative standard
  deviation 0.0116 (merge) against 0.0256 (demosaic + upscale) and 0.0204
  against 0.0424 on a second patch, **2.1 to 2.2x lower**.
- Resolution: **not measurable on this burst.** At f/1.8 the depth of field
  is shallower than the hand's fore-aft sway, so focus moves between frames:
  a stroke that measures MTF50 0.047 cycles per site in one frame measures
  0.043 in another, and the sharpest single-frame edge found anywhere is
  near 0.1 cycles per site, a fifth of the sensor's 0.5 limit. The scene has
  no detail near the sampling limit for super-resolution to recover, and
  merging frames focused at different depths softens rather than sharpens.
  The printed strokes are also almost exactly vertical (under 0.5° of
  slant), which the slanted-edge method cannot sample. A resolution
  measurement needs a stopped-down (f/5.6 to f/8), distant, static target
  with an edge slanted 3 to 10° and shutter speeds of 1/250 s or faster.
- The exposure bracket shot with it (9 frames, 1/250 to 1/40 s) is measured
  under Exposure brackets below.

**Stopped-down burst**: 15 frames, 1/250 s, f/8, ISO 1600, 56 mm, handheld,
a tape-measure blade on a dark shirt. The plain shirt gives too few tiles for
the whole-frame coverage gate, so alignment is fitted on the blade
(`--align-region`); 13 of 15 frames register. On the blade's long edge
(-4.6°, scale ticks reaching within a few sites of it), each frame
demosaiced and upscaled measures MTF50 0.14 to 0.18 cycles per site:

| | Merge (13 frames) | Single frame, same merge | Demosaic + bicubic | Markesteijn-3 + bicubic |
| --- | --- | --- | --- | --- |
| Edge MTF50, luma (cycles per site) | 0.152 | 0.149 | 0.165 | 0.158 |
| Edge MTF50, green | 0.153 | 0.149 | 0.144 | 0.163 |
| Shirt std, R G B (plane removed) | 0.00050 0.00091 0.00075 | 0.00090 0.00117 0.00112 | 0.00098 0.00118 0.00113 | 0.00068 0.00125 0.00090 |

At the blade's other two edges (1500,2380 and 4700,2150) the merge measures
0.140 and 0.119 against Markesteijn's 0.152 and 0.130. On this real burst
the merge is 2% sharper than the same kernels on one frame but 3% to 9%
softer than Markesteijn's demosaic of the reference. The simple demosaic's
luma MTF50 (0.165) exceeds its own green's (0.144); its red and blue add
contrast at the edge that Markesteijn's do not. The shirt patch is not flat
(luma relative std 0.5 to 0.7 in every variant): its standard deviations
mostly measure weave, which the merge softens, not noise. Merging red and blue
independently measures 0.148 here; the green-guided luma now matches the
merged green (0.153), so the remaining gap is in green itself. Per-frame edge
positions after warping scatter by 0.1 to 0.45 sites and differ across the
frame, which one global transform per frame cannot correct.

With `--local`, the RMS of those per-frame edge offsets falls from 0.167 to
0.075 sites and from 0.254 to 0.161 at two of the three edges and rises from
0.099 to 0.138 at the third; merged MTF50 changes by +0.7%, +0.8% and -0.3%
(0.153, 0.120, 0.140). Registration is no longer what limits this burst; the
frames' own sharpness at the edge (0.14 to 0.18 per frame) varies more. Over
the whole frame the fields find 0.3 to 0.9 sites RMS of residual motion
(the global fit used only the blade) and double the merge time (212 s
against 102 s for 13 frames; peak strip memory 130 MiB either way), so
local alignment is off by default.

**Camera JPEGs** of earlier brackets and bursts registered with `align-jpeg`
show:

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

### Exposure brackets

Synthetic (`bracket_merge_extends_range`): the chart at exposures 1, 2.2 and
3.5, shortest as reference. Measured gains match the exposures within 2%.
In a dark flat patch the merge's noise is 0.000327 against 0.000318 for the
ideal inverse-variance combination of the three frames alone (0.000939,
0.000558, 0.000425), and its mean is within 0.4% of the truth. A bright
patch clipped in both long frames merges to 0.49999 against a truth of
0.5. Merged without exposure scaling (and without robustness, which
otherwise rejects the brighter frames as motion), the shadows come out too
bright.

Three X-H2S AE brackets of 9 frames each, in 1/3 EV steps over 2.67 EV,
handheld at 56 mm. Alignment measures gains of 0.155 to 0.395 against the
shortest exposure, matching the true third-stop shutter series (1/40 s:
6.38x, 1/50 s: 5.06x, 1/64 s: 4.03x and so on). The dark patch is flat
in reference coordinates and identical for every variant; each frame merged
alone is in its own units (white 1.0):

| Bracket | Frames | Merge | Best single frame | Gain |
| --- | --- | --- | --- | --- |
| DSCF0591-0599, f/1.8, ISO 800, 1/250-1/40 s | 9 of 9 | 12.18 stops | 10.61 (1/250 s) | +1.57 stops |
| DSCF0648-0656, f/5.6, ISO 800, 1/80-1/13 s | 8 of 9 | 12.25 stops | 10.87 (1/80 s) | +1.38 stops |
| DSCF0667-0675, f/5.6, ISO 12800, 1/640-1/100 s | 9 of 9 | 10.28 stops | 8.34 (1/640 s) | +1.94 stops |

The single frames' variances in the first bracket fit
`2.9e-8 + 3.8e-7 g`: a floor of 0.00017 common to all frames (patch texture
or fixed-pattern noise, which no merge removes) plus noise. Without that
floor the merge's noise is 0.000132 against an ideal of 0.000118, and the
gain over the shortest frame is 2.2 stops. Motion rejection and local
alignment leave the patch unchanged. In the second bracket the 1/13 s
frame fails alignment (motion blur). With it as reference (the sharpest by
gradient energy), 7 of the 8 other frames failed even with a subject
region; with the shortest as reference, 8 of 9 align.

None of these scenes clips: no site of any frame reaches the clip level, so
highlight recovery is shown only on the synthetic chart. A full-frame merge
of the ISO 12800 bracket takes 65 s with a peak strip memory of 92 MiB
(RSS 130 MB). The LCD digits of the remote, invisible in its 1/640 s
frame, are readable in the merge, with no visible ghosting. The 16-bit DNG
quantizes the merged shadows finely enough: the dark patch's noise is 14
counts.

## Limits and next steps

- Compressed RAFs (lossless and lossy) are refused.
- Frames focused at different depths, or blurred by motion, are merged as if
  equally sharp; a per-frame or per-region sharpness weight (shared with the
  focus stage) would keep the sharpest frames' detail. On the real
  stopped-down burst the merge is 3% to 9% softer than Markesteijn's
  demosaic of the best frame; this is the leading lever.
- The DNG color matrix is a placeholder.
- Local alignment is optional and translation-only per tile; robustness
  still compares frames through their global transforms.
- Coverage and inlier gates are caller policy; shallow depth-of-field bursts
  need a policy that accepts a well-supported subject region.
- The exposure stage merges brackets of single frames from CFA; merging
  bursts per exposure step first (an image stage) and the focus stage are
  declared in `plan.rs` and not built.
- The tested brackets span only 2.67 EV and never clip; a wider bracket of a
  scene with clipped highlights is the next real test.
