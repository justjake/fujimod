//! Coverage, paint and compositing of a UI batch in the tiled target layout.
//!
//! Declared unmeasured laws: regular 1/2/4 squared sample-grid winding
//! coverage, straight RGBA8 SRC/SRC_OVER, nearest image lookup with a
//! transparent border, linear ramp interpolation, atomic whole-batch output.
//! Fractional coverage and nonsolid paint retain the reference f64 order.
//! Full-coverage opaque solids use its exact RGBA8 byte result.

use super::batch::{ColorTransform, Draw, Gradient, Texture};
use super::geometry::{self, Point};
use fujimod_emulator_devices::record::{Result, Unsupported};
use std::collections::HashMap;

fn fail<T>(message: &str) -> Result<T> {
    Err(Unsupported(message.into()))
}

/// Per-pixel coverage of one draw: `width` x `height` pixels at (x0, y0).
#[derive(Clone, Debug, PartialEq)]
pub struct Coverage {
    pub x0: i64,
    pub y0: i64,
    pub width: usize,
    pub height: usize,
    pub values: Vec<f64>,
}

impl Coverage {
    fn empty() -> Self {
        Coverage {
            x0: 0,
            y0: 0,
            width: 0,
            height: 0,
            values: Vec::new(),
        }
    }
}

