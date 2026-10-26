use crate::*;
use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::test_host::*;
use fujimod_emulator_image_codecs::h264;
use openh264::{decoder::Decoder, formats::YUVSource};

const SOURCE: usize = 0x10000;
const FIRST: usize = 0x20000;
const SECOND: usize = 0x30000;
const SIZE: usize = 0x10000;
fn configured(h: &mut TestHost) -> AvcCodec {
    configured_backend(
        h,
        fujimod_catalog_construction::VideoEncoderBackend::Software,
    )
}
fn configured_backend(
    h: &mut TestHost,
    backend: fujimod_catalog_construction::VideoEncoderBackend,
) -> AvcCodec {
    let mut d = AvcCodec::new(avc::Options { backend }, Irq { core: 0, line: 75 }).unwrap();
    for (o, v) in [
        (0xe00, 64),
        (0xe04, 0x2000 | 32),
        (0xe08, 1000),
        (0xe0c, 30),
        (0xe10, 30),
        (0xe34, 3),
        (0xe38, 0x8000),
        (0xe44, 4),
        (0xe48, 0x400),
        (0xe94, 0x348),
        (0xea0, 0x100),
        (0x490, FIRST as u32),
        (0x494, SIZE as u32),
        (0x498, SECOND as u32),
        (0x49c, SIZE as u32),
        (0x4a4, 0x1c),
        (0x400, 0x40000000 | SOURCE as u32),
    ] {
        write(&mut d, h, 0xfff60000 + o, v);
    }
    d
}
fn input(h: &mut TestHost, bright: bool) {
    for row in 0..32 {
        for col in 0..32 {
            let p = SOURCE + (row * 32 + col) * 4;
            let y = if (col < 16) ^ bright { 40 } else { 200 };
            h.ram[p..p + 4].copy_from_slice(&[y, y, 96, 160]);
        }
    }
}
fn frame(d: &mut AvcCodec, h: &mut TestHost) -> Vec<u8> {
    let start = read(d, h, 0xfff604ac) as usize;
    let bank = (read(d, h, 0xfff604a4) >> 5) & 1;
    let before = h.ram.clone();
    let effects = h.effects.len();
    write(d, h, 0xfff60c00, 0x8000);
    let deadline = d.next_deadline(h.now).unwrap();
    advance(d, h, deadline - 1);
    assert_eq!(h.ram, before);
    assert_eq!(h.effects.len(), effects);
    if d.save()["backend"] == "software" {
        advance_from_checkpoint(d, h, deadline);
    } else {
        assert!(
            d.checkpoint()
                .err()
                .unwrap()
                .contains("cannot replay identically")
        );
        advance(d, h, deadline);
    }
    let n = ((read(d, h, 0xfff60d88) | (read(d, h, 0xfff60d8c) << 16)) / 8) as usize;
    let base = if bank == 0 { FIRST } else { SECOND };
    let first = n.min(SIZE - start);
    let mut out = h.ram[base + start..base + start + first].to_vec();
    if n > first {
        let other = if bank == 0 { SECOND } else { FIRST };
        out.extend_from_slice(&h.ram[other..other + n - first]);
    }
    assert_eq!(h.effects[effects], "dma");
    assert_eq!(h.effects.last(), Some(&"irq"));
    out
}
#[test]
fn encoded_clip_decodes_pixels_p_frames_and_gop_restarts() {
    let mut h = TestHost::new();
    let mut d = configured(&mut h);
    let mut decoder = Decoder::new().unwrap();
    let mut first = Vec::new();
    for i in 0..8 {
        input(&mut h, i % 2 == 1);
        let bytes = frame(&mut d, &mut h);
        if i == 0 {
            first = bytes.clone();
            assert_eq!(&bytes[..5], &[0, 0, 0, 1, 0x67]);
        }
        if i == 3 {
            assert_eq!(
                bytes[4] & 31,
                5,
                "Later IDRs reuse the initial parameter sets"
            );
        }
        if i == 1 {
            assert_ne!(bytes, first);
            assert_eq!(bytes[4] & 31, 1);
        }
        let decoded = decoder
            .decode(&bytes)
            .unwrap()
            .expect("each DMA frame must decode");
        assert_eq!(decoded.dimensions(), (64, 32));
        let expected = if i % 2 == 0 { (40, 200) } else { (200, 40) };
        assert!((decoded.y()[8 * decoded.strides().0 + 8] as i16 - expected.0).abs() < 12);
        assert!((decoded.y()[8 * decoded.strides().0 + 48] as i16 - expected.1).abs() < 12);
        assert!((decoded.u()[0] as i16 - 96).abs() < 12);
        assert!((decoded.v()[0] as i16 - 160).abs() < 12);
        write(&mut d, &mut h, 0xfff60c24, 0xffff);
        assert_eq!(d.next_deadline(h.now), None);
    }
    assert_eq!(d.save()["frames"], 8);
    assert!(decoder.flush_remaining().unwrap().is_empty());
}
#[test]
fn ring_crossing_status_ack_and_finish_are_clocked() {
    let mut h = TestHost::new();
    let mut d = configured(&mut h);
    input(&mut h, false);
    write(&mut d, &mut h, 0xfff604ac, (SIZE - 16) as u32);
    let clip = frame(&mut d, &mut h);
    assert!(Decoder::new().unwrap().decode(&clip).unwrap().is_some());
    assert_eq!(read(&mut d, &mut h, 0xfff604a4) & 32, 32);
    assert_eq!(
        read(&mut d, &mut h, 0xfff604ac),
        clip.len().next_multiple_of(4) as u64 - 16
    );
    assert_eq!(read(&mut d, &mut h, 0xfff60c24), (1 << 9) | (1 << 12));
    write(&mut d, &mut h, 0xfff60c24, 1 << 9);
    assert_eq!(d.next_deadline(h.now), None);
    assert_eq!(h.levels.get(&(0, 75)), Some(&true));
    write(&mut d, &mut h, 0xfff60c24, 1 << 12);
    assert_eq!(h.levels.get(&(0, 75)), Some(&false));
    write(&mut d, &mut h, 0xfff60c00, 0x2000);
    let deadline = d.next_deadline(h.now).unwrap();
    advance(&mut d, &mut h, deadline - 1);
    assert_eq!(read(&mut d, &mut h, 0xfff60c24), 0);
    advance_from_checkpoint(&mut d, &mut h, deadline);
    assert_eq!(read(&mut d, &mut h, 0xfff60c24), 1 << 15);
}

