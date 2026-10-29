//! Ordered single-sample rasterization over Morton color/depth surfaces.
#[cfg(test)]
use super::Engine;
use super::{
    MAX_BYTES, Reader, Snapshots,
    draw_state::{ColorTargetFormat, CullMode, DrawState, TextureUnit},
    shader::Vector,
};
use std::collections::BTreeMap;
#[derive(Default)]
pub(super) struct Targets {
    buffers: BTreeMap<u64, Vec<u8>>,
    textures: BTreeMap<(u64, usize), Vec<u8>>,
    written: BTreeMap<u64, Vec<bool>>,
}
impl Targets {
    fn buffer(
        &mut self,
        address: u64,
        size: usize,
        read: &mut Reader<'_>,
    ) -> Result<&mut Vec<u8>, String> {
        if !self.buffers.contains_key(&address) {
            if size == 0
                || size > MAX_BYTES
                || self.buffers.values().map(Vec::len).sum::<usize>() + size > MAX_BYTES
            {
                return Err("DMP 3D target bound exceeded".into());
            }
            let bytes = read(address, size)?;
            if bytes.len() != size {
                return Err("DMP short target DMA".into());
            }
            self.buffers.insert(address, bytes);
        }
        let bytes = self.buffers.get_mut(&address).unwrap();
        if bytes.len() != size {
            return Err("DMP target extent changed within list".into());
        }
        Ok(bytes)
    }
    fn mark_written(&mut self, address: u64, offset: usize, count: usize) {
        let extent = self.buffers[&address].len();
        self.written
            .entry(address)
            .or_insert_with(|| vec![false; extent])[offset..offset + count]
            .fill(true);
    }
    pub fn written_ranges(&self) -> Vec<(u64, usize)> {
        let mut ranges = Vec::new();
        for (&address, flags) in &self.written {
            let mut offset = 0;
            while offset < flags.len() {
                if !flags[offset] {
                    offset += 1;
                    continue;
                }
                let begin = offset;
                while offset < flags.len() && flags[offset] {
                    offset += 1;
                }
                ranges.push((address + begin as u64, offset - begin));
            }
        }
        ranges
    }
    pub fn into_snapshots(self) -> Snapshots {
        self.buffers.into_iter().collect()
    }
}
fn morton(x: usize, y: usize, width: usize) -> usize {
    (y & !7) * width
        + (x & !7) * 8
        + (x & 1)
        + ((y & 1) << 1)
        + ((x & 2) << 1)
        + ((y & 2) << 2)
        + ((x & 4) << 2)
        + ((y & 4) << 3)
}
#[derive(Clone, Copy)]
pub(super) struct RasterVertex {
    pub(super) position: Vector,
    pub(super) color: Vector,
    pub(super) uv: [[f32; 2]; 2],
}
fn vertex(state: &DrawState, v: &[Vector; 16]) -> RasterVertex {
    let mut values = [0.; 32];
    for (slot, vector) in v.iter().enumerate().take(state.vertex.output_map.0.len()) {
        for (i, value) in vector.iter().enumerate() {
            let semantic = state.vertex.output_map.0[slot][i] as u32;
            if semantic != 31 {
                values[semantic as usize] = *value;
            }
        }
    }
    RasterVertex {
        position: values[..4].try_into().unwrap(),
        color: values[8..12].try_into().unwrap(),
        uv: [[values[12], values[13]], [values[14], values[15]]],
    }
}
fn mix(a: RasterVertex, b: RasterVertex, t: f32) -> RasterVertex {
    RasterVertex {
        position: std::array::from_fn(|i| a.position[i] + (b.position[i] - a.position[i]) * t),
        color: std::array::from_fn(|i| a.color[i] + (b.color[i] - a.color[i]) * t),
        uv: std::array::from_fn(|u| {
            std::array::from_fn(|i| a.uv[u][i] + (b.uv[u][i] - a.uv[u][i]) * t)
        }),
    }
}
fn plane(v: RasterVertex, i: usize) -> f32 {
    let [x, y, z, w] = v.position;
    match i {
        0 => x + w,
        1 => w - x,
        2 => y + w,
        3 => w - y,
        4 => z + w,
        _ => -z,
    }
}
fn project(state: &DrawState, v: RasterVertex, point: bool) -> RasterVertex {
    let mut v = v;
    v.position = state.viewport.project(v.position, point);
    v
}
pub(super) fn triangle(
    state: &DrawState,
    vertices: [[Vector; 16]; 3],
    read: &mut Reader<'_>,
    out: &mut Targets,
) -> Result<(), String> {
    for v in clipped_triangles(state, vertices)? {
        raster_triangle(state, v, read, out)?;
    }
    Ok(())
}

