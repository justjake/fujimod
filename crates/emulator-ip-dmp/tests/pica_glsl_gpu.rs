#![cfg(not(target_arch = "wasm32"))]

use fujimod_emulator_ip_dmp::{
    gles::{Engine, float24},
    pica::translate_vertex_to_wgsl,
};
use std::{
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake},
};
use wgpu::util::DeviceExt;

const VERTICES: u32 = 3;
const OUTPUTS: usize = 16;
const OUTPUT_STRIDE: usize = OUTPUTS + 1;
const CAPTURE_BYTES: u64 = (VERTICES as usize * OUTPUT_STRIDE * 16) as u64;

struct ThreadWake(std::thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Arc::new(ThreadWake(std::thread::current())).into();
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[derive(Clone)]
struct ProgramCase {
    name: String,
    words: Vec<u32>,
    descriptors: Vec<u32>,
    uniforms: [[u32; 3]; 96],
    integer_params: [u32; 4],
    integers: [[i32; 4]; 4],
    bools: u32,
    inputs: [[f32; 4]; 16],
}

impl ProgramCase {
    fn new(name: impl Into<String>, words: Vec<u32>, descriptors: Vec<u32>) -> Self {
        let uniforms = std::array::from_fn(|slot| {
            pack24([
                (slot % 8 + 1) as f32,
                (slot % 7 + 2) as f32,
                (slot % 6 + 3) as f32,
                (slot % 5 + 4) as f32,
            ])
        });
        let mut inputs = [[0.0, 0.0, 0.0, 1.0]; 16];
        inputs[0] = [0.0, 0.0, 0.0, 1.0];
        inputs[1] = [1.0, 2.0, 3.0, 4.0];
        Self {
            name: name.into(),
            words,
            descriptors,
            uniforms,
            integer_params: [0; 4],
            integers: [[0; 4]; 4],
            bools: 0,
            inputs,
        }
    }

    fn set_loop(&mut self, index: usize, repeats: u8, start: u8, increment: u8) {
        self.integer_params[index] =
            repeats as u32 | (start as u32) << 8 | (increment as u32) << 16;
        self.integers[index] = [repeats as i32, start as i32, (increment as i8) as i32, 0];
    }
}

struct ProgramBuilder {
    words: Vec<u32>,
    descriptors: Vec<u32>,
}

impl ProgramBuilder {
    fn new() -> Self {
        Self {
            words: Vec::new(),
            descriptors: Vec::new(),
        }
    }

    fn descriptor(
        &mut self,
        mask: u8,
        source1: ([u8; 4], bool),
        source2: ([u8; 4], bool),
        source3: ([u8; 4], bool),
    ) -> u32 {
        let pattern = |swizzle: [u8; 4]| {
            (u32::from(swizzle[0] & 3) << 6)
                | (u32::from(swizzle[1] & 3) << 4)
                | (u32::from(swizzle[2] & 3) << 2)
                | u32::from(swizzle[3] & 3)
        };
        let word = u32::from(mask & 15)
            | (u32::from(source1.1) << 4)
            | (pattern(source1.0) << 5)
            | (u32::from(source2.1) << 13)
            | (pattern(source2.0) << 14)
            | (u32::from(source3.1) << 22)
            | (pattern(source3.0) << 23);
        let index = self.descriptors.len() as u32;
        self.descriptors.push(word);
        index
    }

    fn arithmetic(
        &mut self,
        opcode: u8,
        destination: u32,
        source1: u32,
        source2: u32,
        relative: u32,
        descriptor: u32,
    ) {
        self.words.push(
            (u32::from(opcode) << 26)
                | ((destination & 31) << 21)
                | ((relative & 3) << 19)
                | ((source1 & 127) << 12)
                | ((source2 & 31) << 7)
                | (descriptor & 127),
        );
    }

    fn mad(
        &mut self,
        destination: u32,
        source1: u32,
        source2: u32,
        source3: u32,
        relative: u32,
        descriptor: u32,
    ) {
        self.words.push(
            (0x38 << 26)
                | ((destination & 31) << 24)
                | ((relative & 3) << 22)
                | ((source1 & 31) << 17)
                | ((source2 & 127) << 10)
                | ((source3 & 31) << 5)
                | (descriptor & 31),
        );
    }

    fn compare(&mut self, x_mode: u32, y_mode: u32, source1: u32, source2: u32, descriptor: u32) {
        self.arithmetic(
            0x2e,
            (x_mode << 3) | y_mode,
            source1,
            source2,
            0,
            descriptor,
        );
    }

    fn end(&mut self) {
        self.words.push(0x22 << 26);
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
}

impl Gpu {
    fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .map_err(|error| format!("PICA differential-test adapter: {error}"))?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            return Err(format!(
                "PICA differential tests require a GPU adapter, got {:?}: {}",
                info.backend, info.name
            ));
        }
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|error| format!("PICA differential-test device: {error}"))?;
        Ok(Self {
            device,
            queue,
            adapter_name: format!("{:?}: {}", info.backend, info.name),
        })
    }

    fn run(&self, case: &ProgramCase) -> Result<Vec<([[u32; 4]; 16], bool)>, String> {
        let semantics = std::array::from_fn(|index| index as u32);
        let mut wgsl = translate_vertex_to_wgsl(&case.words, &case.descriptors, 0, &semantics)?;
        if !wgsl.contains("fn main_1()") || !wgsl.contains("var<private> outputs:") {
            return Err("translated WGSL output layout changed".to_owned());
        }
        wgsl.push_str(
            r#"

@group(0) @binding(8) var<storage, read_write> captures: array<vec4<u32>>;

@compute @workgroup_size(1)
fn capture_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    gl_VertexIndex_1 = global_id.x;
    main_1();
    for (var i: u32 = 0u; i < 16u; i = i + 1u) {
        captures[global_id.x * 17u + i] = bitcast<vec4<u32>>(outputs[i]);
    }
    captures[global_id.x * 17u + 16u] = vec4<u32>(out_valid, 0u, 0u, 0u);
}
"#,
        );
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("DMP PICA translated compute harness"),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            });
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("DMP PICA differential compute pipeline"),
                layout: None,
                module: &module,
                entry_point: Some("capture_main"),
                compilation_options: Default::default(),
                cache: None,
            });

        let shading = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("DMP PICA shader uniforms and inputs"),
                contents: &shading_bytes(case),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let input_bytes = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("DMP PICA vertex bytes"),
                contents: &[0; 16],
                usage: wgpu::BufferUsages::STORAGE,
            });
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DMP PICA captured shader outputs"),
            size: CAPTURE_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DMP PICA output readback"),
            size: CAPTURE_BYTES,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("DMP PICA differential-test bindings"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: shading.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: input_bytes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: output.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("DMP PICA translated-program differential pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(VERTICES, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, CAPTURE_BYTES);
        self.queue.submit([encoder.finish()]);

        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| format!("PICA differential-test GPU poll: {error}"))?;
        receiver
            .recv()
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let bytes = readback
            .slice(..)
            .get_mapped_range()
            .map_err(|error| error.to_string())?;
        let words: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        drop(bytes);
        readback.unmap();

        let captured = (0..VERTICES as usize)
            .map(|vertex| {
                let start = vertex * OUTPUT_STRIDE * 4;
                let outputs = std::array::from_fn(|index| {
                    let at = start + index * 4;
                    [words[at], words[at + 1], words[at + 2], words[at + 3]]
                });
                let valid = words[start + OUTPUTS * 4] != 0;
                (outputs, valid)
            })
            .collect();
        Ok(captured)
    }
}

