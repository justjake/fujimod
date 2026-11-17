//! Declared still development and scaling of the X-Processor Pro.
//!
//! X-T20 2.11 develops a still in two register-programmed steps (bootemu
//! write log of one JPEG shot, S2 to the end of development):
//!
//! - The development engine 0xfff98000 (GIC 130, step 0x06023320) reads the
//!   RAW at +0x08 (the read-out buffer + 16 lines), stride +0x0c in 16-bit
//!   units, and writes a picture of the size at +0x14 ((lines - 1) << 16 |
//!   (pixels - 1): 4057 x 6033) to +0x1c, stride +0x20 (0xb35a000, 0x1810).
//!   +0x18 (0x1666602) is a mode word whose fields are not decoded.
//! - The resizer 0xfffc3000 reads a packed 4:2:2 picture of the size at +0x08
//!   from +0xc0, stride +0xc4, and writes the size at +0x14 to +0xc8, stride
//!   +0xcc (strides in 16-bit units). +0x18/+0x1c are the horizontal and
//!   vertical steps in source pixels per output pixel: 2^(bits 31..16) x
//!   (bits 15..0) / 0x1000. A shot runs four jobs: the 6000x4000 crop of the
//!   development output into the main picture 0x80000000 (step 1), 6000x2000
//!   (every second line, stride 0x2ee0) to 752x250, 752x250 to 320x213 (13
//!   lines into the 320x240 preview) and 320x240 to 160x120. +0x100 reads
//!   0x11000002 in all of them. Playback magnifies the decoded picture to
//!   the display with 0x11110002, whose two extra bits are not decoded.
//!
//! The JPEG encoder then reads the main picture as packed 4:2:2 (Y0 Y1 Cb
//! Cr). [`PROFILE`] is the declared approximation of both steps, for the
//! declared sensor response of [`crate::imaging::SENSOR_PROFILE`]: each
//! output pixel takes the nearest X-Trans site of each color, the response
//! is inverted to sRGB (black [`crate::imaging::BLACK`], white
//! [`crate::imaging::WHITE`], unity white balance) and encoded as BT.601
//! full-range (JFIF) YCbCr with the pixel pair's chroma averaged. The
//! resizer averages the source pixels each output pixel covers. Lens
//! corrections, noise reduction, sharpening, film simulation, the camera's
//! white balance and tone curve, and the analysis passes (0xfff7e000,
//! 0xfffc4000) are not modeled.
//!
//! With [`FILM_PROFILE`] the development engine instead takes its color
//! from the words the firmware programmed into the still tone unit
//! 0xfffac000, and the stages that unit has no registers for from the color
//! unit 0xfff47000. Both units program the encodings of the X70 film stage
//! ([`fujimod_emulator_imaging::film`]): the still unit holds the brightness
//! table at +0x008, the 3x3 matrix at +0x204 and its three 15-word tone slots
//! at +0x21c/+0x258/+0x294, each slot one output channel (R, G, B, see
//! [`CURVE_SLOT_CHANNELS`]); it has no width, chroma or luma-band words, so
//! those stages (and the live-view curve slots +0x400/+0x43c/+0x478) stay the
//! color unit's. The color unit programs the matrix at +0x300, the brightness
//! table at +0x204, the curve copies at +0x400/+0x43c/+0x478, the Q10 chroma
//! pair at +0x504, the luma band bounds and bands at +0x540 and the curve
//! widths at +0xa58/+0xa5c. A film-simulation diff of two shots (PROVIA and
//! Velvia) changes the still unit's matrix and slots and the color unit's
//! matrix, slots, chroma and bands; a white-balance diff changes the gains at
//! +0x100/+0x104 (and their still-block copy at +0x64/+0x68). Each output
//! pixel takes the nearest site of each color as before, its black-subtracted
//! level is scaled to 14 bits and white balanced, and the film law develops it
//! to Y, Cb and Cr, each color channel through its own curve.
//!
//! White balance is relative, as for the X70's film scene: the declared scene
//! renders neutral at a declared reference of (R, G, B) gains, and the gains
//! the firmware programs into +0x100 (R bits 0-14, G bits 16-30) and +0x104
//! (B bits 0-14) act by their ratio to it, normalized to green. The still
//! block's copy at +0x64/+0x68 carries the same ratios scaled up and clamped,
//! so the color unit's words stay the source. Successive passes of one shot
//! program the same ratios at different scales.

use fujimod_emulator_imaging::film::{Film, FilmPixels};
use std::collections::BTreeMap;

/// Declared development and scaling approximation (see the module notes).
pub const PROFILE: &str = "xpp-declared-xtrans-nearest-srgb-jfif-422-v1";
/// The same, with the color of the programmed still tone and color units and
/// the color unit's white balance.
pub const FILM_PROFILE: &str = "xpp-declared-xtrans-nearest-fff47-film-relative-wb-422-v1";

/// Q12 factors that move the programmed white balance's R and B ratios to
/// green from the declared `reference` gains to the programmed ones.
pub fn white_balance(words: &BTreeMap<u64, u32>, reference: [u32; 3]) -> Result<[u32; 3], String> {
    let word = |o: u64| words.get(&o).copied().unwrap_or(0);
    let gains = [
        word(0x100) & 0x7fff,
        word(0x100) >> 16 & 0x7fff,
        word(0x104) & 0x7fff,
    ];
    if gains.contains(&0) || reference.contains(&0) {
        return Err("Still development before the white balance was programmed".into());
    }
    Ok(std::array::from_fn(|c| {
        let factor = ((u64::from(gains[c]) * u64::from(reference[1])) << 12)
            / (u64::from(gains[1]) * u64::from(reference[c]));
        factor.min(u64::from(u32::MAX)) as u32
    }))
}

/// Color-unit word of each X70 film-stage word the film law reads.
fn color_offset(x70: u64) -> Option<u64> {
    match x70 {
        0x304..=0x32c | 0x400..=0x414 | 0x604 | 0x608 | 0x640..=0x654 => Some(x70 - 0x100),
        0x500 => Some(0xa58),
        0x504 => Some(0xa5c),
        0x508..=0x53c => Some(x70 - 0x108),
        _ => None,
    }
}

