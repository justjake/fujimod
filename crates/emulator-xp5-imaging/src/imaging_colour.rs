//! Colour path of the RIFP still develop (X-T5 4.31): the words the stock firmware programs
//! into the RIFP sub-block pages, and what a develop run does with them.
//!
//! # Where the words come from (static decode; `[S]` seen in the disassembly, `[G]` guess)
//!
//! A develop job builds a hardware parameter block (0xf70 bytes, 0x2e56fa4) from its function
//! switches and from the camera parameter store (0x1494efc / 0x14951b0 read a byte / a 16-bit
//! value by parameter id from the store at [0x2066d90]) `[S]`. 0x2c6dc74 packs each part of
//! that block into a register image and 0x2c6de00 / 0x2c6df78 write the images through one
//! table per block of `{address, image offset, count}` rows (0x18db1e8.., generic writer
//! 0x1370e54; with a register queue the writes go through the RIFP I/O DMA) `[S]`:
//!
//! | Image | Pack | Write | Registers |
//! |---|---|---|---|
//! | +0x000 | 0x1378a48 | 0x1378a68 | 0xffd04000..0xffd040e8 |
//! | +0x0ec | 0x1371bcc | 0x1371bec | 0xffd06000..0xffd060a8 (clip words 0xffd06088..) |
//! | +0x198 | 0x13777c8 | 0x13777e8 | 0xffd06100..0xffd06174, 0xffd06200..0xffd06330, 0xffd06400 |
//! | +0x348 | 0x1370b10 | 0x1370c54 | 0xffd07000.. and 0xffd0b800 |
//! | +0x3c8 | 0x1372f48 | 0x1372f5c | 0xffd07100, 0xffd07200..0xffd07274, 0xffd07300..0xffd0736c |
//! | +0x4b4, +0x4f4 | 0x13768dc, 0x1376c04 | 0x13768fc, 0x1376c24 | 0xffd08000.., 0xffd08100.. |
//! | +0x59c | 0x1377e04 | 0x1377e24 | 0xffd0a010..0xffd0a098 |
//! | +0x5f8..+0x8f8 | 0x13716d4.. | 0x137172c.. | 0xffd0b000..0xffd0b724 |
//!
//! # Modelled words
//!
//! Offsets below are from 0xffd02000 (the RIFP window, which holds these pages).
//!
//! * Colour gains, 0xffd06168 bits 9..0 and 25..16 and 0xffd0616c bits 9..0 (pack 0x1376e34:
//!   three 10-bit fields) `[S]`; unity 0x100 (the firmware programs 0x100 three times when it
//!   has no white balance result) `[S]`; the order red, green, blue is `[G]`.
//! * Stage switches, 0xffd06200: bit 8 from the job's matrix switch, bit 16 from its tone
//!   curve switch (pack 0x1377240; the parameter builder calls the matrix builder 0x2e668c0
//!   and the curve builder 0x2e676c8 when those switches are 1) `[S]`.
//! * Colour matrix, 0xffd06204..0xffd06218: nine 13-bit signed coefficients, two a word (bits
//!   12..0 and 28..16), the third of each row alone in the next word (pack 0x13772d4) `[S]`;
//!   unity 0x400 is `[G]` (the scale of the YCbCr matrix below).
//! * Tone curves, three tables of 31 points of 14 bits, two a word: 0xffd06224..0xffd06260,
//!   0xffd06264..0xffd062a0 and 0xffd062a4..0xffd062e0 (pack 0x1377414, built by 0x2e676c8)
//!   `[S]`; the builder copies one table to all three (0x2e65d60), red, green, blue in that
//!   order is `[G]`. The tables come from the firmware's list of 34 curves (0x1bc8ce8, 0x4c
//!   bytes each: id, ten step bytes, 31 points; lookup 0x1372f9c) `[S]`. Ten 4-bit step
//!   fields, 0xffd0621c / 0xffd06220: each field is the binary logarithm of the input width
//!   of three consecutive segments, in units of the input range over the sum of the widths.
//!   Every curve of the list sums to 1536 units, and the list's straight line (id 16) has
//!   points that rise by 16383 / 1536 a unit `[S]`. That the 1536 units span the whole 14-bit
//!   input (not three quarters of it) is `[G]`.
//! * YCbCr block switches, 0xffd07100 (pack 0x1371c20): bit 4 <- the chroma matrix switch
//!   `[S]`.
//! * YCbCr matrix, 0xffd07200..0xffd07210: nine 13-bit signed coefficients, two a word, in
//!   the order Y(R, G, B), Cb(R, G, B), Cr(R, G, B) `[S]`; unity 0x400: the firmware programs
//!   306, 601, 117 / -173, -339, 512 / 512, -429, -83, BT.601 full range `[S]`.
//! * Chroma matrix, 0xffd07234 (m00 bits 12..0, m01 bits 28..16) and 0xffd07238 (m11 bits
//!   12..0, m10 bits 28..16), 13-bit signed, unity 0x400: the builder 0x2e5f7dc writes the
//!   identity 0x400, 0, 0, 0x400 when the stage is off, else a table entry of the job's film
//!   simulation (0x2e5f4a8) scaled by the three saturation bytes; the monochrome film
//!   simulations get a zero matrix `[S]`. Row 0 is Cb, row 1 is Cr `[G]`. The firmware keeps
//!   the RGB matrix mild and sets the saturation here (gains of 1.3 to 2.5) `[S]` (the
//!   parameter step 0x2d2f608 run on the host for each film simulation index).
//!
//! # What a run does with them ([`ColourPath::ycbcr`])
//!
//! Input: linear 14-bit red, green and blue after the sensor response is inverted (the front
//! end, black level, white balance and demosaic are not decoded; see
//! [`fujimod_emulator_imaging::xtrans`]). Then, in this order `[G]`: colour gains; the matrix
//! when its switch is set; the tone curves when their switch is set, else the declared sRGB
//! encoding; 14 to 8 bits; the YCbCr matrix when it is programmed, else BT.601 full range;
//! the chroma matrix when its switch is set. A path with no gain other than unity, no matrix,
//! no curve and no chroma matrix is the declared law ([`ColourPath::is_declared`]): the
//! caller keeps the declared development, bit for bit. The 11-bit and 10-bit fields of the
//! YCbCr block, noise reduction, sharpening, the film grain and the later blocks are not
//! modelled, nor is the rounding of each stage.
use fujimod_emulator_imaging::xtrans;