/// Inside-sample counts per pixel (row-major), the sample-grid scanline
/// fill: `count / scale**2` is the reference raster's mean.
pub fn fill_counts(
    edges: &[(Point, Point)],
    nonzero: bool,
    x0: i64,
    y0: i64,
    scale: i64,
    width: usize,
    height: usize,
) -> Result<Vec<u8>> {
    let s = scale as usize;
    let (sample_width, sample_height) = (width * s, height * s);
    let (x0f, y0f, scalef) = (x0 as f64, y0 as f64, scale as f64);
    let mut counts = vec![0u8; width * height];
    let mut crossings: Vec<(f64, i64)> = Vec::with_capacity(edges.len());
    let inside = |level: i64| if nonzero { level != 0 } else { level & 1 != 0 };
    for row in 0..sample_height {
        let y = y0f + (row as f64 + 0.5) / scalef;
        crossings.clear();
        for &((ax, ay), (bx, by)) in edges {
            if ay.min(by) <= y && y < ay.max(by) {
                crossings.push((
                    ax + (y - ay) * (bx - ax) / (by - ay),
                    if by > ay { 1 } else { -1 },
                ));
            }
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
        let (mut level, mut previous) = (0i64, None::<f64>);
        let pixel_row = (row / s) * width;
        for &(x, direction) in &crossings {
            if let Some(previous) = previous
                && inside(level)
            {
                let left = ((previous - x0f) * scalef - 0.5).ceil().max(0.0);
                let right = ((x - x0f) * scalef - 0.5).ceil().min(sample_width as f64);
                if left < right {
                    let (left, right) = (left as usize, right as usize);
                    // Divide only at the span edges. Interior pixels gain
                    // one complete sample row; the slice loop vectorizes.
                    let first = left / s;
                    let last = (right - 1) / s;
                    let pixels = &mut counts[pixel_row..pixel_row + width];
                    if first == last {
                        pixels[first] += (right - left) as u8;
                    } else {
                        pixels[first] += (s - left % s) as u8;
                        for count in &mut pixels[first + 1..last] {
                            *count += s as u8;
                        }
                        pixels[last] += ((right - 1) % s + 1) as u8;
                    }
                }
            }
            level = if nonzero {
                level + direction
            } else {
                level + 1
            };
            previous = Some(x);
        }
        if inside(level) {
            return fail("Graphics UI emitted fill edges have an unbalanced scanline");
        }
    }
    Ok(counts)
}

/// `_ui_coverage(_ui_paths(draw), draw)`.
pub fn coverage(draw: &Draw) -> Result<Coverage> {
    let plan = coverage_plan(draw)?;
    if plan.is_empty() {
        return coverage_from_counts(plan, Vec::new());
    }
    let counts = fill_counts(
        &plan.edges,
        plan.nonzero,
        plan.x0,
        plan.y0,
        plan.samples as i64,
        plan.width,
        plan.height,
    )?;
    coverage_from_counts(plan, counts.into_iter().map(u32::from).collect())
}

/// Geometry and clip data for one coverage raster. The edge coordinates stay
/// f64 here so CPU rendering retains its reference arithmetic; GPU callers
/// convert these edges to the shared kernel's f32 input.
pub struct CoveragePlan {
    pub edges: Vec<(Point, Point)>,
    pub nonzero: bool,
    pub samples: u32,
    pub x0: i64,
    pub y0: i64,
    pub width: usize,
    pub height: usize,
    clipped: Vec<bool>,
}

impl CoveragePlan {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Per-pixel clip inclusion in row-major bounds order.
    pub fn clip_mask(&self) -> &[bool] {
        &self.clipped
    }

    /// Pixel bounds as `[left, right, top, bottom)`, with exclusive end values.
    pub fn bounds(&self) -> [u32; 4] {
        if self.is_empty() {
            return [0; 4];
        }
        [
            self.x0 as u32,
            (self.x0 + self.width as i64) as u32,
            self.y0 as u32,
            (self.y0 + self.height as i64) as u32,
        ]
    }
}

fn empty_coverage_plan(samples: u32, nonzero: bool) -> CoveragePlan {
    CoveragePlan {
        edges: Vec::new(),
        nonzero,
        samples,
        x0: 0,
        y0: 0,
        width: 0,
        height: 0,
        clipped: Vec::new(),
    }
}

/// Prepare fill edges, sample bounds and clipping for one draw.
pub fn coverage_plan(draw: &Draw) -> Result<CoveragePlan> {
    coverage_plan_from_paths(geometry::paths(draw)?, &CoverageStyle::from(draw))
}

/// Prepare coverage plans in draw order for a single batched raster dispatch.
pub fn coverage_plans(draws: &[Draw]) -> Result<Vec<CoveragePlan>> {
    draws.iter().map(coverage_plan).collect()
}

/// Apply rasterized sample counts to a prepared draw, including its clip mask.
pub fn coverage_from_counts(plan: CoveragePlan, counts: Vec<u32>) -> Result<Coverage> {
    if plan.is_empty() {
        if !counts.is_empty() {
            return fail("Graphics UI empty coverage plan received rasterized samples");
        }
        return Ok(Coverage::empty());
    }
    let expected = plan.width * plan.height;
    if counts.len() != expected || plan.clipped.len() != expected {
        return fail("Graphics UI coverage raster returned an incomplete sample image");
    }
    let maximum = plan.samples * plan.samples;
    if counts.iter().any(|&count| count > maximum) {
        return fail("Graphics UI coverage raster returned an invalid sample count");
    }
    let divisor = maximum as f64;
    let values = counts
        .iter()
        .zip(&plan.clipped)
        .map(|(&count, &inside)| {
            let value = count as f64 / divisor;
            value * if inside { 1.0 } else { 0.0 }
        })
        .collect();
    Ok(Coverage {
        x0: plan.x0,
        y0: plan.y0,
        width: plan.width,
        height: plan.height,
        values,
    })
}

/// Only the fields the pure coverage operation reads. Runtimes can provide
/// fresh paths/style without constructing a command Draw with invented paint.
pub struct CoverageStyle {
    pub width: u32,
    pub height: u32,
    pub matrix: super::batch::Affine,
    pub stroke: Option<geometry::StrokeStyle>,
    pub nonzero: bool,
    pub samples: u32,
    pub clips: Vec<(i64, i64, i64, i64)>,
}

impl From<&Draw> for CoverageStyle {
    fn from(draw: &Draw) -> Self {
        Self {
            width: draw.target.1,
            height: draw.target.2,
            matrix: draw.matrix,
            stroke: draw.stroke.then(|| geometry::StrokeStyle::from(draw)),
            nonzero: draw.nonzero,
            samples: draw.samples,
            clips: draw.clips.clone(),
        }
    }
}

fn coverage_plan_from_paths(
    paths: Vec<(Vec<Point>, bool)>,
    draw: &CoverageStyle,
) -> Result<CoveragePlan> {
    let (width, height) = (draw.width as i64, draw.height as i64);
    let [a, b, _, d, e, _] = draw.matrix;
    if draw.stroke.is_none() && a * e - b * d == 0.0 {
        // A singular fill collapses its geometry to a line or point.
        return Ok(empty_coverage_plan(
            draw.samples,
            draw.stroke.is_some() || draw.nonzero,
        ));
    }
    let polylines: Vec<Vec<Point>> = if let Some(style) = &draw.stroke {
        geometry::stroke_with_style(&paths, style)?
    } else {
        paths.into_iter().map(|(points, _)| points).collect()
    };
    // Stock fill construction emits its own close edges.
    let edges: Vec<(Point, Point)> = polylines
        .iter()
        .flat_map(|points| points.windows(2).map(|w| (w[0], w[1])))
        .filter(|(a, b)| a != b && a.1 != b.1)
        .collect();
    if edges.is_empty() {
        return Ok(empty_coverage_plan(
            draw.samples,
            draw.stroke.is_some() || draw.nonzero,
        ));
    }
    let nonzero = draw.stroke.is_some() || draw.nonzero;
    let mut sweep: Vec<(f64, i64, i64)> = Vec::with_capacity(edges.len() * 2);
    for &(a, b) in &edges {
        let direction = if b.1 > a.1 { 1 } else { -1 };
        sweep.push((a.1.min(b.1), 1, direction));
        sweep.push((a.1.max(b.1), -1, -direction));
    }
    sweep.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    let (mut count, mut winding) = (0i64, 0i64);
    let mut index = 0;
    while index < sweep.len() {
        let y = sweep[index].0;
        while index < sweep.len() && sweep[index].0 == y {
            count += sweep[index].1;
            winding += sweep[index].2;
            index += 1;
        }
        if if nonzero {
            winding != 0
        } else {
            count & 1 != 0
        } {
            return fail("Graphics UI emitted fill edges have an unbalanced scanline");
        }
    }
    if draw.clips.is_empty() {
        return Ok(empty_coverage_plan(draw.samples, nonzero));
    }
    let points = edges.iter().flat_map(|(a, b)| [*a, *b]);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::NAN, f64::NAN, f64::NAN, f64::NAN);
    for (i, p) in points.enumerate() {
        if i == 0 {
            (min_x, min_y, max_x, max_y) = (p.0, p.1, p.0, p.1);
        } else {
            // Python min/max keep the first of equal values.
            if p.0 < min_x {
                min_x = p.0
            }
            if p.1 < min_y {
                min_y = p.1
            }
            if p.0 > max_x {
                max_x = p.0
            }
            if p.1 > max_y {
                max_y = p.1
            }
        }
    }
    let x0 = 0i64.max(min_x.floor() as i64);
    let y0 = 0i64.max(min_y.floor() as i64);
    let x1 = width.min(max_x.ceil() as i64);
    let y1 = height.min(max_y.ceil() as i64);
    if x0 >= x1 || y0 >= y1 {
        return Ok(empty_coverage_plan(draw.samples, nonzero));
    }
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut clipped = vec![false; w * h];
    for &(left, bottom, right, top) in &draw.clips {
        let (left, bottom, right, top) = (left.max(x0), bottom.max(y0), right.min(x1), top.min(y1));
        if left < right && bottom < top {
            for row in (bottom - y0) as usize..(top - y0) as usize {
                clipped[row * w + (left - x0) as usize..row * w + (right - x0) as usize].fill(true);
            }
        }
    }
    Ok(CoveragePlan {
        edges,
        nonzero,
        samples: draw.samples,
        x0,
        y0,
        width: w,
        height: h,
        clipped,
    })
}