/// Still-tone word of each X70 film-stage word the still development reads
/// from the still tone unit 0xfffac000. The unit has no width, chroma or
/// luma-band words: those stages stay the color unit's, as does the white
/// balance (the still block's copy at 0xfffac064/0x68 is the same ratios
/// scaled up and clamped, so the color unit's words keep the declared
/// reference exact).
fn still_offset(x70: u64) -> Option<u64> {
    match x70 {
        0x304..=0x32c => Some(x70 - 0x304 + 0x008), // brightness table, 0xfffac008
        0x400..=0x414 => Some(x70 - 0x400 + 0x204), // 3x3 matrix, 0xfffac204
        _ => None,
    }
}

/// Tone-curve slot of each output channel, in R, G, B order. Slot 0 is the
/// still unit's 0xfffac21c curve, slot 1 0xfffac258, slot 2 0xfffac294; the
/// order is the only place it lives. Slot 1/2/3 = R/G/B is measured for
/// stills (a card test that halved slot 2 and quartered slot 3 moved the
/// JPEG colour centre from 106.4, 107.3, 109.0 to 126.8, 59.0, 0.7); the
/// live-view order is by eye.
pub const CURVE_SLOT_CHANNELS: [usize; 3] = [0, 1, 2];

/// A 14-bit linear level scaled by a Q12 white-balance factor.
pub fn balanced(linear: u16, factor: u32) -> u16 {
    ((u64::from(linear) * u64::from(factor)) >> 12).min(16383) as u16
}

/// The film law of the programmed units: the still tone unit's matrix, curves
/// and brightness table when `still` is given, with widths, chroma, luma bands
/// and white balance from the color unit; without it, the live-view law of the
/// color unit's words, its three slots one per channel.
pub fn film(
    still: Option<&BTreeMap<u64, u32>>,
    words: &BTreeMap<u64, u32>,
) -> Result<Film, String> {
    let live = |o: u64| words.get(&o).copied().unwrap_or(0);
    let still_word = |o: u64| still.and_then(|w| w.get(&o).copied()).unwrap_or(0);
    if (0x300..0x318).step_by(4).all(|o| live(o) == 0) {
        return Err("Still development before the color unit was programmed".into());
    }
    if still.is_some() && (0x21c..0x2cc).step_by(4).all(|o| still_word(o) == 0) {
        return Err("Still development before the still tone unit was programmed".into());
    }
    let mut film = Film::default();
    let word = |x70: u64| match still.and_then(|_| still_offset(x70)) {
        Some(o) => still_word(o),
        None => color_offset(x70).map_or(0, live),
    };
    let points = |channel: usize, i: usize| {
        let slot = CURVE_SLOT_CHANNELS[channel] as u64;
        match still {
            Some(_) => still_word(0x21c + 0x3c * slot + 4 * i as u64),
            None => live(0x400 + 0x3c * slot + 4 * i as u64),
        }
    };
    film.update_slots(|o| word(o as u64), points)?;
    Ok(film)
}

/// X-Trans 6×6 color sites: 0 red, 1 green, 2 blue.
pub const XTRANS: [[u8; 6]; 6] = [
    [0, 2, 1, 2, 0, 1],
    [1, 1, 0, 1, 1, 2],
    [1, 1, 2, 1, 1, 0],
    [2, 0, 1, 0, 2, 1],
    [1, 1, 2, 1, 1, 0],
    [1, 1, 0, 1, 1, 2],
];

/// Picture geometry: lines and pixels of a size word.
pub fn size(word: u32) -> (usize, usize) {
    ((word >> 16) as usize + 1, (word & 0xffff) as usize + 1)
}

/// Source pixels per output pixel of a step word, if it is one.
pub fn step(word: u32) -> Option<f64> {
    let (shift, fraction) = (word >> 16, word & 0xffff);
    (shift <= 8 && fraction != 0).then(|| f64::from(1u32 << shift) * f64::from(fraction) / 4096.)
}

/// (line, sample) offset of the nearest site of each color from each site
/// of the tile.
fn nearest() -> [[[(i64, i64); 3]; 6]; 6] {
    let mut offsets = [[[(0, 0); 3]; 6]; 6];
    for (y, row) in offsets.iter_mut().enumerate() {
        for (x, site) in row.iter_mut().enumerate() {
            for (color, offset) in site.iter_mut().enumerate() {
                let mut best = (i64::MAX, 0, 0);
                for dy in -3i64..=3 {
                    for dx in -3i64..=3 {
                        let (sy, sx) =
                            ((y as i64 + dy).rem_euclid(6), (x as i64 + dx).rem_euclid(6));
                        if usize::from(XTRANS[sy as usize][sx as usize]) == color {
                            best = best.min((dy * dy + dx * dx, dy, dx));
                        }
                    }
                }
                *offset = (best.1, best.2);
            }
        }
    }
    offsets
}

/// sRGB byte of each sample of the declared response.
fn inverse_response() -> Vec<u8> {
    let (black, white) = (
        f64::from(crate::imaging::BLACK),
        f64::from(crate::imaging::WHITE),
    );
    (0..=u16::MAX)
        .map(|sample| {
            let linear = ((f64::from(sample) - black) / (white - black)).clamp(0., 1.);
            let srgb = if linear <= 0.0031308 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1. / 2.4) - 0.055
            };
            (srgb * 255.).round() as u8
        })
        .collect()
}

/// BT.601 full-range (JFIF) Y, Cb and Cr of an sRGB pixel.
fn ycbcr([r, g, b]: [u8; 3]) -> [i32; 3] {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    [
        (19595 * r + 38470 * g + 7471 * b + 32768) >> 16,
        ((-11059 * r - 21709 * g + 32768 * b + 32768) >> 16) + 128,
        ((32768 * r - 27439 * g - 5329 * b + 32768) >> 16) + 128,
    ]
}

