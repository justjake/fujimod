//! Paired path/fill draw batches (`_ui_batch`) and their register decoders.

use super::regs::Registers;
use super::stream::{self, Geometry, StreamDraw, UI_RAMP_CURSOR, UI_RAMP_LANES, UI_RAMP_VALUES};
use fujimod_emulator_devices::record::{Result, Unsupported};

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Unsupported(message.into()))
}

/// Physical address of a guest RAM range (`_ui_ram`).
pub fn ram(address: u64, length: u64, ram_bytes: u64, name: &str) -> Result<u64> {
    let alias = address & 0xc0000000;
    let physical = address - alias;
    if (alias != 0 && alias != 0x40000000)
        || address & 3 != 0
        || length == 0
        || physical + length > ram_bytes
    {
        return fail(format!(
            "Graphics UI {name} has unresolved RAM range/alias/alignment"
        ));
    }
    Ok(physical)
}

/// B90EF4/B6DF2C's sign1/exponent7/mantissa16 number (`_ui_float`).
pub fn float24(word: u64) -> Result<f64> {
    if word & 0xff000000 != 0 || (word >> 16 & 0x7f) == 0x7f {
        return fail("Graphics UI matrix/radius has unsupported float24 encoding");
    }
    let exponent = (word >> 16 & 0x7f) as i32;
    if exponent == 0 {
        if word & 0x7fffff != 0 {
            return fail("Graphics UI float24 denormal is unproved");
        }
        return Ok(0.0);
    }
    let mut value = (1.0 + (word & 0xffff) as f64 / 65536.0) * 2f64.powi(exponent - 63);
    if word & 0x800000 != 0 {
        value = -value;
    }
    if value.abs() > 16777216.0 {
        return fail("Graphics UI float24 exceeds the declared coordinate bound");
    }
    Ok(value)
}

/// An affine matrix (a, b, c, d, e, f): x' = a x + b y + c, y' = d x + e y + f.
pub type Affine = [f64; 6];

pub fn affine(
    registers: &Registers,
    first: u32,
    row_major: bool,
    allow_singular: bool,
) -> Result<Affine> {
    if (0..9).any(|index| !registers.contains(first + index)) {
        return fail("Graphics UI requires a complete matrix state");
    }
    // B6DF2C sets path40's separate high-byte image-transform control.
    if !row_major && registers.at(first) >> 24 > 1 {
        return fail("Graphics UI unknown path matrix control byte");
    }
    let mut values = [0.0f64; 9];
    for index in 0..9u32 {
        let word = registers.at(first + index);
        values[index as usize] = float24(if !row_major && index == 0 {
            word & 0xffffff
        } else {
            word
        })?;
    }
    let [a, b, c, d, e, f, g, h, i] = if row_major {
        values
    } else {
        let [a, d, g, b, e, h, c, f, i] = values;
        [a, b, c, d, e, f, g, h, i]
    };
    if !(g == 0.0 && h == 0.0 && i == 1.0) {
        return fail("Graphics UI requires an affine matrix");
    }
    if !allow_singular && a * e - b * d == 0.0 {
        return fail("Graphics UI requires a nonsingular affine matrix");
    }
    Ok([a, b, c, d, e, f])
}

#[derive(Clone, Debug, PartialEq)]
pub struct RadialGradient {
    pub quadratics: [f64; 2],
    pub zero_x: bool,
    pub zero_y: bool,
    pub negative_denominator: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    /// (a, c, e, f): t = a x + c + e y + f.
    pub coefficients: (f64, f64, f64, f64),
    pub radial: Option<RadialGradient>,
    /// 128 RGBA samples.
    pub colors: Vec<[u8; 4]>,
    pub spread: u32,
    pub premultiplied: bool,
}