/// The pixel index of (x, y) in an 8x8 Morton-tiled image (`_ui_morton`).
pub fn morton(width: usize, x: usize, y: usize) -> usize {
    let local = (x & 1) | (y & 1) << 1 | (x & 2) << 1 | (y & 2) << 2 | (x & 4) << 2 | (y & 4) << 3;
    ((y >> 3) * (width >> 3) + (x >> 3)) * 64 + local
}

/// NumPy `remainder` for floats (`npy_divmod`'s modulus).
fn remainder(a: f64, b: f64) -> f64 {
    let modulus = a % b; // C fmod
    if modulus != 0.0 {
        if (b < 0.0) != (modulus < 0.0) {
            modulus + b
        } else {
            modulus
        }
    } else {
        0.0f64.copysign(b)
    }
}

/// NumPy `clip(value, low, high)` = minimum(maximum(value, low), high).
fn clip(value: f64, low: f64, high: f64) -> f64 {
    let v = if value.is_nan() || value >= low {
        value
    } else {
        low
    };
    if v.is_nan() || v <= high { v } else { high }
}

/// Per-pixel linear-ramp colors over the coverage rectangle.
fn transform_color(rgba: [f64; 4], transform: Option<&ColorTransform>) -> [f64; 4] {
    match transform {
        None => rgba,
        Some(transform) => [
            (rgba[0] * transform.scale[0] + transform.bias[0]).clamp(0.0, 1.0),
            (rgba[1] * transform.scale[1] + transform.bias[1]).clamp(0.0, 1.0),
            (rgba[2] * transform.scale[2] + transform.bias[2]).clamp(0.0, 1.0),
            (rgba[3] * transform.scale[3] + transform.bias[3]).clamp(0.0, 1.0),
        ],
    }
}