fn byte(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// X-T3 5.11 firmware color of a still development (opt-in), read from the
/// words the firmware programs into two parameter pages of the still RECC
/// engine:
///
/// - the tone page (RECC + 0x4000, 0xfff86000): a 3x3 Q10 matrix at +0x204
///   (rows 8 bytes apart, signed 13-bit halves), then three tone curves at
///   +0x224, +0x264 and +0x2a4 (31 points of 14 bits, two a word, one curve
///   per R, G, B output). The thirty segments between the points go three
///   per nibble of +0x21c / +0x220, each 2^nibble wide; only their ratios
///   count, since the 14-bit input spans their sum;
/// - the YCbCr page (RECC + 0x5000, 0xfff87000): the Q10 RGB to YCbCr matrix
///   at +0x204 (nine packed signed 13-bit halves, rows Y, Cb, Cr).
///
/// Declared, not read: the black and white levels ([`crate::imaging::BLACK`],
/// [`crate::imaging::WHITE`]), no white balance, the points joined linearly,
/// a curve's output taken as its top 8 bits and the chroma offset 128. The
/// pages' other units (brightness gain, chroma modes and matrix, luma bands,
/// channel gains, 3D lookup table) are not applied.
#[derive(Clone, Debug, PartialEq)]
pub struct FirmwareColor {
    matrix: [[i32; 3]; 3],
    /// One 14-bit input to byte lookup per output channel.
    curves: [Vec<u8>; 3],
    ycbcr: [[i32; 3]; 3],
}

const TONE_MATRIX: u64 = 0x204;
const TONE_WIDTHS: u64 = 0x21c;
const TONE_CURVES: [u64; 3] = [0x224, 0x264, 0x2a4];
const YCBCR_MATRIX: u64 = 0x204;

fn signed13(v: u32) -> i32 {
    ((v << 19) as i32) >> 19
}

/// Points joined linearly over segments `widths` wide, at `x / 16384`
/// (rounded).
fn joined(points: &[i64], widths: &[i64], x: i64) -> i64 {
    let mut start = 0;
    for (k, &width) in widths.iter().enumerate() {
        let span = width << 14;
        if x < start + span {
            let rise = points[k + 1] - points[k];
            return points[k] + (rise * (x - start) + span / 2).div_euclid(span);
        }
        start += span;
    }
    *points.last().unwrap()
}

impl FirmwareColor {
    /// The law of the tone page's and the YCbCr page's current words (offsets
    /// within each page). A matrix row of zeros, or a zero luma row, refuses:
    /// the pages are not programmed.
    pub fn from_words(
        tone: &BTreeMap<u64, u32>,
        ycbcr: &BTreeMap<u64, u32>,
    ) -> Result<Self, String> {
        let t = |o: u64| tone.get(&o).copied().unwrap_or(0);
        let c = |o: u64| ycbcr.get(&o).copied().unwrap_or(0);
        let matrix: [[i32; 3]; 3] = std::array::from_fn(|row| {
            let o = TONE_MATRIX + 8 * row as u64;
            [signed13(t(o)), signed13(t(o) >> 16), signed13(t(o + 4))]
        });
        let half = |i: usize| signed13(c(YCBCR_MATRIX + 4 * (i / 2) as u64) >> (16 * (i % 2)));
        let ycbcr: [[i32; 3]; 3] =
            std::array::from_fn(|row| std::array::from_fn(|col| half(3 * row + col)));
        if matrix.iter().any(|r| r == &[0; 3]) || ycbcr[0] == [0; 3] {
            return Err("the firmware color pages are not programmed".into());
        }
        let nibbles = u64::from(t(TONE_WIDTHS)) | u64::from(t(TONE_WIDTHS + 4)) << 32;
        let widths: [i64; 30] = std::array::from_fn(|i| 1 << ((nibbles >> (4 * (i / 3))) & 15));
        let domain: i64 = widths.iter().sum();
        let curves = TONE_CURVES.map(|slot| {
            let points: [i64; 31] = std::array::from_fn(|i| {
                i64::from((t(slot + 4 * (i / 2) as u64) >> (16 * (i % 2))) & 16383)
            });
            (0..16384)
                .map(|v| (joined(&points, &widths, v * domain) >> 6).clamp(0, 255) as u8)
                .collect()
        });
        Ok(Self {
            matrix,
            curves,
            ycbcr,
        })
    }
    /// Y, Cb, Cr of a 14-bit linear (R, G, B) level: the matrix, each
    /// output's curve, then the YCbCr matrix.
    pub fn pixel(&self, linear: [u16; 3]) -> [i32; 3] {
        let l = linear.map(|v| i32::from(v.min(16383)));
        let toned: [i32; 3] = std::array::from_fn(|c| {
            let m = self.matrix[c];
            (m[0] * l[0] + m[1] * l[1] + m[2] * l[2] + 512) >> 10
        });
        let rgb: [i32; 3] =
            std::array::from_fn(|c| i32::from(self.curves[c][toned[c].clamp(0, 16383) as usize]));
        std::array::from_fn(|row| {
            let m = self.ycbcr[row];
            let v = (m[0] * rgb[0] + m[1] * rgb[1] + m[2] * rgb[2] + 512) >> 10;
            if row == 0 { v } else { v + 128 }
        })
    }
}

/// Packed 4:2:2 lines (Y0 Y1 Cb Cr, `2 * width` bytes rounded up to a pair)
/// developed from `lines` lines of little-endian u16 X-Trans samples, `stride`
/// bytes apart, whose first sample is the tile site `phase` (line, sample),
/// with the declared sRGB color or, given one, the film law and its white
/// balance factors.
pub fn develop(
    raw: &[u8],
    stride: usize,
    lines: usize,
    width: usize,
    phase: (usize, usize),
    film: Option<(&Film, [u32; 3])>,
) -> Result<Vec<Vec<u8>>, String> {
    develop_with(raw, stride, lines, width, phase, film, None)
}

/// [`develop`] with the X-T3 firmware color (see [`FirmwareColor`]) in place
/// of the declared sRGB; geometry, sampling and packing are the same.
pub fn develop_firmware(
    raw: &[u8],
    stride: usize,
    lines: usize,
    width: usize,
    phase: (usize, usize),
    color: &FirmwareColor,
) -> Result<Vec<Vec<u8>>, String> {
    develop_with(raw, stride, lines, width, phase, None, Some(color))
}

fn develop_with(
    raw: &[u8],
    stride: usize,
    lines: usize,
    width: usize,
    phase: (usize, usize),
    film: Option<(&Film, [u32; 3])>,
    firmware: Option<&FirmwareColor>,
) -> Result<Vec<Vec<u8>>, String> {
    if lines < 6 || width < 6 || stride < 2 * width || raw.len() < stride * (lines - 1) + 2 * width
    {
        return Err("Unsupported still development geometry".into());
    }
    let (offsets, response) = (nearest(), inverse_response());
    let sample = |y: i64, x: i64| {
        // Extend by whole tiles so border sites keep their color.
        let y = if y < 0 {
            y + 6
        } else if y >= lines as i64 {
            y - 6
        } else {
            y
        } as usize;
        let x = if x < 0 {
            x + 6
        } else if x >= width as i64 {
            x - 6
        } else {
            x
        } as usize;
        let i = y * stride + 2 * x;
        u16::from_le_bytes([raw[i], raw[i + 1]])
    };
    let (black, white) = (
        u32::from(crate::imaging::BLACK),
        u32::from(crate::imaging::WHITE),
    );
    // White-balanced 14-bit linear level.
    let factors = film.map_or([1 << 12; 3], |(_, factors)| factors);
    let level = |c: usize, s: u16| {
        let linear = (u32::from(s).saturating_sub(black) * 16383 / (white - black)) as u16;
        balanced(linear, factors[c])
    };
    let mut pixels = film.map(|(film, _)| FilmPixels::new(film));
    let pairs = width.div_ceil(2);
    Ok((0..lines)
        .map(|y| {
            let row = &offsets[(y + phase.0) % 6];
            let mut line = Vec::with_capacity(4 * pairs);
            for pair in 0..pairs {
                let mut out = [[0; 3]; 2];
                for (i, pixel) in out.iter_mut().enumerate() {
                    let x = (2 * pair + i).min(width - 1);
                    let site = &row[(x + phase.1) % 6];
                    let rgb = [0, 1, 2].map(|c| sample(y as i64 + site[c].0, x as i64 + site[c].1));
                    let levels = [level(0, rgb[0]), level(1, rgb[1]), level(2, rgb[2])];
                    *pixel = match (pixels.as_mut(), firmware) {
                        (Some(pixels), _) => pixels.develop(levels).map(i32::from),
                        (None, Some(color)) => color.pixel(levels),
                        (None, None) => ycbcr(rgb.map(|s| response[usize::from(s)])),
                    };
                }
                if pixels.is_some() {
                    let [a, b] = out.map(|p| p.map(|v| v as u8));
                    line.extend_from_slice(&Film::pack(a, b));
                    continue;
                }
                line.extend_from_slice(&[
                    byte(out[0][0]),
                    byte(out[1][0]),
                    byte((out[0][1] + out[1][1] + 1) >> 1),
                    byte((out[0][2] + out[1][2] + 1) >> 1),
                ]);
            }
            line
        })
        .collect())
}

/// Packed 4:2:2 picture geometry of a resizer job.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub stride: usize,
    pub lines: usize,
    pub width: usize,
    pub out_lines: usize,
    pub out_width: usize,
    pub step_x: f64,
    pub step_y: f64,
}

