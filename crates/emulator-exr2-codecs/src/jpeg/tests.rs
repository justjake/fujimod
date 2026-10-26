use super::*;
use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::test_host::{
    TestHost, advance, advance_from_checkpoint, read, write,
};
use fujimod_emulator_imaging::exr2_isp::stream::{State, StreamHandle};
fn encoder(stream: StreamHandle) -> Encoder {
    Encoder::new(
        encoder::Options {
            completion_ticks: 10,
            reset_ticks: 2,
        },
        (Irq { core: 0, line: 77 }, stream),
    )
    .unwrap()
}
fn configure(d: &mut Encoder, h: &mut TestHost, stream: bool) {
    for (o, v) in [
        (0, 4),
        (0x28, 0xfd0000),
        (0x40, 1),
        (0x44, 8 << 16 | 16),
        (0x80, 4),
        (0x84, 0x100),
    ] {
        write(d, h, 0xfff62000 + o, v);
    }
    for _ in 0..32 {
        write(d, h, 0xfff6208c, 0x03030303);
    }
    write(d, h, 0xfff62084, 1);
    for (o, v) in [
        (0x100, 0x8000),
        (0x10c, 0x9000),
        (0x110, 4096),
        (0x120, if stream { 2 } else { 0 }),
    ] {
        write(d, h, 0xfff62000 + o, v);
    }
}
#[test]
fn memory_encode_publishes_complete_jpeg_and_decoder_pixels_before_irq() {
    let mut h = TestHost::new();
    h.ram[0x8000..0x8100].fill(128);
    h.ram[0x9000..0xa000].fill(0xa5);
    let mut d = encoder(Default::default());
    configure(&mut d, &mut h, false);
    write(&mut d, &mut h, 0xfff62000, 5);
    advance(&mut d, &mut h, 9);
    assert!(h.effects.is_empty());
    advance_from_checkpoint(&mut d, &mut h, 10);
    let count = read(&mut d, &mut h, 0xfff62094) as u32;
    assert!(count > 500);
    assert_eq!(&h.ram[0x9000..0x9004], &[255, 0xd8, 255, 0xc4]);
    let jpeg = &h.ram[0x9000..0x9000 + count as usize];
    assert_eq!(&jpeg[jpeg.len() - 2..], &[255, 0xd9]);
    let mut standard = zune_jpeg::JpegDecoder::new(zune_core::bytestream::ZCursor::new(jpeg));
    assert_eq!(standard.decode().unwrap().len(), 16 * 8 * 3);
    assert_eq!(h.ram[0x9000 + count as usize], 0xa5);
    assert_eq!(h.effects, ["dma", "irq"]);
    assert_eq!(read(&mut d, &mut h, 0xfff62024), 16);
    let mut decoder = Decoder::new(
        decoder::Options {
            completion_ticks: 5,
            reset_ticks: 2,
        },
        Irq { core: 0, line: 76 },
    )
    .unwrap();
    for (o, v) in [
        (8, 0xfd0020),
        (0x100, 0xa000),
        (0x104, 256),
        (0x10c, 0x9000),
        (0x110, count),
        (0, 1),
    ] {
        write(&mut decoder, &mut h, 0xfff63000 + o, v);
    }
    advance(&mut decoder, &mut h, 14);
    assert_eq!(h.irqs.len(), 1);
    advance_from_checkpoint(&mut decoder, &mut h, 15);
    assert_eq!(&h.ram[0xa000..0xa004], &[128, 128, 128, 128]);
    assert_eq!(h.irqs, [(0, 77), (0, 76)]);
    assert_eq!(read(&mut decoder, &mut h, 0xfff63108), count as u64);
    write(&mut decoder, &mut h, 0xfff63004, 16);
    assert_eq!(read(&mut decoder, &mut h, 0xfff63004), 0);
    write(&mut decoder, &mut h, 0xfff63118, 1);
    advance(&mut decoder, &mut h, 16);
    assert_eq!(read(&mut decoder, &mut h, 0xfff63004), 0);
    advance(&mut decoder, &mut h, 17);
    assert_eq!(read(&mut decoder, &mut h, 0xfff63004), 0x10000);
}
#[test]
fn wide_and_tall_jpegs_decode_with_programmed_downsampling_and_bounded_dma() {
    for (width, height) in [(9792, 8), (16, 8200)] {
        let mut h = TestHost::new();
        h.ram[0x10000..0x10000 + width * height * 2].fill(128);
        h.ram[0x60000..0x60002].copy_from_slice(&[255, 216]);
        let mut encoded = encoder(Default::default());
        configure(&mut encoded, &mut h, false);
        for (offset, value) in [
            (0x44, (height << 16 | width) as u32),
            (0x100, 0x10000),
            (0x10c, 0x60000),
            (0x110, 0x10000),
            (0, 5),
        ] {
            write(&mut encoded, &mut h, 0xfff62000 + offset, value);
        }
        advance(&mut encoded, &mut h, 10);
        let count = read(&mut encoded, &mut h, 0xfff62094) as u32;
        let mut decoded = Decoder::new(
            decoder::Options {
                completion_ticks: 5,
                reset_ticks: 2,
            },
            Irq { core: 0, line: 76 },
        )
        .unwrap();
        for (offset, value) in [
            (8, 0xfd0020),
            (0xa0, 1),
            (0x100, 0x80000),
            (0x104, 1),
            (0x10c, 0x60000),
            (0x110, count),
        ] {
            write(&mut decoded, &mut h, 0xfff63000 + offset, value);
        }
        let mut cx = h.cx();
        decoded.write(&mut cx, 0xfff63000, 4, 1);
        assert!(cx.refusal().unwrap().contains("allocation"));
        assert_eq!(decoded.next_deadline(10), None);
        write(&mut decoded, &mut h, 0xfff63104, 0x30000);
        write(&mut decoded, &mut h, 0xfff63000, 1);
        advance(&mut decoded, &mut h, 14);
        assert_eq!(h.irqs, [(0, 77)]);
        advance_from_checkpoint(&mut decoded, &mut h, 15);
        let pitch = ((width / 2 + 15) & !15) * 2;
        for row in [0, height / 2 - 1] {
            let start = 0x80000 + row * pitch;
            assert_eq!(&h.ram[start..start + 4], &[128; 4]);
        }
        assert_eq!(h.irqs, [(0, 77), (0, 76)]);
        assert_eq!(read(&mut decoded, &mut h, 0xfff63108), count as u64);
        let sof = 0x60000
            + h.ram[0x60000..0x60000 + count as usize]
                .windows(2)
                .position(|bytes| bytes == [255, 192])
                .unwrap();
        h.ram[sof + 5..sof + 9].copy_from_slice(&[255, 240, 255, 240]);
        let mut cx = h.cx();
        decoded.write(&mut cx, 0xfff63000, 4, 1);
        assert!(cx.refusal().unwrap().contains("dimensions are unsupported"));
        assert_eq!(decoded.next_deadline(15), None);
        assert_eq!(h.irqs, [(0, 77), (0, 76)]);
    }
}