fn gradient_table(gradient: &Gradient) -> Vec<[f64; 4]> {
    gradient
        .colors
        .iter()
        .map(|rgba| rgba.map(|value| value as f64 / 255.0))
        .collect()
}

#[inline]
fn gradient_color(
    gradient: &Gradient,
    table: &[[f64; 4]],
    x: f64,
    y: f64,
    transform: Option<&ColorTransform>,
) -> [f64; 4] {
    let (a, c, e, f) = gradient.coefficients;
    let mut amount = if let Some(radial) = &gradient.radial {
        // B7F000 encodes focus-relative coordinates divided by
        // R²-|F-C|², with zero-axis terms kept for the quadratic.
        // This solves the ray/circle intersection in that encoding.
        let u = a * x + c;
        let v = e * y + f;
        let cross = if radial.zero_x || radial.zero_y {
            0.0
        } else {
            2.0 * u * v
        };
        let root = (radial.quadratics[0] * u * u + radial.quadratics[1] * v * v + cross)
            .max(0.0)
            .sqrt();
        let linear = (if radial.zero_x { 0.0 } else { u }) + (if radial.zero_y { 0.0 } else { v });
        if radial.negative_denominator {
            linear - root
        } else {
            linear + root
        }
    } else {
        a * x + c + e * y + f
    };
    amount = match gradient.spread {
        0 => clip(amount, 0.0, 1.0),
        1 => remainder(amount, 1.0),
        _ => 1.0 - (remainder(amount, 2.0) - 1.0).abs(),
    };
    let position = clip(amount * 127.0 + 0.5, 0.0, 127.0);
    let low = position.floor() as i64;
    let fraction = position - low as f64;
    let (t0, t1) = (table[low as usize], table[(low + 1).min(127) as usize]);
    let mut source = [0.0f64; 4];
    for k in 0..4 {
        source[k] = t0[k] * (1.0 - fraction) + t1[k] * fraction;
    }
    if gradient.premultiplied {
        let alpha = source[3];
        for value in &mut source[..3] {
            *value = if alpha != 0.0 { *value / alpha } else { 0.0 };
        }
    }
    transform_color(source, transform)
}

#[inline]
fn texture_color(
    texture: &Texture,
    image: &[[u8; 4]],
    x: f64,
    y: f64,
    transform: Option<&ColorTransform>,
) -> [f64; 4] {
    let [a, b, c, d, e, f] = texture.matrix;
    let u = (a * x + b * y + c).floor() as i64;
    let v = (d * x + e * y + f).floor() as i64;
    let valid = u >= 0 && v >= 0 && u < texture.valid_width && v < texture.valid_height;
    let at = (v.clamp(0, texture.height as i64 - 1) * texture.width as i64
        + u.clamp(0, texture.width as i64 - 1)) as usize;
    let texel = image[at];
    let source = transform_color(
        [
            texel[0] as f64 / 255.0,
            texel[1] as f64 / 255.0,
            texel[2] as f64 / 255.0,
            texel[3] as f64 / 255.0,
        ],
        transform,
    );
    let scale = if valid { 1.0 } else { 0.0 };
    [
        source[0] * scale,
        source[1] * scale,
        source[2] * scale,
        source[3] * scale,
    ]
}