fn shading_bytes(case: &ProgramCase) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2256);
    for uniform in &case.uniforms {
        for value in unpack24(*uniform) {
            push_u32(&mut bytes, value.to_bits());
        }
    }
    for integer in case.integers {
        for value in integer {
            push_u32(&mut bytes, value as u32);
        }
    }
    push_u32(&mut bytes, case.bools);
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, 0);
    push_u32(&mut bytes, 0);
    for index in 0..24 {
        push_u32(&mut bytes, index as u32);
    }
    for value in [1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 32.0, 32.0] {
        push_u32(&mut bytes, value.to_bits());
    }
    for _ in 0..16 * 4 {
        push_u32(&mut bytes, 0);
    }
    for input in case.inputs {
        for value in input {
            push_u32(&mut bytes, value.to_bits());
        }
    }
    assert_eq!(bytes.len(), 2256);
    bytes
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn pair(words: &mut Vec<u32>, index: u16, value: u32) {
    words.extend([value, 0xf0000 | index as u32]);
}

fn upload(words: &mut Vec<u32>, index: u16, data: &[u32]) {
    for chunk in data.chunks(2048) {
        words.push(chunk[0]);
        words.push(((chunk.len() as u32 - 1) << 20) | 0xf0000 | index as u32);
        words.extend_from_slice(&chunk[1..]);
        if chunk.len() % 2 == 0 {
            words.push(0);
        }
    }
}

