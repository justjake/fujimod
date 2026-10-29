use super::super::exposure::{NoiseResponse, adc};
use super::{
    encode_response,
    scene::{Scene, Span, blend},
};
use std::sync::Arc;

pub(super) struct Coordinates {
    pub key: ([usize; 2], [usize; 4]),
    pub xs: Vec<usize>,
    pub ys: Vec<usize>,
    x_index: Vec<u16>,
    y_index: Vec<u16>,
}
impl Coordinates {
    pub fn new(
        dimensions: [usize; 2],
        grid: [usize; 2],
        window: [usize; 4],
    ) -> Result<Self, String> {
        Self::validate(grid, window)?;
        let [left, top, width, height] = window;
        let axis = |begin: usize, count: usize, grid: usize, source: usize| {
            let mut points = Vec::with_capacity(count.min(source));
            let mut index = vec![u16::MAX; source];
            for p in begin..begin + count {
                let sample = ((2 * p + 1) * source / (2 * grid)).min(source - 1);
                if points.last() != Some(&sample) {
                    index[sample] = points.len() as u16;
                    points.push(sample);
                }
            }
            (points, index)
        };
        let (mut xs, mut x_index) = axis(left, width, grid[0], dimensions[0]);
        let (mut ys, mut y_index) = axis(top, height, grid[1], dimensions[1]);
        // Nearly complete readouts feed peers using different sampling grids.
        // Keep a dense projection here so those peers do not recompute a PSF
        // for every missing pixel. Small programmed windows remain sparse.
        let full = dimensions[0] * dimensions[1];
        let selected = xs.len() * ys.len();
        if selected < full && selected * 4 >= full * 3 {
            xs = (0..dimensions[0]).collect();
            ys = (0..dimensions[1]).collect();
            x_index = (0..dimensions[0]).map(|x| x as u16).collect();
            y_index = (0..dimensions[1]).map(|y| y as u16).collect();
        }
        Ok(Self {
            key: (grid, window),
            xs,
            ys,
            x_index,
            y_index,
        })
    }
    /// Union of programmed windows. Only selected rows and columns enter the
    /// PSF pass; its halo is supplied by the immutable binned scene.
    pub fn windows(
        dimensions: [usize; 2],
        grid: [usize; 2],
        windows: &[[usize; 4]],
    ) -> Result<Self, String> {
        let mut xs = vec![false; dimensions[0]];
        let mut ys = vec![false; dimensions[1]];
        for &[x, y, w, h] in windows {
            Self::validate(grid, [x, y, w, h])?;
            if w == 0 || h == 0 {
                continue;
            }
            for at in x..x + w {
                xs[((2 * at + 1) * dimensions[0] / (2 * grid[0])).min(dimensions[0] - 1)] = true;
            }
            for at in y..y + h {
                ys[((2 * at + 1) * dimensions[1] / (2 * grid[1])).min(dimensions[1] - 1)] = true;
            }
        }
        let axis = |selected: Vec<bool>| {
            let points: Vec<_> = selected
                .iter()
                .enumerate()
                .filter_map(|(i, &yes)| yes.then_some(i))
                .collect();
            let mut index = vec![u16::MAX; selected.len()];
            for (i, &p) in points.iter().enumerate() {
                index[p] = i as u16;
            }
            (points, index)
        };
        let (xs, x_index) = axis(xs);
        let (ys, y_index) = axis(ys);
        Ok(Self {
            key: (grid, [0, 0, grid[0], grid[1]]),
            xs,
            ys,
            x_index,
            y_index,
        })
    }
    pub fn validate(grid: [usize; 2], window: [usize; 4]) -> Result<(), String> {
        let [left, top, width, height] = window;
        if grid.contains(&0)
            || grid.iter().any(|&n| n > 16384)
            || left.checked_add(width).is_none_or(|end| end > grid[0])
            || top.checked_add(height).is_none_or(|end| end > grid[1])
        {
            return Err("optical readout window exceeds its sampling grid".into());
        }
        Ok(())
    }
    pub fn full(dimensions: [usize; 2]) -> Self {
        Self::new(dimensions, dimensions, [0, 0, dimensions[0], dimensions[1]]).unwrap()
    }
    pub fn complete(&self, dimensions: [usize; 2]) -> bool {
        self.xs.len() == dimensions[0] && self.ys.len() == dimensions[1]
    }
}

