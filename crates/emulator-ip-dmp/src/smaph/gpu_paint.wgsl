@group(0) @binding(0) var<storage, read> params: array<u32>;
@group(0) @binding(1) var<storage, read> coverage: array<u32>;
@group(0) @binding(2) var<storage, read> data: array<u32>;
@group(0) @binding(3) var<storage, read_write> output: array<u32>;
@group(0) @binding(4) var resident_target: texture_2d<u32>;

fn image_byte(offset: u32) -> u32 {
    let word = data[params[19] + offset / 4u];
    return (word >> ((offset & 3u) * 8u)) & 255u;
}

fn pixel_index(x: u32, y: u32, width: u32, tiled: bool) -> u32 {
    if !tiled {
        return y * width + x;
    }
    return ((x & 4u) << 2u)
        + ((x & 0xfffffff8u) << 3u)
        + ((x * 2u) & 4u)
        + (x & 1u)
        + (y & 4u) * 8u
        + (y & 0xfffffff8u) * width
        + ((y * 2u) & 2u)
        + ((y & 2u) << 2u);
}

fn premultiply(color: vec4<u32>) -> vec4<u32> {
    var result = color;
    let alpha = color.a;
    for (var channel = 0u; channel < 3u; channel += 1u) {
        let value = color[channel] * alpha + 128u;
        result[channel] = (value + (value >> 8u)) >> 8u;
    }
    return result;
}

fn unpack_color(word: u32) -> vec4<u32> {
    return vec4<u32>(word & 255u, (word >> 8u) & 255u, (word >> 16u) & 255u, word >> 24u);
}

fn pack_color(color: vec4<u32>) -> u32 {
    return color.r | (color.g << 8u) | (color.b << 16u) | (color.a << 24u);
}

fn packed_color(input_color: vec4<u32>) -> vec4<u32> {
    var color = input_color;
    if params[58] != 0u {
        for (var channel = 0u; channel < 4u; channel += 1u) {
            let scale = bitcast<i32>(params[59u + channel * 2u]);
            let bias = bitcast<i32>(params[60u + channel * 2u]);
            let value = (i32(color[channel]) * scale + bias + 64) >> 7;
            color[channel] = u32(clamp(value, 0, 255));
        }
    }
    if params[14] != 0u {
        for (var channel = 0u; channel < 3u; channel += 1u) {
            color[channel] = data[params[17] + color[channel]];
        }
    }
    if (params[7] & 256u) == 0u {
        color = premultiply(color);
    }
    if (params[7] >> 24u != 1u) {
        return color.wzyx;
    }
    return color;
}

fn expand(value: u32, bits: u32) -> u32 {
    let maximum = (1u << bits) - 1u;
    return (value * 255u + maximum / 2u) / maximum;
}

