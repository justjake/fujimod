//! DMP SMAPH-H low-opcode command streams, shared by XP4 and XP5.
//! Register windows and physical IRQs are explicit processor inputs.
//! Software raster law covers VG fills/strokes, curves, affine transforms,
//! scissor/alpha and nearest Morton images. DMA latency is unmeasured.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

const COMMAND: u32 = 1 << 31;
const COPY: u32 = 1 << 30;
const FILL: u32 = 1 << 26;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_STREAM: usize = 1024 * 1024;
type Bank = BTreeMap<u16, u32>;
mod geometry;
mod scanline;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Stream {
    regs: Bank,
    gamma: Vec<u8>,
    #[serde(default)]
    gradient_colors: Vec<u32>,
    #[serde(default)]
    gradient_deltas: Vec<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
struct PathDraw {
    sequence: u16,
    regs: Bank,
    rectangles: Vec<[i32; 4]>,
}
#[derive(Clone, Serialize, Deserialize)]
struct FillDraw {
    sequence: u16,
    stream: Stream,
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    regs: BTreeMap<u64, u32>,
    path: Stream,
    fill: Stream,
    path_pending: Option<(Stream, Vec<PathDraw>)>,
    pending: Option<Job>,
    deadline: Option<u64>,
    generation: u64,
    status: u32,
    command_ack: u8,
    jobs: u64,
}
#[derive(Serialize, Deserialize)]
struct Job {
    status: u32,
    writes: Vec<(u64, Vec<u8>)>,
    banks: Option<(Stream, Stream)>,
}
pub struct Renderer {
    state: Arc<Mutex<State>>,
    base: u64,
    irq: u32,
}
impl Renderer {
    pub fn new(base: u64, irq: u32) -> Result<Self, String> {
        if base & 0xfff != 0 || base > 0xffffd000 || !(32..1020).contains(&irq) {
            return Err("Invalid DMP register base or physical IRQ".into());
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State::default())),
            base,
            irq,
        })
    }
}

fn lanes(n: u32) -> u32 {
    (0..4)
        .filter(|i| n & (1 << i) != 0)
        .fold(0, |m, i| m | (255 << (8 * i)))
}
fn get(regs: &Bank, i: u16) -> Result<u32, String> {
    regs.get(&i)
        .copied()
        .ok_or_else(|| format!("DMP missing stream register {i:#x}"))
}
fn words(cx: &Cx<'_>, address: u64, count: usize) -> Result<Vec<u32>, String> {
    if address & 7 != 0 || count == 0 || count > MAX_STREAM / 4 || count & 1 != 0 {
        return Err("DMP command address/length exceeds bounded 8-byte list".into());
    }
    Ok(cx
        .ram_bytes(address, count * 4)?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect())
}

/// Flatten guest-provided ring links. Link length is a byte count;
/// reserve limits, cycles and zero padding are checked before parsing.
fn linked_words(cx: &Cx<'_>, address: u64, count: usize) -> Result<Vec<u32>, String> {
    let mut input = words(cx, address, count)?;
    let mut seen = BTreeSet::from([address]);
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        if pos == input.len() {
            return Ok(out);
        }
        let header = input[pos];
        if header & 7 == 3 {
            if pos + 1 >= input.len() || !input[pos + 2..].iter().all(|w| *w == 0) {
                return Err("DMP ring link has trailing commands or missing address".into());
            }
            let bytes = header & !3;
            let next = input[pos + 1] as u64;
            if bytes & 7 != 0 || !seen.insert(next) || seen.len() > 64 {
                return Err("DMP malformed/cyclic ring link".into());
            }
            input = words(cx, next, bytes as usize / 4)?;
            pos = 0;
            continue;
        }
        let count = if header == 0 {
            0
        } else {
            ((header >> 3) & 511) as usize + 1
        };
        if pos + 1 + count > input.len() || out.len() + 1 + count > MAX_STREAM / 4 {
            return Err("DMP truncated/oversized stream packet".into());
        }
        out.extend_from_slice(&input[pos..pos + 1 + count]);
        pos += 1 + count;
    }
}

fn parse_path(input: &[u32], mut stream: Stream) -> Result<(Stream, Vec<PathDraw>), String> {
    let mut draws = Vec::new();
    let mut rects = Vec::new();
    let mut attr = false;
    let mut pos = 0;
    let mut previous = None;
    while pos < input.len() {
        let h = input[pos];
        pos += 1;
        if h == 0 {
            if input[pos..].iter().any(|w| *w != 0) || input.len() - pos > 1 {
                return Err("DMP interior path padding".into());
            }
            break;
        }
        let n = ((h >> 3) & 511) as usize + 1;
        let v = input.get(pos..pos + n).ok_or("DMP truncated path packet")?;
        pos += n;
        match h & 7 {
            1 if h == 9 && v == [0x07090702, 0] => {
                attr = true;
            }
            2 if h == 0x22 && attr && v[4] == v[2].wrapping_neg() => {
                if rects.len() >= 1024 {
                    return Err("DMP rectangle bound".into());
                }
                rects.push([v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32]);
                attr = false;
            }
            4 | 6 => {
                let index = (h >> 16) as u16;
                let mask = lanes(h >> 12 & 15);
                if mask == 0 {
                    return Err("DMP empty path byte lanes".into());
                }
                for (offset, value) in v.iter().enumerate() {
                    let i = index
                        .checked_add(if h & 7 == 6 { offset as u16 } else { 0 })
                        .ok_or("DMP index overflow")?;
                    if !matches!(i, 2..=8|0x40..=0x85) {
                        return Err(format!("DMP unsupported path register {i:#x}"));
                    }
                    let value =
                        (stream.regs.get(&i).copied().unwrap_or(0) & !mask) | (value & mask);
                    stream.regs.insert(i, value);
                    if i == 0x85 && value != 0xffff {
                        let seq = fence(value, previous)?;
                        previous = Some(seq);
                        if rects.is_empty()
                            && stream.regs.get(&3).is_none_or(|v| v & 0x10000000 == 0)
                        {
                            return Err("DMP path fence without supported geometry".into());
                        }
                        draws.push(PathDraw {
                            sequence: seq,
                            regs: stream.regs.clone(),
                            rectangles: std::mem::take(&mut rects),
                        });
                    }
                }
            }
            _ => return Err(format!("DMP unsupported path packet {h:#010x}")),
        }
    }
    if attr || !rects.is_empty() {
        return Err("DMP unfenced path geometry".into());
    }
    Ok((stream, draws))
}
fn fence(value: u32, previous: Option<u16>) -> Result<u16, String> {
    let seq = value as u16;
    if value >> 16 != 1
        || seq == 0
        || previous.is_some_and(|p| seq != if p == 0xffff { 1 } else { p + 1 })
    {
        return Err("DMP invalid/nonconsecutive sequence fence".into());
    }
    Ok(seq)
}
fn parse_fill(input: &[u32], mut stream: Stream) -> Result<(Stream, Vec<FillDraw>), String> {
    let mut draws = Vec::new();
    let mut pos = 0;
    let mut previous = None;
    let mut pending = None;
    while pos < input.len() {
        let h = input[pos];
        pos += 1;
        if h == 0 {
            if input[pos..].iter().any(|w| *w != 0) || input.len() - pos > 1 {
                return Err("DMP interior fill padding".into());
            }
            break;
        }
        if !matches!(h & 7, 4 | 6) {
            return Err(format!("DMP unsupported fill packet {h:#010x}"));
        }
        let count = ((h >> 3) & 511) as usize + 1;
        let values = input
            .get(pos..pos + count)
            .ok_or("DMP truncated fill packet")?;
        pos += count;
        let index = (h >> 16) as u16;
        let mask = lanes(h >> 12 & 15);
        if mask == 0 {
            return Err("DMP empty fill lanes".into());
        }
        if index == 0x188 && h & 7 == 4 {
            if mask != u32::MAX || count != 256 || values.iter().any(|v| *v > 255) {
                return Err("DMP unknown gamma upload".into());
            }
            stream.gamma = values.iter().map(|v| *v as u8).collect();
            continue;
        }
        if index == 0x16c && h & 7 == 4 {
            // The gradient FIFO holds 128 RGBA colors or 128 signed
            // half-deltas, selected by register 0x16b.
            if mask != u32::MAX || count != 128 {
                return Err("DMP gradient FIFO extent/lanes unsupported".into());
            }
            match get(&stream.regs, 0x16b)? {
                0 => stream.gradient_colors = values.to_vec(),
                0x10000 => stream.gradient_deltas = values.to_vec(),
                _ => return Err("DMP gradient FIFO bank unsupported".into()),
            }
            continue;
        }
        for (offset, value) in values.iter().enumerate() {
            let i = index
                .checked_add(if h & 7 == 6 { offset as u16 } else { 0 })
                .ok_or("DMP fill index overflow")?;
            if i > 0x1ff {
                return Err(format!("DMP fill register outside bank {i:#x}"));
            }
            let value = (stream.regs.get(&i).copied().unwrap_or(0) & !mask) | (value & mask);
            stream.regs.insert(i, value);
            if i == 0xa6 && value != 0 {
                if pending.is_some() {
                    return Err("DMP fill fence before previous draw".into());
                }
                let seq = fence(value, previous)?;
                previous = Some(seq);
                pending = Some(seq);
            }
            if i == 0x1a6 {
                if value != 1 {
                    return Err("DMP unsupported raster trigger".into());
                }
                draws.push(FillDraw {
                    sequence: pending.take().ok_or("DMP draw without fill fence")?,
                    stream: stream.clone(),
                });
            }
        }
    }
    if pending.is_some() {
        return Err("DMP fill fence without draw".into());
    }
    Ok((stream, draws))
}