/// Pixels for one immutable exposure, with a pure full-scene law for other
/// coordinates. Inspection may materialize that law without changing state.
pub(crate) struct Projected {
    scene: Arc<Scene>,
    pub(super) coordinates: Arc<Coordinates>,
    radius: [usize; 2],
    fraction: [u32; 2],
    sensor_height: Option<f64>,
    scale: [f64; 3],
    sensor_width: f64,
    illumination: Option<super::Field>,
    noise: NoiseResponse,
    pixels: Arc<[u8]>,
}
impl Projected {
    pub(super) fn new(
        scene: Arc<Scene>,
        coordinates: Arc<Coordinates>,
        radius: usize,
        fraction: u32,
        linear: &[[u16; 3]],
        scale: [f64; 3],
        sensor_width: f64,
        illumination: Option<super::Field>,
        noise: NoiseResponse,
    ) -> Self {
        let mut pixels = vec![0; linear.len() * 6];
        let [width, height] = scene.dimensions;
        let pitch = sensor_width / width as f64;
        // Preserve the original expression's operation order, including the
        // normalized coordinate divisions. Only invariant axes move outwards.
        let xs: Vec<f64> = coordinates
            .xs
            .iter()
            .map(|&x| {
                let x = x as f64 * pitch + pitch * 0.5 - sensor_width * 0.5;
                illumination.map_or(0., |f| f.axis(x))
            })
            .collect();
        for (row, &y) in coordinates.ys.iter().enumerate() {
            let y = y as f64 * pitch + pitch * 0.5 - height as f64 * pitch * 0.5;
            let yy = illumination.map_or(0., |f| f.axis(y));
            for (column, &xx) in xs.iter().enumerate() {
                let field = illumination.map_or(1., |f| f.at(xx, yy));
                let i = row * coordinates.xs.len() + column;
                for c in 0..3 {
                    let value = adc(
                        f64::from(linear[i][c]) * scale[c] / 65535. * field,
                        noise,
                        coordinates.xs[column],
                        coordinates.ys[row],
                        c,
                    );
                    pixels[i * 6 + c * 2..i * 6 + c * 2 + 2].copy_from_slice(&value.to_le_bytes());
                }
            }
        }
        Self {
            scene,
            coordinates,
            radius: [radius; 2],
            fraction: [fraction; 2],
            sensor_height: None,
            scale,
            sensor_width,
            illumination,
            noise,
            pixels: pixels.into(),
        }
    }
    pub(super) fn new_axes(
        scene: Arc<Scene>,
        coordinates: Arc<Coordinates>,
        radius: [usize; 2],
        fraction: [u32; 2],
        linear: &[[u16; 3]],
        scale: [f64; 3],
        sensor: [f64; 2],
        illumination: Option<super::Field>,
        noise: NoiseResponse,
    ) -> Self {
        let pixels = super::encode_response_coordinates(
            linear,
            scale,
            scene.dimensions,
            sensor,
            illumination,
            &coordinates,
            noise,
        );
        Self {
            scene,
            coordinates,
            radius,
            fraction,
            scale,
            sensor_width: sensor[0],
            sensor_height: Some(sensor[1]),
            illumination,
            noise,
            pixels,
        }
    }
    pub(crate) fn sample_offsets(
        &self,
        grid: [usize; 2],
        window: [usize; 4],
        step: usize,
    ) -> Option<super::super::exposure::SampleOffsets> {
        let [width, height] = self.scene.dimensions;
        let [x, y, w, h] = window;
        let columns: Option<Vec<_>> = (x..(x + w).min(grid[0]))
            .step_by(step)
            .map(|x| {
                let i =
                    self.coordinates.x_index[((2 * x + 1) * width / (2 * grid[0])).min(width - 1)];
                (i != u16::MAX).then_some(usize::from(i) * 6)
            })
            .collect();
        let rows: Option<Vec<_>> = (y..(y + h).min(grid[1]))
            .map(|y| {
                let i = self.coordinates.y_index
                    [((2 * y + 1) * height / (2 * grid[1])).min(height - 1)];
                (i != u16::MAX).then_some(usize::from(i) * self.coordinates.xs.len() * 6)
            })
            .collect();
        Some(super::super::exposure::SampleOffsets {
            pixels: self.pixels.clone(),
            columns: columns?,
            rows: rows?,
        })
    }
    pub fn covers(&self, grid: [usize; 2], windows: &[[usize; 4]]) -> bool {
        let [width, height] = self.scene.dimensions;
        windows.iter().all(|&[x, y, w, h]| {
            x + w <= grid[0]
                && y + h <= grid[1]
                && (x..x + w).all(|x| {
                    self.coordinates.x_index[((2 * x + 1) * width / (2 * grid[0])).min(width - 1)]
                        != u16::MAX
                })
                && (y..y + h).all(|y| {
                    self.coordinates.y_index[((2 * y + 1) * height / (2 * grid[1])).min(height - 1)]
                        != u16::MAX
                })
        })
    }
    pub fn prepared_pixels(&self) -> usize {
        self.coordinates.xs.len() * self.coordinates.ys.len()
    }
    pub fn complete(&self) -> bool {
        self.coordinates.complete(self.scene.dimensions)
    }
    #[inline(always)]
    pub fn sample(&self, x: usize, y: usize) -> [u16; 3] {
        let column = self.coordinates.x_index[x];
        let row = self.coordinates.y_index[y];
        if column != u16::MAX && row != u16::MAX {
            let at = (usize::from(row) * self.coordinates.xs.len() + usize::from(column)) * 6;
            return std::array::from_fn(|c| {
                u16::from_le_bytes([self.pixels[at + c * 2], self.pixels[at + c * 2 + 1]])
            });
        }
        self.other_sample(x, y)
    }
    #[cold]
    fn other_sample(&self, x: usize, y: usize) -> [u16; 3] {
        let height = self.scene.dimensions[1];
        let mean = |radius: usize| {
            let mut sum = [0u64; 3];
            for yy in y.saturating_sub(radius)..(y + radius + 1).min(height) {
                let v = self
                    .scene
                    .horizontal(x, yy, self.radius[0], self.fraction[0]);
                for c in 0..3 {
                    sum[c] += u64::from(v[c]);
                }
            }
            let first = self
                .scene
                .horizontal(x, 0, self.radius[0], self.fraction[0]);
            let last = self
                .scene
                .horizontal(x, height - 1, self.radius[0], self.fraction[0]);
            let count = (2 * radius + 1) as u64;
            std::array::from_fn::<_, 3, _>(|c| {
                (sum[c]
                    + u64::from(first[c]) * radius.saturating_sub(y) as u64
                    + u64::from(last[c]) * (y + radius + 1).saturating_sub(height) as u64
                    + count / 2)
                    / count
            })
        };
        let inner = if self.radius[1] == 0 {
            self.scene
                .horizontal(x, y, self.radius[0], self.fraction[0])
                .map(u64::from)
        } else {
            mean(self.radius[1])
        };
        let outer = if self.fraction[1] == 0 {
            inner
        } else {
            mean(self.radius[1] + 1)
        };
        let linear: [u16; 3] = std::array::from_fn(|c| blend(inner[c], self.fraction[1], outer[c]));
        let pitch = self.sensor_width / self.scene.dimensions[0] as f64;
        let field = self.illumination.map_or(1., |f| {
            f.at_point(
                self.sensor_height.map_or(
                    x as f64 * pitch + pitch * 0.5 - self.sensor_width * 0.5,
                    |_| {
                        ((x as f64 + 0.5) / self.scene.dimensions[0] as f64 - 0.5)
                            * self.sensor_width
                    },
                ),
                self.sensor_height.map_or(
                    y as f64 * pitch + pitch * 0.5 - height as f64 * pitch * 0.5,
                    |h| ((y as f64 + 0.5) / height as f64 - 0.5) * h,
                ),
            )
        });
        std::array::from_fn(|c| {
            adc(
                f64::from(linear[c]) * self.scale[c] / 65535. * field,
                self.noise,
                x,
                y,
                c,
            )
        })
    }
    pub fn materialize(&self) -> Arc<[u8]> {
        if self.coordinates.complete(self.scene.dimensions) {
            return self.pixels.clone();
        }
        let all = Coordinates::full(self.scene.dimensions);
        if let Some(height) = self.sensor_height {
            let linear = project_axes(&self.scene, &all, self.radius, self.fraction);
            return super::encode_response_coordinates(
                &linear,
                self.scale,
                self.scene.dimensions,
                [self.sensor_width, height],
                self.illumination,
                &all,
                self.noise,
            );
        }
        let linear = project(&self.scene, &all, self.radius[0], self.fraction[0], None);
        encode_response(
            &linear,
            self.scale,
            self.scene.dimensions,
            self.sensor_width,
            self.illumination,
            self.noise,
        )
    }
}