/// Output lines (`2 * out_width` bytes) of a box-filtered packed 4:2:2
/// picture: each output pixel averages the source pixels its steps cover.
/// Rows start every `stride` bytes and are `2 * width` bytes long; width and
/// stride are separate registers, and X-T20 playback programs a width above
/// the stride for pictures whose width is not a multiple of 20 (a 5008-wide
/// still: width 5020 over a 5008-pixel stride; stock 4240x2384 too), so a
/// row's last pixels are read from the start of the next row.
pub fn resample(source: &[u8], s: Scale) -> Result<Vec<Vec<u8>>, String> {
    if s.width < 2
        || s.width & 1 != 0
        || s.out_width & 1 != 0
        || s.out_width == 0
        || s.lines == 0
        || s.out_lines == 0
        || s.stride == 0
        || source.len() < s.stride * (s.lines - 1) + 2 * s.width
    {
        return Err("Unsupported resizer geometry".into());
    }
    let span = |i: usize, step: f64, limit: usize| {
        let start = ((i as f64 * step) as usize).min(limit - 1);
        let end = (((i + 1) as f64 * step) as usize).clamp(start + 1, limit);
        start..end
    };
    if s.width == s.out_width && s.lines == s.out_lines && s.step_x == 1. && s.step_y == 1. {
        return Ok((0..s.lines)
            .map(|y| source[y * s.stride..y * s.stride + 2 * s.width].to_vec())
            .collect());
    }
    let columns: Vec<_> = (0..s.out_width)
        .map(|x| span(x, s.step_x, s.width))
        .collect();
    Ok((0..s.out_lines)
        .map(|y| {
            let rows = span(y, s.step_y, s.lines);
            let mut line = Vec::with_capacity(2 * s.out_width);
            for pair in columns.as_chunks::<2>().0 {
                let mut out = [[0u32; 3]; 2];
                for (pixel, xs) in out.iter_mut().zip(pair) {
                    let mut n = 0;
                    for row in rows.clone() {
                        let line = &source[row * s.stride..];
                        for x in xs.clone() {
                            let p = &line[(x / 2) * 4..(x / 2) * 4 + 4];
                            pixel[0] += u32::from(p[x & 1]);
                            pixel[1] += u32::from(p[2]);
                            pixel[2] += u32::from(p[3]);
                            n += 1;
                        }
                    }
                    for v in pixel.iter_mut() {
                        *v = (*v + n / 2) / n;
                    }
                }
                line.extend_from_slice(&[
                    out[0][0] as u8,
                    out[1][0] as u8,
                    (out[0][1] + out[1][1]).div_ceil(2) as u8,
                    (out[0][2] + out[1][2]).div_ceil(2) as u8,
                ]);
            }
            line
        })
        .collect())
}

/// Bit positions of Y0, Y1, Y2, Y3, Cb and Cr in a Y0L2 block, 10 bits
/// each (Linux drm_fourcc.h, DRM_FORMAT_Y0L2: one little-endian 64-bit word
/// per 2 x 2 pixels, Y0 9..0, Cb 19..10, Y1 29..20, A0 30, A1 31, Y2 41..32,
/// Cr 51..42, Y3 61..52, A2 62, A3 63). Which pixel each luma sample is
/// (here Y0 Y1 the upper line, Y2 Y3 the lower one) is declared.
pub const Y0L2: [u32; 6] = [0, 20, 32, 52, 10, 42];

