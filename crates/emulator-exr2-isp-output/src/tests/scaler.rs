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
    write(&mut d, &mut h, 0xfff47044, 1);
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff47004, 4, 1);
    assert!(cx.refusal().is_some());
}

#[test]
fn scaler_refuses_zero_chroma_validity_tags_before_scheduling_dma() {
    let mut host = TestHost::new();
    host.ram[0x8000..0x8020].fill(128);
    host.ram[0x8000..0x8004].copy_from_slice(&[32, 33, 0, 208]);
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
        (0x40, 65536),
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
