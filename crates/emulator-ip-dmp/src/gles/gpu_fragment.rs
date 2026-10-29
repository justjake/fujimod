//! Standalone WGSL conformance harness for the DMP fixed-function fragment stage.
//!
//! Production host draws use the render pipeline in `gpu_draw`; this compute
//! harness checks the shared fragment operations against the CPU reference.

use super::Engine;
#[cfg(test)]
use super::raster;
use std::sync::{Mutex, OnceLock};
use wgpu::util::DeviceExt;

const REGISTER_WORDS: usize = 0x500;
const FOG_OFFSET: usize = REGISTER_WORDS;
const FOG_WORDS: usize = 128;
const INPUT_OFFSET: usize = FOG_OFFSET + FOG_WORDS;
const INPUT_WORDS: usize = 16;
const CASE_WORDS: usize = INPUT_OFFSET + INPUT_WORDS;

/// Values interpolated at one fragment by the primitive stage.
#[derive(Clone, Copy, Debug)]
pub(super) struct FragmentInput {
    pub x: u32,
    pub y: u32,
    pub z: f32,
    pub coverage: f32,
    pub primary: [f32; 4],
    pub uv: [[f32; 2]; 2],
    pub lod: [f32; 2],
}

/// One fragment paired with its DMP register and fog state.
pub(super) struct FragmentCase<'a> {
    pub engine: &'a Engine,
    pub input: FragmentInput,
}

struct Gpu {
    pipeline: wgpu::ComputePipeline,
}