/// Map pixels through the model's 8x8 Morton layout. Layout 0xd is linear
/// and 0xb is tiled; other selectors are refused.
fn pixel_index(x: usize, y: usize, width: usize, tiled: bool) -> usize {
    if !tiled {
        return y * width + x;
    }
    ((x & 4) << 2)
        + ((x & !7) << 3)
        + ((x * 2) & 4)
        + (x & 1)
        + (y & 4) * 8
        + (y & !7) * width
        + ((y * 2) & 2)
        + ((y & 2) << 2)
}

/// Decode the model's float32-to-float24 sampler fields, not IEEE
/// half-float. Its sign is bit23,7-bit exponent has bias63,16-bit fraction.
fn float24(word: u32) -> Result<f64, String> {
    if word & !0xffffff != 0 {
        return Err("DMP float24 control bits unresolved".into());
    }
    if word & 0x7fffff == 0 {
        return Ok(0.0);
    }
    let bits =
        ((word & 0x800000) << 8) | ((((word >> 16) & 127) + 64) << 23) | ((word & 65535) << 7);
    Ok(f32::from_bits(bits) as f64)
}
struct Texture {
    width: usize,
    height: usize,
    bytes: Vec<u8>,
    matrix: [f64; 9],
    valid: [usize; 2],
    format: u32,
    premultiplied: bool,
    swizzle: u32,
    rotate: bool,
    opaque: bool,
}
struct Gradient {
    matrix: [f64; 9],
    colors: Vec<u32>,
    spread: u32,
    premultiplied: bool,
    active: bool,
    radial: Option<(u32, [f64; 2], f64)>,
}
impl Gradient {
    fn new(stream: &Stream) -> Result<Self, String> {
        let r = &stream.regs;
        // Linear gradients use uploaded linear bytes; radial gradients use
        // uploaded coefficients and projected focal offsets.
        let descriptor = [get(r, 0x168)?, get(r, 0x169)?, get(r, 0x16a)?];
        if stream.gradient_colors.len() != 128 || stream.gradient_deltas.len() != 128 {
            return Err("DMP gradient ramp incomplete".into());
        }
        let control = get(r, 0x16d)?;
        let spread = (control >> 24) & 3;
        if spread == 3 || control & !0x13ffffff != 0 {
            return Err("DMP gradient spread/control unsupported".into());
        }
        let radial = if descriptor == [0, 0, 0] {
            if control & 0xfffffe != 0x3f0000 {
                return Err("DMP linear gradient scale unsupported".into());
            }
            None
        } else {
            if descriptor[0] & !0x31400000 != 0 || descriptor[0] & 0x1000000 == 0 {
                return Err(format!(
                    "DMP radial gradient descriptor unsupported: {descriptor:x?}"
                ));
            }
            Some((
                descriptor[0],
                [float24(descriptor[1])?, float24(descriptor[2])?],
                float24(control & 0xffffff)?,
            ))
        };
        let mut matrix = [0.; 9];
        for (i, v) in matrix.iter_mut().enumerate() {
            *v = float24(get(r, 0x103 + i as u16)?)?;
        }
        Ok(Self {
            matrix,
            colors: stream.gradient_colors.clone(),
            spread,
            premultiplied: get(r, 0x14e)? & 0x20 != 0,
            // Bit 28 selects a nonzero radial projection. The default linear
            // control value 0x3f0001 is also active.
            active: if radial.is_some() {
                control & 0x10000000 != 0
            } else {
                control & 0x10000001 != 0
            },
            radial,
        })
    }
    fn sample(&self, x: usize, y: usize) -> [u8; 4] {
        let px = x as f64 + 0.5;
        let py = y as f64 + 0.5;
        let m = &self.matrix;
        let w = m[6] * px + m[7] * py + m[8];
        let t = if self.active && w != 0. {
            let a = (m[0] * px + m[1] * py + m[2]) / w;
            let b = (m[3] * px + m[4] * py + m[5]) / w;
            if let Some((flags, coefficients, scale)) = self.radial {
                // Focal offsets are already incorporated in the projected
                // rows. Zero-axis flags suppress their linear contribution;
                // Bit 22 enables the quadratic cross term.
                let cross = if flags & 0x400000 != 0 {
                    2. * a * b
                } else {
                    0.
                };
                let root = (coefficients[0] * a * a + coefficients[1] * b * b + cross)
                    .max(0.)
                    .sqrt();
                (root
                    + if flags & 0x10000000 == 0 { a } else { 0. }
                    + if flags & 0x20000000 == 0 { b } else { 0. })
                    * scale
            } else {
                a + b
            }
        } else {
            0.
        };
        let t = match self.spread {
            1 => t.rem_euclid(1.),
            2 => {
                let v = t.rem_euclid(2.);
                if v > 1. { 2. - v } else { v }
            }
            _ => t.clamp(0., 1.),
        };
        // Declared software nearest-ramp sampling; retain uploaded deltas
        // for a later measured interpolation law, as with nearest images.
        let mut c = self.colors[(t * 127.).round() as usize].to_le_bytes();
        if self.premultiplied && c[3] != 0 {
            let alpha = c[3] as u32;
            for channel in &mut c[..3] {
                *channel = (*channel as u32 * 255 / alpha).min(255) as u8;
            }
        }
        c
    }
}
impl Texture {
    fn new(cx: &Cx<'_>, r: &Bank, target: u64, target_bytes: usize) -> Result<Self, String> {
        let control = get(r, 0x156)?;
        let format = control & 15;
        if !matches!(format, 0 | 2 | 3 | 4 | 7 | 8 | 9) {
            return Err("DMP unknown image format/sampler state".into());
        }
        let dims = get(r, 0x152)?;
        let width = (dims >> 16) as usize;
        let height = (dims & 65535) as usize;
        let valid = get(r, 0x158)?;
        let valid = [(valid >> 16) as usize, (valid & 65535) as usize];
        let bpp = match format {
            7 | 8 => 1,
            2..=4 => 2,
            _ => 4,
        };
        if valid[0] > width
            || valid[1] > height
            || width == 0
            || height == 0
            || (width | height) & 7 != 0
            || width * height * bpp > MAX_BYTES - target_bytes
        {
            return Err("DMP unresolved/bounded texture extent".into());
        }
        let address = get(r, 0x155)? as u64;
        let count = width * height * bpp;
        if address & 3 != 0
            || (address < target + target_bytes as u64 && target < address + count as u64)
        {
            return Err("DMP overlapping/unaligned image source".into());
        }
        let mut matrix = [0.0; 9];
        for (i, v) in matrix.iter_mut().enumerate() {
            *v = float24(get(r, 0x10c + i as u16)?)?;
        }
        if matrix[6] != 0.0 || matrix[7] != 0.0 || matrix[8] != 1.0 {
            return Err("DMP image perspective sampling unresolved".into());
        }
        Ok(Self {
            width,
            height,
            bytes: cx.ram_bytes(address, count)?,
            matrix,
            valid,
            format,
            // Format 0 is RGBA and 9 is RGBX; control bit 5 selects
            // premultiplied input. Swizzle is supplied by the descriptor.
            premultiplied: control & 32 != 0,
            swizzle: (control >> 16) & 255,
            rotate: control & 0x100 != 0,
            opaque: format == 9,
        })
    }
    fn sample(&self, x: usize, y: usize) -> Result<[u8; 4], String> {
        let m = self.matrix;
        let px = x as f64 + 0.5;
        let py = y as f64 + 0.5;
        let tx = (m[0] * px + m[1] * py + m[2]) * self.width as f64;
        let ty = (m[3] * px + m[4] * py + m[5]) * self.height as f64;
        // Declared software nearest sampling with transparent image border.
        if tx < 0.0 || ty < 0.0 || tx >= self.valid[0] as f64 || ty >= self.valid[1] as f64 {
            return Ok([0; 4]);
        }
        let cell = pixel_index(tx as usize, ty as usize, self.width, true);
        if self.format == 7 {
            let v = self.bytes[cell];
            return Ok([v, v, v, 255]);
        }
        if self.format == 8 {
            return Ok([255, 255, 255, self.bytes[cell]]);
        }
        if matches!(self.format, 2..=4) {
            let mut word =
                u16::from_le_bytes(self.bytes[cell * 2..cell * 2 + 2].try_into().unwrap());
            let alpha_bits = match self.format {
                2 => 1,
                4 => 4,
                _ => 0,
            };
            if self.rotate {
                word = word.rotate_left(alpha_bits);
            }
            let expand = |v: u16, bits: u32| {
                ((u32::from(v) * 255 + ((1 << bits) - 1) / 2) / ((1 << bits) - 1)) as u8
            };
            let mut color = match self.format {
                2 => [
                    expand(word >> 11, 5),
                    expand((word >> 6) & 31, 5),
                    expand((word >> 1) & 31, 5),
                    expand(word & 1, 1),
                ],
                3 => [
                    expand(word >> 11, 5),
                    expand((word >> 5) & 63, 6),
                    expand(word & 31, 5),
                    255,
                ],
                _ => [
                    expand(word >> 12, 4),
                    expand((word >> 8) & 15, 4),
                    expand((word >> 4) & 15, 4),
                    expand(word & 15, 4),
                ],
            };
            if !matches!(self.swizzle, 0 | 0x1b | 0x93) {
                return Err("DMP packed texture permutation unsupported".into());
            }
            if (self.swizzle == 0x93) ^ self.rotate {
                color.swap(0, 2);
            }
            return Ok(color);
        }
        let i = cell * 4;
        let b = &self.bytes[i..i + 4];
        let mut color = match (self.swizzle, self.rotate) {
            (0 | 0x1b, false) => [b[3], b[2], b[1], b[0]],
            (0 | 0x1b, true) => [b[0], b[1], b[2], b[3]],
            (0x93, false) => [b[1], b[2], b[3], b[0]],
            (0x93, true) => [b[2], b[1], b[0], b[3]],
            _ => return Err("DMP texture byte permutation unsupported".into()),
        };
        if self.opaque {
            color[3] = 255;
        }
        if self.premultiplied && color[3] != 0 {
            let a = color[3] as u32;
            for c in &mut color[..3] {
                *c = (*c as u32 * 255 / a).min(255) as u8;
            }
        }
        Ok(color)
    }
}
fn packed(mut color: [u8; 4], r: &Bank, gamma: &[u8], order: u32) -> Result<[u8; 4], String> {
    if get(r, 0x184)? & 0x1000000 != 0 {
        // The blend descriptor packs four signed Q7 scales followed by four byte-space
        // Q7 biases, two coefficients per register180..183.
        for (n, channel) in color.iter_mut().enumerate() {
            let shift = (n % 2) * 16;
            let scale = (get(r, 0x180 + (n / 2) as u16)? >> shift) as i16 as i32;
            let bias = (get(r, 0x182 + (n / 2) as u16)? >> shift) as i16 as i32;
            *channel = ((i32::from(*channel) * scale + bias + 64) >> 7).clamp(0, 255) as u8;
        }
    }
    if get(r, 0x184)? & 0x10000 != 0 {
        for c in &mut color[..3] {
            *c = gamma[*c as usize];
        }
    }
    if order & 0x100 == 0 {
        let alpha = color[3] as u32;
        for c in &mut color[..3] {
            let v = *c as u32 * alpha + 128;
            *c = ((v + (v >> 8)) >> 8) as u8;
        }
    }
    Ok(if order >> 24 == 1 {
        color
    } else {
        [color[3], color[2], color[1], color[0]]
    })
}

