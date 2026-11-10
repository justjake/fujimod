//! Register-programmed integer film processing with caller-supplied curves
//! and coefficients.
//! All image loops use slices; 4096-entry Bezier LUTs rebuild only on change.
use serde::{Deserialize, Serialize};
const GROUPS: [(usize, usize); 6] = [
    (0x304, 11),
    (0x400, 6),
    (0x500, 16),
    (0x604, 2),
    (0x640, 1),
    (0x644, 5),
];

fn signed(v: u32, bits: u32) -> i32 {
    (v << (32 - bits)) as i32 >> (32 - bits)
}
fn bezier(points: &[i64], widths: &[usize], x: usize) -> i64 {
    let mut start = 0;
    for (k, &width) in widths.iter().enumerate() {
        if x < start + width {
            let t = (((x - start) << 9) / width) as i64;
            let u = 512 - t;
            let p = &points[3 * k..3 * k + 4];
            return (u * u * u * p[0]
                + 3 * u * u * t * p[1]
                + 3 * u * t * t * p[2]
                + t * t * t * p[3]
                + (1 << 26))
                >> 27;
        }
        start += width;
    }
    *points.last().unwrap()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Film {
    words: Vec<u32>,
    gain: Vec<u16>,
    /// One tone curve per output channel, in R, G, B order.
    curves: [Vec<u16>; 3],
    matrix: [[i32; 3]; 3],
    chroma: [i32; 4],
    bounds: [i32; 4],
    bands: [(i32, i32); 5],
    /// The still hue stage, when its enable bit is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<HueStage>,
}
impl Default for Film {
    fn default() -> Self {
        Self {
            words: Vec::new(),
            gain: vec![0; 4096],
            curves: std::array::from_fn(|_| vec![0; 4096]),
            matrix: [[0; 3]; 3],
            chroma: [0; 4],
            bounds: [0; 4],
            bands: [(0, 0); 5],
            hue: None,
        }
    }
}
impl Film {
    pub fn words(&self) -> &[u32] {
        &self.words
    }
    pub fn matches(&self, r: impl Fn(usize) -> u32) -> bool {
        !self.words.is_empty()
            && GROUPS
                .iter()
                .flat_map(|&(o, n)| (0..n).map(move |i| o + 4 * i))
                .enumerate()
                .all(|(i, o)| self.words[i] == r(o))
    }
    pub fn update(&mut self, r: impl Fn(usize) -> u32) -> Result<(), String> {
        if self.matches(&r) {
            return Ok(());
        }
        self.update_slots(&r, |_, i| r(0x508 + 4 * i))
    }
    /// Update from a word source `r` (matrix, brightness table, widths, chroma,
    /// luma bands) and one 14-word curve point set per output channel, in R, G,
    /// B order: `points(channel, i)` is word `i` of that channel's 28-point
    /// curve. A single-curve law passes the same offsets for all three
    /// channels; equal slots develop exactly like [`Film::update`].
    pub fn update_slots(
        &mut self,
        r: impl Fn(usize) -> u32,
        points: impl Fn(usize, usize) -> u32,
    ) -> Result<(), String> {
        let bounds = std::array::from_fn(|i| ((r(0x640) >> (8 * i)) & 255) as i32);
        if bounds.windows(2).any(|w| w[0] > w[1]) {
            return Err("film luma bounds are not ordered".into());
        }
        self.bounds = bounds;
        for (row, values) in self.matrix.iter_mut().enumerate().take(3) {
            *values = [
                signed(r(0x400 + row * 8), 13),
                signed(r(0x400 + row * 8) >> 16, 13),
                signed(r(0x404 + row * 8), 13),
            ];
        }
        self.chroma = [
            signed(r(0x604), 13),
            signed(r(0x604) >> 16, 13),
            signed(r(0x608), 13),
            signed(r(0x608) >> 16, 13),
        ];
        self.bands = std::array::from_fn(|i| {
            (
                signed(r(0x644 + i * 4), 11),
                ((r(0x644 + i * 4) >> 16) & 1023) as i32,
            )
        });
        let table: [i64; 22] =
            std::array::from_fn(|i| ((r(0x304 + i / 2 * 4) >> (i % 2 * 16)) & 8191) as i64);
        let widths: [usize; 9] = std::array::from_fn(|i| {
            16 << ((r(if i < 8 { 0x500 } else { 0x504 }) >> (if i < 8 { i * 4 } else { 0 })) & 7)
        });
        for (x, gain) in self.gain.iter_mut().enumerate().take(4096) {
            *gain = bezier(&table, &[64, 64, 128, 256, 512, 1024, 2048], x) as u16;
        }
        for (channel, curve) in self.curves.iter_mut().enumerate() {
            let slot: [i64; 28] =
                std::array::from_fn(|i| ((points(channel, i / 2) >> (i % 2 * 16)) & 16383) as i64);
            for (x, value) in curve.iter_mut().enumerate() {
                *value = bezier(&slot, &widths, x) as u16;
            }
        }
        self.words.clear();
        for (o, n) in GROUPS {
            for i in 0..n {
                self.words.push(r(o + i * 4));
            }
        }
        Ok(())
    }
    #[inline(always)]
    pub fn color(&self, w: [u16; 3]) -> [i32; 3] {
        // Register masks bound the hottest products: 14-bit ADC * 13-bit gain
        // is below 2^27; signed 13-bit matrix * 12-bit color is below 2^24.
        // Chroma is clamped before band gain. All pixel arithmetic fits i32;
        // the cubic LUT builder above still requires i64. Keep each arithmetic
        // shift before its sum, as measured by the integer film experiments.
        // Array maps are not inlined in this crate's profile; per-pixel
        // channel arithmetic stays explicit.
        let w = [
            i32::from(w[0].min(16383)),
            i32::from(w[1].min(16383)),
            i32::from(w[2].min(16383)),
        ];
        let b = (3 * (w[0] >> 2) + 7 * (w[1] >> 2) + 6 * (w[2] >> 2)) >> 4;
        let gain = i32::from(self.gain[b as usize]);
        let x = [
            ((w[0] * gain) >> 11).min(4095),
            ((w[1] * gain) >> 11).min(4095),
            ((w[2] * gain) >> 11).min(4095),
        ];
        let mut color = [0; 3];
        for (channel, (color, m)) in color.iter_mut().zip(&self.matrix).enumerate() {
            let value =
                (((m[0] * x[0]) >> 7) + ((m[1] * x[1]) >> 7) + ((m[2] * x[2]) >> 7) + 4) >> 3;
            *color = i32::from(self.curves[channel][value.clamp(0, 4095) as usize] >> 6);
        }
        color
    }
    #[inline(always)]
    fn luminance(color: [i32; 3]) -> i32 {
        let [r, g, b] = color;
        (((306 * r) >> 7) + ((601 * g) >> 7) + ((117 * b) >> 7) + 4) >> 3
    }
    #[inline(always)]
    pub fn luma(&self, w: [u16; 3]) -> u8 {
        Self::luminance(self.color(w)) as u8
    }
    #[inline(always)]
    pub fn develop(&self, w: [u16; 3]) -> [u8; 3] {
        let color = self.color(w);
        let [r, _, b] = color;
        let y = Self::luminance(color);
        let [c0, c1, c2, c3] = self.chroma;
        let cb = (((((c2 * (b - y)) >> 8) + ((c3 * (r - y)) >> 8)) * 578) >> 10).clamp(-512, 511);
        let cr = (((((c0 * (r - y)) >> 8) + ((c1 * (b - y)) >> 8)) * 730) >> 10).clamp(-512, 511);
        let mut band = 0;
        while band < self.bounds.len() && self.bounds[band] <= y {
            band += 1;
        }
        let (shift, gain) = self.bands[band];
        let mut f = gain + ((shift * y) >> 8);
        let mut tint = [0, 0];
        if let Some(hue) = &self.hue {
            f = (f * hue.gain(cb, cr)) >> 10;
            tint = hue.tint;
        }
        [
            y as u8,
            (128 + ((cb * f + 512) >> 10) + tint[0]).clamp(0, 255) as u8,
            (128 + ((cr * f + 512) >> 10) + tint[1]).clamp(0, 255) as u8,
        ]
    }
    #[inline(always)]
    pub fn pack(a: [u8; 3], b: [u8; 3]) -> [u8; 4] {
        let mean = |a: u8, b: u8| {
            let n = u16::from(a) + u16::from(b);
            let h = n / 2;
            (h + ((n & 1) & (h & 1))) as u8
        };
        [a[0], b[0], mean(a[1], b[1]), mean(a[2], b[2])]
    }
}

/// X70/X-E2S still hue stage (FFF44500 bit 12, FFF44544..5c). An approximate
/// fit to X-E2S 1.11 camera measurements of card ÷ stock chroma on a 120-patch
/// hue target, not a recovered law. Luma and chroma direction are kept; chroma
/// magnitude is scaled by
/// `sqrt(cos^2(p) b0^2 + sin^2(p) b1^2) / 16 + window`, `p = d + 41 deg`,
/// with `d` the hue `atan2(Cr, Cb)` minus bank 0's axis: byte 0 governs two
/// opposite lobes and byte 1 the hues between them. `window` is a triangle of
/// height 0.45 and half-width 15 deg at `d = 3 deg`, whatever the bytes.
///
/// With no bank 0 axis, the stage's form for Sepia (`0, 0, 512, 136, -120`),
/// bank 0's vector (`FFF4454c`, Cr low, Cb high) is also added to the output
/// chroma in eighths of a level: an assumed law, chosen so that Sepia's
/// constants give a sepia-strength warm tint, not a measured one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HueStage {
    axis: f64,
    window: f64,
    bytes: [f64; 2],
    /// Output (Cb, Cr) offset in levels.
    #[serde(default)]
    tint: [i32; 2],
}
impl HueStage {
    /// From FFF44000-relative still words, or None when the stage is off.
    pub fn from_still(r: impl Fn(usize) -> u32) -> Option<Self> {
        if r(0x500) & 1 << 12 == 0 {
            return None;
        }
        let vector = |w: u32| (signed(w, 10), signed(w >> 16, 10));
        let (cr, cb) = vector(r(0x544));
        let bytes = [f64::from(r(0x55c) & 31), f64::from(r(0x55c) >> 16 & 31)];
        if cr == 0 && cb == 0 {
            // Bank 1 alone keeps nothing at bytes 0, 0: without an axis,
            // byte 1 applies everywhere.
            let eighths = |v: i32| (v + 4).div_euclid(8);
            let w = r(0x54c);
            return Some(Self {
                axis: 0.0,
                window: 0.0,
                bytes: [bytes[1]; 2],
                tint: [eighths(signed(w >> 16, 13)), eighths(signed(w, 13))],
            });
        }
        // Bank 0 alone keeps 1.7 times the window of both factory banks, and
        // the window scales by (bank 0 width word / 515)^2.5.
        let banks = if vector(r(0x550)) == (0, 0) { 1.7 } else { 1.0 };
        let width = f64::from(r(0x548) & 0x7ff) / 515.0;
        Some(Self {
            axis: libm::atan2(f64::from(cr), f64::from(cb)).to_degrees(),
            window: 0.45 * banks * libm::pow(width, 2.5),
            bytes,
            tint: [0, 0],
        })
    }
    /// Q10 chroma gain for chroma (cb, cr).
    pub fn gain(&self, cb: i32, cr: i32) -> i32 {
        let hue = libm::atan2(f64::from(cr), f64::from(cb)).to_degrees();
        let d = (hue - self.axis + 180.0).rem_euclid(360.0) - 180.0;
        let p = (d + 41.0).to_radians();
        let (s, c) = (libm::sin(p), libm::cos(p));
        let [b0, b1] = self.bytes;
        let g = libm::sqrt(c * c * b0 * b0 + s * s * b1 * b1) / 16.0
            + self.window * (1.0 - (d - 3.0).abs() / 15.0).max(0.0);
        libm::rint(g * 1024.0) as i32
    }
}

