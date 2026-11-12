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
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

mod buffers;
#[cfg(not(target_arch = "wasm32"))]
mod compiler;
mod fragment_program;
#[cfg(not(target_arch = "wasm32"))]
mod native_blend;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum FragmentTarget {
    Packed,
    NormalizedRgba8,
}
mod textures;
use buffers::DrawBuffers;
use fragment_program::FragmentProgram;
use textures::TextureUploads;

const REGISTER_WORDS: usize = 0x500;
const FOG_OFFSET: usize = REGISTER_WORDS;
const FOG_WORDS: usize = 128;
const INPUT_OFFSET: usize = FOG_OFFSET + FOG_WORDS;
const CASE_WORDS: usize = INPUT_OFFSET + 16;

static DRAW_PIPELINES: OnceLock<
    Mutex<HashMap<([u8; 32], FragmentProgram), Arc<wgpu::RenderPipeline>>>,
> = OnceLock::new();
static VERTEX_SHADER_MODULES: OnceLock<Mutex<HashMap<[u8; 32], Arc<wgpu::ShaderModule>>>> =
    OnceLock::new();
static FRAGMENT_SHADER_MODULES: OnceLock<
    Mutex<HashMap<(bool, FragmentTarget), Arc<wgpu::ShaderModule>>>,
> = OnceLock::new();
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

struct DrawSegment {
    start: usize,
    end: usize,
    refresh_addresses: Vec<u64>,
}

