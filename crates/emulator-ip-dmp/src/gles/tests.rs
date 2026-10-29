use super::*;

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn engine_renderer_selection_defaults_to_gpu_and_supports_overrides() {
    assert_eq!(host_gpu_mode_for(false, false), (true, false));
    assert_eq!(host_gpu_mode_for(true, false), (false, false));
    assert_eq!(host_gpu_mode_for(true, true), (true, true));
}

const TEST_SHADER_PROGRAM: [u32; 3] = [0x4c000000, 0x4c201000, 0x88000000];
const TEST_SHADER_DESCRIPTOR: u32 = 0x8d86c36f;

fn pair(words: &mut Vec<u32>, index: u16, value: u32) {
    words.extend([value, 0xf0000 | index as u32]);
}
fn packed(v: Vector) -> [u32; 3] {
    let f = v.map(|f| {
        let b = f.to_bits();
        if b & 0x7fffffff == 0 {
            0
        } else {
            ((b >> 31) << 23) | (((b >> 23 & 255) - 64) << 16) | (b >> 7 & 65535)
        }
    });
    [
        (f[2] >> 16) | (f[3] << 8),
        (f[1] >> 8 & 65535) | (f[2] << 16),
        f[0] | (f[1] << 24),
    ]
}
fn upload(words: &mut Vec<u32>, index: u16, data: &[u32], increment: bool) {
    words.push(data[0]);
    words.push(
        (if increment { 1 << 31 } else { 0 })
            | ((data.len() as u32 - 1) << 20)
            | 0xf0000
            | index as u32,
    );
    words.extend_from_slice(&data[1..]);
    if data.len() % 2 == 0 {
        words.push(0xa5a5a5a5);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn triangle_list(registers: &[(u16, u32)]) -> Vec<u32> {
    let mut words = setup();
    for &(register, value) in registers {
        pair(&mut words, register, value);
    }
    for position in [
        [-0.83, -0.71, -0.5, 1.],
        [0.72, -0.65, -0.5, 1.],
        [-0.13, 0.91, -0.5, 1.],
    ] {
        upload(&mut words, 0x433, &packed(position), true);
        upload(&mut words, 0x433, &packed([0.75, 0.25, 0.5, 0.5]), true);
    }
    words
}

#[cfg(not(target_arch = "wasm32"))]
fn assert_host_gpu_matches_cpu(
    words: &[u32],
    read: impl Fn(u64, usize) -> Vec<u8>,
) -> Option<Snapshots> {
    let mut cpu = Engine::default();
    let expected = cpu
        .execute_cpu(words, &mut |address, length| Ok(read(address, length)))
        .unwrap();
    let actual = match Engine::default()
        .execute(words, &mut |address, length| Ok(read(address, length)))
    {
        Ok(actual) => actual,
        Err(error) if error.starts_with("host GPU adapter:") => {
            eprintln!(
                "skipping DMP host render parity because no host GPU adapter is available: {error}"
            );
            return None;
        }
        Err(error) => panic!("DMP host render path failed: {error}"),
    };
    if actual != expected {
        panic!("{}", super::gpu_draw::snapshot_mismatch(&actual, &expected));
    }
    if std::env::var("FUJIMOD_DMP_VERIFY_GPU").is_ok_and(|value| value == "1") {
        let verified = Engine::default()
            .execute_host_gpu_verified(words, &mut |address, length| Ok(read(address, length)))
            .unwrap_or_else(|error| panic!("DMP host GPU verification failed: {error}"));
        assert_eq!(verified, expected);
    }
    Some(actual)
}

fn setup() -> Vec<u32> {
    let mut words = Vec::new();
    for (r, v) in [
        (0x250, 0x03020100),
        (0x251, 0x0b0a0908),
        (0x252, 0x0f0e0d0c),
        (0x253, 0x1f1f1f1f),
        (0x254, 0x1f1f1f1f),
        (0x255, 0x1f1f1f1f),
        (0x256, 0x1f1f1f1f),
        (0x241, 0x430000),
        (0x243, 0x430000),
        (0x24d, 0x3f0000),
        (0x24e, 0x3f0000),
        (0x267, 0x001f001f),
        (0x31e, 0x00200020),
        (0x31d, 0x10000),
        (0x307, 0xf00),
        (0x300, 0x100),
        (0x301, 0x01010000),
        (0x402, 1 << 28),
        (0x48b, 0x10),
        (0x432, 15),
        (0x49b, 0),
        (0x4a5, 0),
    ] {
        pair(&mut words, r, v);
    }
    upload(&mut words, 0x49c, &TEST_SHADER_PROGRAM, false);
    upload(&mut words, 0x4a6, &[TEST_SHADER_DESCRIPTOR], false);
    words
}

#[test]
fn uploaded_mov_program_translates_to_wgsl_via_naga() {
    let semantics = std::array::from_fn(|index| index as u32);
    let wgsl = crate::pica::translate_vertex_to_wgsl(
        &TEST_SHADER_PROGRAM,
        &[TEST_SHADER_DESCRIPTOR],
        0,
        &semantics,
    )
    .unwrap();
    assert!(wgsl.contains("@vertex"), "{wgsl}");
}

#[test]
fn uploaded_mov_program_draws_triangle_with_exact_top_left_coverage() {
    let mut words = setup();
    for position in [
        [-0.75, -0.75, 0., 1.],
        [0.75, -0.75, 0., 1.],
        [0., 0.75, 0., 1.],
    ] {
        upload(&mut words, 0x433, &packed(position), true);
        upload(&mut words, 0x433, &packed([1., 0., 0., 1.]), true);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if assert_host_gpu_matches_cpu(&words, |_, length| vec![0; length]).is_none() {
        return;
    }
    let writes = Engine::default()
        .execute_cpu(&words, &mut |a, n| {
            assert_eq!(a, 0x10000);
            Ok(vec![0; n])
        })
        .unwrap();
    assert_eq!(writes.len(), 1);
    let bytes = &writes[0].1;
    assert_eq!(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| **p == [255, 0, 0, 255])
            .count(),
        288
    );
    assert!(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0; 4] || *p == [255, 0, 0, 255])
    );
}