/// The horizontal pass uses immutable scene row prefixes. Process nearby
/// columns together so vertical sums touch contiguous cache lines; only the
/// readout's source columns, rows and required PSF halo enter the work buffer.
pub(super) fn project(
    scene: &Scene,
    coordinates: &Coordinates,
    radius: usize,
    fraction: u32,
    horizontal: Option<&super::horizontal::Prepared>,
) -> Vec<[u16; 3]> {
    if radius == 0 && fraction == 0 {
        return project_axes(scene, coordinates, [0; 2], [0; 2]);
    }
    // Select the sample source once per projection, leaving each pixel loop
    // specialized for either cached means or the original scratch path.
    match horizontal {
        Some(prepared) => project_sampled(
            scene,
            coordinates,
            radius,
            fraction,
            |y, xs, offset, out| {
                for (column, value) in out.iter_mut().take(xs.len()).enumerate() {
                    *value = prepared.sample(offset + column, y);
                }
            },
        ),
        None => project_sampled(scene, coordinates, radius, fraction, |y, xs, _, out| {
            horizontal_row(scene, y, xs, radius, fraction, out);
        }),
    }
}
pub(super) fn project_axes(
    scene: &Scene,
    coordinates: &Coordinates,
    radius: [usize; 2],
    fraction: [u32; 2],
) -> Vec<[u16; 3]> {
    project_prepared_axes(scene, coordinates, radius, fraction, None)
}
pub(super) fn project_prepared_axes(
    scene: &Scene,
    coordinates: &Coordinates,
    radius: [usize; 2],
    fraction: [u32; 2],
    horizontal: Option<&super::horizontal::Prepared>,
) -> Vec<[u16; 3]> {
    if radius == [0; 2] && fraction == [0; 2] {
        return coordinates
            .ys
            .iter()
            .flat_map(|&y| coordinates.xs.iter().map(move |&x| scene.linear(x, y)))
            .collect();
    }
    match horizontal {
        Some(prepared) => project_sampled(
            scene,
            coordinates,
            radius[1],
            fraction[1],
            |y, xs, offset, out| {
                for (column, value) in out.iter_mut().take(xs.len()).enumerate() {
                    *value = prepared.sample(offset + column, y);
                }
            },
        ),
        None => project_sampled(
            scene,
            coordinates,
            radius[1],
            fraction[1],
            |y, xs, _, out| horizontal_row(scene, y, xs, radius[0], fraction[0], out),
        ),
    }
}
fn horizontal_row(
    scene: &Scene,
    y: usize,
    xs: &[usize],
    radius: usize,
    fraction: u32,
    out: &mut [[u16; 3]],
) {
    if radius == 0 && fraction == 0 {
        for (&x, value) in xs.iter().zip(out) {
            *value = scene.linear(x, y);
        }
    } else {
        scene.row(y).horizontal_into(xs, radius, fraction, out);
    }
}
fn project_sampled(
    scene: &Scene,
    coordinates: &Coordinates,
    radius: usize,
    fraction: u32,
    sample: impl Fn(usize, &[usize], usize, &mut [[u16; 3]]),
) -> Vec<[u16; 3]> {
    if !scene.stores_prefixes() {
        return project_rows(scene, coordinates, radius, fraction, sample);
    }
    let width = coordinates.xs.len();
    let mut result = vec![[0; 3]; width * coordinates.ys.len()];
    let (Some(&first_y), Some(&last_y)) = (coordinates.ys.first(), coordinates.ys.last()) else {
        return result;
    };
    if width == 0 {
        return result;
    }
    let height = scene.dimensions[1];
    let begin = first_y.saturating_sub(radius + 1);
    let end = (last_y + radius + 2).min(height);
    // Horizontal means of every row a vertical box reaches, each computed
    // once and read in row order by sliding vertical sums.
    let mut plane = vec![[0; 3]; width * (end - begin)];
    for (y, row) in (begin..end).zip(plane.chunks_exact_mut(width)) {
        sample(y, &coordinates.xs, 0, row);
    }
    let mut first = vec![[0; 3]; width];
    let mut last = vec![[0; 3]; width];
    sample(0, &coordinates.xs, 0, &mut first);
    sample(height - 1, &coordinates.xs, 0, &mut last);
    let row = |y: usize| &plane[(y - begin) * width..(y - begin + 1) * width];
    let mut inner = MovingSum::new(width);
    let mut outer = MovingSum::new(width);
    for (&y, out) in coordinates.ys.iter().zip(result.chunks_exact_mut(width)) {
        let a = Span::new(y, height, radius);
        let b = Span::new(y, height, radius + 1);
        inner.slide(a, row);
        if fraction == 0 {
            for (((out, sum), first), last) in
                out.iter_mut().zip(&inner.sums).zip(&first).zip(&last)
            {
                for c in 0..3 {
                    out[c] = a.mean(0, sum[c], first[c], last[c]) as u16;
                }
            }
        } else {
            outer.slide(b, row);
            for ((((out, inner), outer), first), last) in out
                .iter_mut()
                .zip(&inner.sums)
                .zip(&outer.sums)
                .zip(&first)
                .zip(&last)
            {
                for c in 0..3 {
                    out[c] = blend(
                        a.mean(0, inner[c], first[c], last[c]),
                        fraction,
                        b.mean(0, outer[c], first[c], last[c]),
                    );
                }
            }
        }
    }
    result
}