/// Exact, bounded reuse within one raster. Sensor upsampling often reconstructs
/// the same RGB triplet many times. Entries compare all input bits, so hash
/// collisions only cost a film evaluation. Nothing survives a parameter change
/// or enters device state, and the scalar integer law stays the reference.
pub struct FilmPixels<'a> {
    film: &'a Film,
    entries: Vec<(u64, [u8; 3])>,
}
impl<'a> FilmPixels<'a> {
    pub fn new(film: &'a Film) -> Self {
        Self {
            film,
            entries: vec![(u64::MAX, [0; 3]); 4096],
        }
    }
    #[inline(always)]
    pub fn develop(&mut self, rgb: [u16; 3]) -> [u8; 3] {
        let key = u64::from(rgb[0]) | (u64::from(rgb[1]) << 16) | (u64::from(rgb[2]) << 32);
        let index = (key.wrapping_mul(0x9e3779b97f4a7c15) >> 52) as usize;
        let entry = &mut self.entries[index];
        if entry.0 != key {
            *entry = (key, self.film.develop(rgb));
        }
        entry.1
    }
    /// Histogram consumers require Y only. The high key bit marks a partial
    /// entry so a later color consumer still computes its missing chroma.
    #[inline(always)]
    pub fn luma(&mut self, rgb: [u16; 3]) -> u8 {
        let key = u64::from(rgb[0]) | (u64::from(rgb[1]) << 16) | (u64::from(rgb[2]) << 32);
        let index = (key.wrapping_mul(0x9e3779b97f4a7c15) >> 52) as usize;
        let entry = &mut self.entries[index];
        if entry.0 != key && entry.0 != (key | 1 << 63) {
            *entry = (key | 1 << 63, [self.film.luma(rgb), 0, 0]);
        }
        entry.1[0]
    }
}

