//! Prepares guest vertex inputs for the host GPU render pipeline.

use super::{Engine, Reader, draw_state::DrawState, shader::Vector};
use crate::pica::{VERTEX_OUTPUT_SEMANTICS, translate_vertex_to_wgsl_for_draw};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

const MAX_VERTEX_BYTES: usize = 64 * 1024 * 1024;
const MAX_REMAP_ENTRIES: usize = 1 << 24;

struct PackedVertices {
    bytes: Vec<u8>,
    draw_indices: Vec<u32>,
    vertex_remap: Vec<u32>,
    attributes: [[u32; 4]; 16],
    defaults: [[f32; 4]; 16],
}

#[derive(Clone, Copy)]
struct SlotSource {
    group: usize,
    byte_offset: usize,
    format: super::draw_state::AttributeFormat,
}

struct GroupPlan {
    byte_offset: usize,
    stride: usize,
}

#[derive(Clone, Debug)]
pub(super) struct PreparedDraw {
    pub(super) shader_source: Arc<str>,
    pub(super) vertex_module_key: [u8; 32],
    pub(super) pipeline_key: [u8; 32],
    pub(super) shading_bytes: Vec<u8>,
    pub(super) input_bytes: Vec<u8>,
    pub(super) indices: Vec<u32>,
    pub(super) topology: wgpu::PrimitiveTopology,
}

static TRANSLATED_VERTEX_SHADERS: OnceLock<Mutex<HashMap<[u8; 32], Arc<str>>>> = OnceLock::new();
static VERTEX_TRANSLATIONS: AtomicUsize = AtomicUsize::new(0);

pub(super) fn translation_count() -> usize {
    VERTEX_TRANSLATIONS.load(Ordering::Relaxed)
}

