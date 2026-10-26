use super::*;
use crate::device::Observation;

fn model(width: u32, height: u32, pipe: usize) -> Compositor {
    let mut model = Compositor {
        pipes: Default::default(),
        packing: "yycbcr".into(),
        matrix: "bt601-full".into(),
        graphics_profile: Some(GraphicsProfile::Abgr8888Straight),
        backdrop_profile: Some(BackdropProfile::Black),
        rectangle_composition: None,
        ram_bytes: 0x10000,
        max_pixels: 1000,
    };
    put(&mut model, pipe, 0x10, (width - 1) << 16 | (height - 1));
    model
}

fn put(model: &mut Compositor, pipe: usize, offset: u64, value: u32) {
    let mut actions = vec![];
    model
        .write(0, BASES[pipe] + offset, 4, value, &mut actions)
        .unwrap();
    assert!(actions.is_empty());
}

fn window(
    model: &mut Compositor,
    pipe: usize,
    layer: u32,
    channel: u32,
    source: u32,
    dimensions: [u32; 2],
    origin: u32,
    alpha: u32,
) {
    let base = u64::from(layer * 0x50);
    let delta = u64::from(channel * 4);
    let enabled = model.read(BASES[pipe] + base + 0x40, 4).unwrap();
    let old_alpha = model
        .read(BASES[pipe] + 0xe0 + u64::from(layer * 4), 4)
        .unwrap();
    for (offset, value) in [
        (base + 0x34, 1),
        (base + 0x40, enabled | 1 << (channel * 4)),
        (base + delta + 0x44, origin),
        (
            base + delta + 0x54,
            (dimensions[0] - 1) << 16 | (dimensions[1] - 1),
        ),
        (base + delta + 0x64, source),
        (base + delta + 0x74, 0),
        (
            0xe0 + u64::from(layer * 4),
            (old_alpha & !(255 << (channel * 8))) | alpha << (channel * 8),
        ),
    ] {
        put(model, pipe, offset, value);
    }
}

fn render(model: &Compositor, pipe: usize, memory: &[u8]) -> Frame {
    let before = model.clone();
    let mut read = |address: u64, count: usize| {
        let begin = (address & 0x3fffffff) as usize;
        memory
            .get(begin..begin + count)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| "test scanout read escaped RAM".into())
    };
    let mut observation = Observation {
        read: &mut read,
        diagnostic_hashes: true,
    };
    let frame = model
        .frame(pipe, 0, true, &mut observation)
        .unwrap()
        .unwrap();
    assert_eq!(model, &before);
    frame
}

#[test]
fn rectangle_registers_survive_a_disabled_display_without_publishing_a_frame() {
    let mut model = model(2, 2, 1);
    model.rectangle_composition = Some(RectangleComposition::DeclaredLuma);
    for (offset, value) in [
        (0x108, 1),
        (0x11c, 0),
        (0x120, 0x10001),
        (0x174, 0),
        (0x188, 0),
        (0x190, 0),
        (0x18c, 128),
        (0x178, 220),
        (0x16c, 1),
    ] {
        put(&mut model, 1, offset, value);
    }
    let mut read = |_, _| Err("disabled display must not read pixels".into());
    let mut observation = Observation {
        read: &mut read,
        diagnostic_hashes: true,
    };
    assert!(model.frame(1, 0, true, &mut observation).unwrap().is_none());
    window(&mut model, 1, 0, 0, 0x2000, [2, 2], 0, 128);
    let memory = vec![0; 0x10000];
    assert_eq!(render(&model, 1, &memory).rgb.unwrap(), [220; 12]);
    put(&mut model, 1, 0x40, 0);
    assert!(model.frame(1, 0, true, &mut observation).unwrap().is_none());
    put(&mut model, 1, 0x40, 1);
    assert_eq!(render(&model, 1, &memory).rgb.unwrap(), [220; 12]);
}

