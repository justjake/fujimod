struct SurfaceParams {
    width: u32,
    height: u32,
    storage_width: u32,
    storage_height: u32,
    tiles_per_row: u32,
    bytes_per_pixel: u32,
    write_count: u32,
    reserved: u32,
};

struct GuestBytes {
    words: array<u32>,
};

struct PixelWriteMask {
    x: u32,
    y: u32,
    byte_mask: u32,
};

struct PixelWriteMasks {
    values: array<atomic<u32>>,
};

struct ReadbackPixel {
    pixel: u32,
    byte_mask: u32,
};

struct ReadbackPixels {
    values: array<ReadbackPixel>,
};

@group(0) @binding(0) var<uniform> params: SurfaceParams;

fn morton_interleave(x: u32, y: u32) -> u32 {
    let x3 = x & 7u;
    let y3 = y & 7u;
    return (x3 & 1u)
        | ((y3 & 1u) << 1u)
        | ((x3 & 2u) << 1u)
        | ((y3 & 2u) << 2u)
        | ((x3 & 4u) << 2u)
        | ((y3 & 4u) << 3u);
}

fn morton_pixel_offset(x: u32, y: u32) -> u32 {
    let tile_x = x / 8u;
    let tile_y = y / 8u;
    let tile_index = tile_y * params.tiles_per_row + tile_x;
    return tile_index * 64u + morton_interleave(x, y);
}

fn guest_pixel_offset(x: u32, y: u32) -> u32 {
    if params.reserved == 0u {
        return y * params.width + x;
    }
    return morton_pixel_offset(x, y);
}

@group(0) @binding(1) var<storage, read> guest: GuestBytes;
@group(0) @binding(2) var surface_out: texture_storage_2d<r32uint, write>;

fn guest_byte(byte_offset: u32) -> u32 {
    let word = guest.words[byte_offset >> 2u];
    return (word >> ((byte_offset & 3u) * 8u)) & 255u;
}

@compute @workgroup_size(8, 8)
fn upload_morton(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let position = invocation.xy;
    if (position.x >= params.storage_width || position.y >= params.storage_height) {
        return;
    }
    let pixel_offset = guest_pixel_offset(position.x, position.y);
    let base_byte = pixel_offset * params.bytes_per_pixel;
    var pixel = 0u;
    for (var lane = 0u; lane < params.bytes_per_pixel; lane += 1u) {
        pixel |= guest_byte(base_byte + lane) << (lane * 8u);
    }
    textureStore(surface_out, vec2<i32>(position), vec4<u32>(pixel, 0u, 0u, 0u));
}

@group(0) @binding(3) var<storage, read_write> masks: PixelWriteMasks;
@group(0) @binding(4) var mask_out: texture_storage_2d<r32uint, write>;

@compute @workgroup_size(8, 8)
fn clear_write_mask(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let position = invocation.xy;
    if (position.x >= params.storage_width || position.y >= params.storage_height) {
        return;
    }
    let linear_offset = position.y * params.width + position.x;
    atomicStore(&masks.values[linear_offset], 0u);
    textureStore(mask_out, vec2<i32>(position), vec4<u32>(0u, 0u, 0u, 0u));
}

@group(0) @binding(5) var<storage, read> writes: array<PixelWriteMask>;
@group(0) @binding(6) var<storage, read_write> write_masks: PixelWriteMasks;

@compute @workgroup_size(64)
fn record_write_masks(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if (index >= params.write_count) {
        return;
    }
    let write = writes[index];
    let linear_offset = write.y * params.width + write.x;
    atomicOr(&write_masks.values[linear_offset], write.byte_mask);
}

@group(0) @binding(12) var draw_mask_in: texture_2d<u32>;
@group(0) @binding(13) var<storage, read_write> draw_write_masks: PixelWriteMasks;

@compute @workgroup_size(8, 8)
fn accumulate_draw_mask(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let position = invocation.xy;
    if (position.x >= params.storage_width || position.y >= params.storage_height) {
        return;
    }
    let byte_mask = textureLoad(draw_mask_in, vec2<i32>(position), 0).x;
    if (byte_mask != 0u) {
        let linear_offset = position.y * params.width + position.x;
        atomicOr(&draw_write_masks.values[linear_offset], byte_mask);
    }
}

@group(0) @binding(7) var<storage, read_write> resolved_masks: PixelWriteMasks;
@group(0) @binding(8) var resolved_mask_out: texture_storage_2d<r32uint, write>;

@compute @workgroup_size(8, 8)
fn resolve_write_mask(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let position = invocation.xy;
    if (position.x >= params.storage_width || position.y >= params.storage_height) {
        return;
    }
    let linear_offset = position.y * params.width + position.x;
    let byte_mask = atomicLoad(&resolved_masks.values[linear_offset]);
    textureStore(
        resolved_mask_out,
        vec2<i32>(position),
        vec4<u32>(byte_mask, 0u, 0u, 0u),
    );
}

@group(0) @binding(9) var surface_in: texture_2d<u32>;
@group(0) @binding(10) var mask_in: texture_2d<u32>;
@group(0) @binding(11) var<storage, read_write> readback: ReadbackPixels;

@compute @workgroup_size(8, 8)
fn readback_morton(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let position = invocation.xy;
    if (position.x >= params.storage_width || position.y >= params.storage_height) {
        return;
    }
    let guest_offset = guest_pixel_offset(position.x, position.y);
    let pixel = textureLoad(surface_in, vec2<i32>(position), 0).x;
    let byte_mask = textureLoad(mask_in, vec2<i32>(position), 0).x;
    readback.values[guest_offset] = ReadbackPixel(pixel, byte_mask);
}