/// B7F000 linear/radial coefficients and B7E24C/B7DF90's exact ramp upload.
pub fn gradient(registers: &Registers) -> Result<Gradient> {
    let required = [
        0x168u32, 0x16d, 0x14e, 0x11b, 0x106, 0x107, 0x108, 0x109, 0x10a, 0x10b,
    ];
    if required.iter().any(|k| !registers.contains(*k))
        || !matches!(registers.at(0x16d), 0x10..=0x12)
        || !matches!(registers.at(0x14e), 0 | 0x20)
        || registers.at(0x11b) != 0
    {
        return fail(
            "Graphics UI requires enabled gradient and known spread/premultiplication controls",
        );
    }
    if (0..256).any(|index| registers.get(UI_RAMP_LANES + index) != Some(15)) {
        return fail("Graphics UI gradient requires both complete128-entry ramp banks");
    }
    let colors: Vec<u64> = (0..128)
        .map(|index| registers.at(UI_RAMP_VALUES + index))
        .collect();
    for (index, &current) in colors.iter().enumerate() {
        let following = colors[(index + 1).min(127)];
        let delta: u64 = [0u32, 8, 16, 24]
            .iter()
            .map(|&shift| {
                let difference =
                    ((following >> shift & 255) as i64 - (current >> shift & 255) as i64) & 511;
                ((difference as u64) >> 1) << shift
            })
            .sum();
        if registers.at(UI_RAMP_VALUES + 128 + index as u32) != delta {
            return fail("Graphics UI gradient delta bank disagrees with B7DF90");
        }
    }
    let mut coefficients = [0.0f64; 6];
    for (slot, index) in (0x106u32..0x10c).enumerate() {
        coefficients[slot] = float24(registers.at(index))?;
    }
    if coefficients[1] != 0.0 || coefficients[3] != 0.0 {
        return fail("Graphics UI gradient has unsupported coordinate controls");
    }
    let control = registers.at(0x168);
    let radial = if control == 0 {
        None
    } else {
        if control & !0x131 != 0
            || control & 1 == 0
            || !registers.contains(0x169)
            || !registers.contains(0x16a)
        {
            return fail(
                "Graphics UI radial gradient requires complete quadratic coefficients and known controls",
            );
        }
        Some(RadialGradient {
            quadratics: [float24(registers.at(0x169))?, float24(registers.at(0x16a))?],
            zero_x: control & 0x10 != 0,
            zero_y: control & 0x20 != 0,
            negative_denominator: control & 0x100 != 0,
        })
    };
    Ok(Gradient {
        coefficients: (
            coefficients[0],
            coefficients[2],
            coefficients[4],
            coefficients[5],
        ),
        radial,
        colors: colors
            .iter()
            .map(|w| [*w as u8, (w >> 8) as u8, (w >> 16) as u8, (w >> 24) as u8])
            .collect(),
        spread: (registers.at(0x16d) & 3) as u32,
        premultiplied: registers.at(0x14e) != 0,
    })
}

/// The OpenVG color transform on straight paint colors, per RGBA channel:
/// `c' = clamp(c * scale + bias, 0, 1)`.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorTransform {
    pub scale: [f64; 4],
    pub bias: [f64; 4],
}

/// B7F000 writes the eight clamped transform values as signed halfwords of
/// `value * 128` (B7F000's rounding), low then high in 0x180..0x183: scales
/// R, G, B, A, then biases R, G, B, A already multiplied by 255. The
/// identity transform decodes to `None`.
pub fn color_transform(registers: &Registers) -> Result<Option<ColorTransform>> {
    let mut halves = [0i16; 8];
    for index in 0..4u32 {
        let Some(word) = registers.get(0x180 + index) else {
            return fail("Graphics UI color transform is incomplete");
        };
        if word >> 32 != 0 {
            return fail("Graphics UI color transform word exceeds 32 bits");
        }
        halves[2 * index as usize] = word as u16 as i16;
        halves[2 * index as usize + 1] = (word >> 16) as u16 as i16;
    }
    if halves == [128, 128, 128, 128, 0, 0, 0, 0] {
        return Ok(None);
    }
    Ok(Some(ColorTransform {
        scale: std::array::from_fn(|k| halves[k] as f64 / 128.0),
        bias: std::array::from_fn(|k| halves[k + 4] as f64 / (128.0 * 255.0)),
    }))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub source: u64,
    pub width: u32,
    pub height: u32,
    pub valid_width: i64,
    pub valid_height: i64,
    pub matrix: Affine,
}

/// (physical address, width, height, channel order).
pub type Target = (u64, u32, u32, u32);

#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub target: Target,
    pub sequence: u64,
    pub matrix: Affine,
    pub geometry: Vec<Geometry>,
    pub samples: u32,
    pub clips: Vec<(i64, i64, i64, i64)>,
    pub nonzero: bool,
    pub blend: u32,
    pub color: [u8; 4],
    pub stroke: bool,
    pub texture: Option<Texture>,
    pub gradient: Option<Gradient>,
    pub color_transform: Option<ColorTransform>,
    /// Stroke style (stroke draws only).
    pub radius: f64,
    pub cap: u32,
    pub join: u32,
    pub device_radius: bool,
}

