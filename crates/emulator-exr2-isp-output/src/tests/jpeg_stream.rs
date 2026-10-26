//! The JPEG encoder consuming a warp's pixel stream.
use crate::warp::Warp;
use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_imaging::exr2_isp::stream::{State, StreamHandle};
use fujimod_emulator_exr2_codecs::jpeg::{Encoder, encoder};
use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

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
fn warp_stream_requires_both_starts_and_releases_only_at_completion() {
    let mut h = TestHost::new();
    h.ram[0x8000..0x8100].fill(128);
    let stream: StreamHandle = Default::default();
    let mut w = Warp::new(
        fujimod_emulator_imaging::exr2_isp::options::warp::Options {
            completion_ticks: 5,
            stop_ticks: 2,
        },
        (Irq { core: 0, line: 74 }, stream.clone()),
    )
    .unwrap();
    let mut e = encoder(stream.clone());
    configure(&mut e, &mut h, true);
    {
        let mut cx = h.cx();
        e.write(&mut cx, 0xfff62000, 4, 5);
        assert!(cx.refusal().is_some());
    }
    for (o, v) in [
        (4, 0x40),
        (0x10, 0x8000),
        (0x14, 16),
        (0x18, 8 << 16 | 16),
        (0x20, 16),
        (0x28, 8 << 16 | 16),
        (0x40, 4096),
        (0x44, 4096),
        (0x4c, 0x10001),
        (0x50, 7 << 16 | 15),
        (0x54, 7 << 16 | 15),
        (0x148, 0x21),
        (0, 1),
    ] {
        write(&mut w, &mut h, 0xfff65000 + o, v);
    }
    assert_eq!(stream.lock().unwrap().state, State::Ready);
    write(&mut e, &mut h, 0xfff62000, 5);
    assert_eq!(stream.lock().unwrap().state, State::Claimed);
    assert_eq!(stream.lock().unwrap().pixels.len(), 256);
    {
        let mut cx = h.cx();
        w.write(&mut cx, 0xfff65000, 4, 0);
        assert!(cx.refusal().is_some());
    }
    advance(&mut e, &mut h, 10);
    assert_eq!(stream.lock().unwrap().state, State::Finished);
    assert!(stream.lock().unwrap().pixels.is_empty());
    assert_eq!(w.next_deadline(10), Some(10));
    advance(&mut w, &mut h, 10);
    assert_eq!(read(&mut w, &mut h, 0xfff65000), 0x10000);
    assert_eq!(stream.lock().unwrap().state, State::Idle);
    assert_eq!(h.irqs, [(0, 77)]);
}

#[test]
fn warp_stream_with_codec_workers() {
    fujimod_emulator_devices::work::configure(2).unwrap();
    warp_stream_requires_both_starts_and_releases_only_at_completion();
    fujimod_emulator_devices::work::configure(0).unwrap();
}