#[test]
fn partial_status_ack_holds_the_controller_level_through_clear_and_eoi() {
    use fujimod_emulator_devices::exr2::irq;
    let mut h = TestHost::new();
    let mut d = configured(&mut h);
    input(&mut h, false);
    write(&mut d, &mut h, 0xfff604ac, (SIZE - 16) as u32);
    frame(&mut d, &mut h);
    let pins = irq::Inputs::default();
    let mut controller = irq::InterruptController::new(
        irq::Options {
            arbitration: irq::Arbitration::ActiveBitmap,
        },
        irq::Wiring {
            base: 0x1000,
            inputs: pins.clone(),
            enables: None,
            software: vec![],
        },
    )
    .unwrap();
    write(&mut controller, &mut h, 0x1400 + 75 * 4, 0x6000);
    write(&mut controller, &mut h, 0x1800 + 75 * 4, 3);
    write(&mut controller, &mut h, 0x1088, 1 << 11);
    pins.level(75, h.levels[&(0, 75)]).unwrap();
    let now = h.now;
    advance(&mut controller, &mut h, now);
    assert!(pins.irq());
    assert_eq!(read(&mut controller, &mut h, 0x1200), 0x6000);
    write(&mut d, &mut h, 0xfff60c24, 1 << 9);
    assert!(h.levels[&(0, 75)]);
    write(&mut controller, &mut h, 0x1128, 1 << 11);
    write(&mut controller, &mut h, 0x1200, 0);
    assert!(
        pins.irq(),
        "The remaining frame bit must retrigger after EOI"
    );
    assert_eq!(read(&mut controller, &mut h, 0x1200), 0x6000);
    write(&mut d, &mut h, 0xfff60c24, 1 << 12);
    pins.level(75, h.levels[&(0, 75)]).unwrap();
    advance(&mut controller, &mut h, now);
    write(&mut controller, &mut h, 0x1128, 1 << 11);
    write(&mut controller, &mut h, 0x1200, 0);
    assert!(!pins.irq());
}

