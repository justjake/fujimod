//! GPU paint stage for one SMAPH path bounding box.
//!
//! Coverage is supplied by the eighth-pixel coverage stage. This module
//! samples paint, applies color state and blends against the destination.

use super::{Bank, Gradient, Texture};
use fujimod_emulator_gpu::GpuSurface;
use std::sync::OnceLock;
use wgpu::util::DeviceExt;

struct Gpu {
    #[cfg(test)]
    name: String,
    pipeline: wgpu::ComputePipeline,
}

impl Gpu {
    fn new(context: &fujimod_emulator_gpu::GpuContext) -> Self {
        let device = context.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SMAPH path paint"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_paint.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("SMAPH path paint"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            #[cfg(test)]
            name: context.adapter_name().to_owned(),
            pipeline,
        }
    }

    fn paint(
        &self,
        context: &fujimod_emulator_gpu::GpuContext,
        coverage: &[u32],
        bounds: [usize; 4],
        target_extent: [usize; 2],
        target: &[u8],
        samples: u32,
        regs: &Bank,
        gamma: &[u8],
        order: u32,
        tiled: bool,
        flip: bool,
        scissors: &[[u32; 4]],
        solid: [u8; 4],
        blend: u32,
        texture: Option<&Texture>,
        gradient: Option<&Gradient>,
        gradient_deltas: &[u32],
        resident_surface: Option<&GpuSurface>,
    ) -> Result<Vec<u8>, String> {
        let device = context.device();
        let [x0, x1, y0, y1] = bounds;
        let [width, height] = target_extent;
        let bbox_width = x1.saturating_sub(x0);
        let bbox_height = y1.saturating_sub(y0);
        let count = bbox_width
            .checked_mul(bbox_height)
            .ok_or("SMAPH paint extent overflow")?;
        if count == 0 {
            return Ok(Vec::new());
        }
        let target_pixels = width
            .checked_mul(height)
            .ok_or("SMAPH target extent overflow")?;
        let target_bytes = target_pixels
            .checked_mul(4)
            .ok_or("SMAPH target byte extent overflow")?;
        if x1 > width
            || y1 > height
            || width == 0
            || height == 0
            || width > 2048
            || height > 2048
            || !matches!(samples, 1 | 2 | 4)
            || coverage.len() != count
            || target.len() != target_bytes
            || target_bytes > super::MAX_BYTES
            || gamma.len() != 256
            || blend > 9
            || scissors.len() > 16
            || !matches!(order, 1 | 0x101 | 0x01000001 | 0x01000101)
            || (texture.is_some() && gradient.is_some())
            || resident_surface.is_some_and(|surface| {
                let key = surface.key();
                key.format != fujimod_emulator_gpu::SurfaceFormat::Rgba8
                    || key.extent.width as usize != width
                    || key.extent.height as usize != height
            })
        {
            return Err("SMAPH paint inputs have invalid extents or state".into());
        }
        if let Some(texture) = texture {
            let bytes_per_pixel = match texture.format {
                7 | 8 => 1,
                2..=4 => 2,
                0 | 9 => 4,
                _ => return Err("SMAPH paint texture format is unsupported".into()),
            };
            if texture.width == 0
                || texture.height == 0
                || (texture.width | texture.height) & 7 != 0
                || texture.width > 2048
                || texture.height > 2048
                || texture.valid[0] > texture.width
                || texture.valid[1] > texture.height
                || texture.bytes.len() != texture.width * texture.height * bytes_per_pixel
                || texture.bytes.len() > super::MAX_BYTES - target_bytes
                || !matches!(texture.swizzle, 0 | 0x1b | 0x93)
            {
                return Err("SMAPH paint texture state is unsupported".into());
            }
        }

        let mut data = Vec::<u32>::new();
        data.extend(gamma.iter().map(|value| u32::from(*value)));
        let gamma_offset = 0usize;
        let colors_offset = data.len();
        let colors = gradient.map_or(&[][..], |gradient| gradient.colors.as_slice());
        if gradient.is_some() && (colors.len() != 128 || gradient_deltas.len() != 128) {
            return Err("SMAPH paint gradient ramp must contain 128 entries".into());
        }
        if gradient.is_some_and(|gradient| gradient.spread > 2) {
            return Err("SMAPH paint gradient spread is unsupported".into());
        }
        data.extend_from_slice(colors);
        let deltas_offset = data.len();
        data.extend_from_slice(gradient_deltas);

        let target_offset = data.len();
        if resident_surface.is_none() {
            data.extend(
                target
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| u32::from_le_bytes(*bytes)),
            );
        }
        let image_offset = data.len();
        if let Some(texture) = texture {
            for chunk in texture.bytes.chunks(4) {
                let mut bytes = [0; 4];
                bytes[..chunk.len()].copy_from_slice(chunk);
                data.push(u32::from_le_bytes(bytes));
            }
        }
        let scissor_offset = data.len();
        for rect in scissors {
            data.extend_from_slice(rect);
        }
        if data.is_empty() {
            data.push(0);
        }

        let mut params = [0u32; 68];
        params[0] = x0.try_into().map_err(|_| "SMAPH paint x origin")?;
        params[1] = y0.try_into().map_err(|_| "SMAPH paint y origin")?;
        params[2] = bbox_width
            .try_into()
            .map_err(|_| "SMAPH paint bounding width")?;
        params[3] = bbox_height
            .try_into()
            .map_err(|_| "SMAPH paint bounding height")?;
        params[4] = width.try_into().map_err(|_| "SMAPH paint target width")?;
        params[5] = height.try_into().map_err(|_| "SMAPH paint target height")?;
        params[6] = samples;
        params[7] = order;
        params[8] = blend;
        params[9] = u32::from(tiled);
        params[10] = u32::from(flip);
        params[11] = scissors.len() as u32;
        params[12] = scissor_offset
            .try_into()
            .map_err(|_| "SMAPH paint scissor offset")?;
        params[13] = u32::from(reg(regs, 0x184)? & 0x1000000 != 0);
        params[14] = u32::from(reg(regs, 0x184)? & 0x10000 != 0);
        params[15] = if texture.is_some() {
            1
        } else if gradient.is_some() {
            2
        } else {
            0
        };
        params[16] = u32::from_le_bytes(solid);
        params[17] = gamma_offset as u32;
        params[18] = target_offset
            .try_into()
            .map_err(|_| "SMAPH paint target offset")?;
        params[19] = image_offset
            .try_into()
            .map_err(|_| "SMAPH paint image offset")?;
        params[20] = colors_offset
            .try_into()
            .map_err(|_| "SMAPH paint gradient color offset")?;
        params[21] = deltas_offset
            .try_into()
            .map_err(|_| "SMAPH paint gradient delta offset")?;
        params[22] = scissors.len() as u32;
        if let Some(texture) = texture {
            params[23] = texture.width as u32;
            params[24] = texture.height as u32;
            params[25] = texture.valid[0] as u32;
            params[26] = texture.valid[1] as u32;
            params[27] = texture.format;
            params[28] = u32::from(texture.premultiplied);
            params[29] = texture.swizzle;
            params[30] = u32::from(texture.rotate);
            params[31] = u32::from(texture.opaque);
            for (index, value) in texture.matrix.iter().enumerate() {
                params[32 + index] = (*value as f32).to_bits();
            }
        }
        if let Some(gradient) = gradient {
            params[41] = u32::from(gradient.active);
            params[42] = gradient.spread;
            params[43] = u32::from(gradient.premultiplied);
            if let Some((flags, coefficients, scale)) = gradient.radial {
                params[44] = 1;
                params[45] = flags;
                params[46] = (coefficients[0] as f32).to_bits();
                params[47] = (coefficients[1] as f32).to_bits();
                params[48] = (scale as f32).to_bits();
            }
            for (index, value) in gradient.matrix.iter().enumerate() {
                params[49 + index] = (*value as f32).to_bits();
            }
        }
        let transform_enabled = params[13] != 0;
        params[58] = u32::from(transform_enabled);
        if transform_enabled {
            for channel in 0..4 {
                let shift = (channel % 2) * 16;
                let scale = (reg(regs, 0x180 + (channel / 2) as u16)? >> shift) as i16;
                let bias = (reg(regs, 0x182 + (channel / 2) as u16)? >> shift) as i16;
                params[59 + channel * 2] = scale as i32 as u32;
                params[60 + channel * 2] = bias as i32 as u32;
            }
        }

        let resident = resident_surface.is_some();
        let bytes_per_row = if resident {
            bbox_width
                .checked_mul(4)
                .and_then(|bytes| bytes.checked_add(255))
                .map(|bytes| bytes & !255)
                .ok_or("SMAPH paint row stride overflow")?
        } else {
            bbox_width
                .checked_mul(4)
                .ok_or("SMAPH paint row stride overflow")?
        };
        params[66] = u32::from(resident);
        params[67] = (bytes_per_row / 4)
            .try_into()
            .map_err(|_| "SMAPH paint row stride exceeds u32")?;

        let param_bytes: Vec<u8> = params.into_iter().flat_map(u32::to_le_bytes).collect();
        let coverage_bytes: Vec<u8> = coverage
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let data_bytes: Vec<u8> = data.into_iter().flat_map(u32::to_le_bytes).collect();
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SMAPH paint parameters"),
            contents: &param_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let coverage_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SMAPH paint coverage"),
            contents: &coverage_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let data_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SMAPH paint input data"),
            contents: &data_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let byte_count = (bytes_per_row as usize)
            .checked_mul(bbox_height)
            .ok_or("SMAPH paint output buffer size overflow")?;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SMAPH paint output"),
            size: byte_count as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let dummy_target = resident_surface.is_none().then(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("SMAPH paint unused target"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        });
        let target_view = resident_surface.map_or_else(
            || {
                dummy_target
                    .as_ref()
                    .unwrap()
                    .create_view(&Default::default())
            },
            |surface| surface.texture().create_view(&Default::default()),
        );
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SMAPH paint bindings"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: coverage_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: data_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&target_view),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups((params[2]).div_ceil(8), (params[3]).div_ceil(8), 1);
        }
        if let Some(surface) = resident_surface {
            let destination_y = if flip { height - y1 } else { y0 };
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &output,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row as u32),
                        rows_per_image: Some(bbox_height as u32),
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture: surface.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: x0 as u32,
                        y: destination_y as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: bbox_width as u32,
                    height: bbox_height as u32,
                    depth_or_array_layers: 1,
                },
            );
            context.submit(encoder.finish());
            return Ok(Vec::new());
        }
        let download = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SMAPH paint readback"),
            size: byte_count as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&output, 0, &download, 0, byte_count as u64);
        context.submit(encoder.finish());

        let (sender, receiver) = std::sync::mpsc::channel();
        download
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| format!("SMAPH paint GPU poll: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("SMAPH paint readback: {error}"))?
            .map_err(|error| format!("SMAPH paint map: {error}"))?;
        let bytes = download
            .slice(..)
            .get_mapped_range()
            .map_err(|error| format!("SMAPH paint mapped range: {error}"))?;
        let result = bytes.to_vec();
        drop(bytes);
        download.unmap();
        Ok(result)
    }
}

