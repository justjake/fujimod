//! Declared X-Trans RAW law of the modelled still data path.
//!
//! Two halves of one round trip, so a scene survives sensor RAW and development:
//!
//! - [`mosaic_line`] samples an sRGB picture line as X-Trans sensor samples: each site takes
//!   its own color, linearised from sRGB and scaled between [`BLACK`] and [`WHITE`], as 14-bit
//!   values in little-endian 16-bit containers; [`pack12_line`] turns such a line into packed
//!   12-bit samples (the top 12 bits, two samples in three bytes).
//! - [`Raw::rgb`] and [`develop_422_line`] develop such samples: each output pixel takes the
//!   nearest site of each color, the response is inverted to sRGB and encoded as BT.601
//!   full-range (JFIF) YCbCr, the pixel pair's chroma averaged.
//!
//! Colour response and exposure ([`Response`]). The declared law above is
//! [`Response::NEUTRAL`]: every colour spans [`BLACK`]..[`WHITE`] for sRGB 0..255. A model of
//! a real sensor uses [`Response::xt5`]: `raw_c = BLACK + clamp(L_c x k_c x E x S, 0,
//! WHITE - BLACK)`, where `L_c` is the linear scene value of colour `c`, `k_c` the colour gain
//! ([`XT5_COLOUR_GAIN`], from the X-T5 WB_GRBLevelsStandard daylight entry 302 647 460: the
//! green sites collect the most light), `E` the exposure relative to the reference
//! ([`crate::exposure::Exposure::relative`]) and `S` ([`XT5_SCENE_WHITE`]) puts scene white one
//! stop below clipping at the reference exposure. Development with the same response divides by
//! `k_c x E x S`, so a picture that does not clip develops back to the scene (the JPEG stays
//! neutral) while the RAW samples carry the colour response and the exposure.
//!
//! The law is the X-Processor Pro's declared still law (`fujimod_emulator_xpp::development::
//! develop`, profile `xpp-declared-xtrans-nearest-srgb-jfif-422-v1`, and the sensor response
//! of `fujimod_emulator_xpp::imaging`) restated here so XP5 models can use it without the XPP
//! board crate; a test in that crate checks the two agree. Lens corrections, noise reduction,
//! sharpening, film simulation and white balance are not modelled.
use std::sync::OnceLock;

/// X-Trans 6x6 color sites: 0 red, 1 green, 2 blue.
pub const XTRANS: [[u8; 6]; 6] = [
    [0, 2, 1, 2, 0, 1],
    [1, 1, 0, 1, 1, 2],
    [1, 1, 2, 1, 1, 0],
    [2, 0, 1, 0, 2, 1],
    [1, 1, 2, 1, 1, 0],
    [1, 1, 0, 1, 1, 2],
];
/// Declared sample of sRGB black.
pub const BLACK: u16 = 1024;
/// Declared sample of sRGB white (the 14-bit maximum).
pub const WHITE: u16 = 16383;

/// Colour gains `k_c` (red, green, blue) of the X-T5 sensor model: the reciprocals of the
/// daylight white-balance multipliers 647 / 302 (red) and 460 / 302 (blue) over green 302.
pub const XT5_COLOUR_GAIN: [f64; 3] = [302. / 647., 1., 302. / 460.];
/// Signal of linear scene white (before colour gain and exposure) in the X-T5 sensor model:
/// half the range above black, so white is one stop below clipping at the reference exposure.
pub const XT5_SCENE_WHITE: f64 = 0.5 * (WHITE - BLACK) as f64;

