//! Ordered native RGBA8 blending for byte-quantized copy and source-over draws.
//! Other formats and tests retain the packed-surface fragment equations.
use super::super::draw_state::{BlendEquation, BlendFactor, ColorTargetFormat, CullMode};
use super::{DrawBuffers, DrawState, PreparedDraw, fragment_shader_module, vertex_shader_module};
use fujimod_emulator_gpu::GpuContext;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, atomic::Ordering},
};
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) enum Mode {
    Copy,
    SourceAlpha,
    Premultiplied,
}

pub(super) fn mode(state: &DrawState) -> Option<Mode> {
    if state.targets.color_format != ColorTargetFormat::Rgba8
        || state.color_mask.0 != [true; 4]
        || state.fog.enabled
        || state.depth_test.enabled
        || state.stencil_test.enabled
    {
        return None;
    }
    let blend = state.blend;
    if !blend.enabled {
        return (blend.logic_operation == 3).then_some(Mode::Copy);
    }
    if blend.rgb_equation == BlendEquation::Add
        && blend.alpha_equation == BlendEquation::Add
        && blend.source_rgb == BlendFactor::One
        && blend.destination_rgb == BlendFactor::Zero
        && blend.source_alpha == BlendFactor::One
        && blend.destination_alpha == BlendFactor::Zero
    {
        return Some(Mode::Copy);
    }
    if blend.rgb_equation != BlendEquation::Add
        || blend.alpha_equation != BlendEquation::Add
        || blend.destination_rgb != BlendFactor::OneMinusSourceAlpha
        || blend.source_alpha != BlendFactor::One
        || blend.destination_alpha != BlendFactor::OneMinusSourceAlpha
    {
        return None;
    }
    match blend.source_rgb {
        BlendFactor::SourceAlpha => Some(Mode::SourceAlpha),
        BlendFactor::One => Some(Mode::Premultiplied),
        _ => None,
    }
}

struct Transfer {
    import: wgpu::ComputePipeline,
    export: wgpu::ComputePipeline,
}
static TRANSFER: OnceLock<Transfer> = OnceLock::new();
static PIPELINES: OnceLock<Mutex<HashMap<([u8; 32], Mode), Arc<wgpu::RenderPipeline>>>> =
    OnceLock::new();

fn transfer(device: &wgpu::Device) -> &'static Transfer {
    TRANSFER.get_or_init(|| {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("DMP RGBA8 packed transfer"),
            source: wgpu::ShaderSource::Wgsl(include_str!("native_blend.wgsl").into()),
        });
        let make = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("DMP RGBA8 packed transfer"),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Transfer {
            import: make("import_rgba8"),
            export: make("export_rgba8"),
        }
    })
}