#[test]
fn bank_release_preserves_the_other_bank_and_refuses_an_unreleased_wrap() {
    let mut h = TestHost::new();
    let mut d = configured(&mut h);
    let mut decoder = Decoder::new().unwrap();
    input(&mut h, false);
    write(&mut d, &mut h, 0xfff604ac, (SIZE - 16) as u32);
    assert!(decoder.decode(&frame(&mut d, &mut h)).unwrap().is_some());
    write(&mut d, &mut h, 0xfff60c24, 0xffff);
    write(&mut d, &mut h, 0xfff604a4, 0x14);
    input(&mut h, true);
    let bytes = frame(&mut d, &mut h);
    let picture = decoder.decode(&bytes).unwrap().unwrap();
    assert!((picture.y()[8] as i16 - 200).abs() < 12);
    write(&mut d, &mut h, 0xfff60c24, 0xffff);
    write(&mut d, &mut h, 0xfff604ac, (SIZE - 4) as u32);
    assert!(decoder.decode(&frame(&mut d, &mut h)).unwrap().is_some());
    assert_eq!(read(&mut d, &mut h, 0xfff604a4) & 32, 0);
    write(&mut d, &mut h, 0xfff60c24, 0xffff);
    write(&mut d, &mut h, 0xfff604a4, 0x10);
    write(&mut d, &mut h, 0xfff604ac, (SIZE - 4) as u32);
    let effects = h.effects.len();
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff60c00, 4, 0x8000);
    assert!(cx.refusal().unwrap().contains("has not been released"));
    assert_eq!(h.effects.len(), effects);
    assert_eq!(d.next_deadline(h.now), None);
}
#[test]
fn restored_gop_produces_identical_successor_bytes() {
    let mut h = TestHost::new();
    let mut d = configured(&mut h);
    input(&mut h, false);
    frame(&mut d, &mut h);
    write(&mut d, &mut h, 0xfff60c24, 0xffff);
    let checkpoint = d.checkpoint().unwrap();
    let host = h.clone();
    input(&mut h, true);
    let a = frame(&mut d, &mut h);
    let final_state = d.fingerprint().unwrap();
    d.restore_checkpoint(&checkpoint).unwrap();
    h = host;
    input(&mut h, true);
    let b = frame(&mut d, &mut h);
    assert_eq!(a, b);
    assert_eq!(d.fingerprint().unwrap(), final_state);
}
#[test]
fn unsupported_modes_and_invalid_dma_produce_no_completion() {
    for (offset, value) in [
        (0xe00, 63),
        (0xe04, 0x6440),
        (0xe18, 1),
        (0x494, 16),
        (0x498, FIRST as u32),
        (0x400, FIRST as u32),
    ] {
        let mut h = TestHost::new();
        let mut d = configured(&mut h);
        input(&mut h, false);
        write(&mut d, &mut h, 0xfff60000 + offset, value);
        let mut cx = h.cx();
        d.write(&mut cx, 0xfff60c00, 4, 0x8000);
        assert!(cx.refusal().is_some(), "{offset:x}={value:x}");
        assert_eq!(d.next_deadline(h.now), None);
        assert!(h.effects.is_empty());
    }
}
#[test]
fn chroma_conversion_averages_rows_and_preserves_luma() {
    assert_eq!(
        h264::planar_420(2, 2, &[1, 2, 10, 40, 3, 4, 20, 80]).unwrap(),
        vec![1, 2, 3, 4, 15, 60]
    );
}

#[test]
fn start_latches_input_and_destinations_and_queues_finish() {
    let mut h = TestHost::new();
    let mut d = configured(&mut h);
    input(&mut h, false);
    write(&mut d, &mut h, 0xfff60c00, 0x8000);
    let deadline = d.next_deadline(h.now).unwrap();
    input(&mut h, true);
    write(&mut d, &mut h, 0xfff60490, 0x40000);
    {
        let mut cx = h.cx();
        d.write(&mut cx, 0xfff60c00, 4, 0x8000);
        assert!(cx.refusal().unwrap().contains("busy"));
    }
    write(&mut d, &mut h, 0xfff60c00, 0x2000);
    advance_from_checkpoint(&mut d, &mut h, deadline);
    let length =
        ((read(&mut d, &mut h, 0xfff60d88) | read(&mut d, &mut h, 0xfff60d8c) << 16) / 8) as usize;
    let mut decoder = Decoder::new().unwrap();
    let image = decoder
        .decode(&h.ram[FIRST..FIRST + length])
        .unwrap()
        .unwrap();
    assert!((image.y()[8] as i16 - 40).abs() < 12);
    assert!(h.ram[0x40000..0x40000 + length].iter().all(|b| *b == 0));
    // The initial frame did not cross a bank, so it has no bank notice.
    assert_eq!(read(&mut d, &mut h, 0xfff60c24), 1 << 12);
    write(&mut d, &mut h, 0xfff60c24, 1 << 12);
    assert_eq!(d.next_deadline(h.now), Some(deadline + 2));
    advance(&mut d, &mut h, deadline + 2);
    assert_eq!(read(&mut d, &mut h, 0xfff60c24), 1 << 15);
}

#[test]
fn backend_selection_is_explicit() {
    assert!(serde_json::from_str::<avc::Options>("{}").is_err());
    assert!(serde_json::from_str::<avc::Options>(r#"{"backend":"automatic"}"#).is_err());
}

#[test]
#[cfg(all(target_os = "macos", feature = "videotoolbox-tests"))]
fn hardware_backend_obeys_dma_clock_and_decodes_real_frames() {
    let mut h = TestHost::new();
    let mut d = configured_backend(
        &mut h,
        fujimod_catalog_construction::VideoEncoderBackend::Videotoolbox,
    );
    let mut decoder = Decoder::new().unwrap();
    for i in 0..4 {
        input(&mut h, i % 2 == 1);
        let bytes = frame(&mut d, &mut h);
        let image = decoder.decode(&bytes).unwrap().unwrap();
        assert_eq!(image.dimensions(), (64, 32));
        let value = if i % 2 == 0 { 40 } else { 200 };
        assert!((image.y()[8] as i16 - value).abs() < 16);
        assert_eq!(d.save()["backend"], "videotoolbox");
        write(&mut d, &mut h, 0xfff60c24, 0xffff);
    }
    write(&mut d, &mut h, 0xfff60c00, 0x2000);
    let deadline = d.next_deadline(h.now).unwrap();
    advance(&mut d, &mut h, deadline);
    assert!(d.checkpoint().is_ok());
}
