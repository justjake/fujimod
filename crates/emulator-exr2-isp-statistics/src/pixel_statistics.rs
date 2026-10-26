//! Register-selected EXR II color windows and accumulation planes.
//! CFA sample densities come from the selected physical sensor calibration.
use fujimod_emulator_image_formation::exposure::ExposedFrame;

/// A bounded pure memo of pixel measurements. DMA addresses, frame edges and
/// completion clocks are handled by the producer even when these bytes repeat.
#[derive(Default)]
pub(super) struct Cache {
    previous: Option<(
        ExposedFrame,
        [usize; 2],
        [[u32; 23]; 2],
        Vec<fujimod_emulator_imaging::scene::StatisticsSpan>,
    )>,
}
impl Cache {
    /// The last computed measurement, which may still be pending or canceled.
    /// This derived cache observation does not assert DMA publication.
    pub fn source(&self) -> Option<serde_json::Value> {
        self.previous.as_ref().map(|(frame, grid, words, spans)| {
            let maxima: Vec<_> = spans
                .iter()
                .filter(|s| s.destination == 2)
                .map(|s| {
                    s.bytes
                        .chunks_exact(4)
                        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
                        .max()
                        .unwrap_or(0)
                })
                .collect();
            serde_json::json!({"exposure":frame.physical_state(),"grid":grid,
                "contrast_geometry":words[0],"color_geometry":words[1],"color_sum_maxima":maxima})
        })
    }
    pub fn clear(&mut self) {
        self.previous = None;
    }
    pub fn measure(
        &mut self,
        frame: &ExposedFrame,
        grid: [usize; 2],
        registers: &[u32],
    ) -> Result<&[fujimod_emulator_imaging::scene::StatisticsSpan], String> {
        let words = [0x100, 0x200].map(|base| std::array::from_fn(|i| registers[base / 4 + i]));
        let same = self
            .previous
            .as_ref()
            .is_some_and(|(old, dimensions, old_words, _)| {
                old.same_samples(frame)
                    && old.color_sample_weights == frame.color_sample_weights
                    && *dimensions == grid
                    && *old_words == words
            });
        if !same {
            let mut spans = color_spans(frame, grid, registers)?;
            spans.extend(contrast_spans(frame, grid, registers)?);
            self.previous = Some((frame.clone(), grid, words, spans));
        }
        Ok(&self.previous.as_ref().unwrap().3)
    }
}