/// Y, Cb and Cr of pixel `x` of the upper (`line` 0) or lower line of a row
/// of Y0L2 blocks.
pub fn y0l2_sample(row: &[u8], x: usize, line: usize) -> [u16; 3] {
    let at = x / 2 * 8;
    let block = u64::from_le_bytes(row[at..at + 8].try_into().expect("8 bytes"));
    let field = |bit: u32| ((block >> bit) & 0x3ff) as u16;
    [
        field(Y0L2[line * 2 + x % 2]),
        field(Y0L2[4]),
        field(Y0L2[5]),
    ]
}

/// A Y0L2 block from its four luma samples (upper line, then lower) and its
/// chroma; the alpha bits are 0.
pub fn y0l2_block(luma: [u16; 4], cb: u16, cr: u16) -> u64 {
    luma.into_iter()
        .chain([cb, cr])
        .zip(Y0L2)
        .fold(0, |block, (value, bit)| {
            block | u64::from(value & 0x3ff) << bit
        })
}

/// How a picture's lines sit in its pitch rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YcLayout {
    /// One packed 4:2:2 8-bit line (Y0 Y1 Cb Cr per pixel pair) per row.
    Lines,
    /// Two lines per row as Y0L2 blocks of 10-bit samples.
    Y0l2,
}

impl YcLayout {
    /// Picture lines per pitch row.
    pub fn lines(self) -> usize {
        match self {
            Self::Lines => 1,
            Self::Y0l2 => 2,
        }
    }
}

/// 10-bit Y, Cb and Cr of the `width` pixels of a line taken by nearest
/// neighbour from a packed 4:2:2 8-bit line of `window` pixels (as
/// [`develop`] writes them): pixel x reads pixel x * window / width and the
/// chroma of its pair; samples are widened by << 2.
pub fn scaled_line(line: &[u8], window: usize, width: usize) -> Vec<[u16; 3]> {
    (0..width)
        .map(|x| {
            let s = x * window / width;
            let p = s / 2 * 4;
            [line[p + s % 2], line[p + 2], line[p + 3]].map(|b| u16::from(b) << 2)
        })
        .collect()
}