#[test]
fn encode_capacity_failure_does_not_claim_stream_or_write_ram() {
    let mut h = TestHost::new();
    let stream: StreamHandle = Default::default();
    {
        let mut s = stream.lock().unwrap();
        s.state = State::Ready;
        s.width = 16;
        s.height = 8;
        s.pixels = vec![128; 256].into();
    }
    let mut e = encoder(stream.clone());
    configure(&mut e, &mut h, true);
    write(&mut e, &mut h, 0xfff62110, 8);
    let mut cx = h.cx();
    e.write(&mut cx, 0xfff62000, 4, 5);
    assert!(cx.refusal().is_some());
    assert_eq!(stream.lock().unwrap().state, State::Ready);
    assert!(h.effects.is_empty());
    assert_eq!(e.next_deadline(0), None);
}

#[test]
fn unselected_quantization_is_retained_without_fifo_dma_or_irq() {
    let mut h = TestHost::new();
    let mut d = encoder(Default::default());
    configure(&mut d, &mut h, false);
    let uploaded = d.save()[1].clone();
    assert_eq!(d.save()[2], 128);
    write(&mut d, &mut h, 0xfff62084, 0);
    assert_eq!(d.save()[1], uploaded);
    assert_eq!(d.save()[2], 128);
    assert_eq!(d.save()[0][0x84 / 4], 0);
    assert_eq!(d.next_deadline(0), None);
    let before = d.fingerprint().unwrap();
    advance(&mut d, &mut h, 10000);
    assert_eq!(d.fingerprint().unwrap(), before);
    assert!(h.effects.is_empty());
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff6208c, 4, 0x03030303);
    assert!(cx.refusal().unwrap().contains("outside FIFO"));
    assert_eq!(d.fingerprint().unwrap(), before);
    write(&mut d, &mut h, 0xfff62084, 1);
    assert_eq!(d.save()[1], uploaded);
    assert_eq!(d.save()[3], true);
    assert!(h.effects.is_empty());
    write(&mut d, &mut h, 0xfff62000, 5);
    advance(&mut d, &mut h, 10010);
    assert!(read(&mut d, &mut h, 0xfff62094) > 2);
    assert_eq!(h.effects, ["dma", "irq"]);
}