fn reference(case: &ProgramCase) -> Result<[[u32; 4]; 16], String> {
    let mut commands = Vec::new();
    pair(&mut commands, 0x48a, 0);
    pair(&mut commands, 0x480, case.bools);
    pair(&mut commands, 0x402, 1 << 28);
    pair(&mut commands, 0x48b, 0x10);
    pair(&mut commands, 0x432, 15);
    for (index, params) in case.integer_params.iter().copied().enumerate() {
        pair(&mut commands, 0x481 + index as u16, params);
    }
    pair(&mut commands, 0x49b, 0);
    upload(&mut commands, 0x49c, &case.words);
    pair(&mut commands, 0x4a5, 0);
    upload(&mut commands, 0x4a6, &case.descriptors);
    for (slot, words) in case.uniforms.iter().copied().enumerate() {
        pair(&mut commands, 0x490, slot as u32);
        for (offset, value) in words.into_iter().enumerate() {
            pair(&mut commands, 0x491 + offset as u16, value);
        }
    }
    for input in case.inputs.iter().take(2) {
        upload(&mut commands, 0x433, &pack24(*input));
    }

    let mut engine = Engine::default();
    engine.execute_cpu(&commands, &mut |_, _| {
        panic!("immediate inputs need no DMA")
    })?;
    let diagnostics = engine.diagnostics();
    let outputs = diagnostics["last_output"]
        .as_array()
        .ok_or_else(|| "DMP interpreter did not return vertex outputs".to_owned())?;
    let mut result = [[0; 4]; 16];
    for (register, values) in outputs.iter().enumerate() {
        for (component, value) in values
            .as_array()
            .ok_or_else(|| "DMP output register is not a vector".to_owned())?
            .iter()
            .enumerate()
        {
            result[register][component] = (value
                .as_f64()
                .ok_or_else(|| "DMP output component is not finite".to_owned())?
                as f32)
                .to_bits();
        }
    }
    Ok(result)
}

fn pack24(values: [f32; 4]) -> [u32; 3] {
    let values = values.map(|value| {
        let bits = value.to_bits();
        if bits & 0x7fff_ffff == 0 {
            (bits >> 31) << 23
        } else {
            ((bits >> 31) << 23) | (((bits >> 23 & 255) - 64) << 16) | (bits >> 7 & 65535)
        }
    });
    [
        (values[2] >> 16) | (values[3] << 8),
        (values[1] >> 8 & 65535) | (values[2] << 16),
        values[0] | (values[1] << 24),
    ]
}

fn pack_raw24(values: [u32; 4]) -> [u32; 3] {
    [
        (values[2] >> 16) | (values[3] << 8),
        (values[1] >> 8 & 65535) | (values[2] << 16),
        values[0] | (values[1] << 24),
    ]
}

