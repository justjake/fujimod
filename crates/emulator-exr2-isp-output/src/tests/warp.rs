use crate::*;
use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::test_host::*;

#[test]
fn warp_dma_and_jpeg_stream_keep_a_centered_unity_scale_crop() {
    for streamed in [false, true] {
        let mut h = TestHost::new();
        let mut expected = Vec::new();
        for y in 0..4usize {
            for x in (0..8usize).step_by(2) {
                let pair = [(y * 16 + x) as u8, (y * 16 + x + 1) as u8, 128, 128];
                h.ram[0x8000 + y * 16 + x * 2..0x8000 + y * 16 + x * 2 + 4].copy_from_slice(&pair);
                if x == 2 || x == 4 {
                    expected.extend_from_slice(&pair);
                }
            }
        }
        let stream = fujimod_emulator_imaging::exr2_isp::stream::StreamHandle::default();
        let mut d = Warp::new(
            fujimod_emulator_imaging::exr2_isp::options::warp::Options {
                completion_ticks: 5,
                stop_ticks: 2,
            },
            (Irq { core: 0, line: 74 }, stream.clone()),
        )
        .unwrap();
        for (o, v) in [
            (4, if streamed { 0x40 } else { 0 }),
            (0x10, 0x8000),
            (0x14, 8),
            (0x18, 4 << 16 | 8),
            (0x1c, 0x9000),
            (0x20, 4),
            (0x28, 4 << 16 | 4),
            (0x38, 4096),
            (0x3c, 4096),
            (0x40, 4096),
            (0x44, 4096),
            (0x4c, 0x10001),
            (0x50, 3 << 16 | 7),
            (0x54, 3 << 16 | 3),
            (0x148, 0x21),
            (0, 1),
        ] {
            write(&mut d, &mut h, 0xfff65000 + o, v);
        }
        if streamed {
            let stream = stream.lock().unwrap();
            assert_eq!(stream.width, 4);
            assert_eq!(stream.height, 4);
            assert_eq!(stream.pixels.to_vec(), expected);
        } else {
            advance(&mut d, &mut h, 5);
            assert_eq!(&h.ram[0x9000..0x9020], expected);
        }
    }
}

#[test]
fn warp_memory_dma_keeps_alignment_rows_and_acknowledges_status() {
    let mut h = TestHost::new();
    h.ram[0x8000..0x8010]
        .copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 13, 14, 15, 16, 17, 18]);
    h.ram[0x9000..0x9040].fill(0xa5);
    let mut d = Warp::new(
        fujimod_emulator_imaging::exr2_isp::options::warp::Options {
            completion_ticks: 5,
            stop_ticks: 2,
        },
        (Irq { core: 0, line: 74 }, Default::default()),
    )
    .unwrap();
    for (o, v) in [
        (0x10, 0x8000),
        (0x14, 4),
        (0x18, 2 << 16 | 4),
        (0x1c, 0x9000),
        (0x20, 8),
        (0x28, 4 << 16 | 6),
        (0x38, 4096),
        (0x3c, 4096),
        (0x40, 4096),
        (0x44, 4096),
        (0x4c, 0x10001),
        (0x50, 1 << 16 | 3),
        (0x54, 1 << 16 | 3),
        (0x148, 0x21),
        (0, 1),
    ] {
        write(&mut d, &mut h, 0xfff65000 + o, v);
    }
    advance(&mut d, &mut h, 4);
    assert_eq!(h.ram[0x9000], 0xa5);
    let identity = d.fingerprint().unwrap();
    let diagnostic = d.diagnostic_state();
    assert!(diagnostic[1]["pixels"].is_object());
    assert_eq!(
        d.diagnostic_buffer("dma-output").unwrap(),
        &[1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 13, 14, 15, 16, 17, 18]
    );
    assert!(d.diagnostic_buffer("missing").is_err());
    assert_eq!(d.fingerprint().unwrap(), identity);
    advance_from_checkpoint(&mut d, &mut h, 5);
    assert_eq!(&h.ram[0x9000..0x9008], &h.ram[0x8000..0x8008]);
    assert_eq!(&h.ram[0x9010..0x9018], &h.ram[0x8008..0x8010]);
    assert!(h.ram[0x9020..0x9040].iter().all(|&v| v == 0xa5));
    assert_eq!(h.irqs, [(0, 74)]);
    assert_eq!(read(&mut d, &mut h, 0xfff65144), 0x20);
    write(&mut d, &mut h, 0xfff65144, 0x20);
    assert_eq!(read(&mut d, &mut h, 0xfff65144), 0);
    write(&mut d, &mut h, 0xfff65000, 0);
    advance(&mut d, &mut h, 6);
    assert_eq!(read(&mut d, &mut h, 0xfff65000), 0);
    advance(&mut d, &mut h, 7);
    assert_eq!(read(&mut d, &mut h, 0xfff65000), 0x10000);
}

#[test]
fn worker_warp_retains_deadlines_padding_status_and_checkpoint_bytes() {
    fujimod_emulator_devices::work::configure(2).unwrap();
    warp_memory_dma_keeps_alignment_rows_and_acknowledges_status();
    fujimod_emulator_devices::work::configure(0).unwrap();
}

