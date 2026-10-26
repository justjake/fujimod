//! Declared virtual post-ISP image and parameterized analytic statistics.
//!
//! The analytic RGB scene uses normalized 1024x768 coordinates. Output pixels
//! sample a declared source crop by nearest-neighbor pixel centers. RGB becomes
//! BT.601-limited YCbCr using exact decimal Kr=.299/Kb=.114 and integer rational
//! arithmetic, round-nearest with ties away from zero. Each 4:2:2 pair stores
//! [Y0,Y1,mean(Cb0,Cb1),mean(Cr0,Cr1)] with half-up chroma averaging.
//! Histogram bins use limited-range Y at every second horizontal
//! crop pixel, starting at crop_x, on every row. Crops require an even width.
//!
//! These functions have no machine, memory, MMIO, queue, callback or IRQ access.

pub const PROFILE: &str = "declared-rgb-bars-ramp-moving-square-bt601-limited-v1";
pub const MAX_DIMENSION: u64 = 8192;
pub const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_HISTOGRAM_SAMPLES: u64 = 16 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub enum SceneError {
    Invalid(&'static str),
    Allocation,
}

#[derive(Clone, Copy)]
pub struct Scene {
    pub width: u64,
    pub height: u64,
    pub frame: u64,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    pub x: u64,
    pub y: u64,
    pub width: u64,
    pub height: u64,
}

pub fn dimension(value: u64) -> Result<(), SceneError> {
    if value == 0 || value > MAX_DIMENSION {
        return Err(SceneError::Invalid("dimensions must be in 1..8192"));
    }
    Ok(())
}

pub fn validate_crop(scene: Scene, crop: Crop) -> Result<(), SceneError> {
    dimension(scene.width)?;
    dimension(scene.height)?;
    dimension(crop.width)?;
    dimension(crop.height)?;
    if crop
        .x
        .checked_add(crop.width)
        .is_none_or(|end| end > scene.width)
        || crop
            .y
            .checked_add(crop.height)
            .is_none_or(|end| end > scene.height)
    {
        return Err(SceneError::Invalid(
            "crop must be wholly inside the declared source raster",
        ));
    }
    Ok(())
}

/// Pure RGB pattern. All movement repeats after 1024 frames; frame arithmetic
/// is reduced before multiplication so every u64 frame index remains valid.
pub fn rgb(scene: Scene, x: u64, y: u64) -> [u8; 3] {
    let u = x * 1024 / scene.width;
    let v = y * 768 / scene.height;
    let phase = scene.frame % 1024;
    let moving_x = (u + phase * 13) % 1024;
    let mut color = if v < 384 {
        const BARS: [[u8; 3]; 8] = [
            [255, 255, 255],
            [255, 255, 0],
            [0, 255, 255],
            [0, 255, 0],
            [255, 0, 255],
            [255, 0, 0],
            [0, 0, 255],
            [0, 0, 0],
        ];
        BARS[(moving_x / 128) as usize]
    } else {
        let level = (moving_x * 255 / 1023) as u8;
        [level, level, level]
    };
    // A visible orange square crosses the lower-half ramp. Wrapping coordinates
    // keep its area consistent at boundaries; a white border marks its extent.
    let square_x = (u + 1024 - (phase * 7 % 1024)) % 1024;
    let square_y = (v + 768 - (448 + phase * 3 % 192)) % 768;
    if square_x < 160 && square_y < 128 {
        color = if square_x < 8 || square_y < 8 || square_x >= 152 || square_y >= 120 {
            [255, 255, 255]
        } else {
            [255, 96, 16]
        };
    }
    color
}

pub fn round_nearest(numerator: i64, denominator: i64) -> i64 {
    if numerator >= 0 {
        (numerator + denominator / 2) / denominator
    } else {
        -((-numerator + denominator / 2) / denominator)
    }
}

pub fn ycbcr(color: [u8; 3]) -> [u8; 3] {
    let [r, g, b] = color.map(i64::from);
    let luma = 299 * r + 587 * g + 114 * b;
    let y = 16 + round_nearest(219 * luma, 255_000);
    let cb = 128 + round_nearest(224 * (1000 * b - luma), 255 * 1772);
    let cr = 128 + round_nearest(224 * (1000 * r - luma), 255 * 1402);
    [
        y.clamp(16, 235) as u8,
        cb.clamp(16, 240) as u8,
        cr.clamp(16, 240) as u8,
    ]
}

pub fn pair(first: [u8; 3], second: [u8; 3]) -> [u8; 4] {
    let a = ycbcr(first);
    let b = ycbcr(second);
    [
        a[0],
        b[0],
        ((u16::from(a[1]) + u16::from(b[1]) + 1) / 2) as u8,
        ((u16::from(a[2]) + u16::from(b[2]) + 1) / 2) as u8,
    ]
}

pub fn output(
    scene: Scene,
    crop: Crop,
    width: u64,
    height: u64,
    stride: Option<u64>,
) -> Result<Vec<u8>, SceneError> {
    output_rgb(scene, crop, width, height, stride, |x, y| rgb(scene, x, y))
}

pub fn output_rgb(
    scene: Scene,
    crop: Crop,
    width: u64,
    height: u64,
    stride: Option<u64>,
    pixel: impl Fn(u64, u64) -> [u8; 3],
) -> Result<Vec<u8>, SceneError> {
    output_pairs(scene, crop, width, height, stride, |x0, x1, y| {
        pair(pixel(x0, y), pixel(x1, y))
    })
}

pub fn output_pairs(
    scene: Scene,
    crop: Crop,
    width: u64,
    height: u64,
    stride: Option<u64>,
    pixels: impl Fn(u64, u64, u64) -> [u8; 4],
) -> Result<Vec<u8>, SceneError> {
    validate_crop(scene, crop)?;
    dimension(width)?;
    dimension(height)?;
    if width % 2 != 0 {
        return Err(SceneError::Invalid("YYCbCr output width must be even"));
    }
    let row_bytes = width
        .checked_mul(2)
        .ok_or(SceneError::Invalid("row size overflow"))?;
    let stride = stride.unwrap_or(row_bytes);
    if stride < row_bytes {
        return Err(SceneError::Invalid(
            "stride must contain the complete packed row",
        ));
    }
    let length = stride
        .checked_mul(height)
        .filter(|&n| n <= MAX_OUTPUT_BYTES)
        .ok_or(SceneError::Invalid(
            "output span must be at most 64MiB without overflow",
        ))?;
    let length = usize::try_from(length)
        .map_err(|_| SceneError::Invalid("output span is not addressable"))?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(length)
        .map_err(|_| SceneError::Allocation)?;
    result.resize(length, 0); // Every padding byte, including the final row, is zero.
    for y in 0..height {
        let sy = crop.y + (2 * y + 1) * crop.height / (2 * height);
        for x in (0..width).step_by(2) {
            let sx0 = crop.x + (2 * x + 1) * crop.width / (2 * width);
            let sx1 = crop.x + (2 * (x + 1) + 1) * crop.width / (2 * width);
            let word = pixels(sx0, sx1, sy);
            let offset = (y * stride + x * 2) as usize;
            result[offset..offset + 4].copy_from_slice(&word);
        }
    }
    Ok(result)
}

pub fn histogram(scene: Scene, crop: Crop) -> Result<Vec<u8>, SceneError> {
    validate_crop(scene, crop)?;
    if crop.width % 2 != 0 {
        return Err(SceneError::Invalid(
            "half-horizontal histogram crop width must be even",
        ));
    }
    let samples = (crop.width / 2)
        .checked_mul(crop.height)
        .filter(|&n| n <= MAX_HISTOGRAM_SAMPLES)
        .ok_or(SceneError::Invalid(
            "histogram must contain at most 16Mi samples without overflow",
        ))?;
    // The cap also bounds every accumulator below u32::MAX. Count exact input
    // raster samples, rather than histogramming either resampled video output.
    let mut bins = [0u32; 256];
    for y in 0..crop.height {
        for x in (0..crop.width).step_by(2) {
            let bin = ycbcr(rgb(scene, crop.x + x, crop.y + y))[0] as usize;
            bins[bin] += 1;
        }
    }
    debug_assert_eq!(bins.iter().map(|&n| u64::from(n)).sum::<u64>(), samples);
    let mut result = Vec::with_capacity(1024);
    for bin in bins {
        result.extend_from_slice(&bin.to_le_bytes());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(width: u64, height: u64, frame: u64) -> Scene {
        Scene {
            width,
            height,
            frame,
        }
    }
    fn crop(width: u64, height: u64) -> Crop {
        Crop {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    #[test]
    fn known_bt601_limited_pixels_and_neutral_pair() {
        assert_eq!(ycbcr([0, 0, 0]), [16, 128, 128]);
        assert_eq!(ycbcr([255, 255, 255]), [235, 128, 128]);
        assert_eq!(ycbcr([255, 0, 0]), [81, 90, 240]);
        assert_eq!(ycbcr([0, 255, 0]), [145, 54, 34]);
        assert_eq!(ycbcr([0, 0, 255]), [41, 240, 110]);
        assert_eq!(pair([0, 0, 0], [255, 255, 255]), [16, 235, 128, 128]);
    }

    #[test]
    fn packed_order_resampling_and_padding_are_exact() {
        let s = scene(8, 4, 0);
        let tight = output(s, crop(8, 4), 8, 4, None).unwrap();
        let padded = output(s, crop(8, 4), 8, 4, Some(20)).unwrap();
        assert_eq!(tight.len(), 64);
        assert_eq!(padded.len(), 80);
        assert_eq!(&tight[..4], &[235, 210, 72, 137]); // White/yellow, shared chroma.
        for row in 0..4 {
            assert_eq!(
                &padded[row * 20..row * 20 + 16],
                &tight[row * 16..row * 16 + 16]
            );
            assert_eq!(&padded[row * 20 + 16..row * 20 + 20], &[0; 4]);
        }
        // Nearest-neighbor pixel centers select source pixels(2,2)/(6,2),
        // gray levels63/191 in the lower-half ramp.
        let downsample = output(s, crop(8, 4), 2, 1, None).unwrap();
        assert_eq!(downsample, [70, 180, 128, 128]);
    }

    #[test]
    fn scene_is_deterministic_moving_and_frame_overflow_safe() {
        let a = output(scene(256, 192, 0), crop(256, 192), 128, 96, None).unwrap();
        assert_eq!(
            a,
            output(scene(256, 192, 0), crop(256, 192), 128, 96, None).unwrap()
        );
        assert_ne!(
            a,
            output(scene(256, 192, 1), crop(256, 192), 128, 96, None).unwrap()
        );
        assert_eq!(
            a,
            output(scene(256, 192, 1024), crop(256, 192), 128, 96, None).unwrap()
        );
        assert!(output(scene(2, 1, u64::MAX), crop(2, 1), 2, 1, None).is_ok());
    }

    #[test]
    fn histogram_known_bins_and_total_use_half_horizontal_input() {
        let bytes = histogram(
            scene(8, 4, 0),
            Crop {
                x: 0,
                y: 0,
                width: 8,
                height: 1,
            },
        )
        .unwrap();
        assert_eq!(bytes.len(), 1024);
        let bins: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(bins.iter().sum::<u32>(), 4);
        for bin in [235, 170, 106, 41] {
            assert_eq!(bins[bin], 1);
        }
        let roi = Crop {
            x: 36,
            y: 39,
            width: 2400,
            height: 800,
        };
        let bytes = histogram(scene(2496, 853, 1), roi).unwrap();
        let sum: u64 = bytes
            .chunks_exact(4)
            .map(|b| u64::from(u32::from_le_bytes(b.try_into().unwrap())))
            .sum();
        assert_eq!(sum, 960000);
        assert_eq!(bytes, histogram(scene(2496, 853, 1), roi).unwrap());
        assert_ne!(bytes, histogram(scene(2496, 853, 2), roi).unwrap());
        let current_roi = Crop {
            x: 12,
            y: 31,
            width: 2448,
            height: 809,
        };
        let bytes = histogram(scene(2496, 853, 1), current_roi).unwrap();
        let sum: u64 = bytes
            .chunks_exact(4)
            .map(|b| u64::from(u32::from_le_bytes(b.try_into().unwrap())))
            .sum();
        assert_eq!(sum, 990216);
    }

    #[test]
    fn invalid_geometry_spans_and_overflow_are_rejected() {
        let s = scene(8, 4, 0);
        let c = crop(8, 4);
        for (w, h, stride) in [
            (0, 1, None),
            (3, 1, None),
            (2, 0, None),
            (8194, 1, None),
            (8, 4, Some(15)),
            (8, 4, Some(u64::MAX)),
            (8192, 8192, None),
        ] {
            assert!(output(s, c, w, h, stride).is_err());
        }
        assert!(output(scene(0, 4, 0), c, 8, 4, None).is_err());
        assert!(output(s, Crop { x: u64::MAX, ..c }, 8, 4, None).is_err());
        assert!(
            output(
                s,
                Crop {
                    y: 3,
                    height: 2,
                    ..c
                },
                8,
                4,
                None
            )
            .is_err()
        );
        assert!(histogram(s, Crop { width: 3, ..c }).is_err());
        assert!(histogram(scene(8192, 8192, 0), crop(8192, 8192)).is_err());
    }
}