struct PreparedDraw {
    state: DrawState,
    extent: SurfaceExtent,
    color_key: SurfaceKey,
    depth_key: SurfaceKey,
    parameters: Vec<u32>,
    guest_bytes: Arc<Vec<u8>>,
    vertex: gpu_vertex::PreparedDraw,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn render(
    draws: &[Draw],
    recorded_reads: &[(u64, Vec<u8>)],
    expected_snapshots: Option<&Snapshots>,
    read: &mut Reader<'_>,
) -> Result<RenderedList, String> {
    let segments = draw_segments(draws)?;
    if segments.len() > 3 {
        return Err(format!(
            "DMP host GPU command list requires {} submissions (limit 3)",
            segments.len()
        ));
    }
    if segments.is_empty() {
        return Ok(RenderedList {
            snapshots: Vec::new(),
            written_ranges: Vec::new(),
            queue_submissions: 0,
            draws: 0,
        });
    }

    let cache = context()?;
    let _submission = cache.acquire()?;
    let mut current_reads = recorded_reads.to_vec();
    let mut snapshots = BTreeMap::new();
    let mut written_ranges = Vec::new();
    let mut queue_submissions = 0u8;
    let mut draw_count = 0usize;
    for segment in segments {
        let mut requested = expected_snapshots.cloned().unwrap_or_default();
        requested.extend(
            segment
                .refresh_addresses
                .iter()
                .map(|&address| (address, vec![])),
        );
        let requested_snapshots = (!requested.is_empty()).then_some(requested);
        let rendered = render_segment(
            &draws[segment.start..segment.end],
            &current_reads,
            requested_snapshots.as_ref(),
            read,
            cache,
        )?;
        queue_submissions = queue_submissions
            .checked_add(rendered.queue_submissions)
            .ok_or("DMP host GPU queue submission count overflow")?;
        if queue_submissions > 3 {
            return Err(format!(
                "DMP host GPU submitted {queue_submissions} command buffers for one list (limit 3)"
            ));
        }
        draw_count += rendered.draws;
        written_ranges.extend(rendered.written_ranges);
        for (address, bytes) in rendered.snapshots {
            current_reads.push((address, bytes.clone()));
            snapshots.insert(address, bytes);
        }
    }
    Ok(RenderedList {
        snapshots: snapshots.into_iter().collect(),
        written_ranges: merge_written_ranges(written_ranges),
        queue_submissions,
        draws: draw_count,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn render_segment(
    draws: &[Draw],
    recorded_reads: &[(u64, Vec<u8>)],
    expected_snapshots: Option<&Snapshots>,
    read: &mut Reader<'_>,
    cache: &GpuContext,
) -> Result<RenderedList, String> {
    let timing = std::env::var_os("FUJIMOD_DMP_DRAW_TIMING").map(|_| std::time::Instant::now());
    let mut prepared = Vec::new();
    let mut surfaces = BTreeMap::<u64, (SurfaceKey, Option<Vec<u8>>)>::new();
    let mut texture_uploads = TextureUploads::default();
    for draw in draws {
        if draw.vertex.indices.is_empty() {
            continue;
        }
        let state = &draw.state;
        let (extent, color_key, depth_key) = draw_surface_keys(state)?;
        prepare_surface(&mut surfaces, color_key, true, recorded_reads, read)?;

        let load_depth_from_guest = state.depth_test.enabled || state.stencil_test.enabled;
        prepare_surface(
            &mut surfaces,
            depth_key,
            load_depth_from_guest,
            recorded_reads,
            read,
        )?;

        let (parameters, guest_bytes) =
            draw_parameters(draw, recorded_reads, read, &mut texture_uploads)?;
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

    let prepared_at = timing.map(|_| std::time::Instant::now());
    let mut encoder = cache
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("DMP command list"),
        });
    let mut needed = HashSet::new();
    for draw in &prepared {
        needed.insert(draw.color_key);
        #[cfg(not(target_arch = "wasm32"))]
        let needs_depth_attachment = native_blend::mode(&draw.state).is_none();
        #[cfg(target_arch = "wasm32")]
        let needs_depth_attachment = true;
        if needs_depth_attachment {
            needed.insert(draw.depth_key);
        }
    }
    for (key, bytes) in surfaces.values().filter(|(key, _)| needed.contains(key)) {
        cache.get_or_create_target_encoded(&mut encoder, *key, bytes.as_deref())?;
        cache.encode_clear_write_masks(&mut encoder, *key)?;
    }
    let mut snapshots = HashMap::new();
    let buffers = DrawBuffers::new(cache.device(), &prepared)?;
    let mut index = 0;
    while index < prepared.len() {
        #[cfg(not(target_arch = "wasm32"))]
        if native_blend::mode(&prepared[index].state).is_some() {
            let key = prepared[index].color_key;
            let end = (index + 1..prepared.len())
                .find(|&next| {
                    prepared[next].color_key != key
                        || native_blend::mode(&prepared[next].state).is_none()
                })
                .unwrap_or(prepared.len());
            native_blend::render(&prepared[index..end], index, cache, &buffers, &mut encoder)?;
            index = end;
            continue;
        }
        let draw = &prepared[index];
        let pipeline = pipeline_for(cache.device(), &draw.state, &draw.vertex, &draw.parameters)?;
        render_draw(
            draw,
            &pipeline,
            cache,
            &mut encoder,
            &mut snapshots,
            &buffers,
            index,
        )?;
        index += 1;
    }
    // A disabled depth/stencil stage cannot write this surface. Preserve the
    // resident target, but do not transfer its entire extent back to the host.
    let keys = surfaces
        .values()
        .filter_map(|(key, _)| {
            prepared
                .iter()
                .any(|draw| {
                    draw.color_key == *key
                        || (draw.depth_key == *key
                            && (draw.state.depth_test.enabled || draw.state.stencil_test.enabled))
                })
                .then_some(*key)
        })
        .collect::<Vec<_>>();
    let pending = cache.encode_readbacks(&mut encoder, &keys)?;
    let encoded_at = timing.map(|_| std::time::Instant::now());
    cache.submit(encoder.finish());
    let readbacks = cache.finish_readbacks(pending)?;

    if let (Some(start), Some(prepared_at), Some(encoded_at)) = (timing, prepared_at, encoded_at) {
        let native = prepared
            .iter()
            .filter(|draw| native_blend::mode(&draw.state).is_some())
            .count();
        let bytes: usize = prepared.iter().map(|draw| draw.guest_bytes.len()).sum();
        let mut packed = BTreeMap::<String, usize>::new();
        for draw in &prepared {
            if native_blend::mode(&draw.state).is_none() {
                let s = &draw.state;
                let key = format!(
                    "format={:?} mask={:?} depth={} stencil={} fog={} blend={:?}",
                    s.targets.color_format,
                    s.color_mask,
                    s.depth_test.enabled,
                    s.stencil_test.enabled,
                    s.fog.enabled,
                    s.blend
                );
                *packed.entry(key).or_default() += 1;
            }
        }
        for (reason, count) in packed {
            eprintln!("DMP_PACKED count={count} {reason}");
        }
        eprintln!(
            "DMP_DRAW draws={} native={} texture_bytes={} prepare_ms={:.3} encode_ms={:.3} submit_readback_ms={:.3}",
            prepared.len(),
            native,
            bytes,
            (prepared_at - start).as_secs_f64() * 1000.,
            (encoded_at - prepared_at).as_secs_f64() * 1000.,
            encoded_at.elapsed().as_secs_f64() * 1000.
        );
    }
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

fn draw_segments(draws: &[Draw]) -> Result<Vec<DrawSegment>, String> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut dirty_surfaces = Vec::<SurfaceKey>::new();
    let mut known_surfaces = BTreeMap::<u64, SurfaceKey>::new();
    for (index, draw) in draws.iter().enumerate() {
        if draw.vertex.indices.is_empty() {
            continue;
        }
        let (_, color_key, depth_key) = draw_surface_keys(&draw.state)?;
        remember_surface_key(&mut known_surfaces, color_key)?;
        if draw.state.depth_test.enabled || draw.state.stencil_test.enabled {
            remember_surface_key(&mut known_surfaces, depth_key)?;
        }
        let textures = texture_ranges(&draw.state)?;
        let mut refresh = BTreeMap::<u64, SurfaceKey>::new();
        for surface in &dirty_surfaces {
            let surface_range = surface_range(*surface)?;
            if textures
                .iter()
                .any(|texture_range| ranges_overlap(*texture_range, surface_range))
            {
                refresh.insert(surface.guest_address, *surface);
            }
        }
        if !refresh.is_empty() {
            segments.push(DrawSegment {
                start,
                end: index,
                refresh_addresses: refresh.keys().copied().collect(),
            });
            start = index;
            dirty_surfaces.clear();
        }

        if draw.state.color_mask.0.iter().any(|enabled| *enabled)
            && !dirty_surfaces.contains(&color_key)
        {
            dirty_surfaces.push(color_key);
        }
        if ((draw.state.depth_test.enabled && draw.state.depth_test.write_enabled)
            || (draw.state.stencil_test.enabled && draw.state.stencil_test.write_mask != 0))
            && !dirty_surfaces.contains(&depth_key)
        {
            dirty_surfaces.push(depth_key);
        }
    }
    if start < draws.len() {
        segments.push(DrawSegment {
            start,
            end: draws.len(),
            refresh_addresses: Vec::new(),
        });
    }
    Ok(segments)
}

fn remember_surface_key(
    surfaces: &mut BTreeMap<u64, SurfaceKey>,
    key: SurfaceKey,
) -> Result<(), String> {
    if let Some(existing) = surfaces.get(&key.guest_address) {
        if *existing != key {
            return Err(format!(
                "DMP host GPU target {:#x} changes format or extent within one list",
                key.guest_address
            ));
        }
    } else {
        surfaces.insert(key.guest_address, key);
    }
    Ok(())
}

fn draw_surface_keys(state: &DrawState) -> Result<(SurfaceExtent, SurfaceKey, SurfaceKey), String> {
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
    Ok((extent, color_key, depth_key))
}

fn texture_ranges(state: &DrawState) -> Result<Vec<(u64, usize)>, String> {
    let mut ranges = Vec::new();
    for texture in state.textures.iter().filter(|texture| texture.enabled) {
        let bytes_per_pixel = texture.format.bytes_per_pixel()?;
        if texture.width == 0
            || texture.height == 0
            || !texture.width.is_multiple_of(8)
            || !texture.height.is_multiple_of(8)
        {
            return Err("DMP draw texture extent is invalid".into());
        }
        let level_size = |width: usize, height: usize| {
            width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
                .ok_or_else(|| "DMP draw texture size overflow".to_string())
        };
        ranges.push((
            texture.base_address,
            level_size(texture.width, texture.height)?,
        ));
        let mut offset = 0usize;
        for level in 1..=texture.max_level {
            let width = texture.width >> level;
            let height = texture.height >> level;
            if width == 0 || height == 0 || !width.is_multiple_of(8) || !height.is_multiple_of(8) {
                break;
            }
            let address = texture
                .mip_address
                .checked_add(offset as u64)
                .ok_or("DMP draw mip address overflow")?;
            let size = level_size(width, height)?;
            ranges.push((address, size));
            offset = offset
                .checked_add(size)
                .ok_or("DMP draw mip extent overflow")?;
        }
    }
    Ok(ranges)
}

fn surface_range(key: SurfaceKey) -> Result<(u64, usize), String> {
    let byte_len = (key.extent.width as usize)
        .checked_mul(key.extent.height as usize)
        .and_then(|pixels| pixels.checked_mul(key.format.bytes_per_pixel() as usize))
        .ok_or("DMP host GPU surface byte length overflow")?;
    Ok((key.guest_address, byte_len))
}

fn ranges_overlap(left: (u64, usize), right: (u64, usize)) -> bool {
    let Some(left_end) = left.0.checked_add(left.1 as u64) else {
        return true;
    };
    let Some(right_end) = right.0.checked_add(right.1 as u64) else {
        return true;
    };
    left.0 < right_end && right.0 < left_end
}

fn merge_written_ranges(mut ranges: Vec<(u64, usize)>) -> Vec<(u64, usize)> {
    ranges.sort_unstable_by_key(|(address, _)| *address);
    let mut merged = Vec::<(u64, usize)>::new();
    for (address, length) in ranges {
        let end = address.saturating_add(length as u64);
        if let Some((previous_address, previous_length)) = merged.last_mut() {
            let previous_end = previous_address.saturating_add(*previous_length as u64);
            if address <= previous_end {
                *previous_length = previous_end.max(end).saturating_sub(*previous_address) as usize;
                continue;
            }
        }
        merged.push((address, length));
    }
    merged
}

pub(super) fn recorded_memory_range(
    reads: &[(u64, Vec<u8>)],
    address: u64,
    length: usize,
    read: &mut Reader<'_>,
) -> Result<Vec<u8>, String> {
    let end = address
        .checked_add(length as u64)
        .ok_or("DMP recorded memory range overflow")?;
    let mut bytes = recorded_range(reads, address, length)
        .map(Ok)
        .unwrap_or_else(|| read(address, length))?;
    if bytes.len() != length {
        return Ok(bytes);
    }
    for (base, source) in reads {
        let Some(source_end) = base.checked_add(source.len() as u64) else {
            continue;
        };
        let start = address.max(*base);
        let finish = end.min(source_end);
        if start >= finish {
            continue;
        }
        let destination_start = usize::try_from(start - address)
            .map_err(|_| "DMP recorded memory destination offset overflow")?;
        let source_start = usize::try_from(start - base)
            .map_err(|_| "DMP recorded memory source offset overflow")?;
        let count = usize::try_from(finish - start)
            .map_err(|_| "DMP recorded memory overlay extent overflow")?;
        bytes[destination_start..destination_start + count]
            .copy_from_slice(&source[source_start..source_start + count]);
    }
    Ok(bytes)
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(super) async fn render_async(
    draws: &[Draw],
    recorded_reads: &[(u64, Vec<u8>)],
    read: &mut Reader<'_>,
) -> Result<RenderedList, String> {
    let mut prepared = Vec::new();
    let mut surfaces = BTreeMap::<u64, (SurfaceKey, Option<Vec<u8>>)>::new();
    let mut texture_uploads = TextureUploads::default();
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
        let (parameters, guest_bytes) =
            draw_parameters(draw, recorded_reads, read, &mut texture_uploads)?;
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
    let validation_scope = cache
        .device()
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = cache
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("DMP command list"),
        });
    let mut needed = HashSet::new();
    for draw in &prepared {
        needed.insert(draw.color_key);
        #[cfg(not(target_arch = "wasm32"))]
        let needs_depth_attachment = native_blend::mode(&draw.state).is_none();
        #[cfg(target_arch = "wasm32")]
        let needs_depth_attachment = true;
        if needs_depth_attachment {
            needed.insert(draw.depth_key);
        }
    }
    for (key, bytes) in surfaces.values().filter(|(key, _)| needed.contains(key)) {
        cache.get_or_create_target_encoded(&mut encoder, *key, bytes.as_deref())?;
        cache.encode_clear_write_masks(&mut encoder, *key)?;
    }
    let mut snapshots = HashMap::new();
    let buffers = DrawBuffers::new(cache.device(), &prepared)?;
    let mut index = 0;
    while index < prepared.len() {
        #[cfg(not(target_arch = "wasm32"))]
        if native_blend::mode(&prepared[index].state).is_some() {
            let key = prepared[index].color_key;
            let end = (index + 1..prepared.len())
                .find(|&next| {
                    prepared[next].color_key != key
                        || native_blend::mode(&prepared[next].state).is_none()
                })
                .unwrap_or(prepared.len());
            native_blend::render(&prepared[index..end], index, cache, &buffers, &mut encoder)?;
            index = end;
            continue;
        }
        let draw = &prepared[index];
        let pipeline = pipeline_for(cache.device(), &draw.state, &draw.vertex, &draw.parameters)?;
        render_draw(
            draw,
            &pipeline,
            cache,
            &mut encoder,
            &mut snapshots,
            &buffers,
            index,
        )?;
        index += 1;
    }
    // A disabled depth/stencil stage cannot write this surface. Preserve the
    // resident target, but do not transfer its entire extent back to the host.
    let keys = surfaces
        .values()
        .filter_map(|(key, _)| {
            prepared
                .iter()
                .any(|draw| {
                    draw.color_key == *key
                        || (draw.depth_key == *key
                            && (draw.state.depth_test.enabled || draw.state.stencil_test.enabled))
                })
                .then_some(*key)
        })
        .collect::<Vec<_>>();
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
        let bytes = recorded_memory_range(recorded_reads, key.guest_address, byte_len, read)?;
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
    snapshots: &mut HashMap<SurfaceKey, wgpu::Texture>,
    buffers: &DrawBuffers,
    index: usize,
) -> Result<(), String> {
    let extent = draw.extent;
    let scissor = draw.state.scissor;
    let left = u32::from(scissor.left).min(extent.width);
    let top = u32::from(scissor.top).min(extent.height);
    let right = (u32::from(scissor.right) + 1).min(extent.width);
    let bottom = (u32::from(scissor.bottom) + 1).min(extent.height);
    if right <= left || bottom <= top {
        return Ok(());
    }
    let bounds = [left, top, right - left, bottom - top];
    let color_key = draw.color_key;
    let depth_key = draw.depth_key;
    let color_surface = cache
        .get(color_key)?
        .ok_or("DMP color surface is not resident")?;
    let depth_surface = cache
        .get(depth_key)?
        .ok_or("DMP depth surface is not resident")?;

    let device = cache.device();
    let color_input =
        copy_surface_texture(device, encoder, &color_surface, extent, bounds, snapshots);
    let depth_input =
        copy_surface_texture(device, encoder, &depth_surface, extent, bounds, snapshots);
    let color_input_view = color_input.create_view(&Default::default());
    let depth_input_view = depth_input.create_view(&Default::default());
    let color_view = color_surface.texture().create_view(&Default::default());
    let depth_view = depth_surface.texture().create_view(&Default::default());
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("DMP render bindings"),
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
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&color_input_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&depth_input_view),
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
                resource: color_surface.write_mask_accumulator().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: depth_surface.write_mask_accumulator().as_entire_binding(),
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
        pass.set_scissor_rect(left, top, right - left, bottom - top);
        pass.set_index_buffer(buffers.indices(index), wgpu::IndexFormat::Uint32);
        let index_count = u32::try_from(draw.vertex.indices.len())
            .map_err(|_| "DMP host GPU index count exceeds u32")?;
        pass.draw_indexed(0..index_count, 0, 0..1);
    }
    Ok(())
}

