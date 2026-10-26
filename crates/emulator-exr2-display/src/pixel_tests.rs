//! Pixel tests cover byte layout, alpha composition, window stacking and
//! unsupported source formats.
use super::{
    device::{Observation, Options, Scanout},
    scanout,
};
use fujimod_emulator_devices::{Device, test_host::{TestHost, write}};

fn model() -> Scanout {
    Scanout::new(Options {
        packing: "yycbcr".into(),
        matrix: "bt601-full".into(),
        graphics_profile: Some(scanout::ABGR8888_STRAIGHT.into()),
        backdrop_profile: Some(scanout::BLACK.into()),
        rectangle_profile: None,
        ram_bytes: 0x100000,
        max_pixels: 1000,
        reset_ticks: 100,
    })
    .unwrap()
}
fn window(
    d: &mut Scanout,
    h: &mut TestHost,
    pipe: usize,
    (group, channel): (u64, u64),
    source: u32,
    x: u32,
    alpha: u32,
) {
    for (o, v) in [
        (0x34 + group * 0x50, 1),
        (0x40 + group * 0x50, 1 << (channel * 4)),
        (0x44 + group * 0x50 + channel * 4, x << 16),
        (0x54 + group * 0x50 + channel * 4, 0x10001),
        (0x64 + group * 0x50 + channel * 4, source),
        (0x74 + group * 0x50 + channel * 4, 0),
        (0xe0 + group * 4, alpha << (channel * 8)),
    ] {
        write(d, h, scanout::BASES[pipe] + o, v);
    }
}
fn observe(d: &Scanout, h: &TestHost, pipe: usize, pixels: bool) -> fujimod_emulator_devices::exr2::scanout_frame::Frame {
    let mut read = |a: u64, n: usize| {
        Ok(h.ram[(a & 0x3fffffff) as usize..(a & 0x3fffffff) as usize + n].to_vec())
    };
    d.frame(
        pipe,
        pixels,
        &mut Observation {
            read: &mut read,
            diagnostic_hashes: true,
        },
    )
    .unwrap()
    .unwrap()
}
#[test]
fn group1_colors_and_straight_alpha_on_both_pipes() {
    for pipe in 0..2 {
        let mut d = model();
        let mut h = TestHost::new();
        for (o, v) in [(0, 11 << 24), (0x10, 0x10001)] {
            write(&mut d, &mut h, scanout::BASES[pipe] + o, v);
        }
        h.ram[0x2000..0x2010].copy_from_slice(&[
            255, 30, 60, 210, 0, 20, 40, 60, 255, 100, 150, 200, 64, 40, 80, 120,
        ]);
        window(&mut d, &mut h, pipe, (1, 0), 0x2000, 0, 128);
        let before = d.fingerprint().unwrap();
        let ram = h.ram.clone();
        let frame = observe(&d, &h, pipe, true);
        assert_eq!(
            frame.rgb,
            Some(vec![210, 60, 30, 0, 0, 0, 200, 150, 100, 30, 20, 10])
        );
        let metadata = observe(&d, &h, pipe, false);
        assert_eq!(frame.raw, metadata.raw);
        assert_eq!(metadata.rgb, None);
        assert_eq!(observe(&d, &h, pipe, true).rgb, frame.rgb);
        assert_eq!(d.fingerprint().unwrap(), before);
        assert_eq!(h.ram, ram);
        assert!(h.effects.is_empty());
    }
}
#[test]
fn group1_composes_over_group0_with_touch_overlap_containment_and_window_alpha() {
    for pipe in 0..2 {
        for channel in 0..4 {
            for x in 0..=2 {
                for alpha in [128, 64, 13] {
                    let mut d = model();
                    let mut h = TestHost::new();
                    for (o, v) in [(0, 11 << 24 | 3 << 20), (0x10, 0x30001)] {
                        write(&mut d, &mut h, scanout::BASES[pipe] + o, v);
                    }
                    h.ram[0x1000..0x1010].copy_from_slice(&[20, 80, 160, 255].repeat(4));
                    h.ram[0x2000..0x2010].copy_from_slice(&[255, 40, 60, 200].repeat(4));
                    window(&mut d, &mut h, pipe, (0, 2), 0x1000, 0, 128);
                    window(&mut d, &mut h, pipe, (1, channel), 0x2000, x, alpha);
                    let mut expected = vec![0u8; 24];
                    for y in 0..2 {
                        for col in 0..4 {
                            let mut rgb = if col < 2 { [160u32, 80, 20] } else { [0, 0, 0] };
                            if (x..x + 2).contains(&col) {
                                for c in 0..3 {
                                    rgb[c] =
                                        ([200, 60, 40][c] * alpha + rgb[c] * (128 - alpha) + 64)
                                            / 128;
                                }
                            }
                            for c in 0..3 {
                                expected[(y * 4 + col as usize) * 3 + c] = rgb[c] as u8;
                            }
                        }
                    }
                    assert_eq!(
                        observe(&d, &h, pipe, true).rgb,
                        Some(expected),
                        "pipe {pipe}, channel {channel}, x {x}, alpha {alpha}"
                    );
                }
            }
        }
    }
}