fn blend_pixel(source: [u8; 4], dest: &[u8], order: u32, coverage: f64, mode: u32) -> [u8; 4] {
    if coverage == 1.0 && mode == 0 {
        return source;
    }
    let unpack = |b: &[u8]| {
        if order >> 24 == 1 {
            [b[0], b[1], b[2], b[3]]
        } else {
            [b[3], b[2], b[1], b[0]]
        }
    };
    let src = unpack(&source);
    let dst = unpack(dest);
    let sa = src[3] as f64 / 255.;
    let da = dst[3] as f64 / 255.;
    // The low nibble of the blend mode occupies register 0x1a0 bits 16..23.
    // OpenVG1.1 §13.3: blend premultiplied colors, then apply coverage.
    let alpha = match mode {
        0 => sa,
        2 => sa * (1. - da) + da,
        3 | 4 => sa * da,
        9 => (sa + da).min(1.),
        _ => sa + da - sa * da,
    } * coverage
        + da * (1. - coverage);
    let mut out = [0; 4];
    out[3] = (alpha * 255.).round().clamp(0., 255.) as u8;
    for n in 0..3 {
        let s = src[n] as f64 / 255. * if order & 0x100 != 0 { sa } else { 1. };
        let d = dst[n] as f64 / 255. * if order & 0x100 != 0 { da } else { 1. };
        let blended = match mode {
            0 => s,
            1 => s + d * (1. - sa),
            2 => s * (1. - da) + d,
            3 => s * da,
            4 => d * sa,
            5 => s * (1. - da) + d * (1. - sa) + s * d,
            6 => s + d - s * d,
            7 => s * (1. - da) + d * (1. - sa) + (s * da).min(d * sa),
            8 => s * (1. - da) + d * (1. - sa) + (s * da).max(d * sa),
            9 => (s + d).min(1.),
            _ => unreachable!(),
        };
        let mut v = blended * coverage + d * (1. - coverage);
        if order & 0x100 != 0 {
            v = if alpha > 0. { v / alpha } else { 0. };
        }
        out[n] = (v * 255.).round().clamp(0., 255.) as u8;
    }
    if order >> 24 == 1 {
        out
    } else {
        [out[3], out[2], out[1], out[0]]
    }
}