pub(super) fn prepare_draw(
    engine: &Engine,
    state: &DrawState,
    vertex_count: usize,
    indexed: bool,
    read: &mut Reader<'_>,
) -> Result<PreparedDraw, String> {
    if vertex_count == 0 {
        return Ok(PreparedDraw {
            shader_source: Arc::from(""),
            vertex_module_key: [0; 32],
            pipeline_key: [0; 32],
            shading_bytes: Vec::new(),
            input_bytes: vec![0; 4],
            indices: Vec::new(),
            topology: primitive_topology(state.raster.primitive_mode)?,
        });
    }
    if vertex_count > 1_048_576 {
        return Err("DMP host GPU vertex count exceeds bound".into());
    }

    let draw_indices = if indexed {
        let index_address = engine.reg(0x427);
        let width = if index_address >> 31 != 0 { 2 } else { 1 };
        let byte_count = vertex_count
            .checked_mul(width)
            .ok_or("DMP host GPU index byte length overflow")?;
        let address = state
            .vertex
            .base_address
            .checked_add(u64::from(index_address & 0x7fff_ffff))
            .ok_or("DMP host GPU index address overflow")?;
        let bytes = read(address, byte_count)?;
        if bytes.len() != byte_count {
            return Err("DMP host GPU short index DMA".into());
        }
        if width == 2 {
            bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]) as u32)
                .collect()
        } else {
            bytes.into_iter().map(u32::from).collect()
        }
    } else {
        let start = engine.reg(0x42a);
        (0..vertex_count)
            .map(|offset| {
                start
                    .checked_add(offset as u32)
                    .ok_or("DMP host GPU vertex index overflow")
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    let packed = pack_draw_vertices(engine, state, &draw_indices, read)?;
    prepare_packed_draw(engine, state, packed)
}

pub(super) fn prepare_immediate_draw(
    engine: &Engine,
    state: &DrawState,
    vertices: &[[Vector; 16]],
) -> Result<PreparedDraw, String> {
    let stride = 16 * 4 * std::mem::size_of::<f32>();
    let mut bytes = Vec::with_capacity(vertices.len() * stride);
    for vertex in vertices {
        for vector in vertex {
            for component in vector {
                bytes.extend_from_slice(&component.to_le_bytes());
            }
        }
    }
    let attributes = std::array::from_fn(|register| {
        [
            (register * 4 * std::mem::size_of::<f32>()) as u32,
            stride as u32,
            (4 << 16) | (3 << 8),
            0,
        ]
    });
    let defaults = [[0.0, 0.0, 0.0, 1.0]; 16];
    let indices = (0..vertices.len())
        .map(|index| u32::try_from(index).map_err(|_| "DMP immediate vertex index overflow"))
        .collect::<Result<Vec<_>, _>>()?;
    let packed = PackedVertices {
        bytes,
        draw_indices: indices.clone(),
        vertex_remap: indices.clone(),
        attributes,
        defaults,
    };
    prepare_packed_draw(engine, state, packed)
}

fn prepare_packed_draw(
    engine: &Engine,
    state: &DrawState,
    packed: PackedVertices,
) -> Result<PreparedDraw, String> {
    let semantics = draw_vertex_semantics(state);
    let (vertex_module_key, shader_source) = translated_shader(engine, state, &semantics)?;
    let pipeline_key = pipeline_key(vertex_module_key, state)?;

    let draw_indices = packed
        .draw_indices
        .iter()
        .map(|index| {
            packed
                .vertex_remap
                .get(*index as usize)
                .copied()
                .ok_or("DMP host GPU vertex index remap outside buffer")
                .and_then(|index| {
                    u32::try_from(index).map_err(|_| "DMP host GPU remapped vertex exceeds u32")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mode = state.raster.primitive_mode;
    let expand = state.raster.flat_shading || mode == super::draw_state::PrimitiveMode::TriangleFan;
    let (topology, mut indices) = if expand {
        let mut triangles = Vec::with_capacity(draw_indices.len().saturating_sub(2) * 3);
        match mode {
            super::draw_state::PrimitiveMode::Triangles => {
                triangles.extend(draw_indices.chunks_exact(3).flatten().copied());
            }
            super::draw_state::PrimitiveMode::TriangleStrip => {
                for index in 2..draw_indices.len() {
                    if index & 1 == 0 {
                        triangles.extend([
                            draw_indices[index - 2],
                            draw_indices[index - 1],
                            draw_indices[index],
                        ]);
                    } else {
                        triangles.extend([
                            draw_indices[index - 1],
                            draw_indices[index - 2],
                            draw_indices[index],
                        ]);
                    }
                }
            }
            super::draw_state::PrimitiveMode::TriangleFan => {
                for index in 2..draw_indices.len() {
                    triangles.extend([
                        draw_indices[0],
                        draw_indices[index - 1],
                        draw_indices[index],
                    ]);
                }
            }
            other => return Err(format!("DMP unsupported host GPU primitive mode {other:?}")),
        }
        (wgpu::PrimitiveTopology::TriangleList, triangles)
    } else {
        (primitive_topology(mode)?, draw_indices)
    };
    if state.raster.flat_shading {
        for triangle in indices.chunks_exact_mut(3) {
            triangle.rotate_left(2);
        }
    }

    Ok(PreparedDraw {
        shader_source,
        vertex_module_key,
        pipeline_key,
        shading_bytes: shading_bytes(engine, state, packed.attributes, packed.defaults),
        input_bytes: packed.bytes,
        indices,
        topology,
    })
}

fn translated_shader(
    engine: &Engine,
    state: &DrawState,
    semantics: &[u32; VERTEX_OUTPUT_SEMANTICS],
) -> Result<([u8; 32], Arc<str>), String> {
    let entry_point = engine.reg(0x48a) & 0xffff;
    let key = vertex_module_key(
        &engine.code,
        &engine.operands,
        entry_point,
        semantics,
        state.raster.flat_shading,
    );
    let cache = TRANSLATED_VERTEX_SHADERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .map_err(|_| "DMP translated vertex shader cache lock poisoned")?;
    if let Some(source) = cache.get(&key) {
        return Ok((key, Arc::clone(source)));
    }

    let source: Arc<str> = translate_vertex_to_wgsl_for_draw(
        &engine.code,
        &engine.operands,
        entry_point,
        semantics,
        state.raster.flat_shading,
    )?
    .into();
    cache.insert(key, Arc::clone(&source));
    VERTEX_TRANSLATIONS.fetch_add(1, Ordering::Relaxed);
    Ok((key, source))
}

fn vertex_module_key(
    code: &[u32],
    operands: &[u32],
    entry_point: u32,
    semantics: &[u32; VERTEX_OUTPUT_SEMANTICS],
    flat_shading: bool,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"fujimod-dmp-vertex-module-v1");
    hash.update((code.len() as u64).to_le_bytes());
    for word in code {
        hash.update(word.to_le_bytes());
    }
    hash.update((operands.len() as u64).to_le_bytes());
    for word in operands {
        hash.update(word.to_le_bytes());
    }
    hash.update(entry_point.to_le_bytes());
    for semantic in semantics {
        hash.update(semantic.to_le_bytes());
    }
    hash.update([flat_shading as u8]);
    hash.finalize().into()
}

fn pipeline_key(vertex_module_key: [u8; 32], state: &DrawState) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    hash.update(b"fujimod-dmp-render-pipeline-v1");
    hash.update(vertex_module_key);
    match state.raster.cull_mode {
        super::draw_state::CullMode::Disabled => hash.update([0]),
        super::draw_state::CullMode::Front => hash.update([1]),
        super::draw_state::CullMode::Back => hash.update([2]),
        super::draw_state::CullMode::Other(code) => {
            hash.update([3]);
            hash.update(code.to_le_bytes());
        }
    }
    hash.update([match state.raster.primitive_mode {
        super::draw_state::PrimitiveMode::Triangles => 0,
        super::draw_state::PrimitiveMode::TriangleStrip => 1,
        super::draw_state::PrimitiveMode::TriangleFan => 2,
        other => return Err(format!("DMP unsupported host GPU primitive mode {other:?}")),
    }]);
    Ok(hash.finalize().into())
}

fn primitive_topology(
    mode: super::draw_state::PrimitiveMode,
) -> Result<wgpu::PrimitiveTopology, String> {
    match mode {
        super::draw_state::PrimitiveMode::Triangles => Ok(wgpu::PrimitiveTopology::TriangleList),
        super::draw_state::PrimitiveMode::TriangleStrip => {
            Ok(wgpu::PrimitiveTopology::TriangleStrip)
        }
        super::draw_state::PrimitiveMode::TriangleFan => Ok(wgpu::PrimitiveTopology::TriangleList),
        other => Err(format!(
            "DMP host GPU draw does not support primitive mode {other:?}"
        )),
    }
}

fn pack_draw_vertices(
    engine: &Engine,
    state: &DrawState,
    draw_indices: &[u32],
    read: &mut Reader<'_>,
) -> Result<PackedVertices, String> {
    let unique = draw_indices.iter().copied().collect::<BTreeSet<_>>();
    let max_vertex = unique.last().copied().unwrap_or(0) as usize;
    let remap_length = max_vertex
        .checked_add(1)
        .ok_or("DMP host GPU vertex remap overflow")?;
    if remap_length > MAX_REMAP_ENTRIES {
        return Err("DMP host GPU vertex remap exceeds bound".into());
    }
    let mut vertex_remap = vec![0; remap_length];
    for (row, vertex) in unique.iter().copied().enumerate() {
        vertex_remap[vertex as usize] =
            u32::try_from(row).map_err(|_| "DMP host GPU vertex row overflow")?;
    }

    let mut groups = Vec::new();
    let mut slot_sources = [None; 8];
    for (group_index, group) in state.vertex.groups.iter().enumerate() {
        let count = group.attribute_count as usize;
        if count == 0 {
            groups.push(None);
            continue;
        }
        if count > 8 {
            return Err("DMP host GPU fetch group exceeds slot count".into());
        }
        if group.stride == 0 {
            return Err("DMP host GPU zero vertex fetch stride".into());
        }
        let mut at = 0usize;
        for item in 0..count {
            let slot = ((group.order >> (item * 4)) & 15) as usize;
            if slot >= 12 {
                at = at
                    .checked_add(3)
                    .ok_or("DMP host GPU attribute offset overflow")?
                    & !3;
                at = at
                    .checked_add((slot - 11) * 4)
                    .ok_or("DMP host GPU attribute offset overflow")?;
                continue;
            }
            if slot >= 8 {
                return Err("DMP host GPU unsupported attribute format bank".into());
            }
            let format = state.vertex.formats[slot];
            let width = format.bytes_per_component();
            at = at
                .checked_add(width - 1)
                .ok_or("DMP host GPU attribute offset overflow")?
                & !(width - 1);
            let source = SlotSource {
                group: group_index,
                byte_offset: at,
                format,
            };
            at = at
                .checked_add(format.components as usize * width)
                .ok_or("DMP host GPU attribute extent overflow")?;
            if at > group.stride {
                return Err("DMP host GPU vertex fetch exceeds stride".into());
            }
            slot_sources[slot] = Some(source);
        }
        groups.push(Some(GroupPlan {
            byte_offset: 0,
            stride: group.stride,
        }));
    }

    let unique = unique.into_iter().collect::<Vec<_>>();
    let mut bytes = Vec::new();
    for (group_index, plan) in groups.iter_mut().enumerate() {
        let Some(plan) = plan else {
            continue;
        };
        bytes.resize((bytes.len() + 3) & !3, 0);
        plan.byte_offset = bytes.len();
        let group_bytes = unique
            .len()
            .checked_mul(plan.stride)
            .ok_or("DMP host GPU vertex group byte length overflow")?;
        let end = bytes
            .len()
            .checked_add(group_bytes)
            .ok_or("DMP host GPU vertex buffer length overflow")?;
        if end > MAX_VERTEX_BYTES {
            return Err("DMP host GPU vertex buffer exceeds bound".into());
        }
        bytes.resize(end, 0);
        for (row, vertex) in unique.iter().copied().enumerate() {
            let address = state
                .vertex
                .base_address
                .checked_add(state.vertex.groups[group_index].byte_offset)
                .and_then(|base| {
                    (vertex as u64)
                        .checked_mul(plan.stride as u64)
                        .and_then(|offset| base.checked_add(offset))
                })
                .ok_or("DMP host GPU vertex address overflow")?;
            let row_start = plan.byte_offset + row * plan.stride;
            let row_end = row_start + plan.stride;
            let fetched = read(address, plan.stride)?;
            if fetched.len() != plan.stride {
                return Err("DMP host GPU short vertex DMA".into());
            }
            bytes[row_start..row_end].copy_from_slice(&fetched);
        }
    }
    if bytes.is_empty() {
        bytes.resize(4, 0);
    } else {
        bytes.resize((bytes.len() + 3) & !3, 0);
    }

    let mut mapped_slots = [None; 16];
    for (slot, input) in state.vertex.input_map.iter().copied().enumerate() {
        mapped_slots[input as usize] = Some(slot);
    }
    let mut attributes = [[0; 4]; 16];
    let mut defaults = [[0.0, 0.0, 0.0, 1.0]; 16];
    for register in 0..16 {
        let Some(slot) = mapped_slots[register] else {
            continue;
        };
        if let Some(source) = slot_sources[slot] {
            let plan = groups[source.group]
                .as_ref()
                .ok_or("DMP host GPU attribute group missing")?;
            let base = plan
                .byte_offset
                .checked_add(source.byte_offset)
                .ok_or("DMP host GPU attribute base overflow")?;
            attributes[register] = [
                u32::try_from(base).map_err(|_| "DMP host GPU attribute base exceeds u32")?,
                u32::try_from(plan.stride).map_err(|_| "DMP host GPU stride exceeds u32")?,
                ((source.format.components as u32) << 16)
                    | (attribute_type(source.format.kind) << 8),
                0,
            ];
        } else {
            defaults[register] = engine.attributes[slot];
        }
    }

    Ok(PackedVertices {
        bytes,
        draw_indices: draw_indices.to_vec(),
        vertex_remap,
        attributes,
        defaults,
    })
}

fn attribute_type(kind: super::draw_state::AttributeKind) -> u32 {
    match kind {
        super::draw_state::AttributeKind::Signed8 => 0,
        super::draw_state::AttributeKind::Unsigned8 => 1,
        super::draw_state::AttributeKind::Signed16 => 2,
        super::draw_state::AttributeKind::Float32 => 3,
    }
}

fn vertex_semantics(state: &DrawState) -> [u32; VERTEX_OUTPUT_SEMANTICS] {
    let mut semantics = [u32::MAX; VERTEX_OUTPUT_SEMANTICS];
    for (register, vector) in state.vertex.output_map.0.iter().enumerate() {
        for (component, semantic) in vector.iter().copied().enumerate() {
            let semantic = semantic as usize;
            if semantic < semantics.len() {
                semantics[semantic] = (register * 4 + component) as u32;
            }
        }
    }
    semantics
}

fn draw_vertex_semantics(state: &DrawState) -> [u32; VERTEX_OUTPUT_SEMANTICS] {
    let outputs = vertex_semantics(state);
    std::array::from_fn(|semantic| {
        let source = match semantic {
            4..=7 => semantic + 4,
            8..=11 => semantic + 4,
            12..=13 => semantic + 2,
            _ => semantic,
        };
        outputs[source]
    })
}

fn shading_bytes(
    engine: &Engine,
    state: &DrawState,
    attributes: [[u32; 4]; 16],
    defaults: [[f32; 4]; 16],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2256);
    for slot in 0..96 {
        let uniform = engine.uniforms.get(slot).copied().unwrap_or([0; 4]);
        for word in uniform {
            push_u32(&mut bytes, word);
        }
    }
    for index in 0..4 {
        let packed = engine.reg(0x481 + index as u16);
        for value in [
            (packed & 255) as i32,
            (packed >> 8 & 255) as i32,
            ((packed >> 16 & 255) as u8 as i8) as i32,
            (packed >> 24 & 255) as i32,
        ] {
            push_u32(&mut bytes, value as u32);
        }
    }
    push_u32(&mut bytes, engine.reg(0x480));
    push_u32(&mut bytes, engine.reg(0x48a) & 0xffff);
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, 0);
    for semantic in draw_vertex_semantics(state) {
        push_u32(&mut bytes, semantic);
    }
    push_u32(&mut bytes, super::float24(engine.reg(0x24d)).to_bits());
    push_u32(&mut bytes, super::float24(engine.reg(0x24e)).to_bits());
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, 0);
    let viewport = [
        state.viewport.x_origin as f32,
        state.viewport.y_origin as f32,
        super::float24(engine.reg(0x241)) * 2.0,
        super::float24(engine.reg(0x243)) * 2.0,
    ];
    for value in viewport {
        push_u32(&mut bytes, value.to_bits());
    }
    for field in attributes {
        for value in field {
            push_u32(&mut bytes, value);
        }
    }
    for vector in defaults {
        for value in vector {
            push_u32(&mut bytes, value.to_bits());
        }
    }
    debug_assert_eq!(bytes.len(), 2256);
    bytes
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn mov(destination: u32, source: u32) -> u32 {
        (0x13 << 26) | (destination << 21) | (source << 12)
    }

    fn test_engine() -> Engine {
        let mut engine = Engine::default();
        engine.code = vec![mov(0, 0), mov(1, 1), mov(2, 2), 0x22 << 26];
        engine.operands = vec![0x8d86_c36f];
        engine.regs.insert(0x48a, 0);
        engine.regs.insert(0x400, 0x200);
        engine.regs.insert(0x401, 9 | (3 << 4));
        engine.regs.insert(0x403, 0);
        engine.regs.insert(0x404, 0x1c0);
        engine.regs.insert(0x405, (3 << 28) | (12 << 16));
        engine.regs.insert(0x427, 0x8000_0020);
        engine.regs.insert(0x428, 3);
        engine.regs.insert(0x48b, 0x210);
        engine.attributes[2] = [8.0, 9.0, 10.0, 1.0];
        engine
    }

    fn vertex_memory() -> BTreeMap<u64, Vec<u8>> {
        let mut memory = BTreeMap::new();
        memory.insert(0x1020, vec![2, 0, 0, 0, 1, 0]);
        for (index, color, scalar) in [
            (0, [1, 2, 3], 1.25_f32),
            (1, [4, 5, 6], -2.5_f32),
            (2, [7, 8, 9], 3.75_f32),
        ] {
            let mut bytes = vec![color[0], color[1], color[2], 0, 0, 0, 0, 0];
            bytes.extend_from_slice(&scalar.to_le_bytes());
            memory.insert(0x1000 + index * 12, bytes);
        }
        memory
    }

    #[test]
    fn indexed_typed_fetch_prepares_gpu_vertex_inputs() {
        let engine = test_engine();
        let state = engine.draw_state();
        let memory = vertex_memory();
        let mut read = |address, length| {
            memory
                .get(&address)
                .filter(|bytes| bytes.len() == length)
                .cloned()
                .ok_or_else(|| format!("missing test guest range {address:#x}+{length:#x}"))
        };
        let prepared = prepare_draw(&engine, &state, 3, true, &mut read).unwrap();
        assert_eq!(prepared.indices, [2, 0, 1]);
        assert_eq!(prepared.input_bytes.len(), 3 * 12);
        assert_eq!(prepared.shading_bytes.len(), 2256);
        assert!(prepared.shader_source.contains("@vertex"));
        let repeated = prepare_draw(&engine, &state, 3, true, &mut read).unwrap();
        assert!(Arc::ptr_eq(
            &prepared.shader_source,
            &repeated.shader_source
        ));
    }
}