#[test]
fn group1_abgr_pixels_use_byte_zero_alpha_on_both_outputs() {
    let raw = [
        255, 30, 60, 210, 0, 20, 40, 60, 255, 100, 150, 200, 64, 40, 80, 120,
    ];
    for pipe in 0..2 {
        let mut model = model(2, 2, pipe);
        put(&mut model, pipe, 0, 11 << 24);
        window(&mut model, pipe, 1, 0, 0x2000, [2, 2], 0, 128);
        let mut memory = vec![0; 0x10000];
        memory[0x2000..0x2010].copy_from_slice(&raw);
        let frame = render(&model, pipe, &memory);
        assert_eq!(frame.raw, raw);
        assert_eq!(
            frame.rgb.unwrap(),
            [210, 60, 30, 0, 0, 0, 200, 150, 100, 30, 20, 10]
        );
        assert_eq!(&memory[0x2000..0x2010], raw);
    }
}

#[test]
fn group1_blends_over_group0_and_at_its_edge_on_both_outputs() {
    for pipe in 0..2 {
        for x in 0..3 {
            for alpha in [128, 64] {
                let mut model = model(4, 2, pipe);
                put(&mut model, pipe, 0, 11 << 24 | 3 << 20);
                window(&mut model, pipe, 0, 0, 0x2000, [2, 2], 0, 128);
                window(&mut model, pipe, 1, 3, 0x3000, [1, 1], x << 16, alpha);
                let mut memory = vec![0; 0x10000];
                memory[0x2000..0x2010].copy_from_slice(&[20, 80, 160, 255].repeat(4));
                memory[0x3000..0x3004].copy_from_slice(&[255, 40, 80, 200]);
                let frame = render(&model, pipe, &memory);
                let mut expected = vec![0; 4 * 2 * 3];
                for row in 0..2 {
                    expected[row * 12..row * 12 + 6].copy_from_slice(&[160, 80, 20, 160, 80, 20]);
                }
                let front = if alpha == 128 {
                    [200, 80, 40]
                } else if x < 2 {
                    [180, 80, 30]
                } else {
                    [100, 40, 20]
                };
                expected[x as usize * 3..x as usize * 3 + 3].copy_from_slice(&front);
                assert_eq!(frame.rgb.unwrap(), expected);
            }
        }
    }
}

#[test]
fn group1_lower_window_is_above_higher_window_and_scale_moves_its_origin() {
    for pipe in 0..2 {
        let mut model = model(4, 4, pipe);
        put(&mut model, pipe, 0, 11 << 24);
        put(&mut model, pipe, 8, 0x500);
        window(&mut model, pipe, 1, 3, 0x2000, [1, 1], 0x10001, 128);
        window(&mut model, pipe, 1, 0, 0x3000, [1, 1], 0x10001, 128);
        let mut memory = vec![0; 0x10000];
        memory[0x2000..0x2004].copy_from_slice(&[255, 30, 40, 220]);
        memory[0x3000..0x3004].copy_from_slice(&[255, 70, 100, 150]);
        let frame = render(&model, pipe, &memory);
        let mut expected = vec![0; 4 * 4 * 3];
        for y in 2..4 {
            for x in 2..4 {
                expected[(y * 4 + x) * 3..(y * 4 + x) * 3 + 3].copy_from_slice(&[150, 100, 70]);
            }
        }
        assert_eq!(frame.rgb.unwrap(), expected);
    }
}

#[test]
fn every_group0_source_format_keeps_its_byte_order_or_refuses() {
    let orders = [
        [0, 1, 2],
        [1, 0, 2],
        [0, 2, 1],
        [2, 1, 0],
        [2, 0, 1],
        [1, 2, 0],
    ];
    for format in 0..16 {
        let mut model = model(1, 1, 0);
        put(&mut model, 0, 0, format << 20);
        window(&mut model, 0, 0, 0, 0x2000, [1, 1], 0, 128);
        let mut memory = vec![0; 0x10000];
        memory[0x2000..0x2004].copy_from_slice(&[20, 80, 200, 255]);
        if (6..12).contains(&format) {
            let mut read = |_: u64, _: usize| panic!("unsupported format read pixel bytes");
            let mut observation = Observation {
                read: &mut read,
                diagnostic_hashes: true,
            };
            assert!(
                model
                    .frame(0, 0, true, &mut observation)
                    .err()
                    .unwrap()
                    .0
                    .contains("formats 6-11")
            );
        } else {
            let order = orders[if format < 6 { format as usize } else { 0 }];
            let expected = order.map(|index| [20, 80, 200][index]);
            assert_eq!(render(&model, 0, &memory).rgb.unwrap(), expected);
        }
    }
}