// Pixel-center sampling repeats source pixels on a larger sensor grid. Visit
// each constant run once; the multiplicity remains part of the integer sum.
fn run_end(at: usize, end: usize, grid: usize, source: usize) -> usize {
    let sample = ((2 * at + 1) * source / (2 * grid)).min(source - 1);
    (2 * grid * (sample + 1))
        .saturating_sub(source)
        .div_ceil(2 * source)
        .max(at + 1)
        .min(end)
}
fn runs(
    begin: usize,
    end: usize,
    grid: usize,
    source: usize,
) -> impl Iterator<Item = (usize, u64)> {
    let mut at = begin;
    std::iter::from_fn(move || {
        if at >= end {
            return None;
        }
        let begin = at;
        at = run_end(at, end, grid, source);
        Some((begin, (at - begin) as u64))
    })
}
fn rectangle(x: u32, y: u32) -> [usize; 4] {
    [
        (x >> 16 & 0x3fff) as usize,
        (y >> 16 & 0x1fff).saturating_sub(1) as usize,
        (x & 0x1fff) as usize + 1,
        (y & 0x1fff) as usize + 1,
    ]
}
/// All raw statistics footprints, clipped to the programmed sensor grid.
/// Difference pairs lie inside their declared contrast rectangle.
pub(super) fn windows(grid: [usize; 2], r: &[u32]) -> Vec<[usize; 4]> {
    let mut windows = Vec::with_capacity(532);
    for base in [0x100, 0x200] {
        let (x, y, gap) = (
            r[base / 4],
            r[(base + 4) / 4],
            r[if base == 0x100 { 0x108 / 4 } else { 0x258 / 4 }],
        );
        let (w, h) = ((x & 511) as usize + 1, (y & 511) as usize + 1);
        let (left, top) = if base == 0x100 {
            ((x >> 16 & 8191) as usize, (y >> 16 & 8191) as usize)
        } else {
            (
                (x >> 16 & 0x3fff) as usize,
                (y >> 16 & 8191).saturating_sub(1) as usize,
            )
        };
        let (gx, gy) = ((gap >> 16 & 511) as usize, (gap & 511) as usize);
        for row in 0..(y >> 12 & 15) as usize + 1 {
            for col in 0..(x >> 12 & 15) as usize + 1 {
                windows.push([left + col * (w + gx), top + row * (h + gy), w, h]);
            }
        }
    }
    for i in 0..10 {
        windows.push(rectangle(r[(0x208 + i * 8) / 4], r[(0x20c + i * 8) / 4]));
        let (x, y) = (r[(0x10c + i * 8) / 4], r[(0x110 + i * 8) / 4]);
        windows.push([
            (x >> 16 & 8191) as usize,
            (y >> 16 & 8191) as usize,
            (x & 8191) as usize + 1,
            (y & 8191) as usize + 1,
        ]);
    }
    for w in &mut windows {
        let right = (w[0] + w[2]).min(grid[0]);
        let bottom = (w[1] + w[3]).min(grid[1]);
        w[0] = w[0].min(grid[0]);
        w[1] = w[1].min(grid[1]);
        w[2] = right - w[0];
        w[3] = bottom - w[1];
    }
    windows.retain(|w| w[2] != 0 && w[3] != 0);
    windows.sort_unstable();
    windows.dedup();
    windows
}
pub fn color_spans(
    frame: &ExposedFrame,
    grid: [usize; 2],
    r: &[u32],
) -> Result<Vec<fujimod_emulator_imaging::scene::StatisticsSpan>, String> {
    use fujimod_emulator_imaging::scene::StatisticsSpan;
    let sum = |[x, y, w, h]: [usize; 4]| -> Result<[u64; 3], String> {
        if w * h > 32_000_000 {
            return Err("color statistics window exceeds sensor extent".into());
        }
        let mut total = [0u64; 3];
        let [left, top] = [x.min(grid[0]), y.min(grid[1])];
        let [right, bottom] = [(x + w).min(grid[0]), (y + h).min(grid[1])];
        let [width, height] = frame.sample_dimensions();
        if let Some(offsets) =
            frame.sample_offsets(grid, [left, top, right - left, bottom - top], 1)
        {
            let columns: Vec<_> = runs(left, right, grid[0], width)
                .map(|(xx, n)| (offsets.columns[xx - left], n))
                .collect();
            for (yy, rows) in runs(top, bottom, grid[1], height) {
                let row = offsets.rows[yy - top];
                let mut line = [0u64; 3];
                for &(column, n) in &columns {
                    let v = offsets.sample(row, column);
                    for c in 0..3 {
                        line[c] += u64::from(v[c]) * n;
                    }
                }
                for c in 0..3 {
                    total[c] += line[c] * rows;
                }
            }
        } else {
            for (yy, rows) in runs(top, bottom, grid[1], height) {
                for (xx, columns) in runs(left, right, grid[0], width) {
                    let v = frame.sample(xx, yy, grid);
                    for c in 0..3 {
                        total[c] += u64::from(v[c]) * columns * rows;
                    }
                }
            }
        }
        let weights = frame.color_sample_weights.map(u64::from);
        let density: u64 = weights.iter().sum();
        Ok(std::array::from_fn(|c| total[c] * weights[c] / density))
    };
    let (x, y, gaps) = (r[0x200 / 4], r[0x204 / 4], r[0x258 / 4]);
    let mut planes = vec![vec![0u8; 1024]; 3];
    let (left, top) = (
        (x >> 16 & 0x3fff) as usize,
        (y >> 16 & 0x1fff).saturating_sub(1) as usize,
    );
    let (w, h) = ((x & 511) as usize + 1, (y & 511) as usize + 1);
    for row in 0..(y >> 12 & 15) as usize + 1 {
        for col in 0..(x >> 12 & 15) as usize + 1 {
            let v = sum([
                left + col * (w + (gaps >> 16 & 511) as usize),
                top + row * (h + (gaps as usize & 511)),
                w,
                h,
            ])?;
            let at = (row * 16 + col) * 4;
            for c in 0..3 {
                planes[c][at..at + 4]
                    .copy_from_slice(&(v[c].min(u32::MAX as u64) as u32).to_le_bytes());
            }
        }
    }
    let mut out: Vec<_> = planes
        .into_iter()
        .enumerate()
        .map(|(c, bytes)| StatisticsSpan {
            destination: 2,
            offset: (c * 1024) as u32,
            bytes,
        })
        .collect();
    for i in 0..10 {
        let v = sum(rectangle(r[(0x208 + i * 8) / 4], r[(0x20c + i * 8) / 4]))?;
        out.push(StatisticsSpan {
            destination: 3,
            offset: (i * 32) as u32,
            bytes: v.into_iter().flat_map(u64::to_le_bytes).collect(),
        });
    }
    out.push(StatisticsSpan {
        destination: 6,
        offset: 0,
        bytes: vec![0; 16],
    });
    Ok(out)
}