#[derive(Clone)]
pub struct Banks {
    pub path: Registers,
    pub fill: Registers,
}

fn hypot2(x: f64, y: f64) -> f64 {
    fujimod_emulator_devices::math::hypot2(x, y)
}

/// Validate and decode one paired batch (`_ui_batch`).
pub fn batch(
    path_words: &[u32],
    fill_words: &[u32],
    initial_path: Option<&Registers>,
    initial_fill: Option<&Registers>,
    ram_bytes: u64,
    max_bytes: u64,
) -> Result<(Vec<Draw>, Banks)> {
    let (path_draws, path_bank) = stream::parse(path_words, true, initial_path)?;
    let (fill_draws, fill_bank) = stream::parse(fill_words, false, initial_fill)?;
    batch_parsed(
        path_draws, fill_draws, path_bank, fill_bank, ram_bytes, max_bytes,
    )
}

pub fn batch_parsed(
    path_draws: Vec<StreamDraw>,
    fill_draws: Vec<StreamDraw>,
    path_bank: Registers,
    fill_bank: Registers,
    ram_bytes: u64,
    max_bytes: u64,
) -> Result<(Vec<Draw>, Banks)> {
    if path_draws.is_empty() || path_draws.len() != fill_draws.len() || path_draws.len() > 1024 {
        return fail("Graphics UI requires a bounded paired path/fill draw batch");
    }
    let mut draws = Vec::new();
    let mut targets: Vec<(u64, Target)> = Vec::new();
    let (mut texture_ranges, mut scratch_ranges) =
        (Vec::<(u64, u64)>::new(), Vec::<(u64, u64)>::new());
    let mut texture_shapes: Vec<(u64, u32, u32)> = Vec::new();
    for (path, fill) in path_draws.iter().zip(&fill_draws) {
        let (p, f) = (&path.registers, &fill.registers);
        if path.sequence != fill.sequence {
            return fail("Graphics UI path/fill synchronization does not match");
        }
        let required_p = [
            0x60u32, 0x7b, 0x7c, 0x80, 0x81, 0x8b, 0x8c, 0x8e, 0x84, 0x85, 0x86, 0x87, 0x88,
        ];
        let required_f = [
            0xa0u32, 0xa2, 0x115, 0x140, 0x184, 0x185, 0x1a0, 0x1a1, 0x1a2, 0x1a3, 0x1a4, 0x1a5,
            0x1a7, 0x1aa, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8,
        ];
        if required_p.iter().any(|k| !p.contains(*k)) || required_f.iter().any(|k| !f.contains(*k))
        {
            return fail("Graphics UI draw has incomplete framebuffer/path state");
        }
        let (width, height) = (f.at(0x1a3) & 0xffff, f.at(0x1a3) >> 16);
        let frame_bytes = width * height * 4;
        if width == 0
            || height == 0
            || width & 7 != 0
            || height & 7 != 0
            || width * height > 0x100000
            || frame_bytes > max_bytes
            || p.at(0x7b) != (width << 16 | height)
        {
            return fail("Graphics UI framebuffer dimensions exceed supported tiled bounds");
        }
        if f.at(0x1a2) != 0
            || f.at(0x1a1) != 0
            || f.at(0x1aa) != 0
            || !matches!(f.at(0x1a7), 0x100 | 0x101)
        {
            return fail("Graphics UI supports only unmasked format0 RGBA targets");
        }
        let destination = ram(f.at(0x1a4), frame_bytes, ram_bytes, "framebuffer")?;
        let target: Target = (
            destination,
            width as u32,
            height as u32,
            (f.at(0x1a7) & 255) as u32,
        );
        match targets.iter().find(|(key, _)| *key == destination) {
            Some((_, existing)) if *existing != target => {
                return fail("Graphics UI changes dimensions/order of a batch target");
            }
            Some(_) => {}
            None => targets.push((destination, target)),
        }
        let style = p.at(0x60);
        let sample_mode = style & 255;
        if sample_mode > 2 || (style >> 8 & 255) > 1 || style >> 24 > 1 {
            return fail("Graphics UI unknown path/sample mode");
        }
        let samples = 1u64 << sample_mode;
        let padded_height = height + if samples == 1 { height & 8 } else { 0 };
        if p.at(0x80) != (padded_height * samples) << 16
            || p.at(0x8e) != (padded_height * samples) << 16
            || p.at(0x7c) != 1
            || p.at(0x8c) != 1
            || p.at(0x81) != 0x1000101
            || !(f.at(0xa0) & 0xffff == sample_mode || f.at(0xa0) & 0xffff == sample_mode | 0x10)
            || f.at(0xa0) >> 16 != padded_height * samples
            || f.at(0xa2) != width * samples - 1
        {
            return fail("Graphics UI raster/scratch dimensions or draw controls disagree");
        }
        let stride = padded_height * samples * 16;
        let scratch = p.at(0x84);
        if (0..5u32).any(|index| {
            p.at(0x84 + index) != scratch + index as u64 * stride
                || f.at(0xa4 + index) != p.at(0x84 + index)
        }) || p.at(0x8b) != scratch + stride * 5 - 8
        {
            return fail("Graphics UI path/fill scratch banks do not match");
        }
        scratch_ranges.push((ram(scratch, stride * 5, ram_bytes, "scratch")?, stride * 5));
        let matrix = affine(p, 0x40, false, style & 0x100 != 0)?;
        let (paint, gradient) = (f.at(0x140) & 255, f.at(0x140) & 0x400 != 0);
        if !matches!((f.at(0x140) & !0x400) >> 8, 0x500 | 0x2100)
            || !(paint == 0 || paint == 2)
            || (paint != 0 && gradient)
        {
            return fail("Graphics UI unsupported paint/mask controls");
        }
        let expected_flags = if gradient {
            0
        } else if paint == 0 {
            8
        } else {
            10
        };
        if f.at(0x184) != expected_flags && f.at(0x184) != expected_flags | 0x1000000 {
            return fail("Graphics UI unsupported gamma/mask/filter paint controls");
        }
        let color_transform = if f.at(0x184) & 0x1000000 != 0 {
            self::color_transform(f)?
        } else {
            None
        };
        if [
            (0x190u32, 0u64),
            (0x1c0, 0),
            (0x14d, 0),
            (0x144, 0),
            (0x154, 0),
            (0x187, 0),
            (0x178, 1),
            (0x147, 1),
            (0x157, 1),
        ]
        .iter()
        .any(|(k, v)| f.get(*k) != Some(*v))
        {
            return fail("Graphics UI requires known inactive mask/gamma and sampler controls");
        }
        if !gradient && !f.contains(UI_RAMP_CURSOR) && f.get(0x16c) != Some(0x40001) {
            return fail("Graphics UI requires the known inactive ramp/filter initialization");
        }
        let blend = (f.at(0x1a0) >> 16 & 255) as u32;
        if f.at(0x1a0) != (blend as u64) << 16 || blend > 1 || f.at(0x1a5) != (blend != 0) as u64 {
            return fail("Graphics UI supports only SRC and SRC_OVER blend controls");
        }
        let mode = f.at(0x115);
        if !matches!(mode & !1, 0 | 0x10000 | 0x1000000)
            || (mode & 0x1000000 != 0) != (paint != 0)
            || (mode & 0x10000 != 0) != gradient
        {
            return fail("Graphics UI unknown scissor/image mode");
        }
        let mut clips = Vec::new();
        if mode & 1 != 0 {
            let count = f.get(0x100);
            if count.is_none_or(|c| c > 16) {
                return fail("Graphics UI scissor count exceeds the declared bound");
            }
            for index in 0..count.unwrap() as u32 {
                if !f.contains(0xc0 + 2 * index) || !f.contains(0xc1 + 2 * index) {
                    return fail("Graphics UI scissor record is incomplete");
                }
                let (horizontal, vertical) = (f.at(0xc0 + 2 * index), f.at(0xc1 + 2 * index));
                let (x0, y0) = ((horizontal & 0xffff) as i64, (vertical & 0xffff) as i64);
                let (x1, y1) = ((horizontal >> 16) as i64 + 1, (vertical >> 16) as i64 + 1);
                if x0 >= x1 || y0 >= y1 {
                    return fail("Graphics UI scissor has reversed endpoints");
                }
                clips.push((x0, y0, x1.min(width as i64), y1.min(height as i64)));
            }
        } else {
            clips.push((0, 0, width as i64, height as i64));
        }
        let fill_color = f.at(0x185);
        let mut draw = Draw {
            target,
            sequence: path.sequence,
            matrix,
            geometry: path.geometry.clone().unwrap_or_default(),
            samples: samples as u32,
            clips,
            nonzero: f.at(0xa0) & 0x10 != 0,
            blend,
            color: [
                fill_color as u8,
                (fill_color >> 8) as u8,
                (fill_color >> 16) as u8,
                (fill_color >> 24) as u8,
            ],
            stroke: style & 0x100 == 0,
            texture: None,
            gradient: if gradient {
                Some(self::gradient(f)?)
            } else {
                None
            },
            color_transform,
            radius: 0.0,
            cap: 0,
            join: 0,
            device_radius: false,
        };
        if draw.stroke {
            let radius = float24(p.get(0x69).unwrap_or(0))?;
            let (cap, join) = ((style >> 18 & 3) as u32, (style >> 16 & 3) as u32);
            if !(0.0 < radius && radius <= 256.0)
                || cap > 2
                || join > 2
                || p.get(0x6a).unwrap_or(0) != 0
                || style & 0xf00000 != 0
            {
                return fail("Graphics UI unsupported dashed/stroke cap/join/radius state");
            }
            draw.radius = radius;
            draw.cap = cap;
            draw.join = join;
            draw.device_radius = style & 0x1000000 != 0;
            let scale = if draw.device_radius {
                1.0
            } else {
                hypot2(matrix[0], matrix[3]).max(hypot2(matrix[1], matrix[4]))
            };
            if radius * scale > 256.0 {
                return fail("Graphics UI transformed stroke radius exceeds the declared bound");
            }
        }
        if paint != 0 {
            let needed = [0x143u32, 0x153, 0x152, 0x155, 0x156, 0x117, 0x118];
            if needed.iter().any(|k| !f.contains(*k))
                || f.at(0x156) != 0
                || f.at(0x143) != 6
                || f.at(0x153) != 6
                || draw.stroke
            {
                return fail("Graphics UI supports only the bounded format0 image family");
            }
            let (texture_width, texture_height) = (f.at(0x152) >> 16, f.at(0x152) & 0xffff);
            if texture_width == 0
                || texture_height == 0
                || texture_width & 7 != 0
                || texture_height & 7 != 0
                || texture_width * texture_height > 0x100000
                || f.at(0x117) >> 24 > 20
                || f.at(0x118) >> 24 > 20
                || 1u64 << (f.at(0x117) >> 24) != texture_width
                || 1u64 << (f.at(0x118) >> 24) != texture_height
            {
                return fail("Graphics UI texture dimensions/stride/exponents disagree");
            }
            let valid = (
                float24(f.at(0x117) & 0xffffff)? * texture_width as f64,
                float24(f.at(0x118) & 0xffffff)? * texture_height as f64,
            );
            if [valid.0, valid.1]
                .iter()
                .any(|v| v.trunc() != *v || *v <= 0.0)
                || valid.0 > texture_width as f64
                || valid.1 > texture_height as f64
            {
                return fail("Graphics UI texture extent is unresolved");
            }
            let length = texture_width * texture_height * 4;
            let source = ram(f.at(0x155), length, ram_bytes, "texture")?;
            texture_ranges.push((source, length));
            let shape = (source, texture_width as u32, texture_height as u32);
            if !texture_shapes.contains(&shape) {
                texture_shapes.push(shape);
            }
            draw.texture = Some(Texture {
                source,
                width: texture_width as u32,
                height: texture_height as u32,
                valid_width: valid.0 as i64,
                valid_height: valid.1 as i64,
                matrix: affine(f, 0x10c, true, false)?,
            });
        }
        draws.push(draw);
    }
    let target_ranges: Vec<(u64, u64)> = targets
        .iter()
        .map(|(key, t)| (*key, t.1 as u64 * t.2 as u64 * 4))
        .collect();
    let overlap = |a: &(u64, u64), b: &(u64, u64)| a.0 < b.0 + b.1 && b.0 < a.0 + a.1;
    if target_ranges.iter().any(|a| {
        scratch_ranges
            .iter()
            .chain(&texture_ranges)
            .any(|b| overlap(a, b))
    }) {
        return fail("Graphics UI framebuffer overlaps scratch or sampled texture");
    }
    if target_ranges
        .iter()
        .enumerate()
        .any(|(index, a)| target_ranges[index + 1..].iter().any(|b| overlap(a, b)))
    {
        return fail("Graphics UI partially overlapping batch targets are unproved");
    }
    let total: u64 = target_ranges.iter().map(|(_, l)| l).sum::<u64>()
        + texture_shapes
            .iter()
            .map(|(_, w, h)| *w as u64 * *h as u64 * 4)
            .sum::<u64>();
    if total > max_bytes {
        return fail("Graphics UI aggregate destination/texture snapshot exceeds the byte bound");
    }
    Ok((
        draws,
        Banks {
            path: path_bank,
            fill: fill_bank,
        },
    ))
}