fn unpack24(words: [u32; 3]) -> [f32; 4] {
    [
        float24(words[2] & 0x00ff_ffff),
        float24((words[2] >> 24) | (words[1] << 8 & 0x00ff_ffff)),
        float24((words[1] >> 16) | (words[0] << 16 & 0x00ff_ffff)),
        float24(words[0] >> 8),
    ]
}

fn identity() -> [u8; 4] {
    [0, 1, 2, 3]
}

fn fixed_programs() -> Vec<ProgramCase> {
    let existing = ProgramCase::new(
        "existing uploaded MOV fixture",
        vec![0x4c00_0000, 0x4c20_1000, 0x8800_0000],
        vec![0x8d86_c36f],
    );

    let mut temporary = ProgramBuilder::new();
    let identity = identity();
    let descriptor =
        temporary.descriptor(15, (identity, false), (identity, false), (identity, false));
    temporary.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    temporary.arithmetic(0x13, 1, 16, 0, 0, descriptor);
    temporary.end();
    let temporary = ProgramCase::new(
        "zero initialized temporary",
        temporary.words,
        temporary.descriptors,
    );

    let mut denormal = ProgramBuilder::new();
    let descriptor =
        denormal.descriptor(15, (identity, false), (identity, false), (identity, false));
    denormal.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    denormal.arithmetic(0x13, 1, 32, 0, 0, descriptor);
    denormal.end();
    let mut denormal = ProgramCase::new(
        "float24 exponent-zero denormal",
        denormal.words,
        denormal.descriptors,
    );
    denormal.uniforms[0] = pack_raw24([1, 1 << 16, 1 << 16, 1 << 16]);

    let mut call = ProgramBuilder::new();
    let descriptor = call.descriptor(15, (identity, false), (identity, false), (identity, false));
    call.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    call.words.push(flow_word(0x24, 4, 1, 0));
    call.end();
    call.words.push(0x21 << 26);
    call.arithmetic(0x00, 2, 1, 1, 0, descriptor);
    call.end();
    let call = ProgramCase::new("subroutine call", call.words, call.descriptors);

    let mut conditional = ProgramBuilder::new();
    let descriptor =
        conditional.descriptor(15, (identity, false), (identity, false), (identity, false));
    conditional.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    conditional.compare(2, 2, 1, 0, descriptor);
    conditional
        .words
        .push(flow_word(0x28, 4, 1, (1 << 25) | (2 << 22)));
    conditional.arithmetic(0x13, 2, 1, 0, 0, descriptor);
    conditional.arithmetic(0x13, 2, 32, 0, 0, descriptor);
    conditional.end();
    let conditional = ProgramCase::new(
        "conditional branch",
        conditional.words,
        conditional.descriptors,
    );

    let mut loop_program = ProgramBuilder::new();
    let descriptor =
        loop_program.descriptor(15, (identity, false), (identity, false), (identity, false));
    loop_program.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    loop_program.words.push(flow_word(0x29, 3, 0, 0));
    loop_program.arithmetic(0x00, 1, 32, 1, 3, descriptor);
    loop_program.arithmetic(0x00, 2, 1, 0, 0, descriptor);
    loop_program.end();
    let mut loop_program = ProgramCase::new(
        "relative uniform in loop",
        loop_program.words,
        loop_program.descriptors,
    );
    loop_program.set_loop(0, 2, 1, 1);

    let mut unsigned_loop = ProgramBuilder::new();
    let descriptor =
        unsigned_loop.descriptor(15, (identity, false), (identity, false), (identity, false));
    unsigned_loop.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    unsigned_loop.words.push(flow_word(0x29, 3, 0, 0));
    unsigned_loop.arithmetic(0x13, 1, 32, 0, 3, descriptor);
    unsigned_loop.arithmetic(0x00, 2, 1, 1, 0, descriptor);
    unsigned_loop.end();
    let mut unsigned_loop = ProgramCase::new(
        "unsigned loop increment byte",
        unsigned_loop.words,
        unsigned_loop.descriptors,
    );
    unsigned_loop.set_loop(0, 1, 1, 255);

    let mut plain_multiply = ProgramBuilder::new();
    let descriptor =
        plain_multiply.descriptor(15, (identity, false), (identity, false), (identity, false));
    plain_multiply.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    plain_multiply.arithmetic(0x08, 1, 32, 0, 0, descriptor);
    plain_multiply.end();
    let mut plain_multiply = ProgramCase::new(
        "plain multiply preserves zero times infinity",
        plain_multiply.words,
        plain_multiply.descriptors,
    );
    plain_multiply.uniforms[0] = pack_raw24([0x007f_0000, 0x007f_0000, 0x007f_0000, 0]);

    let mut invalid_compare = ProgramBuilder::new();
    let descriptor =
        invalid_compare.descriptor(15, (identity, false), (identity, false), (identity, false));
    invalid_compare.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    invalid_compare.compare(3, 7, 0, 1, descriptor);
    invalid_compare.end();
    let invalid_compare = ProgramCase::new(
        "interpreter-refused comparison encoding",
        invalid_compare.words,
        invalid_compare.descriptors,
    );

    let mut invalid_break = ProgramBuilder::new();
    let descriptor =
        invalid_break.descriptor(15, (identity, false), (identity, false), (identity, false));
    invalid_break.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    invalid_break.words.push(0x20 << 26);
    invalid_break.end();
    let invalid_break = ProgramCase::new(
        "interpreter-refused BREAK",
        invalid_break.words,
        invalid_break.descriptors,
    );

    let mut invalid_breakc = ProgramBuilder::new();
    let descriptor =
        invalid_breakc.descriptor(15, (identity, false), (identity, false), (identity, false));
    invalid_breakc.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    invalid_breakc.words.push(0x23 << 26);
    invalid_breakc.end();
    let invalid_breakc = ProgramCase::new(
        "interpreter-refused BREAKC",
        invalid_breakc.words,
        invalid_breakc.descriptors,
    );

    vec![
        existing,
        temporary,
        denormal,
        call,
        conditional,
        loop_program,
        unsigned_loop,
        plain_multiply,
        invalid_compare,
        invalid_break,
        invalid_breakc,
    ]
}