fn gradient_colors(
    gradient: &Gradient,
    cover: &Coverage,
    transform: Option<&ColorTransform>,
) -> Vec<[f64; 4]> {
    let table = gradient_table(gradient);
    let mut colors = Vec::with_capacity(cover.values.len());
    for row in 0..cover.height {
        for column in 0..cover.width {
            let xx = column as f64;
            let yy = row as f64;
            let x = xx + cover.x0 as f64 + 0.5;
            let y = yy + cover.y0 as f64 + 0.5;
            colors.push(gradient_color(gradient, &table, x, y, transform));
        }
    }
    colors
}

fn texture_colors(
    texture: &Texture,
    image: &[[u8; 4]],
    cover: &Coverage,
    transform: Option<&ColorTransform>,
) -> Vec<[f64; 4]> {
    let (cx, cy) = (cover.x0 as f64 + 0.5, cover.y0 as f64 + 0.5);
    let mut colors = vec![[0.0; 4]; cover.values.len()];
    for row in 0..cover.height {
        let yy = row as f64 + cy;
        for column in 0..cover.width {
            let index = row * cover.width + column;
            if cover.values[index] == 0.0 {
                continue;
            }
            let xx = column as f64 + cx;
            colors[index] = texture_color(texture, image, xx, yy, transform);
        }
    }
    colors
}

#[inline]
fn blend_channel(
    source: f64,
    destination: f64,
    alpha: f64,
    destination_alpha: f64,
    retained: f64,
    out_alpha: f64,
) -> f64 {
    let numerator = source * alpha + destination * destination_alpha * retained;
    if out_alpha != 0.0 {
        numerator / out_alpha
    } else {
        0.0
    }
}

#[allow(clippy::manual_clamp)]
#[inline]
fn byte(value: f64) -> u8 {
    (value.max(0.0).min(1.0) * 255.0 + 0.5).floor() as u8
}

/// One pixel of the reference blend, in place (straight RGBA8).
#[inline]
fn blend_pixel(pixel: &mut [u8; 4], c: f64, [sr, sg, sb, sa]: [f64; 4], blend: bool) {
    let c = (c * 255.).floor() / 255.;
    let alpha = sa * c;
    if c == 0.0 || (blend && alpha == 0.0) {
        return;
    }
    let (dr, dg, db, da) = (
        pixel[0] as f64 / 255.0,
        pixel[1] as f64 / 255.0,
        pixel[2] as f64 / 255.0,
        pixel[3] as f64 / 255.0,
    );
    let retained = if blend { 1.0 - alpha } else { 1.0 - c };
    let out_alpha = alpha + da * retained;
    *pixel = [
        byte(blend_channel(sr, dr, alpha, da, retained, out_alpha)),
        byte(blend_channel(sg, dg, alpha, da, retained, out_alpha)),
        byte(blend_channel(sb, db, alpha, da, retained, out_alpha)),
        byte(out_alpha),
    ];
}

/// A target image kept in its tiled memory layout.
pub struct Target {
    pub address: u64,
    pub bytes: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub order: u32,
}

// Morton columns within one 8x8 tile, in words.
const TILE_COLUMNS: [usize; 8] = [0, 1, 4, 5, 16, 17, 20, 21];

fn composite_span(
    tile: &mut [u8],
    local_y: usize,
    coverage: &[f64],
    local_xs: &[usize],
    source: impl Iterator<Item = [f64; 4]>,
    channels: [usize; 4],
    blend: bool,
) {
    for ((&c, &local_x), source) in coverage.iter().zip(local_xs).zip(source) {
        if c == 0.0 || (blend && source[3] * c == 0.0) {
            continue;
        }
        blend_target_pixel(tile, local_y, local_x, c, source, channels, blend);
    }
}

#[inline]
fn blend_target_pixel(
    tile: &mut [u8],
    local_y: usize,
    local_x: usize,
    coverage: f64,
    source: [f64; 4],
    channels: [usize; 4],
    blend: bool,
) {
    let at = local_y + local_x * 4;
    let bytes: &mut [u8; 4] = (&mut tile[at..at + 4]).try_into().unwrap();
    let mut pixel = [
        bytes[channels[0]],
        bytes[channels[1]],
        bytes[channels[2]],
        bytes[channels[3]],
    ];
    blend_pixel(&mut pixel, coverage, source, blend);
    bytes[channels[0]] = pixel[0];
    bytes[channels[1]] = pixel[1];
    bytes[channels[2]] = pixel[2];
    bytes[channels[3]] = pixel[3];
}