/// Declared raw-green horizontal difference energies. Grid/window geometry is
/// register driven; the spatial filter response is an unmeasured approximation.
/// No focus decision or lens target is supplied by this accumulation block.
pub fn contrast_spans(
    frame: &ExposedFrame,
    grid: [usize; 2],
    r: &[u32],
) -> Result<Vec<fujimod_emulator_imaging::scene::StatisticsSpan>, String> {
    use fujimod_emulator_imaging::scene::StatisticsSpan;
    let energy = |x: usize, y: usize, w: usize, h: usize| -> Result<[u64; 2], String> {
        if w.checked_mul(h).is_none_or(|n| n > 32_000_000) {
            return Err("contrast window exceeds sensor extent".into());
        }
        let mut total = [0u64; 2];
        let [left, top] = [x.min(grid[0]), y.min(grid[1])];
        let [end, bottom] = [(x + w).min(grid[0]), (y + h).min(grid[1])];
        let [width, height] = frame.sample_dimensions();
        // Each distance pairs runs of equal samples; the pairs do not depend on
        // the row, so they are found once per window.
        let pairs = [3, 1].map(|distance| {
            let pair_end = end.saturating_sub(distance);
            let mut pairs = Vec::new();
            let mut xx = left;
            while xx < pair_end {
                let next = run_end(xx, pair_end, grid[0], width)
                    .min(run_end(xx + distance, end, grid[0], width) - distance);
                pairs.push((xx, distance, (next - xx) as u64));
                xx = next;
            }
            pairs
        });
        let offsets = frame.sample_offsets(grid, [left, top, end - left, bottom - top], 1);
        let offset_pairs = offsets.as_ref().map(|offsets| {
            pairs.each_ref().map(|pairs| {
                pairs
                    .iter()
                    .map(|&(xx, distance, n)| {
                        (
                            offsets.columns[xx - left],
                            offsets.columns[xx + distance - left],
                            n,
                        )
                    })
                    .collect::<Vec<_>>()
            })
        });
        for (yy, rows) in runs(top, bottom, grid[1], height) {
            let mut row = [0u64; 2];
            for i in 0..2 {
                if let (Some(offsets), Some(offset_pairs)) = (&offsets, &offset_pairs) {
                    let line = offsets.rows[yy - top];
                    for &(a, b, n) in &offset_pairs[i] {
                        let d = i64::from(offsets.channel(line, b, 1))
                            - i64::from(offsets.channel(line, a, 1));
                        row[i] += (d * d) as u64 * n;
                    }
                } else {
                    for &(xx, distance, n) in &pairs[i] {
                        let a = i64::from(frame.sample(xx, yy, grid)[1]);
                        let b = i64::from(frame.sample(xx + distance, yy, grid)[1]);
                        let d = b - a;
                        row[i] += (d * d) as u64 * n;
                    }
                }
            }
            // Hardware-law rounding is per grid row, before combining repeats.
            total[0] += (row[0] / 2304) * rows;
            total[1] += (row[1] / 256) * rows;
        }
        Ok(total)
    };
    let (x, y, gap) = (r[0x100 / 4], r[0x104 / 4], r[0x108 / 4]);
    let (w, h) = ((x & 511) as usize + 1, (y & 511) as usize + 1);
    let (left, top) = ((x >> 16 & 8191) as usize, (y >> 16 & 8191) as usize);
    let mut planes = [vec![0u8; 1024], vec![0u8; 1024]];
    for row in 0..(y >> 12 & 15) as usize + 1 {
        for col in 0..(x >> 12 & 15) as usize + 1 {
            let values = energy(
                left + col * (w + (gap >> 16 & 511) as usize),
                top + row * (h + (gap & 511) as usize),
                w,
                h,
            )?;
            for c in 0..2 {
                let at = (row * 16 + col) * 4;
                planes[c][at..at + 4]
                    .copy_from_slice(&(values[c].min(u32::MAX as u64) as u32).to_le_bytes());
            }
        }
    }
    let mut out = planes
        .into_iter()
        .enumerate()
        .map(|(i, bytes)| StatisticsSpan {
            destination: 0,
            offset: (i * 1024) as u32,
            bytes,
        })
        .collect::<Vec<_>>();
    let mut bands = vec![0u8; 80];
    for i in 0..10 {
        let (x, y) = (r[(0x10c + i * 8) / 4], r[(0x110 + i * 8) / 4]);
        bands[i * 8..i * 8 + 8].copy_from_slice(
            &energy(
                (x >> 16 & 8191) as usize,
                (y >> 16 & 8191) as usize,
                (x & 8191) as usize + 1,
                (y & 8191) as usize + 1,
            )?[1]
                .to_le_bytes(),
        );
    }
    out.push(StatisticsSpan {
        destination: 1,
        offset: 0,
        bytes: bands,
    });
    Ok(out)
}