fn flow_word(opcode: u8, target: u32, count: u32, flags: u32) -> u32 {
    (u32::from(opcode) << 26) | flags | ((target & 4095) << 10) | (count & 255)
}

fn randomized_programs() -> Vec<ProgramCase> {
    let mut state = 0x1a2b_3c4d_u32;
    let identity = identity();
    let mut cases = Vec::new();
    for sequence in 0..24 {
        let mut builder = ProgramBuilder::new();
        let base = builder.descriptor(15, (identity, false), (identity, false), (identity, false));
        builder.arithmetic(0x13, 0, 0, 0, 0, base);
        if sequence % 8 == 0 {
            builder.compare(3, 7, 0, 1, base);
            builder.end();
            cases.push(ProgramCase::new(
                format!("interpreter-refused randomized compare {sequence}"),
                builder.words,
                builder.descriptors,
            ));
            continue;
        }
        let mut saw_relative = false;
        for operation in 0..4 {
            let relative = if (sequence + operation) % 3 == 0 {
                saw_relative = true;
                (next_random(&mut state) % 3 + 1) as u32
            } else {
                0
            };
            let source1 = if relative != 0 {
                32 + (next_random(&mut state) % 8)
            } else if next_random(&mut state) & 1 != 0 {
                next_random(&mut state) % 2
            } else {
                16 + next_random(&mut state) % 4
            };
            let source2 = if next_random(&mut state) & 1 != 0 {
                next_random(&mut state) % 2
            } else {
                16 + next_random(&mut state) % 4
            };
            let first = (random_swizzle(&mut state), next_random(&mut state) & 1 != 0);
            let second = (random_swizzle(&mut state), next_random(&mut state) & 1 != 0);
            let mask = (next_random(&mut state) % 15 + 1) as u8;
            let descriptor = builder.descriptor(mask, first, second, (identity, false));
            let opcodes = [
                0x00, 0x01, 0x02, 0x03, 0x04, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            ];
            let opcode = opcodes[(next_random(&mut state) as usize) % opcodes.len()];
            builder.arithmetic(
                opcode,
                1 + operation as u32,
                source1,
                source2,
                relative,
                descriptor,
            );
        }
        let src1 = if saw_relative { 32 } else { 1 };
        let relative = if saw_relative { 1 } else { 0 };
        let source1 = random_swizzle(&mut state);
        let descriptor = builder.descriptor(
            15,
            (source1, next_random(&mut state) & 1 != 0),
            (identity, next_random(&mut state) & 1 != 0),
            (identity, next_random(&mut state) & 1 != 0),
        );
        builder.mad(
            5,
            0,
            if saw_relative { 32 } else { 1 },
            src1,
            relative,
            descriptor,
        );
        builder.end();
        cases.push(ProgramCase::new(
            format!("random seed 0x{sequence:08x}"),
            builder.words,
            builder.descriptors,
        ));
    }
    cases
}

