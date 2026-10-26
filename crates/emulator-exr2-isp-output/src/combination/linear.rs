//! Declared packed-422 signed Q8 four-source arithmetic. Rounding, clipping
//! and neutral chroma bias are explicit approximations, not silicon results.
use fujimod_emulator_imaging::exr2_isp::dma::{MAX_BYTES, Raster};
use super::{Prepared, read_rows};
use fujimod_emulator_devices::Cx;

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
        || width % 2 != 0
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
        if !(-256..=256).contains(coefficient) {
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
        bytes += width * height * 2;
        if bytes > MAX_BYTES {
            return Err("combination signed-sum staging exceeds bound".into());
        }
        sources.push(Some(read_rows(cx, source, width * 2)?));
    }
    let mut pixels = vec![0; width * height * 2];
    for (offset, pixel) in pixels.iter_mut().enumerate() {
        let bias = if offset % 4 >= 2 { 128 } else { 0 };
        let mut sum = 0;
        for (coefficient, source) in coefficients.iter().zip(&sources) {
            if let Some(source) = source {
                sum += coefficient * (i32::from(source[offset]) - bias);
            }
        }
        *pixel = (((sum + 128) >> 8) + bias).clamp(0, 255) as u8;
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
    fn signed_difference_and_average_use_actual_source_bytes_and_clip() {
        let (mut host, mut words) = fixture();
        let result = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).unwrap();
        assert_eq!(result.pixels, [0, 230, 0, 255, 170, 0, 128, 1]);
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
    fn invalid_coefficients_sources_and_partial_aliases_refuse_without_effects() {
        for (offset, value) in [
            (0x60, 257),
            (0x60, 0x200),
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
        assert_eq!(result.pixels, [255, 255, 0, 255, 255, 0, 128, 128]);
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
}
