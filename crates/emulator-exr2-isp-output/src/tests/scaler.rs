use crate::*;
use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::test_host::*;
#[test]
fn scaler_dma_padding_reset_and_unsupported_transform() {
    let mut h = TestHost::new();
    h.ram[0x8000..0x8010].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    h.ram[0x9000..0x9020].fill(0xa5);
    let mut d = Scaler::new(
        fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
            completion_ticks: 4,
        },
        Irq { core: 0, line: 67 },
    )
    .unwrap();
    for (o, v) in [
        (0x10, 0x8000),
        (0x14, 8),
        (0x18, 1 << 16 | 8),
        (0x20, 0x9000),
        (0x24, 16),
        (0x28, 1 << 16 | 8),
        (0x40, 65536),
        (0x54, 65536),
        (0x7c, 0x1000),
        (4, 1),
    ] {
        write(&mut d, &mut h, 0xfff47000 + o, v);
    }
    advance(&mut d, &mut h, 3);
    assert!(h.irqs.is_empty());
    advance(&mut d, &mut h, 4);
    assert_eq!(&h.ram[0x9000..0x9010], &h.ram[0x8000..0x8010]);
    assert_eq!(h.ram[0x9010], 0xa5);
    assert_eq!(read(&mut d, &mut h, 0xfff47074), 1);
    write(&mut d, &mut h, 0xfff47078, 1);
    assert_eq!(read(&mut d, &mut h, 0xfff47074), 0);
    write(&mut d, &mut h, 0xfff47048, 1);
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff47004, 4, 1);
    assert!(cx.refusal().is_some());
}

#[test]
fn unprogrammed_transform_copies_equal_geometry_and_refuses_scaling() {
    let run = |dst_width: u32| {
        let mut h = TestHost::new();
        for (i, v) in h.ram[0x8000..0x8020].iter_mut().enumerate() {
            *v = 16 + i as u8;
        }
        h.ram[0x9000..0x9020].fill(0xa5);
        let mut d = Scaler::new(
            fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
                completion_ticks: 4,
            },
            Irq { core: 0, line: 67 },
        )
        .unwrap();
        for (o, v) in [
            (0, 0),
            (8, 1),
            (0x10, 0x8000),
            (0x14, 8),
            (0x18, 2 << 16 | 8),
            (0x20, 0x9000),
            (0x24, dst_width),
            (0x28, 2 << 16 | dst_width),
            (0x2c, 0),
            (0x70, 0),
            (0x7c, 0x1301),
            (0x80, 0xff),
        ] {
            write(&mut d, &mut h, 0xfff47000 + o, v);
        }
        let mut cx = h.cx();
        d.write(&mut cx, 0xfff47004, 4, 3);
        let refused = cx.refusal().is_some();
        drop(cx);
        if !refused {
            advance(&mut d, &mut h, 4);
        }
        (refused, h)
    };
    let (refused, h) = run(8);
    assert!(!refused);
    assert_eq!(&h.ram[0x9000..0x9020], &h.ram[0x8000..0x8020]);
    let (refused, h) = run(16);
    assert!(refused);
    assert_eq!(&h.ram[0x9000..0x9020], &[0xa5; 32]);
}

#[test]
fn scaler_refuses_tags_outside_the_measured_identity_contract() {
    for (step, tag_offset) in [(98304, 0), (65536, 16)] {
        let mut host = TestHost::new();
        host.ram[0x8000..0x8020].fill(128);
        host.ram[0x8000 + tag_offset..0x8004 + tag_offset].copy_from_slice(&[32, 33, 0, 208]);
        host.ram[0x9000..0x9020].fill(0xa5);
        let mut device = Scaler::new(
            fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
                completion_ticks: 4,
            },
            Irq { core: 0, line: 67 },
        )
        .unwrap();
        for (offset, value) in [
            (0x10, 0x8000),
            (0x14, 8),
            (0x18, 2 << 16 | 8),
            (0x20, 0x9000),
            (0x24, 8),
            (0x28, 2 << 16 | 8),
            (0x40, step),
            (0x54, 65536),
            (0x7c, 0x1000),
        ] {
            write(&mut device, &mut host, 0xfff47000 + offset, value);
        }
        let mut cx = host.cx();
        device.write(&mut cx, 0xfff47004, 4, 1);
        assert!(cx.refusal().is_some());
        assert!(device.next_deadline(0).is_none());
        assert_eq!(&host.ram[0x9000..0x9020], &[0xa5; 32]);
        assert!(host.irqs.is_empty());
    }
}