fn pipeline_for(
    device: &wgpu::Device,
    state: &DrawState,
    vertex: &gpu_vertex::PreparedDraw,
    parameters: &[u32],
) -> Result<Arc<wgpu::RenderPipeline>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    let program = FragmentProgram::new(state.raster.flat_shading, parameters);
    #[cfg(target_arch = "wasm32")]
    let program = FragmentProgram::generic(state.raster.flat_shading);
    let key = (vertex.pipeline_key, program.clone());
    if let Some(pipeline) = DRAW_PIPELINES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "DMP host GPU render pipeline lock poisoned")?
        .get(&key)
    {
        return Ok(Arc::clone(pipeline));
    }
    // The shared generic equations remain usable while the host compiler
    // specializes a new control configuration away from the execution owner.
    let pipeline = cached_pipeline(
        device,
        state,
        vertex,
        &FragmentProgram::generic(state.raster.flat_shading),
    )?;
    #[cfg(not(target_arch = "wasm32"))]
    compiler::request(device, state, vertex, program);
    Ok(pipeline)
}

fn cached_pipeline(
    device: &wgpu::Device,
    state: &DrawState,
    vertex: &gpu_vertex::PreparedDraw,
    program: &FragmentProgram,
) -> Result<Arc<wgpu::RenderPipeline>, String> {
    let key = (vertex.pipeline_key, program.clone());
    match state.raster.primitive_mode {
        super::draw_state::PrimitiveMode::Triangles
        | super::draw_state::PrimitiveMode::TriangleStrip
        | super::draw_state::PrimitiveMode::TriangleFan => {}
        _ => return Err("DMP host GPU pipeline received a non-triangle mode".into()),
    }
    let pipelines = DRAW_PIPELINES.get_or_init(|| Mutex::new(HashMap::new()));
    let pipelines = pipelines
        .lock()
        .map_err(|_| "DMP host GPU render pipeline lock poisoned")?;
    if let Some(pipeline) = pipelines.get(&key) {
        return Ok(Arc::clone(pipeline));
    }

    drop(pipelines);
    let vertex_shader = vertex_shader_module(device, vertex)?;
    let fragment_shader =
        fragment_shader_module(device, state.raster.flat_shading, FragmentTarget::Packed)?;
    let pipeline = Arc::new(create_pipeline(
        device,
        &vertex_shader,
        &fragment_shader,
        state,
        vertex.topology,
        program,
    ));
    PIPELINE_CREATIONS.fetch_add(1, Ordering::Relaxed);
    DRAW_PIPELINES
        .get()
        .unwrap()
        .lock()
        .map_err(|_| "DMP host GPU render pipeline lock poisoned")?
        .insert(key, Arc::clone(&pipeline));
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
    target: FragmentTarget,
) -> Result<Arc<wgpu::ShaderModule>, String> {
    let modules = FRAGMENT_SHADER_MODULES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut modules = modules
        .lock()
        .map_err(|_| "DMP host GPU fragment shader module cache lock poisoned")?;
    if let Some(module) = modules.get(&(flat_shading, target)) {
        return Ok(Arc::clone(module));
    }
    let entry = match target {
        FragmentTarget::Packed => include_str!("gpu_draw.wgsl"),
        FragmentTarget::NormalizedRgba8 => include_str!("gpu_draw/native_fragment.wgsl"),
    };
    let fragment_source = format!(
        "{}\n{}\n{}",
        include_str!("gpu_fragment.wgsl"),
        include_str!("draw_fragment.wgsl"),
        entry
    );
    let fragment_source = if target == FragmentTarget::Packed {
        FragmentProgram::shader(fragment_source)
    } else {
        fragment_source
    };
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
    modules.insert((flat_shading, target), Arc::clone(&module));
    FRAGMENT_MODULE_CREATIONS.fetch_add(1, Ordering::Relaxed);
    Ok(module)
}

