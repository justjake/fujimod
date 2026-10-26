//! X70 RGB cell means in the three 16x16 FFF18200 output planes. Register
//! fields select cell regions; arithmetic and the 12-bit mean scale are
//! explicit unmeasured laws.
//! Comparators admit only arriving pixels, as in the adjacent histogram bank;
//! windows crossing a frame boundary are neither resized nor black-padded.
use fujimod_emulator_imaging::analysis::ImageLayout;

pub fn banks(r: &[u32], pixels: &[u8], l: ImageLayout) -> Result<Vec<u8>, String> {
    if pixels.len() != l.bytes()? {
        return Err("metering input raster extent differs".into());
    }
    let gx = r[0x200 / 4];
    let gy = r[0x204 / 4];
    let gap = r[0x258 / 4];
    let cols = ((gx >> 12) & 15) as usize + 1;
    let rows = ((gy >> 12) & 15) as usize + 1;
    let width = (gx & 511) as usize + 1;
    let height = (gy & 511) as usize + 1;
    let left = (gx >> 16 & 8191) as usize;
    let top = (gy >> 16 & 8191).saturating_sub(1) as usize;
    let dx = (gap >> 16 & 511) as usize;
    let dy = (gap & 511) as usize;
    let mut out = vec![0; 3072];
    for cy in 0..rows {
        for cx in 0..cols {
            let x = left + cx * (width + dx);
            let y = top + cy * (height + dy);
            let right = (x + width).min(l.width);
            let bottom = (y + height).min(l.height);
            let samples = right.saturating_sub(x) * bottom.saturating_sub(y);
            if samples == 0 {
                continue;
            }
            let mut sums = [0u64; 3];
            for yy in y..bottom {
                for xx in x..right {
                    let p = yy * l.stride + (xx / 2) * 4;
                    let lum = i32::from(pixels[p + xx % 2]);
                    let cb = i32::from(pixels[p + 2]) - 128;
                    let cr = i32::from(pixels[p + 3]) - 128;
                    let rgb = [
                        (256 * lum + 359 * cr + 128) >> 8,
                        (256 * lum - 88 * cb - 183 * cr + 128) >> 8,
                        (256 * lum + 454 * cb + 128) >> 8,
                    ];
                    for (sum, value) in sums.iter_mut().zip(rgb) {
                        *sum += value.clamp(0, 255) as u64;
                    }
                }
            }
            for (plane, sum) in sums.into_iter().enumerate() {
                let offset = plane * 1024 + cy * 64 + cx * 4;
                let mean = (sum * 16 / samples as u64) as u32;
                out[offset..offset + 4].copy_from_slice(&mean.to_le_bytes());
            }
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cell_means_use_region_geometry_and_all_three_physical_color_planes() {
        let l = ImageLayout {
            width: 8,
            height: 4,
            stride: 16,
        };
        let pixels = (0..4)
            .flat_map(|_| {
                (0..4).flat_map(|x| {
                    [
                        if x < 2 { 100 } else { 200 },
                        if x < 2 { 100 } else { 200 },
                        128,
                        128,
                    ]
                })
            })
            .collect::<Vec<u8>>();
        let mut r = [0; 1024];
        r[0x200 / 4] = 0x1003;
        r[0x204 / 4] = 0x10003;
        let out = banks(&r, &pixels, l).unwrap();
        for plane in 0..3 {
            let a = plane * 1024;
            assert_eq!(u32::from_le_bytes(out[a..a + 4].try_into().unwrap()), 1600);
            assert_eq!(
                u32::from_le_bytes(out[a + 4..a + 8].try_into().unwrap()),
                3200
            );
            assert_eq!(&out[a + 8..a + 12], &[0; 4]);
        }
        r[0x200 / 4] = 0x1fff1003;
        assert_eq!(banks(&r, &pixels, l).unwrap(), vec![0; 3072]);
        assert!(banks(&r, &pixels[..pixels.len() - 1], l).is_err());
    }
}

#[cfg(test)]
mod frame_boundary_tests {
    use super::*;

    #[test]
    fn partial_cells_average_only_arriving_pixels_without_extending_edges() {
        let layout = ImageLayout {
            width: 8,
            height: 4,
            stride: 16,
        };
        let pixels: Vec<u8> = (0..4)
            .flat_map(|_| {
                [
                    20, 20, 128, 128, 40, 40, 128, 128, 80, 80, 128, 128, 200, 200, 128, 128,
                ]
            })
            .collect();
        let mut r = [0; 1024];
        // Two 4x3 cells, starting at (6,2): only a 2x2 part of the first arrives.
        r[0x200 / 4] = 6 << 16 | 1 << 12 | 3;
        r[0x204 / 4] = 3 << 16 | 2;
        let out = banks(&r, &pixels, layout).unwrap();
        for plane in 0..3 {
            let base = plane * 1024;
            assert_eq!(
                u32::from_le_bytes(out[base..base + 4].try_into().unwrap()),
                3200
            );
            assert_eq!(&out[base + 4..base + 8], &[0; 4]);
        }
    }

    #[test]
    fn large_grid_clips_cells_after_final_input_row() {
        // The 16x16 grid remains configured when the input raster is 2496x853.
        let layout = ImageLayout {
            width: 2496,
            height: 853,
            stride: 4992,
        };
        let pixels = [100, 100, 128, 128].repeat(layout.width * layout.height / 2);
        let mut r = [0; 1024];
        let columns = 16;
        let rows = 16;
        let left = 12;
        let top = 56;
        let width = 153;
        let height = 99;
        r[0x200 / 4] = left << 16 | (columns - 1) << 12 | (width - 1);
        r[0x204 / 4] = top << 16 | (rows - 1) << 12 | (height - 1);
        r[0x258 / 4] = 1;
        let out = banks(&r, &pixels, layout).unwrap();
        for plane in 0..3 {
            for row in 0..16 {
                for column in 0..16 {
                    let at = plane * 1024 + row * 64 + column * 4;
                    assert_eq!(
                        u32::from_le_bytes(out[at..at + 4].try_into().unwrap()),
                        if row < 8 { 1600 } else { 0 }
                    );
                }
            }
        }
    }
}