fn reg(regs: &Bank, index: u16) -> Result<u32, String> {
    regs.get(&index)
        .copied()
        .ok_or_else(|| format!("SMAPH paint missing register {index:#x}"))
}

static GPU: OnceLock<Result<Option<Gpu>, String>> = OnceLock::new();

fn shared_gpu() -> Result<Option<&'static Gpu>, String> {
    let context = match fujimod_emulator_gpu::context() {
        Ok(context) => context,
        Err(error) if error.starts_with("host GPU adapter:") => return Ok(None),
        Err(error) => return Err(error),
    };
    match GPU.get_or_init(|| Ok(Some(Gpu::new(context)))) {
        Ok(Some(gpu)) => Ok(Some(gpu)),
        Ok(None) => Ok(None),
        Err(error) => Err(error.clone()),
    }
}

/// Paint a row-major bounding box and return its final RGBA target bytes.
///
/// `coverage` is row-major and uses the same sample count as the path
/// coverage stage. Scissors use exclusive right and bottom coordinates.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(super) fn paint(
    coverage: &[u32],
    bounds: [usize; 4],
    target_extent: [usize; 2],
    target: &[u8],
    samples: u32,
    regs: &Bank,
    gamma: &[u8],
    order: u32,
    tiled: bool,
    flip: bool,
    scissors: &[[u32; 4]],
    solid: [u8; 4],
    blend: u32,
    texture: Option<&Texture>,
    gradient: Option<&Gradient>,
    gradient_deltas: &[u32],
) -> Result<Vec<u8>, String> {
    let gpu = shared_gpu()?.ok_or_else(|| "SMAPH paint has no wgpu adapter".to_owned())?;
    gpu.paint(
        fujimod_emulator_gpu::context()?,
        coverage,
        bounds,
        target_extent,
        target,
        samples,
        regs,
        gamma,
        order,
        tiled,
        flip,
        scissors,
        solid,
        blend,
        texture,
        gradient,
        gradient_deltas,
        None,
    )
}

