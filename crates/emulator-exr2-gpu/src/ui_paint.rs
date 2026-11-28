//! GPU paint and compositing for EXR2 vector UI batches.

use crate::{Inputs, RenderedBatch, batch};
use fujimod_emulator_devices::record::{Result, Unsupported};
use fujimod_emulator_gpu::{SurfaceExtent, SurfaceFormat, SurfaceKey, SurfaceLayout};
use fujimod_emulator_graphics::render;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use wgpu::util::DeviceExt;

use std::task::Poll;

#[cfg(not(target_arch = "wasm32"))]
use std::task::{Context, Wake};

struct PaintKernel {
    pipeline: wgpu::ComputePipeline,
}

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Unsupported(message.into()))
}

pub(super) fn prepare() -> Result<()> {
    kernel().map(|_| ())
}

fn kernel() -> Result<&'static PaintKernel> {
    static KERNEL: OnceLock<std::result::Result<PaintKernel, String>> = OnceLock::new();
    match KERNEL.get_or_init(|| {
        let context = fujimod_emulator_gpu::context()?;
        if !context.supports_read_write_r32uint_storage() {
            return Err("host GPU lacks native R32Uint read/write storage support".into());
        }
        let device = context.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("EXR2 vector GPU paint"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ui_paint.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("EXR2 vector GPU paint"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(PaintKernel { pipeline })
    }) {
        Ok(kernel) => Ok(kernel),
        Err(error) => fail(format!("Graphics UI GPU paint {error}")),
    }
}

fn image_key(texture: &batch::Texture) -> (u64, u32, u32) {
    (texture.source, texture.width, texture.height)
}

fn push_f32(words: &mut Vec<u32>, value: f64) -> Result<()> {
    let narrowed = value as f32;
    if !narrowed.is_finite() {
        return fail("Graphics UI paint coefficient exceeds the GPU float range");
    }
    words.push(narrowed.to_bits());
    Ok(())
}

fn paint_parameters(
    draw: &batch::Draw,
    plan: &render::CoveragePlan,
    raster_offset: u32,
) -> Result<Vec<u32>> {
    let (target_width, target_height, order) = (draw.target.1, draw.target.2, draw.target.3);
    let mode = if let Some(gradient) = &draw.gradient {
        if gradient.radial.is_some() { 3 } else { 2 }
    } else if draw.texture.is_some() {
        1
    } else {
        0
    };
    let texture_dimensions = draw
        .texture
        .as_ref()
        .map(|texture| [texture.width, texture.height])
        .unwrap_or([0; 2]);
    let valid_dimensions = draw
        .texture
        .as_ref()
        .map(|texture| [texture.valid_width, texture.valid_height])
        .unwrap_or([0; 2]);
    let mut words = Vec::with_capacity(41);
    words.extend([
        plan.x0 as u32,
        plan.y0 as u32,
        plan.width as u32,
        plan.height as u32,
        target_width,
        target_height,
        raster_offset,
        plan.samples * plan.samples,
        order,
        u32::from(draw.blend != 0),
        mode,
        texture_dimensions[0],
        texture_dimensions[1],
        valid_dimensions[0] as u32,
        valid_dimensions[1] as u32,
    ]);
    if let Some(texture) = &draw.texture {
        for value in texture.matrix {
            push_f32(&mut words, value)?;
        }
    } else {
        words.extend([0; 6]);
    }
    if let Some(gradient) = &draw.gradient {
        let (a, c, e, f) = gradient.coefficients;
        for value in [a, c, e, f] {
            push_f32(&mut words, value)?;
        }
        words.extend([gradient.spread, u32::from(gradient.premultiplied)]);
    } else {
        words.extend([0; 6]);
    }
    if let Some(radial) = draw
        .gradient
        .as_ref()
        .and_then(|gradient| gradient.radial.as_ref())
    {
        push_f32(&mut words, radial.quadratics[0])?;
        push_f32(&mut words, radial.quadratics[1])?;
        words.push(
            u32::from(radial.zero_x)
                | (u32::from(radial.zero_y) << 1)
                | (u32::from(radial.negative_denominator) << 2),
        );
    } else {
        words.extend([0; 3]);
    }
    words.push(u32::from(draw.color_transform.is_some()));
    if let Some(transform) = &draw.color_transform {
        for value in transform.scale.into_iter().chain(transform.bias) {
            push_f32(&mut words, value)?;
        }
    } else {
        words.extend([0; 8]);
    }
    words.push(u32::from_le_bytes(draw.color));
    words.push(draw.target_format as u32);
    if words.len() != 41 {
        return fail("Graphics UI GPU paint parameters have an invalid extent");
    }
    Ok(words)
}

fn paint_data(draw: &batch::Draw, image: Option<&[u8]>) -> Result<Vec<u32>> {
    if let Some(gradient) = &draw.gradient {
        if gradient.colors.len() != 128 {
            return fail("Graphics UI gradient paint does not contain 128 RGBA entries");
        }
        return Ok(gradient
            .colors
            .iter()
            .map(|rgba| {
                u32::from(rgba[0])
                    | (u32::from(rgba[1]) << 8)
                    | (u32::from(rgba[2]) << 16)
                    | (u32::from(rgba[3]) << 24)
            })
            .collect());
    }
    if let Some(texture) = &draw.texture {
        let bytes =
            image.ok_or_else(|| Unsupported("Graphics UI texture snapshot is missing".into()))?;
        let expected = texture.width as usize * texture.height as usize * 4;
        if bytes.len() != expected {
            return fail("Graphics UI texture paint snapshot is incomplete");
        }
        let words = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| u32::from_le_bytes(*pixel))
            .collect();
        return Ok(words);
    }
    Ok(vec![0])
}

