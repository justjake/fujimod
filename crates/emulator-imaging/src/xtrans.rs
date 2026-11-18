//! Declared X-Trans RAW law of the modelled still data path.
//!
//! Two halves of one round trip, so a scene survives sensor RAW and development:
//!
//! - [`mosaic_line`] samples an sRGB picture line as X-Trans sensor samples: each site takes
//!   its own color, linearised from sRGB and scaled between [`BLACK`] and [`WHITE`], as 14-bit
//!   values in little-endian 16-bit containers.
//! - [`Raw::rgb`] and [`develop_422_line`] develop such samples: each output pixel takes the
//!   nearest site of each color, the response is inverted to sRGB and encoded as BT.601
//!   full-range (JFIF) YCbCr, the pixel pair's chroma averaged.
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
    let (lut, sites) = (response(), &XTRANS[line % 6]);
    for (x, (sample, pixel)) in out
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(rgb.as_chunks::<3>().0)
        .enumerate()
    {
        *sample = lut[usize::from(pixel[usize::from(sites[x % 6])])].to_le_bytes();
    }
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

/// A window of 16-bit X-Trans samples: `lines` lines of `width` little-endian samples, `pitch`
/// bytes apart, whose first sample is tile site `phase` (line, sample).
pub struct Raw<'a> {
    bytes: &'a [u8],
    pitch: usize,
    width: usize,
    lines: usize,
    phase: (usize, usize),
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
        Ok(Self { bytes, pitch, width, lines, phase })
    }
    pub fn width(&self) -> usize {
        self.width
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
        let i = y * self.pitch + 2 * x;
        u16::from_le_bytes([self.bytes[i], self.bytes[i + 1]])
    }
    /// sRGB of the pixel at sample `x` of line `y`, from the nearest site of each color.
    pub fn rgb(&self, x: usize, y: usize) -> [u8; 3] {
        let site = &nearest()[(y + self.phase.0) % 6][(x + self.phase.1) % 6];
        let inverse = inverse_response();
        [0, 1, 2].map(|c| {
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

    #[test]
    fn the_response_spans_black_to_white() {
        assert_eq!(response()[0], BLACK);
        assert_eq!(response()[255], WHITE);
        for v in [0u8, 1, 17, 128, 254, 255] {
            assert_eq!(inverse_response()[usize::from(response()[usize::from(v)])], v);
        }
    }

    #[test]
    fn a_short_window_refuses() {
        assert!(Raw::new(&[0; 71], 12, 6, 6, (0, 0)).is_err());
        assert!(Raw::new(&[0; 200], 10, 6, 6, (0, 0)).is_err());
    }
}
