use super::*;
use fujimod_emulator_devices::test_host::recording::RecordingHost;
#[test]
fn native_pixmap_copy_preserves_words_flips_rows_and_keeps_pitch_padding() {
    let mut host = RecordingHost {
        ram_base: 0x1000,
        ram: vec![0xa5; 0x4000],
        ..Default::default()
    };
    for y in 0..16 {
        for x in 0..32 {
            let i = pixel_index(x, y, 32, true) * 4;
            host.ram[i..i + 4].copy_from_slice(&[255, x as u8, y as u8, 31]);
        }
    }
    let mut state = State::default();
    state.regs.extend([
        (0xc00, 0x1000),
        (0xc04, 0x3000),
        (0xc08, 0x00100020),
        (0xc0c, 144),
        (0xc10, 1),
    ]);
    let job = state.copy_job(&Cx::new(&mut host, 0, 0, 0), 1, PixmapReadback::RotatesChannels).unwrap();
    assert_eq!(job.writes.len(), 1);
    let (address, bytes) = &job.writes[0];
    assert_eq!(*address, 0x3000);
    assert_eq!(bytes.len(), 144 * 16);
    for y in 0..16 {
        for x in 0..32 {
            assert_eq!(
                &bytes[y * 144 + x * 4..y * 144 + x * 4 + 4],
                &[255, x as u8, (15 - y) as u8, 31]
            );
        }
        assert_eq!(&bytes[y * 144 + 128..(y + 1) * 144], &[0xa5; 16]);
    }
    assert!(host.ram[0x2000..].iter().all(|b| *b == 0xa5));
}
#[test]
fn exact_checkpoint_restores_streams_and_counters_and_refuses_pending_host_work() {
    use fujimod_emulator_devices::test_host::{TestHost, assert_exact_restore, write};
    let mut renderer = Renderer::new(0xff93c000, 92, PixmapReadback::RotatesChannels).unwrap();
    let mut host = TestHost::new();
    write(&mut renderer, &mut host, 0xff93c000 + 0xc00, 0x1000);
    let saved = assert_exact_restore(&mut renderer, |d| {
        let mut host = TestHost::new();
        write(d, &mut host, 0xff93c000 + 0xc04, 0x3000);
    });
    renderer.state.lock().unwrap().next_host_request = 7;
    let counted = renderer.checkpoint().unwrap();
    renderer.state.lock().unwrap().next_host_request = 0;
    renderer.restore_checkpoint(&counted).unwrap();
    assert_eq!(renderer.state.lock().unwrap().next_host_request, 7);
    renderer.state.lock().unwrap().host_request = Some(PendingHostRequest {
        id: 1,
        payload: vec![],
        published: false,
    });
    assert!(renderer.checkpoint().is_err());
    assert!(
        Renderer::new(0xff93c000, 92, PixmapReadback::RotatesChannels)
            .unwrap()
            .restore_checkpoint(&saved)
            .is_ok()
    );
}
#[test]
fn equal_shape_copy_rotates_or_keeps_rgba8_words_as_the_processor_selects() {
    // The renderer stores RGBA8 as memory bytes A,B,G,R: 0x10,0xe0,0x80,0x20 is
    // red 0x20, green 0x80, blue 0xe0, alpha 0x10.
    let copy = |readback| {
        let mut host = RecordingHost {
            ram_base: 0x1000,
            ram: vec![0; 0x2000],
            ..Default::default()
        };
        for y in 0..8 {
            for x in 0..8 {
                let i = pixel_index(x, y, 8, true) * 4;
                host.ram[i..i + 4].copy_from_slice(&[0x10, 0xe0, 0x80, 0x20]);
            }
        }
        let mut state = State::default();
        state.regs.extend([
            (0xc00, 0x1000),
            (0xc04, 0x2000),
            (0xc08, 0x00080008),
            (0xc0c, 0x00080008),
            (0xc10, 1),
        ]);
        let job = state
            .copy_job(&Cx::new(&mut host, 0, 0, 0), 1, readback)
            .unwrap();
        job.writes[0].1[..4].to_vec()
    };
    assert_eq!(copy(PixmapReadback::RotatesChannels), [0xe0, 0x80, 0x20, 0x10]);
    assert_eq!(copy(PixmapReadback::PreservesWords), [0x10, 0xe0, 0x80, 0x20]);
}
