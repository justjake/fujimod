@group(0) @binding(3) var color_destination: texture_2d<u32>;
@group(0) @binding(4) var depth_destination: texture_2d<u32>;

struct DrawFragmentInput {
    @builtin(position) position: vec4<f32>,
    @location(0) primary: vec4<f32>,
    @location(1) texcoords01: vec4<f32>,
    @location(2) texcoord2: vec2<f32>,
    @location(3) @interpolate(linear) depth: f32,
    @location(6) @interpolate(flat) valid: u32,
};

struct DrawFragmentOutput {
    @location(0) color: u32,
    @location(1) depth: u32,
    @location(2) color_mask: u32,
    @location(3) depth_mask: u32,
};

fn unpack_surface(pixel: u32, format: u32) -> vec4<f32> {
    if format == 0u {
        return vec4<f32>(
            f32((pixel >> 24u) & 255u),
            f32((pixel >> 16u) & 255u),
            f32((pixel >> 8u) & 255u),
            f32(pixel & 255u),
        ) / 255.0;
    }
    if format == 1u {
        return vec4<f32>(
            f32((pixel >> 11u) & 31u) / 31.0,
            f32((pixel >> 5u) & 63u) / 63.0,
            f32(pixel & 31u) / 31.0,
            1.0,
        );
    }
    if format == 2u {
        return vec4<f32>(
            f32((pixel >> 11u) & 31u) / 31.0,
            f32((pixel >> 6u) & 31u) / 31.0,
            f32((pixel >> 1u) & 31u) / 31.0,
            f32(pixel & 1u),
        );
    }
    return vec4<f32>(
        f32((pixel >> 12u) & 15u) / 15.0,
        f32((pixel >> 8u) & 15u) / 15.0,
        f32((pixel >> 4u) & 15u) / 15.0,
        f32(pixel & 15u) / 15.0,
    );
}

fn store_surface(pixel: u32, color: vec4<f32>, format: u32, mask: u32) -> u32 {
    var result = pixel;
    let clamped = clamp(color, vec4<f32>(0.0), vec4<f32>(1.0));
    if format == 0u {
        for (var channel = 0u; channel < 4u; channel += 1u) {
            if (mask & (1u << channel)) != 0u {
                let value = u32(floor(clamped[channel] * 255.0 + 0.5));
                let shift = (3u - channel) * 8u;
                result = (result & ~(255u << shift)) | (value << shift);
            }
        }
        return result;
    }

    var packed = 0u;
    var bits = 0u;
    if format == 1u {
        packed = (u32(clamped.x * 31.0) << 11u)
            | (u32(clamped.y * 63.0) << 5u)
            | u32(clamped.z * 31.0);
        if (mask & 1u) != 0u { bits |= 0xf800u; }
        if (mask & 2u) != 0u { bits |= 0x07e0u; }
        if (mask & 4u) != 0u { bits |= 0x001fu; }
    } else if format == 2u {
        packed = (u32(clamped.x * 31.0) << 11u)
            | (u32(clamped.y * 31.0) << 6u)
            | (u32(clamped.z * 31.0) << 1u)
            | u32(clamped.w);
        if (mask & 1u) != 0u { bits |= 0xf800u; }
        if (mask & 2u) != 0u { bits |= 0x07c0u; }
        if (mask & 4u) != 0u { bits |= 0x003eu; }
        if (mask & 8u) != 0u { bits |= 0x0001u; }
    } else {
        packed = (u32(clamped.x * 15.0) << 12u)
            | (u32(clamped.y * 15.0) << 8u)
            | (u32(clamped.z * 15.0) << 4u)
            | u32(clamped.w * 15.0);
        if (mask & 1u) != 0u { bits |= 0xf000u; }
        if (mask & 2u) != 0u { bits |= 0x0f00u; }
        if (mask & 4u) != 0u { bits |= 0x00f0u; }
        if (mask & 8u) != 0u { bits |= 0x000fu; }
    }
    return (pixel & ~bits) | (packed & bits);
}