/// Largest 14-bit level.
const FULL: i32 = 0x3fff;
/// Points of one tone curve.
pub const POINTS: usize = 31;

const GAINS: u64 = 0x4168;
const SWITCHES: u64 = 0x4200;
const MATRIX: u64 = 0x4204;
const STEPS: u64 = 0x421c;
const CURVES: [u64; 3] = [0x4224, 0x4264, 0x42a4];
const YC_SWITCHES: u64 = 0x5100;
const YCBCR: u64 = 0x5200;
const CHROMA: u64 = 0x5234;

/// The colour path words of one RIFP run (see the module notes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColourPath {
    /// Colour gains (red, green, blue), unity 0x100.
    pub gains: [i32; 3],
    /// The colour matrix when its switch is set, unity 0x400.
    pub matrix: Option<[[i32; 3]; 3]>,
    /// The tone curves (red, green, blue) when their switch is set: the output of each 14-bit
    /// input level.
    curves: Option<Box<[Vec<u16>; 3]>>,
    /// The programmed curve points and steps, for the run's log line.
    points: Option<([[u16; POINTS]; 3], [u8; 10])>,
    /// The YCbCr matrix when it is programmed, unity 0x400.
    pub ycbcr: Option<[[i32; 3]; 3]>,
    /// The chroma matrix (rows Cb, Cr) when its switch is set, unity 0x400.
    pub chroma: Option<[[i32; 2]; 2]>,
}

/// A 13-bit signed field.
fn signed13(field: u32) -> i32 {
    ((field & 0x1fff) as i32) << 19 >> 19
}

/// Nine 13-bit signed fields from `at`, two a word, read as three rows whose third field has
/// a word alone (`packed` false: the colour matrix) or as nine consecutive halves (`packed`
/// true: the YCbCr matrix).
fn matrix(word: &impl Fn(u64) -> u32, at: u64, packed: bool) -> [[i32; 3]; 3] {
    let half = |index: u64| signed13(word(at + index / 2 * 4) >> (index % 2 * 16));
    std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            let (row, column) = (row as u64, column as u64);
            half(if packed { 3 * row + column } else { 4 * row + column })
        })
    })
}

/// The input level of each curve point: three segments a step field, each `1 << field` units
/// wide, the units scaled so that the last point is at the end of the 14-bit range (see the
/// module notes). Step fields of zero give evenly spaced points.
fn inputs(steps: &[u8; 10]) -> [i32; POINTS] {
    let mut units = [0i64; POINTS];
    for segment in 0..POINTS - 1 {
        units[segment + 1] = units[segment] + (1 << steps[segment / 3]);
    }
    let total = units[POINTS - 1];
    units.map(|unit| (unit * i64::from(FULL + 1) / total) as i32)
}