#[test]
fn unused_nonfinite_uniform_words_survive_upload_checkpoint_and_draw() {
    let mut words = setup();
    pair(&mut words, 0x490, 0x80000006);
    for value in [0x3f800000, 0x7fffffff, 0x7fffffff, 0x00001d00] {
        pair(&mut words, 0x491, value);
    }
    for position in [
        [-0.75, -0.75, 0., 1.],
        [0.75, -0.75, 0., 1.],
        [0., 0.75, 0., 1.],
    ] {
        upload(&mut words, 0x433, &packed(position), true);
        upload(&mut words, 0x433, &packed([1., 0., 0., 1.]), true);
    }

    #[cfg(not(target_arch = "wasm32"))]
    if assert_host_gpu_matches_cpu(&words, |_, length| vec![0; length]).is_none() {
        return;
    }

    let mut engine = Engine::default();
    let writes = engine
        .execute_cpu(&words, &mut |a, n| {
            assert_eq!(a, 0x10000);
            Ok(vec![0; n])
        })
        .unwrap();
    assert_eq!(
        engine.uniforms[6],
        [0x00001d00, 0x7fffffff, 0x7fffffff, 0x3f800000]
    );
    assert_eq!(
        writes[0]
            .1
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| **p == [255, 0, 0, 255])
            .count(),
        288
    );

    let checkpoint = serde_json::to_value(&engine).unwrap();
    let restored: Engine = serde_json::from_value(checkpoint).unwrap();
    assert_eq!(restored.uniforms[6], engine.uniforms[6]);
}

#[test]
fn failed_list_rolls_back_program_registers_and_outputs() {
    let mut engine = Engine::default();
    let before = serde_json::to_value(&engine).unwrap();
    let mut words = setup();
    pair(&mut words, 0xdead, 1);
    assert!(
        engine
            .execute_cpu(&words, &mut |_, _| panic!("no draw DMA"))
            .is_err()
    );
    assert_eq!(serde_json::to_value(&engine).unwrap(), before);
}
#[test]
fn packet_padding_is_opaque_and_truncation_refuses() {
    let mut engine = Engine::default();
    let mut words = vec![];
    upload(&mut words, 0x49c, &[0x84000000, 0x88000000], false);
    assert!(engine.execute_cpu(&words, &mut |_, _| panic!()).is_ok());
    words.pop();
    assert!(engine.execute_cpu(&words, &mut |_, _| panic!()).is_err());
}

