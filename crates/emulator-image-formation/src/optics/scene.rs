use fujimod_emulator_imaging::scene::{Frame, FramePixels, mosaic::MappedSites};
use std::{borrow::Cow, sync::Arc};

/// Immutable scene irradiance. Row prefixes are independent of the lens,
/// exposure and readout window; each scene is linearized once per projection.
/// Linear values are 16-bit: sRGB frames use the sRGB curve, and mosaic frames
/// are normalized to the frame's largest site radiance. The projection's
/// response scale restores the exposure, so one scene serves every exposure.
pub(super) struct Scene {
    /// Identity of the scene pixels, shared with every binned derivative.
    pub frame: Arc<Frame>,
    pub dimensions: [usize; 2],
    prefix: Vec<[u32; 3]>,
    /// Linear mosaic sites, for a full-resolution mosaic scene.
    sites: Option<MappedSites>,
}
impl Scene {
    /// Scene irradiance of an RGB or mosaic frame. RAW samples have no scene
    /// radiance and are never projected.
    pub fn new(frame: Arc<Frame>) -> Result<Self, String> {
        let dimensions = [frame.width, frame.height];
        let mut sites = None;
        let prefix = match &frame.pixels {
            FramePixels::Rgb24(bytes) => {
                let linear: [u16; 256] = std::array::from_fn(|i| {
                    let v = i as f64 / 255.;
                    let v = if v <= 0.04045 {
                        v / 12.92
                    } else {
                        libm::pow((v + 0.055) / 1.055, 2.4)
                    };
                    libm::rint(v * 65535.) as u16
                });
                let [width, _] = dimensions;
                prefix_sums(dimensions, |x, y| {
                    let at = (y * width + x) * 3;
                    std::array::from_fn(|c| linear[bytes[at + c] as usize])
                })
            }
            FramePixels::Mosaic { .. } => {
                let mosaic = frame.mosaic().ok_or("mosaic scene has no site radiance")?;
                let scale = if mosaic.max > 0. {
                    65535. / f64::from(mosaic.max)
                } else {
                    0.
                };
                sites = Some(
                    mosaic.mapped(|v| libm::rint(f64::from(v) * scale).clamp(0., 65535.) as u16),
                );
                Vec::new()
            }
            FramePixels::Raw16(_) => {
                return Err("optical scenes need RGB or mosaic radiance".into());
            }
        };
        Ok(Self {
            frame,
            dimensions,
            prefix,
            sites,
        })
    }
    /// Integrate linear scene cells over each readout pixel before optical
    /// filtering. Integer overlap weights preserve a flat field exactly.
    pub fn binned(&self, dimensions: [usize; 2]) -> Self {
        let [width, height] = dimensions;
        let [sw, sh] = self.dimensions;
        // Overlap weights of each readout column, shared by every row.
        struct Column {
            first: usize,
            last: usize,
            first_weight: u64,
            last_weight: u64,
        }
        let columns: Vec<_> = (0..width)
            .map(|x| {
                let left = x * sw;
                let right = (x + 1) * sw;
                let first = left / width;
                let last = (right - 1) / width;
                Column {
                    first,
                    last,
                    first_weight: ((first + 1) * width - left) as u64,
                    last_weight: (right - last * width) as u64,
                }
            })
            .collect();
        let mut prefix = vec![[0u32; 3]; (width + 1) * height];
        let mut row_prefix = Vec::new();
        // Horizontal integrals of one source row, reused by each readout row
        // the source row overlaps.
        let mut horizontal = vec![[0u64; 3]; width];
        let mut horizontal_y = None;
        let mut colors = vec![[0u64; 3]; width];
        let area = (sw * sh) as u64;
        for (y, out) in prefix.chunks_exact_mut(width + 1).enumerate() {
            let top = y * sh;
            let bottom = (y + 1) * sh;
            colors.fill([0; 3]);
            for sy in top / height..bottom.div_ceil(height) {
                if horizontal_y != Some(sy) {
                    let row = self.row_prefix(sy, &mut row_prefix);
                    for (sums, column) in horizontal.iter_mut().zip(&columns) {
                        let (first, last) = (column.first, column.last);
                        for c in 0..3 {
                            sums[c] = if first == last {
                                u64::from(row[first + 1][c] - row[first][c]) * sw as u64
                            } else {
                                let a = u64::from(row[first + 1][c] - row[first][c]);
                                let b = u64::from(row[last + 1][c] - row[last][c]);
                                a * column.first_weight
                                    + b * column.last_weight
                                    + u64::from(row[last][c] - row[first + 1][c]) * width as u64
                            };
                        }
                    }
                    horizontal_y = Some(sy);
                }
                let wy = (bottom.min((sy + 1) * height) - top.max(sy * height)) as u64;
                for (color, sums) in colors.iter_mut().zip(&horizontal) {
                    for c in 0..3 {
                        color[c] += sums[c] * wy;
                    }
                }
            }
            let mut sum = [0u32; 3];
            for (out, color) in out[1..].iter_mut().zip(&colors) {
                for c in 0..3 {
                    sum[c] += ((color[c] + area / 2) / area) as u32;
                }
                *out = sum;
            }
        }
        Self {
            frame: self.frame.clone(),
            dimensions,
            prefix,
            sites: None,
        }
    }
    #[inline(always)]
    pub fn linear(&self, x: usize, y: usize) -> [u16; 3] {
        if let Some(sites) = &self.sites {
            sites.site(x, y)
        } else {
            let row = y * (self.dimensions[0] + 1);
            std::array::from_fn(|c| (self.prefix[row + x + 1][c] - self.prefix[row + x][c]) as u16)
        }
    }
    pub fn stores_prefixes(&self) -> bool {
        !self.prefix.is_empty()
    }
    /// Borrow stored prefixes, or compute only this mosaic row.
    pub fn row(&self, y: usize) -> Row<'_> {
        let width = self.dimensions[0];
        let prefix = if self.sites.is_some() {
            let mut prefix = Vec::new();
            self.row_prefix(y, &mut prefix);
            Cow::Owned(prefix)
        } else {
            Cow::Borrowed(&self.prefix[y * (width + 1)..(y + 1) * (width + 1)])
        };
        Row { prefix }
    }
    /// Row `y`'s prefix sums: stored, or computed into `prefix` for a
    /// mosaic scene.
    fn row_prefix<'a>(&'a self, y: usize, prefix: &'a mut Vec<[u32; 3]>) -> &'a [[u32; 3]] {
        let width = self.dimensions[0];
        let Some(sites) = &self.sites else {
            return &self.prefix[y * (width + 1)..(y + 1) * (width + 1)];
        };
        prefix.resize(width + 1, [0; 3]);
        sites.row_prefix(y, prefix);
        prefix
    }
    pub fn horizontal(&self, x: usize, y: usize, radius: usize, fraction: u32) -> [u16; 3] {
        if radius == 0 && fraction == 0 {
            return self.linear(x, y);
        }
        self.row(y).horizontal(x, radius, fraction)
    }
}