fn texture_color(x: u32, y: u32) -> vec4<u32> {
    let width = params[23];
    let height = params[24];
    let matrix = 32u;
    let px = f32(x) + 0.5;
    let py = f32(y) + 0.5;
    let tx = (bitcast<f32>(params[matrix]) * px
        + bitcast<f32>(params[matrix + 1u]) * py
        + bitcast<f32>(params[matrix + 2u])) * f32(width);
    let ty = (bitcast<f32>(params[matrix + 3u]) * px
        + bitcast<f32>(params[matrix + 4u]) * py
        + bitcast<f32>(params[matrix + 5u])) * f32(height);
    if tx < 0.0 || ty < 0.0 || tx >= f32(params[25]) || ty >= f32(params[26]) {
        return vec4<u32>(0u);
    }

    let cell = pixel_index(u32(tx), u32(ty), width, true);
    let format = params[27];
    if format == 7u {
        let value = image_byte(cell);
        return vec4<u32>(value, value, value, 255u);
    }
    if format == 8u {
        return vec4<u32>(255u, 255u, 255u, image_byte(cell));
    }
    if format == 2u || format == 3u || format == 4u {
        var word = image_byte(cell * 2u)
            | (image_byte(cell * 2u + 1u) << 8u);
        let alpha_bits = select(0u, select(4u, 1u, format == 2u), format == 2u || format == 4u);
        if params[30] != 0u {
            word = ((word << alpha_bits) | (word >> (16u - alpha_bits))) & 65535u;
        }
        var color = vec4<u32>(0u);
        if format == 2u {
            color = vec4<u32>(expand(word >> 11u, 5u), expand((word >> 6u) & 31u, 5u),
                expand((word >> 1u) & 31u, 5u), expand(word & 1u, 1u));
        } else if format == 3u {
            color = vec4<u32>(expand(word >> 11u, 5u), expand((word >> 5u) & 63u, 6u),
                expand(word & 31u, 5u), 255u);
        } else {
            color = vec4<u32>(expand(word >> 12u, 4u), expand((word >> 8u) & 15u, 4u),
                expand((word >> 4u) & 15u, 4u), expand(word & 15u, 4u));
        }
        if (params[29] != 0u && params[29] != 27u && params[29] != 147u) {
            return vec4<u32>(0u);
        }
        if ((params[29] == 147u) != (params[30] != 0u)) {
            color = color.zyxw;
        }
        return color;
    }

    let b0 = image_byte(cell * 4u);
    let b1 = image_byte(cell * 4u + 1u);
    let b2 = image_byte(cell * 4u + 2u);
    let b3 = image_byte(cell * 4u + 3u);
    var color = vec4<u32>(0u);
    if (params[29] == 0u || params[29] == 27u) {
        if params[30] == 0u {
            color = vec4<u32>(b3, b2, b1, b0);
        } else {
            color = vec4<u32>(b0, b1, b2, b3);
        }
    } else if params[29] == 147u {
        if params[30] == 0u {
            color = vec4<u32>(b1, b2, b3, b0);
        } else {
            color = vec4<u32>(b2, b1, b0, b3);
        }
    } else {
        return vec4<u32>(0u);
    }
    if params[31] != 0u {
        color.a = 255u;
    }
    if params[28] != 0u && color.a != 0u {
        for (var channel = 0u; channel < 3u; channel += 1u) {
            color[channel] = min(color[channel] * 255u / color.a, 255u);
        }
    }
    return color;
}

fn gradient_color(x: u32, y: u32) -> vec4<u32> {
    let m = 49u;
    let px = f32(x) + 0.5;
    let py = f32(y) + 0.5;
    let w = bitcast<f32>(params[m + 6u]) * px
        + bitcast<f32>(params[m + 7u]) * py
        + bitcast<f32>(params[m + 8u]);
    var t = 0.0;
    if params[41] != 0u && w != 0.0 {
        let a = (bitcast<f32>(params[m]) * px
            + bitcast<f32>(params[m + 1u]) * py
            + bitcast<f32>(params[m + 2u])) / w;
        let b = (bitcast<f32>(params[m + 3u]) * px
            + bitcast<f32>(params[m + 4u]) * py
            + bitcast<f32>(params[m + 5u])) / w;
        if params[44] != 0u {
            let flags = params[45];
            let cross = select(0.0, 2.0 * a * b, (flags & 0x400000u) != 0u);
            let root = sqrt(max(bitcast<f32>(params[46]) * a * a
                + bitcast<f32>(params[47]) * b * b + cross, 0.0));
            let linear_a = select(a, 0.0, (flags & 0x10000000u) != 0u);
            let linear_b = select(b, 0.0, (flags & 0x20000000u) != 0u);
            t = (root + linear_a + linear_b) * bitcast<f32>(params[48]);
        } else {
            t = a + b;
        }
    }
    if params[42] == 1u {
        t = t - floor(t);
    } else if params[42] == 2u {
        let value = t - floor(t / 2.0) * 2.0;
        t = select(value, 2.0 - value, value > 1.0);
    } else {
        t = clamp(t, 0.0, 1.0);
    }
    let index = min(u32(floor(t * 127.0 + 0.5)), 127u);
    var color = unpack_color(data[params[20] + index]);
    if params[43] != 0u && color.a != 0u {
        for (var channel = 0u; channel < 3u; channel += 1u) {
            color[channel] = min(color[channel] * 255u / color.a, 255u);
        }
    }
    return color;
}

struct Soft64 { mantissa: vec2<u32>, exponent: i32, }

