//! Intern immutable texture uploads within one render segment. Sampler controls
//! remain per draw; only layout and register-addressed image bytes form the key.
use super::super::draw_state::TextureFormat;
use super::{DrawState, Reader, recorded_memory_range};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Image {
    address: u64,
    mip_address: u64,
    width: usize,
    height: usize,
    max_level: u8,
    format: TextureFormat,
}
struct Upload {
    bytes: Arc<Vec<u8>>,
    remap: [[u32; 3]; 2],
}
#[derive(Default)]
pub(super) struct TextureUploads {
    images: HashMap<[Option<Image>; 2], Upload>,
    dma: HashMap<(u64, usize), Vec<u8>>,
}
impl TextureUploads {
    pub(super) fn prepare(
        &mut self,
        state: &DrawState,
        registers: &mut [u32],
        recorded_reads: &[(u64, Vec<u8>)],
        read: &mut Reader<'_>,
    ) -> Result<Arc<Vec<u8>>, String> {
        let key = state.textures.map(|texture| {
            texture.enabled.then_some(Image {
                address: texture.base_address,
                mip_address: texture.mip_address,
                width: texture.width,
                height: texture.height,
                max_level: texture.max_level,
                format: texture.format,
            })
        });
        let upload = match self.images.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let bytes = texture_bytes(state, registers, recorded_reads, read, &mut self.dma)?;
                let remap = std::array::from_fn(|unit| {
                    let base = if unit == 0 { 0x280 } else { 0x290 };
                    [
                        registers[base + 5],
                        registers[base + 9],
                        registers[base + 4] & 0xf0000,
                    ]
                });
                entry.insert(Upload {
                    bytes: Arc::new(bytes),
                    remap,
                })
            }
        };
        for (unit, image) in key.iter().enumerate() {
            if image.is_none() {
                continue;
            }
            let base = if unit == 0 { 0x280 } else { 0x290 };
            let [address, mip_address, levels] = upload.remap[unit];
            registers[base + 5] = address;
            registers[base + 9] = mip_address;
            registers[base + 4] = (registers[base + 4] & !0xf0000) | levels;
        }
        Ok(Arc::clone(&upload.bytes))
    }
}

fn texture_bytes(
    state: &DrawState,
    registers: &mut [u32],
    recorded_reads: &[(u64, Vec<u8>)],
    read: &mut Reader<'_>,
    texture_dma: &mut HashMap<(u64, usize), Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for (unit, texture) in state.textures.iter().enumerate() {
        if !texture.enabled {
            continue;
        }
        let bytes_per_pixel = texture.format.bytes_per_pixel()?;
        if texture.width == 0
            || texture.height == 0
            || !texture.width.is_multiple_of(8)
            || !texture.height.is_multiple_of(8)
        {
            return Err("DMP draw texture extent is invalid".into());
        }
        let (base_register, mip_register) = if unit == 0 {
            (0x285, 0x289)
        } else {
            (0x295, 0x299)
        };
        let base_offset = align_four(bytes.len());
        bytes.resize(base_offset, 0);
        registers[base_register] = u32::try_from(base_offset)
            .map_err(|_| "DMP draw texture buffer address exceeds u32")?;
        append_mip_level(
            &mut bytes,
            texture.base_address,
            texture.width,
            texture.height,
            bytes_per_pixel,
            recorded_reads,
            read,
            texture_dma,
        )?;

        let mip_offset = align_four(bytes.len());
        bytes.resize(mip_offset, 0);
        registers[mip_register] =
            u32::try_from(mip_offset).map_err(|_| "DMP draw mip buffer address exceeds u32")?;
        let mut maximum_level = 0u8;
        for level in 1..=texture.max_level {
            let width = texture.width >> level;
            let height = texture.height >> level;
            if width == 0 || height == 0 || !width.is_multiple_of(8) || !height.is_multiple_of(8) {
                break;
            }
            let mut address_offset = 0usize;
            for previous in 1..level {
                address_offset +=
                    (texture.width >> previous) * (texture.height >> previous) * bytes_per_pixel;
            }
            append_mip_level(
                &mut bytes,
                texture.mip_address + address_offset as u64,
                width,
                height,
                bytes_per_pixel,
                recorded_reads,
                read,
                texture_dma,
            )?;
            maximum_level = level;
        }
        let levels_register = if unit == 0 { 0x284 } else { 0x294 };
        registers[levels_register] =
            (registers[levels_register] & !0x000f_0000) | (u32::from(maximum_level) << 16);
    }
    if bytes.is_empty() {
        bytes.extend_from_slice(&[0; 4]);
    } else {
        bytes.resize(align_four(bytes.len()), 0);
    }
    Ok(bytes)
}

fn append_mip_level(
    bytes: &mut Vec<u8>,
    address: u64,
    width: usize,
    height: usize,
    bytes_per_pixel: usize,
    recorded_reads: &[(u64, Vec<u8>)],
    read: &mut Reader<'_>,
    texture_dma: &mut HashMap<(u64, usize), Vec<u8>>,
) -> Result<(), String> {
    let size = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or("DMP draw texture size overflow")?;
    // One segment samples a stable guest-memory view. The list renderer splits
    // before any later draw samples a target written by an earlier segment.
    let level = match texture_dma.entry((address, size)) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(recorded_memory_range(recorded_reads, address, size, read)?)
        }
    };
    if level.len() != size {
        return Err(format!(
            "DMP draw texture at {address:#x} has {} bytes, expected {size}",
            level.len()
        ));
    }
    bytes.extend_from_slice(level);
    Ok(())
}

fn align_four(length: usize) -> usize {
    (length + 3) & !3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gles::Engine;

    fn registers(engine: &Engine) -> Vec<u32> {
        let mut values = vec![0; 0x500];
        for (&index, &value) in &engine.regs {
            values[index as usize] = value;
        }
        values
    }
    #[test]
    fn sampled_images_are_shared_across_filter_border_and_lod_bias_changes() {
        let mut engine = Engine::default();
        engine.regs.extend([
            (0x280, 1),
            (0x282, 8 << 16 | 8),
            (0x284, 3 << 16),
            (0x285, 0x1000),
        ]);
        let mut uploads = TextureUploads::default();
        let mut reads = Vec::new();
        let mut reader = |address, length| {
            reads.push((address, length));
            Ok(vec![(address >> 12) as u8; length])
        };
        let first = uploads
            .prepare(
                &engine.draw_state(),
                &mut registers(&engine),
                &[],
                &mut reader,
            )
            .unwrap();
        engine
            .regs
            .extend([(0x281, 0x12345678), (0x283, 6), (0x284, 3 << 16 | 0x120)]);
        let mut params = registers(&engine);
        let second = uploads
            .prepare(&engine.draw_state(), &mut params, &[], &mut reader)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(params[0x285], 0);
        assert_eq!(params[0x289], 256);
        assert_eq!(params[0x284], 0x120);
        engine.regs.insert(0x285, 0x2000);
        let third = uploads
            .prepare(
                &engine.draw_state(),
                &mut registers(&engine),
                &[],
                &mut reader,
            )
            .unwrap();
        assert_ne!(first, third);
        assert_eq!(reads, [(0x1000, 256), (0x2000, 256)]);
    }
}
