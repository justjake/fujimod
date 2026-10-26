//! Small, prefiltered studio reflection maps. No per-frame capture or extra passes.
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
    },
};

pub(crate) fn studio_light(images: &mut Assets<Image>) -> EnvironmentMapLight {
    EnvironmentMapLight {
        diffuse_map: images.add(cubemap(16, true)),
        specular_map: images.add(cubemap(128, false)),
        intensity: 1800.,
        ..default()
    }
}
fn cubemap(size: u32, diffuse: bool) -> Image {
    let levels = size.ilog2() + 1;
    let mut data = Vec::new();
    // TextureDataOrder::LayerMajor: all mips for each cube face.
    for face in 0..6 {
        for level in 0..levels {
            let width = size >> level;
            let roughness = if diffuse {
                1.
            } else {
                level as f32 / (levels - 1) as f32
            };
            for y in 0..width {
                for x in 0..width {
                    let u = (x as f32 + 0.5) / width as f32 * 2. - 1.;
                    let v = (y as f32 + 0.5) / width as f32 * 2. - 1.;
                    let direction = match face {
                        0 => Vec3::new(1., -v, -u),
                        1 => Vec3::new(-1., -v, u),
                        2 => Vec3::new(u, 1., v),
                        3 => Vec3::new(u, -1., -v),
                        4 => Vec3::new(u, -v, 1.),
                        _ => Vec3::new(-u, -v, -1.),
                    }
                    .normalize();
                    let sky = direction.y * 0.5 + 0.5;
                    let mut color =
                        Vec3::new(0.025, 0.02, 0.018).lerp(Vec3::new(0.1, 0.13, 0.18), sky);
                    let spread = 0.025 + roughness * roughness * 0.6;
                    for (position, tint) in [
                        (Vec3::new(-1., 1.5, 1.), Vec3::new(1., 0.94, 0.84)),
                        (Vec3::new(1., 0.7, -1.), Vec3::new(0.55, 0.7, 1.)),
                        (Vec3::new(0., 2., -0.2), Vec3::splat(0.75)),
                    ] {
                        let radiance = ((direction.dot(position.normalize()) - 1.) / spread).exp()
                            * 0.025
                            / spread;
                        color += tint * radiance;
                    }
                    data.extend(
                        color
                            .clamp(Vec3::ZERO, Vec3::ONE)
                            .to_array()
                            .map(|v| (v * 255.).round() as u8),
                    );
                    data.push(255);
                }
            }
        }
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = levels;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image.data = Some(data);
    image
}
