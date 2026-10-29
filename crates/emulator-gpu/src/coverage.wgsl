struct CoverageJob {
    bounds: vec4<u32>,
    parameters: vec4<u32>,
    stroke_radius: i32,
    coverage_offset: u32,
    padding: vec2<u32>,
}

struct CoverageEdge {
    high: vec4<f32>,
    low: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> jobs: array<CoverageJob>;
@group(0) @binding(1) var<storage, read> edges: array<CoverageEdge>;
@group(0) @binding(2) var<storage, read_write> coverage: array<u32>;

fn edge_in_grid(index: u32) -> vec4<i32> {
    return vec4<i32>(round(edges[index].high * 8.0));
}

fn two_sum(a: f32, b: f32) -> vec2<f32> {
    let sum = a + b;
    let other = sum - a;
    let error = (a - (sum - other)) + (b - other);
    return vec2<f32>(sum, error);
}

fn float_add(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let sum = two_sum(a.x, b.x);
    return two_sum(sum.x, sum.y + a.y + b.y);
}

fn float_sub(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return float_add(a, -b);
}

fn float_mul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let product = a.x * b.x;
    let error = fma(a.x, b.x, -product) + a.x * b.y + a.y * b.x + a.y * b.y;
    return two_sum(product, error);
}

fn float_greater(a: vec2<f32>, b: vec2<f32>) -> bool {
    let difference = float_sub(a, b);
    return difference.x > 0.0 || (difference.x == 0.0 && difference.y > 0.0);
}

fn float_less_equal(a: vec2<f32>, b: vec2<f32>) -> bool {
    return !float_greater(a, b);
}

fn multiply_u32(a: u32, b: u32) -> vec2<u32> {
    let a_high = a >> 16u;
    let a_low = a & 0xffffu;
    let b_high = b >> 16u;
    let b_low = b & 0xffffu;
    let first = a_low * b_low;
    let middle = a_high * b_low + (first >> 16u);
    let middle_low = middle & 0xffffu;
    let middle_high = middle >> 16u;
    let upper_middle = a_low * b_high + middle_low;
    let high = a_high * b_high + middle_high + (upper_middle >> 16u);
    let low = (upper_middle << 16u) | (first & 0xffffu);
    return vec2<u32>(low, high);
}

fn less_equal_u64(a: vec2<u32>, b: vec2<u32>) -> bool {
    return a.y < b.y || (a.y == b.y && a.x <= b.x);
}

fn add_u64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let low = a.x + b.x;
    let carry = select(0u, 1u, low < a.x);
    return vec2<u32>(low, a.y + b.y + carry);
}

fn square_u64(value: i32) -> vec2<u32> {
    let magnitude = u32(abs(value));
    return multiply_u32(magnitude, magnitude);
}