/// A sensor colour response and exposure (see the module notes): colour `c` at linear scene
/// level `L` gives sample `BLACK + clamp(L x colour_gain[c] x exposure x white, 0, WHITE -
/// BLACK)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Response {
    pub colour_gain: [f64; 3],
    /// Exposure relative to the reference ([`crate::exposure::Exposure::relative`]).
    pub exposure: f64,
    /// Signal of linear scene white at unit colour gain and reference exposure.
    pub white: f64,
}
impl Response {
    /// The declared law: every colour spans [`BLACK`]..[`WHITE`].
    pub const NEUTRAL: Self = Self {
        colour_gain: [1.; 3],
        exposure: 1.,
        white: (WHITE - BLACK) as f64,
    };
    /// The X-T5 sensor model at `exposure` (relative to the reference).
    pub fn xt5(exposure: f64) -> Self {
        Self {
            colour_gain: XT5_COLOUR_GAIN,
            exposure: if exposure.is_finite() && exposure > 0. { exposure } else { 1. },
            white: XT5_SCENE_WHITE,
        }
    }
    fn is_neutral(&self) -> bool {
        *self == Self::NEUTRAL
    }
    /// Signal of linear level 1 of colour `c`.
    fn scale(&self, c: usize) -> f64 {
        self.colour_gain[c] * self.exposure * self.white
    }
    /// Sample of linear scene level `linear` of colour `c` (clipped to [`WHITE`]).
    pub fn sample(&self, c: usize, linear: f64) -> u16 {
        BLACK + (linear * self.scale(c)).round().clamp(0., f64::from(WHITE - BLACK)) as u16
    }
    /// Linear scene level of sample `sample` of colour `c` (not clamped).
    pub fn linear(&self, c: usize, sample: u16) -> f64 {
        (f64::from(sample) - f64::from(BLACK)) / self.scale(c)
    }
    /// The sample of each sRGB byte for each colour ([`mosaic_line_with`]).
    pub fn mosaic_lut(&self) -> MosaicLut {
        if self.is_neutral() {
            return MosaicLut([*response(); 3]);
        }
        let linear = linear_lut();
        MosaicLut(std::array::from_fn(|c| std::array::from_fn(|v| self.sample(c, linear[v]))))
    }
    /// The sRGB byte of each sample for each colour ([`Raw::with_response`]).
    fn inverse_lut(&self) -> Box<[Vec<u8>; 3]> {
        Box::new(std::array::from_fn(|c| {
            (0..=u16::MAX)
                .map(|sample| srgb_byte(self.linear(c, sample)))
                .collect()
        }))
    }
}

/// Samples of each sRGB byte for each colour (red, green, blue) of one [`Response`].
pub struct MosaicLut(pub [[u16; 256]; 3]);

/// Linear level of each sRGB byte.
fn linear_lut() -> &'static [f64; 256] {
    static LUT: OnceLock<[f64; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|v| srgb_to_linear(v as u8)))
}

/// Linear level of an sRGB byte.
pub fn srgb_to_linear(v: u8) -> f64 {
    let srgb = f64::from(v) / 255.;
    if srgb <= 0.04045 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB byte of a linear level (clamped to 0..1).
pub fn srgb_byte(linear: f64) -> u8 {
    let linear = linear.clamp(0., 1.);
    let srgb = if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1. / 2.4) - 0.055
    };
    (srgb * 255.).round() as u8
}

/// Sample of each sRGB byte.
fn response() -> &'static [u16; 256] {
    static LUT: OnceLock<[u16; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|v| {
            let srgb = v as f64 / 255.;
            let linear = if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            };
            BLACK + (linear * f64::from(WHITE - BLACK)).round() as u16
        })
    })
}

/// sRGB byte of each sample.
fn inverse_response() -> &'static [u8] {
    static LUT: OnceLock<Vec<u8>> = OnceLock::new();
    LUT.get_or_init(|| {
        let (black, white) = (f64::from(BLACK), f64::from(WHITE));
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
    })
}

/// (line, sample) offset of the nearest site of each color from each site of the tile.
fn nearest() -> &'static [[[(i64, i64); 3]; 6]; 6] {
    static OFFSETS: OnceLock<[[[(i64, i64); 3]; 6]; 6]> = OnceLock::new();
    OFFSETS.get_or_init(|| {
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
    })
}

/// Line `line` of a mosaic whose first sample is tile site (0, 0): `rgb` holds `width` sRGB
/// pixels (3 bytes each), `out` receives `width` little-endian 16-bit samples.
pub fn mosaic_line(rgb: &[u8], line: usize, out: &mut [u8]) {
    let lut = response();
    mosaic_line_sites(rgb, (line, 0), out, |_, v| lut[usize::from(v)]);
}

/// [`mosaic_line`] with a response's samples (`lut`, [`Response::mosaic_lut`]) and whose first
/// sample is tile site `site` (line, sample).
pub fn mosaic_line_with(rgb: &[u8], site: (usize, usize), out: &mut [u8], lut: &MosaicLut) {
    mosaic_line_sites(rgb, site, out, |c, v| lut.0[c][usize::from(v)]);
}

fn mosaic_line_sites(
    rgb: &[u8],
    (line, column): (usize, usize),
    out: &mut [u8],
    sample: impl Fn(usize, u8) -> u16,
) {
    let sites = &XTRANS[line % 6];
    for (x, (container, pixel)) in out
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(rgb.as_chunks::<3>().0)
        .enumerate()
    {
        let c = usize::from(sites[(x + column) % 6]);
        *container = sample(c, pixel[c]).to_le_bytes();
    }
}