fn composite_solid(
    target: &mut Target,
    cover: &Coverage,
    color: [f64; 4],
    blend: bool,
    opaque_solid: Option<[u8; 4]>,
) {
    let channels: [usize; 4] = if target.order == 0 {
        [3, 2, 1, 0]
    } else {
        [2, 1, 0, 3]
    };
    if let Some(rgba) = opaque_solid {
        let packed = if target.order == 0 {
            [rgba[3], rgba[2], rgba[1], rgba[0]]
        } else {
            [rgba[2], rgba[1], rgba[0], rgba[3]]
        };
        let pair = [
            packed[0], packed[1], packed[2], packed[3], packed[0], packed[1], packed[2], packed[3],
        ];
        for (row, coverage) in cover.values.chunks_exact(cover.width).enumerate() {
            let y = cover.y0 as usize + row;
            let tile_row = (y >> 3) * (target.width >> 3) * 256;
            let local_y = ((y & 1) << 1 | (y & 2) << 2 | (y & 4) << 3) * 4;
            let mut column = 0;
            while column < cover.width {
                let x = cover.x0 as usize + column;
                let lane = x & 7;
                let span = (8 - lane).min(cover.width - column);
                let base = tile_row + (x >> 3) * 256;
                let tile = &mut target.bytes[base..base + 256];
                let mut offset = 0;
                while offset < span {
                    let c = coverage[column + offset];
                    let local_x = TILE_COLUMNS[lane + offset];
                    if (lane + offset) & 1 == 0
                        && offset + 1 < span
                        && c == 1.0
                        && coverage[column + offset + 1] == 1.0
                    {
                        let at = local_y + local_x * 4;
                        tile[at..at + 8].copy_from_slice(&pair);
                        offset += 2;
                        continue;
                    }
                    if c != 0.0 {
                        let at = local_y + local_x * 4;
                        if c == 1.0 {
                            tile[at..at + 4].copy_from_slice(&packed);
                        } else {
                            blend_target_pixel(tile, local_y, local_x, c, color, channels, blend);
                        }
                    }
                    offset += 1;
                }
                column += span;
            }
        }
        return;
    }

    for (row, coverage) in cover.values.chunks_exact(cover.width).enumerate() {
        let y = cover.y0 as usize + row;
        let tile_row = (y >> 3) * (target.width >> 3) * 256;
        let local_y = ((y & 1) << 1 | (y & 2) << 2 | (y & 4) << 3) * 4;
        let mut column = 0;
        while column < cover.width {
            let x = cover.x0 as usize + column;
            let lane = x & 7;
            let span = (8 - lane).min(cover.width - column);
            let base = tile_row + (x >> 3) * 256;
            let tile = &mut target.bytes[base..base + 256];
            composite_span(
                tile,
                local_y,
                &coverage[column..column + span],
                &TILE_COLUMNS[lane..lane + span],
                std::iter::repeat_n(color, span),
                channels,
                blend,
            );
            column += span;
        }
    }
}

fn composite_pixels(target: &mut Target, cover: &Coverage, colors: &[[f64; 4]], blend: bool) {
    let channels: [usize; 4] = if target.order == 0 {
        [3, 2, 1, 0]
    } else {
        [2, 1, 0, 3]
    };
    for (row, coverage) in cover.values.chunks_exact(cover.width).enumerate() {
        let y = cover.y0 as usize + row;
        let tile_row = (y >> 3) * (target.width >> 3) * 256;
        let local_y = ((y & 1) << 1 | (y & 2) << 2 | (y & 4) << 3) * 4;
        let source_row = &colors[row * cover.width..(row + 1) * cover.width];
        let mut column = 0;
        while column < cover.width {
            let x = cover.x0 as usize + column;
            let lane = x & 7;
            let span = (8 - lane).min(cover.width - column);
            let base = tile_row + (x >> 3) * 256;
            let tile = &mut target.bytes[base..base + 256];
            composite_span(
                tile,
                local_y,
                &coverage[column..column + span],
                &TILE_COLUMNS[lane..lane + span],
                source_row[column..column + span].iter().copied(),
                channels,
                blend,
            );
            column += span;
        }
    }
}