#[test]
fn centered_phases_place_a_rectangle_without_touching_canvas_padding() {
    let mut h = TestHost::new();
    for y in 0..8 {
        for x in (0..16).step_by(2) {
            let at = 0x8000 + (y * 16 + x) * 2;
            h.ram[at..at + 4].copy_from_slice(&[
                (y * 20 + x * 2) as u8,
                (y * 20 + (x + 1) * 2) as u8,
                80,
                160,
            ]);
        }
    }
    h.ram[0x9000..0x9100].fill(0xa5);
    let mut d = Scaler::new(
        fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
            completion_ticks: 4,
        },
        Irq { core: 0, line: 67 },
    )
    .unwrap();
    for (offset, value) in [
        (0x10, 0x8000),
        (0x14, 16),
        (0x18, 8 << 16 | 16),
        (0x20, 0x9000),
        (0x24, 16),
        (0x28, 4 << 16 | 8),
        (0x2c, 2 << 16 | 4),
        (0x40, 65536),
        (0x4c, 32768),
        (0x54, 65536),
        (0x60, 4 << 16 | 8),
        (0x64, 4 << 16 | 8),
        (0x7c, 0x1000),
        (4, 1),
    ] {
        write(&mut d, &mut h, 0xfff47000 + offset, value);
    }
    assert_eq!(d.next_deadline(0), Some(4));
    let saved = d.checkpoint().unwrap();
    advance(&mut d, &mut h, 3);
    assert!(h.irqs.is_empty());
    advance(&mut d, &mut h, 4);
    assert_eq!(h.irqs, [(0, 67)]);
    let output = h.ram[0x9000..0x9100].to_vec();
    for y in 0..8 {
        for x in (0..16).step_by(2) {
            let at = 0x9000 + (y * 16 + x) * 2;
            let expected = if (2..6).contains(&y) && (4..12).contains(&x) {
                [
                    (y * 20 + x * 2 + 1) as u8,
                    (y * 20 + (x + 1) * 2 + 1) as u8,
                    80,
                    160,
                ]
            } else {
                [0xa5; 4]
            };
            assert_eq!(&h.ram[at..at + 4], expected);
        }
    }
    d.restore_checkpoint(&saved).unwrap();
    h.ram[0x9000..0x9100].fill(0xa5);
    h.ram[0x8000..0x8100].fill(0);
    h.irqs.clear();
    advance(&mut d, &mut h, 4);
    assert_eq!(&h.ram[0x9000..0x9100], output);
    h.ram[0x8000..0x8100].fill(128);
    write(&mut d, &mut h, 0xfff4704c, 0x20000000);
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff47004, 4, 1);
    assert!(cx.refusal().unwrap().contains("misses source"));
    assert!(d.next_deadline(4).is_none());
}

#[test]
fn centered_scale_replicates_edges_in_aligned_destination_tails() {
    let mut host = TestHost::new();
    for y in 0..30 {
        for x in (0..40).step_by(2) {
            let at = 0x8000 + (y * 40 + x) * 2;
            host.ram[at..at + 4].copy_from_slice(&[
                (x + 2 * y) as u8,
                (x + 1 + 2 * y) as u8,
                (64 + y) as u8,
                (96 + x / 2) as u8,
            ]);
        }
    }
    host.ram[0x9000..0x9400].fill(0xa5);
    let mut scaler = Scaler::new(
        fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
            completion_ticks: 4,
        },
        Irq { core: 0, line: 67 },
    )
    .unwrap();
    for (offset, value) in [
        (0x10, 0x8000),
        (0x14, 40),
        (0x18, 30 << 16 | 40),
        (0x20, 0x9000),
        (0x24, 32),
        (0x28, 16 << 16 | 24),
        (0x40, 0x1ffff),
        (0x54, 0x1c924),
        (0x60, 15 << 16 | 20),
        (0x64, 7 << 16 | 10),
        (0x7c, 0x1000),
        (4, 1),
    ] {
        write(&mut scaler, &mut host, 0xfff47000 + offset, value);
    }
    assert_eq!(scaler.next_deadline(0), Some(4));
    advance(&mut scaler, &mut host, 4);
    // The ramp independently fixes the four corner colors. Both luma lanes
    // and the shared chroma pair replicate the same source edge.
    for (row, pair, expected) in [
        (0, 0, [5, 7, 67, 96]),
        (0, 11, [44, 44, 67, 115]),
        (15, 0, [58, 60, 93, 96]),
        (15, 11, [97, 97, 93, 115]),
    ] {
        let at = 0x9000 + row * 64 + pair * 4;
        assert_eq!(&host.ram[at..at + 4], expected);
    }
    for row in 0..16 {
        assert_eq!(
            &host.ram[0x9000 + row * 64 + 48..0x9000 + (row + 1) * 64],
            &[0xa5; 16]
        );
    }
    assert_eq!(host.irqs, [(0, 67)]);
}

