use wgpu::util::DeviceExt;

#[test]
fn rgba8_native_blend_matches_byte_quantized_fragment_equations() {
    let context = fujimod_emulator_gpu::context().unwrap();
    let device = context.device();
    let queue = context.queue();
    let (width, height) = (256u32, 256u32);
    let mut seed = 0x12345678u32;
    let mut byte = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 24) as u8
    };
    let mut source = Vec::new();
    let mut destination = Vec::new();
    for index in 0..width * height {
        source.push([byte(), byte(), byte(), index as u8]);
        destination.push([byte(), byte(), byte(), byte()]);
    }
    let mut source_bytes = Vec::new();
    for pixel in &source {
        for value in pixel {
            source_bytes.extend_from_slice(&(f32::from(*value) / 255.).to_le_bytes());
        }
    }
    let source_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("blend conformance sources"),
        contents: &source_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("blend conformance target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native blend conformance"), source: wgpu::ShaderSource::Wgsl(r#"
        @group(0) @binding(0) var<storage, read> colors: array<vec4<f32>>;
        struct Vertex { @builtin(position) position: vec4<f32>, @location(0) @interpolate(flat) index: u32 };
        @vertex fn vertex(@builtin(vertex_index) v: u32, @builtin(instance_index) instance: u32) -> Vertex {
            let corners = array<vec2<f32>, 6>(vec2(0.,0.),vec2(1.,0.),vec2(0.,1.),vec2(0.,1.),vec2(1.,0.),vec2(1.,1.));
            let pixel = vec2<f32>(f32(instance % 256u), f32(instance / 256u)) + corners[v];
            return Vertex(vec4<f32>(pixel.x / 128. - 1., 1. - pixel.y / 128., 0., 1.), instance);
        }
        @fragment fn fragment(input: Vertex) -> @location(0) vec4<f32> { return colors[input.index]; }
        "#.into()),
    });
    let layout = wgpu::TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(width * 4),
        rows_per_image: Some(height),
    };
    for factor in [wgpu::BlendFactor::SrcAlpha, wgpu::BlendFactor::One] {
        let submission = context.acquire().unwrap();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native blend conformance"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: factor,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: source_buffer.as_entire_binding(),
            }],
        });
        let bytes: Vec<_> = destination.iter().flatten().copied().collect();
        queue.write_texture(
            texture.as_image_copy(),
            &bytes,
            layout,
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let download = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.draw(0..6, 0..width * height);
        }
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &download,
                layout,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        download
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).unwrap();
            });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receiver.recv().unwrap().unwrap();
        let mapped = download.slice(..).get_mapped_range().unwrap();
        // The lock only orders submissions; release it before comparing so a
        // failed expectation cannot poison it for the rest of the suite.
        drop(submission);
        let mut mismatches = 0;
        for (index, (pixel, old)) in source.iter().zip(&destination).enumerate() {
            let source = pixel.map(|c| f32::from(c) / 255.);
            let destination = old.map(|c| f32::from(c) / 255.);
            for channel in 0..4 {
                let alpha = if channel == 3 || factor == wgpu::BlendFactor::One {
                    1.
                } else {
                    source[3]
                };
                let expected = ((source[channel] * alpha + destination[channel] * (1. - source[3]))
                    .clamp(0., 1.)
                    * 255.
                    + 0.5)
                    .floor() as u8;
                // Vulkan §36.1 requires basic blend operations only to have
                // "a precision and dynamic range no lower than that used to
                // represent destination components". An RGBA8 driver may
                // therefore round each blend term to eight bits and land one
                // byte away from these equations; bigger errors still fail.
                if mapped[index * 4 + channel].abs_diff(expected) > 1 {
                    mismatches += 1;
                }
            }
        }
        assert_eq!(mismatches, 0, "{factor:?}");
    }
}

#[test]
fn native_preparation_validates_smooth_and_flat_fragment_interfaces() {
    let context = fujimod_emulator_gpu::context().unwrap();
    super::native_blend::prepare(context).unwrap();
}
