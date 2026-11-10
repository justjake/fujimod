//! Packed-422 signed Q8 four-source arithmetic: every byte, chroma included,
//! is an uncentered Q8 sum of the source bytes, rounded and clamped once at
//! the end. A camera run (three primaries and a constant Y 0, Cb Cr 255 with
//! coefficients 256, 256, 256, -256) matched this on all 16,384 output bytes:
//! neutral inputs give chroma 3 * 128 - 255 = 129. A second run weighted the
//! constant by -264 (below the stock driver's -256 limit, which the 10-bit lane
//! holds): luma read round(264 * 43 / 256) = 44 levels off, not 45, so the sum
//! rounds to nearest. Weights down to the lane's -512 are accepted; above 256
//! remains unmeasured.
use super::Prepared;
use fujimod_emulator_devices::Cx;
use fujimod_emulator_imaging::exr2_isp::dma::{Raster, physical};

pub(super) fn prepare(cx: &Cx<'_>, word: impl Fn(u16) -> u32) -> Result<Prepared, String> {
    let count = (word(4) >> 4 & 7) as usize;
    let width = (word(0x38) & 8191) as usize;
    let height = (word(0x38) >> 16 & 8191) as usize;
    let input_window = (word(0x3c) & 1023) as usize;
    let output_window = (word(0x3c) >> 16 & 1023) as usize;
    let format = word(0xd0);
    let input_alignment = 1usize << (format >> 12 & 7);
    let output_alignment = 1usize << (format >> 28 & 7);
    if word(4) & !0x70 != 4
        || !(1..=4).contains(&count)
        || width == 0
        || !width.is_multiple_of(2)
        || height == 0
        || word(0x38) & !0x1fff1fff != 0
        || word(0x3c) & !0x03ff03ff != 0
        || output_window == 0
        || input_window < output_window
        || format & !0x70007000 != 0
        || input_alignment > 16
        || output_alignment > 16
        || word(0x40) != 0
        || word(0x4c) != 0
    {
        return Err("unsupported combination signed-sum format or geometry".into());
    }
    let raster = |offset: u16| -> Result<Raster, String> {
        let address = word(offset);
        let pitch = word(offset + 4);
        if address & !0x3fffffff != 0 || pitch & !0x7fff != 0 {
            return Err("invalid combination signed-sum address/pitch".into());
        }
        Ok(Raster {
            address: u64::from(address),
            width,
            height,
            pitch: pitch as usize * 2,
        })
    };
    let destination = raster(0x30)?;
    destination.validate(width * 2)?;
    if destination.pitch % output_alignment != 0 {
        return Err("misaligned combination signed-sum destination".into());
    }
    let mut coefficients = [0i32; 4];
    let mut sources = Vec::new();
    let mut bytes = width * height * 2;
    for (index, coefficient) in coefficients.iter_mut().enumerate().take(count) {
        let lane = (word(0x60 + index as u16 / 2 * 4) >> (index % 2 * 16)) & 65535;
        if lane & !1023 != 0 {
            return Err("invalid combination signed-sum coefficient lane".into());
        }
        let value = lane & 1023;
        *coefficient = if value & 512 != 0 {
            value as i32 - 1024
        } else {
            value as i32
        };
        if !(-512..=256).contains(coefficient) {
            return Err("combination signed-sum coefficient exceeds Q8 range".into());
        }
        if *coefficient == 0 {
            sources.push(None);
            continue;
        }
        let source = raster(0x10 + index as u16 * 8)?;
        if source.pitch % input_alignment != 0
            || source != destination
                && source.active_rows_overlap(width * 2, destination, width * 2)?
        {
            return Err("misaligned or partially overlapping combination signed-sum source".into());
        }
        source.validate(width * 2)?;
        bytes += width * height * 2;
        sources.push(Some(source));
    }
    // Only the completed output is retained until DMA. Reading one source row
    // at a time avoids retaining every full raster at START. The per-raster
    // validation still bounds the output, addresses, strides and source spans.
    // No destination write occurs until the existing DMA completion path.
    let mut pixels = vec![0; width * height * 2];
    let row_bytes = width * 2;
    let mut row = vec![0u8; row_bytes];
    let mut sums = vec![0i32; row_bytes];
    for y in 0..height {
        sums.fill(0);
        for (coefficient, source) in coefficients.iter().zip(&sources) {
            if let Some(source) = source {
                cx.ram_read(
                    physical(source.address + (y * source.pitch) as u64, row_bytes)?,
                    &mut row,
                )?;
                for (sum, &value) in sums.iter_mut().zip(&row) {
                    *sum += coefficient * i32::from(value);
                }
            }
        }
        for (&sum, pixel) in sums.iter().zip(&mut pixels[y * row_bytes..(y + 1) * row_bytes]) {
            *pixel = ((sum + 128) >> 8).clamp(0, 255) as u8;
        }
    }
    Ok(Prepared {
        destination,
        pixels,
        bytes,
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::TestHost;
    use std::collections::BTreeMap;
    pub(in super::super) fn fixture() -> (TestHost, BTreeMap<u16, u32>) {
        let mut host = TestHost::new();
        host.ram[0x10000..0x10008].copy_from_slice(&[100, 240, 10, 250, 180, 0, 128, 128]);
        host.ram[0x20000..0x20008].copy_from_slice(&[200, 10, 250, 10, 10, 200, 128, 255]);
        (
            host,
            BTreeMap::from([
                (4, 0x24),
                (0x10, 0x10000),
                (0x14, 8),
                (0x18, 0x20000),
                (0x1c, 8),
                (0x30, 0x30000),
                (0x34, 8),
                (0x38, 0x00010004),
                (0x3c, 0x00080008),
                (0x60, 0x03000100),
                (0xd0, 0x40004000),
            ]),
        )
    }
    #[test]
    fn uncentered_difference_and_average_use_actual_source_bytes_and_clip() {
        let (mut host, mut words) = fixture();
        let result = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).unwrap();
        assert_eq!(result.pixels, [0, 230, 0, 240, 170, 0, 0, 0]);
        assert!(host.effects.is_empty());
        words.insert(0x60, 0x00800080);
        let result = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).unwrap();
        assert_eq!(result.pixels, [150, 125, 130, 130, 95, 100, 128, 192]);
        // A previous pyramid's LUT/mask registers are inactive in operation 4.
        words.insert(0x100, 0xffffffff);
        words.insert(0x54, 0x4000);
        assert_eq!(
            prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0))
                .unwrap()
                .pixels,
            result.pixels
        );
    }
    #[test]
    fn four_input_bias_matches_the_camera_law() {
        let (mut host, mut words) = fixture();
        host.ram[0x10000..0x10008].copy_from_slice(&[60, 70, 128, 128, 90, 30, 140, 120]);
        host.ram[0x20000..0x20008].copy_from_slice(&[0, 0, 255, 255, 0, 0, 255, 255]);
        words.insert(4, 0x44);
        for (offset, value) in [(0x18, 0x10000), (0x20, 0x10000), (0x24, 8), (0x28, 0x20000), (0x2c, 8)] {
            words.insert(offset, value);
        }
        words.insert(0x60, 0x01000100);
        words.insert(0x64, 0x03000100);
        let result = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).unwrap();
        assert_eq!(result.pixels, [180, 210, 129, 129, 255, 90, 165, 105]);
    }
    #[test]
    fn a_weight_below_minus_256_rounds_to_nearest() {
        let (mut host, mut words) = fixture();
        host.ram[0x10000..0x10008].copy_from_slice(&[20, 21, 128, 128, 60, 10, 128, 128]);
        host.ram[0x20000..0x20008].copy_from_slice(&[43, 43, 255, 255, 43, 43, 255, 255]);
        words.insert(4, 0x44);
        for (offset, value) in [(0x18, 0x10000), (0x20, 0x10000), (0x24, 8), (0x28, 0x20000), (0x2c, 8)] {
            words.insert(offset, value);
        }
        words.insert(0x60, 0x01000100);
        words.insert(0x64, (((-264i32) as u32 & 1023) << 16) | 0x100);
        let result = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).unwrap();
        assert_eq!(result.pixels, [16, 19, 121, 121, 136, 0, 121, 121]);
    }
    #[test]
    fn invalid_coefficients_sources_and_partial_aliases_refuse_without_effects() {
        for (offset, value) in [
            (0x60, 257),
            (0x60, 0x400),
            (0x18, 0),
            (0x30, 0x10004),
            (4, 0x124),
            (4, 4),
            (4, 0x54),
            (0xd0, 0x40004100),
        ] {
            let (mut host, mut words) = fixture();
            words.insert(offset, value);
            assert!(prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).is_err());
            assert!(host.effects.is_empty());
        }
    }
    #[test]
    fn declared_source_count_gates_descriptors_and_coefficient_lanes() {
        let (mut host, mut words) = fixture();
        words.insert(4, 0x34);
        words.insert(0x18, 0x10000);
        words.insert(0x20, 0x10000);
        words.insert(0x24, 8);
        words.insert(0x60, 0x01000100);
        // The fourth lane is retained even when only three sources participate.
        words.insert(0x64, 0x01000100);
        let result = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).unwrap();
        assert_eq!(result.pixels, [255, 255, 30, 255, 255, 0, 255, 255]);
        assert_eq!(result.bytes, 32);
        words.insert(0x64, 0xffff0100);
        assert_eq!(
            prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0))
                .unwrap()
                .pixels,
            result.pixels
        );
        words.insert(0x20, 0);
        assert!(prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).is_err());
        assert!(host.effects.is_empty());
    }

    #[test]
    fn full_size_padded_pair_exceeds_aggregate_staging_without_exceeding_raster_bounds() {
        let (mut host, mut words) = fixture();
        let width = 4896;
        let height = 3264;
        let pitch = 9888;
        let row_bytes = width * 2;
        let first = 0x10000;
        let second = first + pitch * height;
        let output = second + pitch * height;
        host.ram.resize(output + row_bytes * height, 0xee);
        for (address, pattern) in [(first, [100, 240, 10, 250]), (second, [201, 11, 251, 11])] {
            for y in 0..height {
                for pixel in host.ram[address + y * pitch..address + y * pitch + row_bytes]
                    .as_chunks_mut::<4>()
                    .0
                {
                    pixel.copy_from_slice(&pattern);
                }
            }
        }
        for (offset, value) in [
            (0x10, first as u32),
            (0x14, (pitch / 2) as u32),
            (0x18, second as u32),
            (0x1c, (pitch / 2) as u32),
            (0x30, output as u32),
            (0x34, width as u32),
            (0x38, ((height << 16) | width) as u32),
            (0x60, 0x00800080),
        ] {
            words.insert(offset, value);
        }
        let result = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).unwrap();
        assert_eq!(result.bytes, row_bytes * height * 3);
        assert!(result.bytes > fujimod_emulator_imaging::exr2_isp::dma::MAX_BYTES);
        assert_eq!(result.pixels.len(), row_bytes * height);
        assert!(
            result
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [151, 126, 131, 131])
        );
        assert!(host.ram[output..].iter().all(|&byte| byte == 0xee));
        assert!(host.effects.is_empty());
    }
}