/// Paint a complete EXR2 vector batch, keeping coverage and pixels on the GPU.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn render(
    draws: &[batch::Draw],
    cpu: i64,
    host: &mut dyn Inputs,
) -> Result<RenderedBatch> {
    block_on(render_async(draws, cpu, host))
}

/// Paint a complete EXR2 vector batch without blocking WebGPU readback.
// The submission guard is held across the readback awaits on purpose: it keeps
// GPU paints serialized until their results are mapped back.
#[allow(clippy::await_holding_lock)]
pub(super) async fn render_async(
    draws: &[batch::Draw],
    cpu: i64,
    host: &mut dyn Inputs,
) -> Result<RenderedBatch> {
    let mut target_bytes = Vec::<(SurfaceKey, Vec<u8>)>::new();
    let mut target_slots = HashMap::<u64, usize>::new();
    let mut texture_bytes = HashMap::<(u64, u32, u32), Vec<u8>>::new();
    for draw in draws {
        let (address, width, height, _) = draw.target;
        if let std::collections::hash_map::Entry::Vacant(entry) = target_slots.entry(address) {
            let length = (width as u64)
                .checked_mul(height as u64)
                .and_then(|count| count.checked_mul(draw.target_format.bytes_per_pixel()))
                .ok_or_else(|| Unsupported("Graphics UI target size overflow".into()))?;
            let bytes = host.read(cpu, address, length).map_err(Unsupported)?;
            if bytes.len() as u64 != length {
                return fail("Graphics UI destination snapshot is incomplete");
            }
            let key = SurfaceKey {
                guest_address: address,
                format: match draw.target_format {
                    batch::TargetFormat::Rgba8 => SurfaceFormat::Rgba8,
                    batch::TargetFormat::Rgba4444 => SurfaceFormat::Rgba4,
                },
                extent: SurfaceExtent { width, height },
                layout: SurfaceLayout::Morton8x8,
            };
            entry.insert(target_bytes.len());
            target_bytes.push((key, bytes));
        }
        if let Some(texture) = &draw.texture {
            let key = image_key(texture);
            if let std::collections::hash_map::Entry::Vacant(entry) = texture_bytes.entry(key) {
                let length = (texture.width as u64)
                    .checked_mul(texture.height as u64)
                    .and_then(|count| count.checked_mul(4))
                    .ok_or_else(|| Unsupported("Graphics UI texture size overflow".into()))?;
                let bytes = host
                    .read(cpu, texture.source, length)
                    .map_err(Unsupported)?;
                if bytes.len() as u64 != length {
                    return fail("Graphics UI texture snapshot is incomplete");
                }
                entry.insert(bytes);
            }
        }
    }

    let context = fujimod_emulator_gpu::context()
        .map_err(|error| Unsupported(format!("Graphics UI GPU paint {error}")))?;
    let _submission = context
        .acquire()
        .map_err(|error| Unsupported(format!("Graphics UI GPU paint {error}")))?;
    for (key, bytes) in &target_bytes {
        context
            .get_or_create_target(*key, Some(bytes))
            .map_err(|error| Unsupported(format!("Graphics UI GPU paint {error}")))?;
    }

    let plans = render::coverage_plans(draws)?;
    let device = context.device();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("EXR2 vector GPU paint"),
    });
    // Coverage controls both the pixels and the guest-visible completion time.
    // The reference scanline rasterizer produces the same counts for each run.
    let mut counts = Vec::<u32>::new();
    let mut rasters = Vec::with_capacity(plans.len());
    for plan in &plans {
        if !matches!(plan.samples, 1 | 2 | 4) {
            return fail("Graphics UI uses an unsupported coverage sample grid");
        }
        let bounds = plan.bounds();
        if bounds[1] > 2048 || bounds[3] > 2048 {
            return fail("Graphics UI coverage exceeds the GPU bounds");
        }
        if plan.edges.iter().any(|&(start, end)| {
            [start.0, start.1, end.0, end.1]
                .iter()
                .any(|v| v.abs() > 16_777_216.0)
        }) {
            return fail("Graphics UI fill edge exceeds the GPU coordinate range");
        }
        let length = plan
            .width
            .checked_mul(plan.height)
            .and_then(|n| n.checked_add(counts.len()))
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| Unsupported("Graphics UI coverage size overflow".into()))?;
        let limits = device.limits();
        if length > limits.max_storage_buffer_binding_size as usize
            || length as u64 > limits.max_buffer_size
        {
            return fail("Graphics UI coverage exceeds host GPU limits");
        }
        let offset = u32::try_from(counts.len())
            .map_err(|_| Unsupported("Graphics UI coverage offset overflow".into()))?;
        let values = if plan.is_empty() {
            Vec::new()
        } else {
            render::fill_counts(
                &plan.edges,
                plan.nonzero,
                plan.x0,
                plan.y0,
                i64::from(plan.samples),
                plan.width,
                plan.height,
            )?
        };
        rasters.push((offset, values.len()));
        counts.extend(values.into_iter().map(u32::from));
    }
    let coverage_bytes = words_as_bytes(&counts);
    let coverage_buffer = (!counts.is_empty()).then(|| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("EXR2 vector coverage"),
            contents: &coverage_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        })
    });
    let surfaces: Vec<_> = target_bytes
        .iter()
        .map(|(key, _)| {
            context
                .get(*key)
                .map_err(|error| Unsupported(format!("Graphics UI GPU paint {error}")))?
                .ok_or_else(|| Unsupported("Graphics UI GPU target is not resident".into()))
        })
        .collect::<Result<_>>()?;
    let touched = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("EXR2 vector GPU touched-halfword count"),
        contents: &0u32.to_le_bytes(),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    });
    let touched_readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("EXR2 vector GPU touched-halfword readback"),
        size: 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    if let Some(coverage_buffer) = &coverage_buffer {
        let paint = kernel()?;
        for ((draw, plan), raster) in draws.iter().zip(&plans).zip(&rasters) {
            if plan.is_empty() || raster.1 == 0 {
                continue;
            }
            let target_index = *target_slots
                .get(&draw.target.0)
                .ok_or_else(|| Unsupported("Graphics UI target lookup failed".into()))?;
            let surface = &surfaces[target_index];
            let image = draw
                .texture
                .as_ref()
                .and_then(|texture| texture_bytes.get(&image_key(texture)))
                .map(Vec::as_slice);
            let params = paint_parameters(draw, plan, raster.0)?;
            let params_bytes = words_as_bytes(&params);
            let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("EXR2 vector GPU paint parameters"),
                contents: &params_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
            let clip_values: Vec<u32> = plan
                .clip_mask()
                .iter()
                .map(|&inside| u32::from(inside))
                .collect();
            let clip_bytes = words_as_bytes(&clip_values);
            let clip_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("EXR2 vector GPU clip mask"),
                contents: &clip_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
            let data = paint_data(draw, image)?;
            let data_bytes = words_as_bytes(&data);
            let data_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("EXR2 vector GPU paint data"),
                contents: &data_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
            let source_view = surface.texture().create_view(&Default::default());
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("EXR2 vector GPU paint bindings"),
                layout: &paint.pipeline.get_bind_group_layout(0),
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
                        resource: clip_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: data_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: touched.as_entire_binding(),
                    },
                ],
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("EXR2 vector GPU paint"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&paint.pipeline);
                pass.set_bind_group(0, &bindings, &[]);
                pass.dispatch_workgroups(
                    (plan.width as u32).div_ceil(8),
                    (plan.height as u32).div_ceil(8),
                    1,
                );
            }
        }
    }
    encoder.copy_buffer_to_buffer(&touched, 0, &touched_readback, 0, 4);
    let pending = context
        .encode_readbacks(
            &mut encoder,
            &target_bytes.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
        )
        .map_err(|error| Unsupported(format!("Graphics UI GPU readback {error}")))?;
    context.submit(encoder.finish());

    let readbacks = context
        .finish_readbacks_async(pending)
        .await
        .map_err(|error| Unsupported(format!("Graphics UI GPU readback {error}")))?;
    let touched_halfwords = map_buffer_async(&touched_readback, device).await?;
    let touched_halfwords = u32::from_le_bytes(touched_halfwords[..4].try_into().unwrap()) as u64;

    let mut output_by_key = HashMap::new();
    for (key, readback) in readbacks {
        output_by_key.insert(key, Arc::new(readback.bytes));
    }
    let output = target_bytes
        .iter()
        .map(|(key, _)| {
            output_by_key
                .remove(key)
                .map(|bytes| (key.guest_address, bytes))
                .ok_or_else(|| Unsupported("Graphics UI GPU readback omitted a target".into()))
        })
        .collect::<Result<_>>()?;
    Ok((output, touched_halfwords * 2))
}

