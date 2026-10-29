struct Parameters {
    words: array<u32>,
}

struct GuestMemory {
    words: array<u32>,
}

struct FragmentStatus {
    words: array<u32>,
}

@group(0) @binding(0) var<storage, read> params: Parameters;
@group(0) @binding(1) var<storage, read_write> guest: GuestMemory;
@group(0) @binding(2) var<storage, read_write> status: FragmentStatus;

const FOG_OFFSET: u32 = 1280u;
const INPUT_OFFSET: u32 = 1408u;
const CASE_WORDS: u32 = 1424u;

fn param(case_index: u32, index: u32) -> u32 {
    return params.words[case_index * CASE_WORDS + index];
}

fn reg(case_index: u32, index: u32) -> u32 {
    return param(case_index, index);
}

fn input_word(case_index: u32, index: u32) -> u32 {
    return param(case_index, INPUT_OFFSET + index);
}

fn signed13(value: u32) -> i32 {
    return bitcast<i32>((value & 8191u) << 19u) >> 19;
}

fn signed16(value: u32) -> i32 {
    return bitcast<i32>((value & 65535u) << 16u) >> 16;
}

fn memory_byte(address: u32) -> u32 {
    let word = guest.words[address >> 2u];
    return (word >> ((address & 3u) * 8u)) & 255u;
}

fn write_memory_byte(address: u32, value: u32) {
    let index = address >> 2u;
    let shift = (address & 3u) * 8u;
    let mask = 255u << shift;
    guest.words[index] = (guest.words[index] & ~mask) | ((value & 255u) << shift);
}

fn memory_u16(address: u32) -> u32 {
    return memory_byte(address) | (memory_byte(address + 1u) << 8u);
}

fn memory_u24(address: u32) -> u32 {
    return memory_u16(address) | (memory_byte(address + 2u) << 16u);
}

fn memory_u32(address: u32) -> u32 {
    return memory_u24(address) | (memory_byte(address + 3u) << 24u);
}

fn write_u16(address: u32, value: u32) {
    write_memory_byte(address, value);
    write_memory_byte(address + 1u, value >> 8u);
}

fn write_u24(address: u32, value: u32) {
    write_u16(address, value);
    write_memory_byte(address + 2u, value >> 16u);
}

fn morton_index(x: u32, y: u32, width: u32) -> u32 {
    return (y & ~7u) * width
        + (x & ~7u) * 8u
        + (x & 1u)
        + ((y & 1u) << 1u)
        + ((x & 2u) << 1u)
        + ((y & 2u) << 2u)
        + ((x & 4u) << 2u)
        + ((y & 4u) << 3u);
}

fn wrap_coord(value: i32, extent: u32, mode: u32) -> vec2<i32> {
    let n = i32(extent);
    if mode == 0u {
        return vec2<i32>(clamp(value, 0, n - 1), 1);
    }
    if mode == 1u {
        if value < 0 || value >= n {
            return vec2<i32>(-1, 0);
        }
        return vec2<i32>(value, 1);
    }
    if mode == 2u {
        return vec2<i32>((value % n + n) % n, 1);
    }
    let period = 2 * n;
    let mirrored = (value % period + period) % period;
    if mirrored < n {
        return vec2<i32>(mirrored, 1);
    }
    return vec2<i32>(period - 1 - mirrored, 1);
}