fn uzero(v: vec2<u32>) -> bool { return v.x == 0u && v.y == 0u; }
fn uge(a: vec2<u32>, b: vec2<u32>) -> bool { return a.y > b.y || (a.y == b.y && a.x >= b.x); }
fn uadd(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let lo = a.x + b.x;
    return vec2<u32>(lo, a.y + b.y + select(0u, 1u, lo < a.x));
}
fn usub(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    return vec2<u32>(a.x - b.x, a.y - b.y - select(0u, 1u, a.x < b.x));
}
fn uinc(a: vec2<u32>) -> vec2<u32> {
    let lo = a.x + 1u;
    return vec2<u32>(lo, a.y + select(0u, 1u, lo == 0u));
}
fn ubit(a: vec2<u32>, bit: u32) -> bool {
    if bit < 32u { return ((a.x >> bit) & 1u) != 0u; }
    return ((a.y >> (bit - 32u)) & 1u) != 0u;
}
fn ushl(a: vec2<u32>, n: u32) -> vec2<u32> {
    if n == 0u { return a; }
    if n < 32u { return vec2<u32>(a.x << n, (a.y << n) | (a.x >> (32u - n))); }
    if n < 64u { return vec2<u32>(0u, a.x << (n - 32u)); }
    return vec2<u32>(0u);
}
fn ushr(a: vec2<u32>, n: u32) -> vec2<u32> {
    if n == 0u { return a; }
    if n < 32u { return vec2<u32>((a.x >> n) | (a.y << (32u - n)), a.y >> n); }
    if n < 64u { return vec2<u32>(a.y >> (n - 32u), 0u); }
    return vec2<u32>(0u);
}
fn ushr_sticky(a: vec2<u32>, n: u32) -> vec2<u32> {
    var out = ushr(a, n);
    var lost = false;
    if n < 32u { lost = (a.x & ((1u << n) - 1u)) != 0u; }
    else if n == 32u { lost = a.x != 0u; }
    else if n < 64u { lost = a.x != 0u || (a.y & ((1u << (n - 32u)) - 1u)) != 0u; }
    else { lost = !uzero(a); }
    out.x |= select(0u, 1u, lost);
    return out;
}
fn soft_zero() -> Soft64 { return Soft64(vec2<u32>(0u), 0); }
fn soft_from_u32(v: u32) -> Soft64 {
    if v == 0u { return soft_zero(); }
    var e = 0i; var rest = v;
    loop { if rest <= 1u { break; } rest >>= 1u; e += 1; }
    return Soft64(ushl(vec2<u32>(v, 0u), u32(52 - e)), e);
}
fn soft_zero_value(v: Soft64) -> bool { return uzero(v.mantissa); }
fn soft_less(a: Soft64, b: Soft64) -> bool {
    if soft_zero_value(a) { return !soft_zero_value(b); }
    if soft_zero_value(b) { return false; }
    return a.exponent < b.exponent || (a.exponent == b.exponent &&
        (a.mantissa.y < b.mantissa.y || (a.mantissa.y == b.mantissa.y && a.mantissa.x < b.mantissa.x)));
}
fn soft_pack(input_ext: vec2<u32>, input_exponent: i32) -> Soft64 {
    var ext = input_ext;
    var e = input_exponent;
    if uzero(ext) { return soft_zero(); }
    if ubit(ext, 56u) { ext = ushr_sticky(ext, 1u); e += 1; }
    else { loop { if ubit(ext, 55u) { break; } ext = ushl(ext, 1u); e -= 1; } }
    var m = ushr(ext, 3u);
    let tail = ext.x & 7u;
    if (tail & 4u) != 0u && ((tail & 3u) != 0u || (m.x & 1u) != 0u) { m = uinc(m); }
    if ubit(m, 53u) { m = ushr(m, 1u); e += 1; }
    return Soft64(m, e);
}
fn soft_add(a: Soft64, b: Soft64) -> Soft64 {
    if soft_zero_value(a) { return b; }
    if soft_zero_value(b) { return a; }
    var hi = a; var lo = b;
    if soft_less(hi, lo) { hi = b; lo = a; }
    let ext_hi = ushl(hi.mantissa, 3u);
    let ext_lo = ushr_sticky(ushl(lo.mantissa, 3u), u32(hi.exponent - lo.exponent));
    return soft_pack(uadd(ext_hi, ext_lo), hi.exponent);
}
fn soft_sub(a: Soft64, b: Soft64) -> Soft64 {
    if soft_zero_value(b) { return a; }
    if soft_less(a, b) { return soft_zero(); }
    if a.exponent == b.exponent && all(a.mantissa == b.mantissa) { return soft_zero(); }
    let ext_a = ushl(a.mantissa, 3u);
    let ext_b = ushr_sticky(ushl(b.mantissa, 3u), u32(a.exponent - b.exponent));
    return soft_pack(usub(ext_a, ext_b), a.exponent);
}
fn soft_mul(a: Soft64, b: Soft64) -> Soft64 {
    if soft_zero_value(a) || soft_zero_value(b) { return soft_zero(); }
    let al = array<u32,4>(a.mantissa.x & 65535u, a.mantissa.x >> 16u, a.mantissa.y & 65535u, a.mantissa.y >> 16u);
    let bl = array<u32,4>(b.mantissa.x & 65535u, b.mantissa.x >> 16u, b.mantissa.y & 65535u, b.mantissa.y >> 16u);
    var p = array<u32,8>(0u,0u,0u,0u,0u,0u,0u,0u);
    for (var i=0u;i<4u;i+=1u) {
        var carry=0u;
        for (var j=0u;j<4u;j+=1u) { let k=i+j; let t=p[k]+al[i]*bl[j]+carry; p[k]=t&65535u; carry=t>>16u; }
        p[i+4u]=carry;
    }
    let upper=(p[6]&512u)!=0u; let shift=select(49u,50u,upper); let wi=shift/16u; let bs=shift&15u;
    var l=array<u32,4>(0u,0u,0u,0u);
    for (var i=0u;i<4u;i+=1u) { let k=wi+i; var v=p[k]>>bs; if bs!=0u && k+1u<8u { v|=p[k+1u]<<(16u-bs); } l[i]=v&65535u; }
    var ext=vec2<u32>(l[0]|(l[1]<<16u),l[2]|(l[3]<<16u));
    var sticky=false; for(var bit=0u;bit<shift;bit+=1u){if(p[bit/16u]&(1u<<(bit&15u)))!=0u{sticky=true;}}
    ext.x|=select(0u,1u,sticky);
    return soft_pack(ext,a.exponent+b.exponent+select(0,1,upper));
}
fn soft_div(a: Soft64, b: Soft64) -> Soft64 {
    if soft_zero_value(a) || soft_zero_value(b) { return soft_zero(); }
    let ge=uge(a.mantissa,b.mantissa); var e=a.exponent-b.exponent; var rem=a.mantissa;
    var q=vec2<u32>(0u); var loops=56u;
    if ge { rem=usub(rem,b.mantissa); q.x=1u; loops=55u; } else { e-=1; }
    for(var i=0u;i<loops;i+=1u){rem=ushl(rem,1u);var bit=0u;if uge(rem,b.mantissa){rem=usub(rem,b.mantissa);bit=1u;}q=ushl(q,1u);q.x|=bit;}
    q.x|=select(0u,1u,!uzero(rem));
    return soft_pack(q,e);
}
fn soft_min(a: Soft64, b: Soft64) -> Soft64 {
    if soft_less(a, b) { return a; }
    return b;
}
fn soft_max(a: Soft64, b: Soft64) -> Soft64 {
    if soft_less(a, b) { return b; }
    return a;
}
fn soft_round_byte(v: Soft64)->u32 {
    let scaled=soft_mul(v,soft_from_u32(255u)); if soft_zero_value(scaled){return 0u;}
    let shift=u32(52-scaled.exponent); if shift>64u{return 0u;}
    var integer=ushr(scaled.mantissa,shift); if shift>0u && ubit(scaled.mantissa,shift-1u){integer=uinc(integer);}
    if integer.y!=0u || integer.x>255u{return 255u;} return integer.x;
}

