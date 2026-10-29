//! SMAPH 3D packets, attribute DMA, and uploaded vector programs.
//! Execution consumes only register values and addressed DMA inputs.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
mod capture;
mod draw_state;
#[cfg(not(target_os = "emscripten"))]
mod gpu_draw;
#[cfg(all(test, not(target_os = "emscripten")))]
mod gpu_fragment;
#[cfg(not(target_os = "emscripten"))]
mod gpu_vertex;
mod raster;
mod shader;
mod webgpu_wire;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod wasm_gpu;
use draw_state::DrawState;
use shader::Vector;
pub type Snapshots = Vec<(u64, Vec<u8>)>;
pub type Reader<'a> = dyn FnMut(u64, usize) -> Result<Vec<u8>, String> + 'a;
pub use webgpu_wire::{WebGpuRequest, decode_webgpu_request, encode_webgpu_result};
pub struct HostGpuExecution {
    pub snapshots: Snapshots,
    pub draws: usize,
    pub queue_submissions: u8,
}
const MAX_BYTES: usize = 64 * 1024 * 1024;
type Uniform = [u32; 4];
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Engine {
    #[serde(skip)]
    written_ranges: Vec<(u64, usize)>,
    vertices_executed: u64,
    primitive_index: u64,
    #[cfg(not(target_os = "emscripten"))]
    #[serde(skip)]
    gpu_primitives: Option<Vec<gpu_draw::Draw>>,
    #[cfg(not(target_os = "emscripten"))]
    #[serde(skip)]
    gpu_immediate: Option<(DrawState, Vec<[Vector; 16]>)>,
    last_output: Option<[Vector; 16]>,
    regs: BTreeMap<u16, u32>,
    code: Vec<u32>,
    operands: Vec<u32>,
    uniforms: Vec<Uniform>,
    code_cursor: usize,
    operand_cursor: usize,
    uniform_cursor: u32,
    uniform_words: Vec<u32>,
    attribute_words: Vec<u32>,
    attribute_cursor: usize,
    attributes: [Vector; 16],
    immediate: Vec<Vector>,
    vertices: Vec<[Vector; 16]>,
    #[serde(default)]
    pending_inputs: Vec<[Vector; 16]>,
    fog: Vec<u32>,
    fog_cursor: usize,
}
impl Default for Engine {
    fn default() -> Self {
        Self {
            written_ranges: Vec::new(),
            vertices_executed: 0,
            primitive_index: 0,
            #[cfg(not(target_os = "emscripten"))]
            gpu_primitives: None,
            #[cfg(not(target_os = "emscripten"))]
            gpu_immediate: None,
            last_output: None,
            regs: BTreeMap::new(),
            code: vec![],
            operands: vec![],
            uniforms: vec![[0; 4]; 96],
            code_cursor: 0,
            operand_cursor: 0,
            uniform_cursor: 0,
            uniform_words: vec![],
            attribute_words: vec![],
            attribute_cursor: 0,
            attributes: [[0., 0., 0., 1.]; 16],
            immediate: vec![],
            vertices: vec![],
            pending_inputs: vec![],
            fog: vec![0; 128],
            fog_cursor: 0,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn host_gpu_mode_for(cpu_only: bool, verify: bool) -> (bool, bool) {
    (verify || !cpu_only, verify)
}

#[cfg(not(target_arch = "wasm32"))]
fn host_gpu_mode() -> (bool, bool) {
    let enabled = |name| std::env::var(name).is_ok_and(|value| value == "1");
    host_gpu_mode_for(enabled("FUJIMOD_DMP_CPU"), enabled("FUJIMOD_DMP_VERIFY_GPU"))
}
pub fn float24(word: u32) -> f32 {
    let sign = (word & 0x800000) << 8;
    let exp = word >> 16 & 127;
    let mantissa = word & 65535;
    if exp == 0 {
        return f32::from_bits(sign)
            + if mantissa == 0 {
                0.
            } else {
                (if sign == 0 { 1. } else { -1. }) * (mantissa as f32 / 65536.) * 2f32.powi(-62)
            };
    }
    if exp == 127 {
        return f32::from_bits(sign | 0x7f800000 | mantissa << 7);
    }
    f32::from_bits(sign | ((exp + 64) << 23) | mantissa << 7)
}
fn unpack(v: &[u32]) -> Vector {
    [
        float24(v[2] & 0xffffff),
        float24((v[2] >> 24) | (v[1] << 8 & 0xffffff)),
        float24((v[1] >> 16) | (v[0] << 16 & 0xffffff)),
        float24(v[0] >> 8),
    ]
}
fn upload(bank: &mut Vec<u32>, cursor: &mut usize, value: u32, limit: usize) -> Result<(), String> {
    if *cursor >= limit {
        return Err("DMP shader upload exceeds bank".into());
    }
    if bank.len() <= *cursor {
        bank.resize(*cursor + 1, 0);
    }
    bank[*cursor] = value;
    *cursor += 1;
    Ok(())
}

fn read_guest_draw_memory(
    read: &mut Reader<'_>,
    cache: &mut BTreeMap<(u64, usize), Vec<u8>>,
    address: u64,
    length: usize,
) -> Result<Vec<u8>, String> {
    let key = (address, length);
    let bytes = read(address, length)?;
    cache.insert(key, bytes.clone());
    Ok(bytes)
}

impl Engine {
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({"registers":self.regs,"vertices_executed":self.vertices_executed,"last_output":self.last_output,"code_words":self.code.len(),"host_rasterizer":"CPU"})
    }
    pub fn reg(&self, index: u16) -> u32 {
        self.regs.get(&index).copied().unwrap_or(0)
    }
    /// Transactional validation and rendering: a refused list changes neither
    /// register/program state nor memory. Outputs commit at the device deadline.
    pub fn execute(&mut self, words: &[u32], read: &mut Reader<'_>) -> Result<Snapshots, String> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(directory) = capture::capture_directory() {
            return self.execute_captured(words, read, &directory);
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let (gpu_requested, verify_gpu) = host_gpu_mode();
            if gpu_requested {
                return if verify_gpu {
                    self.execute_host_gpu_verified(words, read)
                } else {
                    self.execute_host_gpu(words, read)
                };
            }
        }

        self.execute_cpu(words, read)
    }

    fn execute_cpu(
        &mut self,
        words: &[u32],
        read: &mut Reader<'_>,
    ) -> Result<Snapshots, String> {
        let mut next = self.clone();
        let mut output = raster::Targets::default();
        next.packets(words, read, &mut output, 0)?;
        next.written_ranges = output.written_ranges();
        let snapshots = output.into_snapshots();
        *self = next;
        Ok(snapshots)
    }
    /// Execute one list through the host GPU vertex and fragment paths.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn execute_host_gpu(
        &mut self,
        words: &[u32],
        read: &mut Reader<'_>,
    ) -> Result<Snapshots, String> {
        let (next, rendered) = self.render_host_gpu(words, read, None)?;
        *self = next;
        Ok(rendered.snapshots)
    }