/// The output of every 14-bit input level of a curve of `points` at `inputs`, linear between
/// points.
fn expand(points: &[u16; POINTS], inputs: &[i32; POINTS]) -> Vec<u16> {
    let mut segment = 0;
    (0..=FULL)
        .map(|level| {
            while segment + 2 < POINTS && level >= inputs[segment + 1] {
                segment += 1;
            }
            let (x0, x1) = (inputs[segment], inputs[segment + 1]);
            let (y0, y1) = (i32::from(points[segment]), i32::from(points[segment + 1]));
            let y = y0 + (y1 - y0) * (level - x0).clamp(0, x1 - x0) / (x1 - x0).max(1);
            y.clamp(0, FULL) as u16
        })
        .collect()
}

impl ColourPath {
    /// The colour path of the RIFP words `word` gives (offsets from 0xffd02000).
    pub fn from_words(word: impl Fn(u64) -> u32) -> Self {
        let gains = [word(GAINS) & 0x3ff, word(GAINS) >> 16 & 0x3ff, word(GAINS + 4) & 0x3ff]
            .map(|gain| if gain == 0 { 0x100 } else { gain as i32 });
        let switches = word(SWITCHES);
        let steps: [u8; 10] =
            std::array::from_fn(|i| (word(STEPS + i as u64 / 8 * 4) >> (i % 8 * 4) & 0xf) as u8);
        let points: [[u16; POINTS]; 3] = std::array::from_fn(|curve| {
            std::array::from_fn(|point| {
                let point = point as u64;
                (word(CURVES[curve] + point / 2 * 4) >> (point % 2 * 16) & 0x3fff) as u16
            })
        });
        // A curve switch without points (all zero) would develop black: not a programmed curve.
        let curved = switches & 1 << 16 != 0 && points.iter().flatten().any(|point| *point != 0);
        let at = inputs(&steps);
        let ycbcr = matrix(&word, YCBCR, true);
        let colour = matrix(&word, MATRIX, false);
        let half = |at: u64, high: bool| signed13(word(at) >> if high { 16 } else { 0 });
        let chroma = [
            [half(CHROMA, false), half(CHROMA, true)],
            [half(CHROMA + 4, true), half(CHROMA + 4, false)],
        ];
        Self {
            gains,
            matrix: (switches & 1 << 8 != 0 && colour != [[0; 3]; 3]).then_some(colour),
            curves: curved.then(|| Box::new(points.each_ref().map(|curve| expand(curve, &at)))),
            points: curved.then_some((points, steps)),
            ycbcr: (ycbcr != [[0; 3]; 3]).then_some(ycbcr),
            // A zero matrix with the switch set is a programmed stage: the monochrome film
            // simulations.
            chroma: (word(YC_SWITCHES) & 1 << 4 != 0 && chroma != IDENTITY).then_some(chroma),
        }
    }
    /// No stage of the firmware changes the declared development: unity gains, no matrix, no
    /// curve, and a YCbCr matrix that is absent or BT.601 full range.
    pub fn is_declared(&self) -> bool {
        self.gains == [0x100; 3]
            && self.matrix.is_none()
            && self.curves.is_none()
            && self.ycbcr.is_none_or(|ycbcr| ycbcr == BT601)
            && self.chroma.is_none()
    }
    /// Y, Cb and Cr (not clamped) of linear 14-bit red, green and blue (see the module notes).
    pub fn ycbcr(&self, linear: [u16; 3]) -> [i32; 3] {
        let mut rgb: [i32; 3] =
            std::array::from_fn(|c| ((i32::from(linear[c]) * self.gains[c]) >> 8).min(FULL));
        if let Some(matrix) = &self.matrix {
            rgb = matrix.map(|row| {
                ((row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2] + 512) >> 10).clamp(0, FULL)
            });
        }
        let bytes: [i32; 3] = match &self.curves {
            Some(curves) => std::array::from_fn(|c| i32::from(curves[c][rgb[c] as usize] >> 6)),
            None => rgb.map(|level| i32::from(srgb()[level as usize])),
        };
        let ycbcr = self.ycbcr.unwrap_or(BT601);
        let row = |row: [i32; 3]| (row[0] * bytes[0] + row[1] * bytes[1] + row[2] * bytes[2] + 512) >> 10;
        let (y, mut cb, mut cr) = (row(ycbcr[0]), row(ycbcr[1]), row(ycbcr[2]));
        if let Some(m) = &self.chroma {
            (cb, cr) = ((m[0][0] * cb + m[0][1] * cr + 512) >> 10, (m[1][0] * cb + m[1][1] * cr + 512) >> 10);
        }
        [y, cb + 128, cr + 128]
    }
    /// One line for the run's log: which stages are programmed.
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("gains {:#x?}", self.gains)];
        if let Some(matrix) = &self.matrix {
            parts.push(format!("matrix {matrix:?}"));
        }
        if let Some((points, steps)) = &self.points {
            parts.push(format!(
                "tone curves (steps {steps:?}) red {:?} green {:?} blue {:?}",
                points[0], points[1], points[2]
            ));
        }
        if let Some(chroma) = &self.chroma {
            parts.push(format!("chroma matrix {chroma:?}"));
        }
        match &self.ycbcr {
            Some(ycbcr) if *ycbcr != BT601 => parts.push(format!("YCbCr matrix {ycbcr:?}")),
            Some(_) => parts.push("YCbCr matrix BT.601 full range".into()),
            None => parts.push("no YCbCr matrix".into()),
        }
        parts.join(", ")
    }
}