fn blend_pixel(source_word: u32, destination_word: u32, covered: u32) -> u32 {
    if covered == params[6] * params[6] && params[8] == 0u { return source_word; }
    let order = params[7];
    var source = unpack_color(source_word); var destination = unpack_color(destination_word);
    if order >> 24u != 1u { source = source.wzyx; destination = destination.wzyx; }
    let zero=soft_zero(); let one=soft_from_u32(1u); let scale=soft_from_u32(255u);
    let sa=soft_div(soft_from_u32(source.a),scale); let da=soft_div(soft_from_u32(destination.a),scale);
    let amount=soft_div(soft_from_u32(covered),soft_from_u32(params[6]*params[6])); let inverse=soft_sub(one,amount);
    let mode=params[8]; var alpha=sa;
    if mode==2u { alpha=soft_add(soft_mul(sa,soft_sub(one,da)),da); }
    else if mode==3u || mode==4u { alpha=soft_mul(sa,da); }
    else if mode==9u { alpha=soft_min(soft_add(sa,da),one); }
    else if mode!=0u { alpha=soft_sub(soft_add(sa,da),soft_mul(sa,da)); }
    alpha=soft_add(soft_mul(alpha,amount),soft_mul(da,inverse));
    var result=vec4<u32>(0u,0u,0u,soft_round_byte(alpha));
    for(var channel=0u;channel<3u;channel+=1u){
        var s=soft_div(soft_from_u32(source[channel]),scale); var d=soft_div(soft_from_u32(destination[channel]),scale);
        if (order&256u)!=0u { s=soft_mul(s,sa); d=soft_mul(d,da); }
        var blended=s;
        if mode==1u { blended=soft_add(s,soft_mul(d,soft_sub(one,sa))); }
        else if mode==2u { blended=soft_add(soft_mul(s,soft_sub(one,da)),d); }
        else if mode==3u { blended=soft_mul(s,da); }
        else if mode==4u { blended=soft_mul(d,sa); }
        else if mode==5u { blended=soft_add(soft_add(soft_mul(s,soft_sub(one,da)),soft_mul(d,soft_sub(one,sa))),soft_mul(s,d)); }
        else if mode==6u { blended=soft_sub(soft_add(s,d),soft_mul(s,d)); }
        else if mode==7u { blended=soft_add(soft_add(soft_mul(s,soft_sub(one,da)),soft_mul(d,soft_sub(one,sa))),soft_min(soft_mul(s,da),soft_mul(d,sa))); }
        else if mode==8u { blended=soft_add(soft_add(soft_mul(s,soft_sub(one,da)),soft_mul(d,soft_sub(one,sa))),soft_max(soft_mul(s,da),soft_mul(d,sa))); }
        else if mode==9u { blended=soft_min(soft_add(s,d),one); }
        var value=soft_add(soft_mul(blended,amount),soft_mul(d,inverse));
        if (order&256u)!=0u && !soft_zero_value(alpha) { value=soft_div(value,alpha); }
        result[channel]=soft_round_byte(value);
    }
    if order>>24u==1u{return pack_color(result);} return pack_color(result.wzyx);
}


