//! Declared packed-422 pyramid blend, not camera-measured arithmetic.
//!
//! Four addressed 1/2-scale levels and an addressed 8-bit mask feed a 256-row
//! selector/weight table. Nearest sampling and 8-bit fractional interpolation
//! are explicit approximations. The tile widths bound internal working windows;
//! this implementation evaluates their full-raster equivalent. No firmware
//! state, instruction identity or callback participates in preparation.
use fujimod_emulator_imaging::exr2_isp::dma::{MAX_BYTES, Raster};
use super::{Prepared, read_rows};
use fujimod_emulator_devices::Cx;

pub(super) fn prepare(cx: &Cx<'_>, word: impl Fn(u16) -> u32) -> Result<Prepared, String> {
    let control = word(4);
    let mask_scale = 1usize << ((control >> 12) & 3);
    let width = (word(0x38) & 8191) as usize;
    let height = (word(0x38) >> 16 & 8191) as usize;
    let tile_in = (word(0x3c) & 1023) as usize;
    let tile_out = (word(0x3c) >> 16 & 1023) as usize;
    let format = word(0xd0);
    let input_alignment = 1usize << (format >> 12 & 7);
    let output_alignment = 1usize << (format >> 28 & 7);
    if control & !0x3007 != 0
        || control & 7 != 2
        || mask_scale > 4
        || width == 0
        || height == 0
        || width % 16 != 0
        || height % 8 != 0
        || word(0x38) & !0x1fff1fff != 0
        || word(0x3c) & !0x03ff03ff != 0
        || tile_out == 0
        || tile_in <= tile_out
        || tile_in % 8 != 0
        || tile_out % 8 != 0
        || format & !0x70007000 != 0
        || input_alignment > 16
        || output_alignment > 16
        || word(0x40) != 0
        || word(0x4c) != 0
    {
        return Err("unsupported combination pyramid geometry, format or transform".into());
    }
    let raster = |offset: u16, width, height, bytes_per_unit| -> Result<Raster, String> {
        let address = u64::from(word(offset));
        let pitch = word(offset + 4);
        if address & !0x3fffffff != 0 || pitch & !0x7fff != 0 {
            return Err("invalid combination address/pitch fields".into());
        }
        Ok(Raster {
            address,
            width,
            height,
            pitch: pitch as usize * bytes_per_unit,
        })
    };
    let destination = raster(0x30, width, height, 2)?;
    destination.validate(width * 2)?;
    if destination.pitch % output_alignment != 0 {
        return Err("misaligned combination destination pitch".into());
    }
    let mut sources = Vec::with_capacity(4);
    let mut bytes = width * height * 2;
    for level in 0..4 {
        let w = width >> level;
        let h = height >> level;
        let source = raster(0x10 + level * 8, w, h, 2)?;
        if source.pitch % input_alignment != 0
            || source.active_rows_overlap(w * 2, destination, width * 2)?
        {
            return Err("misaligned or overlapping combination source".into());
        }
        bytes += w * h * 2;
        if bytes > MAX_BYTES {
            return Err("combination source staging exceeds bound".into());
        }
        sources.push(read_rows(cx, source, w * 2)?);
    }
    let mask_width = width / mask_scale;
    let mask = raster(0x54, mask_width, height / mask_scale, 1)?;
    if mask.active_rows_overlap(mask_width, destination, width * 2)? {
        return Err("combination mask overlaps destination".into());
    }
    bytes += mask_width * mask.height;
    if bytes > MAX_BYTES {
        return Err("combination mask staging exceeds bound".into());
    }
    let mask = read_rows(cx, mask, mask_width)?;
    let mut table = [0u16; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        *entry = (word(0x100 + (index as u16 / 2) * 4) >> (index % 2 * 16)) as u16;
        if *entry & !0x33ff != 0 {
            return Err("combination table selector exceeds four addressed levels".into());
        }
    }
    let sample = |level: usize, x: usize, y: usize, component: usize| -> u8 {
        let w = width >> level;
        let x = (x >> level).min(w - 1);
        let y = y >> level;
        let offset = if component < 2 { x % 2 } else { component };
        sources[level][y * w * 2 + (x / 2) * 4 + offset]
    };
    let mut pixels = vec![0; width * height * 2];
    for block in (0..width).step_by(tile_out) {
        for y in 0..height {
            for x in (block..(block + tile_out).min(width)).step_by(2) {
                for component in 0..4 {
                    let x = x + usize::from(component == 1);
                    let index = mask[(y / mask_scale) * mask_width + x / mask_scale] as usize;
                    let entry = table[index];
                    let a = sample((entry >> 8 & 3) as usize, x, y, component);
                    let b = sample((entry >> 12 & 3) as usize, x, y, component);
                    let weight = entry & 255;
                    pixels[y * width * 2 + (x / 2) * 4 + component] =
                        ((u16::from(a) * weight + u16::from(b) * (256 - weight) + 128) >> 8) as u8;
                }
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
        let mut words = BTreeMap::from([
            (4, 0x2002),
            (0x30, 0x80000),
            (0x34, 24),
            (0x38, 0x00080010),
            (0x3c, 0x00080018),
            (0x54, 0x60000),
            (0x58, 8),
            (0xd0, 0x30004000),
        ]);
        let pairs: [&[u8]; 4] = [
            &[10, 20, 30, 40, 50, 60, 70, 80],
            &[110, 120, 130, 140, 150, 160, 170, 180],
            &[210, 220, 230, 240, 90, 100, 110, 120],
            &[50, 60, 70, 80],
        ];
        for level in 0..4 {
            let address = 0x10000 + level * 0x10000;
            words.insert(0x10 + level as u16 * 8, address as u32);
            words.insert(0x14 + level as u16 * 8, 32);
            let width = 16 >> level;
            for y in 0..8 >> level {
                for offset in 0..width * 2 {
                    host.ram[address + y * 64 + offset] = pairs[level][offset % pairs[level].len()];
                }
            }
        }
        for y in 0..2 {
            host.ram[0x60000 + y * 8..0x60004 + y * 8].copy_from_slice(&[0, 64, 128, 255]);
        }
        for (index, entry) in [(0, 0x0180u32), (64, 0x1240), (128, 0x23ff), (255, 0x3000)] {
            let offset = 0x100 + (index / 2) * 4;
            let shift = index % 2 * 16;
            let current = words.entry(offset).or_default();
            *current = (*current & !(0xffff << shift)) | (entry << shift);
        }
        (host, words)
    }

    #[test]
    fn mask_and_table_blend_all_four_nonconstant_addressed_levels() {
        let (mut host, words) = fixture();
        let prepared = prepare(&host.cx(), |offset| {
            words.get(&offset).copied().unwrap_or(0)
        })
        .unwrap();
        assert_eq!(prepared.destination.pitch, 48);
        // Independently calculated from the four explicit packed source rows:
        // mask 0 uses 1/2 level1+level0; mask64 uses 1/4 level2+3/4 level1;
        // mask128 nearly selects level3; mask255 selects level3 exactly.
        assert_eq!(
            &prepared.pixels[..32],
            &[
                60, 65, 80, 90, 85, 90, 100, 110, 168, 168, 185, 195, 175, 175, 185, 195, 60, 60,
                70, 80, 60, 60, 70, 80, 60, 60, 70, 80, 60, 60, 70, 80,
            ]
        );
        assert!(host.effects.is_empty());
        assert_eq!(host.ram[0x80000..0x80180], [0; 384]);
    }

    #[test]
    fn invalid_sources_formats_and_aliases_refuse_without_output() {
        for (offset, value) in [
            (0x30, 0x10000),
            (0x14, 0),
            (0xd0, 0x30004100),
            (0x38, 0x00080011),
            (0x100, 0x400),
            (0x54, 0x80000),
            (0x3c, 0),
        ] {
            let (mut host, mut words) = fixture();
            words.insert(offset, value);
            assert!(prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0)).is_err());
            assert!(host.effects.is_empty());
        }
    }
    #[test]
    fn inactive_signed_coefficients_and_other_mode_tables_do_not_change_pyramid_pixels() {
        let (mut host, mut words) = fixture();
        let before = prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0))
            .unwrap()
            .pixels;
        words.insert(0x60, 0x03000100);
        words.insert(0xd4, 0x43210000);
        words.insert(0x300, 0xffffffff);
        assert_eq!(
            prepare(&host.cx(), |o| words.get(&o).copied().unwrap_or(0))
                .unwrap()
                .pixels,
            before
        );
    }
}