pub fn still_offset(o: usize) -> Option<usize> {
    [
        (0x300, 0x104, 12),
        (0x400, 0x304, 6),
        (0x500, 0x31c, 17),
        (0x600, 0x520, 22),
    ]
    .iter()
    .find(|&&(start, _, n)| o >= start && o < start + n * 4)
    .map(|&(start, dest, _)| dest + o - start)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Wider scalar oracle for the measured integer law. Keep it independent
    // of the optimized pixel path, including its intermediate storage widths.
    fn wide_reference(f: &Film, rgb: [u16; 3]) -> [u8; 3] {
        let input = rgb.map(|v| i64::from(v.min(16383)));
        let brightness = (3 * (input[0] / 4) + 7 * (input[1] / 4) + 6 * (input[2] / 4)) / 16;
        let gain = i64::from(f.gain[brightness as usize]);
        let channels = input.map(|v| ((v * gain) >> 11).min(4095));
        let mut color = [0i64; 3];
        for (row, out) in color.iter_mut().enumerate() {
            let mut sum = 4;
            for (coefficient, channel) in f.matrix[row].iter().zip(channels) {
                sum += (i64::from(*coefficient) * channel) >> 7;
            }
            *out = i64::from(f.curves[row][(sum >> 3).clamp(0, 4095) as usize] >> 6);
        }
        let [red, green, blue] = color;
        let luma = (((306 * red) >> 7) + ((601 * green) >> 7) + ((117 * blue) >> 7) + 4) >> 3;
        let matrix = f.chroma.map(i64::from);
        let chroma = [
            (((matrix[2] * (blue - luma)) >> 8) + ((matrix[3] * (red - luma)) >> 8)) * 578,
            (((matrix[0] * (red - luma)) >> 8) + ((matrix[1] * (blue - luma)) >> 8)) * 730,
        ];
        let band = f.bounds.iter().filter(|&&b| i64::from(b) <= luma).count();
        let (shift, gain) = f.bands[band];
        let factor = i64::from(gain) + ((i64::from(shift) * luma) >> 8);
        let chroma = chroma.map(|v| {
            (128 + ((((v >> 10).clamp(-512, 511)) * factor + 512) >> 10)).clamp(0, 255) as u8
        });
        [luma as u8, chroma[0], chroma[1]]
    }
    #[test]
    fn narrow_pixel_arithmetic_matches_wide_law_at_register_and_adc_extremes() {
        let mut random = 7u64;
        let mut next = || {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            (random >> 32) as u32
        };
        for bank in 0..32 {
            let mut words = fujimod_emulator_devices::test_host::film_words();
            if bank != 0 {
                for (i, word) in words.iter_mut().enumerate() {
                    if let Some(mask) = crate::exr2_isp::parameters::mask(i * 4) {
                        *word = match bank {
                            1 => 0,
                            2 => mask,
                            _ => next() & mask,
                        };
                    }
                }
                let mut bounds = words[0x640 / 4].to_le_bytes();
                bounds.sort();
                words[0x640 / 4] = u32::from_le_bytes(bounds);
            }
            let mut film = Film::default();
            film.update(|o| words[o / 4]).unwrap();
            // Sweep every u16 value (including ADC clipping) on every channel,
            // with independent random companions and saturated endpoints.
            for value in 0..=u16::MAX {
                let rgb = [value, next() as u16, next() as u16];
                for rgb in [rgb, [rgb[1], rgb[0], rgb[2]], [rgb[2], rgb[1], rgb[0]]] {
                    assert_eq!(film.luma(rgb), wide_reference(&film, rgb)[0]);
                    assert_eq!(
                        film.develop(rgb),
                        wide_reference(&film, rgb),
                        "bank {bank}, input {rgb:?}"
                    );
                }
            }
            for rgb in [
                [0; 3],
                [16383; 3],
                [u16::MAX; 3],
                [0, 16383, 0],
                [16383, 0, 16383],
            ] {
                assert_eq!(film.develop(rgb), wide_reference(&film, rgb));
            }
        }
    }
    #[test]
    fn raster_memo_preserves_scalar_integer_colors_across_collisions() {
        let words = fujimod_emulator_devices::test_host::film_words();
        let mut film = Film::default();
        film.update(|o| words[o / 4]).unwrap();
        let mut pixels = FilmPixels::new(&film);
        let mut state = 7u64;
        for i in 0..65_536 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let rgb = if i % 3 == 0 {
                [4096, 8192, 16383]
            } else {
                [state as u16, (state >> 16) as u16, (state >> 32) as u16]
            };
            assert_eq!(pixels.luma(rgb), film.develop(rgb)[0]);
            assert_eq!(pixels.develop(rgb), film.develop(rgb));
            assert_eq!(pixels.luma(rgb), film.develop(rgb)[0]);
            assert_eq!(pixels.develop(rgb), film.develop(rgb));
        }
    }
    #[test]
    fn bezier_endpoints_and_half_even_chroma() {
        assert_eq!(bezier(&[0, 10, 20, 30], &[16], 0), 0);
        assert_eq!(bezier(&[0, 10, 20, 30], &[16], 8), 15);
        assert_eq!(bezier(&[0, 10, 20, 30], &[16], 16), 30);
        assert_eq!(Film::pack([12, 2, 3], [34, 3, 4]), [12, 34, 2, 4]);
    }
    #[test]
    fn each_channel_develops_through_its_own_slot() {
        let words = fujimod_emulator_devices::test_host::film_words();
        let at = |o: usize| words[o / 4];
        // Three distinct point sets: stock, half, and inverted.
        let half = |i: usize| {
            let w = at(0x508 + 4 * i);
            ((w & 0x3fff) / 2) | ((((w >> 16) & 0x3fff) / 2) << 16)
        };
        let inverted = |i: usize| {
            let w = at(0x508 + 4 * i);
            (0x3fff - (w & 0x3fff)) | ((0x3fff - ((w >> 16) & 0x3fff)) << 16)
        };
        let point = |channel: usize, i: usize| match channel {
            0 => at(0x508 + 4 * i),
            1 => half(i),
            _ => inverted(i),
        };
        let mut film = Film::default();
        film.update_slots(at, point).unwrap();
        for channel in 0..3 {
            let mut single = Film::default();
            single.update_slots(at, |_, i| point(channel, i)).unwrap();
            for rgb in [
                [0, 0, 0],
                [1000, 2000, 3000],
                [6000, 3000, 2000],
                [16383; 3],
            ] {
                assert_eq!(
                    film.color(rgb)[channel],
                    single.color(rgb)[channel],
                    "channel {channel} input {rgb:?}"
                );
            }
        }
    }

    #[test]
    fn equal_slots_develop_like_the_single_curve_law() {
        let words = fujimod_emulator_devices::test_host::film_words();
        let at = |o: usize| words[o / 4];
        let mut single = Film::default();
        single.update(at).unwrap();
        let mut slots = Film::default();
        slots.update_slots(at, |_, i| at(0x508 + 4 * i)).unwrap();
        for rgb in [
            [0; 3],
            [1, 1, 1],
            [1000, 2000, 3000],
            [6000, 3000, 2000],
            [16383; 3],
            [0, 16383, 0],
        ] {
            assert_eq!(slots.color(rgb), single.color(rgb));
            assert_eq!(slots.develop(rgb), single.develop(rgb));
        }
        let mut pixels = FilmPixels::new(&slots);
        for i in 0..4096 {
            let rgb = [i as u16 * 3, i as u16, (i as u16).wrapping_mul(7)];
            assert_eq!(pixels.develop(rgb), single.develop(rgb), "input {rgb:?}");
        }
    }

    #[test]
    fn hue_stage_matches_the_camera_hue_map() {
        // Factory Partial Color red banks 7 and 1 turned to 131 deg.
        let words = |enable: u32, bytes: u32| {
            move |o: usize| match o {
                0x500 => enable,
                0x544 => 0x033c_00e2,
                0x548 => 515,
                0x550 => 0x02bd_0174,
                0x554 => 1736,
                0x55c => bytes,
                _ => 0,
            }
        };
        assert!(HueStage::from_still(words(0, 0x0010_0010)).is_none());
        let at = |stage: &HueStage, offset: f64| {
            let (s, c) = (131.0 + offset).to_radians().sin_cos();
            f64::from(stage.gain((100.0 * c) as i32, (100.0 * s) as i32)) / 1024.0
        };
        let half = HueStage::from_still(words(1 << 12, 0x0008_0008)).unwrap();
        let byte0 = HueStage::from_still(words(1 << 12, 0x0000_0010)).unwrap();
        let equal = HueStage::from_still(words(1 << 12, 0x0010_0010)).unwrap();
        // Card 4 medians at these offsets from the axis.
        for (offset, measured) in [(-130.4, 0.03), (-54.3, 0.99), (39.7, 0.18), (144.9, 1.0)] {
            assert!((at(&half, offset) - 0.5).abs() < 0.01);
            assert!((at(&byte0, offset) - measured).abs() < 0.04, "{offset}");
        }
        assert!((at(&equal, 8.8) - 1.21).abs() < 0.08 && (at(&equal, 90.0) - 1.0).abs() < 0.01);
        let mut film = Film {
            chroma: [1024, 0, 1024, 0],
            bands: [(0, 256); 5],
            bounds: [255; 4],
            curves: std::array::from_fn(|_| (0..4096).map(|x| (x * 4) as u16).collect()),
            matrix: [[1024, 0, 0], [0, 1024, 0], [0, 0, 1024]],
            gain: vec![2048; 4096],
            ..Default::default()
        };
        let plain = film.develop([9000, 5000, 3000]);
        film.hue = Some(half);
        let staged = film.develop([9000, 5000, 3000]);
        assert_eq!(plain[0], staged[0]);
        for c in 1..3 {
            assert!(((i32::from(plain[c]) - 128) / 2 - (i32::from(staged[c]) - 128)).abs() <= 1);
        }
    }
    #[test]
    fn hue_stage_without_an_axis_tints_gray() {
        // Sepia's code constants: no axis, width 512, vector (136, -120).
        let sepia = |o: usize| match o {
            0x500 => 1 << 12,
            0x548 => 512,
            0x54c => (136 & 0x1fff) | (-120i32 as u32 & 0x1fff) << 16,
            0x55c => 0x0010_0010,
            _ => 0,
        };
        let stage = HueStage::from_still(sepia).unwrap();
        assert_eq!(stage.tint, [-15, 17]);
        let mut film = Film {
            bands: [(0, 256); 5],
            bounds: [255; 4],
            curves: std::array::from_fn(|_| (0..4096).map(|x| (x * 4) as u16).collect()),
            matrix: [[1024, 0, 0], [0, 1024, 0], [0, 0, 1024]],
            gain: vec![2048; 4096],
            ..Default::default()
        };
        let gray = film.develop([6000, 6000, 6000]);
        film.hue = Some(stage);
        let toned = film.develop([6000, 6000, 6000]);
        assert_eq!((gray[1], gray[2]), (128, 128));
        assert_eq!((toned[0], toned[1], toned[2]), (gray[0], 113, 145));
    }
    #[test]
    fn zero_and_signed_matrix_law() {
        let mut f = Film::default();
        f.update(|_| 0).unwrap();
        assert_eq!(f.develop([16383; 3]), [0, 128, 128]);
        assert_eq!(signed(0x1000, 13), -4096);
        assert_eq!(signed(0x7ff, 11), -1);
    }
}

