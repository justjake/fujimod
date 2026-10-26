//! RAW transport and nearest same-color reconstruction. CFA layout is body
//! data. Full-field nearest sampling is a declared spatial approximation,
//! separate from the measured integer film arithmetic.
use fujimod_emulator_imaging::film::{Film, FilmPixels};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Packing {
    U16,
    Packed12,
}
impl Packing {
    pub fn row_bytes(self, width: usize) -> usize {
        width * if self == Self::U16 { 4 } else { 3 } / 2
    }
}

pub struct RawView<'a> {
    pub bytes: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub packing: Packing,
}
impl RawView<'_> {
    pub fn validate(&self) -> Result<(), String> {
        if self.width < 2
            || self.width > 8192
            || !self.width.is_multiple_of(2)
            || self.height == 0
            || self.height > 8192
            || self.pitch < self.packing.row_bytes(self.width)
            || self.pitch.checked_mul(self.height) != Some(self.bytes.len())
        {
            return Err("unsupported RAW raster".into());
        }
        Ok(())
    }
    /// PDAF transport restores packed12 to the U14 domain; still color
    /// consumes the stored U12 value directly through `sample`.
    pub fn phase_sample(&self, x: usize, y: usize) -> u16 {
        let v = self.sample(x, y);
        if self.packing == Packing::Packed12 {
            v << 2
        } else {
            v
        }
    }
    #[inline]
    pub fn sample(&self, x: usize, y: usize) -> u16 {
        match self.packing {
            Packing::U16 => self.sample_as::<false>(x, y),
            Packing::Packed12 => self.sample_as::<true>(x, y),
        }
    }
    #[inline(always)]
    fn byte_offset<const PACKED12: bool>(x: usize) -> usize {
        if PACKED12 { x * 3 / 2 } else { x * 2 }
    }
    #[inline(always)]
    fn sample_as<const PACKED12: bool>(&self, x: usize, y: usize) -> u16 {
        self.sample_from_offset::<PACKED12>(y * self.pitch + Self::byte_offset::<PACKED12>(x), x)
    }
    #[inline(always)]
    fn sample_from_offset<const PACKED12: bool>(&self, i: usize, x: usize) -> u16 {
        let v = u16::from_le_bytes([self.bytes[i], self.bytes[i + 1]]);
        if !PACKED12 {
            v
        } else if x & 1 == 0 {
            v & 4095
        } else {
            v >> 4
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cfa {
    layout: Vec<u8>,
    nearest: Vec<[(i8, i8); 3]>,
}

// RawView caps width at 8192: x <= 8191, so horizontal byte offsets top out
// at 16382 for U16 and 12286 for Packed12; both values fit these u16 fields.
#[derive(Clone, Copy)]
struct CfaColumnSample {
    x: u16,
    byte_offset: u16,
}

#[derive(Clone, Copy)]
struct CfaColumn {
    phase: u8,
    samples: [[CfaColumnSample; 3]; 6],
}

#[derive(Clone, Copy)]
struct CfaRowSample {
    y: usize,
    byte_offset: usize,
}

impl Cfa {
    /// Six-by-six RGB color indices (0/1/2), supplied by the body graph.
    pub fn new(layout: Vec<u8>) -> Result<Self, String> {
        if layout.len() != 36
            || layout.iter().any(|&v| v > 2)
            || (0..3).any(|c| !layout.contains(&c))
        {
            return Err("CFA must contain a complete six-by-six RGB tile".into());
        }
        let mut nearest = vec![[(0, 0); 3]; 36];
        for y in 0..6 {
            for x in 0..6 {
                for c in 0..3 {
                    let (_, dy, dx) = (-3i32..=3)
                        .flat_map(|dy| (-3i32..=3).map(move |dx| (dy * dy + dx * dx, dy, dx)))
                        .filter(|&(_, dy, dx)| {
                            layout[((y + dy).rem_euclid(6) * 6 + (x + dx).rem_euclid(6)) as usize]
                                == c
                        })
                        .min()
                        .ok_or("CFA has no nearby color sample")?;
                    nearest[(y * 6 + x) as usize][c as usize] = (dy as i8, dx as i8);
                }
            }
        }
        Ok(Self { layout, nearest })
    }
    pub fn color(&self, x: usize, y: usize) -> usize {
        self.layout[y % 6 * 6 + x % 6] as usize
    }
    /// CFA phase at the first sample of a programmed sensor window.
    pub fn window(&self, left: usize, top: usize) -> Self {
        let (left, top) = (left % 6, top % 6);
        Self::new((0..36).map(|i| self.color(left + i % 6, top + i / 6) as u8).collect())
            .expect("windowing preserves a complete CFA tile")
    }
    #[inline]
    pub fn rgb(&self, raw: &RawView<'_>, x: usize, y: usize) -> [u16; 3] {
        self.sample_positions(raw, x, y)
            .map(|(x, y)| raw.sample(x, y))
    }
    #[inline]
    fn sample_positions(&self, raw: &RawView<'_>, x: usize, y: usize) -> [(usize, usize); 3] {
        let row = &self.nearest[y % 6 * 6..][..6];
        self.sample_positions_from_row(raw, row, x, y)
    }
    #[inline(always)]
    fn sample_positions_from_row(
        &self,
        raw: &RawView<'_>,
        nearest_row: &[[(i8, i8); 3]],
        x: usize,
        y: usize,
    ) -> [(usize, usize); 3] {
        nearest_row[x % 6].map(|(dy, dx)| {
            let wrap = |v: usize, d: i8, n: usize| {
                let v = v as isize + isize::from(d);
                if v < 0 {
                    (v + 6) as usize
                } else if v >= n as isize {
                    (v - 6) as usize
                } else {
                    v as usize
                }
            };
            (wrap(x, dx, raw.width), wrap(y, dy, raw.height))
        })
    }
    /// `spatial_gain` is Q11 at the original CFA sample coordinates. It is
    /// applied after the WB/exposure product and before 14-bit clipping.
    pub fn render(
        &self,
        raw: &RawView<'_>,
        film: &Film,
        black: u16,
        gains: [u16; 3],
        exposure: u16,
        spatial_gain: impl Fn(usize, usize) -> u16,
        crop: [usize; 4],
        width: usize,
        height: usize,
        out: &mut [u8],
    ) -> Result<(), String> {
        Self::validate_render(raw, gains, exposure, crop, width, height, out.len())?;
        self.render_rows(
            raw,
            film,
            black,
            gains,
            exposure,
            spatial_gain,
            crop,
            width,
            height,
            0..height,
            out,
        );
        Ok(())
    }
    pub fn validate_render(
        raw: &RawView<'_>,
        gains: [u16; 3],
        exposure: u16,
        crop: [usize; 4],
        width: usize,
        height: usize,
        output_bytes: usize,
    ) -> Result<(), String> {
        raw.validate()?;
        let [left, top, crop_width, crop_height] = crop;
        if raw.width < 6
            || raw.height < 6
            || width == 0
            || !width.is_multiple_of(2)
            || height == 0
            || output_bytes != width * height * 2
            || gains.contains(&0)
            || exposure == 0
            || crop_width == 0
            || crop_height == 0
            || left >= raw.width
            || top >= raw.height
            || crop_width > raw.width - left
            || crop_height > raw.height - top
        {
            return Err("nearest CFA color requires complete tiles and programmed gains".into());
        }
        Ok(())
    }
    pub fn render_rows(
        &self,
        raw: &RawView<'_>,
        film: &Film,
        black: u16,
        gains: [u16; 3],
        exposure: u16,
        spatial_gain: impl Fn(usize, usize) -> u16,
        crop: [usize; 4],
        width: usize,
        height: usize,
        rows: std::ops::Range<usize>,
        out: &mut [u8],
    ) {
        let [left, top, crop_width, crop_height] = crop;
        let interior = left >= 3
            && top >= 3
            && raw.width - (left + crop_width) >= 3
            && raw.height - (top + crop_height) >= 3;
        if interior {
            match raw.packing {
                Packing::U16 => self.render_rows_inner::<true, false>(
                    raw,
                    film,
                    black,
                    gains,
                    exposure,
                    &spatial_gain,
                    crop,
                    width,
                    height,
                    rows,
                    out,
                ),
                Packing::Packed12 => self.render_rows_inner::<true, true>(
                    raw,
                    film,
                    black,
                    gains,
                    exposure,
                    &spatial_gain,
                    crop,
                    width,
                    height,
                    rows,
                    out,
                ),
            }
        } else {
            match raw.packing {
                Packing::U16 => self.render_rows_inner::<false, false>(
                    raw,
                    film,
                    black,
                    gains,
                    exposure,
                    &spatial_gain,
                    crop,
                    width,
                    height,
                    rows,
                    out,
                ),
                Packing::Packed12 => self.render_rows_inner::<false, true>(
                    raw,
                    film,
                    black,
                    gains,
                    exposure,
                    &spatial_gain,
                    crop,
                    width,
                    height,
                    rows,
                    out,
                ),
            }
        }
    }
    fn render_rows_inner<const INTERIOR: bool, const PACKED12: bool>(
        &self,
        raw: &RawView<'_>,
        film: &Film,
        black: u16,
        gains: [u16; 3],
        exposure: u16,
        spatial_gain: &impl Fn(usize, usize) -> u16,
        crop: [usize; 4],
        width: usize,
        height: usize,
        rows: std::ops::Range<usize>,
        out: &mut [u8],
    ) {
        if INTERIOR {
            self.render_interior_rows::<PACKED12>(
                raw,
                film,
                black,
                gains,
                exposure,
                spatial_gain,
                crop,
                width,
                height,
                rows,
                out,
            );
        } else {
            self.render_edge_rows::<PACKED12>(
                raw,
                film,
                black,
                gains,
                exposure,
                spatial_gain,
                crop,
                width,
                height,
                rows,
                out,
            );
        }
    }
    fn render_interior_rows<const PACKED12: bool>(
        &self,
        raw: &RawView<'_>,
        film: &Film,
        black: u16,
        gains: [u16; 3],
        exposure: u16,
        spatial_gain: &impl Fn(usize, usize) -> u16,
        crop: [usize; 4],
        width: usize,
        height: usize,
        rows: std::ops::Range<usize>,
        out: &mut [u8],
    ) {
        let [left, top, crop_width, crop_height] = crop;
        let columns: Vec<_> = (0..width)
            .map(|x| {
                let raw_x = left + x * crop_width / width;
                let phase = raw_x % 6;
                let samples = std::array::from_fn(|row_phase| {
                    self.nearest[row_phase * 6 + phase].map(|(_, dx)| {
                        let x = raw_x.wrapping_add_signed(isize::from(dx));
                        CfaColumnSample {
                            x: x as u16,
                            byte_offset: RawView::byte_offset::<PACKED12>(x) as u16,
                        }
                    })
                });
                CfaColumn {
                    phase: phase as u8,
                    samples,
                }
            })
            .collect();
        let exposure_gains = gains.map(|gain| u64::from(gain) * u64::from(exposure));
        let mut pixels = FilmPixels::new(film);
        for (local_y, y) in rows.enumerate() {
            let yy = top + y * crop_height / height;
            let row_phase = yy % 6;
            let nearest_row = &self.nearest[row_phase * 6..][..6];
            let row_samples: [[CfaRowSample; 3]; 6] = std::array::from_fn(|x_phase| {
                nearest_row[x_phase].map(|(dy, _)| {
                    let y = yy.wrapping_add_signed(isize::from(dy));
                    CfaRowSample {
                        y,
                        byte_offset: y * raw.pitch,
                    }
                })
            });
            for x in (0..width).step_by(2) {
                let mut pixel = |x: usize| {
                    let column = &columns[x];
                    let samples = &column.samples[row_phase];
                    let row_samples = row_samples[usize::from(column.phase)];
                    let w = std::array::from_fn(|c| {
                        let sample = samples[c];
                        let row_sample = row_samples[c];
                        let sx = usize::from(sample.x);
                        let sy = row_sample.y;
                        let value = (u64::from(
                            raw.sample_from_offset::<PACKED12>(
                                row_sample.byte_offset + usize::from(sample.byte_offset),
                                sx,
                            )
                            .saturating_sub(black),
                        ) * exposure_gains[c])
                            >> 16;
                        ((value * u64::from(spatial_gain(sx, sy))) >> 11).min(16383) as u16
                    });
                    pixels.develop(w)
                };
                out[(local_y * width + x) * 2..(local_y * width + x) * 2 + 4]
                    .copy_from_slice(&Film::pack(pixel(x), pixel(x + 1)));
            }
        }
    }
    fn render_edge_rows<const PACKED12: bool>(
        &self,
        raw: &RawView<'_>,
        film: &Film,
        black: u16,
        gains: [u16; 3],
        exposure: u16,
        spatial_gain: &impl Fn(usize, usize) -> u16,
        crop: [usize; 4],
        width: usize,
        height: usize,
        rows: std::ops::Range<usize>,
        out: &mut [u8],
    ) {
        let [left, top, crop_width, crop_height] = crop;
        let xs: Vec<_> = (0..width).map(|x| left + x * crop_width / width).collect();
        let exposure_gains = gains.map(|gain| u64::from(gain) * u64::from(exposure));
        let mut pixels = FilmPixels::new(film);
        for (local_y, y) in rows.enumerate() {
            let yy = top + y * crop_height / height;
            let nearest_row = &self.nearest[yy % 6 * 6..][..6];
            for x in (0..width).step_by(2) {
                let mut pixel = |x: usize| {
                    let positions = self.sample_positions_from_row(raw, nearest_row, xs[x], yy);
                    let w = std::array::from_fn(|c| {
                        let (sx, sy) = positions[c];
                        let value =
                            (u64::from(raw.sample_as::<PACKED12>(sx, sy).saturating_sub(black))
                                * exposure_gains[c])
                                >> 16;
                        ((value * u64::from(spatial_gain(sx, sy))) >> 11).min(16383) as u16
                    });
                    pixels.develop(w)
                };
                out[(local_y * width + x) * 2..(local_y * width + x) * 2 + 4]
                    .copy_from_slice(&Film::pack(pixel(x), pixel(x + 1)));
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[test]
    fn interior_raster_matches_scalar_for_both_packings_crops_and_scaling() {
        let cfa = Cfa::new(vec![
            1, 2, 1, 1, 0, 1, 0, 1, 0, 2, 1, 2, 1, 2, 1, 1, 0, 1, 1, 0, 1, 1, 2, 1, 2, 1, 2, 0, 1,
            0, 1, 0, 1, 1, 2, 1,
        ])
        .unwrap();
        let words = fujimod_emulator_devices::test_host::film_words();
        let mut film = Film::default();
        film.update(|o| words[o / 4]).unwrap();
        for packing in [Packing::U16, Packing::Packed12] {
            let pitch = packing.row_bytes(48) + 8;
            let bytes: Vec<_> = (0..pitch * 30)
                .map(|i| ((i * 73 + i / 17) % 256) as u8)
                .collect();
            let raw = RawView {
                bytes: &bytes,
                width: 48,
                height: 30,
                pitch,
                packing,
            };
            let spatial_gain = |x, y| [1024u16, 2048, 3072][(x + 2 * y) % 3];
            for (crop, width, height) in [
                ([0, 0, 48, 30], 48, 30),
                ([6, 4, 36, 22], 50, 37),
                ([7, 5, 31, 19], 20, 11),
            ] {
                let actual_gain_calls = RefCell::new(Vec::new());
                let mut actual = vec![0; width * height * 2];
                cfa.render(
                    &raw,
                    &film,
                    127,
                    [311, 257, 493],
                    193,
                    |x, y| {
                        actual_gain_calls.borrow_mut().push((x, y));
                        spatial_gain(x, y)
                    },
                    crop,
                    width,
                    height,
                    &mut actual,
                )
                .unwrap();
                let expected_gain_calls = RefCell::new(Vec::new());
                let pixel = |x, y| {
                    let positions = cfa.sample_positions(
                        &raw,
                        crop[0] + x * crop[2] / width,
                        crop[1] + y * crop[3] / height,
                    );
                    let rgb = positions.map(|(x, y)| raw.sample(x, y));
                    film.develop(std::array::from_fn(|c| {
                        expected_gain_calls
                            .borrow_mut()
                            .push((positions[c].0, positions[c].1));
                        let value =
                            (u64::from(rgb[c].saturating_sub(127)) * [311u64, 257, 493][c] * 193)
                                >> 16;
                        ((value * u64::from(spatial_gain(positions[c].0, positions[c].1))) >> 11)
                            .min(16383) as u16
                    }))
                };
                let mut expected = Vec::new();
                for y in 0..height {
                    for x in (0..width).step_by(2) {
                        expected.extend_from_slice(&Film::pack(pixel(x, y), pixel(x + 1, y)));
                    }
                }
                assert_eq!(actual, expected);
                assert_eq!(
                    actual_gain_calls.borrow().as_slice(),
                    expected_gain_calls.borrow().as_slice()
                );
                assert_eq!(actual_gain_calls.borrow().len(), width * height * 3);
                let mut x_phases = [false; 6];
                let mut y_phases = [false; 6];
                for &(x, y) in actual_gain_calls.borrow().iter() {
                    x_phases[x % 6] = true;
                    y_phases[y % 6] = true;
                }
                assert!(x_phases.into_iter().all(|seen| seen));
                assert!(y_phases.into_iter().all(|seen| seen));
            }
        }
    }
    #[test]
    fn near_maximum_raw_x_matches_scalar_for_both_packings() {
        let cfa = Cfa::new((0..36).map(|i| [0, 1, 2, 2, 2, 2][i % 6]).collect()).unwrap();
        let words = fujimod_emulator_devices::test_host::film_words();
        let mut film = Film::default();
        film.update(|o| words[o / 4]).unwrap();

        for packing in [Packing::U16, Packing::Packed12] {
            let width = 8192;
            let height = 12;
            let pitch = packing.row_bytes(width) + 8;
            let mut bytes = vec![0; pitch * height];
            for y in 0..height {
                for x in 0..width {
                    let value = ((x * 17 + y * 97 + 23) % 4096) as u16;
                    if packing == Packing::U16 {
                        bytes[y * pitch + x * 2..y * pitch + x * 2 + 2]
                            .copy_from_slice(&value.to_le_bytes());
                    } else {
                        let i = y * pitch + x * 3 / 2;
                        if x % 2 == 0 {
                            bytes[i] = value as u8;
                            bytes[i + 1] = (value >> 8) as u8;
                        } else {
                            bytes[i] |= (value << 4) as u8;
                            bytes[i + 1] = (value >> 4) as u8;
                        }
                    }
                }
            }
            let raw = RawView {
                bytes: &bytes,
                width,
                height,
                pitch,
                packing,
            };
            raw.validate().unwrap();

            assert_eq!(RawView::byte_offset::<false>(width - 1), 16382);
            assert_eq!(RawView::byte_offset::<true>(width - 1), 12286);
            assert_eq!(
                u16::try_from(RawView::byte_offset::<false>(width - 1)),
                Ok(16382)
            );
            assert_eq!(
                u16::try_from(RawView::byte_offset::<true>(width - 1)),
                Ok(12286)
            );
            assert_eq!(
                raw.sample(width - 1, 5),
                (((width - 1) * 17 + 5 * 97 + 23) % 4096) as u16
            );

            let crop = [8171, 3, 18, 6];
            let (out_width, out_height) = (18, 8);
            let spatial_gain = |x, y| 768 + ((x * 7 + y * 11) % 5) as u16 * 256;
            let actual_gain_calls = RefCell::new(Vec::new());
            let mut actual = vec![0; out_width * out_height * 2];
            cfa.render(
                &raw,
                &film,
                127,
                [311, 257, 493],
                193,
                |x, y| {
                    actual_gain_calls.borrow_mut().push((x, y));
                    spatial_gain(x, y)
                },
                crop,
                out_width,
                out_height,
                &mut actual,
            )
            .unwrap();

            let expected_gain_calls = RefCell::new(Vec::new());
            let pixel = |x, y| {
                let positions = cfa.sample_positions(
                    &raw,
                    crop[0] + x * crop[2] / out_width,
                    crop[1] + y * crop[3] / out_height,
                );
                let rgb = positions.map(|(x, y)| raw.sample(x, y));
                film.develop(std::array::from_fn(|c| {
                    let (sx, sy) = positions[c];
                    expected_gain_calls.borrow_mut().push((sx, sy));
                    let value =
                        (u64::from(rgb[c].saturating_sub(127)) * [311u64, 257, 493][c] * 193) >> 16;
                    ((value * u64::from(spatial_gain(sx, sy))) >> 11).min(16383) as u16
                }))
            };
            let mut expected = Vec::new();
            for y in 0..out_height {
                for x in (0..out_width).step_by(2) {
                    expected.extend_from_slice(&Film::pack(pixel(x, y), pixel(x + 1, y)));
                }
            }
            assert_eq!(actual, expected);
            assert_eq!(
                actual_gain_calls.borrow().as_slice(),
                expected_gain_calls.borrow().as_slice()
            );
            assert_eq!(actual_gain_calls.borrow().len(), out_width * out_height * 3);
            assert!(
                actual_gain_calls
                    .borrow()
                    .iter()
                    .any(|&(x, _)| x == width - 2)
            );
        }
    }
    #[test]
    fn packed12_and_u16_row_padding() {
        let a = RawView {
            bytes: &[0x23, 0xc1, 0xab, 0x55, 0x55],
            width: 2,
            height: 1,
            pitch: 5,
            packing: Packing::Packed12,
        };
        a.validate().unwrap();
        assert_eq!([a.sample(0, 0), a.sample(1, 0)], [0x123, 0xabc]);
        let b = RawView {
            bytes: &[0x23, 1, 0xbc, 0xa, 0x55, 0x55],
            width: 2,
            height: 1,
            pitch: 6,
            packing: Packing::U16,
        };
        b.validate().unwrap();
        assert_eq!([b.sample(0, 0), b.sample(1, 0)], [0x123, 0xabc]);
    }
    #[test]
    fn cfa_rejects_missing_color() {
        assert!(Cfa::new(vec![1; 36]).is_err());
    }
    #[test]
    fn spatial_gain_precedes_reconstruction_and_clipping_for_both_packings() {
        let cfa = Cfa::new(vec![
            1, 2, 1, 1, 0, 1, 0, 1, 0, 2, 1, 2, 1, 2, 1, 1, 0, 1, 1, 0, 1, 1, 2, 1, 2, 1, 2, 0, 1,
            0, 1, 0, 1, 1, 2, 1,
        ])
        .unwrap();
        let words = fujimod_emulator_devices::test_host::film_words();
        let mut film = Film::default();
        film.update(|o| words[o / 4]).unwrap();
        let gains = [713, 409, 923];
        // Deliberately cross the 14-bit boundary before a gain below unity.
        let exposure = 2999;
        let gain = |x: usize, y: usize| [512u16, 2048, 4096][(x + 2 * y) % 3];
        for packing in [Packing::U16, Packing::Packed12] {
            let pitch = packing.row_bytes(24) + 8;
            let mut source = vec![0xa5; pitch * 18];
            let mut calibrated = vec![0xa5; (24 * 2 + 8) * 18];
            for y in 0..18 {
                for x in 0..24 {
                    let value = ((x * 59 + y * 113 + 127) % 4096) as u16;
                    if packing == Packing::U16 {
                        source[y * pitch + x * 2..y * pitch + x * 2 + 2]
                            .copy_from_slice(&value.to_le_bytes());
                    } else {
                        let i = y * pitch + x * 3 / 2;
                        if x % 2 == 0 {
                            source[i] = value as u8;
                            source[i + 1] = (value >> 8) as u8;
                        } else {
                            source[i] |= (value << 4) as u8;
                            source[i + 1] = (value >> 4) as u8;
                        }
                    }
                    let v = ((u64::from(value.saturating_sub(127))
                        * gains[cfa.color(x, y)]
                        * exposure)
                        >> 16)
                        * u64::from(gain(x, y))
                        >> 11;
                    let i = y * (24 * 2 + 8) + x * 2;
                    calibrated[i..i + 2].copy_from_slice(&(v.min(16383) as u16).to_le_bytes());
                }
            }
            let raw = RawView {
                bytes: &source,
                width: 24,
                height: 18,
                pitch,
                packing,
            };
            let expected = RawView {
                bytes: &calibrated,
                width: 24,
                height: 18,
                pitch: 56,
                packing: Packing::U16,
            };
            let mut a = vec![0; 20 * 14 * 2];
            let mut b = a.clone();
            cfa.render(
                &raw,
                &film,
                127,
                gains.map(|g| g as u16),
                exposure as u16,
                gain,
                [2, 2, 20, 14],
                20,
                14,
                &mut a,
            )
            .unwrap();
            cfa.render(
                &expected,
                &film,
                0,
                [256; 3],
                256,
                |_, _| 2048,
                [2, 2, 20, 14],
                20,
                14,
                &mut b,
            )
            .unwrap();
            assert_eq!(a, b);
        }
    }
    #[test]
    fn active_crop_excludes_raw_transport_border_in_every_output_row() {
        // Camera RAW/ISP analysis: raw-clone.md establishes the (24,16)
        // still crop. A smaller raster checks the same centered-crop law.
        let mut bytes = vec![0; 64 * 48 * 2];
        for y in 4..44 {
            for x in 4..60 {
                bytes[(y * 64 + x) * 2..(y * 64 + x) * 2 + 2]
                    .copy_from_slice(&4096u16.to_le_bytes());
            }
        }
        let raw = RawView {
            bytes: &bytes,
            width: 64,
            height: 48,
            pitch: 128,
            packing: Packing::U16,
        };
        let cfa = Cfa::new((0..36).map(|i| (i % 3) as u8).collect()).unwrap();
        let words = fujimod_emulator_devices::test_host::film_words();
        let mut film = Film::default();
        film.update(|o| words[o / 4]).unwrap();
        let mut out = vec![0; 40 * 36 * 2];
        cfa.render(
            &raw,
            &film,
            0,
            [256; 3],
            256,
            |_, _| 2048,
            [12, 6, 40, 36],
            40,
            36,
            &mut out,
        )
        .unwrap();
        assert!(out.chunks_exact(4).all(|p| p == [64, 64, 128, 128]));
        cfa.render(
            &raw,
            &film,
            0,
            [256; 3],
            256,
            |_, _| 2048,
            [0, 0, 64, 48],
            40,
            36,
            &mut out,
        )
        .unwrap();
        assert_eq!(&out[..4], &[0, 0, 128, 128]);
        assert!(
            cfa.render(
                &raw,
                &film,
                0,
                [256; 3],
                256,
                |_, _| 2048,
                [32, 0, 64, 48],
                40,
                36,
                &mut out
            )
            .is_err()
        );
    }
}