/// Clip one DMP triangle and return the screen-space triangles that survive.
/// Positions are snapped to the DMP 1/16-pixel grid before either renderer
/// consumes them, so host rasterization sees the same primitive edges.
pub(super) fn clipped_triangles(
    state: &DrawState,
    vertices: [[Vector; 16]; 3],
) -> Result<Vec<[RasterVertex; 3]>, String> {
    let mut polygon: Vec<_> = vertices.iter().map(|v| vertex(state, v)).collect();
    if state.raster.flat_shading {
        let provoking = polygon[2].color;
        for v in &mut polygon {
            v.color = provoking;
        }
    }
    if polygon
        .iter()
        .any(|v| v.position.iter().any(|x| !x.is_finite()))
    {
        return Ok(Vec::new());
    }
    for p in 0..6 {
        let mut clipped = Vec::new();
        for i in 0..polygon.len() {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            let da = plane(a, p);
            let db = plane(b, p);
            if da >= 0. {
                clipped.push(a);
            }
            if (da >= 0.) != (db >= 0.) {
                clipped.push(mix(a, b, da / (da - db)));
            }
        }
        polygon = clipped;
        if polygon.len() < 3 {
            return Ok(Vec::new());
        }
    }
    let (width, height) = extent(state)?;
    let mut result = Vec::new();
    for i in 1..polygon.len() - 1 {
        let mut v = [
            project(state, polygon[0], false),
            project(state, polygon[i], false),
            project(state, polygon[i + 1], false),
        ];
        let q = v.map(|vertex| {
            [
                (vertex.position[0] * 16.).round() as i64,
                (vertex.position[1] * 16.).round() as i64,
            ]
        });
        let area = edge(q[0], q[1], q[2]);
        if area == 0
            || state.raster.cull_mode == CullMode::Front && area > 0
            || state.raster.cull_mode == CullMode::Back && area < 0
        {
            continue;
        }
        for index in 0..3 {
            v[index].position[0] = q[index][0] as f32 / 16.;
            v[index].position[1] = q[index][1] as f32 / 16.;
        }
        // Preserve the CPU bounds check before handing geometry to the host.
        if width == 0 || height == 0 {
            continue;
        }
        result.push(v);
    }
    Ok(result)
}
fn extent(state: &DrawState) -> Result<(usize, usize), String> {
    state.targets.extent.dimensions()
}
fn edge(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> i64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn raster_triangle(
    state: &DrawState,
    mut v: [RasterVertex; 3],
    read: &mut Reader<'_>,
    out: &mut Targets,
) -> Result<(), String> {
    let (width, height) = extent(state)?;
    let mut q = v.map(|v| {
        [
            (v.position[0] * 16.).round() as i64,
            (v.position[1] * 16.).round() as i64,
        ]
    });
    let mut area = edge(q[0], q[1], q[2]);
    if area == 0 {
        return Ok(());
    }
    let cull = state.raster.cull_mode;
    if (cull == CullMode::Front && area > 0) || (cull == CullMode::Back && area < 0) {
        return Ok(());
    }
    if area < 0 {
        q.swap(1, 2);
        v.swap(1, 2);
        area = -area;
    }
    let x0 = (q.iter().map(|p| p[0]).min().unwrap().div_euclid(16)).clamp(0, width as i64) as usize;
    let x1 = ((q.iter().map(|p| p[0]).max().unwrap() + 15).div_euclid(16)).clamp(0, width as i64)
        as usize;
    let y0 =
        (q.iter().map(|p| p[1]).min().unwrap().div_euclid(16)).clamp(0, height as i64) as usize;
    let y1 = ((q.iter().map(|p| p[1]).max().unwrap() + 15).div_euclid(16)).clamp(0, height as i64)
        as usize;
    let edges = [(q[1], q[2]), (q[2], q[0]), (q[0], q[1])];
    let inclusive = edges.map(|(a, b)| b[1] < a[1] || (b[1] == a[1] && b[0] > a[0]));
    for y in y0..y1 {
        for x in x0..x1 {
            let p = [x as i64 * 16 + 8, y as i64 * 16 + 8];
            let weights = edges.map(|(a, b)| edge(a, b, p));
            if weights
                .iter()
                .enumerate()
                .any(|(i, w)| *w < 0 || (*w == 0 && !inclusive[i]))
            {
                continue;
            }
            let b = weights.map(|w| w as f32 / area as f32);
            let depth = v[0].position[2]
                + b[1] * (v[1].position[2] - v[0].position[2])
                + b[2] * (v[2].position[2] - v[0].position[2]);
            let mut t = std::array::from_fn::<_, 3, _>(|i| b[i] * v[i].position[3]);
            let sum = t.iter().sum::<f32>();
            if sum == 0. {
                continue;
            }
            for w in &mut t {
                *w /= sum;
            }
            let color = std::array::from_fn(|c| {
                v[0].color[c]
                    + t[1] * (v[1].color[c] - v[0].color[c])
                    + t[2] * (v[2].color[c] - v[0].color[c])
            });
            let uv = std::array::from_fn(|u| {
                std::array::from_fn(|c| {
                    v[0].uv[u][c]
                        + t[1] * (v[1].uv[u][c] - v[0].uv[u][c])
                        + t[2] * (v[2].uv[u][c] - v[0].uv[u][c])
                })
            });
            let lod = std::array::from_fn(|unit| {
                let texture = &state.textures[unit];
                let scales = [texture.width as f32, texture.height as f32];
                let derivative = |axis| {
                    let db = edges.map(|(a, b)| {
                        if axis == 0 {
                            -(b[1] - a[1]) as f32 * 16. / area as f32
                        } else {
                            (b[0] - a[0]) as f32 * 16. / area as f32
                        }
                    });
                    let ds = (0..3).map(|i| db[i] * v[i].position[3]).sum::<f32>();
                    let d: [f32; 2] = std::array::from_fn(|c| {
                        ((0..3)
                            .map(|i| db[i] * v[i].position[3] * v[i].uv[unit][c])
                            .sum::<f32>()
                            - uv[unit][c] * ds)
                            / sum
                            * scales[c]
                    });
                    d[0] * d[0] + d[1] * d[1]
                };
                let rho = derivative(0).max(derivative(1));
                if rho > 0. {
                    0.5 * libm::log2f(rho)
                } else {
                    -128.
                }
            });
            fragment(state, (x, y, depth), color, uv, lod, read, out)?;
        }
    }
    Ok(())
}
/// The smooth-line distance filter is quantized to eight coverage steps.
pub(super) fn line(
    state: &DrawState,
    outputs: [[Vector; 16]; 2],
    read: &mut Reader<'_>,
    out: &mut Targets,
) -> Result<(), String> {
    if state.raster.line_stipple {
        return Err("DMP line stipple is unsupported".into());
    }
    let mut a = vertex(state, &outputs[0]);
    let mut b = vertex(state, &outputs[1]);
    for p in 0..6 {
        let da = plane(a, p);
        let db = plane(b, p);
        if da < 0. && db < 0. {
            return Ok(());
        }
        if da < 0. {
            a = mix(a, b, da / (da - db));
        } else if db < 0. {
            b = mix(a, b, da / (da - db));
        }
    }
    let a = project(state, a, false);
    let b = project(state, b, false);
    let dx = b.position[0] - a.position[0];
    let dy = b.position[1] - a.position[1];
    let length = libm::sqrtf(dx * dx + dy * dy);
    if length == 0. {
        return Ok(());
    }
    let half = state.raster.line_half_width_32nds as f32 / 32.;
    let smooth = state.raster.line_smooth;
    let radius = half + if smooth { 0.625 } else { 0. };
    let (width, height) = extent(state)?;
    for y in (a.position[1].min(b.position[1]) - radius).floor().max(0.) as usize
        ..(a.position[1].max(b.position[1]) + radius)
            .ceil()
            .min(height as f32) as usize
    {
        for x in (a.position[0].min(b.position[0]) - radius).floor().max(0.) as usize
            ..(a.position[0].max(b.position[0]) + radius)
                .ceil()
                .min(width as f32) as usize
        {
            let px = x as f32 - a.position[0];
            let py = y as f32 - a.position[1];
            let along = (px * dx + py * dy) / length;
            let distance = (px * dy - py * dx).abs() / length;
            let coverage = if smooth {
                let end = (along * std::f32::consts::SQRT_2 + half).clamp(0., 1.)
                    * ((length - along) * 2. - half).clamp(0., 1.);
                ((((radius - distance) * std::f32::consts::FRAC_1_SQRT_2).clamp(0., 1.) * end * 8.)
                    .floor()
                    / 8.)
                    .clamp(0., 1.)
            } else if along >= 0. && along < length && distance <= half {
                1.
            } else {
                0.
            };
            if coverage == 0. {
                continue;
            }
            let t = (along / length).clamp(0., 1.);
            let mut v = mix(a, b, t);
            if state.raster.flat_shading {
                v.color = b.color;
            }
            fragment_coverage_state(
                state,
                (x, y, v.position[2], coverage),
                v.color,
                v.uv,
                [0.; 2],
                read,
                out,
            )?;
        }
    }
    Ok(())
}
pub(super) fn point(
    state: &DrawState,
    output: &[Vector; 16],
    read: &mut Reader<'_>,
    out: &mut Targets,
) -> Result<(), String> {
    let source = vertex(state, output);
    if source.position.iter().any(|v| !v.is_finite()) {
        return Ok(());
    }
    if source.position[0].abs() > 1.
        || source.position[1].abs() > 1.
        || source.position[2] < -1.
        || source.position[2] > 0.
    {
        return Ok(());
    }
    let size = source.position[3].clamp(1., 2048.);
    let v = project(state, source, true);
    let (width, height) = extent(state)?;
    let x0 = (v.position[0] - size * 0.5).ceil().clamp(0., width as f32) as usize;
    let y0 = (v.position[1] - size * 0.5).ceil().clamp(0., height as f32) as usize;
    let x1 = (v.position[0] + size * 0.5).ceil().clamp(0., width as f32) as usize;
    let y1 = (v.position[1] + size * 0.5).ceil().clamp(0., height as f32) as usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let mut uv = v.uv;
            for (unit, coords) in uv.iter_mut().enumerate() {
                if state.raster.point_coord_replace[unit] {
                    *coords = [
                        (x as f32 + 0.5 - v.position[0]) / size + 0.5,
                        (y as f32 + 0.5 - v.position[1]) / size + 0.5,
                    ];
                }
            }
            let lod = std::array::from_fn(|unit| {
                (state.textures[unit].width.max(state.textures[unit].height) as f32 / size)
                    .max(0.0001)
                    .log2()
            });
            fragment(state, (x, y, v.position[2]), v.color, uv, lod, read, out)?;
        }
    }
    Ok(())
}
fn unpack_color(p: &[u8], format: u32) -> Result<Vector, String> {
    let expand = |v: u32, n: u32| v as f32 / ((1 << n) - 1) as f32;
    let word = if p.len() >= 2 {
        u16::from_le_bytes([p[0], p[1]]) as u32
    } else {
        0
    };
    Ok(match format {
        0 => [
            p[3] as f32 / 255.,
            p[2] as f32 / 255.,
            p[1] as f32 / 255.,
            p[0] as f32 / 255.,
        ],
        1 => [
            p[2] as f32 / 255.,
            p[1] as f32 / 255.,
            p[0] as f32 / 255.,
            1.,
        ],
        2 => [
            expand(word >> 11, 5),
            expand(word >> 6 & 31, 5),
            expand(word >> 1 & 31, 5),
            (word & 1) as f32,
        ],
        3 => [
            expand(word >> 11, 5),
            expand(word >> 5 & 63, 6),
            expand(word & 31, 5),
            1.,
        ],
        4 => [
            expand(word >> 12, 4),
            expand(word >> 8 & 15, 4),
            expand(word >> 4 & 15, 4),
            expand(word & 15, 4),
        ],
        5 => [
            p[1] as f32 / 255.,
            p[1] as f32 / 255.,
            p[1] as f32 / 255.,
            p[0] as f32 / 255.,
        ],
        7 => [
            p[0] as f32 / 255.,
            p[0] as f32 / 255.,
            p[0] as f32 / 255.,
            1.,
        ],
        8 => [1., 1., 1., p[0] as f32 / 255.],
        _ => return Err(format!("DMP unsupported texture format {format}")),
    })
}
fn sample_level(
    sampler: &TextureUnit,
    uv: [f32; 2],
    level: u32,
    minifying: bool,
    read: &mut Reader<'_>,
    out: &mut Targets,
) -> Result<Vector, String> {
    if !sampler.wrap_x.is_supported() || !sampler.wrap_y.is_supported() {
        return Err("DMP unsupported texture wrap mode".into());
    }
    let base_width = sampler.width;
    let base_height = sampler.height;
    let width = base_width >> level;
    let height = base_height >> level;
    let bpp = sampler.format.bytes_per_pixel()?;
    let address = if level == 0 {
        sampler.base_address
    } else {
        let mut offset = 0;
        for i in 1..level {
            offset += (base_width >> i) * (base_height >> i) * bpp;
        }
        sampler.mip_address + offset as u64
    };
    if width == 0
        || height == 0
        || !width.is_multiple_of(8)
        || !height.is_multiple_of(8)
        || width * height * bpp > MAX_BYTES
    {
        return Err("DMP invalid sampler extent".into());
    }
    let size = width * height * bpp;
    if !out.textures.contains_key(&(address, size)) {
        let bytes = if let Some(bytes) = out.buffers.get(&address) {
            bytes.clone()
        } else {
            read(address, size)?
        };
        if bytes.len() < size {
            return Err("DMP short texture DMA".into());
        }
        if out.textures.values().map(Vec::len).sum::<usize>() + bytes.len() > MAX_BYTES {
            return Err("DMP texture cache exceeds byte bound".into());
        }
        out.textures.insert((address, size), bytes);
    }
    let bytes = &out.textures[&(address, size)];
    let border = sampler.border_color.map(|v| v as f32 / 255.);
    let unpack_format = sampler.format.unpack_code()?;
    let texel = |x: i32, y: i32| -> Result<Vector, String> {
        if let (Some(x), Some(y)) = (
            sampler.wrap_x.coordinate(x, width),
            sampler.wrap_y.coordinate(y, height),
        ) {
            let i = morton(x, y, width) * bpp;
            unpack_color(&bytes[i..i + bpp], unpack_format)
        } else {
            Ok(border)
        }
    };
    let x = uv[0] * width as f32;
    let y = uv[1] * height as f32;
    let linear = if minifying {
        sampler.minification_linear
    } else {
        sampler.magnification_linear
    };
    if !linear {
        return texel(x.floor() as i32, y.floor() as i32);
    }
    let x = x - 0.5;
    let y = y - 0.5;
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - x.floor();
    let fy = y - y.floor();
    let a = texel(ix, iy)?;
    let b = texel(ix + 1, iy)?;
    let c = texel(ix, iy + 1)?;
    let d = texel(ix + 1, iy + 1)?;
    Ok(std::array::from_fn(|i| {
        (a[i] * (1. - fx) + b[i] * fx) * (1. - fy) + (c[i] * (1. - fx) + d[i] * fx) * fy
    }))
}
fn sample(
    sampler: &TextureUnit,
    uv: [f32; 2],
    lod: f32,
    read: &mut Reader<'_>,
    out: &mut Targets,
) -> Result<Vector, String> {
    let bias = sampler.lod_bias as f32 / 256.;
    let limit = sampler.max_level as u32;
    let l = (lod + bias).clamp(0., limit as f32);
    let minifying = lod + bias > 0.;
    if !sampler.mip_linear {
        return sample_level(sampler, uv, l.round() as u32, minifying, read, out);
    }
    let low = l.floor() as u32;
    let a = sample_level(sampler, uv, low, minifying, read, out)?;
    let high = (low + 1).min(limit);
    if low == high {
        return Ok(a);
    }
    let b = sample_level(sampler, uv, high, minifying, read, out)?;
    Ok(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * l.fract()))
}
fn combine_state(
    state: &DrawState,
    color: Vector,
    textures: [Vector; 2],
) -> Result<Vector, String> {
    // Each stage consumes and produces unsigned eight-bit components.
    let quantize = |v: Vector| v.map(|c| (c.clamp(0., 1.) * 255.).round() as i64);
    let color = color.map(|c| (c.clamp(0., 1.) * 255.) as i64);
    let textures = textures.map(quantize);
    let mut previous = color;
    for stage in state.combiners {
        let constant = stage.constant_color.map(i64::from);
        let source = |code: u8| -> Result<[i64; 4], String> {
            Ok(match code {
                0 => color,
                3 => textures[0],
                4 => textures[1],
                14 => constant,
                15 => previous,
                _ => return Err(format!("DMP unsupported combiner source {code}")),
            })
        };
        let mut rgb = [[0; 4]; 3];
        let mut alpha = [0; 3];
        for i in 0..3 {
            let a = source(stage.rgb_sources[i])?;
            let op = stage.rgb_operands[i];
            if op > 3 {
                return Err("DMP unsupported RGB combiner operand".into());
            }
            rgb[i] = std::array::from_fn(|c| match op {
                0 => a[c],
                1 => 255 - a[c],
                2 => a[3],
                _ => 255 - a[3],
            });
            let a = source(stage.alpha_sources[i])?;
            let op = stage.alpha_operands[i];
            if op > 1 {
                return Err("DMP unsupported alpha combiner operand".into());
            }
            alpha[i] = if op == 0 { a[3] } else { 255 - a[3] };
        }
        // Numerators keep a common denominator until scale and saturation.
        let operation = |op: u8, a: i64, b: i64, c: i64| -> Result<i64, String> {
            Ok(match op {
                0 => a * 255,
                1 => a * b,
                2 => (a + b) * 255,
                3 => (a + b - 128) * 255,
                4 => a * c + b * (255 - c),
                5 => (a - b) * 255,
                6 | 7 => (0..3)
                    .map(|i| (2 * rgb[0][i] - 255) * (2 * rgb[1][i] - 255))
                    .sum(),
                _ => return Err(format!("DMP unsupported combiner operation {op}")),
            })
        };
        let mut result = [0; 4];
        for c in 0..4 {
            let op = if c == 3 {
                stage.alpha_operation
            } else {
                stage.rgb_operation
            };
            let value = if c == 3 {
                operation(op, alpha[0], alpha[1], alpha[2])?
            } else {
                operation(op, rgb[0][c], rgb[1][c], rgb[2][c])?
            };
            let shift = if c == 3 {
                stage.alpha_scale
            } else {
                stage.rgb_scale
            };
            result[c] = ((value * (1 << shift) + 127).div_euclid(255)).clamp(0, 255);
        }
        previous = result;
    }
    Ok(previous.map(|c| c as f32 / 255.))
}