/// Render a validated batch in list order. `targets` holds each target's
/// current bytes by physical address, `textures` each sampled image's by
/// (address, width, height). Returns the new target bytes in first-use
/// order and the covered byte count (`touched`).
pub type RenderOutput = (Vec<(u64, Vec<u8>)>, u64);

pub fn render(
    draws: &[Draw],
    targets_in: &HashMap<u64, Vec<u8>>,
    textures_in: &HashMap<(u64, u32, u32), Vec<u8>>,
    coverage_of: &mut dyn FnMut(&Draw) -> Result<std::rc::Rc<Coverage>>,
) -> Result<RenderOutput> {
    let mut targets: Vec<Target> = Vec::new();
    let mut textures: HashMap<(u64, u32, u32), Vec<[u8; 4]>> = HashMap::new();
    for draw in draws {
        let (address, width, height, order) = draw.target;
        if !targets.iter().any(|t| t.address == address) {
            let Some(bytes) = targets_in.get(&address) else {
                return fail("Graphics UI destination/texture snapshot is incomplete");
            };
            targets.push(Target {
                address,
                bytes: bytes.clone(),
                width: width as usize,
                height: height as usize,
                order,
            });
        }
        if let Some(texture) = &draw.texture {
            let key = (texture.source, texture.width, texture.height);
            if let std::collections::hash_map::Entry::Vacant(entry) = textures.entry(key) {
                let Some(payload) = textures_in.get(&key) else {
                    return fail("Graphics UI destination/texture snapshot is incomplete");
                };
                let (w, h) = (texture.width as usize, texture.height as usize);
                let mut pixels = Vec::with_capacity(w * h);
                for y in 0..h {
                    for x in 0..w {
                        let at = morton(w, x, y) * 4;
                        pixels.push([
                            payload[at + 3],
                            payload[at + 2],
                            payload[at + 1],
                            payload[at],
                        ]);
                    }
                }
                entry.insert(pixels);
            }
        }
    }
    let mut touched = 0u64;
    for draw in draws {
        let cover = coverage_of(draw)?;
        if cover.values.is_empty() || cover.values.iter().all(|&v| v == 0.0) {
            continue;
        }
        let target = targets
            .iter_mut()
            .find(|t| t.address == draw.target.0)
            .unwrap();
        let color_transform = draw.color_transform.as_ref();
        let color = transform_color(
            draw.color.map(|value| value as f64 / 255.0),
            color_transform,
        );
        if let Some(gradient) = &draw.gradient {
            let colors = gradient_colors(gradient, &cover, color_transform);
            composite_pixels(target, &cover, &colors, draw.blend != 0);
        } else if let Some(texture) = &draw.texture {
            let image = &textures[&(texture.source, texture.width, texture.height)];
            let colors = texture_colors(texture, image, &cover, color_transform);
            composite_pixels(target, &cover, &colors, draw.blend != 0);
        } else {
            if draw.blend == 0 || color[3] != 0.0 {
                composite_solid(
                    target,
                    &cover,
                    color,
                    draw.blend != 0,
                    (draw.color[3] == 255 && draw.color_transform.is_none()).then_some(draw.color),
                );
            }
        }
        touched += cover.values.iter().filter(|&&v| v != 0.0).count() as u64 * 4;
    }
    Ok((
        targets.into_iter().map(|t| (t.address, t.bytes)).collect(),
        touched,
    ))
}