fn unpack_texel(address: u32, format: u32) -> vec4<f32> {
    let b0 = memory_byte(address);
    if format == 0u {
        return vec4<f32>(
            f32(memory_byte(address + 3u)) / 255.0,
            f32(memory_byte(address + 2u)) / 255.0,
            f32(memory_byte(address + 1u)) / 255.0,
            f32(b0) / 255.0,
        );
    }
    if format == 1u {
        return vec4<f32>(
            f32(memory_byte(address + 2u)) / 255.0,
            f32(memory_byte(address + 1u)) / 255.0,
            f32(b0) / 255.0,
            1.0,
        );
    }
    if format == 2u || format == 3u || format == 4u {
        let word = b0 | (memory_byte(address + 1u) << 8u);
        if format == 2u {
            return vec4<f32>(
                f32(word >> 11u) / 31.0,
                f32((word >> 6u) & 31u) / 31.0,
                f32((word >> 1u) & 31u) / 31.0,
                f32(word & 1u),
            );
        }
        if format == 3u {
            return vec4<f32>(
                f32(word >> 11u) / 31.0,
                f32((word >> 5u) & 63u) / 63.0,
                f32(word & 31u) / 31.0,
                1.0,
            );
        }
        return vec4<f32>(
            f32(word >> 12u) / 15.0,
            f32((word >> 8u) & 15u) / 15.0,
            f32((word >> 4u) & 15u) / 15.0,
            f32(word & 15u) / 15.0,
        );
    }
    if format == 5u {
        let luminance = f32(memory_byte(address + 1u)) / 255.0;
        return vec4<f32>(luminance, luminance, luminance, f32(b0) / 255.0);
    }
    if format == 7u {
        let luminance = f32(b0) / 255.0;
        return vec4<f32>(luminance, luminance, luminance, 1.0);
    }
    if format == 8u {
        return vec4<f32>(1.0, 1.0, 1.0, f32(b0) / 255.0);
    }
    return vec4<f32>(0.0);
}

fn texture_texel(
    width: u32,
    height: u32,
    control: u32,
    format: u32,
    address: u32,
    border: vec4<f32>,
    x: i32,
    y: i32,
) -> vec4<f32> {
    let wrapped_x = wrap_coord(x, width, (control >> 12u) & 7u);
    let wrapped_y = wrap_coord(y, height, (control >> 8u) & 7u);
    if wrapped_x.y == 0 || wrapped_y.y == 0 {
        return border;
    }
    let pixel = morton_index(u32(wrapped_x.x), u32(wrapped_y.x), width);
    var bytes_per_pixel = 1u;
    if format == 0u {
        bytes_per_pixel = 4u;
    } else if format == 1u {
        bytes_per_pixel = 3u;
    } else if format >= 2u && format <= 5u {
        bytes_per_pixel = 2u;
    }
    return unpack_texel(address + pixel * bytes_per_pixel, format);
}

fn sample_level(
    case_index: u32,
    unit: u32,
    uv: vec2<f32>,
    level: u32,
    minifying: bool,
) -> vec4<f32> {
    let base = select(0x291u, 0x281u, unit == 0u);
    let dimensions = reg(case_index, base + 1u);
    let base_width = dimensions >> 16u;
    let base_height = dimensions & 65535u;
    let width = base_width >> level;
    let height = base_height >> level;
    let control = reg(case_index, base + 2u);
    let format_register = select(base + 5u, base + 13u, unit == 0u);
    let format = reg(case_index, format_register) & 15u;
    var bytes_per_pixel = 1u;
    if format == 0u {
        bytes_per_pixel = 4u;
    } else if format == 1u {
        bytes_per_pixel = 3u;
    } else if format >= 2u && format <= 5u {
        bytes_per_pixel = 2u;
    }
    var address = reg(case_index, base + 4u);
    if level > 0u {
        var offset = 0u;
        for (var mip = 1u; mip < level; mip += 1u) {
            offset += (base_width >> mip) * (base_height >> mip) * bytes_per_pixel;
        }
        address = reg(case_index, base + 8u) + offset;
    }
    let border_word = reg(case_index, base);
    let border = vec4<f32>(
        f32(border_word & 255u),
        f32((border_word >> 8u) & 255u),
        f32((border_word >> 16u) & 255u),
        f32(border_word >> 24u),
    ) / 255.0;
    let x = uv.x * f32(width);
    let y = uv.y * f32(height);
    let filter_bit = select(2u, 4u, minifying);
    if (control & filter_bit) == 0u {
        return texture_texel(
            width,
            height,
            control,
            format,
            address,
            border,
            i32(floor(x)),
            i32(floor(y)),
        );
    }
    let centered_x = x - 0.5;
    let centered_y = y - 0.5;
    let ix = i32(floor(centered_x));
    let iy = i32(floor(centered_y));
    let fx = centered_x - floor(centered_x);
    let fy = centered_y - floor(centered_y);
    let a = texture_texel(width, height, control, format, address, border, ix, iy);
    let b = texture_texel(width, height, control, format, address, border, ix + 1, iy);
    let c = texture_texel(width, height, control, format, address, border, ix, iy + 1);
    let d = texture_texel(width, height, control, format, address, border, ix + 1, iy + 1);
    var result = vec4<f32>(0.0);
    for (var channel = 0u; channel < 4u; channel += 1u) {
        result[channel] = (a[channel] * (1.0 - fx) + b[channel] * fx) * (1.0 - fy)
            + (c[channel] * (1.0 - fx) + d[channel] * fx) * fy;
    }
    return result;
}

