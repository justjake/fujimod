//! Optional host render path for one DMP command list.

use super::{
    Reader, Snapshots,
    draw_state::{ColorTargetFormat, CullMode, DepthTargetFormat, DrawState},
    gpu_vertex,
};
use fujimod_emulator_gpu::{
    GpuContext, GpuSurface, SurfaceExtent, SurfaceFormat, SurfaceKey, SurfaceLayout, context,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};
use wgpu::util::DeviceExt;

const REGISTER_WORDS: usize = 0x500;
const FOG_OFFSET: usize = REGISTER_WORDS;
const FOG_WORDS: usize = 128;
const INPUT_OFFSET: usize = FOG_OFFSET + FOG_WORDS;
const CASE_WORDS: usize = INPUT_OFFSET + 16;

static DRAW_PIPELINES: OnceLock<Mutex<HashMap<[u8; 32], Arc<wgpu::RenderPipeline>>>> =
    OnceLock::new();
static VERTEX_SHADER_MODULES: OnceLock<Mutex<HashMap<[u8; 32], Arc<wgpu::ShaderModule>>>> =
    OnceLock::new();
static FRAGMENT_SHADER_MODULES: OnceLock<Mutex<HashMap<bool, Arc<wgpu::ShaderModule>>>> =
    OnceLock::new();
static VERTEX_MODULE_CREATIONS: AtomicUsize = AtomicUsize::new(0);
static FRAGMENT_MODULE_CREATIONS: AtomicUsize = AtomicUsize::new(0);
static PIPELINE_CREATIONS: AtomicUsize = AtomicUsize::new(0);

pub(super) fn shader_cache_creation_counts() -> (usize, usize, usize) {
    (
        VERTEX_MODULE_CREATIONS.load(Ordering::Relaxed),
        FRAGMENT_MODULE_CREATIONS.load(Ordering::Relaxed),
        PIPELINE_CREATIONS.load(Ordering::Relaxed),
    )
}

fn color_surface_format(format: ColorTargetFormat) -> Result<SurfaceFormat, String> {
    match format {
        ColorTargetFormat::Rgba8 => Ok(SurfaceFormat::Rgba8),
        ColorTargetFormat::Rgb565 => Ok(SurfaceFormat::Rgb565),
        ColorTargetFormat::Rgb5A1 => Ok(SurfaceFormat::Rgb5A1),
        ColorTargetFormat::Rgba4 => Ok(SurfaceFormat::Rgba4),
        ColorTargetFormat::Other(value) => {
            Err(format!("DMP unsupported color surface format {value}"))
        }
    }
}