#[test]
fn warp_bounds_a_logical_source_view_that_crosses_its_row_pitch() {
    let mut h = TestHost::new();
    for (i, byte) in h.ram[0x8000..0x8020].iter_mut().enumerate() {
        *byte = i as u8;
    }
    h.ram[0x9000..0x9020].fill(0xa5);
    let mut d = Warp::new(
        fujimod_emulator_imaging::exr2_isp::options::warp::Options {
            completion_ticks: 5,
            stop_ticks: 2,
        },
        (Irq { core: 0, line: 74 }, Default::default()),
    )
    .unwrap();
    for (o, v) in [
        (0x10, 0x8000),
        (0x14, 4),
        (0x18, 2 << 16 | 6),
        (0x1c, 0x9000),
        (0x20, 4),
        (0x28, 2 << 16 | 4),
        (0x38, 4096),
        (0x3c, 4096),
        (0x40, 4096),
        (0x44, 4096),
        (0x4c, 0x10001),
        (0x50, 1 << 16 | 5),
        (0x54, 1 << 16 | 3),
        (0x148, 0x21),
        (0, 1),
    ] {
        write(&mut d, &mut h, 0xfff65000 + o, v);
    }
    advance(&mut d, &mut h, 5);
    assert_eq!(&h.ram[0x9000..0x9008], &[1, 4, 2, 3, 5, 8, 6, 7]);
    assert_eq!(&h.ram[0x9008..0x9010], &[9, 12, 10, 11, 13, 16, 14, 15]);
    assert!(h.ram[0x9010..0x9020].iter().all(|&byte| byte == 0xa5));
}

#[test]
fn warp_rejects_a_selected_source_endpoint_past_physical_dram() {
    assert_eq!(
        fujimod_emulator_image_processing::resample::Centered::source_bytes(
            6,
            2,
            8,
            4,
            2,
            [5, 1],
            [4096, 4096],
            0,
        )
        .unwrap(),
        20
    );
    assert!(fujimod_emulator_imaging::exr2_isp::dma::physical(0x3ffffff0, 20).is_err());
}

/// Warp registers for one stock-programmed geometry: 0x34 shift, 0x38/0x3c
/// scales, 0x50 source limits, 0x54 destination centers.
fn span_registers(shift: u32, scale: [u32; 2], limit: [u32; 2], center: [u32; 2]) -> Vec<u32> {
    let mut r = vec![0; 0x14c / 4];
    r[0x34 / 4] = shift;
    r[0x38 / 4] = scale[0];
    r[0x3c / 4] = scale[1];
    r[0x50 / 4] = limit[1] << 16 | limit[0];
    r[0x54 / 4] = center[1] << 16 | center[0];
    r
}

/// X70 1.10 camera, 2026-11-01: an M 65:24 JPEG with limit 1812 for a
/// 1812.02-row span saved a green first row and froze one shot with HIGH
/// PERFORMANCE on; the shift-1 RAF preview with 2448 x 908 limits hung the
/// encoder. Covering limits saved every combination.
#[test]
fn warp_refuses_centered_spans_past_the_programmed_source_limit() {
    let m_jpeg = |limit_y| span_registers(0, [5803, 5803], [4895, limit_y], [3455, 1279]);
    let raf_preview = |limit| span_registers(1, [5222, 5223], limit, [1919, 711]);
    let refused = crate::warp::centered_span_fits(&m_jpeg(1812)).unwrap_err();
    assert!(
        refused.contains("vertical span 1279 x 5803/4096 << 0"),
        "{refused}"
    );
    let refused = crate::warp::centered_span_fits(&raf_preview([2448, 908])).unwrap_err();
    assert!(
        refused.contains("horizontal span 1919 x 5222/4096 << 1"),
        "{refused}"
    );
    crate::warp::centered_span_fits(&m_jpeg(1813)).unwrap();
    crate::warp::centered_span_fits(&raf_preview([4894, 1814])).unwrap();
    // Stock full-frame scales end exactly on the limit.
    crate::warp::centered_span_fits(&span_registers(0, [4095, 4095], [3, 1], [3, 1])).unwrap();

    let mut h = TestHost::new();
    let mut d = Warp::new(
        fujimod_emulator_imaging::exr2_isp::options::warp::Options {
            completion_ticks: 5,
            stop_ticks: 2,
        },
        (Irq { core: 0, line: 74 }, Default::default()),
    )
    .unwrap();
    for (o, v) in [
        (0x10, 0x8000),
        (0x14, 4896),
        (0x18, 1813 << 16 | 4896),
        (0x1c, 0x9000),
        (0x20, 3456),
        (0x28, 1280 << 16 | 3456),
        (0x38, 5803),
        (0x3c, 5803),
        (0x40, 4096),
        (0x44, 4096),
        (0x4c, 0x10001),
        (0x50, 1812 << 16 | 4895),
        (0x54, 1279 << 16 | 3455),
        (0x148, 0x21),
    ] {
        write(&mut d, &mut h, 0xfff65000 + o, v);
    }
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff65000, 4, 1);
    assert_eq!(
        cx.refusal(),
        Some("warp vertical span 1279 x 5803/4096 << 0 overruns source limit 1812")
    );
}