async fn map_buffer_async(buffer: &wgpu::Buffer, _device: &wgpu::Device) -> Result<Vec<u8>> {
    struct MappingState {
        result: Option<std::result::Result<(), String>>,
        waker: Option<std::task::Waker>,
    }

    let state = Arc::new(std::sync::Mutex::new(MappingState {
        result: None,
        waker: None,
    }));
    let callback_state = Arc::clone(&state);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let wake = callback_state.lock().ok().and_then(|mut state| {
                state.result = Some(result.map_err(|error| error.to_string()));
                state.waker.take()
            });
            if let Some(waker) = wake {
                waker.wake();
            }
        });

    #[cfg(not(target_arch = "wasm32"))]
    _device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| Unsupported(format!("Graphics UI GPU touched-halfword poll: {error}")))?;

    std::future::poll_fn(|context| {
        let Ok(mut state) = state.lock() else {
            return Poll::Ready(Err(Unsupported(
                "Graphics UI GPU touched-halfword mapping state lock poisoned".into(),
            )));
        };
        if let Some(result) = state.result.take() {
            Poll::Ready(result.map_err(|error| {
                Unsupported(format!("Graphics UI GPU touched-halfword map: {error}"))
            }))
        } else {
            state.waker = Some(context.waker().clone());
            Poll::Pending
        }
    })
    .await?;

    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|error| Unsupported(format!("Graphics UI GPU touched-halfword range: {error}")))?;
    let bytes = mapped.to_vec();
    drop(mapped);
    buffer.unmap();
    Ok(bytes)
}