fn depth_surface_format(format: DepthTargetFormat) -> Result<SurfaceFormat, String> {
    match format {
        DepthTargetFormat::Depth16 => Ok(SurfaceFormat::D16),
        DepthTargetFormat::Depth24 => Ok(SurfaceFormat::D24),
        DepthTargetFormat::Depth24Stencil8 => Ok(SurfaceFormat::D24S8),
        DepthTargetFormat::Other(value) => {
            Err(format!("DMP unsupported depth surface format {value}"))
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Draw {
    state: DrawState,
    vertex: gpu_vertex::PreparedDraw,
    registers: Vec<(u16, u32)>,
}

impl Draw {
    pub(super) fn new(
        state: DrawState,
        vertex: gpu_vertex::PreparedDraw,
        registers: &BTreeMap<u16, u32>,
    ) -> Self {
        Self {
            state,
            vertex,
            registers: registers
                .iter()
                .map(|(&index, &value)| (index, value))
                .collect(),
        }
    }
}

pub(super) struct RenderedList {
    pub snapshots: Snapshots,
    pub written_ranges: Vec<(u64, usize)>,
    pub queue_submissions: u8,
    pub draws: usize,
}

struct PreparedDraw {
    state: DrawState,
    extent: SurfaceExtent,
    color_key: SurfaceKey,
    depth_key: SurfaceKey,
    parameters: Vec<u32>,
    guest_bytes: Vec<u8>,
    vertex: gpu_vertex::PreparedDraw,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn render(
    draws: &[Draw],
    recorded_reads: &[(u64, Vec<u8>)],
    expected_snapshots: Option<&Snapshots>,
    read: &mut Reader<'_>,
) -> Result<RenderedList, String> {
    let mut prepared = Vec::new();
    let mut surfaces = BTreeMap::<u64, (SurfaceKey, Option<Vec<u8>>)>::new();
    for draw in draws {
        if draw.vertex.indices.is_empty() {
            continue;
        }
        let state = &draw.state;
        let (width, height) = state.targets.extent.dimensions()?;
        let extent = SurfaceExtent {
            width: width as u32,
            height: height as u32,
        };
        let color_key = SurfaceKey {
            guest_address: state.targets.color_address,
            format: color_surface_format(state.targets.color_format)?,
            extent,
            layout: SurfaceLayout::Morton8x8,
        };
        let depth_key = depth_surface_key(state, extent)?;
        if color_key.guest_address == depth_key.guest_address {
            return Err("DMP host GPU color and depth targets alias".into());
        }
        prepare_surface(&mut surfaces, color_key, true, recorded_reads, read)?;

        let load_depth_from_guest = state.depth_test.enabled || state.stencil_test.enabled;
        prepare_surface(
            &mut surfaces,
            depth_key,
            load_depth_from_guest,
            recorded_reads,
            read,
        )?;

        let (parameters, guest_bytes) = draw_parameters(draw, recorded_reads, read)?;
        prepared.push(PreparedDraw {
            state: state.clone(),
            extent,
            color_key,
            depth_key,
            parameters,
            guest_bytes,
            vertex: draw.vertex.clone(),
        });
    }
    if prepared.is_empty() {
        return Ok(RenderedList {
            snapshots: Vec::new(),
            written_ranges: Vec::new(),
            queue_submissions: 0,
            draws: 0,
        });
    }
    let draw_count = prepared.len();

    let cache = context()?;
    let _submission = cache.acquire()?;
    let mut encoder = cache
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("DMP command list"),
        });
    for (key, bytes) in surfaces.values() {
        cache.get_or_create_target_encoded(&mut encoder, *key, bytes.as_deref())?;
    }
    for (key, _) in surfaces.values() {
        cache.encode_clear_write_masks(&mut encoder, *key)?;
    }
    for prepared in &prepared {
        let pipeline = pipeline_for(cache.device(), &prepared.state, &prepared.vertex)?;
        render_draw(prepared, &pipeline, cache, &mut encoder)?;
    }
    let keys = surfaces.values().map(|(key, _)| *key).collect::<Vec<_>>();
    let pending = cache.encode_readbacks(&mut encoder, &keys)?;
    cache.submit(encoder.finish());
    let readbacks = cache.finish_readbacks(pending)?;

    let mut snapshots = Vec::new();
    let mut written_ranges = Vec::new();
    for (key, readback) in readbacks {
        let address = key.guest_address;
        if expected_snapshots
            .is_some_and(|snapshots| snapshots.iter().any(|(expected, _)| *expected == address))
            || !readback.written_ranges.is_empty()
        {
            snapshots.push((address, readback.bytes));
            written_ranges.extend(readback.written_ranges);
        }
    }
    written_ranges.sort_unstable_by_key(|(address, _)| *address);
    Ok(RenderedList {
        snapshots,
        written_ranges,
        queue_submissions: 1,
        draws: draw_count,
    })
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(super) async fn render_async(
    draws: &[Draw],
    recorded_reads: &[(u64, Vec<u8>)],
    read: &mut Reader<'_>,
) -> Result<RenderedList, String> {
    let mut prepared = Vec::new();
    let mut surfaces = BTreeMap::<u64, (SurfaceKey, Option<Vec<u8>>)>::new();
    for draw in draws {
        if draw.vertex.indices.is_empty() {
            continue;
        }
        let state = &draw.state;
        let (width, height) = state.targets.extent.dimensions()?;
        let extent = SurfaceExtent {
            width: width as u32,
            height: height as u32,
        };
        let color_key = SurfaceKey {
            guest_address: state.targets.color_address,
            format: color_surface_format(state.targets.color_format)?,
            extent,
            layout: SurfaceLayout::Morton8x8,
        };
        let depth_key = depth_surface_key(state, extent)?;
        if color_key.guest_address == depth_key.guest_address {
            return Err("DMP host GPU color and depth targets alias".into());
        }
        prepare_surface(&mut surfaces, color_key, true, recorded_reads, read)?;
        prepare_surface(
            &mut surfaces,
            depth_key,
            state.depth_test.enabled || state.stencil_test.enabled,
            recorded_reads,
            read,
        )?;
        let (parameters, guest_bytes) = draw_parameters(draw, recorded_reads, read)?;
        prepared.push(PreparedDraw {
            state: state.clone(),
            extent,
            color_key,
            depth_key,
            parameters,
            guest_bytes,
            vertex: draw.vertex.clone(),
        });
    }
    if prepared.is_empty() {
        return Ok(RenderedList {
            snapshots: Vec::new(),
            written_ranges: Vec::new(),
            queue_submissions: 0,
            draws: 0,
        });
    }
    let draw_count = prepared.len();

    let cache = context()?;
    let _submission = cache.acquire()?;
    let validation_scope = cache.device().push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = cache
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("DMP command list"),
        });
    for (key, bytes) in surfaces.values() {
        cache.get_or_create_target_encoded(&mut encoder, *key, bytes.as_deref())?;
    }
    for (key, _) in surfaces.values() {
        cache.encode_clear_write_masks(&mut encoder, *key)?;
    }
    for prepared in &prepared {
        let pipeline = pipeline_for(cache.device(), &prepared.state, &prepared.vertex)?;
        render_draw(prepared, &pipeline, cache, &mut encoder)?;
    }
    let keys = surfaces.values().map(|(key, _)| *key).collect::<Vec<_>>();
    let pending = cache.encode_readbacks(&mut encoder, &keys)?;
    cache.submit(encoder.finish());
    let readbacks = cache.finish_readbacks_async(pending).await?;
    if let Some(error) = validation_scope.pop().await {
        return Err(format!("DMP WebGPU validation: {error}"));
    }

    let mut snapshots = Vec::new();
    let mut written_ranges = Vec::new();
    for (key, readback) in readbacks {
        if !readback.written_ranges.is_empty() {
            snapshots.push((key.guest_address, readback.bytes));
            written_ranges.extend(readback.written_ranges);
        }
    }
    written_ranges.sort_unstable_by_key(|(address, _)| *address);
    Ok(RenderedList {
        snapshots,
        written_ranges,
        queue_submissions: 1,
        draws: draw_count,
    })
}

pub(super) fn snapshot_mismatch(actual: &Snapshots, expected: &Snapshots) -> String {
    let count = actual.len().min(expected.len());
    for index in 0..count {
        let (actual_address, actual_bytes) = &actual[index];
        let (expected_address, expected_bytes) = &expected[index];
        if actual_address != expected_address || actual_bytes != expected_bytes {
            let byte = actual_bytes
                .iter()
                .zip(expected_bytes)
                .position(|(actual, expected)| actual != expected);
            return format!(
                "DMP host GPU snapshot differs from CPU at GPU {actual_address:#x}, CPU {expected_address:#x}, byte {byte:?} (lengths {} and {})",
                actual_bytes.len(),
                expected_bytes.len(),
            );
        }
    }
    format!(
        "DMP host GPU returned {} snapshots, CPU returned {}",
        actual.len(),
        expected.len()
    )
}

fn prepare_surface(
    surfaces: &mut BTreeMap<u64, (SurfaceKey, Option<Vec<u8>>)>,
    key: SurfaceKey,
    load_guest_bytes: bool,
    recorded_reads: &[(u64, Vec<u8>)],
    read: &mut Reader<'_>,
) -> Result<(), String> {
    if let Some((existing, _)) = surfaces.get(&key.guest_address) {
        if *existing != key {
            return Err(format!(
                "DMP host GPU target {:#x} changes format or extent within one list",
                key.guest_address
            ));
        }
        return Ok(());
    }
    let byte_len = (key.extent.width as usize)
        .checked_mul(key.extent.height as usize)
        .and_then(|pixels| pixels.checked_mul(key.format.bytes_per_pixel() as usize))
        .ok_or("DMP host GPU surface byte length overflow")?;
    let bytes = if load_guest_bytes {
        let bytes = recorded_range(recorded_reads, key.guest_address, byte_len)
            .map(Ok)
            .unwrap_or_else(|| read(key.guest_address, byte_len))?;
        if bytes.len() != byte_len {
            return Err(format!(
                "DMP host GPU target {:#x} supplied {} bytes, expected {byte_len}",
                key.guest_address,
                bytes.len()
            ));
        }
        Some(bytes)
    } else {
        None
    };
    surfaces.insert(key.guest_address, (key, bytes));
    Ok(())
}

fn depth_surface_key(state: &DrawState, extent: SurfaceExtent) -> Result<SurfaceKey, String> {
    let depth_active = state.depth_test.enabled || state.stencil_test.enabled;
    let guest_address = if depth_active {
        state.targets.depth_address
    } else {
        u64::MAX
            - ((state.targets.extent.packed as u64) << 4)
            - state.targets.depth_format.code() as u64
    };
    Ok(SurfaceKey {
        guest_address,
        format: depth_surface_format(state.targets.depth_format)?,
        extent,
        layout: SurfaceLayout::Morton8x8,
    })
}

fn recorded_range(reads: &[(u64, Vec<u8>)], address: u64, length: usize) -> Option<Vec<u8>> {
    let end = address.checked_add(length as u64)?;
    reads.iter().find_map(|(base, bytes)| {
        let read_end = base.checked_add(bytes.len() as u64)?;
        if address < *base || end > read_end {
            return None;
        }
        let start = usize::try_from(address - base).ok()?;
        Some(bytes[start..start + length].to_vec())
    })
}

fn render_draw(
    draw: &PreparedDraw,
    pipeline: &wgpu::RenderPipeline,
    cache: &GpuContext,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), String> {
    let extent = draw.extent;
    let color_key = draw.color_key;
    let depth_key = draw.depth_key;
    let color_surface = cache
        .get(color_key)?
        .ok_or("DMP color surface is not resident")?;
    let depth_surface = cache
        .get(depth_key)?
        .ok_or("DMP depth surface is not resident")?;

    let device = cache.device();
    let parameter_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("DMP draw parameters"),
        contents: &encode_words(&draw.parameters),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let guest_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("DMP draw textures"),
        contents: &draw.guest_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("DMP vertex uniforms"),
        contents: &draw.vertex.shading_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let input_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("DMP vertex attributes"),
        contents: &draw.vertex.input_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("DMP vertex indices"),
        contents: &encode_words(&draw.vertex.indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let color_input = copy_surface_texture(device, encoder, &color_surface, extent);
    let depth_input = copy_surface_texture(device, encoder, &depth_surface, extent);
    let color_input_view = color_input.create_view(&Default::default());
    let depth_input_view = depth_input.create_view(&Default::default());
    let color_view = color_surface.texture().create_view(&Default::default());
    let depth_view = depth_surface.texture().create_view(&Default::default());
    let color_mask_view = color_surface
        .write_mask_texture()
        .create_view(&Default::default());
    let depth_mask_view = depth_surface
        .write_mask_texture()
        .create_view(&Default::default());
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("DMP render bindings"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: parameter_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: guest_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&color_input_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&depth_input_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: input_buffer.as_entire_binding(),
            },
        ],
    });

    {
        let attachments = [
            Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &depth_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &color_mask_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &depth_mask_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            }),
        ];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("DMP draw"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.set_viewport(
            0.0,
            0.0,
            extent.width as f32,
            extent.height as f32,
            0.0,
            1.0,
        );
        pass.set_scissor_rect(0, 0, extent.width, extent.height);
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        let index_count = u32::try_from(draw.vertex.indices.len())
            .map_err(|_| "DMP host GPU index count exceeds u32")?;
        pass.draw_indexed(0..index_count, 0, 0..1);
    }
    cache.encode_accumulate_draw_mask(encoder, color_key)?;
    cache.encode_accumulate_draw_mask(encoder, depth_key)?;
    Ok(())
}

