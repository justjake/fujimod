use super::*;
use fujimod_emulator_devices::{
    Host,
    test_host::{TestHost, advance, write},
};
use fujimod_emulator_imaging::analysis::ImageLayout;

fn fixture() -> (Processing, TestHost) {
    let routes = fujimod_emulator_devices::exr2::irq_routes::IrqRoutes::default();
    routes.set(88, Some(0)).unwrap();
    let device = Processing::new(
        companion::Options::default(),
        Options {
            irq: 88,
            pixels_per_tick: 4,
        },
        Wiring {
            frames: ImageFrames::new(ImageLayout {
                width: 8,
                height: 4,
                stride: 16,
            })
            .unwrap(),
            routes,
        },
    )
    .unwrap();
    let mut h = TestHost::new();
    let descriptor = [0x03000207u32, 0x5000, 80, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    h.ram_write(0x4000, &descriptor).unwrap();
    let mut packet = [0u8; 80];
    for (offset, value) in [
        (0, 0x6000u32),
        (4, 0x10000 | 36),
        (8, 0x8000),
        (12, 8),
        (16, 1 << 16 | 2),
        (20, 6 << 16 | 24),
        (48, 0x9000),
        (52, 0xa000),
        (60, 8),
        (64, 0xb000),
        (68, 0xc000),
        (72, 0x80808080),
    ] {
        packet[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    h.ram_write(0x5000, &packet).unwrap();
    let pixels = (0..9)
        .flat_map(|y| {
            (0..18).flat_map(move |pair| {
                let a = (pair * 2 * 128 + y * 16) as u16;
                let b = ((pair * 2 + 1) * 128 + y * 16) as u16;
                [a as u8, ((a >> 8) | ((b & 15) << 4)) as u8, (b >> 4) as u8]
            })
        })
        .collect::<Vec<_>>();
    h.ram_write(0x6000, &pixels).unwrap();
    h.ram_write(0xb000, &[255; 256]).unwrap();
    h.ram_write(
        0xc000,
        &(0..1024).map(|i| (i / 4) as u8).collect::<Vec<_>>(),
    )
    .unwrap();
    h.ram[0x7fff..0x8041].fill(0xa5);
    h.effects.clear();
    (device, h)
}
fn start(d: &mut Processing, h: &mut TestHost) {
    write(d, h, 0xfe80c108, 0x4000);
    write(d, h, 0xfe80c440, 0x40001);
    write(d, h, 0xfe80c100, 1);
}
#[test]
fn raw_dma_crop_tone_phases_statistics_and_checkpoint_complete_before_irq() {
    let (mut d, mut h) = fixture();
    start(&mut d, &mut h);
    assert_eq!(d.next_deadline(0), Some(37));
    let saved = d.checkpoint().unwrap();
    let before = d.fingerprint().unwrap();
    assert!(before["job"]["outputs"][0]["bytes"].is_object()); // Large result buffers are hashes, not JSON arrays.
    h.ram[0x6000..0x6200].fill(0);
    h.ram[0xc000..0xc400].fill(255);
    advance(&mut d, &mut h, 36);
    assert!(h.effects.is_empty());
    assert_eq!(d.fingerprint().unwrap(), before);
    advance(&mut d, &mut h, 37);
    assert_eq!(h.effects, ["dma", "dma", "dma", "irq"]);
    assert_eq!(h.irqs, [(0, 88)]);
    // Independent fixture samples: x=2,5,...,23; sensor rows=1,2,4,5.
    let expected = [1u8, 2, 4, 5]
        .into_iter()
        .flat_map(|y| {
            (0..4).flat_map(move |pair| [(2 + 6 * pair) * 8 + y, (5 + 6 * pair) * 8 + y, 128, 128])
        })
        .collect::<Vec<_>>();
    assert_eq!(&h.ram[0x8000..0x8040], expected);
    assert_eq!(h.ram[0x7fff], 0xa5);
    assert_eq!(h.ram[0x8040], 0xa5);
    assert!(
        h.ram[0x9000..0x9008]
            .chunks_exact(2)
            .all(|b| u16::from_le_bytes([b[0], b[1]]) == 128)
    );
    assert_eq!(
        u16::from_le_bytes(h.ram[0xa000..0xa002].try_into().unwrap()),
        128
    );
    let after = d.fingerprint().unwrap();
    let mut replay = TestHost::new();
    d.restore_checkpoint(&saved).unwrap();
    assert_eq!(d.fingerprint().unwrap(), before);
    advance(&mut d, &mut replay, 37);
    for (a, n) in [(0x8000, 64), (0x9000, 8), (0xa000, 2)] {
        assert_eq!(&replay.ram[a..a + n], &h.ram[a..a + n]);
    }
    assert_eq!(d.fingerprint().unwrap(), after);
}
#[test]
fn raw_mask_and_tone_are_addressed_inputs_and_invalid_requests_have_no_effects() {
    let (mut d, mut h) = fixture();
    // Defective first sensor cell: interpolate from the neighboring same phase.
    h.ram_write(0xb000, &(1u32 << 16).to_le_bytes()).unwrap();
    start(&mut d, &mut h);
    advance(&mut d, &mut h, 37);
    assert_eq!(h.ram[0x8000], 41);
    let (mut d, mut h) = fixture();
    h.ram[0xc000..0xc400].fill(73);
    start(&mut d, &mut h);
    advance(&mut d, &mut h, 37);
    assert!(
        h.ram[0x8000..0x8040]
            .chunks_exact(4)
            .all(|b| b == [73, 73, 128, 128])
    );
    for (address, value) in [
        (0x5004, 2 << 16 | 36),
        (0x5008, 0x9000),
        (0xb000, 0x1000ffff),
    ] {
        let (mut d, mut h) = fixture();
        h.ram_write(address, &(value as u32).to_le_bytes()).unwrap();
        write(&mut d, &mut h, 0xfe80c108, 0x4000);
        write(&mut d, &mut h, 0xfe80c440, 0x40001);
        let before = d.fingerprint().unwrap();
        h.effects.clear();
        let mut cx = h.cx();
        d.write(&mut cx, 0xfe80c100, 4, 1);
        assert!(cx.refusal().is_some());
        assert_eq!(d.fingerprint().unwrap(), before);
        assert!(h.effects.is_empty());
        assert!(h.ram[0x8000..0x8040].iter().all(|&v| v == 0xa5));
    }
}
#[test]
fn raw_stop_cancels_all_outputs_and_route_change_refuses_before_dma() {
    let (mut d, mut h) = fixture();
    start(&mut d, &mut h);
    write(&mut d, &mut h, 0xfe80c100, 0);
    advance(&mut d, &mut h, 37);
    assert!(h.effects.is_empty());
    let (mut d, mut h) = fixture();
    start(&mut d, &mut h);
    d.wiring.routes.set(88, Some(1)).unwrap();
    h.now = 37;
    let mut cx = h.cx();
    d.advance(&mut cx);
    assert!(cx.refusal().is_some());
    assert!(h.effects.is_empty());
}

#[test]
fn raw_phase_statistics_change_with_the_captured_region() {
    let (mut d, mut h) = fixture();
    h.ram_write(0x5014, &(24u32 << 16 | 24).to_le_bytes())
        .unwrap();
    let raw = (0..25)
        .flat_map(|y| {
            let value = if y < 13 { 1024u16 } else { 2048u16 };
            (0..18).flat_map(move |_| {
                [
                    value as u8,
                    ((value >> 8) | ((value & 15) << 4)) as u8,
                    (value >> 4) as u8,
                ]
            })
        })
        .collect::<Vec<_>>();
    h.ram_write(0x6000, &raw).unwrap();
    start(&mut d, &mut h);
    advance(&mut d, &mut h, 145);
    assert_eq!(&h.ram[0xa000..0xa004], &[128, 0, 171, 16]);
    assert_eq!(&h.ram[0x9000..0x9008], &[128, 0, 171, 0, 128, 0, 128, 0]);
}