#[test]
fn zero_center_translation_uses_phase_at_each_transfer_burst() {
    for burst in [0, 1, 2] {
        let mut h = TestHost::new();
        for y in 0..2 {
            for pair in 0..4 {
                let at = 0x8000 + y * 16 + pair * 4;
                h.ram[at..at + 4].copy_from_slice(&[
                    (pair * 2 + 1) as u8,
                    (pair * 2 + 2) as u8,
                    80,
                    160,
                ]);
            }
        }
        h.ram[0x9000..0x9040].fill(0xa5);
        let mut d = Scaler::new(
            fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
                completion_ticks: 4,
            },
            Irq { core: 0, line: 67 },
        )
        .unwrap();
        for (offset, value) in [
            (8, burst),
            (0x10, 0x8000),
            (0x14, 8),
            (0x18, 2 << 16 | 8),
            (0x20, 0x9000),
            (0x24, 16),
            (0x28, 2 << 16 | 8),
            (0x40, 65536),
            (0x4c, 2 * 65536),
            (0x54, 65536),
            (0x7c, 0x1000),
            (4, 1),
        ] {
            write(&mut d, &mut h, 0xfff47000 + offset, value);
        }
        advance(&mut d, &mut h, 4);
        for y in 0..2 {
            assert_eq!(
                &h.ram[0x9000 + y * 32..0x9010 + y * 32],
                &[3, 4, 80, 160, 5, 6, 80, 160, 7, 8, 80, 160, 8, 8, 80, 160]
            );
            assert_eq!(&h.ram[0x9010 + y * 32..0x9020 + y * 32], &[0xa5; 16]);
        }
        assert_eq!(h.irqs, [(0, 67)]);
    }
}

#[test]
fn factorized_shears_compose_in_vertical_then_horizontal_order() {
    for (vertical, horizontal, x, y, expected) in [
        (32768, 0, 2, 1, 40),
        (-32768i32, 0, 4, 3, 32),
        (32768, 32768, 2, 2, 62),
    ] {
        let mut h = TestHost::new();
        for y in 0..8 {
            for x in (0..8).step_by(2) {
                let at = 0x8000 + (y * 8 + x) * 2;
                h.ram[at..at + 4].copy_from_slice(&[
                    (x * 4 + y * 16) as u8,
                    ((x + 1) * 4 + y * 16) as u8,
                    80,
                    160,
                ]);
            }
        }
        let mut d = Scaler::new(
            fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
                completion_ticks: 4,
            },
            Irq { core: 0, line: 67 },
        )
        .unwrap();
        for (offset, value) in [
            (0x10, 0x8000),
            (0x14, 8),
            (0x18, 8 << 16 | 8),
            (0x20, 0x9000),
            (0x24, 8),
            (0x28, 8 << 16 | 8),
            (0x40, 65536),
            (0x44, horizontal),
            (0x50, vertical as u32 & 0x3ffff),
            (0x54, 65536),
            (0x7c, 0x1000),
            (4, 1),
        ] {
            write(&mut d, &mut h, 0xfff47000 + offset, value);
        }
        advance(&mut d, &mut h, 4);
        assert_eq!(h.ram[0x9000 + (y * 8 + x) / 2 * 4 + x % 2], expected);
        assert_eq!(h.irqs, [(0, 67)]);
    }
}