impl Gpu {
    fn new(context: &fujimod_emulator_gpu::GpuContext) -> Result<Self, String> {
        let device = context.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("DMP fragment stage"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_fragment.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("DMP fragment stage"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self { pipeline })
    }

    fn dispatch(
        &self,
        context: &fujimod_emulator_gpu::GpuContext,
        cases: &[FragmentCase<'_>],
        memory: &[u8],
    ) -> Result<Vec<u8>, String> {
        let device = context.device();
        let queue = context.queue();
        if cases.is_empty() || memory.is_empty() || !memory.len().is_multiple_of(4) {
            return Err("DMP fragment input extent is invalid".into());
        }
        let parameter_words = cases
            .len()
            .checked_mul(CASE_WORDS)
            .ok_or("DMP fragment parameter extent overflow")?;
        let limits = device.limits();
        if parameter_words * 4 > limits.max_storage_buffer_binding_size as usize
            || memory.len() > limits.max_storage_buffer_binding_size as usize
            || memory.len() as u64 > limits.max_buffer_size
        {
            return Err("DMP fragment storage exceeds the device limit".into());
        }

        let mut parameters = vec![0u32; parameter_words];
        for (index, case) in cases.iter().enumerate() {
            let base = index * CASE_WORDS;
            for (&register, &value) in &case.engine.regs {
                let register = register as usize;
                if register >= REGISTER_WORDS {
                    return Err("DMP fragment register exceeds the packed state".into());
                }
                parameters[base + register] = value;
            }
            let fog = &case.engine.fog;
            if fog.len() != FOG_WORDS {
                return Err("DMP fragment fog table extent is invalid".into());
            }
            parameters[base + FOG_OFFSET..base + INPUT_OFFSET].copy_from_slice(fog);
            let input = &case.input;
            let at = base + INPUT_OFFSET;
            parameters[at..at + INPUT_WORDS].copy_from_slice(&[
                input.x,
                input.y,
                input.z.to_bits(),
                input.coverage.to_bits(),
                input.primary[0].to_bits(),
                input.primary[1].to_bits(),
                input.primary[2].to_bits(),
                input.primary[3].to_bits(),
                input.uv[0][0].to_bits(),
                input.uv[0][1].to_bits(),
                input.uv[1][0].to_bits(),
                input.uv[1][1].to_bits(),
                input.lod[0].to_bits(),
                input.lod[1].to_bits(),
                0,
                0,
            ]);
        }
        let parameter_bytes = encode_words(&parameters);
        let parameter_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("DMP fragment parameters"),
            contents: &parameter_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let memory_words: Vec<u32> = memory
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
            .collect();
        let memory_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("DMP fragment guest memory"),
            contents: &encode_words(&memory_words),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let status_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("DMP fragment status"),
            contents: &encode_words(&vec![0; cases.len()]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let download = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DMP fragment readback"),
            size: memory.len() as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let status_download = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DMP fragment status readback"),
            size: (cases.len() * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("DMP fragment bindings"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: parameter_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: memory_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: status_buffer.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups((cases.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&memory_buffer, 0, &download, 0, memory.len() as u64);
        encoder.copy_buffer_to_buffer(
            &status_buffer,
            0,
            &status_download,
            0,
            (cases.len() * 4) as u64,
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        download
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| format!("DMP fragment GPU polling: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("DMP fragment readback: {error}"))?
            .map_err(|error| format!("DMP fragment readback: {error}"))?;
        let bytes = download
            .slice(..)
            .get_mapped_range()
            .map_err(|error| format!("DMP fragment mapped readback: {error}"))?;
        let result = bytes.to_vec();
        drop(bytes);
        download.unmap();
        let (sender, receiver) = std::sync::mpsc::channel();
        status_download
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| format!("DMP fragment status polling: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("DMP fragment status readback: {error}"))?
            .map_err(|error| format!("DMP fragment status readback: {error}"))?;
        let statuses = status_download
            .slice(..)
            .get_mapped_range()
            .map_err(|error| format!("DMP fragment mapped status: {error}"))?;
        let status_error =
            statuses
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .find_map(|(index, bytes)| {
                    let status = u32::from_le_bytes(*bytes);
                    let error = match status {
                        0 => return None,
                        1 => "DMP unsupported blend factor".to_string(),
                        2 => "DMP unsupported blend equation".to_string(),
                        3 => "DMP unsupported logic operation".to_string(),
                        status if status & 0xffff_fff0 == 0x10000 => {
                            return Some(format!(
                                "DMP fragment {index}: DMP unsupported sampler format {}",
                                status & 15
                            ));
                        }
                        status => format!("unsupported status {status}"),
                    };
                    Some(format!("DMP fragment {index}: {error}"))
                });
        drop(statuses);
        status_download.unmap();
        if let Some(error) = status_error {
            return Err(error);
        }
        Ok(result)
    }
}

fn encode_words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

static GPU: OnceLock<Result<Mutex<Gpu>, String>> = OnceLock::new();

/// Run one or more fragments through the host compute implementation.
///
/// `memory` is indexed by the DMP's byte addresses, as it is by the CPU
/// sampler and target reader. Returned bytes include any color/depth writes.
pub(super) fn dispatch(cases: &[FragmentCase<'_>], memory: &[u8]) -> Result<Vec<u8>, String> {
    let context = fujimod_emulator_gpu::context()?;
    let gpu = GPU.get_or_init(|| Gpu::new(context).map(Mutex::new));
    match gpu {
        Ok(gpu) => gpu
            .lock()
            .map_err(|_| "DMP fragment GPU lock poisoned")?
            .dispatch(context, cases, memory),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gles::raster::Targets;

    const CASE_COUNT: usize = 128;
    const REGION_SIZE: usize = 0x3000;
    const MEMORY_BASE: usize = 0x1000;

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u32 {
            let mut value = self.0;
            value ^= value << 13;
            value ^= value >> 7;
            value ^= value << 17;
            self.0 = value;
            (value >> 16) as u32
        }

        fn below(&mut self, bound: u32) -> u32 {
            self.next() % bound
        }

        fn unit(&mut self) -> f32 {
            (self.next() >> 8) as f32 / 16_777_215.0
        }

        fn range(&mut self, low: f32, high: f32) -> f32 {
            low + (high - low) * self.unit()
        }
    }

    fn set(engine: &mut Engine, register: u16, value: u32) {
        engine.regs.insert(register, value);
    }

    fn skip_without_adapter() -> bool {
        let context = match fujimod_emulator_gpu::context() {
            Ok(context) => context,
            Err(error) if error.starts_with("host GPU adapter:") => {
                eprintln!(
                    "skipping DMP fragment test because no host GPU adapter is available: {error}"
                );
                return true;
            }
            Err(error) => panic!("DMP fragment GPU setup failed: {error}"),
        };
        match GPU.get_or_init(|| Gpu::new(context).map(Mutex::new)) {
            Ok(_) => false,
            Err(error) => panic!("DMP fragment GPU setup failed: {error}"),
        }
    }

    fn random_combiner(rng: &mut Random, engine: &mut Engine, base: u16) {
        let sources = [0u32, 3, 4, 14, 15];
        let mut source_word = 0;
        for lane in 0..3 {
            source_word |= sources[rng.below(sources.len() as u32) as usize] << (lane * 4);
        }
        for lane in 0..3 {
            source_word |= sources[rng.below(sources.len() as u32) as usize] << (16 + lane * 4);
        }
        let mut operand_word = 0;
        for lane in 0..3 {
            operand_word |= rng.below(4) << (lane * 4);
            operand_word |= rng.below(2) << (12 + lane * 4);
        }
        let operations = rng.below(8) | (rng.below(8) << 16);
        let scale = rng.below(4) | (rng.below(4) << 16);
        set(engine, base, source_word);
        set(engine, base + 1, operand_word);
        set(engine, base + 2, operations);
        set(engine, base + 3, rng.next());
        set(engine, base + 4, scale);
    }

    fn random_texture(rng: &mut Random, engine: &mut Engine, unit: usize, address: u32) {
        const FORMATS: [u32; 8] = [0, 1, 2, 3, 4, 5, 7, 8];
        let base = if unit == 0 { 0x281 } else { 0x291 };
        let limit = rng.below(2);
        let dimension = if limit == 1 || rng.below(2) == 1 {
            16
        } else {
            8
        };
        let format = FORMATS[rng.below(FORMATS.len() as u32) as usize];
        let bias = (rng.below(1025) as i32 - 512) as u32 & 8191;
        let wrap_x = rng.below(4);
        let wrap_y = rng.below(4);
        let min_filter = rng.below(2) << 2;
        let mag_filter = rng.below(2) << 1;
        let mip_linear = rng.below(2) << 24;
        set(engine, base, rng.next());
        set(engine, base + 1, (dimension << 16) | dimension);
        set(
            engine,
            base + 2,
            (wrap_x << 12) | (wrap_y << 8) | min_filter | mag_filter | mip_linear,
        );
        set(engine, base + 3, (limit << 16) | bias);
        set(engine, base + 4, address);
        set(engine, base + 8, address + 0x400);
        set(engine, if unit == 0 { 0x28e } else { 0x296 }, format);
    }

    fn random_case(rng: &mut Random, memory: &mut [u8], index: usize) -> (Engine, FragmentInput) {
        let region = MEMORY_BASE + index * REGION_SIZE;
        let mut engine = Engine::default();
        engine.fog = (0..128).map(|_| rng.next()).collect();
        set(&mut engine, 0x280, rng.below(4));
        random_texture(rng, &mut engine, 0, (region + 0x000) as u32);
        random_texture(rng, &mut engine, 1, (region + 0x800) as u32);
        random_combiner(rng, &mut engine, 0x2c0);
        random_combiner(rng, &mut engine, 0x2c8);

        let fog_enabled = rng.below(2);
        let fog_table = rng.below(2) << 2;
        set(&mut engine, 0x2e0, fog_enabled | fog_table);
        set(&mut engine, 0x2e1, rng.next());
        set(&mut engine, 0x2e2, rng.next() & 0xffff);
        set(&mut engine, 0x2e3, rng.next() & 0xffff);
        set(
            &mut engine,
            0x304,
            rng.below(2) | (rng.below(8) << 4) | (rng.below(256) << 8),
        );
        let stencil_enabled = rng.below(2);
        set(
            &mut engine,
            0x305,
            stencil_enabled
                | (rng.below(8) << 4)
                | (rng.below(256) << 8)
                | (rng.below(256) << 16)
                | (rng.below(256) << 24),
        );
        set(
            &mut engine,
            0x306,
            rng.below(8) | (rng.below(8) << 4) | (rng.below(8) << 8),
        );
        let depth_enabled = rng.below(2);
        let depth_written = if depth_enabled == 1 && rng.below(2) == 1 {
            0x1000
        } else {
            0
        };
        let color_mask = rng.below(16);
        set(
            &mut engine,
            0x307,
            depth_enabled | depth_written | (color_mask << 8) | (rng.below(8) << 4),
        );
        set(&mut engine, 0x316, [0u32, 2, 3][rng.below(3) as usize]);
        set(&mut engine, 0x317, rng.below(4) << 16);
        let color_blend = rng.below(2);
        set(&mut engine, 0x300, color_blend << 8);
        set(&mut engine, 0x302, 3);
        set(&mut engine, 0x303, rng.next());
        let mut blend_code = || rng.below(15);
        let rgb_source = blend_code();
        let rgb_destination = blend_code();
        let alpha_source = blend_code();
        let alpha_destination = blend_code();
        let rgb_equation = rng.below(5);
        let alpha_equation = rng.below(5);
        set(
            &mut engine,
            0x301,
            rgb_equation
                | (alpha_equation << 8)
                | (rgb_source << 16)
                | (rgb_destination << 20)
                | (alpha_source << 24)
                | (alpha_destination << 28),
        );
        set(&mut engine, 0x266, 0);
        set(&mut engine, 0x267, 0x0007_0007);
        set(&mut engine, 0x31e, 0x0008_0008);
        set(&mut engine, 0x31c, (region + 0x1000) as u32);
        set(&mut engine, 0x31d, (region + 0x1100) as u32);

        for (start, end) in [
            (region, region + 0x400),
            (region + 0x400, region + 0x500),
            (region + 0x800, region + 0xc00),
            (region + 0xc00, region + 0xd00),
            (region + 0x1000, region + 0x1200),
        ] {
            for byte in &mut memory[start..end] {
                *byte = rng.next() as u8;
            }
        }

        let input = FragmentInput {
            x: rng.below(8),
            y: rng.below(8),
            z: rng.range(-0.1, 1.1),
            coverage: rng.unit(),
            primary: std::array::from_fn(|_| rng.unit()),
            uv: std::array::from_fn(|_| [rng.range(-0.25, 1.25), rng.range(-0.25, 1.25)]),
            lod: [rng.range(-1.5, 2.5), rng.range(-1.5, 2.5)],
        };
        (engine, input)
    }

    #[test]
    fn wgsl_fragment_matches_cpu_for_randomized_state_and_inputs() {
        if skip_without_adapter() {
            return;
        }
        let mut rng = Random(0xa891_7c5d_93b1_42e7);
        let mut initial = vec![0u8; MEMORY_BASE + CASE_COUNT * REGION_SIZE];
        let mut engines = Vec::with_capacity(CASE_COUNT);
        let mut inputs = Vec::with_capacity(CASE_COUNT);
        for index in 0..CASE_COUNT {
            let (engine, input) = random_case(&mut rng, &mut initial, index);
            engines.push(engine);
            inputs.push(input);
        }
        let cases: Vec<_> = engines
            .iter()
            .zip(inputs.iter().copied())
            .map(|(engine, input)| FragmentCase { engine, input })
            .collect();
        let actual = dispatch(&cases, &initial).expect("WGSL fragment compute dispatch");
        let mut expected = initial.clone();
        for (index, case) in cases.iter().enumerate() {
            let input = case.input;
            let mut read = |address: u64, length: usize| {
                let start = usize::try_from(address).map_err(|error| error.to_string())?;
                let end = start
                    .checked_add(length)
                    .ok_or_else(|| "CPU fragment read extent overflow".to_string())?;
                initial
                    .get(start..end)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| "CPU fragment read exceeds test memory".to_string())
            };
            let mut targets = Targets::default();
            raster::fragment_coverage(
                case.engine,
                (input.x as usize, input.y as usize, input.z, input.coverage),
                input.primary,
                input.uv,
                input.lod,
                &mut read,
                &mut targets,
            )
            .unwrap_or_else(|error| panic!("CPU fragment {index}: {error}"));
            for (address, bytes) in targets.into_snapshots() {
                let start = address as usize;
                expected[start..start + bytes.len()].copy_from_slice(&bytes);
            }
        }
        if actual != expected {
            let offset = actual
                .iter()
                .zip(&expected)
                .position(|(actual, expected)| actual != expected)
                .unwrap_or(actual.len().min(expected.len()));
            let case_index = offset.saturating_sub(MEMORY_BASE) / REGION_SIZE;
            let case = &cases[case_index];
            let engine = case.engine;
            panic!(
                "WGSL fragment differs from CPU at byte {offset:#x}, case {case_index}, input {:?}, registers 0x301={:#x} 0x302={:#x} 0x307={:#x} 0x317={:#x}: initial {:?}, GPU {:?}, CPU {:?}",
                case.input,
                engine.reg(0x301),
                engine.reg(0x302),
                engine.reg(0x307),
                engine.reg(0x317),
                initial.get(offset..offset.saturating_add(8)),
                actual.get(offset..offset.saturating_add(8)),
                expected.get(offset..offset.saturating_add(8)),
            );
        }
    }

    #[test]
    fn wgsl_fragment_refuses_unsupported_blend_and_logic_operations() {
        if skip_without_adapter() {
            return;
        }
        let mut rng = Random(0x3f21_1d74_912e_0ab5);
        let mut memory = vec![0u8; MEMORY_BASE + 2 * REGION_SIZE];
        let (mut factor_engine, input) = random_case(&mut rng, &mut memory, 0);
        set(&mut factor_engine, 0x304, 0);
        set(&mut factor_engine, 0x305, 0);
        set(&mut factor_engine, 0x307, 15 << 8);
        set(&mut factor_engine, 0x300, 0x100);
        set(&mut factor_engine, 0x301, 15 << 16);
        let factor_case = [FragmentCase {
            engine: &factor_engine,
            input,
        }];
        assert!(matches!(
            dispatch(&factor_case, &memory),
            Err(error) if error == "DMP fragment 0: DMP unsupported blend factor"
        ));

        let (mut logic_engine, logic_input) = random_case(&mut rng, &mut memory, 1);
        set(&mut logic_engine, 0x304, 0);
        set(&mut logic_engine, 0x305, 0);
        set(&mut logic_engine, 0x307, 15 << 8);
        set(&mut logic_engine, 0x300, 0);
        set(&mut logic_engine, 0x302, 2);
        let logic_case = [FragmentCase {
            engine: &logic_engine,
            input: logic_input,
        }];
        assert!(matches!(
            dispatch(&logic_case, &memory),
            Err(error) if error == "DMP fragment 0: DMP unsupported logic operation"
        ));
    }

    #[test]
    fn wgsl_fragment_refuses_unsupported_texture_codes() {
        if skip_without_adapter() {
            return;
        }
        let mut rng = Random(0xe477_b2c1_6a03_95fd);
        let mut memory = vec![0u8; MEMORY_BASE + REGION_SIZE];
        let (mut engine, input) = random_case(&mut rng, &mut memory, 0);
        set(&mut engine, 0x280, 1);
        set(&mut engine, 0x28e, 6);
        let case = [FragmentCase {
            engine: &engine,
            input,
        }];
        assert!(matches!(
            dispatch(&case, &memory),
            Err(error) if error == "DMP fragment 0: DMP unsupported sampler format 6"
        ));
    }
}
