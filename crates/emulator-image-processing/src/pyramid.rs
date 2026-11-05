//! Phase-zero half-size luma reduction with replicated original-source edges.
//! Seven-tap passes retain ten-bit horizontal precision. The optional five-tap
//! smoothing precedes reduction and extends the original image, rather than
//! replicating a filtered intermediate at its boundary.

#[derive(Clone, Copy)]
pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub packed: bool,
}
const SEVEN: [u32; 7] = [10, 32, 54, 64, 54, 32, 10];
const FIVE: [u32; 5] = [4, 32, 56, 32, 4];

pub fn half_luma(
    source: &[u8],
    raster: Raster,
    smooth: bool,
    out: &mut [u8],
) -> Result<(), String> {
    let Raster {
        width,
        height,
        pitch,
        packed,
    } = raster;
    if !(2..=8190).contains(&width)
        || !(2..=8190).contains(&height)
        || width % 2 != 0
        || height % 2 != 0
        || pitch < width * if packed { 2 } else { 1 }
        || pitch > 32752
        || pitch * height > 32 << 20
        || source.len() != pitch * height
        || out.len() != width / 2 * (height / 2)
    {
        return Err("invalid bounded half-luma raster".into());
    }
    let sample = |x: isize, y: isize| -> u32 {
        let x = x.clamp(0, width as isize - 1) as usize;
        let y = y.clamp(0, height as isize - 1) as usize;
        source[y * pitch + if packed { x / 2 * 4 + x % 2 } else { x }] as u32
    };
    let dw = width / 2;
    let extent = width + 6;
    let mut horizontal = vec![0u16; 5 * extent];
    let mut tags = [usize::MAX; 5];
    let mut vertical = vec![0u16; extent];
    let mut reduced = vec![0u16; 7 * dw];
    // Seven reduced rows and five smoothing rows bound scratch memory by
    // width, even when the source uses full-resolution shared-pitch storage.
    for y in -3isize..=(height as isize - 2 + 3) {
        let slot = (y + 3) as usize % 7;
        if smooth {
            for dy in -2isize..=2 {
                let sy = (y + dy).clamp(0, height as isize - 1) as usize;
                let bank = sy % 5;
                if tags[bank] != sy {
                    for (column, value) in horizontal[bank * extent..(bank + 1) * extent]
                        .iter_mut()
                        .enumerate()
                    {
                        let x = column as isize - 3;
                        let sum: u32 = FIVE
                            .iter()
                            .enumerate()
                            .map(|(i, &w)| w * sample(x + i as isize - 2, sy as isize))
                            .sum();
                        *value = ((sum + 16) >> 5) as u16;
                    }
                    tags[bank] = sy;
                }
            }
            for (x, value) in vertical.iter_mut().enumerate() {
                let sum: u32 = FIVE
                    .iter()
                    .enumerate()
                    .map(|(i, &w)| {
                        let sy = (y + i as isize - 2).clamp(0, height as isize - 1) as usize;
                        w * horizontal[(sy % 5) * extent + x] as u32
                    })
                    .sum();
                *value = ((sum + 64) >> 7) as u16;
            }
        }
        for x in 0..dw {
            let sum: u32 = SEVEN
                .iter()
                .enumerate()
                .map(|(i, &w)| {
                    w * if smooth {
                        vertical[x * 2 + i] as u32
                    } else {
                        sample((x * 2 + i) as isize - 3, y)
                    }
                })
                .sum();
            reduced[slot * dw + x] = if smooth {
                (sum + 128) >> 8
            } else {
                (sum + 32) >> 6
            } as u16;
        }
        let center = y - 3;
        if center >= 0 && center % 2 == 0 {
            let row = center as usize / 2;
            for x in 0..dw {
                let sum: u32 = SEVEN
                    .iter()
                    .enumerate()
                    .map(|(i, &w)| {
                        let bank = (center + i as isize) as usize % 7;
                        w * reduced[bank * dw + x] as u32
                    })
                    .sum();
                out[row * dw + x] = ((sum + 512) >> 10) as u8;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