fn pipeline_for(
    device: &wgpu::Device,
    state: &DrawState,
    vertex: &gpu_vertex::PreparedDraw,
) -> Result<Arc<wgpu::RenderPipeline>, String> {
    let key = vertex.pipeline_key;
    match state.raster.primitive_mode {
        super::draw_state::PrimitiveMode::Triangles
        | super::draw_state::PrimitiveMode::TriangleStrip
        | super::draw_state::PrimitiveMode::TriangleFan => {}
        _ => return Err("DMP host GPU pipeline received a non-triangle mode".into()),
    }
    let pipelines = DRAW_PIPELINES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut pipelines = pipelines
        .lock()
        .map_err(|_| "DMP host GPU render pipeline lock poisoned")?;
    if let Some(pipeline) = pipelines.get(&key) {
        return Ok(Arc::clone(pipeline));
    }

    let vertex_shader = vertex_shader_module(device, vertex)?;
    let fragment_shader = fragment_shader_module(device, state.raster.flat_shading)?;
    let pipeline = Arc::new(create_pipeline(
        device,
        &vertex_shader,
        &fragment_shader,
        state,
        vertex.topology,
    ));
    PIPELINE_CREATIONS.fetch_add(1, Ordering::Relaxed);
    pipelines.insert(key, Arc::clone(&pipeline));
    Ok(pipeline)
}

