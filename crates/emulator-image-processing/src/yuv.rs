//! Explicit bounded YUV storage conversions, independent of device execution.

/// Expand tightly packed I420 Y/Cb/Cr planes to Y0,Y1,Cb,Cr 4:2:2.
/// Adjacent luma rows share the same source chroma row; luma is unchanged.
pub fn i420_to_yycbcr(width: usize, height: usize, source: &[u8]) -> Result<Vec<u8>, String> {
    let pixels = width.checked_mul(height).ok_or("I420 geometry overflows")?;
    let input_bytes = pixels.checked_mul(3).ok_or("I420 byte size overflows")? / 2;
    let output_bytes = pixels
        .checked_mul(2)
        .ok_or("packed422 byte size overflows")?;
    if width == 0
        || height == 0
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || source.len() != input_bytes
    {
        return Err("I420 conversion requires complete even-sized Y/Cb/Cr planes".into());
    }
    let mut output = vec![0; output_bytes];
    for row in 0..height {
        for pair in 0..width / 2 {
            let y = row * width + pair * 2;
            let c = row / 2 * (width / 2) + pair;
            output[y * 2..y * 2 + 4].copy_from_slice(&[
                source[y],
                source[y + 1],
                source[pixels + c],
                source[pixels + pixels / 4 + c],
            ]);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinct_chroma_planes_and_rows_repeat_without_changing_luma() {
        let mut input = (0u8..16).collect::<Vec<_>>();
        input.extend_from_slice(&[20, 30, 40, 50, 70, 80, 90, 100]);
        assert_eq!(
            i420_to_yycbcr(4, 4, &input).unwrap(),
            [
                0, 1, 20, 70, 2, 3, 30, 80, 4, 5, 20, 70, 6, 7, 30, 80, 8, 9, 40, 90, 10, 11, 50,
                100, 12, 13, 40, 90, 14, 15, 50, 100,
            ]
        );
    }
    #[test]
    fn odd_partial_and_overflowing_planes_refuse() {
        for (w, h, bytes) in [
            (0, 4, 0),
            (4, 0, 0),
            (3, 4, 18),
            (4, 3, 18),
            (4, 4, 23),
            (4, 4, 25),
        ] {
            assert!(i420_to_yycbcr(w, h, &vec![0; bytes]).is_err());
        }
        assert!(i420_to_yycbcr(usize::MAX, 2, &[]).is_err());
    }
}