fn rectangles(
    cx: &mut Cx<'_>,
    paths: &[PathDraw],
    fills: &[FillDraw],
) -> Result<Vec<(u64, Vec<u8>)>, String> {
    if paths.len() != fills.len() || paths.len() > 4096 {
        return Err("DMP unmatched/bounded draw lists".into());
    }
    let mut output: BTreeMap<u64, (u32, u32, Vec<u8>)> = BTreeMap::new();
    for (p, f) in paths.iter().zip(fills) {
        if p.sequence != f.sequence {
            return Err("DMP path/fill sequence mismatch".into());
        }
        let r = &f.stream.regs;
        let dims = get(r, 0x1a3)?;
        let width = dims & 65535;
        let height = dims >> 16;
        let count = width as usize * height as usize * 4;
        let style = get(&p.regs, 0x65)?;
        let mode = (style >> 12) & 3;
        if mode > 2 {
            return Err("DMP unsupported sample mode".into());
        }
        let samples = 1 << mode;
        let pd = get(&p.regs, 0x40)?;
        if width == 0
            || height == 0
            || count > MAX_BYTES
            || pd != (width * samples) | ((height * samples) << 16)
        {
            return Err("DMP invalid framebuffer/path dimensions".into());
        }
        let image = get(r, 0x115)? & 0x1000000 != 0;
        let paint = get(r, 0x140)?;
        let gradient = paint & 0x400 != 0;
        // Register writes update only the low selector lanes. A solid draw
        // can retain the upper210000 image pipeline bank from an earlier draw.
        if !matches!(paint & !0x403, 0x50000 | 0x210000)
            || paint & 3 != if image { 2 } else { 0 }
            || (gradient && image)
        {
            return Err(format!(
                "DMP unsupported paint={:#x} image/scissor={:#x} flags={:#x}",
                get(r, 0x140)?,
                get(r, 0x115)?,
                get(r, 0x184)?
            ));
        }
        if get(r, 0x1a2)? != 0 || get(r, 0x1a1)? != 0 || get(r, 0x1aa)? != 0 {
            return Err("DMP unsupported mask/target format".into());
        }
        let blend = (get(r, 0x1a0)? >> 16) & 255;
        if blend > 9 {
            return Err("DMP unsupported blend mode".into());
        }
        if f.stream.gamma.len() != 256 {
            return Err("DMP incomplete gamma table".into());
        }
        let order = get(r, 0x1a7)?;
        if !matches!(order, 1 | 0x101 | 0x01000001 | 0x01000101) {
            return Err("DMP unresolved target order".into());
        }
        let tiled = match get(r, 0x1b0)? {
            0xb => true,
            0xd => false,
            _ => return Err("DMP target layout unresolved".into()),
        };
        if tiled && (width | height) & 7 != 0 {
            return Err("DMP unaligned tiled target".into());
        }
        let flip = get(r, 0x1b3)?;
        if flip > 1 {
            return Err("DMP target flip invalid".into());
        }
        let band = get(r, 0xa3)?;
        let by = get(r, 0xa4)?;
        let bh = (band >> 16).div_ceil(samples);
        if band & 65535 != width * samples || bh == 0 || by >= height {
            return Err("DMP raster band invalid".into());
        }
        let matrix: Vec<f64> = (0x41..=0x49)
            .map(|i| get(&p.regs, i).map(|v| f32::from_bits(v) as f64))
            .collect::<Result<_, _>>()?;
        if matrix.iter().any(|v| !v.is_finite())
            || matrix[2] != 0.
            || matrix[5] != 0.
            || matrix[8] != 1.
        {
            return Err("DMP non-affine path transform".into());
        }
        let mut contours = if p.rectangles.is_empty() {
            geometry::load(cx, &p.regs)?
        } else {
            p.rectangles
                .iter()
                .map(|r| {
                    let [x, y, w, h] = *r;
                    vec![
                        [x as f64, y as f64],
                        [(x + w) as f64, y as f64],
                        [(x + w) as f64, (y + h) as f64],
                        [x as f64, (y + h) as f64],
                    ]
                })
                .collect()
        };
        // The path matrix is applied unchanged in pixel units.
        // Supersampling changes the raster bands/grid, not this transform.
        for points in &mut contours {
            for p in points {
                let [x, y] = *p;
                *p = [
                    matrix[0] * x + matrix[3] * y + matrix[6],
                    matrix[1] * x + matrix[4] * y + matrix[7],
                ];
            }
        }
        let stroke = p.rectangles.is_empty() && style & 0x100 == 0;
        let radius = if stroke {
            f32::from_bits(get(&p.regs, 0x5c)?) as f64 * matrix[0].hypot(matrix[1])
        } else {
            0.
        };
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for point in contours.iter().flatten() {
            bounds[0] = bounds[0].min(point[0] - radius);
            bounds[1] = bounds[1].min(point[1] - radius);
            bounds[2] = bounds[2].max(point[0] + radius);
            bounds[3] = bounds[3].max(point[1] + radius);
        }
        let x0 = bounds[0].floor().clamp(0., width as f64) as usize;
        let x1 = bounds[2].ceil().clamp(0., width as f64) as usize;
        let y0 = bounds[1].floor().max(by as f64).clamp(0., height as f64) as usize;
        let y1 = bounds[3]
            .ceil()
            .min((by + bh) as f64)
            .clamp(0., height as f64) as usize;
        let mut clips = Vec::new();
        if get(r, 0x115)? & 1 != 0 {
            let n = get(r, 0x100)?;
            if n > 16 {
                return Err("DMP scissor count exceeds16".into());
            }
            for i in 0..n {
                let h = get(r, 0xc0 + i as u16 * 2)?;
                let v = get(r, 0xc1 + i as u16 * 2)?;
                clips.push([h & 65535, v & 65535, (h >> 16) + 1, (v >> 16) + 1]);
            }
        } else {
            clips.push([0, 0, width, height]);
        }
        let address = get(r, 0x1a4)? as u64;
        if !output.contains_key(&address) {
            if output.values().map(|(_, _, b)| b.len()).sum::<usize>() + count > MAX_BYTES {
                return Err("DMP aggregate output too large".into());
            }
            output.insert(address, (width, height, cx.ram_bytes(address, count)?));
        }
        let texture = if image {
            Some(Texture::new(cx, r, address, count)?)
        } else {
            None
        };
        let gradient = if gradient {
            Some(Gradient::new(&f.stream)?)
        } else {
            None
        };
        let (ow, oh, bytes) = output.get_mut(&address).unwrap();
        if (*ow, *oh) != (width, height) {
            return Err("DMP target resized within batch".into());
        }
        let solid = packed(get(r, 0x185)?.to_le_bytes(), r, &f.stream.gamma, order)?;
        for y in y0..y1 {
            let coverage = scanline::coverage(
                &contours,
                x0,
                x1,
                y,
                samples,
                if stroke { Some(radius) } else { None },
                get(r, 0xa5)? & 0x100 != 0,
            );
            for x in x0..x1 {
                if !clips.iter().any(|c| {
                    x as u32 >= c[0] && (x as u32) < c[2] && y as u32 >= c[1] && (y as u32) < c[3]
                }) {
                    continue;
                }
                let covered = coverage[x - x0];
                if covered == 0 {
                    continue;
                }
                let source = if let Some(t) = &texture {
                    packed(t.sample(x, y)?, r, &f.stream.gamma, order)?
                } else if let Some(g) = &gradient {
                    packed(g.sample(x, y), r, &f.stream.gamma, order)?
                } else {
                    solid
                };
                let row = if flip == 1 {
                    height as usize - 1 - y
                } else {
                    y
                };
                let i = pixel_index(x, row, width as usize, tiled) * 4;
                let pixel = blend_pixel(
                    source,
                    &bytes[i..i + 4],
                    order,
                    covered as f64 / (samples * samples) as f64,
                    blend,
                );
                bytes[i..i + 4].copy_from_slice(&pixel);
            }
        }
    }
    Ok(output.into_iter().map(|(a, (_, _, b))| (a, b)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    use fujimod_emulator_devices::test_host::{bus::TestBus, recording::RecordingHost};
    /// The pixmap high-address register beside the renderer reads zero and
    /// refuses nonzero writes, like the XP5 register at the same address.
    struct AddressHigh;
    impl Device for AddressHigh {
        fn name(&self) -> &'static str {
            "dmp-pixmap-high-zero"
        }
        fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
            if address != 0xff90e73c || n != 4 {
                cx.unmodeled();
            }
            0
        }
        fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
            if address != 0xff90e73c || n != 4 {
                cx.unmodeled();
            } else if v != 0 {
                cx.fail("DMP high-address pixmap DMA is unresolved");
            }
        }
    }
    fn register(bus: &mut TestBus, base: u64, size: u64) -> Result<(), String> {
        bus.register(base, size, Box::new(Renderer::new(base, 153)?))?;
        bus.register(0xff90e73c, 4, Box::new(AddressHigh))?;
        Ok(())
    }
    fn put(host: &mut RecordingHost, address: u64, words: &[u32]) {
        host.ram_write(
            address,
            &words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }
    #[test]
    fn pending_dma_has_exact_deadline_early_noop_and_restorable_program_independent_effects() {
        let experiment = |base, irq, pc, core| {
            let mut renderer = Renderer::new(base, irq).unwrap();
            let mut host = RecordingHost {
                now: 100,
                ram_base: 0x1000,
                ram: vec![0xa5; 64],
                ..Default::default()
            };
            for (o, v) in [
                (0x10, 0x1000),
                (0x14, 0x1020),
                (0x18, 0x44332211),
                (0x1c, 0x0f100201),
            ] {
                let mut cx = Cx::new(&mut host, core, pc, 0);
                renderer.write(&mut cx, base + o, 4, v);
                assert!(cx.refusal().is_none(), "{:?}", cx.refusal());
            }
            let before = renderer.save();
            assert_eq!(renderer.next_deadline(host.now), Some(1260));
            let token = host.events[0].2;
            host.now = 1259;
            renderer.event(&mut Cx::new(&mut host, core, pc, 0), token);
            renderer.advance(&mut Cx::new(&mut host, core, pc, 0));
            assert_eq!(renderer.save(), before);
            assert_eq!(host.ram, vec![0xa5; 64]);
            assert!(host.irqs.is_empty());
            let mut restored = Renderer::new(base, irq).unwrap();
            restored.restore(&before).unwrap();
            host.now = 1260;
            renderer.event(&mut Cx::new(&mut host, core, pc, 0), token + 1);
            assert_eq!(renderer.save(), before);
            restored.advance(&mut Cx::new(&mut host, core, pc, 0));
            assert_eq!(&host.ram[..32], &[0x11, 0x22, 0x33, 0x44].repeat(8));
            assert_eq!(&host.ram[32..], &[0xa5; 32]);
            assert_eq!(host.irqs, [(irq, true)]);
            assert_eq!(restored.next_deadline(host.now), None);
            let done = restored.save();
            restored.event(&mut Cx::new(&mut host, core, pc, 0), token);
            assert_eq!(restored.save(), done);
            assert_eq!(host.irqs, [(irq, true)]);
            (done, host.ram)
        };
        assert_eq!(
            experiment(0xff93c000, 153, 0x1000, 0),
            experiment(0xfffd0000, 92, 0x2345678, 3)
        );
    }
    #[test]
    fn linear_gradient_fifo_projection_spread_and_premultiplication() {
        let mut words = vec![0x16bf004, 0, 0x16cf3fc];
        words.extend((0..128).map(|i| u32::from_le_bytes([i * 2, 0, 0, 255])));
        words.extend([0x16bf004, 0x10000, 0x16cf3fc]);
        words.extend([0; 128]);
        let (mut stream, draws) = parse_fill(&words, Stream::default()).unwrap();
        assert!(draws.is_empty());
        for i in 0x103..=0x10b {
            stream.regs.insert(i, 0);
        }
        stream.regs.insert(0x103, 0x3c0000); // x/8, in float24.
        stream.regs.insert(0x10b, 0x3f0000); // homogeneous w=1.
        stream.regs.extend([
            (0x168, 0),
            (0x169, 0),
            (0x16a, 0),
            (0x16d, 0x3f0001),
            (0x14e, 0),
        ]);
        let mut g = Gradient::new(&stream).unwrap();
        assert_eq!(g.sample(0, 0), [16, 0, 0, 255]);
        assert_eq!(g.sample(7, 0), [238, 0, 0, 255]);
        assert_eq!(g.sample(8, 0), [254, 0, 0, 255]);
        g.spread = 1;
        assert_eq!(g.sample(8, 0), [16, 0, 0, 255]);
        g.spread = 2;
        assert_eq!(g.sample(8, 0), [238, 0, 0, 255]);
        g.colors.fill(u32::from_le_bytes([64, 32, 16, 128]));
        g.premultiplied = true;
        assert_eq!(g.sample(0, 0), [127, 63, 31, 128]);
    }
    fn append_packet(words: &mut Vec<u32>, index: u16, lanes: u32, operation: u32, values: &[u32]) {
        assert!(!values.is_empty() && values.len() <= 512);
        words.push(
            (u32::from(index) << 16)
                | (lanes << 12)
                | (((values.len() as u32 - 1) & 511) << 3)
                | operation,
        );
        words.extend_from_slice(values);
    }
    fn path_program(sequence: u16, rect: [i32; 4]) -> Vec<u32> {
        let [x, y, width, height] = rect;
        let mut words = Vec::new();
        append_packet(&mut words, 0x40, 15, 4, &[0x0010_0010]);
        append_packet(
            &mut words,
            0x41,
            15,
            6,
            &[
                1.0f32.to_bits(), 0, 0,
                0, 1.0f32.to_bits(), 0,
                0, 0, 1.0f32.to_bits(),
            ],
        );
        append_packet(&mut words, 0x65, 15, 4, &[0]);
        words.extend([9, 0x0709_0702, 0]);
        words.extend([
            0x22,
            x as u32,
            y as u32,
            width as u32,
            height as u32,
            width.wrapping_neg() as u32,
        ]);
        append_packet(&mut words, 0x85, 15, 4, &[(1 << 16) | u32::from(sequence)]);
        if words.len() & 1 != 0 {
            words.push(0);
        }
        words
    }
    fn fill_program(sequence: u16, image: bool) -> Vec<u32> {
        let mut words = Vec::new();
        append_packet(&mut words, 0x1a3, 3, 4, &[16]);
        append_packet(&mut words, 0x1a3, 12, 4, &[16 << 16]);
        append_packet(&mut words, 0x1a4, 15, 4, &[0x102000]);
        append_packet(
            &mut words,
            0x140,
            15,
            4,
            &[if image { 0x0021_0002 } else { 0x0005_0000 }],
        );
        append_packet(
            &mut words,
            0x115,
            15,
            4,
            &[if image { 0x0100_0000 } else { 0 }],
        );
        for (index, value) in [
            (0x1a0, 0),
            (0x1a1, 0),
            (0x1a2, 0),
            (0x1aa, 0),
            (0x184, 0),
            (0x1a7, 0x101),
            (0x1b0, 0xb),
            (0x1b3, 0),
            (0xa3, (16 << 16) | 16),
            (0xa4, 0),
            (0xa5, 0),
            (0x185, 0),
        ] {
            append_packet(&mut words, index, 15, 4, &[value]);
        }
        let gamma: Vec<u32> = (0..=255).collect();
        append_packet(&mut words, 0x188, 15, 4, &gamma);
        if image {
            append_packet(
                &mut words,
                0x10c,
                15,
                6,
                &[0x3b0000, 0, 0, 0, 0x3b0000, 0, 0, 0, 0x3f0000],
            );
            append_packet(&mut words, 0x152, 15, 4, &[(16 << 16) | 16]);
            append_packet(&mut words, 0x155, 15, 4, &[0x103800]);
            append_packet(&mut words, 0x156, 15, 4, &[0]);
            append_packet(&mut words, 0x158, 15, 4, &[(16 << 16) | 16]);
        }
        append_packet(&mut words, 0xa6, 15, 4, &[(1 << 16) | u32::from(sequence)]);
        append_packet(&mut words, 0x1a6, 15, 4, &[1]);
        if words.len() & 1 != 0 {
            words.push(0);
        }
        words
    }
    fn submit(bus: &mut TestBus, host: &mut RecordingHost, path: &[u32], fill: &[u32], pc: u64) {
        for (offset, value) in [
            (0x10d4, 0x100000),
            (0x10dc, (path.len() / 2) as u32),
            (0x10d0, 1),
            (0x10e4, 0x100800),
            (0x10ec, (fill.len() / 2) as u32),
            (0x10e0, 1),
        ] {
            bus.write(
                &mut Cx::new(host, 0, pc, 0),
                0xff93c000 + offset,
                4,
                u64::from(value),
            )
            .unwrap_or_else(|error| panic!("write {offset:#x} failed: {error}"));
        }
    }
    #[test]
    fn authored_path_and_fill_commands_drive_dma_fences_and_ack() {
        let path = path_program(1, [0, 0, 16, 16]);
        let fill = fill_program(1, false);
        let mut host = RecordingHost {
            ram: vec![0; 0x4000],
            events: Vec::new(),
            irqs: Vec::new(),
            ram_base: 0x100000,
            ..RecordingHost::default()
        };
        put(&mut host, 0x100000, &path);
        put(&mut host, 0x100800, &fill);
        let mut bus = TestBus::default();
        register(&mut bus, 0xff93c000, 0x3000).unwrap();
        submit(&mut bus, &mut host, &path, &fill, 0x1000);
        assert_eq!(host.events.len(), 1);
        assert!(host.irqs.is_empty());
        assert!(host.bytes(0x102000, 1024).unwrap().iter().all(|b| *b == 0));
        let (device, at, token) = host.events.pop().unwrap();
        host.now = at;
        bus.event(&mut Cx::new(&mut host, 0, 0, 0), device, token).unwrap();
        assert!(host.bytes(0x102000, 1024).unwrap().iter().all(|b| *b == 0));
        assert_eq!(host.irqs.last(), Some(&(153, true)));
        for (ack, status) in [(0x1800, COMMAND), (0x1020, 0)] {
            bus.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff93c000 + ack, 4, 0)
                .unwrap();
            assert_eq!(
                bus.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff93c034, 4)
                    .unwrap(),
                status as u64
            );
        }
        assert_eq!(host.irqs.last(), Some(&(153, false)));

        let mut bad_geometry = path.clone();
        bad_geometry[22] = 0;
        assert!(parse_path(&bad_geometry, Stream::default()).is_err());
        assert!(bus
            .write(&mut Cx::new(&mut host, 0, 0, 0), 0xff93cc18, 4, 1)
            .is_err());
        assert!(host.events.is_empty());
        put(&mut host, 0x100000, &[11, 0x100000]);
        assert!(linked_words(&Cx::new(&mut host, 0, 0, 0), 0x100000, 2).is_err());

        let image_path = path_program(2, [0, 0, 16, 16]);
        let image_fill = fill_program(2, true);
        for y in 0..16 {
            for x in 0..16 {
                let i = pixel_index(x, y, 16, true) * 4;
                host.ram_write(0x103800 + i as u64, &[255, 0, y as u8 * 16, x as u8 * 16])
                    .unwrap();
            }
        }
        put(&mut host, 0x100000, &image_path);
        put(&mut host, 0x100800, &image_fill);
        let mut bus = TestBus::default();
        register(&mut bus, 0xff93c000, 0x3000).unwrap();
        submit(&mut bus, &mut host, &image_path, &image_fill, 0x2000);
        let (device, at, token) = host.events.pop().unwrap();
        host.now = at;
        bus.event(&mut Cx::new(&mut host, 0, 0, 0), device, token).unwrap();
        let output = host.bytes(0x102000, 1024).unwrap();
        assert_eq!(output.len(), 1024);
        assert!(output.iter().any(|byte| *byte != 0));
        assert_eq!(float24(0x3b0000).unwrap(), 1.0 / 16.0);
        let mut sample_regs = parse_fill(&image_fill, Stream::default())
            .unwrap()
            .1
            .pop()
            .unwrap()
            .stream
            .regs;
        sample_regs.insert(0x10c, 0x3a0000);
        let texture = Texture::new(
            &Cx::new(&mut host, 0, 0, 0),
            &sample_regs,
            0x102000,
            1024,
        )
        .unwrap();
        assert_eq!(texture.sample(1, 1).unwrap(), [0, 16, 0, 255]);
    }

}