fn fragment(
    state: &DrawState,
    (x, y, z): (usize, usize, f32),
    primary: Vector,
    uv: [[f32; 2]; 2],
    lod: [f32; 2],
    read: &mut Reader<'_>,
    out: &mut Targets,
) -> Result<(), String> {
    fragment_coverage_state(state, (x, y, z, 1.), primary, uv, lod, read, out)
}
#[cfg(test)]
pub(super) fn fragment_coverage(
    engine: &Engine,
    position: (usize, usize, f32, f32),
    primary: Vector,
    uv: [[f32; 2]; 2],
    lod: [f32; 2],
    read: &mut Reader<'_>,
    out: &mut Targets,
) -> Result<(), String> {
    let state = engine.draw_state();
    fragment_coverage_state(&state, position, primary, uv, lod, read, out)
}

fn fragment_coverage_state(
    state: &DrawState,
    (x, y, z, coverage): (usize, usize, f32, f32),
    primary: Vector,
    uv: [[f32; 2]; 2],
    lod: [f32; 2],
    read: &mut Reader<'_>,
    out: &mut Targets,
) -> Result<(), String> {
    let (width, height) = extent(state)?;
    let scissor = state.scissor;
    if x < scissor.left as usize
        || y < scissor.top as usize
        || x > scissor.right as usize
        || y > scissor.bottom as usize
    {
        return Ok(());
    }
    let textures = [
        if state.textures[0].enabled {
            sample(&state.textures[0], uv[0], lod[0], read, out)?
        } else {
            [0.; 4]
        },
        if state.textures[1].enabled {
            sample(&state.textures[1], uv[1], lod[1], read, out)?
        } else {
            [0.; 4]
        },
    ];
    let mut color = combine_state(state, primary, textures)?;
    color[3] *= (coverage * 255.).floor() / 255.;
    let alpha = state.alpha_test;
    if alpha.enabled
        && !alpha.function.passes(
            (color[3].clamp(0., 1.) * 255.) as u32,
            alpha.reference as u32,
        )
    {
        return Ok(());
    }
    let fog = &state.fog;
    if fog.enabled {
        let factor = if fog.table_enabled {
            let position = (z.clamp(0., 1.) * 128.).min(127.9999);
            let word = fog.table[position as usize];
            let slope = (((word & 8191) as i32) << 19) >> 19;
            ((word >> 13 & 2047) as f32 / 2048. + slope as f32 / 2048. * position.fract())
                .clamp(0., 1.)
        } else {
            (z * fog.scale as f32 / 2048. + fog.bias as f32 / 2048.).clamp(0., 1.)
        };
        let fc = fog.color;
        for i in 0..3 {
            color[i] = color[i] * factor + fc[i] as f32 / 255. * (1. - factor);
        }
    }
    let pixel = morton(x, y, width);
    let depth = state.depth_test;
    let stencil = state.stencil_test;
    if stencil.enabled || depth.enabled {
        let bpp = state.targets.depth_format.bytes_per_pixel()?;
        let address = state.targets.depth_address;
        let bytes = out.buffer(address, width * height * bpp, read)?;
        let i = pixel * bpp;
        let old = if bpp == 2 {
            u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap()) as u32
        } else if bpp == 3 {
            u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], 0])
        } else {
            u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap())
        };
        let old_stencil = (old >> 24) as u8;
        let reference = stencil.reference;
        let value_mask = stencil.value_mask;
        let stencil_pass = !stencil.enabled
            || stencil.function.passes(
                (reference & value_mask) as u32,
                (old_stencil & value_mask) as u32,
            );
        let maximum = state.targets.depth_format.depth_maximum()?;
        let new_depth = (z.clamp(0., 1.) * maximum as f32) as u32;
        let depth_pass = !depth.enabled || depth.function.passes(new_depth, old & maximum);
        if stencil.enabled && state.targets.depth_format.has_stencil() {
            let op = if !stencil_pass {
                stencil.operations[0]
            } else if !depth_pass {
                stencil.operations[1]
            } else {
                stencil.operations[2]
            };
            let mask = stencil.write_mask;
            bytes[i + 3] = old_stencil & !mask | op.apply(old_stencil, reference) & mask;
        }
        let stencil_written =
            stencil.enabled && state.targets.depth_format.has_stencil() && stencil.write_mask != 0;
        if !stencil_pass || !depth_pass {
            if stencil_written {
                out.mark_written(address, i + 3, 1);
            }
            return Ok(());
        }
        if depth.write_enabled {
            if bpp == 2 {
                bytes[i..i + 2].copy_from_slice(&(new_depth as u16).to_le_bytes());
            } else {
                bytes[i..i + 3].copy_from_slice(&new_depth.to_le_bytes()[..3]);
            }
        }
        if stencil_written {
            out.mark_written(address, i + 3, 1);
        }
        if depth.write_enabled {
            out.mark_written(address, i, if bpp == 2 { 2 } else { 3 });
        }
    }
    if state.color_mask.0.iter().all(|enabled| !enabled) {
        return Ok(());
    }
    let format = state.targets.color_format;
    let format_code = format.code();
    let sample_format = format.unpack_code()?;
    let bpp = format.bytes_per_pixel()?;
    let address = state.targets.color_address;
    let bytes = out.buffer(address, width * height * bpp, read)?;
    let i = pixel * bpp;
    let destination = unpack_color(&bytes[i..i + bpp], sample_format)?;
    let blend = state.blend;
    let constant = blend.constant_color.map(|v| v as f32 / 255.);
    if !blend.enabled {
        if blend.logic_operation != 3 {
            return Err("DMP unsupported logic operation".into());
        }
    } else {
        let source = color;
        for c in 0..4 {
            let (source_factor, destination_factor, equation) = if c == 3 {
                (
                    blend.source_alpha,
                    blend.destination_alpha,
                    blend.alpha_equation,
                )
            } else {
                (blend.source_rgb, blend.destination_rgb, blend.rgb_equation)
            };
            let a = source[c] * source_factor.factor(c, source, destination, constant)?;
            let b = destination[c] * destination_factor.factor(c, source, destination, constant)?;
            color[c] = equation.apply(a, b, color[c])?;
        }
    }
    for c in 0..4 {
        if !state.color_mask.0[c] {
            color[c] = destination[c];
        }
    }
    if matches!(format, ColorTargetFormat::Rgba8) {
        for c in 0..4 {
            if state.color_mask.0[c] {
                bytes[i + 3 - c] = (color[c].clamp(0., 1.) * 255.).round() as u8;
            }
        }
    } else {
        let quantize = |c: usize, n: u32| (color[c].clamp(0., 1.) * ((1 << n) - 1) as f32) as u16;
        let word = match format_code {
            1 => quantize(0, 5) << 11 | quantize(1, 6) << 5 | quantize(2, 5),
            2 => quantize(0, 5) << 11 | quantize(1, 5) << 6 | quantize(2, 5) << 1 | quantize(3, 1),
            _ => quantize(0, 4) << 12 | quantize(1, 4) << 8 | quantize(2, 4) << 4 | quantize(3, 4),
        };
        let channel_masks: [u16; 4] = match format_code {
            1 => [0xf800, 0x07e0, 0x001f, 0],
            2 => [0xf800, 0x07c0, 0x003e, 1],
            _ => [0xf000, 0x0f00, 0x00f0, 0x000f],
        };
        let bits = (0..4)
            .filter(|c| state.color_mask.0[*c])
            .fold(0, |bits, c| bits | channel_masks[c]);
        let previous = u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap());
        bytes[i..i + 2].copy_from_slice(&((previous & !bits) | (word & bits)).to_le_bytes());
    }
    if matches!(format, ColorTargetFormat::Rgba8) {
        for c in 0..4 {
            if state.color_mask.0[c] {
                out.mark_written(address, i + 3 - c, 1);
            }
        }
    } else {
        out.mark_written(address, i, bpp);
    }
    Ok(())
}

#[cfg(test)]
fn combine(engine: &Engine, color: Vector, textures: [Vector; 2]) -> Result<Vector, String> {
    combine_state(&engine.draw_state(), color, textures)
}

#[cfg(test)]
mod combiner_tests {
    use super::*;
    #[test]
    fn signed_add_scales_and_saturates_each_eight_bit_stage() {
        let mut e = Engine::default();
        for base in [0x2c0, 0x2c8] {
            e.regs.insert(
                base,
                0x00e000e0
                    | if base == 0x2c0 {
                        0x00030003
                    } else {
                        0x000f000f
                    },
            );
            e.regs.insert(base + 2, 0x00030003);
            e.regs.insert(base + 3, 0x20202020);
            e.regs.insert(base + 4, 0x00020002);
        }
        for input in 0..=255 {
            let first = ((input + 32 - 128) * 4i32).clamp(0, 255);
            let second = ((first + 32 - 128) * 4).clamp(0, 255);
            let out = combine(&e, [1.; 4], [[input as f32 / 255.; 4]; 2]).unwrap();
            assert_eq!(out.map(|v| (v * 255.).round() as i32), [second; 4]);
        }
    }
}