fn pipeline(
    device: &wgpu::Device,
    draw: &PreparedDraw,
    mode: Mode,
) -> Result<Arc<wgpu::RenderPipeline>, String> {
    let key = (draw.vertex.pipeline_key, mode);
    let cache = PIPELINES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(pipeline) = cache
        .lock()
        .map_err(|_| "DMP native blend cache lock poisoned")?
        .get(&key)
    {
        return Ok(Arc::clone(pipeline));
    }
    let vertex = vertex_shader_module(device, &draw.vertex)?;
    let fragment = fragment_shader_module(
        device,
        draw.state.raster.flat_shading,
        super::FragmentTarget::NormalizedRgba8,
    )?;
    let blend = match mode {
        Mode::Copy => None,
        Mode::SourceAlpha | Mode::Premultiplied => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: if mode == Mode::SourceAlpha {
                    wgpu::BlendFactor::SrcAlpha
                } else {
                    wgpu::BlendFactor::One
                },
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        }),
    };
    let pipeline = Arc::new(
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("DMP native RGBA8 draw"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &vertex,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: draw.vertex.topology,
                strip_index_format: (draw.vertex.topology
                    == wgpu::PrimitiveTopology::TriangleStrip)
                    .then_some(wgpu::IndexFormat::Uint32),
                front_face: wgpu::FrontFace::Cw,
                cull_mode: match draw.state.raster.cull_mode {
                    CullMode::Front => Some(wgpu::Face::Front),
                    CullMode::Back => Some(wgpu::Face::Back),
                    _ => None,
                },
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: Some("native_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        }),
    );
    super::PIPELINE_CREATIONS.fetch_add(1, Ordering::Relaxed);
    cache
        .lock()
        .map_err(|_| "DMP native blend cache lock poisoned")?
        .insert(key, Arc::clone(&pipeline));
    Ok(pipeline)
}

pub(super) fn render(
    draws: &[PreparedDraw],
    first: usize,
    context: &GpuContext,
    buffers: &DrawBuffers,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), String> {
    let draw = &draws[0];
    let extent = draw.extent;
    let device = context.device();
    let surface = context
        .get(draw.color_key)?
        .ok_or("DMP native color surface is not resident")?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("DMP native RGBA8 target"),
        size: wgpu::Extent3d {
            width: extent.width,
            height: extent.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let native_view = texture.create_view(&Default::default());
    let packed_view = surface.texture().create_view(&Default::default());
    let params: Vec<_> = [extent.width, extent.height, 0, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let parameter_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &params,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let transfer = transfer(device);
    {
        let pipeline = &transfer.import;
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: parameter_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&packed_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&native_view),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("DMP RGBA8 import"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.dispatch_workgroups(extent.width.div_ceil(8), extent.height.div_ceil(8), 1);
    }
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("DMP ordered native RGBA8 batch"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &native_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for (offset, draw) in draws.iter().enumerate() {
            let [x, y, width, height] = draw.state.viewport.rectangle();
            pass.set_viewport(x, y, width, height, 0., 1.);
            let scissor = draw.state.scissor;
            let left = u32::from(scissor.left).min(extent.width);
            let top = u32::from(scissor.top).min(extent.height);
            let right = (u32::from(scissor.right) + 1).min(extent.width);
            let bottom = (u32::from(scissor.bottom) + 1).min(extent.height);
            if left >= right || top >= bottom {
                continue;
            }
            let pipeline = pipeline(
                device,
                draw,
                mode(&draw.state).ok_or("DMP invalid native blend batch")?,
            )?;
            let index = first + offset;
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffers.parameters(index),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffers.guest(index),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: buffers.uniforms(index),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: buffers.inputs(index),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: surface.write_mask_accumulator().as_entire_binding(),
                    },
                ],
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.set_scissor_rect(left, top, right - left, bottom - top);
            pass.set_index_buffer(buffers.indices(index), wgpu::IndexFormat::Uint32);
            let count = u32::try_from(draw.vertex.indices.len())
                .map_err(|_| "DMP native index count exceeds u32")?;
            pass.draw_indexed(0..count, 0, 0..1);
        }
    }
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &transfer.export.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: parameter_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&native_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&packed_view),
            },
        ],
    });
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("DMP RGBA8 export"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&transfer.export);
    pass.set_bind_group(0, &bindings, &[]);
    pass.dispatch_workgroups(extent.width.div_ceil(8), extent.height.div_ceil(8), 1);
    Ok(())
}

// This pipeline is compiled but never submitted. It primes the shared fragment
// library and transfer kernels before an interactive owner starts accepting
// input. Guest vertex programs are still translated from their actual uploads.
static PREPARED: OnceLock<Vec<wgpu::RenderPipeline>> = OnceLock::new();
pub(super) fn prepare(context: &GpuContext) -> Result<(), String> {
    let device = context.device();
    transfer(device);
    if PREPARED.get().is_some() {
        return Ok(());
    }
    let mut pipelines = Vec::new();
    for flat in [false, true] {
        let source = include_str!("prepare_vertex.wgsl");
        let source = if flat {
            source.replace(
                "@location(0) primary",
                "@location(0) @interpolate(flat) primary",
            )
        } else {
            source.to_owned()
        };
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("DMP host renderer preparation"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let fragment =
            fragment_shader_module(device, flat, super::FragmentTarget::NormalizedRgba8)?;
        pipelines.push(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("DMP host renderer preparation"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &vertex,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &fragment,
                    entry_point: Some("native_fragment"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }),
        );
    }
    let _ = PREPARED.set(pipelines);
    Ok(())
}