fn create_pipeline(
    device: &wgpu::Device,
    vertex_shader: &wgpu::ShaderModule,
    fragment_shader: &wgpu::ShaderModule,
    state: &DrawState,
    topology: wgpu::PrimitiveTopology,
    program: &FragmentProgram,
) -> wgpu::RenderPipeline {
    let constants = program.constants();
    let constants: Vec<_> = constants
        .iter()
        .map(|(name, value)| (name.as_str(), *value))
        .collect();
    let targets = [Some(color_target()), Some(color_target())];
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
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
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
    bounds: [u32; 4],
    snapshots: &mut HashMap<SurfaceKey, wgpu::Texture>,
) -> wgpu::Texture {
    let texture = snapshots.entry(surface.key()).or_insert_with(|| {
        device.create_texture(&wgpu::TextureDescriptor {
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
        })
    });
    // Destination reads use only the fragment's own pixel. Hardware scissor
    // bounds therefore also bound the feedback snapshot, for arbitrary shaders.
    let [left, top, width, height] = bounds;
    let origin = wgpu::Origin3d {
        x: left,
        y: top,
        z: 0,
    };
    let mut source = surface.texture().as_image_copy();
    source.origin = origin;
    let mut destination = texture.as_image_copy();
    destination.origin = origin;
    encoder.copy_texture_to_texture(
        source,
        destination,
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    texture.clone()
}

fn draw_parameters(
    primitive: &Draw,
    recorded_reads: &[(u64, Vec<u8>)],
    read: &mut Reader<'_>,
    textures: &mut TextureUploads,
) -> Result<(Vec<u32>, Arc<Vec<u8>>), String> {
    let mut registers = vec![0u32; REGISTER_WORDS];
    for &(index, value) in &primitive.registers {
        registers[index as usize] = value;
    }
    let guest = textures.prepare(&primitive.state, &mut registers, recorded_reads, read)?;
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

fn encode_words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_blend_tests;

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn prepare(context: &GpuContext) -> Result<(), String> {
    native_blend::prepare(context)
}