fn sample_texture(case_index: u32, unit: u32, uv: vec2<f32>, lod: f32) -> vec4<f32> {
    let base = select(0x291u, 0x281u, unit == 0u);
    let levels = reg(case_index, base + 3u);
    let bias = f32(signed13(levels)) / 256.0;
    let limit = (levels >> 16u) & 15u;
    let biased_lod = lod + bias;
    let clamped_lod = clamp(biased_lod, 0.0, f32(limit));
    let minifying = biased_lod > 0.0;
    if (reg(case_index, base + 2u) & (1u << 24u)) == 0u {
        return sample_level(case_index, unit, uv, u32(floor(clamped_lod + 0.5)), minifying);
    }
    let low = u32(floor(clamped_lod));
    let a = sample_level(case_index, unit, uv, low, minifying);
    let high = min(low + 1u, limit);
    if low == high {
        return a;
    }
    let b = sample_level(case_index, unit, uv, high, minifying);
    let fraction = clamped_lod - floor(clamped_lod);
    return a + (b - a) * fraction;
}

fn combiner_source(
    code: u32,
    color: vec4<i32>,
    textures: array<vec4<i32>, 2>,
    constant: vec4<i32>,
    previous: vec4<i32>,
) -> vec4<i32> {
    if code == 0u {
        return color;
    }
    if code == 3u {
        return textures[0];
    }
    if code == 4u {
        return textures[1];
    }
    if code == 14u {
        return constant;
    }
    return previous;
}

fn rgb_operand(value: vec4<i32>, operand: u32) -> vec4<i32> {
    if operand == 1u {
        return vec4<i32>(255) - value;
    }
    if operand == 2u {
        return vec4<i32>(value.w);
    }
    if operand == 3u {
        return vec4<i32>(255 - value.w);
    }
    return value;
}

fn alpha_operand(value: vec4<i32>, operand: u32) -> i32 {
    if operand == 1u {
        return 255 - value.w;
    }
    return value.w;
}

fn combiner_operation(
    operation: u32,
    a: i32,
    b: i32,
    c: i32,
    rgb_a: vec4<i32>,
    rgb_b: vec4<i32>,
) -> i32 {
    if operation == 0u {
        return a * 255;
    }
    if operation == 1u {
        return a * b;
    }
    if operation == 2u {
        return (a + b) * 255;
    }
    if operation == 3u {
        return (a + b - 128) * 255;
    }
    if operation == 4u {
        return a * c + b * (255 - c);
    }
    if operation == 5u {
        return (a - b) * 255;
    }
    var dot = 0;
    for (var channel = 0u; channel < 3u; channel += 1u) {
        dot += (2 * rgb_a[channel] - 255) * (2 * rgb_b[channel] - 255);
    }
    return dot;
}

fn euclidean_div_255(value: i32) -> i32 {
    if value < 0 {
        return -((-value + 254) / 255);
    }
    return value / 255;
}

