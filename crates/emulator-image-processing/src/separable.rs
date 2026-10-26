//! Declared integer component filters: replicated edges, rounded horizontal
//! and vertical passes, then pixel-center rational bilinear sampling. These
//! software numerical laws are not measured Fuji filter precision.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Samples {
    Scalar16,
    YyCbCr422,
    Luma8,
}
impl Samples {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Scalar16 | Self::YyCbCr422 => 2,
            Self::Luma8 => 1,
        }
    }
}

pub struct Geometry {
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub output_width: usize,
    pub output_height: usize,
    pub input: Samples,
    pub output: Samples,
}
pub fn filter(
    source: &[u8],
    g: Geometry,
    horizontal: &[u32],
    vertical: &[u32],
    ratio: [usize; 2],
    out: &mut [u8],
    a: &mut [u16],
    b: &mut [u16],
) -> Result<(), String> {
    let Geometry {
        width: w,
        height: h,
        pitch,
        output_width: dw,
        output_height: dh,
        input,
        output,
    } = g;
    let [num, den] = ratio;
    if w == 0
        || h == 0
        || dw == 0
        || dh == 0
        || pitch < w * input.bytes_per_pixel()
        || source.len() != pitch * h
        || out.len() != dw * dh * output.bytes_per_pixel()
        || a.len() < w * h
        || b.len() < w * h
        || input != output && !(input == Samples::YyCbCr422 && output == Samples::Luma8)
        || input == Samples::YyCbCr422 && !w.is_multiple_of(2)
        || output == Samples::YyCbCr422 && !dw.is_multiple_of(2)
        || num == 0
        || den == 0
        || num > 512
        || den > 512
        || [horizontal, vertical].iter().any(|k| {
            k.is_empty()
                || k.len() % 2 == 0
                || k.iter().sum::<u32>() == 0
                || k.iter().sum::<u32>() > 256
        })
    {
        return Err("invalid bounded separable raster geometry/filter".into());
    }
    for channel in 0..if output == Samples::YyCbCr422 { 3 } else { 1 } {
        let sw = if channel == 0 { w } else { w / 2 };
        let ow = if channel == 0 { dw } else { dw / 2 };
        for y in 0..h {
            for x in 0..sw {
                a[y * sw + x] = if input == Samples::Scalar16 {
                    u16::from_le_bytes(
                        source[y * pitch + x * 2..y * pitch + x * 2 + 2]
                            .try_into()
                            .unwrap(),
                    )
                } else if input == Samples::Luma8 {
                    source[y * pitch + x] as u16
                } else {
                    source[y * pitch
                        + if channel == 0 {
                            x / 2 * 4 + x % 2
                        } else {
                            x * 4 + channel + 1
                        }] as u16
                };
            }
        }
        for (kernel, along_x) in [(horizontal, true), (vertical, false)] {
            let sum: u32 = kernel.iter().sum();
            let radius = kernel.len() / 2;
            for y in 0..h {
                for x in 0..sw {
                    let mut total = 0;
                    for (n, &weight) in kernel.iter().enumerate() {
                        let sx = if along_x {
                            (x + n).saturating_sub(radius).min(sw - 1)
                        } else {
                            x
                        };
                        let sy = if along_x {
                            y
                        } else {
                            (y + n).saturating_sub(radius).min(h - 1)
                        };
                        total += a[sy * sw + sx] as u32 * weight;
                    }
                    b[y * sw + x] = ((total + sum / 2) / sum) as u16;
                }
            }
            a[..sw * h].copy_from_slice(&b[..sw * h]);
        }
        let axis = |n: usize, limit: usize| {
            let unit = 2 * den;
            let pos = ((2 * n + 1) * num)
                .saturating_sub(den)
                .min((limit - 1) * unit);
            (
                pos / unit,
                (pos / unit + 1).min(limit - 1),
                (pos % unit) as u64,
            )
        };
        let unit = (2 * den) as u64;
        for y in 0..dh {
            let (y0, y1, fy) = axis(y, h);
            for x in 0..ow {
                let (x0, x1, fx) = axis(x, sw);
                let sample =
                    |y: usize| a[y * sw + x0] as u64 * (unit - fx) + a[y * sw + x1] as u64 * fx;
                let value = ((sample(y0) * (unit - fy) + sample(y1) * fy + unit * unit / 2)
                    / (unit * unit)) as u16;
                if output == Samples::Scalar16 {
                    out[(y * dw + x) * 2..(y * dw + x) * 2 + 2]
                        .copy_from_slice(&value.to_le_bytes());
                } else if output == Samples::Luma8 {
                    out[y * dw + x] = value as u8;
                } else {
                    let at = y * dw * 2
                        + if channel == 0 {
                            x / 2 * 4 + x % 2
                        } else {
                            x * 4 + channel + 1
                        };
                    out[at] = value as u8;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replicated_edge_rounding_and_component_grids() {
        let source = [0, 100, 20, 200, 200, 240, 60, 100];
        let mut out = [0; 8];
        filter(
            &source,
            Geometry {
                width: 4,
                height: 1,
                pitch: 8,
                output_width: 4,
                output_height: 1,
                input: Samples::YyCbCr422,
                output: Samples::YyCbCr422,
            },
            &[1, 2, 1],
            &[1],
            [1, 1],
            &mut out,
            &mut [0; 4],
            &mut [0; 4],
        )
        .unwrap();
        assert_eq!(out, [25, 100, 30, 175, 185, 230, 50, 125]);
    }
    #[test]
    fn scalar_precision_and_pixel_center_rational_sampling() {
        let source = [0u16, 1000, 2000, 3000]
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut out = [0; 4];
        filter(
            &source,
            Geometry {
                width: 4,
                height: 1,
                pitch: 8,
                output_width: 2,
                output_height: 1,
                input: Samples::Scalar16,
                output: Samples::Scalar16,
            },
            &[1],
            &[1],
            [2, 1],
            &mut out,
            &mut [0; 4],
            &mut [0; 4],
        )
        .unwrap();
        assert_eq!(out, [0xf4, 1, 0xc4, 9]);
    }
}

#[cfg(test)]
mod conversion_tests {
    use super::*;

    #[test]
    fn packed_half_size_luma_excludes_chroma_and_retains_byte_range() {
        let source = [
            0, 20, 1, 250, 100, 120, 3, 240, 40, 60, 10, 20, 140, 160, 30, 40,
        ];
        let mut out = [0; 2];
        filter(
            &source,
            Geometry {
                width: 4,
                height: 2,
                pitch: 8,
                output_width: 2,
                output_height: 1,
                input: Samples::YyCbCr422,
                output: Samples::Luma8,
            },
            &[1],
            &[1],
            [2, 1],
            &mut out,
            &mut [0; 8],
            &mut [0; 8],
        )
        .unwrap();
        assert_eq!(out, [30, 130]);
    }
}