pub(super) struct Row<'a> {
    prefix: Cow<'a, [[u32; 3]]>,
}
impl Row<'_> {
    #[inline(always)]
    pub fn horizontal(&self, x: usize, radius: usize, fraction: u32) -> [u16; 3] {
        let row = &self.prefix;
        let width = row.len() - 1;
        let inner = Span::new(x, width, radius);
        let outer = Span::new(x, width, radius + 1);
        let mut result = [0; 3];
        for c in 0..3 {
            let first = row[1][c] as u16;
            let last = (row[width][c] - row[width - 1][c]) as u16;
            let a = if radius == 0 {
                u64::from(row[x + 1][c] - row[x][c])
            } else {
                inner.mean(
                    u64::from(row[inner.first][c]),
                    u64::from(row[inner.last][c]),
                    first,
                    last,
                )
            };
            let b = if fraction == 0 {
                a
            } else {
                outer.mean(
                    u64::from(row[outer.first][c]),
                    u64::from(row[outer.last][c]),
                    first,
                    last,
                )
            };
            result[c] = blend(a, fraction, b);
        }
        result
    }
    /// `horizontal` at each of `xs`. Interior points, whose boxes need no
    /// edge extension, read their prefix differences directly.
    #[inline(always)]
    pub fn horizontal_into(
        &self,
        xs: &[usize],
        radius: usize,
        fraction: u32,
        out: &mut [[u16; 3]],
    ) {
        let row: &[[u32; 3]] = &self.prefix;
        let width = row.len() - 1;
        let (inner, outer) = (2 * radius as u64 + 1, 2 * radius as u64 + 3);
        for (&x, value) in xs.iter().zip(out) {
            *value = if x > radius && x + radius + 2 <= width {
                let a = (&row[x - radius], &row[x + radius + 1]);
                let mut result = [0; 3];
                if fraction == 0 {
                    for c in 0..3 {
                        result[c] = ((u64::from(a.1[c] - a.0[c]) + radius as u64) / inner) as u16;
                    }
                } else {
                    let b = (&row[x - radius - 1], &row[x + radius + 2]);
                    for c in 0..3 {
                        result[c] = blend(
                            (u64::from(a.1[c] - a.0[c]) + radius as u64) / inner,
                            fraction,
                            (u64::from(b.1[c] - b.0[c]) + radius as u64 + 1) / outer,
                        );
                    }
                }
                result
            } else {
                self.horizontal(x, radius, fraction)
            };
        }
    }
}
/// Row prefix sums of linear values; the first column of each row is zero.
fn prefix_sums(dimensions: [usize; 2], linear: impl Fn(usize, usize) -> [u16; 3]) -> Vec<[u32; 3]> {
    let [width, height] = dimensions;
    let mut prefix = vec![[0; 3]; (width + 1) * height];
    for (y, row) in prefix.chunks_exact_mut(width + 1).enumerate() {
        let mut sum = [0u32; 3];
        for (x, out) in row[1..].iter_mut().enumerate() {
            let value = linear(x, y);
            for c in 0..3 {
                sum[c] += u32::from(value[c]);
            }
            *out = sum;
        }
    }
    prefix
}
/// A clamped box interval, shared across channels and readout columns.
#[derive(Clone, Copy)]
pub(super) struct Span {
    pub first: usize,
    pub last: usize,
    before: u64,
    after: u64,
    count: u64,
}
impl Span {
    #[inline(always)]
    pub fn new(point: usize, length: usize, radius: usize) -> Self {
        Self {
            first: point.saturating_sub(radius),
            last: (point + radius + 1).min(length),
            before: radius.saturating_sub(point) as u64,
            after: (point + radius + 1).saturating_sub(length) as u64,
            count: (2 * radius + 1) as u64,
        }
    }
    #[inline(always)]
    pub fn mean(self, a: u64, b: u64, first: u16, last: u16) -> u64 {
        (b - a + u64::from(first) * self.before + u64::from(last) * self.after + self.count / 2)
            / self.count
    }
}