fn stroke_covers(point: vec2<i32>, edge: vec4<i32>, radius: i32) -> bool {
    if radius < 0 {
        return false;
    }
    let a = edge.xy;
    let b = edge.zw;
    let direction = b - a;
    let offset = point - a;
    let length_squared = direction.x * direction.x + direction.y * direction.y;
    let radius_squared = radius * radius;
    if length_squared == 0 {
        let distance_squared = add_u64(square_u64(offset.x), square_u64(offset.y));
        return less_equal_u64(distance_squared, square_u64(radius));
    }

    let projection = offset.x * direction.x + offset.y * direction.y;
    if projection <= 0 {
        let distance_squared = add_u64(square_u64(offset.x), square_u64(offset.y));
        return less_equal_u64(distance_squared, square_u64(radius));
    }
    if projection >= length_squared {
        let end_offset = point - b;
        let distance_squared = add_u64(square_u64(end_offset.x), square_u64(end_offset.y));
        return less_equal_u64(distance_squared, square_u64(radius));
    }

    let cross = direction.x * offset.y - direction.y * offset.x;
    return less_equal_u64(
        square_u64(cross),
        multiply_u32(u32(radius_squared), u32(length_squared)),
    );
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let job = jobs[id.z];
    if id.x >= job.bounds.z || id.y >= job.bounds.w {
        return;
    }

    let x = job.bounds.x + id.x;
    let y = job.bounds.y + id.y;
    let samples = job.parameters.x;
    let edge_offset = job.parameters.y;
    let edge_count = job.parameters.z;
    let stroke_enabled = (job.parameters.w & 1u) != 0u;
    let nonzero = (job.parameters.w & 2u) != 0u;
    let float_pixel_edges = (job.parameters.w & 4u) != 0u;
    let radius = job.stroke_radius;
    var pixel_coverage = 0u;

    for (var sy = 0u; sy < samples; sy += 1u) {
        let py = i32(y * 8u + ((sy * 2u + 1u) * 4u) / samples);
        for (var sx = 0u; sx < samples; sx += 1u) {
            let px = i32(x * 8u + ((sx * 2u + 1u) * 4u) / samples);
            let point = vec2<i32>(px, py);
            var covered = false;

            if stroke_enabled {
                for (var edge_index = 0u; edge_index < edge_count; edge_index += 1u) {
                    if stroke_covers(point, edge_in_grid(edge_offset + edge_index), radius) {
                        covered = true;
                        break;
                    }
                }
            } else {
                var winding = 0i;
                for (var edge_index = 0u; edge_index < edge_count; edge_index += 1u) {
                    if float_pixel_edges {
                        let edge = edges[edge_offset + edge_index];
                        let a_x = vec2<f32>(edge.high.x, edge.low.x);
                        let a_y = vec2<f32>(edge.high.y, edge.low.y);
                        let b_x = vec2<f32>(edge.high.z, edge.low.z);
                        let b_y = vec2<f32>(edge.high.w, edge.low.w);
                        let local_px = id.x * 8u + ((sx * 2u + 1u) * 4u) / samples;
                        let local_py = id.y * 8u + ((sy * 2u + 1u) * 4u) / samples;
                        let sample_x = vec2<f32>(f32(local_px) / 8.0, 0.0);
                        let sample_y = vec2<f32>(f32(local_py) / 8.0, 0.0);
                        let upward = float_greater(b_y, a_y);
                        let crosses_row = (float_less_equal(a_y, sample_y)
                                && float_greater(b_y, sample_y))
                            || (float_less_equal(b_y, sample_y)
                                && float_greater(a_y, sample_y));
                        if crosses_row {
                            let delta_x = float_sub(b_x, a_x);
                            if delta_x.x == 0.0 && delta_x.y == 0.0 {
                                if float_greater(a_x, sample_x) {
                                    winding += select(-1, 1, upward);
                                }
                            } else {
                                let relative = float_sub(
                                    float_mul(delta_x, float_sub(sample_y, a_y)),
                                    float_mul(
                                        float_sub(b_y, a_y),
                                        float_sub(sample_x, a_x),
                                    ),
                                );
                                let positive = relative.x > 0.0
                                    || (relative.x == 0.0 && relative.y > 0.0);
                                let negative = relative.x < 0.0
                                    || (relative.x == 0.0 && relative.y < 0.0);
                                if (upward && positive) || (!upward && negative) {
                                    winding += select(-1, 1, upward);
                                }
                            }
                        }
                    } else {
                        let edge = edge_in_grid(edge_offset + edge_index);
                        let a = edge.xy;
                        let b = edge.zw;
                        let upward = b.y > a.y;
                        let crosses_row = (a.y <= py && b.y > py) || (b.y <= py && a.y > py);
                        if crosses_row {
                            let relative = (b.x - a.x) * (py - a.y)
                                - (b.y - a.y) * (px - a.x);
                            if (upward && relative > 0) || (!upward && relative < 0) {
                                winding += select(-1, 1, upward);
                            }
                        }
                    }
                }
                covered = select((winding & 1) != 0, winding != 0, nonzero);
            }

            pixel_coverage += select(0u, 1u, covered);
        }
    }

    coverage[job.coverage_offset + id.y * job.bounds.z + id.x] = pixel_coverage;
}
