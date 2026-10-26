//! Declared integer image-analysis laws. No registers, DMA, IRQs or firmware
//! policy belong here; callers supply bounded raster and grid dimensions.
use fujimod_emulator_imaging::analysis::ImageLayout;

/// Resample packed Y0,Y1,Cb,Cr by the nearest source pair, preserving the
/// pair's chroma and zeroing output row padding. No filter is inferred.
pub fn nearest_pairs(
    source: &[u8],
    input: ImageLayout,
    out: &mut [u8],
    output: ImageLayout,
) -> Result<(), String> {
    if source.len() != input.bytes()? || out.len() != output.bytes()? {
        return Err("packed image extent differs from its raster".into());
    }
    out.fill(0);
    for y in 0..output.height {
        let sy = y * input.height / output.height;
        for pair in 0..output.width / 2 {
            let sx = (pair * (input.width / 2) / (output.width / 2)) * 4;
            let from = sy * input.stride + sx;
            let to = y * output.stride + pair * 4;
            out[to..to + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
    Ok(())
}

/// Declared packed-luma histogram: sample every second pixel from the local
/// window origin. Window comparators exclude samples outside the input raster;
/// they do not resize the window or introduce black padding. Counts are LE u32.
pub fn histogram_luma(
    source: &[u8],
    input: ImageLayout,
    grid: [usize; 2],
    window: [usize; 4],
    output: &mut [u8; 1024],
) -> Result<(), String> {
    if source.len() != input.bytes()?
        || grid.contains(&0)
        || grid.iter().any(|&v| v > 16384)
        || window.iter().any(|v| *v > 8192)
        || window[2] == 0
        || window[3] == 0
    {
        return Err("histogram window or raster exceeds its bound".into());
    }
    let [left, top, width, height] = window;
    let mut bins = [0u32; 256];
    if grid == [input.width, input.height] {
        // The common live-view grid addresses the packed raster directly.
        // Keep the window's x parity while walking whole YYCbCr pairs; row
        // padding and samples beyond the comparators remain excluded.
        let right = (left + width).min(input.width);
        if left < right {
            let begin = left / 2 * 4;
            let end = begin + (right - left).div_ceil(2) * 4;
            for row in source.chunks_exact(input.stride).skip(top).take(height) {
                for pair in row[begin..end].chunks_exact(4) {
                    bins[pair[left % 2] as usize] += 1;
                }
            }
        }
    } else {
        for y in top..(top + height).min(grid[1]) {
            let yy = ((2 * y + 1) * input.height / (2 * grid[1])).min(input.height - 1);
            for x in (left..(left + width).min(grid[0])).step_by(2) {
                let xx = ((2 * x + 1) * input.width / (2 * grid[0])).min(input.width - 1);
                let luma = source[yy * input.stride + (xx / 2) * 4 + xx % 2];
                bins[luma as usize] += 1;
            }
        }
    }
    for (word, bin) in output.chunks_exact_mut(4).zip(bins) {
        word.copy_from_slice(&bin.to_le_bytes());
    }
    Ok(())
}

pub fn sad_curve(
    left: &[u16],
    right: &[u16],
    width: usize,
    height: usize,
    radius: usize,
) -> Result<[u32; 41], String> {
    if radius > 20
        || width <= 2 * radius
        || width
            .checked_mul(height)
            .is_none_or(|n| n == 0 || n > left.len() || n > right.len())
    {
        return Err("SAD raster exceeds its samples or comparison guard".into());
    }
    let mut curve = [0; 41];
    for shift in -(radius as isize)..=radius as isize {
        let mut sad = 0u64;
        for row in 0..height {
            for x in radius..width - radius {
                sad += left[row * width + x]
                    .abs_diff(right[row * width + (x as isize + shift) as usize])
                    as u64;
            }
        }
        curve[(20 + shift) as usize] =
            u32::try_from(sad).map_err(|_| "SAD exceeds result width")?;
    }
    Ok(curve)
}

#[derive(Clone, Copy)]
pub struct WindowGrid {
    pub columns: usize,
    pub rows: usize,
    pub origin: [usize; 2],
    pub window: [usize; 2],
    pub step: [usize; 2],
}
pub fn window_features(
    image: &[u8],
    width: usize,
    height: usize,
    grid: WindowGrid,
    out: &mut [u8],
) -> Result<(), String> {
    let WindowGrid {
        columns,
        rows,
        origin,
        window,
        step,
    } = grid;
    if width == 0
        || height == 0
        || columns == 0
        || rows == 0
        || window.contains(&0)
        || width > 2048
        || height > 2048
        || columns > 64
        || rows > 64
        || origin
            .iter()
            .chain(window.iter())
            .chain(step.iter())
            .any(|v| *v > 8192)
        || image.len() != width * height * 2
        || out.len() != columns * rows * 8
        || origin[0] + (columns - 1) * step[0] + window[0] > width
        || origin[1] + (rows - 1) * step[1] + window[1] > height
    {
        return Err("window grid exceeds its raster or result extent".into());
    }
    for row in 0..rows {
        for col in 0..columns {
            let x = origin[0] + col * step[0];
            let y = origin[1] + row * step[1];
            let mut best = (0, 0, 0u32);
            for dy in 0..window[1] {
                for dx in 0..window[0] {
                    let px = x + dx;
                    let py = y + dy;
                    let gx = pixel(image, width, px.saturating_sub(1), py).abs_diff(pixel(
                        image,
                        width,
                        (px + 1).min(width - 1),
                        py,
                    ));
                    let gy = pixel(image, width, px, py.saturating_sub(1)).abs_diff(pixel(
                        image,
                        width,
                        px,
                        (py + 1).min(height - 1),
                    ));
                    let score = gx.min(gy) as u32;
                    if score > best.2 {
                        best = (dx, dy, score);
                    }
                }
            }
            if best.2 == 0 {
                best.0 = window[0] / 2;
                best.1 = window[1] / 2;
            }
            record(
                &mut out[(row * columns + col) * 8..][..8],
                x + best.0,
                y + best.1,
                best.2,
            );
        }
    }
    Ok(())
}

pub fn track_features(
    first: &[u8],
    second: &[u8],
    features: &[u8],
    width: usize,
    height: usize,
    out: &mut [u8],
) -> Result<(), String> {
    if !(3..=2048).contains(&width)
        || !(3..=2048).contains(&height)
        || first.len() != width * height * 2
        || second.len() != first.len()
        || features.is_empty()
        || !features.len().is_multiple_of(8)
        || features.len() != out.len()
    {
        return Err("invalid tracking raster or feature extent".into());
    }
    for index in 0..features.len() / 8 {
        let f = &features[index * 8..][..8];
        let x = u16::from_le_bytes([f[0], f[1]]) as usize;
        let y = u16::from_le_bytes([f[2], f[3]]) as usize;
        let score = u32::from_le_bytes(f[4..8].try_into().unwrap());
        if x >= width || y >= height {
            return Err("tracked feature exceeds source image".into());
        }
        let (nx, ny, score) = if score == 0 || x == 0 || y == 0 || x >= width - 1 || y >= height - 1
        {
            (x, y, 0)
        } else {
            let mut best = (u32::MAX, isize::MAX, 0isize, 0isize);
            for dy in -2isize..=2 {
                for dx in -2isize..=2 {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx < 1 || ny < 1 || nx >= width as isize - 1 || ny >= height as isize - 1 {
                        continue;
                    }
                    let mut error = 0u32;
                    for py in -1isize..=1 {
                        for px in -1isize..=1 {
                            error += pixel(
                                first,
                                width,
                                (x as isize + px) as usize,
                                (y as isize + py) as usize,
                            )
                            .abs_diff(pixel(
                                second,
                                width,
                                (nx + px) as usize,
                                (ny + py) as usize,
                            )) as u32;
                        }
                    }
                    best = best.min((error, dx.abs() + dy.abs(), dy, dx));
                }
            }
            (
                (x as isize + best.3) as usize,
                (y as isize + best.2) as usize,
                score.saturating_sub(best.0 / 9),
            )
        };
        record(&mut out[index * 8..][..8], nx, ny, score);
    }
    Ok(())
}
fn pixel(image: &[u8], width: usize, x: usize, y: usize) -> u16 {
    let i = (y * width + x) * 2;
    u16::from_le_bytes([image[i], image[i + 1]])
}
fn record(out: &mut [u8], x: usize, y: usize, score: u32) {
    out[..2].copy_from_slice(&(x as u16).to_le_bytes());
    out[2..4].copy_from_slice(&(y as u16).to_le_bytes());
    out[4..].copy_from_slice(&score.to_le_bytes());
}

#[cfg(test)]
#[path = "analysis_tests.rs"]
mod tests;