fn lod_from_derivatives(uv: vec2<f32>, dimensions: vec2<f32>) -> f32 {
    let dx = dpdx(uv) * dimensions;
    let dy = dpdy(uv) * dimensions;
    let rho = max(dot(dx, dx), dot(dy, dy));
    if rho > 0.0 {
        return 0.5 * log2(rho);
    }
    return -128.0;
}

fn shade_color_and_depth(
    input: DrawFragmentInput,
    source_color: vec4<f32>,
) -> DrawFragmentOutput {
    let z = input.depth;
    var color = source_color;
    var output: DrawFragmentOutput;
    output.color_mask = 0u;
    output.depth_mask = 0u;
    let color_code = (reg(0u, 0x317u) >> 16u) & 15u;
    let channel_mask = (reg(0u, 0x307u) >> 8u) & 15u;
    let old_color = textureLoad(color_destination, vec2<i32>(input.position.xy), 0).x;
    var destination = vec4<f32>(0.0);
    if channel_mask != 0u {
        destination = unpack_surface(old_color, color_code);
    }

    let depth_control = reg(0u, 0x307u);
    let stencil = reg(0u, 0x305u);
    let operations = reg(0u, 0x306u);
    let old_depth = textureLoad(depth_destination, vec2<i32>(input.position.xy), 0).x;
    var result_depth = old_depth;
    var depth_pass = true;
    var stencil_pass = true;
    let depth_active = (depth_control & 1u) != 0u || (stencil & 1u) != 0u;
    let depth_format = reg(0u, 0x316u);
    if depth_active {
        var maximum = 0x00ffffffu;
        var depth_bytes = 3u;
        if depth_format == 0u {
            maximum = 65535u;
            depth_bytes = 2u;
        } else if depth_format == 3u {
            depth_bytes = 4u;
        }
        let old_stencil = (old_depth >> 24u) & 255u;
        let reference = (stencil >> 16u) & 255u;
        let value_mask = (stencil >> 24u) & 255u;
        stencil_pass = (stencil & 1u) == 0u || comparison(
            stencil >> 4u,
            reference & value_mask,
            old_stencil & value_mask,
        );
        let depth = u32(clamp(z, 0.0, 1.0) * f32(maximum));
        depth_pass = (depth_control & 1u) == 0u
            || comparison(depth_control >> 4u, depth, old_depth & maximum);
        if (stencil & 1u) != 0u && depth_format == 3u {
            var operation = (operations >> 8u) & 7u;
            if !stencil_pass {
                operation = operations & 7u;
            } else if !depth_pass {
                operation = (operations >> 4u) & 7u;
            }
            let write_mask = (stencil >> 8u) & 255u;
            let updated = (old_stencil & ~write_mask)
                | (stencil_result(operation, old_stencil, reference) & write_mask);
            result_depth = (old_depth & 0x00ffffffu) | (updated << 24u);
            if write_mask != 0u {
                output.depth_mask |= 1u << 3u;
            }
        }
        if !stencil_pass || !depth_pass {
            output.color = old_color;
            output.depth = result_depth;
            return output;
        }
        if (depth_control & 0x1001u) == 0x1001u {
            result_depth = (result_depth & ~maximum) | depth;
            output.depth_mask |= (1u << depth_bytes) - 1u;
        }
    }

    if channel_mask != 0u {
        let format = color_code;
        let control = reg(0u, 0x301u);
        let constant_word = reg(0u, 0x303u);
        let constant = vec4<f32>(
            f32(constant_word & 255u),
            f32((constant_word >> 8u) & 255u),
            f32((constant_word >> 16u) & 255u),
            f32(constant_word >> 24u),
        ) / 255.0;
        if (reg(0u, 0x300u) & 0x100u) != 0u {
            let source = color;
            for (var channel = 0u; channel < 4u; channel += 1u) {
                let shift = select(16u, 24u, channel == 3u);
                let source_factor = (control >> shift) & 15u;
                let destination_factor = (control >> (shift + 4u)) & 15u;
                let equation = select(control & 255u, (control >> 8u) & 255u, channel == 3u);
                let a = source[channel]
                    * blend_factor(source_factor, channel, source, destination, constant);
                let b = destination[channel]
                    * blend_factor(destination_factor, channel, source, destination, constant);
                if equation == 0u {
                    color[channel] = a + b;
                } else if equation == 1u {
                    color[channel] = a - b;
                } else if equation == 2u {
                    color[channel] = b - a;
                } else if equation == 3u {
                    color[channel] = min(source[channel], destination[channel]);
                } else {
                    color[channel] = max(source[channel], destination[channel]);
                }
            }
        }
        for (var channel = 0u; channel < 4u; channel += 1u) {
            if (channel_mask & (1u << channel)) == 0u {
                color[channel] = destination[channel];
            }
        }
        output.color = store_surface(old_color, color, format, channel_mask);
        if format == 0u {
            for (var channel = 0u; channel < 4u; channel += 1u) {
                if (channel_mask & (1u << channel)) != 0u {
                    output.color_mask |= 1u << (3u - channel);
                }
            }
        } else {
            output.color_mask = 3u;
        }
    } else {
        output.color = old_color;
    }
    output.depth = result_depth;
    return output;
}