fn next_random(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

fn random_swizzle(state: &mut u32) -> [u8; 4] {
    [
        (next_random(state) & 3) as u8,
        (next_random(state) & 3) as u8,
        (next_random(state) & 3) as u8,
        (next_random(state) & 3) as u8,
    ]
}

fn call_depth_program(depth: usize) -> ProgramCase {
    let mut builder = ProgramBuilder::new();
    let identity = identity();
    let descriptor =
        builder.descriptor(15, (identity, false), (identity, false), (identity, false));
    builder.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    builder.words.push(flow_word(0x24, 100, 1, 0));
    builder.end();
    let mut code = vec![0x21 << 26; depth * 100 + 2];
    code[..builder.words.len()].copy_from_slice(&builder.words);
    for level in 1..depth {
        let at = level * 100;
        code[at] = flow_word(0x24, ((level + 1) * 100) as u32, 1, 0);
    }
    ProgramCase::new(format!("{depth} nested calls"), code, builder.descriptors)
}

fn over_budget_program() -> ProgramCase {
    let identity = identity();
    let mut builder = ProgramBuilder::new();
    let descriptor =
        builder.descriptor(15, (identity, false), (identity, false), (identity, false));
    builder.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    builder.words.push(flow_word(0x29, 4, 0, 0));
    builder.words.push(flow_word(0x29, 3, 1, 0));
    builder.words.push(0x21 << 26);
    builder.words.push(0x21 << 26);
    builder.end();
    let mut case = ProgramCase::new(
        "nested loops exceed 16,384 instructions",
        builder.words,
        builder.descriptors,
    );
    case.set_loop(0, 255, 0, 0);
    case.set_loop(1, 255, 0, 0);
    case
}

fn nonfinite_output_program(raw: u32) -> ProgramCase {
    let identity = identity();
    let mut builder = ProgramBuilder::new();
    let descriptor =
        builder.descriptor(15, (identity, false), (identity, false), (identity, false));
    builder.arithmetic(0x13, 0, 0, 0, 0, descriptor);
    builder.arithmetic(0x13, 1, 32, 0, 0, descriptor);
    builder.end();
    let mut case = ProgramCase::new(
        "nonfinite float24 final output",
        builder.words,
        builder.descriptors,
    );
    case.uniforms[0] = pack_raw24([raw, 0x3f0000, 0x3f0000, 0x3f0000]);
    case
}

fn assert_gpu_matches(gpu: &Gpu, case: &ProgramCase) {
    let expected = reference(case).unwrap_or_else(|error| {
        panic!(
            "{}: interpreter refused a comparison case: {error}",
            case.name
        )
    });
    let actual = gpu
        .run(case)
        .unwrap_or_else(|error| panic!("{}: GPU translator failed: {error}", case.name));
    assert_eq!(actual.len(), VERTICES as usize, "{}", case.name);
    for (vertex, (outputs, valid)) in actual.into_iter().enumerate() {
        assert!(
            valid,
            "{}: translated shader refused vertex {vertex}",
            case.name
        );
        assert_eq!(
            outputs, expected,
            "{}: vertex {vertex}; words={:08x?}, descriptors={:08x?}",
            case.name, case.words, case.descriptors
        );
    }
}

#[test]
fn translated_programs_match_the_dmp_interpreter_on_the_gpu() {
    let gpu = Gpu::new().expect("a hardware GPU adapter is required for PICA conformance");
    eprintln!("PICA vertex conformance adapter: {}", gpu.adapter_name);
    for case in fixed_programs().into_iter().chain(randomized_programs()) {
        if case.name.contains("interpreter-refused") {
            assert!(reference(&case).is_err(), "{}", case.name);
            let semantics = std::array::from_fn(|index| index as u32);
            assert!(
                translate_vertex_to_wgsl(&case.words, &case.descriptors, 0, &semantics).is_err(),
                "{}",
                case.name
            );
            continue;
        }
        if case.name == "unsigned loop increment byte"
            || case.name == "plain multiply preserves zero times infinity"
        {
            assert!(reference(&case).is_err(), "{}", case.name);
            let actual = gpu
                .run(&case)
                .unwrap_or_else(|error| panic!("{}: GPU translator failed: {error}", case.name));
            assert!(
                actual.iter().all(|(_, valid)| !valid),
                "{}: expected the translated invocation to be refused",
                case.name
            );
            continue;
        }
        assert_gpu_matches(&gpu, &case);
    }

    assert_gpu_matches(&gpu, &call_depth_program(32));
    let overflow = call_depth_program(33);
    assert!(
        reference(&overflow).is_err(),
        "33-frame interpreter refusal"
    );
    let actual = gpu
        .run(&overflow)
        .expect("translator must expose stack refusal");
    assert!(actual.iter().all(|(_, valid)| !valid));

    let over_budget = over_budget_program();
    assert!(
        reference(&over_budget).is_err(),
        "instruction-budget refusal"
    );
    let actual = gpu
        .run(&over_budget)
        .expect("translator must expose instruction-budget refusal");
    assert!(actual.iter().all(|(_, valid)| !valid));

    let nonfinite = nonfinite_output_program((127 << 16) | 0x1234);
    assert!(
        reference(&nonfinite).is_err(),
        "nonfinite DMP output refusal"
    );
    let actual = gpu
        .run(&nonfinite)
        .expect("translator must expose nonfinite output refusal");
    assert!(actual.iter().all(|(_, valid)| !valid));
    let expected_nan = float24((127 << 16) | 0x1234).to_bits();
    assert_eq!(actual[0].0[1][0], expected_nan, "float24 NaN payload");

    let plain_multiply = fixed_programs()
        .into_iter()
        .find(|case| case.name == "plain multiply preserves zero times infinity")
        .unwrap();
    let actual = gpu.run(&plain_multiply).unwrap();
    assert!(actual.iter().all(|(_, valid)| !valid));
}

#[test]
fn float24_translator_decoder_matches_the_dmp_interpreter() {
    for raw in [
        0,
        1,
        0x0080_0001,
        0x003f_0000,
        0x007f_0000,
        0x007f_0001,
        0x00ff_1234,
    ] {
        assert_eq!(
            fujimod_emulator_ip_dmp::pica::isa::decode_float24(raw).to_bits(),
            float24(raw).to_bits(),
            "raw float24 {raw:#08x}"
        );
    }
}