fn combine(case_index: u32, primary: vec4<f32>, texture0: vec4<f32>, texture1: vec4<f32>) -> vec4<f32> {
    let color = vec4<i32>(clamp(primary, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0);
    let textures = array<vec4<i32>, 2>(
        vec4<i32>(floor(clamp(texture0, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0 + 0.5)),
        vec4<i32>(floor(clamp(texture1, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0 + 0.5)),
    );
    var previous = color;
    for (var unit = 0u; unit < 2u; unit += 1u) {
        let base = 0x2c0u + unit * 8u;
        let sources = reg(case_index, base);
        let operands = reg(case_index, base + 1u);
        let operations = reg(case_index, base + 2u);
        let constant_word = reg(case_index, base + 3u);
        let constant = vec4<i32>(
            i32(constant_word & 255u),
            i32((constant_word >> 8u) & 255u),
            i32((constant_word >> 16u) & 255u),
            i32(constant_word >> 24u),
        );
        let scale = reg(case_index, base + 4u);
        var rgb: array<vec4<i32>, 3> = array<vec4<i32>, 3>(
            vec4<i32>(0),
            vec4<i32>(0),
            vec4<i32>(0),
        );
        var alpha: array<i32, 3> = array<i32, 3>(0, 0, 0);
        for (var argument = 0u; argument < 3u; argument += 1u) {
            let rgb_source = combiner_source(
                (sources >> (argument * 4u)) & 15u,
                color,
                textures,
                constant,
                previous,
            );
            rgb[argument] = rgb_operand(
                rgb_source,
                (operands >> (argument * 4u)) & 15u,
            );
            let alpha_source = combiner_source(
                (sources >> (16u + argument * 4u)) & 15u,
                color,
                textures,
                constant,
                previous,
            );
            alpha[argument] = alpha_operand(
                alpha_source,
                (operands >> (12u + argument * 4u)) & 15u,
            );
        }
        var result = vec4<i32>(0);
        for (var channel = 0u; channel < 4u; channel += 1u) {
            var operation = operations & 15u;
            if channel == 3u && operation != 7u {
                operation = (operations >> 16u) & 15u;
            }
            var value = 0;
            if channel == 3u {
                value = combiner_operation(operation, alpha[0], alpha[1], alpha[2], rgb[0], rgb[1]);
            } else {
                value = combiner_operation(operation, rgb[0][channel], rgb[1][channel], rgb[2][channel], rgb[0], rgb[1]);
            }
            let shift = select((scale >> 16u) & 3u, scale & 3u, channel != 3u);
            let scaled = value * (1 << shift) + 127;
            result[channel] = clamp(euclidean_div_255(scaled), 0, 255);
        }
        previous = result;
    }
    return vec4<f32>(previous) / 255.0;
}

fn comparison(operation: u32, a: u32, b: u32) -> bool {
    let code = operation & 7u;
    if code == 0u { return false; }
    if code == 1u { return true; }
    if code == 2u { return a == b; }
    if code == 3u { return a != b; }
    if code == 4u { return a < b; }
    if code == 5u { return a <= b; }
    if code == 6u { return a > b; }
    return a >= b;
}

fn stencil_result(operation: u32, old: u32, reference: u32) -> u32 {
    let code = operation & 7u;
    if code == 0u { return old; }
    if code == 1u { return 0u; }
    if code == 2u { return reference; }
    if code == 3u { return min(old + 1u, 255u); }
    if code == 4u { return select(old - 1u, old, old == 0u); }
    if code == 5u { return (~old) & 255u; }
    if code == 6u { return (old + 1u) & 255u; }
    return (old - 1u) & 255u;
}

fn blend_factor(code: u32, channel: u32, source: vec4<f32>, dest: vec4<f32>, constant: vec4<f32>) -> f32 {
    if code == 0u { return 0.0; }
    if code == 1u { return 1.0; }
    if code == 2u { return source[channel]; }
    if code == 3u { return 1.0 - source[channel]; }
    if code == 4u { return dest[channel]; }
    if code == 5u { return 1.0 - dest[channel]; }
    if code == 6u { return source.w; }
    if code == 7u { return 1.0 - source.w; }
    if code == 8u { return dest.w; }
    if code == 9u { return 1.0 - dest.w; }
    if code == 10u { return constant[channel]; }
    if code == 11u { return 1.0 - constant[channel]; }
    if code == 12u { return constant.w; }
    if code == 13u { return 1.0 - constant.w; }
    if channel == 3u { return 1.0; }
    return min(source.w, 1.0 - dest.w);
}

fn process_fragment(case_index: u32) {
    let x = input_word(case_index, 0u);
    let y = input_word(case_index, 1u);
    let z = bitcast<f32>(input_word(case_index, 2u));
    let coverage = bitcast<f32>(input_word(case_index, 3u));
    let primary = vec4<f32>(
        bitcast<f32>(input_word(case_index, 4u)),
        bitcast<f32>(input_word(case_index, 5u)),
        bitcast<f32>(input_word(case_index, 6u)),
        bitcast<f32>(input_word(case_index, 7u)),
    );
    let uv0 = vec2<f32>(bitcast<f32>(input_word(case_index, 8u)), bitcast<f32>(input_word(case_index, 9u)));
    let uv1 = vec2<f32>(bitcast<f32>(input_word(case_index, 10u)), bitcast<f32>(input_word(case_index, 11u)));
    let lod0 = bitcast<f32>(input_word(case_index, 12u));
    let lod1 = bitcast<f32>(input_word(case_index, 13u));

    let dimensions = reg(case_index, 0x31eu);
    let width = dimensions & 8191u;
    let height = (dimensions >> 16u) & 8191u;
    let origin = reg(case_index, 0x266u);
    let end = reg(case_index, 0x267u);
    if x < (origin & 65535u) || y < (origin >> 16u)
        || x > (end & 65535u) || y > (end >> 16u) {
        return;
    }

    var texture0 = vec4<f32>(0.0);
    var texture1 = vec4<f32>(0.0);
    let texture_enable = reg(case_index, 0x280u);
    if (texture_enable & 1u) != 0u {
        let format = reg(case_index, 0x28eu) & 15u;
        if format > 5u && format != 7u && format != 8u {
            status.words[case_index] = 0x10000u | format;
            return;
        }
        let uv = vec2<f32>(bitcast<f32>(input_word(case_index, 8u)), bitcast<f32>(input_word(case_index, 9u)));
        texture0 = sample_texture(case_index, 0u, uv, lod0);
    }
    if (texture_enable & 2u) != 0u {
        let format = reg(case_index, 0x296u) & 15u;
        if format > 5u && format != 7u && format != 8u {
            status.words[case_index] = 0x10000u | format;
            return;
        }
        let uv = vec2<f32>(bitcast<f32>(input_word(case_index, 10u)), bitcast<f32>(input_word(case_index, 11u)));
        texture1 = sample_texture(case_index, 1u, uv, lod1);
    }
    var color = combine(case_index, primary, texture0, texture1);
    color.w *= floor(coverage * 255.0) / 255.0;

    let alpha = reg(case_index, 0x304u);
    if (alpha & 1u) != 0u {
        let value = u32(clamp(color.w, 0.0, 1.0) * 255.0);
        if !comparison(alpha >> 4u, value, (alpha >> 8u) & 255u) {
            return;
        }
    }

    let fog = reg(case_index, 0x2e0u);
    if (fog & 1u) != 0u {
        var factor = 0.0;
        if (fog & 4u) != 0u {
            let position = min(clamp(z, 0.0, 1.0) * 128.0, 127.9999);
            let word = param(case_index, FOG_OFFSET + u32(position));
            let slope = signed13(word);
            factor = clamp(
                f32((word >> 13u) & 2047u) / 2048.0
                    + f32(slope) / 2048.0 * fract(position),
                0.0,
                1.0,
            );
        } else {
            let scale = f32(signed16(reg(case_index, 0x2e2u))) / 2048.0;
            let bias = f32(signed16(reg(case_index, 0x2e3u))) / 2048.0;
            factor = clamp(z * scale + bias, 0.0, 1.0);
        }
        let fog_color = reg(case_index, 0x2e1u);
        for (var channel = 0u; channel < 3u; channel += 1u) {
            let fog_component = f32((fog_color >> (channel * 8u)) & 255u) / 255.0;
            color[channel] = color[channel] * factor + fog_component * (1.0 - factor);
        }
    }

    let pixel = morton_index(x, y, width);
    let depth_control = reg(case_index, 0x307u);
    let stencil = reg(case_index, 0x305u);
    let operations = reg(case_index, 0x306u);
    if (stencil & 1u) != 0u || (depth_control & 1u) != 0u {
        let format = reg(case_index, 0x316u);
        var bytes_per_pixel = 0u;
        var maximum = 0x00ffffffu;
        if format == 0u {
            bytes_per_pixel = 2u;
            maximum = 65535u;
        } else if format == 2u {
            bytes_per_pixel = 3u;
        } else {
            bytes_per_pixel = 4u;
        }
        let depth_address = reg(case_index, 0x31cu) + pixel * bytes_per_pixel;
        var old = memory_u24(depth_address);
        if bytes_per_pixel == 2u {
            old = memory_u16(depth_address);
        } else if bytes_per_pixel == 4u {
            old = memory_u32(depth_address);
        }
        let old_stencil = (old >> 24u) & 255u;
        let reference = (stencil >> 16u) & 255u;
        let value_mask = (stencil >> 24u) & 255u;
        let stencil_pass = (stencil & 1u) == 0u || comparison(
            stencil >> 4u,
            reference & value_mask,
            old_stencil & value_mask,
        );
        let depth = u32(clamp(z, 0.0, 1.0) * f32(maximum));
        let depth_pass = (depth_control & 1u) == 0u
            || comparison(depth_control >> 4u, depth, old & maximum);
        if (stencil & 1u) != 0u && bytes_per_pixel == 4u {
            var operation = (operations >> 8u) & 7u;
            if !stencil_pass {
                operation = operations & 7u;
            } else if !depth_pass {
                operation = (operations >> 4u) & 7u;
            }
            let write_mask = (stencil >> 8u) & 255u;
            let updated = (old_stencil & ~write_mask)
                | (stencil_result(operation, old_stencil, reference) & write_mask);
            write_memory_byte(depth_address + 3u, updated);
        }
        if !stencil_pass || !depth_pass {
            return;
        }
        if (depth_control & 0x1001u) == 0x1001u {
            if bytes_per_pixel == 2u {
                write_u16(depth_address, depth);
            } else {
                write_u24(depth_address, depth);
            }
        }
    }

    if ((depth_control >> 8u) & 15u) == 0u {
        return;
    }
    let format = (reg(case_index, 0x317u) >> 16u) & 15u;
    var bytes_per_pixel = 2u;
    if format == 0u {
        bytes_per_pixel = 4u;
    }
    let color_address = reg(case_index, 0x31du) + pixel * bytes_per_pixel;
    var target_format = 4u;
    if format == 0u {
        target_format = 0u;
    } else if format == 1u {
        target_format = 3u;
    } else if format == 2u {
        target_format = 2u;
    }
    let destination = unpack_texel(color_address, target_format);
    let control = reg(case_index, 0x301u);
    let constant_word = reg(case_index, 0x303u);
    let constant = vec4<f32>(
        f32(constant_word & 255u),
        f32((constant_word >> 8u) & 255u),
        f32((constant_word >> 16u) & 255u),
        f32(constant_word >> 24u),
    ) / 255.0;
    if (reg(case_index, 0x300u) & 0x100u) != 0u {
        for (var channel = 0u; channel < 4u; channel += 1u) {
            let shift = select(16u, 24u, channel == 3u);
            let source_factor = (control >> shift) & 15u;
            let destination_factor = (control >> (shift + 4u)) & 15u;
            let equation = select(control & 255u, (control >> 8u) & 255u, channel == 3u);
            if source_factor > 14u || destination_factor > 14u {
                status.words[case_index] = 1u;
                return;
            }
            if equation > 4u {
                status.words[case_index] = 2u;
                return;
            }
        }
        let source = color;
        for (var channel = 0u; channel < 4u; channel += 1u) {
            let shift = select(16u, 24u, channel == 3u);
            let a = source[channel]
                * blend_factor((control >> shift) & 15u, channel, source, destination, constant);
            let b = destination[channel]
                * blend_factor((control >> (shift + 4u)) & 15u, channel, source, destination, constant);
            let equation = select(control & 255u, (control >> 8u) & 255u, channel == 3u);
            if equation == 0u {
                color[channel] = a + b;
            } else if equation == 1u {
                color[channel] = a - b;
            } else if equation == 2u {
                color[channel] = b - a;
            } else if equation == 3u {
                color[channel] = min(color[channel], b);
            } else {
                color[channel] = max(color[channel], b);
            }
        }
    } else if reg(case_index, 0x302u) != 3u {
        status.words[case_index] = 3u;
        return;
    }

    let mask = (depth_control >> 8u) & 15u;
    for (var channel = 0u; channel < 4u; channel += 1u) {
        if (mask & (1u << channel)) == 0u {
            color[channel] = destination[channel];
        }
    }
    if format == 0u {
        for (var channel = 0u; channel < 4u; channel += 1u) {
            if (mask & (1u << channel)) != 0u {
                let value = u32(floor(clamp(color[channel], 0.0, 1.0) * 255.0 + 0.5));
                write_memory_byte(color_address + 3u - channel, value);
            }
        }
    } else {
        let packed = clamp(color, vec4<f32>(0.0), vec4<f32>(1.0));
        var word = 0u;
        if format == 1u {
            word = (u32(packed.x * 31.0) << 11u)
                | (u32(packed.y * 63.0) << 5u)
                | u32(packed.z * 31.0);
        } else if format == 2u {
            word = (u32(packed.x * 31.0) << 11u)
                | (u32(packed.y * 31.0) << 6u)
                | (u32(packed.z * 31.0) << 1u)
                | u32(packed.w);
        } else {
            word = (u32(packed.x * 15.0) << 12u)
                | (u32(packed.y * 15.0) << 8u)
                | (u32(packed.z * 15.0) << 4u)
                | u32(packed.w * 15.0);
        }
        var bits = 0u;
        for (var channel = 0u; channel < 4u; channel += 1u) {
            if (mask & (1u << channel)) != 0u {
                if format == 1u {
                    if channel == 0u { bits |= 0xf800u; }
                    if channel == 1u { bits |= 0x07e0u; }
                    if channel == 2u { bits |= 0x001fu; }
                } else if format == 2u {
                    if channel == 0u { bits |= 0xf800u; }
                    if channel == 1u { bits |= 0x07c0u; }
                    if channel == 2u { bits |= 0x003eu; }
                    if channel == 3u { bits |= 0x0001u; }
                } else {
                    if channel == 0u { bits |= 0xf000u; }
                    if channel == 1u { bits |= 0x0f00u; }
                    if channel == 2u { bits |= 0x00f0u; }
                    if channel == 3u { bits |= 0x000fu; }
                }
            }
        }
        let previous = memory_u16(color_address);
        write_u16(color_address, (previous & ~bits) | (word & bits));
    }
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let case_count = arrayLength(&params.words) / CASE_WORDS;
    if id.x >= case_count {
        return;
    }
    process_fragment(id.x);
}