/// The hardware copy rearranges Morton 8x8 tiles into bottom-up rows and keeps
/// each word's bytes unchanged. It does not rotate channels.
pub fn copy_untile(input: &[u8], width: usize, height: usize) -> Vec<u8> {
    if width == 0 {
        return Vec::new();
    }
    if !width.is_multiple_of(8) || !height.is_multiple_of(8) {
        // The public helper historically accepts arbitrary dimensions;
        // modeled copy commands use complete 8x8 tiles.
        let mut output = Vec::with_capacity(width * height * 4);
        for row in 0..height {
            for x in 0..width {
                let at = morton(width, x, height - 1 - row) * 4;
                output.extend_from_slice(&input[at..at + 4]);
            }
        }
        return output;
    }
    let mut output = vec![0; width * height * 4];
    let (words, _) = input.as_chunks::<4>();
    for (row, output_row) in output.chunks_exact_mut(width * 4).enumerate() {
        let y = height - 1 - row;
        let base = (y >> 3) * (width >> 3) * 64;
        let local_y = (y & 1) << 1 | (y & 2) << 2 | (y & 4) << 3;
        for (tile_x, span) in output_row.as_chunks_mut::<32>().0.iter_mut().enumerate() {
            let tile = &words[base + tile_x * 64..base + (tile_x + 1) * 64];
            for (destination, &local_x) in span.as_chunks_mut::<4>().0.iter_mut().zip(&TILE_COLUMNS)
            {
                *destination = tile[local_y + local_x];
            }
        }
    }
    output
}

#[cfg(test)]
mod color_transform_tests {
    use super::*;
    use crate::batch::{ColorTransform, color_transform};
    use crate::regs::Registers;

    fn halfwords(values: [i16; 8]) -> Registers {
        let mut registers = Registers::new();
        for index in 0..4 {
            let (low, high) = (values[2 * index] as u16, values[2 * index + 1] as u16);
            registers.set(0x180 + index as u32, u64::from(low) | u64::from(high) << 16);
        }
        registers
    }

    fn solid(color: [u8; 4], color_transform: Option<ColorTransform>) -> Draw {
        Draw {
            target: (0x1000, 8, 8, 1),
            sequence: 0,
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            geometry: Vec::new(),
            samples: 1,
            clips: vec![(0, 0, 8, 8)],
            nonzero: true,
            blend: 0,
            color,
            stroke: false,
            texture: None,
            gradient: None,
            color_transform,
            radius: 0.0,
            cap: 0,
            join: 0,
            device_radius: false,
        }
    }

    fn render_solid(draw: Draw) -> [u8; 4] {
        let cover = std::rc::Rc::new(Coverage {
            x0: 0,
            y0: 0,
            width: 8,
            height: 8,
            values: vec![1.0; 64],
        });
        let targets = HashMap::from([(0x1000u64, vec![0u8; 256])]);
        let (output, _) = render(&[draw], &targets, &HashMap::new(), &mut |_| {
            Ok(cover.clone())
        })
        .unwrap();
        // Order 1 stores R, G, B, A in bytes 2, 1, 0, 3.
        let bytes = &output[0].1;
        [bytes[2], bytes[1], bytes[0], bytes[3]]
    }

    #[test]
    fn identity_halfwords_decode_to_no_transform() {
        // B7F000 encodes scale 1.0 as 0x80 and bias 0 as 0.
        let registers = halfwords([128, 128, 128, 128, 0, 0, 0, 0]);
        assert_eq!(color_transform(&registers).unwrap(), None);
        assert!(color_transform(&Registers::new()).is_err());
    }

    #[test]
    fn signed_halfwords_decode_as_scales_and_byte_biases() {
        // Scales 0.5, 2, -1, 1; biases 0.25, -0.5, 1, 0 (written as value * 255 * 128).
        let registers = halfwords([64, 256, -128, 128, 8160, -16320, 32640, 0]);
        let transform = color_transform(&registers).unwrap().unwrap();
        assert_eq!(transform.scale, [0.5, 2.0, -1.0, 1.0]);
        assert_eq!(transform.bias, [0.25, -0.5, 1.0, 0.0]);
    }

    #[test]
    fn solid_paint_is_scaled_biased_and_clamped_per_channel() {
        let transform =
            color_transform(&halfwords([64, 256, -128, 64, 8160, -16320, 32640, 0])).unwrap();
        assert_eq!(
            render_solid(solid([200, 200, 100, 255], None)),
            [200, 200, 100, 255]
        );
        // R 200*0.5+63.75, G 200*2-127.5 clamps to 255, B -100+255, A 255*0.5.
        assert_eq!(
            render_solid(solid([200, 200, 100, 255], transform)),
            [164, 255, 155, 128]
        );
    }
}