@fragment
fn draw_fragment(input: DrawFragmentInput) -> DrawFragmentOutput {
    if input.valid == 0u {
        discard;
    }
    let x = u32(input.position.x);
    let y = u32(input.position.y);
    let z = input.depth;
    let origin = reg(0u, 0x266u);
    let end = reg(0u, 0x267u);
    if x < (origin & 65535u) || y < (origin >> 16u)
        || x > (end & 65535u) || y > (end >> 16u) {
        discard;
    }

    var texture0 = vec4<f32>(0.0);
    var texture1 = vec4<f32>(0.0);
    let texture_enable = reg(0u, 0x280u);
    if (texture_enable & 1u) != 0u {
        let tex_dimensions = reg(0u, 0x282u);
        let size = vec2<f32>(f32(tex_dimensions >> 16u), f32(tex_dimensions & 65535u));
        texture0 = sample_texture(
            0u,
            0u,
            input.texcoords01.xy,
            lod_from_derivatives(input.texcoords01.xy, size),
        );
    }
    if (texture_enable & 2u) != 0u {
        let tex_dimensions = reg(0u, 0x292u);
        let size = vec2<f32>(f32(tex_dimensions >> 16u), f32(tex_dimensions & 65535u));
        texture1 = sample_texture(
            0u,
            1u,
            input.texcoords01.zw,
            lod_from_derivatives(input.texcoords01.zw, size),
        );
    }

    var color = combine(0u, input.primary, texture0, texture1);
    let alpha = reg(0u, 0x304u);
    if (alpha & 1u) != 0u {
        let value = u32(clamp(color.w, 0.0, 1.0) * 255.0);
        if !comparison(alpha >> 4u, value, (alpha >> 8u) & 255u) {
            discard;
        }
    }

    let fog = reg(0u, 0x2e0u);
    if (fog & 1u) != 0u {
        var factor = 0.0;
        if (fog & 4u) != 0u {
            let position = min(clamp(z, 0.0, 1.0) * 128.0, 127.9999);
            let word = param(0u, FOG_OFFSET + u32(position));
            factor = clamp(
                f32((word >> 13u) & 2047u) / 2048.0
                    + f32(signed13(word)) / 2048.0 * fract(position),
                0.0,
                1.0,
            );
        } else {
            let scale = f32(signed16(reg(0u, 0x2e2u))) / 2048.0;
            let bias = f32(signed16(reg(0u, 0x2e3u))) / 2048.0;
            factor = clamp(z * scale + bias, 0.0, 1.0);
        }
        let fog_color = reg(0u, 0x2e1u);
        for (var channel = 0u; channel < 3u; channel += 1u) {
            let component = f32((fog_color >> (channel * 8u)) & 255u) / 255.0;
            color[channel] = color[channel] * factor + component * (1.0 - factor);
        }
    }

    let output = shade_color_and_depth(input, color);
    return output;
}