#[test]
fn programmed_restart_nine_reaches_the_real_codestream_and_decoder() {
    // Restart interval 9 emits ten cyclic RST markers for a 128x96 4:2:2
    // image (96 MCUs).
    let mut h = TestHost::new();
    h.ram[0x8000..0xe000].fill(128);
    h.ram[0x10000..0x14000].fill(0xa5);
    h.ram[0x10000..0x10002].copy_from_slice(&[255, 0xd8]);
    let mut d = encoder(Default::default());
    configure(&mut d, &mut h, false);
    for (o, v) in [
        (0x44, 96 << 16 | 128),
        (0x80, 9),
        (0x10c, 0x10000),
        (0x110, 0x4000),
    ] {
        write(&mut d, &mut h, 0xfff62000 + o, v);
    }
    write(&mut d, &mut h, 0xfff62000, 5);
    advance(&mut d, &mut h, 9);
    assert_eq!(&h.ram[0x10002..0x10006], &[0xa5; 4]);
    assert!(h.effects.is_empty());
    advance(&mut d, &mut h, 10);
    let bytes = read(&mut d, &mut h, 0xfff62094) as usize;
    let encoded = &h.ram[0x10000..0x10000 + bytes];
    assert!(encoded.windows(6).any(|v| v == [255, 0xdd, 0, 4, 0, 9]));
    let markers = encoded
        .windows(2)
        .filter(|v| v[0] == 255 && (0xd0..=0xd7).contains(&v[1]))
        .map(|v| v[1] - 0xd0)
        .collect::<Vec<_>>();
    assert_eq!(markers, (0..10).map(|v| v % 8).collect::<Vec<_>>());
    let options = zune_core::options::DecoderOptions::new_safe().set_use_unsafe(false);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(
        zune_core::bytestream::ZCursor::new(encoded),
        options,
    );
    let rgb = decoder.decode().unwrap();
    assert_eq!(rgb.len(), 128 * 96 * 3);
    assert!(rgb.iter().all(|v| (127..=129).contains(v)));
    assert_eq!(h.effects, ["dma", "irq"]);
    assert_eq!(h.irqs, [(0, 77)]);
}

#[test]
fn unqualified_source_field_names_its_register_and_refuses_before_any_effect() {
    let mut h = TestHost::new();
    let mut d = encoder(Default::default());
    configure(&mut d, &mut h, false);
    write(&mut d, &mut h, 0xfff62048, 0x12);
    let before = d.fingerprint().unwrap();
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff62000, 4, 5);
    let message = cx.refusal().unwrap();
    assert!(message.contains("0xfff62048=0x12"));
    assert_eq!(d.fingerprint().unwrap(), before);
    assert!(h.effects.is_empty());
    assert_eq!(d.next_deadline(0), None);
}

#[test]
fn implicit_and_explicit_packed_input_modes_encode_the_same_addressed_pixels() {
    let mut encodings = Vec::new();
    for mode in [0, 0x11] {
        let mut h = TestHost::new();
        h.ram[0x8000..0x8100].fill(128);
        let mut d = encoder(Default::default());
        configure(&mut d, &mut h, false);
        write(&mut d, &mut h, 0xfff62048, mode);
        write(&mut d, &mut h, 0xfff62000, 5);
        assert_eq!(d.next_deadline(0), Some(10));
        advance(&mut d, &mut h, 9);
        assert!(h.effects.is_empty());
        advance(&mut d, &mut h, 10);
        let count = read(&mut d, &mut h, 0xfff62094) as usize;
        assert!(count > 500);
        assert_eq!(h.effects, ["dma", "irq"]);
        assert_eq!(h.irqs, [(0, 77)]);
        encodings.push(h.ram[0x9000..0x9000 + count].to_vec());
    }
    assert_eq!(encodings[0], encodings[1]);
}