#[cfg(not(target_arch = "wasm32"))]
struct ThreadWake(std::thread::Thread);

#[cfg(not(target_arch = "wasm32"))]
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let waker = Arc::new(ThreadWake(std::thread::current())).into();
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn words_as_bytes(words: &[u32]) -> Vec<u8> {
    // A sized copy vectorizes; collecting a flattened iterator went byte by
    // byte through every pixel of a clip mask.
    let mut bytes = vec![0; words.len() * 4];
    for (chunk, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Inputs;
    use fujimod_emulator_graphics::batch::{ColorTransform, Gradient, Texture};
    use std::rc::Rc;

    struct TestInputs {
        regions: HashMap<u64, Vec<u8>>,
    }

    impl Inputs for TestInputs {
        fn has_reader(&self) -> bool {
            true
        }

        fn has_irq(&self) -> bool {
            true
        }

        fn routed(&self) -> bool {
            true
        }

        fn read(
            &mut self,
            _cpu: i64,
            address: u64,
            length: u64,
        ) -> std::result::Result<Vec<u8>, String> {
            self.regions
                .get(&address)
                .filter(|bytes| bytes.len() as u64 == length)
                .cloned()
                .ok_or_else(|| format!("unexpected test read at {address:#x} for {length} bytes"))
        }

        fn targets(&mut self) -> Result<(usize, i64)> {
            Ok((0, 0))
        }
    }

    fn path(bounds: [f64; 4]) -> Vec<fujimod_emulator_graphics::stream::Geometry> {
        let [left, top, right, bottom] = bounds;
        vec![
            (0x28, 0, vec![left, top]),
            (0x29, 0, vec![right, top]),
            (0x29, 0, vec![right, bottom]),
            (0x29, 0, vec![left, bottom]),
            (0x29, 0, vec![left, top]),
        ]
    }

    fn draw(
        address: u64,
        order: u32,
        bounds: [f64; 4],
        samples: u32,
        blend: u32,
        color: [u8; 4],
    ) -> batch::Draw {
        batch::Draw {
            target: (address, 8, 8, order),
            target_format: batch::TargetFormat::Rgba8,
            sequence: 0,
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            geometry: path(bounds),
            samples,
            clips: vec![(0, 0, 8, 8)],
            nonzero: false,
            blend,
            color,
            stroke: false,
            texture: None,
            gradient: None,
            color_transform: None,
            radius: 0.0,
            cap: 0,
            join: 0,
            device_radius: false,
        }
    }

    fn cpu_coverage(draw: &batch::Draw) -> Result<Rc<render::Coverage>> {
        render::coverage(draw).map(Rc::new)
    }

    fn compare_with_cpu(order: u32, format: batch::TargetFormat) {
        let context = match fujimod_emulator_gpu::context() {
            Ok(context) => context,
            Err(error) if error.starts_with("host GPU adapter:") => {
                eprintln!("skipping EXR2 GPU paint test without an adapter: {error}");
                return;
            }
            Err(error) => panic!("EXR2 GPU paint context failed: {error}"),
        };
        assert!(
            context.supports_read_write_r32uint_storage(),
            "the EXR2 paint path requires in-place R32Uint storage support"
        );

        let target_address = 0x175000 + u64::from(order) * 0x1000 + (format as u64) * 0x10000;
        let texture_address = target_address + 0x1000;
        let target: Vec<u8> = (0..8 * 8 * format.bytes_per_pixel())
            .map(|index| (index * 37 + 11) as u8)
            .collect();
        let texture_bytes: Vec<u8> = (0..8 * 8 * 4).map(|index| (index * 19 + 7) as u8).collect();
        let mut draws = vec![
            draw(
                target_address,
                order,
                [0.0, 0.0, 8.0, 8.0],
                1,
                0,
                [19, 79, 137, 211],
            ),
            draw(
                target_address,
                order,
                [1.25, 0.7, 6.6, 6.1],
                4,
                1,
                [223, 57, 91, 149],
            ),
        ];
        let mut gradient_draw = draw(
            target_address,
            order,
            [0.4, 1.2, 7.6, 7.1],
            2,
            1,
            [0, 0, 0, 0],
        );
        gradient_draw.gradient = Some(Gradient {
            coefficients: (0.047, 0.11, 0.029, 0.17),
            radial: None,
            colors: (0..128)
                .map(|index| {
                    [
                        (index * 17 % 256) as u8,
                        (index * 41 % 256) as u8,
                        (255 - index * 13 % 256) as u8,
                        (64 + index * 7 % 192) as u8,
                    ]
                })
                .collect(),
            spread: 1,
            premultiplied: false,
        });
        gradient_draw.color_transform = Some(ColorTransform {
            scale: [0.875, 1.125, 0.75, 1.0],
            bias: [0.03125, 0.0, 0.015625, -0.015625],
        });
        let mut texture_draw = draw(
            target_address,
            order,
            [2.1, 0.3, 7.4, 5.8],
            4,
            0,
            [0, 0, 0, 0],
        );
        texture_draw.texture = Some(Texture {
            source: texture_address,
            width: 8,
            height: 8,
            valid_width: 7,
            valid_height: 6,
            matrix: [1.0, 0.0, 0.25, 0.0, 1.0, -0.5],
        });
        draws.extend([gradient_draw, texture_draw]);
        for draw in &mut draws {
            draw.target_format = format;
        }

        let mut host = TestInputs {
            regions: HashMap::from([
                (target_address, target.clone()),
                (texture_address, texture_bytes.clone()),
            ]),
        };
        let actual = render(&draws, 0, &mut host).expect("GPU paint should render");
        let repeated = render(&draws, 0, &mut host).expect("repeated GPU paint should render");
        assert_eq!(actual, repeated);

        let targets = HashMap::from([(target_address, target)]);
        let textures = HashMap::from([((texture_address, 8, 8), texture_bytes)]);
        let mut coverage = cpu_coverage;
        let (expected, expected_touched) =
            super::render::render(&draws, &targets, &textures, &mut coverage)
                .expect("CPU paint reference should render");
        assert_eq!(actual.1, expected_touched);
        assert_eq!(actual.0[0].0, expected[0].0);
        if actual.0[0].1.as_slice() != expected[0].1.as_slice() {
            let differences: Vec<_> = actual.0[0]
                .1
                .as_chunks::<4>()
                .0
                .iter()
                .zip(expected[0].1.as_chunks::<4>().0)
                .enumerate()
                .filter(|(_, (actual, expected))| actual != expected)
                .take(8)
                .map(|(index, (actual, expected))| (index, actual.to_vec(), expected.to_vec()))
                .collect();
            panic!("paint output differed for order {order}: {differences:?}");
        }
    }

    #[test]
    fn repeated_coverage_matches_gpu_reference_and_covered_bytes() {
        use fujimod_emulator_gpu::{
            CoverageEdge, CoverageKernel, CoverageRequest, EdgeCoordinates, FillRule, SampleGrid,
        };

        let context = match fujimod_emulator_gpu::context() {
            Ok(context) => context,
            Err(error) if error.starts_with("host GPU adapter:") => {
                eprintln!("skipping EXR2 GPU paint test without an adapter: {error}");
                return;
            }
            Err(error) => panic!("EXR2 GPU paint context failed: {error}"),
        };
        let shape = draw(
            0x190000,
            0,
            [1.25, 0.75, 6.5, 6.0],
            4,
            0,
            [19, 79, 137, 211],
        );
        let plan = render::coverage_plan(&shape).unwrap();
        let reference = render::fill_counts(
            &plan.edges,
            plan.nonzero,
            plan.x0,
            plan.y0,
            i64::from(plan.samples),
            plan.width,
            plan.height,
        )
        .unwrap();
        let expected: Vec<u32> = reference.iter().copied().map(u32::from).collect();
        let edges: Vec<_> = plan
            .edges
            .iter()
            .map(|&(a, b)| CoverageEdge::from_f64([a.0, a.1], [b.0, b.1]))
            .collect();
        let request = CoverageRequest {
            bounds: plan.bounds(),
            edges: &edges,
            edge_coordinates: EdgeCoordinates::FloatPixel,
            fill_rule: FillRule::EvenOdd,
            sample_grid: SampleGrid::FourByFour,
            stroke_radius: None,
        };
        let kernel = CoverageKernel::new(context);
        for _ in 0..3 {
            let gpu = kernel.coverage_batch(context, &[request]).unwrap();
            assert_eq!(gpu, vec![expected.clone()]);
        }

        // Multiple draws exercise the same covered-byte count used for the
        // vector device's guest-visible completion deadline.
        let draws = vec![shape; 32];
        let mut host = TestInputs {
            regions: HashMap::from([(0x190000, vec![0; 8 * 8 * 4])]),
        };
        let covered_pixels = reference
            .iter()
            .zip(plan.clip_mask())
            .filter(|(count, inside)| **count != 0 && **inside)
            .count() as u64;
        assert!(covered_pixels > 0);
        let expected_bytes = covered_pixels * 4 * draws.len() as u64;
        let first = render(&draws, 0, &mut host).unwrap();
        assert_eq!(first.1, expected_bytes);
        for _ in 0..2 {
            let repeated = render(&draws, 0, &mut host).unwrap();
            assert_eq!(repeated.1, expected_bytes);
            assert_eq!(repeated, first);
        }
    }

    #[test]
    fn rgba8_src_src_over_texture_and_linear_ramp_match_cpu_reference() {
        compare_with_cpu(0, batch::TargetFormat::Rgba8);
        compare_with_cpu(1, batch::TargetFormat::Rgba8);
    }
    #[test]
    fn rgba4444_src_src_over_texture_and_linear_ramp_match_cpu_reference() {
        compare_with_cpu(0, batch::TargetFormat::Rgba4444);
        compare_with_cpu(1, batch::TargetFormat::Rgba4444);
    }

    #[test]
    fn packed_target_shader_and_parameters_validate() {
        let module = wgpu::naga::front::wgsl::parse_str(include_str!("ui_paint.wgsl")).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        let mut draw = draw(0x123400, 0, [0., 0., 8., 8.], 1, 0, [1, 2, 3, 4]);
        for format in [batch::TargetFormat::Rgba8, batch::TargetFormat::Rgba4444] {
            draw.target_format = format;
            let plan = render::coverage_plan(&draw).unwrap();
            let params = paint_parameters(&draw, &plan, 0).unwrap();
            assert_eq!(params.len(), 41);
            assert_eq!(params[40], format as u32);
        }
    }
    #[test]
    fn rgba4444_and_rgba8_targets_share_a_batch_with_exact_byte_counts() {
        let mut packed = draw(
            0x1c0000,
            0,
            [0., 0., 8., 8.],
            1,
            0,
            [0x12, 0x34, 0x56, 0x78],
        );
        packed.target_format = batch::TargetFormat::Rgba4444;
        let full = draw(
            0x1c1000,
            0,
            [0., 0., 8., 8.],
            1,
            0,
            [0x12, 0x34, 0x56, 0x78],
        );
        let mut host = TestInputs {
            regions: HashMap::from([(0x1c0000, vec![0xac; 128]), (0x1c1000, vec![0xac; 256])]),
        };
        let (output, touched) = render(&[packed, full], 0, &mut host).unwrap();
        assert_eq!(touched, 128 + 256);
        assert_eq!(output[0].0, 0x1c0000);
        assert_eq!(output[0].1.as_slice(), [0x57, 0x13].repeat(64));
        assert_eq!(output[1].0, 0x1c1000);
        assert_eq!(output[1].1.as_slice(), [0x78, 0x56, 0x34, 0x12].repeat(64));
    }
}