/// The YCbCr matrix the stock firmware programs: BT.601 full range, unity 0x400.
pub const BT601: [[i32; 3]; 3] = [[306, 601, 117], [-173, -339, 512], [512, -429, -83]];
/// The chroma matrix of a stage that is off: unity 0x400.
const IDENTITY: [[i32; 2]; 2] = [[0x400, 0], [0, 0x400]];

/// The declared sRGB byte of each linear 14-bit level.
fn srgb() -> &'static [u8] {
    static LUT: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        (0..=FULL).map(|level| xtrans::srgb_byte(f64::from(level) / f64::from(FULL))).collect()
    })
}

/// The linear 14-bit level of each sample of each colour under `response` (the inverted
/// sensor response: scene black 0, scene white [`FULL`], clamped).
pub fn linear_levels(response: &xtrans::Response) -> Box<[Vec<u16>; 3]> {
    Box::new(std::array::from_fn(|c| {
        (0..=u16::MAX)
            .map(|sample| {
                (response.linear(c, sample).clamp(0., 1.) * f64::from(FULL)).round() as u16
            })
            .collect()
    }))
}

/// One packed 4:2:2 line (Y0 Y1 Cb Cr) of `width` pixels (rounded up to a pair; an odd last
/// pixel repeats) developed from line `y` of `raw` through `path`: pixel `x` takes the nearest
/// site of each colour of sample `column(x)`, linearised by `levels` ([`linear_levels`]).
pub fn develop_422_line(
    raw: &xtrans::Raw<'_>,
    path: &ColourPath,
    levels: &[Vec<u16>; 3],
    y: usize,
    width: usize,
    column: impl Fn(usize) -> usize,
) -> Vec<u8> {
    let byte = |v: i32| v.clamp(0, 255) as u8;
    let pairs = width.div_ceil(2);
    let mut line = Vec::with_capacity(4 * pairs);
    for pair in 0..pairs {
        let [a, b] = [0, 1].map(|i| {
            let x = (2 * pair + i).min(width - 1);
            let sites = raw.sites(column(x), y);
            path.ycbcr(std::array::from_fn(|c| levels[c][usize::from(sites[c])]))
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
    use std::collections::BTreeMap;

    /// The words of a run: the YCbCr matrix and unity gains the stock firmware programs.
    fn stock() -> BTreeMap<u64, u32> {
        BTreeMap::from([
            (0x4168, 0x0100_0100),
            (0x416c, 0x100),
            (0x5200, 0x0259_0132),
            (0x5204, 0x1f53_0075),
            (0x5208, 0x0200_1ead),
            (0x520c, 0x1e53_0200),
            (0x5210, 0x1fad),
        ])
    }
    fn path(words: &BTreeMap<u64, u32>) -> ColourPath {
        ColourPath::from_words(|offset| words.get(&offset).copied().unwrap_or(0))
    }
    /// Program one curve table: `points` two a word.
    fn curve(words: &mut BTreeMap<u64, u32>, at: u64, points: &[u16; POINTS]) {
        for (i, pair) in points.chunks(2).enumerate() {
            let high = pair.get(1).copied().unwrap_or(0);
            words.insert(at + 4 * i as u64, u32::from(pair[0]) | u32::from(high) << 16);
        }
    }

    #[test]
    fn the_stock_words_without_switches_are_the_declared_law() {
        let stock = path(&stock());
        assert_eq!(stock.ycbcr, Some(BT601));
        assert!(stock.is_declared());
        assert!(path(&BTreeMap::new()).is_declared());
        // Declared: sRGB encoding, then BT.601 full range. Mid grey stays neutral.
        let [y, cb, cr] = stock.ycbcr([0x1000; 3]);
        assert_eq!(y, i32::from(xtrans::srgb_byte(f64::from(0x1000) / f64::from(FULL))));
        assert_eq!((cb, cr), (128, 128));
        assert_eq!(stock.ycbcr([FULL as u16; 3]), [255, 128, 128]);
    }

    #[test]
    fn a_programmed_tone_curve_replaces_the_declared_encoding() {
        let mut words = stock();
        words.insert(0x4200, 1 << 16);
        // Evenly spaced points (no step fields): a straight line and a line at half height.
        let line: [u16; POINTS] = std::array::from_fn(|i| ((i * 0x3fff) / (POINTS - 1)) as u16);
        let half = line.map(|point| point / 2);
        for at in CURVES {
            curve(&mut words, at, &line);
        }
        let straight = path(&words);
        assert!(!straight.is_declared());
        // A straight curve: the byte is the level's top 8 bits (within the points' rounding).
        let [y, cb, cr] = straight.ycbcr([0x2000; 3]);
        assert!((y - 0x80).abs() <= 1 && (cb, cr) == (128, 128), "{y} {cb} {cr}");
        assert_eq!(straight.ycbcr([0x3fff; 3]), [255, 128, 128]);
        for at in CURVES {
            curve(&mut words, at, &half);
        }
        let dark = path(&words);
        let [y, cb, cr] = dark.ycbcr([0x2000; 3]);
        assert!((y - 0x40).abs() <= 1 && (cb, cr) == (128, 128), "{y} {cb} {cr}");
        // Different curves a colour: grey is no longer neutral.
        curve(&mut words, CURVES[0], &line);
        let [_, cb, cr] = path(&words).ycbcr([0x2000; 3]);
        assert!(cr > 128 && cb < 128, "red curve above the others: Cb {cb} Cr {cr}");
        // The switch alone, without points, is not a programmed curve.
        let mut empty = stock();
        empty.insert(0x4200, 1 << 16);
        assert!(path(&empty).is_declared());
    }

    #[test]
    fn curve_points_follow_the_step_fields() {
        // No step fields: 30 equal segments.
        let even = inputs(&[0; 10]);
        assert_eq!((even[0], even[1], even[30]), (0, (FULL + 1) / 30, FULL + 1));
        // Three segments a field, each 2^field units; 1536 units span the input range.
        let steps = [3, 3, 4, 5, 6, 6, 6, 7, 6, 6];
        let at = inputs(&steps);
        assert_eq!(at[..7], [0, 85, 170, 256, 341, 426, 512]);
        assert_eq!((at[12], at[13], at[24], at[30]), (2048, 2730, 12288, FULL + 1));
        // A straight line through those points is the identity, whatever the steps.
        let ramp: [u16; POINTS] = std::array::from_fn(|i| (at[i].min(FULL)) as u16);
        let table = expand(&ramp, &at);
        assert_eq!(table.len(), 0x4000);
        assert!(table.iter().enumerate().all(|(level, out)| (i32::from(*out) - level as i32).abs() <= 1));
        // The step fields come from 0xffd0621c (eight) and 0xffd06220 (two), four bits each.
        let mut words = stock();
        words.insert(0x4200, 1 << 16);
        words.insert(0x421c, 0x7666_5433);
        words.insert(0x4220, 0x66);
        curve(&mut words, CURVES[0], &ramp);
        curve(&mut words, CURVES[1], &ramp);
        curve(&mut words, CURVES[2], &ramp);
        let identity = path(&words);
        assert_eq!(identity.points.as_ref().unwrap().1, steps);
        let [y, cb, cr] = identity.ycbcr([2048; 3]);
        assert_eq!((y, cb, cr), (2048 >> 6, 128, 128));
    }

    #[test]
    fn the_chroma_matrix_scales_the_colour_differences() {
        let colour = [0x3000u16, 0x1000, 0x200];
        let plain = path(&stock()).ycbcr(colour);
        // Switch (0xffd07100 bit 4) and a matrix of twice unity on both rows.
        let mut words = stock();
        words.insert(0x5100, 1 << 4);
        words.insert(0x5234, 0x800);
        words.insert(0x5238, 0x800);
        let double = path(&words);
        assert_eq!(double.chroma, Some([[0x800, 0], [0, 0x800]]));
        assert!(!double.is_declared());
        let out = double.ycbcr(colour);
        assert_eq!(out[0], plain[0]);
        assert_eq!((out[1] - 128, out[2] - 128), (2 * (plain[1] - 128), 2 * (plain[2] - 128)));
        // The cross terms: m01 in bits 28..16 of the first word, m10 in bits 28..16 of the second.
        words.insert(0x5234, 0x0400_0000);
        words.insert(0x5238, 0x1c00_0000);
        let crossed = path(&words);
        assert_eq!(crossed.chroma, Some([[0, 0x400], [-0x400, 0]]));
        let out = crossed.ycbcr(colour);
        assert_eq!((out[1] - 128, out[2] - 128), (plain[2] - 128, -(plain[1] - 128)));
        // A zero matrix with the switch set removes the colour (the monochrome films).
        words.insert(0x5234, 0);
        words.insert(0x5238, 0);
        let mono = path(&words);
        assert_eq!(mono.chroma, Some([[0; 2]; 2]));
        assert_eq!(mono.ycbcr(colour), [plain[0], 128, 128]);
        // Without the switch, or with the identity, the stage is off.
        words.remove(&0x5100);
        assert!(path(&words).chroma.is_none());
        let mut identity = stock();
        identity.insert(0x5100, 1 << 4);
        identity.insert(0x5234, 0x400);
        identity.insert(0x5238, 0x400);
        assert!(path(&identity).is_declared());
    }

    #[test]
    fn gains_and_the_matrix_apply_before_the_curve() {
        let mut words = stock();
        // Red gain 2.0.
        words.insert(0x4168, 0x0100_0200);
        let gained = path(&words);
        assert!(!gained.is_declared());
        let [_, cb, cr] = gained.ycbcr([0x800; 3]);
        assert!(cr > 128 && cb < 128);
        // A matrix that swaps red and blue (unity 0x400), switch bit 8.
        let mut words = stock();
        words.insert(0x4200, 1 << 8);
        words.insert(0x4204, 0);
        words.insert(0x4208, 0x400);
        words.insert(0x420c, 0x0400_0000);
        words.insert(0x4210, 0);
        words.insert(0x4214, 0x400);
        words.insert(0x4218, 0);
        let swapped = path(&words);
        assert_eq!(swapped.matrix, Some([[0, 0, 0x400], [0, 0x400, 0], [0x400, 0, 0]]));
        let plain = path(&stock());
        assert_eq!(swapped.ycbcr([0x3000, 0x1000, 0x200]), plain.ycbcr([0x200, 0x1000, 0x3000]));
        // Negative coefficients are 13-bit signed.
        assert_eq!(signed13(0x1fad), -83);
    }

    #[test]
    fn a_flat_patch_develops_through_the_programmed_path() {
        let (width, lines, pitch) = (24, 12, 64);
        let rgb = [200u8, 100, 30].repeat(width);
        let mut bytes = vec![0u8; pitch * lines];
        for y in 0..lines {
            xtrans::mosaic_line(&rgb, y, &mut bytes[y * pitch..y * pitch + 2 * width]);
        }
        let raw = xtrans::Raw::new(&bytes, pitch, width, lines, (0, 0)).unwrap();
        let levels = linear_levels(&xtrans::Response::NEUTRAL);
        // The declared path through the model equals the declared development within the
        // rounding of the 14-bit intermediate.
        let declared = xtrans::develop_422_line(&raw, 5, 8, |x| x);
        let modelled = develop_422_line(&raw, &path(&stock()), &levels, 5, 8, |x| x);
        for (a, b) in declared.iter().zip(&modelled) {
            assert!((i32::from(*a) - i32::from(*b)).abs() <= 1, "{declared:?} {modelled:?}");
        }
        // A programmed curve changes the patch.
        let mut words = stock();
        words.insert(0x4200, 1 << 16);
        let line: [u16; POINTS] = std::array::from_fn(|i| ((i * 0x3fff) / (POINTS - 1)) as u16);
        for at in CURVES {
            curve(&mut words, at, &line);
        }
        let linear = develop_422_line(&raw, &path(&words), &levels, 5, 8, |x| x);
        assert_ne!(linear, modelled);
        assert!(linear[0] < modelled[0], "a straight curve is darker than the sRGB encoding");
    }
}