@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params[2] || id.y >= params[3] {
        return;
    }
    let x = params[0] + id.x;
    let y = params[1] + id.y;
    let output_row = select(id.y, params[3] - 1u - id.y, params[66] != 0u && params[10] != 0u);
    let output_index = output_row * params[67] + id.x;
    let destination_y = select(y, params[5] - 1u - y, params[10] != 0u);
    let target_index = pixel_index(x, destination_y, params[4], params[9] != 0u);
    var destination_word = 0u;
    if params[66] != 0u {
        destination_word = textureLoad(resident_target, vec2<i32>(i32(x), i32(destination_y)), 0).x;
    } else {
        destination_word = data[params[18] + target_index];
    }
    var inside_scissor = params[11] == 0u;
    for (var index = 0u; index < params[11]; index += 1u) {
        let offset = params[12] + index * 4u;
        let rect = vec4<u32>(data[offset], data[offset + 1u], data[offset + 2u], data[offset + 3u]);
        inside_scissor = inside_scissor || (x >= rect.x && x < rect.z && y >= rect.y && y < rect.w);
    }
    let covered = coverage[output_index];
    if !inside_scissor || covered == 0u {
        output[output_index] = destination_word;
        return;
    }

    var source = unpack_color(params[16]);
    if params[15] == 1u {
        source = texture_color(x, y);
    } else if params[15] == 2u {
        source = gradient_color(x, y);
    }
    source = packed_color(source);
    output[output_index] = blend_pixel(pack_color(source), destination_word, covered);
}