#[inline(always)]
pub(super) fn blend(inner: u64, fraction: u32, outer: u64) -> u16 {
    if fraction == 0 {
        inner as u16
    } else {
        ((inner * u64::from(65536 - fraction) + outer * u64::from(fraction) + 32768) >> 16) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rgb(bytes: Vec<u8>, dimensions: [usize; 2]) -> Scene {
        Scene::new(Arc::new(
            Frame::new(
                dimensions[0],
                dimensions[1],
                FramePixels::Rgb24(bytes.into()),
            )
            .unwrap(),
        ))
        .unwrap()
    }
    #[test]
    fn computed_mosaic_rows_match_dense_optics_bins_and_checkpoint_restore() {
        use super::super::readout::{Coordinates, project_axes};
        use fujimod_emulator_imaging::scene::mosaic::Radiance;
        let [width, height] = [13, 8];
        let cfa = std::array::from_fn(|y| std::array::from_fn(|x| ((x + 2 * y) % 3) as u8));
        let counts: Vec<u16> = (0..width * height)
            .map(|i| (i * 2357 % 65536) as u16)
            .collect();
        let norm = 1. / 64800.;
        let frame = Arc::new(
            Frame::new(
                width,
                height,
                FramePixels::Mosaic {
                    cfa,
                    radiance: Radiance::from_counts(counts.iter().copied(), norm),
                },
            )
            .unwrap(),
        );
        // Independent reconstruction of the former full-resolution f32 scene.
        let values: Vec<_> = counts
            .iter()
            .map(|&v| (f64::from(v) / norm) as f32)
            .collect();
        let max = values.iter().copied().fold(0f32, f32::max);
        let dense = Scene {
            frame: frame.clone(),
            dimensions: [width, height],
            sites: None,
            prefix: prefix_sums([width, height], |x, y| {
                let mut sum = [0f64; 3];
                let mut count = [0u32; 3];
                for sy in y / 6 * 6..((y / 6 + 1) * 6).min(height) {
                    for sx in x / 6 * 6..((x / 6 + 1) * 6).min(width) {
                        let c = usize::from(cfa[sy % 6][sx % 6]);
                        sum[c] += f64::from(values[sy * width + sx]);
                        count[c] += 1;
                    }
                }
                let own = usize::from(cfa[y % 6][x % 6]);
                std::array::from_fn(|c| {
                    let v = if c == own {
                        values[y * width + x]
                    } else if count[c] == 0 {
                        0.
                    } else {
                        (sum[c] / f64::from(count[c])) as f32
                    };
                    libm::rint(f64::from(v) * (65535. / f64::from(max))).clamp(0., 65535.) as u16
                })
            }),
        };
        let saved = frame.checkpoint().unwrap();
        let restored = Arc::new(Frame::from_checkpoint(&saved).unwrap());
        assert!(restored.same_pixels(&frame));
        for frame in [frame, restored] {
            let scene = Scene::new(frame).unwrap();
            assert!(scene.prefix.is_empty());
            assert!(!scene.stores_prefixes());
            for y in 0..height {
                for x in 0..width {
                    assert_eq!(scene.linear(x, y), dense.linear(x, y));
                    for radius in [0, 1, 5, 20] {
                        for fraction in [0, 1, 32768, 65535] {
                            assert_eq!(
                                scene.horizontal(x, y, radius, fraction),
                                dense.horizontal(x, y, radius, fraction)
                            );
                        }
                    }
                }
            }
            for dimensions in [[1, 1], [7, 3], [13, 8], [17, 11]] {
                let binned = scene.binned(dimensions);
                assert!(binned.stores_prefixes());
                assert_eq!(binned.prefix, dense.binned(dimensions).prefix);
            }
            let sparse = Coordinates::new([width, height], [23, 17], [3, 2, 15, 12]).unwrap();
            // Widely separated rows exercise disjoint sliding windows too.
            let separated = Coordinates::new([width, height], [3, 2], [0, 0, 3, 2]).unwrap();
            for coordinates in [Coordinates::full([width, height]), sparse, separated] {
                for radius in [[0, 0], [1, 0], [0, 3], [2, 4], [20, 15]] {
                    for fraction in [[0, 0], [1, 65535], [32768, 16384]] {
                        assert_eq!(
                            project_axes(&scene, &coordinates, radius, fraction),
                            project_axes(&dense, &coordinates, radius, fraction),
                            "radius {radius:?} fraction {fraction:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn binning_integrates_linear_light_and_preserves_flat_fields() {
        let flat = rgb(vec![91; 7 * 5 * 3], [7, 5]);
        let small = flat.binned([3, 2]);
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(small.linear(x, y), flat.linear(0, 0));
            }
        }
        let alternating: Vec<_> = (0..8 * 6)
            .flat_map(|i| [if i % 2 == 0 { 0 } else { 255 }; 3])
            .collect();
        let scene = rgb(alternating, [8, 6]);
        let small = scene.binned([4, 3]);
        for y in 0..3 {
            for x in 0..4 {
                assert_eq!(small.linear(x, y), [32768; 3]);
            }
        }
        assert_ne!(small.linear(0, 0), rgb(vec![128; 3], [1, 1]).linear(0, 0));
    }
    #[test]
    fn nonintegral_bins_match_independent_cell_overlap_integration() {
        let bytes: Vec<_> = (0..7 * 5 * 3)
            .map(|i| ((i * 71 + 23) % 256) as u8)
            .collect();
        let scene = rgb(bytes, [7, 5]);
        for dims in [[3, 2], [4, 3], [1, 1], [7, 5]] {
            let small = scene.binned(dims);
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let mut sum = [0u64; 3];
                    for sy in 0..5 {
                        for sx in 0..7 {
                            let wx = ((x + 1) * 7)
                                .min((sx + 1) * dims[0])
                                .saturating_sub((x * 7).max(sx * dims[0]));
                            let wy = ((y + 1) * 5)
                                .min((sy + 1) * dims[1])
                                .saturating_sub((y * 5).max(sy * dims[1]));
                            let raw = scene.linear(sx, sy);
                            for c in 0..3 {
                                sum[c] += u64::from(raw[c]) * (wx * wy) as u64;
                            }
                        }
                    }
                    assert_eq!(small.linear(x, y), sum.map(|s| ((s + 17) / 35) as u16));
                }
            }
        }
    }
}