fn vertex_shader_module(
    device: &wgpu::Device,
    vertex: &gpu_vertex::PreparedDraw,
) -> Result<Arc<wgpu::ShaderModule>, String> {
    let modules = VERTEX_SHADER_MODULES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut modules = modules
        .lock()
        .map_err(|_| "DMP host GPU vertex shader module cache lock poisoned")?;
    if let Some(module) = modules.get(&vertex.vertex_module_key) {
        return Ok(Arc::clone(module));
    }
    let module = Arc::new(device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("DMP translated vertex program"),
        source: wgpu::ShaderSource::Wgsl(vertex.shader_source.to_string().into()),
    }));
    modules.insert(vertex.vertex_module_key, Arc::clone(&module));
    VERTEX_MODULE_CREATIONS.fetch_add(1, Ordering::Relaxed);
    Ok(module)
}

fn fragment_shader_module(
    device: &wgpu::Device,
    flat_shading: bool,
) -> Result<Arc<wgpu::ShaderModule>, String> {
    let modules = FRAGMENT_SHADER_MODULES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut modules = modules
        .lock()
        .map_err(|_| "DMP host GPU fragment shader module cache lock poisoned")?;
    if let Some(module) = modules.get(&flat_shading) {
        return Ok(Arc::clone(module));
    }
    let fragment_source = format!(
        "{}\n{}",
        include_str!("gpu_fragment.wgsl"),
        include_str!("gpu_draw.wgsl")
    );
    let fragment_source = if flat_shading {
        fragment_source.replace(
            "@location(0) primary: vec4<f32>",
            "@location(0) @interpolate(flat) primary: vec4<f32>",
        )
    } else {
        fragment_source
    };
    let module = Arc::new(device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("DMP fragment program"),
        source: wgpu::ShaderSource::Wgsl(fragment_source.into()),
    }));
    modules.insert(flat_shading, Arc::clone(&module));
    FRAGMENT_MODULE_CREATIONS.fetch_add(1, Ordering::Relaxed);
    Ok(module)
}