#[test]
fn host_gpu_and_cpu_produce_identical_pixels_and_preserve_constant_alpha() {
    let mut words = setup();
    for position in [
        [-0.83, -0.71, -0.5, 1.],
        [0.72, -0.65, -0.5, 1.],
        [-0.13, 0.91, -0.5, 1.],
    ] {
        upload(&mut words, 0x433, &packed(position), true);
        upload(&mut words, 0x433, &packed([1., 0.25, 0.5, 1.]), true);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if assert_host_gpu_matches_cpu(&words, |_, length| vec![0; length]).is_none() {
        return;
    }
    let mut cpu = Engine::default();
    let expected = cpu.execute_cpu(&words, &mut |_, n| Ok(vec![0; n])).unwrap();
    let actual = Engine::default()
        .execute(&words, &mut |_, n| Ok(vec![0; n]))
        .unwrap();
    assert_eq!(actual, expected);
    assert!(
        actual[0]
            .1
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[0] == 0 || p[0] == 255)
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn host_render_path_matches_cpu_snapshots_and_written_ranges() {
    let mut words = setup();
    for position in [
        [-0.83, -0.71, -0.5, 1.],
        [0.72, -0.65, -0.5, 1.],
        [-0.13, 0.91, -0.5, 1.],
    ] {
        upload(&mut words, 0x433, &packed(position), true);
        upload(&mut words, 0x433, &packed([1., 0.25, 0.5, 1.]), true);
    }
    let mut cpu = Engine::default();
    let expected = cpu
        .execute_cpu(&words, &mut |_, length| Ok(vec![0; length]))
        .unwrap();
    let actual = match Engine::default()
        .execute_host_gpu(&words, &mut |_, length| Ok(vec![0; length]))
    {
        Ok(actual) => actual,
        Err(error) if error.starts_with("host GPU adapter:") => {
            eprintln!(
                "skipping DMP host render test because no host GPU adapter is available: {error}"
            );
            return;
        }
        Err(error) => panic!("DMP host render path failed: {error}"),
    };
    assert_eq!(actual, expected);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn host_render_path_matches_dmp_fragment_state_regressions() {
    for format in [0, 1, 2, 3, 4, 5, 7, 8] {
        let words = triangle_list(&[
            (0x280, 1),
            (0x282, 0x0008_0008),
            (0x283, 0),
            (0x284, 0),
            (0x285, 0x40000),
            (0x28e, format),
            (0x2c0, 0x0003_0003),
            (0x2c2, 0),
            (0x2c8, 0x000f_000f),
            (0x2ca, 0),
        ]);
        let initial_texture = if format == 8 { 0x80 } else { 0x5a };
        let Some(actual) = assert_host_gpu_matches_cpu(&words, |address, length| {
            if address == 0x40000 {
                vec![initial_texture; length]
            } else {
                vec![0; length]
            }
        }) else {
            return;
        };
        if format == 8 {
            assert!(
                actual[0]
                    .1
                    .as_chunks::<4>()
                    .0
                    .contains(&[0x80, 255, 255, 255])
            );
        }
    }

    let blend = (6 << 16) | (7 << 20) | (1 << 24);
    let blend_words = triangle_list(&[(0x300, 0x100), (0x301, blend)]);
    if assert_host_gpu_matches_cpu(&blend_words, |_, length| vec![0x60; length]).is_none() {
        return;
    }

    let mask_words = triangle_list(&[(0x307, 1 << 8)]);
    if assert_host_gpu_matches_cpu(&mask_words, |_, length| vec![0x60; length]).is_none() {
        return;
    }

    let combiner_words = triangle_list(&[
        (0x280, 1),
        (0x282, 0x0008_0008),
        (0x283, 0),
        (0x284, 0),
        (0x285, 0x40000),
        (0x28e, 0),
        (0x2c0, 0x0030_0030),
        (0x2c2, 0x0001_0001),
        (0x2c8, 0x000f_000f),
        (0x2ca, 0),
    ]);
    if assert_host_gpu_matches_cpu(&combiner_words, |address, length| {
        if address == 0x40000 {
            vec![0x83; length]
        } else {
            vec![0; length]
        }
    })
    .is_none()
    {
        return;
    }

    for (fog_control, table_words) in [(1, false), (5, true)] {
        let mut registers = vec![
            (0x2e0, fog_control),
            (0x2e1, 0x0000_ff00),
            (0x2e2, 1024),
            (0x2e3, 0),
        ];
        if table_words {
            registers.push((0x2e6, 0));
        }
        let mut fog_list = setup();
        for &(register, value) in &registers {
            pair(&mut fog_list, register, value);
        }
        if table_words {
            for _ in 0..128 {
                pair(&mut fog_list, 0x2e8, (1024 << 13) | 256);
            }
        }
        for position in [
            [-0.83, -0.71, -0.5, 1.],
            [0.72, -0.65, -0.5, 1.],
            [-0.13, 0.91, -0.5, 1.],
        ] {
            upload(&mut fog_list, 0x433, &packed(position), true);
            upload(&mut fog_list, 0x433, &packed([0.75, 0.25, 0.5, 0.5]), true);
        }
        if assert_host_gpu_matches_cpu(&fog_list, |_, length| vec![0; length]).is_none() {
            return;
        }
    }

    let depth_stencil_words = triangle_list(&[
        (0x316, 3),
        (0x31c, 0x20000),
        (
            0x305,
            1 | (1 << 4) | (0xff << 8) | (0x80 << 16) | (0xff << 24),
        ),
        (0x306, 2 << 8),
        (0x307, 0x1f11),
    ]);
    if assert_host_gpu_matches_cpu(&depth_stencil_words, |_, length| vec![0; length]).is_none() {
        return;
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn host_render_path_preserves_fragment_refusals() {
    for format in [6, 9] {
        let words = triangle_list(&[
            (0x280, 1),
            (0x282, 0x0008_0008),
            (0x285, 0x40000),
            (0x28e, format),
        ]);
        let error = Engine::default()
            .execute_host_gpu(&words, &mut |_, length| Ok(vec![0; length]))
            .unwrap_err();
        assert!(error.contains(&format!("DMP unsupported sampler format {format}")));
    }

    let factor_words = triangle_list(&[(0x300, 0x100), (0x301, 15 << 16), (0x302, 3)]);
    assert_eq!(
        Engine::default()
            .execute_host_gpu(&factor_words, &mut |_, length| Ok(vec![0; length]))
            .unwrap_err(),
        "DMP unsupported blend factor"
    );

    let logic_words = triangle_list(&[(0x300, 0), (0x302, 2)]);
    assert_eq!(
        Engine::default()
            .execute_host_gpu(&logic_words, &mut |_, length| Ok(vec![0; length]))
            .unwrap_err(),
        "DMP unsupported logic operation"
    );
}

#[test]
fn cpu_scan_conversion_matches_integer_reference_at_shared_edges_and_subpixels() {
    for seed in 1..24i64 {
        let q = [
            [seed, seed * 3],
            [16 * 32 - seed, seed + 2],
            [seed * 5, 16 * 32 - seed * 2],
        ];
        let mut words = setup();
        for [x, y] in q {
            upload(
                &mut words,
                0x433,
                &packed([x as f32 / 256. - 1., y as f32 / 256. - 1., -0.5, 1.]),
                true,
            );
            upload(&mut words, 0x433, &packed([1.; 4]), true);
        }
        let mut engine = Engine::default();
        let snapshots = engine
            .execute_cpu(&words, &mut |_, length| Ok(vec![0; length]))
            .unwrap();
        let pixels = &snapshots[0].1;
        let mut oriented = q;
        if edge(oriented[0], oriented[1], oriented[2]) < 0 {
            oriented.swap(1, 2);
        }
        let edges = [
            (oriented[1], oriented[2]),
            (oriented[2], oriented[0]),
            (oriented[0], oriented[1]),
        ];
        let inclusive = edges.map(|(a, b)| b[1] < a[1] || (b[1] == a[1] && b[0] > a[0]));
        for y in 0..32usize {
            for x in 0..32usize {
                let point = [x as i64 * 16 + 8, y as i64 * 16 + 8];
                let covered = edges.iter().enumerate().all(|(index, (a, b))| {
                    let value = edge(*a, *b, point);
                    value > 0 || value == 0 && inclusive[index]
                });
                let morton = (y & !7) * 32
                    + (x & !7) * 8
                    + (x & 1)
                    + ((y & 1) << 1)
                    + ((x & 2) << 1)
                    + ((y & 2) << 2)
                    + ((x & 4) << 2)
                    + ((y & 4) << 3);
                let actual = pixels[morton * 4..morton * 4 + 4] != [0; 4];
                assert_eq!(actual, covered, "seed {seed}, pixel ({x}, {y})");
            }
        }
    }
}

fn edge(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> i64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

#[test]
fn linked_streams_execute_and_cycles_refuse_without_dma_output() {
    let mut words = vec![];
    pair(&mut words, 0x438, 1);
    pair(&mut words, 0x43a, 0x400);
    pair(&mut words, 0x43c, 1);
    let mut engine = Engine::default();
    let end = [0x12345678u32, 0xf0210];
    assert!(
        engine
            .execute_cpu(&words, &mut |a, n| {
                assert_eq!((a, n), (0x2000, 8));
                Ok(end.iter().flat_map(|w| w.to_le_bytes()).collect())
            })
            .unwrap()
            .is_empty()
    );
    // A length-24 link that points to itself never acquires a framebuffer.
    words[0] = 3;
    let raw: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    assert!(engine.execute_cpu(&words, &mut |_, _| Ok(raw.clone())).is_err());
}

#[test]
fn checkpoint_between_vertices_retains_uploaded_program_and_primitive_assembly() {
    let positions = [
        [-0.75, -0.75, 0., 1.],
        [0.75, -0.75, 0., 1.],
        [0., 0.75, 0., 1.],
    ];
    let mut first = setup();
    for position in &positions[..2] {
        upload(&mut first, 0x433, &packed(*position), true);
        upload(&mut first, 0x433, &packed([1., 0., 0., 1.]), true);
    }
    let mut engine = Engine::default();
    assert!(
        engine
            .execute_cpu(&first, &mut |_, _| panic!(
                "incomplete triangle must not fetch a target"
            ))
            .unwrap()
            .is_empty()
    );
    let mut restored: Engine =
        serde_json::from_value(serde_json::to_value(&engine).unwrap()).unwrap();
    let mut last = vec![];
    upload(&mut last, 0x433, &packed(positions[2]), true);
    upload(&mut last, 0x433, &packed([1., 0., 0., 1.]), true);
    let expected = engine.execute_cpu(&last, &mut |_, n| Ok(vec![0; n])).unwrap();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut gpu_restored = restored.clone();
        let actual = match gpu_restored.execute_host_gpu(&last, &mut |_, n| Ok(vec![0; n])) {
            Ok(actual) => actual,
            Err(error) if error.starts_with("host GPU adapter:") => {
                eprintln!(
                    "skipping checkpointed DMP render parity because no host GPU adapter is available: {error}"
                );
                return;
            }
            Err(error) => panic!("checkpointed DMP host render path failed: {error}"),
        };
        assert_eq!(actual, expected);
    }
    assert_eq!(
        restored.execute_cpu(&last, &mut |_, n| Ok(vec![0; n])).unwrap(),
        expected
    );
}

#[test]
fn flat_shading_uses_the_provoking_vertex_color() {
    let mut words = setup();
    pair(&mut words, 0x258, 1);
    for (position, color) in [
        ([-0.75, -0.75, 0., 1.], [1., 0., 0., 1.]),
        ([0.75, -0.75, 0., 1.], [0., 1., 0., 1.]),
        ([0., 0.75, 0., 1.], [0., 0., 1., 1.]),
    ] {
        upload(&mut words, 0x433, &packed(position), true);
        upload(&mut words, 0x433, &packed(color), true);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if assert_host_gpu_matches_cpu(&words, |_, length| vec![0; length]).is_none() {
        return;
    }
    let writes = Engine::default()
        .execute_cpu(&words, &mut |_, n| Ok(vec![0; n]))
        .unwrap();
    assert_eq!(
        writes[0]
            .1
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| **p == [255, 255, 0, 0])
            .count(),
        288
    );
}

#[test]
fn completion_ranges_preserve_concurrent_pixels_and_masked_channels() {
    for mask in [15, 1] {
        let mut words = setup();
        pair(&mut words, 0x307, mask << 8);
        for position in [
            [-0.75, -0.75, 0., 1.],
            [0.75, -0.75, 0., 1.],
            [0., 0.75, 0., 1.],
        ] {
            upload(&mut words, 0x433, &packed(position), true);
            upload(&mut words, 0x433, &packed([0.; 4]), true);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if assert_host_gpu_matches_cpu(&words, |_, length| vec![0; length]).is_none() {
            return;
        }
        let mut engine = Engine::default();
        let snapshots = engine.execute_cpu(&words, &mut |_, n| Ok(vec![0; n])).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0].1.iter().all(|&byte| byte == 0));
        let ranges = engine.written_ranges();
        assert_eq!(
            ranges.iter().map(|(_, n)| n).sum::<usize>(),
            288 * if mask == 15 { 4 } else { 1 }
        );
        // Another producer changes memory after submission and before the
        // completion commits. Equal-valued GPU stores still take effect;
        // untouched pixels and disabled channels retain the later write.
        let mut memory = vec![0x5a; 32 * 32 * 4];
        let mut written = vec![false; memory.len()];
        for &(address, length) in ranges {
            let offset = (address - 0x10000) as usize;
            memory[offset..offset + length]
                .copy_from_slice(&snapshots[0].1[offset..offset + length]);
            written[offset..offset + length].fill(true);
            if mask == 1 {
                assert_eq!((length, offset % 4), (1, 3));
            }
        }
        assert!(
            memory
                .iter()
                .zip(&written)
                .all(|(&byte, &written)| byte == if written { 0 } else { 0x5a })
        );
        assert_eq!(&memory[memory.len() - 4..], &[0x5a; 4]);
    }
}