impl State {
    fn r(&self, o: u64) -> u32 {
        self.regs.get(&o).copied().unwrap_or(0)
    }
    fn idle(&self) -> Result<(), String> {
        if self.pending.is_some() || self.status != 0 {
            Err("DMP submission before idle/ISR acknowledgment".into())
        } else {
            Ok(())
        }
    }
    fn begin(&mut self, cx: &mut Cx<'_>, job: Job) {
        let count = job.writes.iter().map(|(_, b)| b.len() as u64).sum::<u64>();
        let Some(at) = cx.now().checked_add(1000 + count * 5) else {
            cx.fail("DMP DMA deadline overflow");
            return;
        };
        self.generation += 1;
        self.pending = Some(job);
        self.deadline = Some(at);
        cx.schedule(at, self.generation);
    }
    fn fill_job(&self, cx: &Cx<'_>, control: u32) -> Result<Job, String> {
        self.idle()?;
        if self.path_pending.is_some() || !matches!(control, 0x0f100201 | 0x0f100001) {
            return Err("DMP unsupported fill control/pending command".into());
        }
        for o in [0x10, 0x14, 0x18] {
            if !self.regs.contains_key(&o) {
                return Err("DMP incomplete fill descriptor".into());
            }
        }
        let start = self.r(0x10) as u64;
        let end = self.r(0x14) as u64;
        let count = end
            .checked_sub(start)
            .filter(|n| *n > 0 && *n <= MAX_BYTES as u64 && *n & 3 == 0)
            .ok_or("DMP fill range exceeds bound")? as usize;
        if start & 3 != 0 {
            return Err("DMP unaligned fill".into());
        }
        cx.ram_bytes(start, count)?;
        let pattern = self.r(0x18).to_le_bytes();
        let mut bytes = vec![0; count];
        for p in bytes.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&pattern);
        }
        Ok(Job {
            status: FILL,
            writes: vec![(start, bytes)],
            banks: None,
        })
    }
    fn copy_job(&self, cx: &Cx<'_>, trigger: u32) -> Result<Job, String> {
        self.idle()?;
        let control = self.r(0xc10);
        let mode = control & 255;
        let format = (control >> 8) & 15;
        if trigger != 1
            || self.path_pending.is_some()
            || !matches!(mode, 1 | 2 | 8)
            || control & !0xffff != 0
            || format != (control >> 12) & 15
            || !matches!(format, 0 | 2 | 3 | 4)
        {
            return Err(format!(
                "DMP unsupported copy trigger{trigger:#x}/control{control:#x}"
            ));
        }
        let source = self.r(0xc00) as u64;
        let dest = self.r(0xc04) as u64;
        // Matching format nibbles select 32-bit,
        //2=RGB565,3=RGBA5551,4=RGBA4444. Uploads retain the pixel word.
        let bpp = if format == 0 { 4 } else { 2 };
        let (count, shape) = if mode == 8 {
            let count = self.r(0xc20) as usize;
            if self.r(0xc24) == 0 || self.r(0xc24) != self.r(0xc28) {
                return Err("DMP unequal linear copy pitches".into());
            }
            (count, None)
        } else {
            let dims = self.r(0xc08);
            let width = (dims & 65535) as usize;
            let height = (dims >> 16) as usize;
            if dims != self.r(0xc0c) || width == 0 || height == 0 || (width | height) & 7 != 0 {
                return Err("DMP copy needs equal 8-aligned extents".into());
            }
            (width * height * bpp, Some((width, height)))
        };
        if count == 0
            || count > MAX_BYTES
            || (source | dest) & 3 != 0
            || (source < dest + count as u64 && dest < source + count as u64)
        {
            return Err("DMP copy range/alignment/overlap invalid".into());
        }
        let input = cx.ram_bytes(source, count)?;
        cx.ram_bytes(dest, count)?;
        let bytes = if let Some((width, height)) = shape {
            let mut bytes = vec![0; count];
            // Control 2 uploads linear pixels into Morton tiles. Control 1
            // reads a native pixmap back with inverted-y word rotation.
            for y in 0..height {
                for x in 0..width {
                    let (i, j) = if mode == 2 {
                        ((y * width + x) * bpp, pixel_index(x, y, width, true) * bpp)
                    } else {
                        (
                            pixel_index(x, y, width, true) * bpp,
                            ((height - 1 - y) * width + x) * bpp,
                        )
                    };
                    if bpp == 4 {
                        let mut p = u32::from_le_bytes(input[i..i + 4].try_into().unwrap());
                        if mode == 1 {
                            p = p.rotate_right(8);
                        }
                        bytes[j..j + 4].copy_from_slice(&p.to_le_bytes());
                    } else {
                        let mut p = u16::from_le_bytes(input[i..i + 2].try_into().unwrap());
                        if mode == 1 {
                            p = p.rotate_right(match format {
                                3 => 1,
                                4 => 4,
                                _ => 0,
                            });
                        }
                        bytes[j..j + 2].copy_from_slice(&p.to_le_bytes());
                    }
                }
            }
            bytes
        } else {
            input
        };
        Ok(Job {
            status: COPY,
            writes: vec![(dest, bytes)],
            banks: None,
        })
    }
}
fn known(o: u64) -> bool {
    matches!(
        o,
        0x10 | 0x14
            | 0x18
            | 0x1c
            | 0x34
            | 0x58
            | 0xc00
            | 0xc04
            | 0xc08
            | 0xc0c
            | 0xc10
            | 0xc18
            | 0xc20
            | 0xc24
            | 0xc28
            | 0x1000
            | 0x1020
            | 0x10a0
            | 0x10c0
            | 0x10c4
            | 0x10d0
            | 0x10d4
            | 0x10dc
            | 0x10e0
            | 0x10e4
            | 0x10ec
            | 0x1200
            | 0x1214
            | 0x1248
            | 0x1298
            | 0x1800
            | 0x1880
            | 0x18c0
            | 0x20e0
            | 0x20e8
            | 0x20f0
    )
}
impl Device for Renderer {
    fn name(&self) -> &'static str {
        "dmp-smaph-h"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(self.base);
        if n != 4 || !known(o) {
            cx.unmodeled();
            return 0;
        }
        let s = self.state.lock().unwrap();
        if o == 0x34 {
            s.status as u64
        } else if o == 0x58 {
            if s.pending.is_some() { FILL as u64 } else { 0 }
        } else {
            s.r(o) as u64
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(self.base);
        if n != 4 || !known(o) {
            cx.unmodeled();
            return;
        }
        let mut s = self.state.lock().unwrap();
        let v = v as u32;
        let result = (|| -> Result<(), String> {
            if matches!(o, 0x58 | 0x1200) || (o == 0x34 && v != 0) {
                return Err("DMP write to read-only status".into());
            }
            if s.pending.is_some() {
                return Err("DMP register mutation during DMA".into());
            }
            match o {
                // An explicit write to status register 0x34 clears it.
                0x34 => {
                    s.status = 0;
                    cx.set_irq(self.irq, false);
                }
                0x1c if v != 0 => {
                    let job = s.fill_job(cx, v)?;
                    s.regs.insert(o, v);
                    s.begin(cx, job);
                }
                0xc18 if v != 0 => {
                    let job = s.copy_job(cx, v)?;
                    s.regs.insert(o, v);
                    s.begin(cx, job);
                }
                0x10d0 if v == 1 => {
                    s.idle()?;
                    if s.path_pending.is_some() {
                        return Err("DMP duplicate path submission".into());
                    }
                    let data = linked_words(cx, s.r(0x10d4) as u64, s.r(0x10dc) as usize * 2)?;
                    let parsed = parse_path(&data, s.path.clone())?;
                    s.path_pending = Some(parsed);
                    s.regs.insert(o, v);
                }
                0x10e0 if v == 1 => {
                    s.idle()?;
                    let data = linked_words(cx, s.r(0x10e4) as u64, s.r(0x10ec) as usize * 2)?;
                    let (fill, draws) = parse_fill(&data, s.fill.clone())?;
                    let (path, paths) = s
                        .path_pending
                        .as_ref()
                        .ok_or("DMP fill submission without path list")?;
                    let writes = rectangles(cx, paths, &draws)?;
                    let job = Job {
                        status: COMMAND,
                        writes,
                        banks: Some((path.clone(), fill)),
                    };
                    s.path_pending = None;
                    s.command_ack = 0;
                    s.regs.insert(o, v);
                    s.begin(cx, job);
                }
                0x10d0 | 0x10e0 | 0x20f0 => {
                    return Err(format!("DMP unsupported command trigger{o:#x}={v:#x}"));
                }
                0x1c | 0xc18 if v == 0 => {
                    s.status &= if o == 0x1c { !FILL } else { !COPY };
                    s.regs.insert(o, v);
                    cx.set_irq(self.irq, s.status != 0);
                }
                0x1800 | 0x1020 if v == 0 => {
                    if s.status & COMMAND != 0 {
                        s.command_ack |= if o == 0x1800 { 1 } else { 2 };
                        if s.command_ack == 3 {
                            s.status &= !COMMAND;
                        }
                    }
                    s.regs.insert(o, v);
                    cx.set_irq(self.irq, s.status != 0);
                }
                0x1800 | 0x1020 | 0x1000 if v != 0 => {
                    return Err("DMP unknown init/reset value".into());
                }
                0x1880 | 0x10a0 if v != 0x12345678 => {
                    return Err("DMP unknown initialization marker".into());
                }
                0x18c0 | 0x10c4 if !matches!(v, 0xffffffff | 0xfffffff0) => {
                    return Err("DMP unknown interrupt/mask setup".into());
                }
                0x10c0 if v != u32::MAX => return Err("DMP unknown init mask".into()),
                0x1248 if v != 1 => return Err("DMP unknown engine enable".into()),
                0x1214 if !matches!(v, 0xffff | 0x100ffff | 0x1000000) => {
                    return Err("DMP unknown list synchronization mode".into());
                }
                0x1298 if v != 0 => return Err("DMP unknown list setup".into()),
                _ => {
                    s.regs.insert(o, v);
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            cx.fail(e);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        let mut s = self.state.lock().unwrap();
        if token != s.generation || s.deadline.is_none_or(|at| cx.now() < at) {
            return;
        }
        s.deadline = None;
        if let Some(job) = s.pending.take() {
            for (a, b) in &job.writes {
                if let Err(e) = cx.ram_write(*a, b) {
                    cx.fail(e);
                    return;
                }
            }
            if let Some((p, f)) = job.banks {
                s.path = p;
                s.fill = f;
            }
            s.status |= job.status;
            s.jobs += 1;
            cx.log(&format!(
                "DMP job{} DMA completed status{:#x},{} destinations",
                s.jobs,
                s.status,
                job.writes.len()
            ));
            cx.set_irq(self.irq, true);
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(&*self.state.lock().unwrap()).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid DMP checkpoint: {e}"))?;
        if state.pending.is_some() != state.deadline.is_some() || state.command_ack > 3 {
            return Err("Invalid DMP pending DMA/checkpoint state".into());
        }
        *self.state.lock().unwrap() = state;
        Ok(())
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.lock().unwrap().deadline
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let generation = self.state.lock().unwrap().generation;
        self.event(cx, generation);
    }
}