fn create_pipeline(
    device: &wgpu::Device,
    vertex_shader: &wgpu::ShaderModule,
    fragment_shader: &wgpu::ShaderModule,
    state: &DrawState,
    topology: wgpu::PrimitiveTopology,
) -> wgpu::RenderPipeline {
    let targets = [
        Some(color_target()),
        Some(color_target()),
        Some(color_target()),
        Some(color_target()),
    ];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("DMP fixed-function draw state"),
        layout: None,
        vertex: wgpu::VertexState {
            module: vertex_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology,
            strip_index_format: (topology == wgpu::PrimitiveTopology::TriangleStrip)
                .then_some(wgpu::IndexFormat::Uint32),
            front_face: wgpu::FrontFace::Cw,
            cull_mode: match state.raster.cull_mode {
                CullMode::Disabled | CullMode::Other(_) => None,
                CullMode::Front => Some(wgpu::Face::Front),
                CullMode::Back => Some(wgpu::Face::Back),
            },
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: fragment_shader,
            entry_point: Some("draw_fragment"),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn color_target() -> wgpu::ColorTargetState {
    wgpu::ColorTargetState {
        format: wgpu::TextureFormat::R32Uint,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    }
}

fn copy_surface_texture(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    surface: &GpuSurface,
    extent: SurfaceExtent,
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("DMP draw destination snapshot"),
        size: wgpu::Extent3d {
            width: extent.width,
            height: extent.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    encoder.copy_texture_to_texture(
        surface.texture().as_image_copy(),
        texture.as_image_copy(),
        wgpu::Extent3d {
            width: extent.width,
            height: extent.height,
            depth_or_array_layers: 1,
        },
    );
    texture
}

fn draw_parameters(
    primitive: &Draw,
    recorded_reads: &[(u64, Vec<u8>)],
    read: &mut Reader<'_>,
) -> Result<(Vec<u32>, Vec<u8>), String> {
    let mut registers = vec![0u32; REGISTER_WORDS];
    for &(index, value) in &primitive.registers {
        registers[index as usize] = value;
    }
    let guest = texture_bytes(&primitive.state, &mut registers, recorded_reads, read)?;
    validate_blend(&primitive.state)?;
    let mut parameters = vec![0u32; CASE_WORDS];
    parameters[..REGISTER_WORDS].copy_from_slice(&registers);
    if primitive.state.fog.table.len() != FOG_WORDS {
        return Err("DMP draw fog table extent is invalid".into());
    }
    parameters[FOG_OFFSET..INPUT_OFFSET].copy_from_slice(&primitive.state.fog.table);
    Ok((parameters, guest))
}

fn validate_blend(state: &DrawState) -> Result<(), String> {
    if state.color_mask.0.iter().all(|enabled| !enabled) {
        return Ok(());
    }
    let blend = state.blend;
    if !blend.enabled {
        if blend.logic_operation != 3 {
            return Err("DMP unsupported logic operation".into());
        }
        return Ok(());
    }
    if [
        blend.source_rgb,
        blend.destination_rgb,
        blend.source_alpha,
        blend.destination_alpha,
    ]
    .iter()
    .any(|factor| matches!(factor, super::draw_state::BlendFactor::Other(_)))
    {
        return Err("DMP unsupported blend factor".into());
    }
    if [blend.rgb_equation, blend.alpha_equation]
        .iter()
        .any(|equation| matches!(equation, super::draw_state::BlendEquation::Other(_)))
    {
        return Err("DMP unsupported blend equation".into());
    }
    Ok(())
}

fn texture_bytes(
    state: &DrawState,
    registers: &mut [u32],
    recorded_reads: &[(u64, Vec<u8>)],
    read: &mut Reader<'_>,
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
) -> Result<(), String> {
    let size = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or("DMP draw texture size overflow")?;
    let level = recorded_range(recorded_reads, address, size)
        .map(Ok)
        .unwrap_or_else(|| read(address, size))?;
    if level.len() != size {
        return Err(format!(
            "DMP draw texture at {address:#x} has {} bytes, expected {size}",
            level.len()
        ));
    }
    bytes.extend_from_slice(&level);
    Ok(())
}

fn align_four(length: usize) -> usize {
    (length + 3) & !3
}

fn encode_words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}