/// Bytes of a packed 12-bit line of `width` samples (an odd last sample takes a whole pair).
pub fn packed12_bytes(width: usize) -> usize {
    width.div_ceil(2) * 3
}

/// Pack a line of little-endian 16-bit 14-bit samples (`samples`, as [`mosaic_line`] writes)
/// into 12-bit samples, two in three bytes, little-endian: bytes `a[7:0]`, `b[3:0] a[11:8]`,
/// `b[11:4]`, where `a` and `b` are the samples' top 12 of 14 bits. This is the packing of the
/// sensor input RAW DMA channels' packed lines. `out` holds [`packed12_bytes`] of the width.
pub fn pack12_line(samples: &[u8], out: &mut [u8]) {
    let twelve = |pair: &[u8]| u16::from_le_bytes([pair[0], pair[1]]) >> 2;
    for (pair, packed) in samples.chunks(4).zip(out.chunks_exact_mut(3)) {
        let a = twelve(pair);
        let b = if pair.len() == 4 { twelve(&pair[2..]) } else { 0 };
        packed[0] = a as u8;
        packed[1] = (a >> 8) as u8 | (b << 4) as u8;
        packed[2] = (b >> 4) as u8;
    }
}

/// Sample `x` of a packed 12-bit line, scaled to the 14-bit range of the law (the top two
/// bits repeated into the bottom two, so 0 and 4095 map to 0 and [`WHITE`]).
fn unpack12(line: &[u8], x: usize) -> u16 {
    let at = x / 2 * 3;
    let twelve = if x.is_multiple_of(2) {
        u16::from(line[at]) | (u16::from(line[at + 1]) & 0xf) << 8
    } else {
        u16::from(line[at + 1]) >> 4 | u16::from(line[at + 2]) << 4
    };
    twelve << 2 | twelve >> 10
}

/// BT.601 full-range (JFIF) Y, Cb and Cr of an sRGB pixel.
pub fn ycbcr_jfif([r, g, b]: [u8; 3]) -> [i32; 3] {
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

/// A window of X-Trans samples: `lines` lines of `width` samples, `pitch` bytes apart, whose
/// first sample is tile site `phase` (line, sample). Samples are 14-bit values in little-endian
/// 16-bit containers ([`Raw::new`]) or packed 12-bit ([`Raw::packed12`], scaled to 14 bits).
pub struct Raw<'a> {
    bytes: &'a [u8],
    pitch: usize,
    width: usize,
    lines: usize,
    phase: (usize, usize),
    packed12: bool,
    /// The sRGB byte of each sample for each colour ([`Raw::with_response`]); `None` is the
    /// declared law's.
    inverse: Option<Box<[Vec<u8>; 3]>>,
}

impl<'a> Raw<'a> {
    pub fn new(
        bytes: &'a [u8],
        pitch: usize,
        width: usize,
        lines: usize,
        phase: (usize, usize),
    ) -> Result<Self, String> {
        if lines < 6
            || width < 6
            || pitch < 2 * width
            || bytes.len() < pitch * (lines - 1) + 2 * width
        {
            return Err(format!(
                "X-Trans RAW window {width}x{lines} pitch {pitch} ({} bytes) is not developable",
                bytes.len()
            ));
        }
        Ok(Self { bytes, pitch, width, lines, phase, packed12: false, inverse: None })
    }
    /// A window of packed 12-bit samples ([`pack12_line`]); otherwise as [`Raw::new`].
    pub fn packed12(
        bytes: &'a [u8],
        pitch: usize,
        width: usize,
        lines: usize,
        phase: (usize, usize),
    ) -> Result<Self, String> {
        let line = packed12_bytes(width);
        if lines < 6 || width < 6 || pitch < line || bytes.len() < pitch * (lines - 1) + line {
            return Err(format!(
                "packed 12-bit X-Trans RAW window {width}x{lines} pitch {pitch} ({} bytes) is not developable",
                bytes.len()
            ));
        }
        Ok(Self { bytes, pitch, width, lines, phase, packed12: true, inverse: None })
    }
    /// Develop with `response` (divide by its colour gains and exposure) instead of the
    /// declared law.
    pub fn with_response(mut self, response: Response) -> Self {
        self.inverse = (!response.is_neutral()).then(|| response.inverse_lut());
        self
    }
    pub fn width(&self) -> usize {
        self.width
    }
    /// The 14-bit sample at sample `x` of line `y` (packed 12-bit samples scaled to 14 bits).
    pub fn sample_at(&self, x: usize, y: usize) -> u16 {
        self.sample(y as i64, x as i64)
    }
    pub fn lines(&self) -> usize {
        self.lines
    }
    fn sample(&self, y: i64, x: i64) -> u16 {
        // Extend by whole tiles so border sites keep their color.
        let fold = |v: i64, n: usize| {
            if v < 0 {
                v + 6
            } else if v >= n as i64 {
                v - 6
            } else {
                v
            }
        };
        let (y, x) = (fold(y, self.lines) as usize, fold(x, self.width) as usize);
        if self.packed12 {
            return unpack12(&self.bytes[y * self.pitch..], x);
        }
        let i = y * self.pitch + 2 * x;
        u16::from_le_bytes([self.bytes[i], self.bytes[i + 1]])
    }
    /// sRGB of the pixel at sample `x` of line `y`, from the nearest site of each color.
    pub fn rgb(&self, x: usize, y: usize) -> [u8; 3] {
        let site = &nearest()[(y + self.phase.0) % 6][(x + self.phase.1) % 6];
        let neutral = inverse_response();
        [0, 1, 2].map(|c| {
            let inverse = self.inverse.as_ref().map_or(neutral, |lut| &lut[c][..]);
            inverse[usize::from(self.sample(y as i64 + site[c].0, x as i64 + site[c].1))]
        })
    }
}

