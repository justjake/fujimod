//! Declared packed-422 mask blend. Mask sampling and Q8 interpolation are
//! approximations; an optional third source is added as a centered residual.
use super::{Prepared, read_rows};
use fujimod_emulator_devices::Cx;
use fujimod_emulator_imaging::exr2_isp::dma::{MAX_BYTES, Raster};

pub(super) fn prepare(cx: &Cx<'_>, word: impl Fn(u16) -> u32) -> Result<Prepared, String> {
    let control = word(4);
    let source_count = match control & !0x3000 {
        0x23 => 2,
        0x33 => 3,
        _ => 0,
    };
    let scale = 1usize << ((control >> 12) & 3);
    let width = (word(0x38) & 8191) as usize;
    let height = (word(0x38) >> 16 & 8191) as usize;
    let input_window = (word(0x3c) & 1023) as usize;
    let output_window = (word(0x3c) >> 16 & 1023) as usize;
    let format = word(0xd0);
    let input_alignment = 1usize << (format >> 12 & 7);
    let output_alignment = 1usize << (format >> 28 & 7);
    if source_count == 0
        || scale > 4
        || width == 0
        || !width.is_multiple_of(2)
        || height == 0
        || !width.is_multiple_of(scale)
        || !height.is_multiple_of(scale)
        || word(0x38) & !0x1fff1fff != 0
        || word(0x3c) & !0x03ff03ff != 0
        || output_window == 0
        || input_window < output_window
        || format & !0x70007000 != 0
        || input_alignment > 16
        || output_alignment > 16
        || source_count == 2 && (word(0x20) != 0 || word(0x24) != 0)
        || source_count == 3 && word(0x20) == 0
        || word(0x40) != 0
        || word(0x4c) != 0
    {
        let registers = [
            0x10, 0x14, 0x18, 0x1c, 0x20, 0x24, 0x28, 0x2c, 0x30, 0x34, 0x38, 0x3c, 0x40, 0x4c,
            0xd0,
        ]
        .map(|offset| (offset, word(offset)));
        return Err(format!(
            "unsupported combination mask-blend format or geometry: control={control:#x} size={:#x} windows={:#x} format={format:#x} registers={registers:x?}",
            word(0x38),
            word(0x3c)
        ));
    }
    let raster = |offset: u16, width, height, units| -> Result<Raster, String> {
        let address = word(offset);
        let pitch = word(offset + 4);
        if address & !0x3fffffff != 0 || pitch & !0x7fff != 0 {
            return Err("invalid combination mask-blend address/pitch".into());
        }
        Ok(Raster {
            address: u64::from(address),
            width,
            height,
            pitch: pitch as usize * units,
        })
    };
    let destination = raster(0x30, width, height, 2)?;
    destination.validate(width * 2)?;
    if destination.pitch % output_alignment != 0 {
        return Err("misaligned combination mask-blend destination".into());
    }
    let mut sources = Vec::new();
    let mut bytes = width * height * 2;
    for offset in [0x10, 0x18, 0x20].into_iter().take(source_count) {
        let source = raster(offset, width, height, 2)?;
        if source.pitch % input_alignment != 0
            || source != destination
                && source.active_rows_overlap(width * 2, destination, width * 2)?
        {
            return Err("misaligned or partially overlapping combination mask-blend source".into());
        }
        bytes += width * height * 2;
        if bytes > MAX_BYTES {
            return Err("combination mask-blend staging exceeds bound".into());
        }
        sources.push(read_rows(cx, source, width * 2)?);
    }
    let mask_width = width / scale;
    let mask = raster(0x28, mask_width, height / scale, 1)?;
    if mask.active_rows_overlap(mask_width, destination, width * 2)? {
        return Err("combination blend mask overlaps destination".into());
    }
    bytes += mask_width * mask.height;
    if bytes > MAX_BYTES {
        return Err("combination blend mask staging exceeds bound".into());
    }
    let mask = read_rows(cx, mask, mask_width)?;
    let mut pixels = vec![0; width * height * 2];
    for y in 0..height {
        for x in (0..width).step_by(2) {
            for component in 0..4 {
                let x = x + usize::from(component == 1);
                let offset = y * width * 2 + (x / 2) * 4 + component;
                let weight = u16::from(mask[(y / scale) * mask_width + x / scale]);
                let blended = ((u16::from(sources[0][offset]) * weight
                    + u16::from(sources[1][offset]) * (256 - weight)
                    + 128)
                    >> 8) as i32;
                let residual = if source_count == 3 {
                    let neutral = if component >= 2 { 128 } else { 0 };
                    i32::from(sources[2][offset]) - neutral
                } else {
                    0
                };
                pixels[offset] = (blended + residual).clamp(0, 255) as u8;
            }
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
        for y in 0..4 {
            host.ram[0x10000 + y * 16..0x10010 + y * 16].copy_from_slice(&[
                100, 200, 0, 255, 10, 20, 30, 40, 50, 60, 70, 80, 200, 100, 255, 0,
            ]);
            host.ram[0x20000 + y * 16..0x20010 + y * 16].copy_from_slice(&[
                20, 40, 200, 0, 100, 200, 250, 10, 30, 40, 50, 60, 0, 200, 0, 255,
            ]);
        }
        host.ram[0x40000..0x40004].copy_from_slice(&[0, 64, 128, 255]);
        (
            host,
            BTreeMap::from([
                (4, 0x1023),
                (0x10, 0x10000),
                (0x14, 8),
                (0x18, 0x20000),
                (0x1c, 8),
                (0x28, 0x40000),
                (0x2c, 4),
                (0x30, 0x30000),
                (0x34, 8),
                (0x38, 0x00040008),
                (0x3c, 0x00080010),
                (0xd0, 0x30004000),
            ]),
        )
    }
    #[test]
    fn addressed_mask_changes_each_pixel_pair_and_latches_two_real_sources() {
        let (mut host, words) = fixture();
        let result = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).unwrap();
        assert_eq!(
            &result.pixels[..16],
            &[
                20, 40, 200, 0, 78, 155, 195, 18, 40, 50, 60, 70, 199, 100, 254, 1
            ]
        );
        assert!(host.effects.is_empty());
    }
    #[test]
    fn third_source_is_added_as_a_centered_residual() {
        let (mut host, mut words) = fixture();
        for y in 0..4 {
            host.ram[0x50000 + y * 16..0x50010 + y * 16].copy_from_slice(&[
                1, 2, 128, 128, 3, 4, 128, 128, 5, 6, 128, 128, 7, 8, 128, 128,
            ]);
        }
        words.insert(4, 0x1033);
        words.insert(0x20, 0x50000);
        words.insert(0x24, 8);
        let result = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).unwrap();
        assert_eq!(
            &result.pixels[..16],
            &[
                21, 42, 200, 0, 81, 159, 195, 18, 45, 56, 60, 70, 206, 108, 254, 1
            ]
        );
        assert_eq!(result.bytes, 264);
        assert!(host.effects.is_empty());
    }
    #[test]
    fn unsupported_source_count_bad_scale_and_output_mask_alias_refuse() {
        for (offset, value) in [
            (0x20, 0x50000),
            (4, 0x3023),
            (4, 0x1043),
            (0x30, 0x40000),
            (0x1c, 0),
            (0xd0, 0x30004100),
        ] {
            let (mut host, mut words) = fixture();
            words.insert(offset, value);
            assert!(prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).is_err());
            assert!(host.effects.is_empty());
        }
    }
}