#[test]
fn worker_codec_retains_exact_stream_bytes_error_state_and_completion() {
    fujimod_emulator_devices::work::configure(2).unwrap();
    memory_encode_publishes_complete_jpeg_and_decoder_pixels_before_irq();
    encode_capacity_failure_does_not_claim_stream_or_write_ram();
    programmed_restart_nine_reaches_the_real_codestream_and_decoder();
    implicit_and_explicit_packed_input_modes_encode_the_same_addressed_pixels();
    fujimod_emulator_devices::work::configure(0).unwrap();
}

#[test]
fn stream_encode_start_retains_pending_warp_without_joining_or_publishing() {
    use std::sync::mpsc;
    fujimod_emulator_devices::work::configure(2).unwrap();
    let (release, held) = mpsc::channel();
    let (started, running) = mpsc::channel();
    let stream: StreamHandle = Default::default();
    {
        let mut s = stream.lock().unwrap();
        s.state = State::Ready;
        s.width = 16;
        s.height = 8;
        s.pixels.defer(move |_| {
            started.send(()).unwrap();
            held.recv().unwrap();
            vec![128; 256]
        });
    }
    running.recv().unwrap();
    let mut h = TestHost::new();
    h.ram[0x9000..0xa000].fill(0xa5);
    let mut e = encoder(stream.clone());
    configure(&mut e, &mut h, true);
    write(&mut e, &mut h, 0xfff62000, 5);
    assert_eq!(stream.lock().unwrap().state, State::Claimed);
    assert!(h.effects.is_empty());
    assert!(h.ram[0x9000..0xa000].iter().all(|&v| v == 0xa5));
    release.send(()).unwrap();
    advance_from_checkpoint(&mut e, &mut h, 9);
    assert!(h.effects.is_empty());
    advance(&mut e, &mut h, 10);
    assert_eq!(h.effects, ["dma", "irq"]);
    assert_eq!(stream.lock().unwrap().state, State::Finished);
    assert_eq!(&h.ram[0x9000..0x9004], &[255, 0xd8, 255, 0xc4]);
    fujimod_emulator_devices::work::configure(0).unwrap();
}

#[test]
fn deferred_entropy_overflow_survives_checkpoint_and_completes_without_ready_or_ram() {
    fujimod_emulator_devices::work::configure(2).unwrap();
    let stream: StreamHandle = Default::default();
    {
        let mut s = stream.lock().unwrap();
        s.state = State::Ready;
        s.width = 16;
        s.height = 8;
        s.pixels = (0..256)
            .map(|i| ((i * 73 + i / 7) % 256) as u8)
            .collect::<Vec<_>>()
            .into();
    }
    let mut h = TestHost::new();
    h.ram[0x9000..0xa000].fill(0xa5);
    let mut e = encoder(stream.clone());
    configure(&mut e, &mut h, true);
    write(&mut e, &mut h, 0xfff62110, 640);
    write(&mut e, &mut h, 0xfff62000, 5);
    let saved = e.checkpoint().unwrap();
    e.restore_checkpoint(&saved).unwrap();
    assert!(h.effects.is_empty());
    h.now = 10;
    {
        let mut cx = h.cx();
        e.advance(&mut cx);
        assert_eq!(cx.refusal(), None);
    }
    // Stock channel8_complete: no READY bit means status 2, zero bytes.
    assert_eq!(read(&mut e, &mut h, 0xfff62024) & 0x30, 0x20);
    assert_eq!(read(&mut e, &mut h, 0xfff62094), 0);
    assert_eq!(stream.lock().unwrap().state, State::Finished);
    assert!(h.ram[0x9000..0xa000].iter().all(|&v| v == 0xa5));
    assert_eq!(h.irqs, [(0, 77)]);
    assert_eq!(e.next_deadline(10), None);
    fujimod_emulator_devices::work::configure(0).unwrap();
}