/// Paint a bounding box directly into its shared resident target.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_resident(
    surface: &GpuSurface,
    coverage: &[u32],
    bounds: [usize; 4],
    target_extent: [usize; 2],
    target: &[u8],
    samples: u32,
    regs: &Bank,
    gamma: &[u8],
    order: u32,
    tiled: bool,
    flip: bool,
    scissors: &[[u32; 4]],
    solid: [u8; 4],
    blend: u32,
    texture: Option<&Texture>,
    gradient: Option<&Gradient>,
    gradient_deltas: &[u32],
) -> Result<(), String> {
    let gpu = shared_gpu()?.ok_or_else(|| "SMAPH paint has no wgpu adapter".to_owned())?;
    gpu.paint(
        fujimod_emulator_gpu::context()?,
        coverage,
        bounds,
        target_extent,
        target,
        samples,
        regs,
        gamma,
        order,
        tiled,
        flip,
        scissors,
        solid,
        blend,
        texture,
        gradient,
        gradient_deltas,
        Some(surface),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{paint, shared_gpu};
    use crate::smaph::{Bank, Gradient, Texture, blend_pixel, packed, pixel_index};

    fn require_gpu() -> bool {
        match shared_gpu() {
            Ok(Some(gpu)) => {
                println!("SMAPH GPU paint adapter: {}", gpu.name);
                true
            }
            Ok(None) => {
                println!("SKIP SMAPH GPU paint: no wgpu adapter is available");
                false
            }
            Err(error) => panic!("SMAPH GPU paint adapter initialization failed: {error}"),
        }
    }

    fn regs(color_transform: bool, gamma_enabled: bool) -> Bank {
        let mut regs = Bank::new();
        regs.insert(
            0x184,
            u32::from(color_transform) * 0x1000000 + u32::from(gamma_enabled) * 0x10000,
        );
        if color_transform {
            regs.extend([
                (0x180, 0x0080_0100),
                (0x181, 0xffc0_0180),
                (0x182, 0x0020_ffc0),
                (0x183, 0xffe0_0040),
            ]);
        }
        regs
    }

    fn gamma() -> Vec<u8> {
        (0..=255)
            .map(|value| ((value * 73 + 19) & 255) as u8)
            .collect()
    }

    fn target(seed: u32, width: usize, height: usize) -> Vec<u8> {
        let mut value = seed;
        (0..width * height * 4)
            .map(|_| {
                value ^= value << 13;
                value ^= value >> 17;
                value ^= value << 5;
                value as u8
            })
            .collect()
    }

    fn gpu_coverage(
        contours: &[Vec<[f64; 2]>],
        bounds: [usize; 4],
        samples: u32,
        nonzero: bool,
    ) -> Vec<u32> {
        crate::smaph::gpu_coverage::coverage(
            contours, bounds[0], bounds[1], bounds[2], bounds[3], samples, None, nonzero,
        )
        .unwrap_or_else(|error| panic!("GPU coverage failed: {error}"))
    }

    #[allow(clippy::too_many_arguments)]
    fn cpu_paint(
        coverage: &[u32],
        bounds: [usize; 4],
        extent: [usize; 2],
        target: &[u8],
        samples: u32,
        regs: &Bank,
        gamma: &[u8],
        order: u32,
        tiled: bool,
        flip: bool,
        scissors: &[[u32; 4]],
        solid: [u8; 4],
        blend: u32,
        texture: Option<&Texture>,
        gradient: Option<&Gradient>,
    ) -> Vec<u8> {
        let [x0, x1, y0, y1] = bounds;
        let [width, height] = extent;
        let mut output = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
        for y in y0..y1 {
            for x in x0..x1 {
                let row = if flip { height - 1 - y } else { y };
                let index = pixel_index(x, row, width, tiled) * 4;
                let destination = &target[index..index + 4];
                let coverage_index = (y - y0) * (x1 - x0) + x - x0;
                let covered = coverage[coverage_index];
                let in_scissor = scissors.is_empty()
                    || scissors.iter().any(|rect| {
                        x as u32 >= rect[0]
                            && (x as u32) < rect[2]
                            && y as u32 >= rect[1]
                            && (y as u32) < rect[3]
                    });
                if covered == 0 || !in_scissor {
                    output.extend_from_slice(destination);
                    continue;
                }
                let source = if let Some(texture) = texture {
                    texture.sample(x, y).unwrap()
                } else if let Some(gradient) = gradient {
                    gradient.sample(x, y)
                } else {
                    solid
                };
                let source = packed(source, regs, gamma, order).unwrap();
                let pixel = blend_pixel(
                    source,
                    destination,
                    order,
                    f64::from(covered) / f64::from(samples * samples),
                    blend,
                );
                output.extend_from_slice(&pixel);
            }
        }
        output
    }

    #[allow(clippy::too_many_arguments)]
    fn compare(
        coverage: &[u32],
        bounds: [usize; 4],
        extent: [usize; 2],
        target: &[u8],
        samples: u32,
        regs: &Bank,
        gamma: &[u8],
        order: u32,
        tiled: bool,
        flip: bool,
        scissors: &[[u32; 4]],
        solid: [u8; 4],
        blend: u32,
        texture: Option<&Texture>,
        gradient: Option<&Gradient>,
        gradient_deltas: &[u32],
    ) {
        let expected = cpu_paint(
            coverage, bounds, extent, target, samples, regs, gamma, order, tiled, flip, scissors,
            solid, blend, texture, gradient,
        );
        let actual = paint(
            coverage,
            bounds,
            extent,
            target,
            samples,
            regs,
            gamma,
            order,
            tiled,
            flip,
            scissors,
            solid,
            blend,
            texture,
            gradient,
            gradient_deltas,
        )
        .unwrap_or_else(|error| panic!("GPU paint failed: {error}"));
        let mismatch = actual
            .iter()
            .zip(&expected)
            .position(|(actual, expected)| actual != expected);
        if let Some(byte) = mismatch {
            let pixel = byte / 4;
            let x = bounds[0] + pixel % (bounds[1] - bounds[0]);
            let y = bounds[2] + pixel / (bounds[1] - bounds[0]);
            let row = if flip { extent[1] - 1 - y } else { y };
            let offset = pixel_index(x, row, extent[0], tiled) * 4;
            let source = if let Some(texture) = texture {
                texture.sample(x, y).unwrap()
            } else if let Some(gradient) = gradient {
                gradient.sample(x, y)
            } else {
                solid
            };
            let source = packed(source, regs, gamma, order).unwrap();
            eprintln!(
                "paint mismatch pixel=({x},{y}) channel={} coverage={} source={source:?} texture={:?} destination={:?} gpu={:?} cpu={:?} blend={blend} order={order:#x}",
                byte % 4,
                coverage[pixel],
                texture.map(|texture| (
                    texture.format,
                    texture.swizzle,
                    texture.rotate,
                    texture.premultiplied
                )),
                &target[offset..offset + 4],
                &actual[pixel * 4..pixel * 4 + 4],
                &expected[pixel * 4..pixel * 4 + 4],
            );
            if let Some(texture) = texture {
                let raw = texture.sample(x, y).unwrap();
                let solid_gpu = paint(
                    coverage,
                    bounds,
                    extent,
                    target,
                    samples,
                    regs,
                    gamma,
                    order,
                    tiled,
                    flip,
                    scissors,
                    raw,
                    blend,
                    None,
                    None,
                    &[],
                )
                .unwrap();
                eprintln!(
                    "texture raw={raw:?} packed={source:?} isolated solid GPU={:?}",
                    &solid_gpu[pixel * 4..pixel * 4 + 4],
                );
            }
        }
        assert!(
            actual == expected,
            "paint mismatch at byte {mismatch:?}, blend={blend}, order={order:#x}, samples={samples}"
        );
    }

    fn rectangle_coverage(bounds: [usize; 4], samples: u32) -> Vec<u32> {
        let contours = vec![vec![[2.0, 2.0], [14.0, 2.0], [14.0, 13.0], [2.0, 13.0]]];
        gpu_coverage(&contours, bounds, samples, false)
    }

    fn image(format: u32, swizzle: u32, rotate: bool, premultiplied: bool) -> Texture {
        let width = 16usize;
        let height = 16usize;
        let bytes_per_pixel = match format {
            7 | 8 => 1,
            2..=4 => 2,
            _ => 4,
        };
        let mut bytes = vec![0; width * height * bytes_per_pixel];
        for y in 0..height {
            for x in 0..width {
                let cell = pixel_index(x, y, width, true);
                match format {
                    7 | 8 => bytes[cell] = (x * 13 + y * 7) as u8,
                    2..=4 => {
                        let value = match format {
                            2 => {
                                (((x * 2) as u16 & 31) << 11)
                                    | (((y * 2) as u16 & 31) << 6)
                                    | (((x + y) as u16 & 31) << 1)
                                    | ((x ^ y) as u16 & 1)
                            }
                            3 => {
                                (((x * 2) as u16 & 31) << 11)
                                    | (((y * 4) as u16 & 63) << 5)
                                    | ((x + y) as u16 & 31)
                            }
                            _ => {
                                (((x * 17) as u16 & 15) << 12)
                                    | (((y * 13) as u16 & 15) << 8)
                                    | ((((x + y) * 7) as u16 & 15) << 4)
                                    | ((x ^ y) as u16 & 15)
                            }
                        };
                        bytes[cell * 2..cell * 2 + 2].copy_from_slice(&value.to_le_bytes());
                    }
                    _ => {
                        let offset = cell * 4;
                        bytes[offset..offset + 4].copy_from_slice(&[
                            (x * 13) as u8,
                            (y * 11) as u8,
                            (x * 3 + y * 5) as u8,
                            64 + ((x * 7 + y * 9) % 192) as u8,
                        ]);
                    }
                }
            }
        }
        Texture {
            width,
            height,
            bytes,
            matrix: [1.0 / 16.0, 0.0, 0.0, 0.0, 1.0 / 16.0, 0.0, 0.0, 0.0, 1.0],
            valid: [width, height],
            format,
            premultiplied,
            swizzle,
            rotate,
            opaque: format == 9,
        }
    }

    fn gradient(radial: bool) -> Gradient {
        let colors = (0..128)
            .map(|index| {
                u32::from_le_bytes([
                    (index * 2) as u8,
                    (255 - index) as u8,
                    (index * 5) as u8,
                    64 + (index % 192) as u8,
                ])
            })
            .collect();
        Gradient {
            matrix: [1.0 / 16.0, 0.0, 0.0, 0.0, 1.0 / 16.0, 0.0, 0.0, 0.0, 1.0],
            colors,
            spread: 1,
            premultiplied: true,
            active: true,
            radial: radial.then_some((0x10400000, [0.5, 0.25], 0.5)),
        }
    }

    #[test]
    fn existing_solid_and_image_path_draws_match_cpu() {
        if !require_gpu() {
            return;
        }
        let extent = [16, 16];
        let bounds = [0, 16, 0, 16];
        let target = vec![0; 16 * 16 * 4];
        let gamma = (0..=255).map(|value| value as u8).collect::<Vec<_>>();
        let regs = regs(false, false);
        let coverage = rectangle_coverage(bounds, 1);
        compare(
            &coverage,
            bounds,
            extent,
            &target,
            1,
            &regs,
            &gamma,
            0x101,
            true,
            false,
            &[],
            [12, 34, 56, 255],
            0,
            None,
            None,
            &[],
        );

        let texture = image(0, 0, false, false);
        let mut source = vec![0; 16 * 16 * 4];
        for y in 0..16 {
            for x in 0..16 {
                let index = pixel_index(x, y, 16, true) * 4;
                source[index..index + 4].copy_from_slice(&[255, 0, (y * 16) as u8, (x * 16) as u8]);
            }
        }
        let fixture = Texture {
            bytes: source,
            ..texture
        };
        compare(
            &coverage,
            bounds,
            extent,
            &target,
            1,
            &regs,
            &gamma,
            0x101,
            true,
            false,
            &[],
            [0; 4],
            0,
            Some(&fixture),
            None,
            &[],
        );
    }

    #[test]
    fn every_supported_image_format_matches_cpu() {
        if !require_gpu() {
            return;
        }
        let extent = [16, 16];
        let bounds = [1, 15, 1, 15];
        let coverage = rectangle_coverage(bounds, 4);
        let gamma = gamma();
        let target = target(0x12345678, 16, 16);
        let regs = regs(true, true);
        let scissors = [[2, 3, 14, 13]];
        for format in [0, 2, 3, 4, 7, 8, 9] {
            let swizzle = if format == 2 || format == 4 {
                0x93
            } else {
                0x1b
            };
            let texture = image(format, swizzle, format == 4, format == 0 || format == 9);
            compare(
                &coverage,
                bounds,
                extent,
                &target,
                4,
                &regs,
                &gamma,
                0x101,
                true,
                true,
                &scissors,
                [0; 4],
                6,
                Some(&texture),
                None,
                &[],
            );
        }
    }

    #[test]
    fn linear_and_radial_gradients_match_cpu() {
        if !require_gpu() {
            return;
        }
        let extent = [16, 16];
        let bounds = [0, 16, 0, 16];
        let coverage = rectangle_coverage(bounds, 2);
        let target = target(0x9e3779b9, 16, 16);
        let gamma = gamma();
        let regs = regs(true, true);
        let deltas: Vec<u32> = (0u32..128)
            .map(|index| index.wrapping_mul(0x01010101))
            .collect();
        for radial in [false, true] {
            let gradient = gradient(radial);
            compare(
                &coverage,
                bounds,
                extent,
                &target,
                2,
                &regs,
                &gamma,
                0x101,
                false,
                false,
                &[[3, 2, 15, 14]],
                [0; 4],
                5,
                None,
                Some(&gradient),
                &deltas,
            );
        }
    }

    #[test]
    fn randomized_paints_match_cpu_byte_for_byte() {
        if !require_gpu() {
            return;
        }
        let extent = [16, 16];
        let bounds = [1, 15, 1, 15];
        let contours = vec![vec![
            [1.25, 2.125],
            [14.75, 1.5],
            [13.125, 14.5],
            [3.5, 12.75],
        ]];
        let coverage = gpu_coverage(&contours, bounds, 4, true);
        let gamma = gamma();
        let scissors = [[2, 1, 15, 14], [6, 4, 12, 15]];
        let mut seed = 0x7f4a7c15u32;
        for iteration in 0..96 {
            let mut next = || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed
            };
            let mut color = [0; 4];
            for channel in &mut color {
                *channel = next() as u8;
            }
            let regs = regs(iteration & 1 != 0, iteration & 2 != 0);
            let order = if iteration & 4 != 0 { 0x101 } else { 1 };
            let blend = iteration % 10;
            let target = target(next(), 16, 16);
            let (texture, gradient, deltas) = match iteration % 3 {
                1 => {
                    let formats = [0, 2, 3, 4, 7, 8, 9];
                    let format = formats[(iteration as usize / 3) % formats.len()];
                    let swizzles = [0, 0x1b, 0x93];
                    (
                        Some(image(
                            format,
                            swizzles[next() as usize % swizzles.len()],
                            next() & 1 != 0,
                            next() & 1 != 0,
                        )),
                        None,
                        Vec::new(),
                    )
                }
                2 => {
                    let mut ramp = gradient(iteration & 8 != 0);
                    ramp.spread = next() % 3;
                    ramp.premultiplied = next() & 1 != 0;
                    for color in &mut ramp.colors {
                        *color = u32::from_le_bytes([
                            next() as u8,
                            next() as u8,
                            next() as u8,
                            next() as u8,
                        ]);
                    }
                    let deltas = (0..128).map(|_| next()).collect::<Vec<_>>();
                    (None, Some(ramp), deltas)
                }
                _ => (None, None, Vec::new()),
            };
            compare(
                &coverage,
                bounds,
                extent,
                &target,
                4,
                &regs,
                &gamma,
                order,
                iteration & 8 != 0,
                iteration & 16 != 0,
                if iteration & 32 != 0 { &scissors } else { &[] },
                color,
                blend,
                texture.as_ref(),
                gradient.as_ref(),
                &deltas,
            );
        }
    }
}
