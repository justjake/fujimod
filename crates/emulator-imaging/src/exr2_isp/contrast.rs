//! Declared coarse/fine horizontal-gradient statistics on programmed regions.
//! Layout uses 64-byte-row banks.
//! Squared differences and scale are unmeasured ISP arithmetic, not AF policy.
use crate::analysis::ImageLayout;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Arithmetic {
    SquaredHorizontalGradients { luma_bits: u8 },
}

impl Arithmetic {
    pub fn validate(&self) -> Result<(), String> {
        let Self::SquaredHorizontalGradients { luma_bits } = *self;
        if !(8..=12).contains(&luma_bits) {
            return Err("contrast luma depth must be 8 through 12 bits".into());
        }
        Ok(())
    }
}

pub fn energies(pixels: &[u8], l: ImageLayout, rect: [usize; 4]) -> Result<(u32, u32), String> {
    scaled_energies(pixels, l, rect, 8)
}
fn scaled_energies(
    pixels: &[u8],
    l: ImageLayout,
    rect: [usize; 4],
    bits: u8,
) -> Result<(u32, u32), String> {
    if !(8..=12).contains(&bits) {
        return Err("contrast luma depth must be 8 through 12 bits".into());
    }
    let [x, y, w, h] = rect;
    if w == 0 || h == 0 || x + w > l.width || y + h > l.height || pixels.len() != l.bytes()? {
        return Err("contrast window escapes its connected raster".into());
    }
    let mut fine = 0u64;
    let mut coarse = 0u64;
    for row in y..y + h {
        let sample = |col: usize| i32::from(pixels[row * l.stride + (col / 2) * 4 + col % 2]);
        let mut f = 0u64;
        let mut c = 0u64;
        for col in x..x + w {
            if col + 1 < x + w {
                let d = (sample(col + 1) - sample(col)) << (bits - 8);
                f += (d * d) as u64;
            }
            if col + 3 < x + w {
                let d = (sample(col + 3) - sample(col)) << (bits - 8);
                c += (d * d) as u64;
            }
        }
        fine += f / 256;
        coarse += c / 2304;
    }
    Ok((
        u32::try_from(coarse).map_err(|_| "coarse contrast overflows")?,
        u32::try_from(fine).map_err(|_| "fine contrast overflows")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declared_luma_depth_preserves_weak_cell_energy_before_normalization() {
        let l = ImageLayout {
            width: 8,
            height: 4,
            stride: 16,
        };
        let pixels = (0..4)
            .flat_map(|_| (0..4).flat_map(|i| [i * 2, i * 2 + 1, 128, 128]))
            .collect::<Vec<u8>>();
        assert_eq!(
            scaled_energies(&pixels, l, [0, 0, 8, 4], 8).unwrap(),
            (0, 0)
        );
        assert_eq!(
            scaled_energies(&pixels, l, [0, 0, 8, 4], 12).unwrap(),
            (20, 28)
        );
        assert!(scaled_energies(&pixels, l, [0, 0, 8, 4], 13).is_err());
    }
    #[test]
    fn programmed_grid_and_band_sample_only_their_pixel_windows() {
        let l = ImageLayout {
            width: 12,
            height: 8,
            stride: 24,
        };
        let pixels = (0..8)
            .flat_map(|_| {
                (0..6).flat_map(|i| {
                    [
                        if i < 2 {
                            0
                        } else if i == 2 {
                            0
                        } else {
                            255
                        },
                        if i < 2 { 0 } else { 255 },
                        128,
                        128,
                    ]
                })
            })
            .collect::<Vec<u8>>();
        let mut r = [0; 1024];
        r[0x100 / 4] = 0x00021003;
        r[0x104 / 4] = 0x00011001;
        r[0x10c / 4] = 2 << 16 | 7;
        r[0x110 / 4] = 1 << 16 | 1;
        let (grid, bands) = banks(
            &r,
            &pixels,
            l,
            &Arithmetic::SquaredHorizontalGradients { luma_bits: 8 },
        )
        .unwrap();
        let word = |o: usize| u32::from_le_bytes(grid[o..o + 4].try_into().unwrap());
        assert_eq!(word(0), 56);
        assert_eq!(word(0x400), 508);
        assert_eq!(word(4), 0);
        assert_eq!(u64::from_le_bytes(bands[..8].try_into().unwrap()), 508);
        r[0x100 / 4] = 0x1fff1003;
        assert!(
            banks(
                &r,
                &pixels,
                l,
                &Arithmetic::SquaredHorizontalGradients { luma_bits: 8 }
            )
            .is_err()
        );
    }
}
pub fn banks(
    r: &[u32],
    pixels: &[u8],
    l: ImageLayout,
    arithmetic: &Arithmetic,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let Arithmetic::SquaredHorizontalGradients { luma_bits } = *arithmetic;
    let gx = r[0x100 / 4];
    let gy = r[0x104 / 4];
    let cols = ((gx >> 12) & 15) as usize + 1;
    let rows = ((gy >> 12) & 15) as usize + 1;
    if cols > 8 || rows > 8 {
        return Err("contrast grid exceeds eight rows or columns".into());
    }
    let width = (gx & 511) as usize + 1;
    let height = (gy & 511) as usize + 1;
    let x = (gx >> 16 & 8191) as usize;
    let y = (gy >> 16 & 8191) as usize;
    let mut grid = vec![0; 2048];
    for cy in 0..rows {
        for cx in 0..cols {
            let (coarse, fine) = scaled_energies(
                pixels,
                l,
                [x + cx * width, y + cy * height, width, height],
                luma_bits,
            )?;
            let o = cy * 64 + cx * 4;
            grid[o..o + 4].copy_from_slice(&coarse.to_le_bytes());
            grid[0x400 + o..0x404 + o].copy_from_slice(&fine.to_le_bytes());
        }
    }
    let mut bands = vec![0; 80];
    for i in 0..10 {
        let a = r[(0x10c + i * 8) / 4];
        let b = r[(0x110 + i * 8) / 4];
        let (_, fine) = scaled_energies(
            pixels,
            l,
            [
                (a >> 16 & 8191) as usize,
                (b >> 16 & 8191) as usize,
                (a & 8191) as usize + 1,
                (b & 8191) as usize + 1,
            ],
            luma_bits,
        )?;
        bands[i * 8..i * 8 + 8].copy_from_slice(&u64::from(fine).to_le_bytes());
    }
    Ok((grid, bands))
}