#[test]
fn identity_zero_chroma_matches_the_measured_control() {
    let mut host = TestHost::new();
    host.ram[0x10000..0x10000 + 528 * 32].fill(0xa5);
    let (width, height, pitch) = (256, 32, 528);
    for y in 0..height {
        for x in 0..width / 2 {
            let at = 0x8000 + y * pitch + x * 4;
            host.ram[at..at + 4].copy_from_slice(&[
                ((2 * x + 3 * y) % 256) as u8,
                ((2 * x + 1 + 3 * y) % 256) as u8,
                ((80 + 11 * x + 3 * y) % 256) as u8,
                ((96 + 7 * x + 9 * y) % 256) as u8,
            ]);
        }
        host.ram[0x8000 + y * pitch + width * 2..0x8000 + (y + 1) * pitch].fill(0);
    }
    let source = host.ram[0x8000..0x8000 + pitch * height].to_vec();
    let mut device = Scaler::new(
        fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
            completion_ticks: 4,
        },
        Irq { core: 0, line: 67 },
    )
    .unwrap();
    for (offset, value) in [
        (0x10, 0x8000),
        (0x14, (pitch / 2) as u32),
        (0x18, ((height << 16) | width) as u32),
        (0x20, 0x10000),
        (0x24, (pitch / 2) as u32),
        (0x28, ((height << 16) | width) as u32),
        (0x40, 65536),
        (0x54, 65536),
        (0x60, 16 << 16 | 128),
        (0x64, 16 << 16 | 128),
        (0x7c, 0x1000),
        (4, 1),
    ] {
        write(&mut device, &mut host, 0xfff47000 + offset, value);
    }
    advance(&mut device, &mut host, 3);
    assert!(host.irqs.is_empty());
    assert!(
        host.ram[0x10000..0x10000 + pitch * height]
            .iter()
            .all(|&v| v == 0xa5)
    );
    advance(&mut device, &mut host, 4);
    let pixels: Vec<u8> = (0..height)
        .flat_map(|y| {
            host.ram[0x10000 + y * pitch..0x10000 + y * pitch + width * 2]
                .iter()
                .copied()
        })
        .collect();
    // Full 256x32 hardware identity control, including 62 invalidated pairs.
    assert_eq!(
        format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&pixels)),
        "83dc56dc906da7311c4508df4016d24af4cd3e5e2034dee445ccfd4ec192e26c"
    );
    assert_eq!(
        &host.ram[0x8000..0x8000 + pitch * height],
        source.as_slice()
    );
    for y in 0..height {
        assert!(
            host.ram[0x10000 + y * pitch + width * 2..0x10000 + (y + 1) * pitch]
                .iter()
                .all(|&v| v == 0xa5)
        );
    }
    assert_eq!(read(&mut device, &mut host, 0xfff47074), 1);
    assert_eq!(host.irqs.len(), 1);
}

#[test]
fn centered_scaler_ignores_tags_outside_its_sample_support() {
    for (input_x, following_row_tag) in [(2, false), (10, false), (2, true)] {
        let mut host = TestHost::new();
        for y in 0..12 {
            for pair in 0..8 {
                let at = 0x8000 + y * 32 + pair * 4;
                host.ram[at..at + 4].copy_from_slice(if pair < 6 {
                    &[80, 80, 96, 160]
                } else {
                    &[0, 0, 0, 0]
                });
            }
        }
        if following_row_tag {
            host.ram[0x8000 + 11 * 32 + 2] = 0;
        }
        host.ram[0x10000..0x18000].fill(0xa5);
        let source = host.ram[0x8000..0x8180].to_vec();
        let mut device = Scaler::new(
            fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
                completion_ticks: 4,
            },
            Irq { core: 0, line: 67 },
        )
        .unwrap();
        for (offset, value) in [
            (0x10, 0x8000),
            (0x14, 16),
            (0x18, 12 << 16 | 16),
            (0x20, 0x10000),
            (0x24, 128),
            (0x28, 128 << 16 | 128),
            (0x40, 4096),
            (0x54, 4096),
            (0x60, 2 << 16 | input_x),
            (0x64, 8 << 16 | 8),
            (0x7c, 0x1000),
        ] {
            write(&mut device, &mut host, 0xfff47000 + offset, value);
        }
        let refused = {
            let mut cx = host.cx();
            device.write(&mut cx, 0xfff47004, 4, 1);
            cx.refusal().is_some()
        };
        assert_eq!(refused, input_x == 10 || following_row_tag);
        advance(&mut device, &mut host, 4);
        if !refused {
            assert!(
                host.ram[0x10000..0x18000]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pair| *pair == [80, 80, 96, 160])
            );
            assert_eq!(host.irqs, [(0, 67)]);
        } else {
            assert!(host.ram[0x10000..0x18000].iter().all(|&byte| byte == 0xa5));
            assert!(host.irqs.is_empty());
        }
        assert_eq!(&host.ram[0x8000..0x8180], source.as_slice());
    }
}