/// One packed 4:2:2 line (Y0 Y1 Cb Cr) of `width` pixels (rounded up to a pair; an odd last
/// pixel repeats) developed from line `y` of `raw`, pixel `x` taken from sample `column(x)`.
pub fn develop_422_line(
    raw: &Raw<'_>,
    y: usize,
    width: usize,
    column: impl Fn(usize) -> usize,
) -> Vec<u8> {
    let pairs = width.div_ceil(2);
    let mut line = Vec::with_capacity(4 * pairs);
    for pair in 0..pairs {
        let [a, b] = [0, 1].map(|i| {
            let x = (2 * pair + i).min(width - 1);
            ycbcr_jfif(raw.rgb(column(x), y))
        });
        line.extend_from_slice(&[
            byte(a[0]),
            byte(b[0]),
            byte((a[1] + b[1] + 1) >> 1),
            byte((a[2] + b[2] + 1) >> 1),
        ]);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat sRGB colour survives the mosaic and development exactly away from the borders.
    #[test]
    fn a_flat_colour_round_trips_through_the_mosaic() {
        let (width, lines, pitch) = (24, 12, 64);
        let colour = [200u8, 100, 30];
        let rgb = colour.repeat(width);
        let mut bytes = vec![0u8; pitch * lines];
        for y in 0..lines {
            mosaic_line(&rgb, y, &mut bytes[y * pitch..y * pitch + 2 * width]);
        }
        // Site (0, 0) is red, (0, 1) blue, (1, 0) green.
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), response()[200]);
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), response()[30]);
        assert_eq!(u16::from_le_bytes([bytes[pitch], bytes[pitch + 1]]), response()[100]);
        let raw = Raw::new(&bytes, pitch, width, lines, (0, 0)).unwrap();
        for y in 0..lines {
            for x in 0..width {
                assert_eq!(raw.rgb(x, y), colour, "pixel ({x}, {y})");
            }
        }
        let line = develop_422_line(&raw, 5, 8, |x| x);
        let [yy, cb, cr] = ycbcr_jfif(colour).map(byte);
        assert_eq!(line, [yy, yy, cb, cr].repeat(4));
    }

    /// Packed 12-bit lines keep the top 12 bits; a flat colour still develops exactly.
    #[test]
    fn a_flat_colour_round_trips_through_packed_12_bit_samples() {
        let mut packed = [0u8; 3];
        pack12_line(&[0xff, 0x3f, 0x04, 0x00], &mut packed);
        // 16383 -> 4095, 4 -> 1.
        assert_eq!(packed, [0xff, 0x1f, 0x00]);
        assert_eq!((unpack12(&packed, 0), unpack12(&packed, 1)), (WHITE, 4));
        let mut odd = [0u8; 3];
        pack12_line(&[0x00, 0x10], &mut odd);
        assert_eq!(odd, [0x00, 0x04, 0x00]);
        let (width, lines, pitch) = (24, 12, 40);
        let colour = [200u8, 100, 30];
        let rgb = colour.repeat(width);
        let (mut samples, mut bytes) = (vec![0u8; 2 * width], vec![0u8; pitch * lines]);
        for y in 0..lines {
            mosaic_line(&rgb, y, &mut samples);
            pack12_line(&samples, &mut bytes[y * pitch..y * pitch + packed12_bytes(width)]);
        }
        let raw = Raw::packed12(&bytes, pitch, width, lines, (0, 0)).unwrap();
        for y in 0..lines {
            for x in 0..width {
                assert_eq!(raw.rgb(x, y), colour, "pixel ({x}, {y})");
            }
        }
        assert!(Raw::packed12(&bytes, 35, width, lines, (0, 0)).is_err());
    }

    #[test]
    fn the_response_spans_black_to_white() {
        assert_eq!(response()[0], BLACK);
        assert_eq!(response()[255], WHITE);
        for v in [0u8, 1, 17, 128, 254, 255] {
            assert_eq!(inverse_response()[usize::from(response()[usize::from(v)])], v);
        }
    }

    /// Every sRGB byte survives the X-T5 response at and above the reference exposure, at
    /// every site colour; below it, the coarser red steps keep it within one step.
    #[test]
    fn every_byte_round_trips_through_the_xt5_response() {
        for (exposure, tolerance) in [(1., 0), (1.134, 0), (1.9, 0), (0.5, 1)] {
            let response = Response::xt5(exposure);
            let (lut, inverse) = (response.mosaic_lut(), response.inverse_lut());
            for c in 0..3 {
                for v in 0..=255u8 {
                    let back = inverse[c][usize::from(lut.0[c][usize::from(v)])];
                    assert!(
                        back.abs_diff(v) <= tolerance,
                        "exposure {exposure} colour {c} byte {v} -> {back}"
                    );
                }
            }
        }
    }

    /// Scene white sits one stop below clipping on green at the reference exposure; red and
    /// blue take their colour gains; black is the pedestal; two stops more clip white.
    #[test]
    fn the_xt5_response_scales_by_colour_gain_and_exposure() {
        let lut = Response::xt5(1.).mosaic_lut();
        assert_eq!(lut.0.map(|c| c[0]), [BLACK; 3]);
        assert_eq!(lut.0[1][255], BLACK + 7680);
        assert_eq!(lut.0[0][255], BLACK + (XT5_SCENE_WHITE * 302. / 647.).round() as u16);
        assert_eq!(lut.0[2][255], BLACK + (XT5_SCENE_WHITE * 302. / 460.).round() as u16);
        let doubled = Response::xt5(2.).mosaic_lut();
        for c in 0..3 {
            for v in [64usize, 128, 200] {
                let (one, two) = (lut.0[c][v] - BLACK, doubled.0[c][v] - BLACK);
                assert!(two.abs_diff(2 * one) <= 1, "colour {c} byte {v}: {one} -> {two}");
            }
        }
        assert_eq!(Response::xt5(4.).mosaic_lut().0[1][255], WHITE);
        assert_eq!(Response::xt5(f64::NAN), Response::xt5(1.));
    }

    /// A flat colour mosaiced with the X-T5 response at an offset site develops back exactly
    /// with the same response, and the neutral law's mosaic is unchanged.
    #[test]
    fn a_flat_colour_round_trips_through_the_xt5_response() {
        let (width, lines, pitch) = (24, 12, 64);
        let colour = [200u8, 100, 30];
        let rgb = colour.repeat(width);
        let response = Response::xt5(1.25);
        let lut = response.mosaic_lut();
        let mut bytes = vec![0u8; pitch * lines];
        for y in 0..lines {
            mosaic_line_with(&rgb, (y + 2, 3), &mut bytes[y * pitch..y * pitch + 2 * width], &lut);
        }
        // Site (2, 3) is green, (2, 5) red, (3, 3) red, (2, 2) blue.
        let at = |y: usize, x: usize| u16::from_le_bytes([bytes[y * pitch + 2 * x], bytes[y * pitch + 2 * x + 1]]);
        assert_eq!(at(0, 0), lut.0[1][100]);
        assert_eq!(at(0, 2), lut.0[0][200]);
        assert_eq!(at(1, 0), lut.0[0][200]);
        assert!(at(0, 0) < 7680 + BLACK);
        let raw = Raw::new(&bytes, pitch, width, lines, (2, 3)).unwrap().with_response(response);
        for y in 0..lines {
            for x in 0..width {
                assert_eq!(raw.rgb(x, y), colour, "pixel ({x}, {y})");
            }
        }
        let (mut neutral, mut with) = ([0u8; 48], [0u8; 48]);
        mosaic_line(&rgb, 4, &mut neutral);
        mosaic_line_with(&rgb, (4, 0), &mut with, &Response::NEUTRAL.mosaic_lut());
        assert_eq!(neutral, with);
    }

    #[test]
    fn a_short_window_refuses() {
        assert!(Raw::new(&[0; 71], 12, 6, 6, (0, 0)).is_err());
        assert!(Raw::new(&[0; 200], 10, 6, 6, (0, 0)).is_err());
    }
}