    /// Render through the host GPU and refuse results that differ from the
    /// CPU reference snapshots or write ranges.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn execute_host_gpu_verified(
        &mut self,
        words: &[u32],
        read: &mut Reader<'_>,
    ) -> Result<Snapshots, String> {
        let mut reference = self.clone();
        reference.gpu_primitives = None;
        reference.gpu_immediate = None;
        let mut cpu_output = raster::Targets::default();
        reference.packets(words, read, &mut cpu_output, 0)?;
        let cpu_written_ranges = cpu_output.written_ranges();
        let cpu_snapshots = cpu_output.into_snapshots();

        let (next, rendered) = self.render_host_gpu(words, read, Some(&cpu_snapshots))?;
        if rendered.snapshots != cpu_snapshots {
            return Err(gpu_draw::snapshot_mismatch(
                &rendered.snapshots,
                &cpu_snapshots,
            ));
        }
        if rendered.written_ranges != cpu_written_ranges {
            return Err(format!(
                "DMP host GPU write ranges differ: GPU {:?}, CPU {:?}",
                rendered.written_ranges, cpu_written_ranges
            ));
        }
        *self = next;
        Ok(rendered.snapshots)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn render_host_gpu(
        &self,
        words: &[u32],
        read: &mut Reader<'_>,
        expected_snapshots: Option<&Snapshots>,
    ) -> Result<(Self, gpu_draw::RenderedList), String> {
        let mut next = self.clone();
        next.gpu_primitives = Some(Vec::new());
        next.gpu_immediate = None;
        if !next.vertices.is_empty() {
            if next.vertices.len() != next.pending_inputs.len() {
                return Err("DMP checkpoint lost pending immediate vertex inputs".into());
            }
            let mut inputs = next.pending_inputs.clone();
            if next.draw_state().raster.primitive_mode == draw_state::PrimitiveMode::TriangleStrip
                && next.primitive_index & 1 != 0
                && inputs.len() >= 2
            {
                inputs.swap(0, 1);
            }
            next.gpu_immediate = Some((next.draw_state(), inputs));
            next.vertices.clear();
            next.pending_inputs.clear();
        }
        let mut unused_output = raster::Targets::default();
        let mut recorded_reads = Vec::new();
        {
            let mut recording_reader = |address, length| {
                let bytes = read(address, length)?;
                recorded_reads.push((address, bytes.clone()));
                Ok(bytes)
            };
            next.packets(words, &mut recording_reader, &mut unused_output, 0)?;
        }
        next.flush_gpu_immediate()?;
        let draws = next.gpu_primitives.take().unwrap_or_default();
        let rendered = gpu_draw::render(&draws, &recorded_reads, expected_snapshots, read)?;
        if rendered.queue_submissions > 3 {
            return Err(format!(
                "DMP host GPU submitted {} command buffers for one list (limit 3)",
                rendered.queue_submissions
            ));
        }
        next.gpu_primitives = None;
        next.gpu_immediate = None;
        next.written_ranges = rendered.written_ranges.clone();
        Ok((next, rendered))
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn execute_captured(
        &mut self,
        words: &[u32],
        read: &mut Reader<'_>,
        directory: &std::path::Path,
    ) -> Result<Snapshots, String> {
        let capture_state = capture::serialize_engine(self)?;
        let mut next = self.clone();
        let mut output = raster::Targets::default();
        let mut capture_reads = Vec::new();
        {
            let mut recording_reader = |address, length| {
                let bytes = read(address, length)?;
                capture_reads.push((address, bytes.clone()));
                Ok(bytes)
            };
            next.packets(words, &mut recording_reader, &mut output, 0)?;
        }
        next.written_ranges = output.written_ranges();
        let snapshots = output.into_snapshots();
        capture::write_capture(
            directory,
            &capture_state,
            words,
            &capture_reads,
            &snapshots,
            &next.written_ranges,
        )?;
        *self = next;
        Ok(snapshots)
    }
    /// Byte ranges written by the most recent successful command list.
    /// Surface snapshots also contain pixels read for blending or tests;
    /// those bytes must not be committed by the device as DMA writes.
    pub fn written_ranges(&self) -> &[(u64, usize)] {
        &self.written_ranges
    }
    fn packets(
        &mut self,
        words: &[u32],
        read: &mut Reader<'_>,
        output: &mut raster::Targets,
        depth: usize,
    ) -> Result<(), String> {
        if words.len() > 1024 * 1024 / 4 || depth > 16 {
            return Err("DMP 3D command bound exceeded".into());
        }
        let mut at = 0;
        while at < words.len() {
            let header = *words.get(at + 1).ok_or("DMP truncated 3D header")?;
            let count = ((header >> 20 & 2047) + 1) as usize;
            let extent = (count + 2) & !1;
            if at + extent > words.len() {
                return Err("DMP truncated 3D packet".into());
            }
            let mask = (0..4)
                .filter(|i| header & (1 << (16 + i)) != 0)
                .fold(0, |m, i| m | (255 << (8 * i)));
            if header & 65535 == 0x43c && mask == u32::MAX && count == 1 && words[at] == 1 {
                if at + extent != words.len() {
                    return Err("DMP 3D link is not terminal".into());
                }
                let length = self.reg(0x438) as usize * 8;
                if length == 0 || length > 1024 * 1024 {
                    return Err("DMP 3D link extent exceeds bound".into());
                }
                let bytes = read(self.reg(0x43a) as u64 * 8, length)?;
                if bytes.len() != length {
                    return Err("DMP short linked command DMA".into());
                }
                let linked: Vec<_> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from_le_bytes(*b))
                    .collect();
                return self.packets(&linked, read, output, depth + 1);
            }
            for j in 0..count {
                let index = (header & 65535)
                    .checked_add(if header >> 31 != 0 { j as u32 } else { 0 })
                    .filter(|v| *v <= 65535)
                    .ok_or("DMP 3D index overflow")? as u16;
                let value = words[at + if j == 0 { 0 } else { j + 1 }];
                if mask != 0 {
                    self.write(index, value, mask, read, output)?;
                } else if !matches!(index, 0x200 | 0x280 | 0x2c0) {
                    return Err(format!("DMP unsupported zero-lane packet {index:#x}"));
                }
            }
            at += extent;
        }
        Ok(())
    }
    fn write(
        &mut self,
        index: u16,
        value: u32,
        mask: u32,
        read: &mut Reader<'_>,
        out: &mut raster::Targets,
    ) -> Result<(), String> {
        if !matches!(index, 0x200|0x210|0x240..=0x244|0x247..=0x256|0x258..=0x259|0x264..=0x268|0x26d..=0x273|0x278|0x280..=0x285|0x287|0x289|0x28d..=0x28e|0x291..=0x297|0x299|0x2c0..=0x2c4|0x2c8..=0x2cc|0x2e0..=0x2e4|0x2e6|0x2e8|0x2f0|0x300..=0x309|0x312..=0x31e|0x320|0x400..=0x41a|0x427..=0x435|0x438|0x43a|0x451..=0x453|0x45e..=0x45f|0x480..=0x484|0x489..=0x48d|0x48f..=0x498|0x49b..=0x4a3|0x4a5..=0x4ad)
        {
            return Err(format!("DMP unsupported 3D register {index:#x}"));
        }
        #[cfg(not(target_os = "emscripten"))]
        if self.gpu_immediate.is_some() && !matches!(index, 0x432..=0x435) {
            self.flush_gpu_immediate()?;
        }
        let value = self.reg(index) & !mask | value & mask;
        self.regs.insert(index, value);
        match index {
            0x320 | 0x2f0 | 0x287 | 0x297 if value != 1 => {
                return Err(format!(
                    "DMP unsupported cache/engine control {index:#x}={value:#x}"
                ));
            }
            0x28d if value > 1 => return Err("DMP unsupported sampler latch control".into()),
            0x48d if value & !7 != 0 => return Err("DMP unsupported output flags".into()),
            0x431 | 0x45f if value > 1 => return Err("DMP unsupported primitive trigger".into()),
            0x49b => {
                if value >> 16 > 4096 {
                    return Err("DMP shader patch length exceeds bank".into());
                }
                self.code_cursor = (value & 65535) as usize;
            }
            0x49c..=0x4a3 => upload(&mut self.code, &mut self.code_cursor, value, 4096)?,
            0x4a5 => self.operand_cursor = value as usize,
            0x4a6..=0x4ad => upload(&mut self.operands, &mut self.operand_cursor, value, 128)?,
            0x490 => {
                self.uniform_cursor = value;
                self.uniform_words.clear();
            }
            0x491..=0x498 => {
                self.uniform_words.push(value);
                let count = if self.uniform_cursor >> 31 != 0 { 4 } else { 3 };
                if self.uniform_words.len() == count {
                    let slot = (self.uniform_cursor & 0x7fffffff) as usize;
                    let words = if count == 3 {
                        unpack(&self.uniform_words).map(f32::to_bits)
                    } else {
                        std::array::from_fn(|i| self.uniform_words[3 - i])
                    };
                    *self
                        .uniforms
                        .get_mut(slot)
                        .ok_or("DMP uniform upload outside bank")? = words;
                    self.uniform_cursor += 1;
                    self.uniform_words.clear();
                }
            }
            0x432 => {
                self.attribute_cursor = value as usize;
                self.attribute_words.clear();
                self.immediate.clear();
            }
            0x433..=0x435 => {
                self.attribute_words.push(value);
                if self.attribute_words.len() == 3 {
                    let v = unpack(&self.attribute_words);
                    self.attribute_words.clear();
                    if self.attribute_cursor == 15 {
                        self.immediate.push(v);
                        let state = self.draw_state();
                        let n = state.vertex.immediate_attribute_count as usize;
                        if n > 8 {
                            return Err(
                                "DMP immediate attribute count exceeds permutation bank".into()
                            );
                        }
                        if self.immediate.len() == n {
                            let mut inputs = self.attributes;
                            let state = self.draw_state();
                            for (i, v) in self.immediate.iter().enumerate() {
                                inputs[state.vertex.input_map[i] as usize] = *v;
                            }
                            self.immediate.clear();
                            self.vertex(&inputs, &state, read, out)?;
                        }
                    } else {
                        *self
                            .attributes
                            .get_mut(self.attribute_cursor)
                            .ok_or("DMP attribute upload outside bank")? = v;
                        self.attribute_cursor += 1;
                    }
                }
            }
            0x42e | 0x42f if value != 0 => {
                if value != 1 {
                    return Err("DMP invalid draw trigger".into());
                }
                let count = self.reg(0x428) as usize;
                if count > 1_048_576 {
                    return Err("DMP vertex count exceeds bound".into());
                }
                let state = self.draw_state();
                #[cfg(not(target_os = "emscripten"))]
                if self.gpu_primitives.is_some() {
                    let vertex =
                        gpu_vertex::prepare_draw(self, &state, count, index == 0x42f, read)?;
                    self.vertices_executed += count as u64;
                    self.last_output = None;
                    self.record_gpu_draw(state, vertex);
                } else {
                    self.draw_interpreted(index, count, &state, read, out)?;
                }
                #[cfg(target_os = "emscripten")]
                self.draw_interpreted(index, count, &state, read, out)?;
            }
            0x45f | 0x431 if value == 1 => {
                self.vertices.clear();
                self.primitive_index = 0;
            }
            0x210 if value != 0x12345678 => {
                return Err("DMP invalid command completion marker".into());
            }
            0x2e6 => self.fog_cursor = (value & 127) as usize,
            0x2e8 => {
                if self.fog_cursor >= 128 {
                    return Err("DMP fog upload outside bank".into());
                }
                self.fog[self.fog_cursor] = value;
                self.fog_cursor += 1;
            }
            _ => {}
        }
        Ok(())
    }
    fn draw_interpreted(
        &mut self,
        index: u16,
        count: usize,
        state: &DrawState,
        read: &mut Reader<'_>,
        out: &mut raster::Targets,
    ) -> Result<(), String> {
        let mut draw_dma = BTreeMap::new();
        let index_addr = self.reg(0x427);
        let indices = if index == 0x42f {
            Some(read_guest_draw_memory(
                read,
                &mut draw_dma,
                (self.reg(0x400) as u64 * 8) + (index_addr as u64 & 0x7fff_ffff),
                count * if index_addr >> 31 != 0 { 2 } else { 1 },
            )?)
        } else {
            None
        };
        if indices
            .as_ref()
            .is_some_and(|bytes| bytes.len() != count * if index_addr >> 31 != 0 { 2 } else { 1 })
        {
            return Err("DMP short index DMA".into());
        }
        for n in 0..count {
            let vertex = if let Some(bytes) = &indices {
                if index_addr >> 31 != 0 {
                    u16::from_le_bytes(bytes[n * 2..n * 2 + 2].try_into().unwrap()) as usize
                } else {
                    bytes[n] as usize
                }
            } else {
                n + self.reg(0x42a) as usize
            };
            let inputs = self.fetch(vertex, state, &mut |address, length| {
                read_guest_draw_memory(read, &mut draw_dma, address, length)
            })?;
            self.vertex(&inputs, state, read, out)?;
        }
        Ok(())
    }
    fn fetch(
        &self,
        vertex: usize,
        state: &DrawState,
        read: &mut Reader<'_>,
    ) -> Result<[Vector; 16], String> {
        let mut slots = self.attributes;
        for group in state.vertex.groups {
            let count = group.attribute_count as u32;
            let stride = group.stride;
            if count == 0 {
                continue;
            }
            if count > 8 {
                return Err("DMP fetch group exceeds slot count".into());
            }
            if stride == 0 {
                return Err("DMP zero vertex fetch stride".into());
            }
            let address = state.vertex.base_address
                + group.byte_offset
                + (vertex
                    .checked_mul(stride)
                    .ok_or("DMP vertex offset overflow")? as u64);
            let bytes = read(address, stride)?;
            if bytes.len() != stride {
                return Err("DMP short vertex DMA".into());
            }
            let mut at = 0;
            for i in 0..count as usize {
                let slot = group.order >> (i * 4) & 15;
                if slot >= 12 {
                    at = (at + 3) & !3;
                    at += (slot as usize - 11) * 4;
                    continue;
                }
                if slot >= 8 {
                    return Err("DMP unsupported attribute format bank".into());
                }
                let format = state.vertex.formats[slot as usize];
                let components = format.components as usize;
                let width = format.bytes_per_component();
                at = (at + width - 1) & !(width - 1);
                let mut v = [0., 0., 0., 1.];
                for component in v.iter_mut().take(components) {
                    let b = bytes
                        .get(at..at + width)
                        .ok_or("DMP vertex fetch exceeds stride")?;
                    *component = match format.kind {
                        draw_state::AttributeKind::Signed8 => (b[0] as i8) as f32,
                        draw_state::AttributeKind::Unsigned8 => b[0] as f32,
                        draw_state::AttributeKind::Signed16 => {
                            i16::from_le_bytes(b.try_into().unwrap()) as f32
                        }
                        draw_state::AttributeKind::Float32 => {
                            f32::from_le_bytes(b.try_into().unwrap())
                        }
                    };
                    at += width;
                }
                slots[slot as usize] = v;
            }
        }
        let mut inputs = [[0., 0., 0., 1.]; 16];
        for (i, v) in slots.iter().enumerate().take(8) {
            inputs[state.vertex.input_map[i] as usize] = *v;
        }
        Ok(inputs)
    }
    fn vertex(
        &mut self,
        inputs: &[Vector; 16],
        state: &DrawState,
        read: &mut Reader<'_>,
        out: &mut raster::Targets,
    ) -> Result<(), String> {
        #[cfg(not(target_os = "emscripten"))]
        if self.gpu_primitives.is_some() {
            self.vertices_executed += 1;
            self.last_output = None;
            let pending = self
                .gpu_immediate
                .get_or_insert_with(|| (state.clone(), Vec::new()));
            pending.1.push(*inputs);
            return Ok(());
        }
        let vertex = shader::execute(self, inputs)?;
        self.vertex_output(vertex, inputs, state, read, out)
    }
    fn vertex_output(
        &mut self,
        vertex: [Vector; 16],
        inputs: &[Vector; 16],
        state: &DrawState,
        read: &mut Reader<'_>,
        out: &mut raster::Targets,
    ) -> Result<(), String> {
        self.vertices_executed += 1;
        self.last_output = Some(vertex);
        let mode = state.raster.primitive_mode;
        if mode == draw_state::PrimitiveMode::Points {
            return raster::point(state, &vertex, read, out);
        }
        self.vertices.push(vertex);
        self.pending_inputs.push(*inputs);
        let n = self.vertices.len();
        match mode {
            draw_state::PrimitiveMode::Triangles if n == 3 => {
                let triangle = [self.vertices[0], self.vertices[1], self.vertices[2]];
                raster::triangle(state, triangle, read, out)?;
                self.vertices.clear();
                self.pending_inputs.clear();
            }
            draw_state::PrimitiveMode::TriangleStrip if n >= 3 => {
                let t = if self.primitive_index & 1 != 0 {
                    [
                        self.vertices[n - 2],
                        self.vertices[n - 3],
                        self.vertices[n - 1],
                    ]
                } else {
                    [
                        self.vertices[n - 3],
                        self.vertices[n - 2],
                        self.vertices[n - 1],
                    ]
                };
                raster::triangle(state, t, read, out)?;
                self.primitive_index += 1;
                if n > 3 {
                    self.vertices.remove(0);
                    self.pending_inputs.remove(0);
                }
            }
            draw_state::PrimitiveMode::TriangleFan if n >= 3 => {
                let triangle = [self.vertices[0], self.vertices[n - 2], self.vertices[n - 1]];
                raster::triangle(state, triangle, read, out)?;
                self.vertices.remove(1);
                self.pending_inputs.remove(1);
            }
            draw_state::PrimitiveMode::Lines | draw_state::PrimitiveMode::LineStrip if n == 2 => {
                raster::line(state, [self.vertices[0], self.vertices[1]], read, out)?;
                if mode == draw_state::PrimitiveMode::Lines {
                    self.vertices.clear();
                    self.pending_inputs.clear();
                } else {
                    self.vertices.remove(0);
                    self.pending_inputs.remove(0);
                }
            }
            draw_state::PrimitiveMode::Triangles
            | draw_state::PrimitiveMode::TriangleStrip
            | draw_state::PrimitiveMode::TriangleFan
            | draw_state::PrimitiveMode::Lines
            | draw_state::PrimitiveMode::LineStrip => {}
            draw_state::PrimitiveMode::Other(code) => {
                return Err(format!("DMP unsupported primitive mode {code}"));
            }
            draw_state::PrimitiveMode::Points => unreachable!(),
        }
        Ok(())
    }
    #[cfg(not(target_os = "emscripten"))]
    fn flush_gpu_immediate(&mut self) -> Result<(), String> {
        let Some((state, vertices)) = self.gpu_immediate.take() else {
            return Ok(());
        };
        if vertices.len() < 3 {
            return Ok(());
        }
        let vertex = gpu_vertex::prepare_immediate_draw(self, &state, &vertices)?;
        self.record_gpu_draw(state, vertex);
        Ok(())
    }
    #[cfg(not(target_os = "emscripten"))]
    fn record_gpu_draw(&mut self, state: DrawState, vertex: gpu_vertex::PreparedDraw) {
        if let Some(draws) = &mut self.gpu_primitives {
            draws.push(gpu_draw::Draw::new(state, vertex, &self.regs));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn replay_captures(directory: &std::path::Path) -> Result<usize, String> {
    let translations_before = gpu_vertex::translation_count();
    let (vertex_modules_before, fragment_modules_before, pipelines_before) =
        gpu_draw::shader_cache_creation_counts();
    let count = capture::replay_directory(directory)?;
    if std::env::var("FUJIMOD_DMP_SHADER_CACHE_STATS").is_ok_and(|value| value == "1") {
        let translations = gpu_vertex::translation_count() - translations_before;
        let (vertex_modules, fragment_modules, pipelines) =
            gpu_draw::shader_cache_creation_counts();
        eprintln!(
            "DMP shader cache for replay: {translations} Naga vertex translations, {} vertex modules, {} fragment modules, {} pipelines",
            vertex_modules - vertex_modules_before,
            fragment_modules - fragment_modules_before,
            pipelines - pipelines_before,
        );
    }
    Ok(count)
}

#[cfg(test)]
mod tests;