/// Host sRGB interpreted as linear camera RGB. Sensor reference white is
/// supplied by the body; register WB replaces it by ratio. All transcendental
/// math is confined to this cached table, using libm on every target.
pub struct RgbFilm {
    pub film: Film,
    linear: Vec<u16>,
    key: Option<([u16; 3], Option<[u16; 3]>)>,
}
impl Default for RgbFilm {
    fn default() -> Self {
        Self {
            film: Film::default(),
            linear: vec![0; 768],
            key: None,
        }
    }
}
impl RgbFilm {
    pub fn update(
        &mut self,
        r: impl Fn(usize) -> u32,
        reference_white: Option<[u16; 3]>,
    ) -> Result<(), String> {
        self.film.update(&r)?;
        let gains = [
            (r(0x200) & 32767) as u16,
            (r(0x200) >> 16 & 32767) as u16,
            (r(0x204) & 32767) as u16,
        ];
        if reference_white.is_some_and(|w| w.contains(&0)) {
            return Err("film reference white is zero".into());
        }
        let key = (gains, reference_white);
        if self.key == Some(key) {
            return Ok(());
        }
        for c in 0..3 {
            let ratio = reference_white.map_or(1., |w| gains[c] as f64 / w[c] as f64);
            for i in 0..256 {
                let s = i as f64 / 255.;
                let v = if s <= 0.04045 {
                    s / 12.92
                } else {
                    libm::pow((s + 0.055) / 1.055, 2.4)
                };
                self.linear[c * 256 + i] =
                    libm::rint(v * 4095. * ratio * 4.).clamp(0., 16383.) as u16;
            }
        }
        self.key = Some(key);
        Ok(())
    }
    pub fn pixel(&self, rgb: [u8; 3]) -> [u8; 3] {
        self.film.develop(std::array::from_fn(|c| {
            self.linear[c * 256 + rgb[c] as usize]
        }))
    }
    /// Nearest pixel-center mapping through a declared sensor grid/crop.
    /// Padding stays in the caller's buffer. No intermediate full raster.
    pub fn render(
        &self,
        rgb: &[u8],
        source: [usize; 2],
        grid: [usize; 2],
        crop: [usize; 4],
        size: [usize; 2],
        pitch: usize,
        out: &mut [u8],
    ) -> Result<(), String> {
        let [sw, sh] = source;
        let [gw, gh] = grid;
        let [cx, cy, cw, ch] = crop;
        let [w, h] = size;
        if sw == 0
            || sh == 0
            || sw > 8192
            || sh > 8192
            || rgb.len() != sw * sh * 3
            || gw == 0
            || gh == 0
            || gw > 8192
            || gh > 8192
            || cw == 0
            || ch == 0
            || cx >= gw
            || cy >= gh
            || cw > gw - cx
            || ch > gh - cy
            || w == 0
            || w % 2 != 0
            || w > 8192
            || h == 0
            || h > 8192
            || pitch < w * 2
            || out.len() != pitch * h
        {
            return Err("invalid host film raster/crop".into());
        }
        for y in 0..h {
            let gy = cy + (2 * y + 1) * ch / (2 * h);
            let sy = ((2 * gy + 1) * sh / (2 * gh)).min(sh - 1);
            for x in (0..w).step_by(2) {
                let pixel = |x: usize| {
                    let gx = cx + (2 * x + 1) * cw / (2 * w);
                    let sx = ((2 * gx + 1) * sw / (2 * gw)).min(sw - 1);
                    let i = (sy * sw + sx) * 3;
                    self.pixel(rgb[i..i + 3].try_into().unwrap())
                };
                let at = y * pitch + x * 2;
                out[at..at + 4].copy_from_slice(&Film::pack(pixel(x), pixel(x + 1)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod color_tests {
    use super::*;
    #[test]
    fn neutral_matrix_tone_and_chroma() {
        let r = fujimod_emulator_devices::test_host::film_words();
        let mut f = Film::default();
        f.update(|o| r[o / 4]).unwrap();
        assert_eq!(f.develop([1024; 3]), [16, 128, 128]);
        assert_eq!(f.develop([4096; 3]), [64, 128, 128]);
        assert_eq!(f.develop([4096, 0, 0]), [19, 117, 160]);
        let lut = f.curves[0].as_ptr();
        f.update(|o| r[o / 4]).unwrap();
        assert_eq!(f.curves[0].as_ptr(), lut);
    }
    #[test]
    fn host_film_lut_crop_and_padding() {
        let r = fujimod_emulator_devices::test_host::film_words();
        let mut f = RgbFilm::default();
        f.update(|o| r[o / 4], Some([256; 3])).unwrap();
        assert_eq!(f.linear[0], 0);
        assert_eq!(f.linear[255], 16380);
        let mut out = [0xa5; 8];
        f.render(
            &[0, 0, 0, 255, 255, 255],
            [2, 1],
            [2, 1],
            [0, 0, 2, 1],
            [2, 1],
            8,
            &mut out,
        )
        .unwrap();
        assert_eq!(&out[4..], &[0xa5; 4]);
        assert_eq!(&out[..4], &Film::pack(f.pixel([0; 3]), f.pixel([255; 3])));
    }
}