/// Sweep computed mosaic rows across all selected columns. Sliding vertical
/// sums recompute departing rows rather than retaining a full horizontal plane.
/// Scratch stays proportional to one source row and the selected columns.
fn project_rows(
    scene: &Scene,
    coordinates: &Coordinates,
    radius: usize,
    fraction: u32,
    sample: impl Fn(usize, &[usize], usize, &mut [[u16; 3]]),
) -> Vec<[u16; 3]> {
    let width = coordinates.xs.len();
    let height = scene.dimensions[1];
    let mut result = vec![[0; 3]; width * coordinates.ys.len()];
    if width == 0 || coordinates.ys.is_empty() {
        return result;
    }
    let mut first = vec![[0; 3]; width];
    let mut last = vec![[0; 3]; width];
    sample(0, &coordinates.xs, 0, &mut first);
    sample(height - 1, &coordinates.xs, 0, &mut last);
    let mut scratch = vec![[0; 3]; width];
    let mut inner = MovingSum::new(width);
    let mut outer = MovingSum::new(width);
    for (row, &y) in coordinates.ys.iter().enumerate() {
        let a = Span::new(y, height, radius);
        let b = Span::new(y, height, radius + 1);
        let mut sample_row = |sy, values: &mut [[u16; 3]]| sample(sy, &coordinates.xs, 0, values);
        inner.advance(a, &mut scratch, &mut sample_row);
        if fraction != 0 {
            outer.advance(b, &mut scratch, &mut sample_row);
        }
        for x in 0..width {
            result[row * width + x] = std::array::from_fn(|c| {
                let mean = a.mean(0, inner.sums[x][c], first[x][c], last[x][c]);
                let other = if fraction == 0 {
                    mean
                } else {
                    b.mean(0, outer.sums[x][c], first[x][c], last[x][c])
                };
                blend(mean, fraction, other)
            });
        }
    }
    result
}
struct MovingSum {
    span: Option<Span>,
    sums: Vec<[u64; 3]>,
}
impl MovingSum {
    fn new(width: usize) -> Self {
        Self {
            span: None,
            sums: vec![[0; 3]; width],
        }
    }
    /// Move the window to `next`, reading retained rows.
    fn slide<'a>(&mut self, next: Span, row: impl Fn(usize) -> &'a [[u16; 3]]) {
        let add = |sums: &mut [[u64; 3]], y: usize, sign: bool| {
            for (sum, value) in sums.iter_mut().zip(row(y)) {
                for c in 0..3 {
                    if sign {
                        sum[c] += u64::from(value[c]);
                    } else {
                        sum[c] -= u64::from(value[c]);
                    }
                }
            }
        };
        if let Some(old) = self.span {
            for y in old.first..next.first.min(old.last) {
                add(&mut self.sums, y, false);
            }
        }
        let begin = self.span.map_or(next.first, |old| old.last.max(next.first));
        for y in begin..next.last {
            add(&mut self.sums, y, true);
        }
        self.span = Some(next);
    }
    fn advance(
        &mut self,
        next: Span,
        scratch: &mut [[u16; 3]],
        sample: &mut impl FnMut(usize, &mut [[u16; 3]]),
    ) {
        if let Some(old) = self.span {
            for y in old.first..next.first.min(old.last) {
                sample(y, scratch);
                for (sum, value) in self.sums.iter_mut().zip(scratch.iter()) {
                    for c in 0..3 {
                        sum[c] -= u64::from(value[c]);
                    }
                }
            }
        }
        let begin = self.span.map_or(next.first, |old| old.last.max(next.first));
        for y in begin..next.last {
            sample(y, scratch);
            for (sum, value) in self.sums.iter_mut().zip(scratch.iter()) {
                for c in 0..3 {
                    sum[c] += u64::from(value[c]);
                }
            }
        }
        self.span = Some(next);
    }
}