/// One pitch row of `layout` from its upper line and, for Y0L2, its lower
/// one (10-bit samples; packed lines narrow them by >> 2). A pixel pair, or
/// a Y0L2 block, takes the chroma of its upper left pixel.
pub fn yc_row(layout: YcLayout, upper: &[[u16; 3]], lower: &[[u16; 3]]) -> Vec<u8> {
    match layout {
        YcLayout::Lines => upper
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0][0], p[1][0], p[0][1], p[0][2]].map(|v| (v >> 2) as u8))
            .collect(),
        YcLayout::Y0l2 => upper
            .as_chunks::<2>()
            .0
            .iter()
            .zip(lower.as_chunks::<2>().0)
            .flat_map(|(u, d)| {
                y0l2_block([u[0][0], u[1][0], d[0][0], d[1][0]], u[0][1], u[0][2]).to_le_bytes()
            })
            .collect(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The two color pages' words as the X-T3 5.11 firmware programmed them
    /// for the still of a bring-up capture run (offsets within each page).
    pub(crate) fn firmware_units() -> (BTreeMap<u64, u32>, BTreeMap<u64, u32>) {
        let curve = [
            0x0443_0000,
            0x058d_048c,
            0x0780_0699,
            0x0a7c_0878,
            0x0e76_0c99,
            0x155e_1244,
            0x1da3_1811,
            0x2540_21db,
            0x2b64_28aa,
            0x3009_2dad,
            0x336d_31c3,
            0x38dc_369b,
            0x3bdb_3add,
            0x3da6_3cca,
            0x3f4b_3e82,
            0x0000_3fff,
        ];
        let mut tone: BTreeMap<u64, u32> = [
            (0x200, 0x0101_0100),
            (0x204, 0x1fb8_041b),
            (0x208, 0x0000_002d),
            (0x20c, 0x0470_1fc5),
            (0x210, 0x0000_1fcb),
            (0x214, 0x1f36_0049),
            (0x218, 0x0000_0481),
            (0x21c, 0x7666_5433),
            (0x220, 0x0000_0066),
        ]
        .into();
        for slot in [0x224, 0x264, 0x2a4] {
            for (i, w) in curve.iter().enumerate() {
                tone.insert(slot + 4 * i as u64, *w);
            }
        }
        let ycbcr = [
            (0x200, 0x3000_1515),
            (0x204, 0x0259_0132),
            (0x208, 0x1f53_0075),
            (0x20c, 0x0200_1ead),
            (0x210, 0x1e53_0200),
            (0x214, 0x0000_1fad),
        ]
        .into();
        (tone, ycbcr)
    }

    #[test]
    fn the_firmware_color_units_decode_as_the_shot_programs_them() {
        let (tone, ycbcr) = firmware_units();
        let color = FirmwareColor::from_words(&tone, &ycbcr).unwrap();
        assert_eq!(
            color.matrix,
            [[1051, -72, 45], [-59, 1136, -53], [73, -202, 1153]]
        );
        assert_eq!(
            color.ycbcr,
            [[306, 601, 117], [-173, -339, 512], [512, -429, -83]]
        );
        for curve in &color.curves {
            assert_eq!(curve.len(), 16384);
            assert!(curve.windows(2).all(|w| w[0] <= w[1]));
            // Three segments 2^n wide per nibble 3 3 4 5 6 6 6 7 6 6 sum to
            // 1536: input 256 (x 24) is point 3 (1421), input 8192 (x 768)
            // point 21 (13165), as 14-bit levels' top bytes.
            assert_eq!(
                (curve[0], curve[256], curve[8192], curve[16383]),
                (0, 22, 205, 255)
            );
        }
        // Between knots the points are joined linearly: input 128 (x 12) is
        // halfway from point 1 to point 2.
        let point = |i: u64| i64::from((tone[&(0x224 + 4 * (i / 2))] >> (16 * (i % 2))) & 16383);
        assert_eq!(
            i64::from(color.curves[0][128]),
            ((point(1) + point(2) + 1) / 2) >> 6
        );
        // Rows of the matrix sum to 1024, so grey stays grey.
        assert_eq!(color.pixel([16383; 3]), [255, 128, 128]);
        assert_eq!(color.pixel([0; 3]), [0, 128, 128]);
        let grey = color.pixel([4096; 3]);
        assert_eq!(grey[1..], [128, 128]);
        // A red level reads red: Cr above, Cb below the neutral 128.
        let red = color.pixel([8192, 2048, 2048]);
        assert!(red[2] > 140 && red[1] < 128, "{red:?}");
        // Words are read, not built in: a halved red curve darkens red.
        let mut half = tone.clone();
        for i in 0..16u64 {
            let w = half[&(0x224 + 4 * i)];
            half.insert(0x224 + 4 * i, (w >> 1) & 0x7fff_7fff);
        }
        let darker = FirmwareColor::from_words(&half, &ycbcr)
            .unwrap()
            .pixel([8192, 2048, 2048]);
        assert!(darker[2] < red[2], "{darker:?} {red:?}");
        assert!(FirmwareColor::from_words(&BTreeMap::new(), &ycbcr).is_err());
        assert!(FirmwareColor::from_words(&tone, &BTreeMap::new()).is_err());
    }

    #[test]
    fn a_uniform_grey_develops_neutral_through_the_firmware_color() {
        let (tone, ycbcr) = firmware_units();
        let color = FirmwareColor::from_words(&tone, &ycbcr).unwrap();
        let (black, white) = (crate::imaging::BLACK, crate::imaging::WHITE);
        let grey = black + (white - black) / 4;
        let (lines, width, stride) = (12, 12, 32);
        let mut raw = vec![0; stride * lines];
        for y in 0..lines {
            for x in 0..width {
                raw[y * stride + 2 * x..][..2].copy_from_slice(&grey.to_le_bytes());
            }
        }
        let out = develop_firmware(&raw, stride, lines, width, (2, 2), &color).unwrap();
        assert_eq!((out.len(), out[0].len()), (12, 24));
        let linear = ((u32::from(grey - black)) * 16383 / u32::from(white - black)) as u16;
        let [y, ..] = color.pixel([linear; 3]);
        for line in &out {
            for p in line.as_chunks::<4>().0 {
                assert_eq!(*p, [y as u8, y as u8, 128, 128]);
            }
        }
        // The declared path of the same RAW is untouched by the option.
        let declared = develop(&raw, stride, lines, width, (2, 2), None).unwrap();
        assert_ne!(declared, out);
        assert!(develop_firmware(&raw, stride, lines, width + 8, (2, 2), &color).is_err());
    }

    /// drm_fourcc.h's layout: Y0 1, Cb 5, Y1 2, Y2 3, Cr 6, Y3 4.
    #[test]
    fn y0l2_blocks_hold_the_drm_fourcc_bit_positions() {
        let block = y0l2_block([1, 2, 3, 4], 5, 6);
        assert_eq!(block, 1 | 5 << 10 | 2 << 20 | 3 << 32 | 6 << 42 | 4 << 52);
        assert_eq!(y0l2_block([0x7ff; 4], 0x7ff, 0x7ff) >> 30 & 3, 0, "alpha 0");
        let row = [
            block.to_le_bytes(),
            y0l2_block([7, 8, 9, 10], 11, 12).to_le_bytes(),
        ]
        .concat();
        let samples: Vec<[u16; 3]> = (0..4)
            .flat_map(|x| [y0l2_sample(&row, x, 0), y0l2_sample(&row, x, 1)])
            .collect();
        assert_eq!(
            samples,
            [
                [1, 5, 6],
                [3, 5, 6],
                [2, 5, 6],
                [4, 5, 6],
                [7, 11, 12],
                [9, 11, 12],
                [8, 11, 12],
                [10, 11, 12]
            ]
        );
    }

    #[test]
    fn yc_rows_pack_lines_or_y0l2_blocks_from_scaled_lines() {
        // 8 pixels: luma 10 x pixel, Cb the pair, Cr 200 - the pair.
        let line: Vec<u8> = (0..4u8)
            .flat_map(|p| [20 * p, 20 * p + 10, p, 200 - p])
            .collect();
        let scaled = scaled_line(&line, 8, 6);
        let lumas: Vec<u16> = scaled.iter().map(|s| s[0] >> 2).collect();
        assert_eq!(lumas, [0, 10, 20, 40, 50, 60]);
        assert_eq!(scaled[3][1..], [2 << 2, 198 << 2]);
        let packed = yc_row(YcLayout::Lines, &scaled, &[]);
        assert_eq!(packed[..8], [0, 10, 0, 200, 20, 40, 1, 199]);
        let lower = scaled_line(&line, 8, 6);
        let tiled = yc_row(YcLayout::Y0l2, &scaled, &lower);
        assert_eq!(tiled.len(), 3 * 8);
        assert_eq!(y0l2_sample(&tiled[8..], 1, 1), [40 << 2, 1 << 2, 199 << 2]);
        assert_eq!(YcLayout::Y0l2.lines(), 2);
    }

    #[test]
    fn size_and_step_words_decode_as_the_shot_programs_them() {
        assert_eq!(size(0xfd8_1790), (4057, 6033));
        assert_eq!(size(0xf9f_176f), (4000, 6000));
        assert_eq!(step(0x1000), Some(1.));
        assert_eq!(step(0x1_1000), Some(2.));
        assert!((step(0x2_2005).unwrap() - 8.0049).abs() < 1e-3);
        assert_eq!(step(0x2_0000), None);
    }

    #[test]
    fn a_uniform_color_develops_to_its_jfif_ycbcr() {
        // Linear red 1, green 0.5, blue 0 through the declared response.
        let (black, white) = (crate::imaging::BLACK, crate::imaging::WHITE);
        let level = |c: u8| match c {
            0 => white,
            1 => black + ((0.5 * f64::from(white - black)) as u16),
            _ => black,
        };
        let (lines, width, stride) = (12, 12, 32);
        let mut raw = vec![0; stride * lines];
        for y in 0..lines {
            for x in 0..width {
                let v = level(XTRANS[(y + 2) % 6][(x + 2) % 6]);
                raw[y * stride + 2 * x..][..2].copy_from_slice(&v.to_le_bytes());
            }
        }
        let out = develop(&raw, stride, lines, width, (2, 2), None).unwrap();
        assert_eq!((out.len(), out[0].len()), (12, 24));
        let [y, cb, cr] = ycbcr([255, 188, 0]);
        for line in &out {
            for p in line.as_chunks::<4>().0 {
                assert_eq!(*p, [y as u8, y as u8, cb as u8, cr as u8]);
            }
        }
        assert!(develop(&raw, stride, lines, width + 8, (2, 2), None).is_err());
    }

    #[test]
    fn the_wrong_tile_phase_mixes_the_colors() {
        let (lines, width, stride) = (12, 12, 24);
        let mut raw = vec![0; stride * lines];
        for y in 0..lines {
            for x in 0..width {
                let v = if XTRANS[y % 6][x % 6] == 0 {
                    crate::imaging::WHITE
                } else {
                    0
                };
                raw[y * stride + 2 * x..][..2].copy_from_slice(&v.to_le_bytes());
            }
        }
        let red = develop(&raw, stride, lines, width, (0, 0), None).unwrap();
        assert!(red.iter().flatten().skip(3).step_by(4).all(|&cr| cr == 255));
        let shifted = develop(&raw, stride, lines, width, (0, 1), None).unwrap();
        assert!(
            shifted
                .iter()
                .flatten()
                .skip(3)
                .step_by(4)
                .any(|&cr| cr != 255)
        );
    }

    #[test]
    fn the_resizer_copies_at_step_one_and_averages_when_scaling() {
        let s = Scale {
            stride: 16,
            lines: 2,
            width: 4,
            out_lines: 2,
            out_width: 4,
            step_x: 1.,
            step_y: 1.,
        };
        let mut source = vec![0; 32];
        source[..8].copy_from_slice(&[10, 20, 100, 200, 30, 40, 110, 210]);
        source[16..24].copy_from_slice(&[50, 60, 120, 220, 70, 80, 130, 230]);
        let copy = resample(&source, s).unwrap();
        assert_eq!(copy, vec![source[..8].to_vec(), source[16..24].to_vec()]);
        let half = resample(
            &source,
            Scale {
                out_lines: 1,
                out_width: 2,
                step_x: 2.,
                step_y: 2.,
                ..s
            },
        )
        .unwrap();
        // Y: (10 + 20 + 50 + 60) / 4 and (30 + 40 + 70 + 80) / 4; chroma of all four pixels each.
        assert_eq!(half, vec![vec![35, 55, 115, 215]]);
        assert!(resample(&source, Scale { stride: 0, ..s }).is_err());
    }

    /// Color-unit words of an emulated X-T20 2.11 PROVIA shot (bootemu).
    pub(crate) fn provia() -> BTreeMap<u64, u32> {
        let mut words = BTreeMap::new();
        let groups: [(u64, &[u32]); 6] = [
            (
                0x204,
                &[
                    0x2fe026c, 0x362033e, 0x37c0374, 0x37e037e, 0x36a0372, 0x34e0356, 0x3240340,
                    0x3000312, 0x2d602dc, 0x2a002c8, 0x200022e,
                ],
            ),
            (
                0x300,
                &[0x1fa90409, 0x4e, 0x4831fb8, 0x1fc5, 0x1eed004c, 0x4c7],
            ),
            (
                0x400,
                &[
                    0xc50000, 0x47203bb, 0x66b056d, 0x99c076d, 0xdf50be8, 0x15971291, 0x1dc2185f,
                    0x253f21c8, 0x2b6528a9, 0x30092daf, 0x336b31c2, 0x38da36a2, 0x3cd83ade,
                    0x3fff3e9a, 0x3fff0000,
                ],
            ),
            (0x504, &[0x1fd60434, 0x6f0577]),
            (
                0x540,
                &[
                    0xe6c08040, 0xd00080, 0xe00040, 0x1000000, 0x13007c0, 0x1dc0700,
                ],
            ),
            (0xa58, &[0x65554322, 6]),
        ];
        for (start, group) in groups {
            for (i, &w) in group.iter().enumerate() {
                words.insert(start + 4 * i as u64, w);
            }
        }
        for copy in [0x43c, 0x478] {
            for i in 0..15 {
                words.insert(copy + 4 * i, words[&(0x400 + 4 * i)]);
            }
        }
        words
    }

    /// The still tone block (0x1a51edc / 0xfffac000 offsets) a PROVIA still
    /// programs: brightness table, matrix, and the three curve slots. The
    /// words are the color unit's measured copies at their still offsets.
    pub(crate) fn still() -> BTreeMap<u64, u32> {
        let live = provia();
        let mut words = BTreeMap::new();
        for i in 0..11 {
            words.insert(0x008 + 4 * i, live[&(0x204 + 4 * i)]);
        }
        for i in 0..6 {
            words.insert(0x204 + 4 * i, live[&(0x300 + 4 * i)]);
        }
        for slot in 0..3 {
            for i in 0..15 {
                words.insert(
                    0x21c + 0x3c * slot + 4 * i,
                    live[&(0x400 + 0x3c * slot + 4 * i)],
                );
            }
        }
        words
    }

    /// Halve every point of one still slot.
    fn halve_slot(words: &mut BTreeMap<u64, u32>, slot: u64) {
        for i in 0..14 {
            let w = words[&(0x21c + 0x3c * slot + 4 * i)];
            let half = ((w & 0x3fff) / 2) | ((((w >> 16) & 0x3fff) / 2) << 16);
            words.insert(0x21c + 0x3c * slot + 4 * i, half);
        }
    }

    #[test]
    fn the_color_unit_decodes_as_the_x70_film_stage() {
        let words = provia();
        assert_eq!(color_offset(0x400), Some(0x300));
        assert_eq!(color_offset(0x508), Some(0x400));
        assert_eq!(color_offset(0x500), Some(0xa58));
        assert_eq!(color_offset(0x700), None);
        let film = film(None, &words).unwrap();
        // Gray stays neutral, white is white and black is black.
        let gray = film.develop([4096; 3]);
        assert!(
            gray[1].abs_diff(128) <= 1 && gray[2].abs_diff(128) <= 1,
            "{gray:?}"
        );
        assert!(film.develop([16383; 3])[0] >= 250);
        assert!(film.develop([0; 3])[0] <= 5);
        // Red has red chroma.
        let red = film.develop([16383, 0, 0]);
        assert!(red[2] > 200 && red[1] < 128, "{red:?}");
        assert!(super::film(None, &BTreeMap::new()).is_err());
    }

    #[test]
    fn the_still_tone_unit_holds_its_own_matrix_and_three_slots() {
        assert_eq!(CURVE_SLOT_CHANNELS, [0, 1, 2]);
        let live = provia();
        let stock = still();
        let stock_film = film(Some(&stock), &live).unwrap();
        let live_film = film(None, &live).unwrap();
        let input = [6000, 3000, 2000];
        // The stock still block carries the color unit's words: equal slots
        // develop like the single-curve law.
        assert_eq!(stock_film.develop(input), live_film.develop(input));

        // A still slot moves its channel in the still film only.
        for slot in 0..3 {
            let mut slots = stock.clone();
            halve_slot(&mut slots, slot);
            let changed = film(Some(&slots), &live).unwrap();
            let (stock_color, changed_color) = (stock_film.color(input), changed.color(input));
            for channel in 0..3 {
                let of_slot = CURVE_SLOT_CHANNELS
                    .iter()
                    .position(|&s| s == slot as usize)
                    .unwrap();
                if channel == of_slot {
                    assert_ne!(changed_color[channel], stock_color[channel], "slot {slot}");
                } else {
                    assert_eq!(changed_color[channel], stock_color[channel], "slot {slot}");
                }
            }
            assert_eq!(
                film(None, &live).unwrap().develop(input),
                live_film.develop(input)
            );
        }

        // The still matrix is the still block's, the live matrix the color
        // unit's: a mismatch shows in the right unit's output only.
        let mut still_matrix = stock.clone();
        still_matrix.insert(0x204, 0x1f71040b);
        assert_ne!(
            film(Some(&still_matrix), &live).unwrap().color(input),
            stock_film.color(input)
        );
        let mut live_matrix = live.clone();
        live_matrix.insert(0x300, 0x1f71040b);
        assert_ne!(
            film(None, &live_matrix).unwrap().color(input),
            live_film.color(input)
        );
        assert_eq!(
            film(Some(&stock), &live_matrix).unwrap().color(input),
            stock_film.color(input)
        );
    }

    #[test]
    fn each_still_slot_is_the_channel_the_constant_names() {
        let live = provia();
        let stock = still();
        let mut slots = stock.clone();
        halve_slot(&mut slots, 1);
        halve_slot(&mut slots, 2);
        halve_slot(&mut slots, 2);
        let joint = film(Some(&slots), &live).unwrap();
        for (channel, &slot) in CURVE_SLOT_CHANNELS.iter().enumerate() {
            // Every slot carries this channel's curve: a single-curve film.
            let mut all = stock.clone();
            let slot = slot as u64;
            for other in 0..3 {
                for i in 0..15 {
                    let w = slots[&(0x21c + 0x3c * slot + 4 * i)];
                    all.insert(0x21c + 0x3c * other + 4 * i, w);
                }
            }
            let single = film(Some(&all), &live).unwrap();
            for input in [[1000, 2000, 3000], [6000, 3000, 2000], [16383, 8000, 4000]] {
                assert_eq!(
                    joint.color(input)[channel],
                    single.color(input)[channel],
                    "channel {channel} input {input:?}"
                );
            }
        }
    }

    #[test]
    fn the_film_law_develops_a_uniform_color() {
        let words = provia();
        let film = film(None, &words).unwrap();
        let (lines, width, stride) = (12, 12, 32);
        let mut raw = vec![0; stride * lines];
        for y in 0..lines {
            for x in 0..width {
                let v = if XTRANS[(y + 2) % 6][(x + 2) % 6] == 1 {
                    crate::imaging::WHITE
                } else {
                    crate::imaging::BLACK
                };
                raw[y * stride + 2 * x..][..2].copy_from_slice(&v.to_le_bytes());
            }
        }
        let unity = [1 << 12; 3];
        let out = develop(&raw, stride, lines, width, (2, 2), Some((&film, unity))).unwrap();
        let [y, cb, cr] = film.develop([0, 16383, 0]);
        for line in &out {
            for p in line.as_chunks::<4>().0 {
                assert_eq!(*p, [y, y, cb, cr]);
            }
        }
        assert!(cb < 128 && cr < 128);
    }

    #[test]
    fn white_balance_acts_by_its_ratio_to_the_reference() {
        let mut words = provia();
        // An AUTO pass and an incandescent pass of emulated shots (bootemu).
        words.insert(0x100, 0x12e_025b);
        words.insert(0x104, 0x227);
        let reference = [603, 302, 551];
        assert_eq!(white_balance(&words, reference).unwrap(), [4096; 3]);
        // The same ratios at twice the scale.
        assert_eq!(white_balance(&words, [1206, 604, 1102]).unwrap(), [4096; 3]);
        words.insert(0x100, 0x12e_018a);
        words.insert(0x104, 0x34c);
        let [r, g, b] = white_balance(&words, reference).unwrap();
        assert!(
            r < 4096 * 7 / 10 && g == 4096 && b > 4096 * 15 / 10,
            "{r} {g} {b}"
        );
        // Gray turns blue under the incandescent preset.
        let film = film(None, &words).unwrap();
        let gray = crate::imaging::BLACK + (crate::imaging::WHITE - crate::imaging::BLACK) / 4;
        let raw: Vec<u8> = (0..12 * 12).flat_map(|_| gray.to_le_bytes()).collect();
        let out = develop(&raw, 24, 12, 12, (0, 0), Some((&film, [r, g, b]))).unwrap();
        assert!(out[6][2] > 140 && out[6][3] < 116, "{:?}", &out[6][..4]);
        words.remove(&0x104);
        assert!(white_balance(&words, reference).is_err());
    }

    #[test]
    fn a_width_above_the_stride_reads_into_the_next_row() {
        // Playback of a 5008-wide still declares 5020 pixels over a 5008-pixel
        // stride; scaled down here: 6 pixels over a 4-pixel stride, 2 lines.
        let s = Scale {
            stride: 8,
            lines: 2,
            width: 6,
            out_lines: 2,
            out_width: 6,
            step_x: 1.,
            step_y: 1.,
        };
        let source: Vec<u8> = (0..20).collect();
        let rows = resample(&source, s).unwrap();
        assert_eq!(rows[0], (0..12).collect::<Vec<u8>>());
        assert_eq!(rows[1], (8..20).collect::<Vec<u8>>());
    }
}
