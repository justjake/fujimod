//! Declared fixed-luma Y0,Y1,Cb,Cr input at programmed imaging DMA outputs.
//! This source is a scenario input, not sensor/ISP or coefficient fidelity.
use fujimod_emulator_devices::Cx;

pub fn paint(
    cx: &mut Cx<'_>,
    address: u64,
    width: u32,
    height: u32,
    stride: u32,
    luma: u8,
) -> Result<(), String> {
    if !(16..=235).contains(&luma)
        || width == 0
        || width & 1 != 0
        || height == 0
        || width > 4096
        || height > 4096
        || stride < width * 2
        || stride & 3 != 0
        || u64::from(stride) * u64::from(height) > 32 * 1024 * 1024
    {
        return Err("fixed YUV source geometry/range exceeds modeled bounds".into());
    }
    address
        .checked_add(u64::from(stride) * u64::from(height))
        .ok_or("YUV DMA address overflow")?;
    let row = [luma, luma, 128, 128].repeat(width as usize / 2);
    for y in 0..height {
        cx.ram_write(address + u64::from(y) * u64::from(stride), &row)?;
    }
    Ok(())
}

/// Half-size packed422 sampling with the controller's normalized symmetric
/// seven-tap horizontal filter. Co-sited sampling and edge clamping are model
/// assumptions; no camera filter/phase capture has qualified them.
pub fn half_422(
    input: &[u8],
    width: usize,
    height: usize,
    weights: Option<[u8; 4]>,
) -> Result<Vec<u8>, String> {
    if width < 4
        || width & 3 != 0
        || height < 2
        || height & 1 != 0
        || input.len() != width * height * 2
    {
        return Err("invalid packed422 half-size input".into());
    }
    if weights.is_some_and(|w| {
        2 * (u32::from(w[0]) + u32::from(w[1]) + u32::from(w[2])) + u32::from(w[3]) != 256
    }) {
        return Err("MOVRSZ horizontal filter is not normalized".into());
    }
    let taps = weights.map(|w| [w[0], w[1], w[2], w[3], w[2], w[1], w[0]]);
    let sample = |row: usize, channel: usize, x: usize| -> u8 {
        let at = row * width * 2;
        input[at
            + if channel == 0 {
                x / 2 * 4 + x % 2
            } else {
                x * 4 + channel + 1
            }]
    };
    let filter = |row: usize, channel: usize, x: usize| -> u8 {
        let Some(taps) = taps else {
            return sample(row, channel, x);
        };
        let count = if channel == 0 { width } else { width / 2 };
        let sum: u32 = taps
            .into_iter()
            .enumerate()
            .map(|(tap, weight)| {
                let x = (x as isize + tap as isize - 3).clamp(0, count as isize - 1) as usize;
                u32::from(sample(row, channel, x)) * u32::from(weight)
            })
            .sum();
        ((sum + 128) >> 8) as u8
    };
    let mut output = Vec::with_capacity(width * height / 2);
    for y in 0..height / 2 {
        for group in 0..width / 4 {
            output.extend_from_slice(&[
                filter(y * 2, 0, group * 4),
                filter(y * 2, 0, group * 4 + 2),
                filter(y * 2, 1, group * 2),
                filter(y * 2, 2, group * 2),
            ]);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn half_sampling_keeps_channels_and_applies_programmed_normalized_weights() {
        let mut bytes = Vec::new();
        for row in 0..2 {
            for group in 0..4u8 {
                bytes.extend_from_slice(&[group * 32 + row, group * 32 + 16 + row, 128, 128]);
            }
        }
        let nearest = half_422(&bytes, 8, 2, None).unwrap();
        assert_eq!(nearest, [0, 32, 128, 128, 64, 96, 128, 128]);
        let filtered = half_422(&bytes, 8, 2, Some([0, 0, 64, 128])).unwrap();
        assert_eq!(filtered, [4, 32, 128, 128, 64, 96, 128, 128]);
        assert!(half_422(&bytes, 8, 2, Some([0, 0, 0, 0])).is_err());
    }
}
