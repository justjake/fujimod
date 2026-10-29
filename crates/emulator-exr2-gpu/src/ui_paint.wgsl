requires readonly_and_readwrite_storage_textures;

@group(0) @binding(0) var<storage, read> params: array<u32>;
@group(0) @binding(1) var<storage, read> coverage: array<u32>;
@group(0) @binding(2) var<storage, read> clip_mask: array<u32>;
@group(0) @binding(3) var resident_surface: texture_storage_2d<r32uint, read_write>;
@group(0) @binding(4) var<storage, read> paint_data: array<u32>;
@group(0) @binding(5) var<storage, read_write> touched_pixels: atomic<u32>;

fn parameter_float(index: u32) -> f32 {
    return bitcast<f32>(params[index]);
}

fn color_transform(color: vec4<f32>) -> vec4<f32> {
    if params[30u] == 0u {
        return color;
    }
    let scale = vec4<f32>(
        parameter_float(31u),
        parameter_float(32u),
        parameter_float(33u),
        parameter_float(34u),
    );
    let bias = vec4<f32>(
        parameter_float(35u),
        parameter_float(36u),
        parameter_float(37u),
        parameter_float(38u),
    );
    return clamp(color * scale + bias, vec4<f32>(0.0), vec4<f32>(1.0));
}

fn packed_rgba(word: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(word & 255u),
        f32((word >> 8u) & 255u),
        f32((word >> 16u) & 255u),
        f32((word >> 24u) & 255u),
    ) / 255.0;
}

fn texture_rgba(word: u32) -> vec4<f32> {
    return vec4<f32>(
        f32((word >> 24u) & 255u),
        f32((word >> 16u) & 255u),
        f32((word >> 8u) & 255u),
        f32(word & 255u),
    ) / 255.0;
}

fn morton_index(width: u32, x: u32, y: u32) -> u32 {
    let local = (x & 1u)
        | ((y & 1u) << 1u)
        | ((x & 2u) << 1u)
        | ((y & 2u) << 2u)
        | ((x & 4u) << 2u)
        | ((y & 4u) << 3u);
    return ((y >> 3u) * (width >> 3u) + (x >> 3u)) * 64u + local;
}

fn gradient_color(x: f32, y: f32, radial: bool) -> vec4<f32> {
    let a = parameter_float(21u);
    let c = parameter_float(22u);
    let e = parameter_float(23u);
    let f = parameter_float(24u);
    var amount = a * x + c + e * y + f;
    if radial {
        let u = a * x + c;
        let v = e * y + f;
        let flags = params[29u];
        var cross = 0.0;
        if (flags & 3u) == 0u {
            cross = 2.0 * u * v;
        }
        let root = sqrt(max(
            parameter_float(27u) * u * u
                + parameter_float(28u) * v * v
                + cross,
            0.0,
        ));
        var linear = 0.0;
        if (flags & 1u) == 0u {
            linear += u;
        }
        if (flags & 2u) == 0u {
            linear += v;
        }
        if (flags & 4u) != 0u {
            amount = linear - root;
        } else {
            amount = linear + root;
        }
    }
    if params[25u] == 0u {
        amount = clamp(amount, 0.0, 1.0);
    } else if params[25u] == 1u {
        amount -= floor(amount);
    } else {
        let remainder = amount - floor(amount / 2.0) * 2.0;
        amount = 1.0 - abs(remainder - 1.0);
    }
    let position = clamp(amount * 127.0 + 0.5, 0.0, 127.0);
    let low = u32(floor(position));
    let high = min(low + 1u, 127u);
    let fraction = position - f32(low);
    let first = packed_rgba(paint_data[low]);
    let second = packed_rgba(paint_data[high]);
    var color = first * (1.0 - fraction) + second * fraction;
    if params[26u] != 0u {
        if color.a != 0.0 {
            color = vec4<f32>(color.rgb / color.a, color.a);
        } else {
            color = vec4<f32>(0.0);
        }
    }
    return color_transform(color);
}

fn source_color(x: f32, y: f32) -> vec4<f32> {
    if params[10u] == 2u {
        return gradient_color(x, y, false);
    }
    if params[10u] == 3u {
        return gradient_color(x, y, true);
    }
    if params[10u] == 1u {
        let a = parameter_float(15u);
        let b = parameter_float(16u);
        let c = parameter_float(17u);
        let d = parameter_float(18u);
        let e = parameter_float(19u);
        let f = parameter_float(20u);
        let u = i32(floor(a * x + b * y + c));
        let v = i32(floor(d * x + e * y + f));
        let valid = u >= 0
            && v >= 0
            && u < i32(params[13u])
            && v < i32(params[14u]);
        let source_x = u32(clamp(u, 0, i32(params[11u]) - 1));
        let source_y = u32(clamp(v, 0, i32(params[12u]) - 1));
        let index = morton_index(params[11u], source_x, source_y);
        var color = texture_rgba(paint_data[index]);
        if !valid {
            color = vec4<f32>(0.0);
        }
        return color_transform(color);
    }
    return color_transform(packed_rgba(params[39u]));
}

fn quantize(value: f32) -> u32 {
    return u32(floor(clamp(value, 0.0, 1.0) * 255.0 + 0.5));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    if invocation.x >= params[2u] || invocation.y >= params[3u] {
        return;
    }
    let local_index = invocation.y * params[2u] + invocation.x;
    if clip_mask[local_index] == 0u {
        return;
    }
    let coverage_count = coverage[params[6u] + local_index];
    if coverage_count == 0u {
        return;
    }
    atomicAdd(&touched_pixels, 1u);
    let coverage_byte = coverage_count * 255u / params[7u];
    if coverage_byte == 0u {
        return;
    }

    let x = params[0u] + invocation.x;
    let y = params[1u] + invocation.y;
    let packed_destination = textureLoad(resident_surface, vec2<i32>(i32(x), i32(y))).r;
    var destination = vec4<f32>(
        f32(packed_destination & 255u),
        f32((packed_destination >> 8u) & 255u),
        f32((packed_destination >> 16u) & 255u),
        f32((packed_destination >> 24u) & 255u),
    ) / 255.0;
    if params[8u] == 0u {
        destination = destination.wzyx;
    } else {
        destination = destination.zyxw;
    }
    let coverage_fraction = f32(coverage_byte) / 255.0;
    let source = source_color(f32(x) + 0.5, f32(y) + 0.5);
    let alpha = source.a * coverage_fraction;
    if params[9u] != 0u && alpha == 0.0 {
        return;
    }
    let retained = select(1.0 - coverage_fraction, 1.0 - alpha, params[9u] != 0u);
    let output_alpha = alpha + destination.a * retained;
    var output_rgb = vec3<f32>(0.0);
    if output_alpha != 0.0 {
        output_rgb = (source.rgb * alpha + destination.rgb * destination.a * retained) / output_alpha;
    }
    let rgba = vec4<u32>(
        quantize(output_rgb.r),
        quantize(output_rgb.g),
        quantize(output_rgb.b),
        quantize(output_alpha),
    );
    var packed_output = rgba.w << 24u | rgba.x << 16u | rgba.y << 8u | rgba.z;
    if params[8u] == 0u {
        packed_output = rgba.x << 24u | rgba.y << 16u | rgba.z << 8u | rgba.w;
    }
    textureStore(resident_surface, vec2<i32>(i32(x), i32(y)), vec4<u32>(packed_output, 0u, 0u, 0u));
}