#[cfg(test)]
#[path = "pixel_statistics_tests.rs"]
mod exact_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_imaging::exposure::Calibration;
    use fujimod_emulator_imaging::exposure::Exposure;
    use fujimod_emulator_imaging::scene::{Frame, FramePixels, SensorFrames};
    use std::sync::{Arc, Mutex};
    #[test]
    fn color_grid_keeps_horizontal_and_vertical_gaps_on_their_register_axes() {
        let pixels = (0..12)
            .flat_map(|y| (0..12).flat_map(move |x| [20 + x + y * 12; 3]))
            .collect();
        let frame = ExposedFrame::from_frame(
            Arc::new(Frame::new(12, 12, FramePixels::Rgb24(pixels)).unwrap()),
            Exposure {
                seconds: 0.0001,
                analog_gain: 1.,
            },
            8.,
            Calibration {
                full_scale: 16380,
                reference_white: [256; 3],
                saturation_lux_seconds: 1.,
                color_sample_weights: [2, 5, 2],
            },
        )
        .unwrap();
        let mut r = vec![0; 0x1000 / 4];
        // Two 2x2 cells on each axis; upper gap field is horizontal.
        r[0x200 / 4] = 1 << 16 | 1 << 12 | 1;
        r[0x204 / 4] = 3 << 16 | 1 << 12 | 1;
        r[0x258 / 4] = 3 << 16 | 1;
        let spans = color_spans(&frame, [12, 12], &r).unwrap();
        let footprints = windows([12, 12], &r);
        for (row, y) in [2, 5].into_iter().enumerate() {
            for (col, x) in [1, 6].into_iter().enumerate() {
                assert!(footprints.contains(&[x, y, 2, 2]));
                let sum: u64 = (y..y + 2)
                    .flat_map(|yy| (x..x + 2).map(move |xx| (xx, yy)))
                    .map(|(xx, yy)| u64::from(frame.sample(xx, yy, [12, 12])[0]))
                    .sum();
                for (channel, weight) in [2, 5, 2].into_iter().enumerate() {
                    let at = (row * 16 + col) * 4;
                    assert_eq!(
                        u32::from_le_bytes(spans[channel].bytes[at..at + 4].try_into().unwrap()),
                        (sum * weight / 9) as u32
                    );
                }
            }
        }
        assert!(!footprints.contains(&[4, 2, 2, 2]));
        assert!(!footprints.contains(&[1, 7, 2, 2]));
        assert_eq!(&spans[1].bytes[8..64], &[0; 56]);
    }
    #[test]
    fn contrast_energies_follow_pixels_and_the_square_of_exposure() {
        let mut r = vec![0; 0x1000 / 4];
        r[0x100 / 4] = 63;
        r[0x104 / 4] = 7;
        let calibration = Calibration {
            full_scale: 16380,
            reference_white: [256; 3],
            saturation_lux_seconds: 1.,
            color_sample_weights: [2, 5, 2],
        };
        let measure = |pixels: Vec<u8>, seconds: f64| {
            let frame = ExposedFrame::from_frame(
                Arc::new(Frame::new(64, 8, FramePixels::Rgb24(pixels.into())).unwrap()),
                Exposure {
                    seconds,
                    analog_gain: 1.,
                },
                8.,
                calibration,
            )
            .unwrap();
            let spans = contrast_spans(&frame, [64, 8], &r).unwrap();
            u32::from_le_bytes(spans[1].bytes[..4].try_into().unwrap())
        };
        let pixels = (0..64 * 8)
            .flat_map(|x| [if x % 8 < 4 { 30 } else { 180 }; 3])
            .collect::<Vec<_>>();
        let first = measure(pixels.clone(), 0.0001);
        let second = measure(pixels, 0.0002);
        assert!(first > 0);
        assert!((i64::from(second) - 4 * i64::from(first)).abs() <= 64);
        assert_eq!(measure(vec![90; 64 * 8 * 3], 0.0001), 0);
    }
    #[test]
    fn metering_integrates_exposed_raw_with_programmed_extent_and_cfa_density() {
        let frames = Arc::new(Mutex::new(SensorFrames::new(
            Frame::new(1, 1, FramePixels::Rgb24(vec![255; 3].into())).unwrap(),
        )));
        let exposure = Arc::new(Mutex::new(Exposure {
            seconds: 0.0001,
            analog_gain: 1.,
        }));
        let aperture = Arc::new(Mutex::new(8.));
        let calibration = Calibration {
            full_scale: 16380,
            reference_white: [256; 3],
            saturation_lux_seconds: 1.,
            color_sample_weights: [2, 5, 2],
        };
        let frame = ExposedFrame::capture(&frames, &exposure, &aperture, calibration)
            .unwrap()
            .unwrap();
        let v = u64::from(frame.sample(0, 0, [9, 9])[0]);
        let mut r = vec![0; 0x1000 / 4];
        r[0x200 / 4] = 2;
        r[0x204 / 4] = (1 << 16) | 2;
        r[0x208 / 4] = (6 << 16) | 5;
        r[0x20c / 4] = (7 << 16) | 5;
        let spans = color_spans(&frame, [9, 9], &r).unwrap();
        for (c, n) in [2, 5, 2].into_iter().enumerate() {
            assert_eq!(
                u32::from_le_bytes(spans[c].bytes[..4].try_into().unwrap()),
                (v * n) as u32
            );
        }
        let region = &spans[3].bytes;
        assert_eq!(u64::from_le_bytes(region[8..16].try_into().unwrap()), v * 5); // clipped 3×3
        exposure.lock().unwrap().seconds *= 2.;
        let next = ExposedFrame::capture(&frames, &exposure, &aperture, calibration)
            .unwrap()
            .unwrap();
        let doubled = color_spans(&next, [9, 9], &r).unwrap();
        let old = u32::from_le_bytes(spans[1].bytes[..4].try_into().unwrap());
        let new = u32::from_le_bytes(doubled[1].bytes[..4].try_into().unwrap());
        assert!((new as i64 - 2 * old as i64).abs() <= 5);
        assert_eq!(
            color_spans(&frame, [9, 9], &r).unwrap()[1].bytes,
            spans[1].bytes
        );
    }
}
