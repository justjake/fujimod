use fujimod_emulator_devices::exr2::irq_routes::IrqRoutes;
use crate::*;
use fujimod_emulator_exr2_isp_testing::*;
use fujimod_emulator_imaging::analysis::ImageLayout;
use fujimod_emulator_imaging::scene::{FrameEdge, ImageFrames, StatisticsFrames, StatisticsSpan};
use fujimod_emulator_devices::Cx;
use fujimod_emulator_devices::Device;
pub(super) fn producer(
    hist: bool,
) -> (Block18000, StatisticsFrames, ImageFrames, IrqRoutes, Memory) {
    producer_with_layout(hist, 4, 2)
}
fn producer_with_layout(
    hist: bool,
    width: usize,
    height: usize,
) -> (Block18000, StatisticsFrames, ImageFrames, IrqRoutes, Memory) {
    producer_with_clock(hist, width, height, None)
}
fn producer_with_clock(
    hist: bool,
    width: usize,
    height: usize,
    pixels_per_tick: Option<u64>,
) -> (Block18000, StatisticsFrames, ImageFrames, IrqRoutes, Memory) {
    producer_with_metering(hist, width, height, pixels_per_tick, None)
}
fn producer_with_metering(
    hist: bool,
    width: usize,
    height: usize,
    pixels_per_tick: Option<u64>,
    metering: Option<fujimod_emulator_imaging::exr2_isp::options::metering::Arithmetic>,
) -> (Block18000, StatisticsFrames, ImageFrames, IrqRoutes, Memory) {
    let images = ImageFrames::from_pixels(
        ImageLayout {
            width,
            height,
            stride: width * 2,
        },
        vec![5; width * height * 2],
    )
    .unwrap();
    let spans = vec![
        StatisticsSpan {
            destination: 0,
            offset: 0,
            bytes: vec![3; 4],
        },
        StatisticsSpan {
            destination: 1,
            offset: 0,
            bytes: vec![4; 4],
        },
    ];
    let frames = StatisticsFrames::new(spans).unwrap();
    let routes = routes(0x2e);
    let mut d = Block18000::new(
        fujimod_emulator_imaging::exr2_isp::options::producer::Options {
            pixels_per_tick,
            contrast: None,
            metering,
            clock: fujimod_emulator_imaging::exr2_isp::dma::Clock::Fixed { ticks: 5 },
            ack_ticks: 2,
            stop_ticks: 3,
            histogram: hist,
            external_statistics: false,
            diagnostic_reads: true,
        },
        producer::Wiring {
            grid: None,

            frames: frames.clone(),
            images: images.clone(),
            routes: routes.clone(),
        },
    )
    .unwrap();
    let mut m = Memory::default();
    for i in 0..9 {
        write(&mut d, &mut m, 0xfff18500 + i * 4, 0x4000 + i * 0x4000);
    }
    for (o, v) in [
        (0x524, 0x200),
        (0x88c, 0),
        (0x8f4, 1),
        (0x8a0, 0x80003000u32),
        (0x8f8, 0),
        (0x8c4, 2),
        (0x8f8, 0x10000000),
        (0x884, 1),
        (4, 0x33173317),
    ] {
        write(&mut d, &mut m, 0xfff18000 + o, v as u64);
    }
    if hist {
        for (o, v) in [
            (0x604, (width - 1) as u32),
            (0x608, 0x10000 | (height - 1) as u32),
            (0x700, 0xa000),
            (0x704, 0x200),
            (0x984, 1),
            (0x9f8, 0x10000000),
            (8, 1),
        ] {
            write(&mut d, &mut m, 0xfff18000 + o, v as u64);
        }
    }
    advance(&mut d, &mut m, 2);
    (d, frames, images, routes, m)
}
fn producer_with_raster_banks() -> (
    Block18000,
    StatisticsFrames,
    ImageFrames,
    IrqRoutes,
    Memory,
    ImageLayout,
    Vec<u8>,
) {
    let layout = ImageLayout {
        width: 64,
        height: 48,
        stride: 128,
    };
    let pixels: Vec<u8> = (0..layout.bytes().unwrap())
        .map(|i| ((i * 47 + i / 7 * 13) % 256) as u8)
        .collect();
    let images = ImageFrames::from_pixels(layout, pixels.clone()).unwrap();
    let frames = StatisticsFrames::new(vec![StatisticsSpan {
        destination: 4,
        offset: 0,
        bytes: vec![3; 4],
    }])
    .unwrap();
    let routes = routes(0x2e);
    let mut d = Block18000::new(
        fujimod_emulator_imaging::exr2_isp::options::producer::Options {
            pixels_per_tick: None,
            contrast: Some(
                fujimod_emulator_imaging::exr2_isp::contrast::Arithmetic::SquaredHorizontalGradients { luma_bits: 8 },
            ),
            metering: Some(fujimod_emulator_imaging::exr2_isp::options::metering::Arithmetic::RgbCellMeans12),
            clock: fujimod_emulator_imaging::exr2_isp::dma::Clock::Fixed { ticks: 5 },
            ack_ticks: 2,
            stop_ticks: 3,
            histogram: false,
            external_statistics: false,
            diagnostic_reads: true,
        },
        producer::Wiring {
            grid: None,
            frames: frames.clone(),
            images: images.clone(),
            routes: routes.clone(),
        },
    )
    .unwrap();
    let mut m = Memory::default();
    for i in 0..9 {
        write(&mut d, &mut m, 0xfff18500 + i * 4, 0x4000 + i * 0x4000);
    }
    for (offset, value) in [
        (0x100, layout.width as u32 - 1),
        (0x104, layout.height as u32 - 1),
        (0x200, layout.width as u32 - 1),
        (0x204, layout.height as u32 - 1),
        (0x88c, 0),
        (0x8f4, 1),
        (0x8a0, 0x80003000),
        (0x8f8, 0),
        (0x8c4, 2),
        (0x8f8, 0x10000000),
        (0x884, 1),
        (4, 0x33173317),
    ] {
        write(&mut d, &mut m, 0xfff18000 + offset, value as u64);
    }
    advance(&mut d, &mut m, 2);
    (d, frames, images, routes, m, layout, pixels)
}
#[test]
fn statistics_dma_histogram_and_paired_ack_backpressure() {
    let (mut d, frames, images, _, mut m) = producer(true);
    images
        .publish(FrameEdge { index: 1, tick: 10 }, &[5; 16])
        .unwrap();
    frames.publish(FrameEdge { index: 1, tick: 10 }).unwrap();
    advance(&mut d, &mut m, 10);
    write(&mut d, &mut m, 0xfff18500, 0xb000);
    frames.update(0, &[9; 4]).unwrap();
    advance(&mut d, &mut m, 14);
    assert!(m.effects.is_empty());
    advance(&mut d, &mut m, 15);
    assert_eq!(
        m.effects,
        [
            Effect::Dma(0x4000, 4),
            Effect::Dma(0x8000, 4),
            Effect::Dma(0xa000, 1024),
            Effect::Irq(0, 0x2e)
        ]
    );
    assert_eq!(&m.bytes[0x4000..0x4004], &[3; 4]);
    assert_eq!(
        u32::from_le_bytes(m.bytes[0xa014..0xa018].try_into().unwrap()),
        4
    );
    assert_eq!(
        m.bytes[0xa000..0xa400]
            .iter()
            .map(|v| *v as u32)
            .sum::<u32>(),
        4
    );
    assert_eq!(d.next_deadline(15), None);
    frames.publish(FrameEdge { index: 2, tick: 16 }).unwrap();
    advance(&mut d, &mut m, 16);
    assert_eq!(d.next_deadline(16), None);
    reject(&mut d, &mut m, 0xfff189f4, 1);
    for ch in 0..2 {
        let clear = 0xfff1888c + ch * 0x100;
        let flag = 0xfff188f4 + ch * 0x100;
        write(&mut d, &mut m, clear, 0);
        write(&mut d, &mut m, flag, 1);
        assert_eq!(read(&mut d, &mut m, clear), 0);
        assert_eq!(read(&mut d, &mut m, flag), 1);
        let now = m.now + 2;
        advance(&mut d, &mut m, now);
        assert_eq!(read(&mut d, &mut m, clear), 0);
        assert_eq!(read(&mut d, &mut m, flag), 0);
    }
    images
        .publish(FrameEdge { index: 3, tick: 30 }, &[5; 16])
        .unwrap();
    frames.publish(FrameEdge { index: 3, tick: 30 }).unwrap();
    advance(&mut d, &mut m, 30);
    advance(&mut d, &mut m, 35);
    assert_eq!(&m.bytes[0xb000..0xb004], &[9; 4]);
}

#[test]
fn switching_from_sensor_statistics_restores_the_external_span_layout() {
    use fujimod_emulator_image_formation::exposure::ExposedFrame;
    use fujimod_emulator_imaging::exposure::{Calibration, Exposure};
    use fujimod_emulator_imaging::scene::{Frame, FramePixels};
    let (mut d, frames, images, _, mut m) = producer(false);
    let edge = FrameEdge { index: 1, tick: 10 };
    images.publish(edge, &[5; 16]).unwrap();
    images.0.lock().unwrap().radiance = Some(std::sync::Arc::new(
        ExposedFrame::from_frame(
            std::sync::Arc::new(Frame::new(1, 1, FramePixels::Rgb24(vec![128; 3].into())).unwrap()),
            Exposure {
                seconds: 0.001,
                analog_gain: 1.,
            },
            8.,
            Calibration {
                full_scale: 16380,
                reference_white: [256; 3],
                saturation_lux_seconds: 1.,
                color_sample_weights: [2, 5, 2],
            },
        )
        .unwrap(),
    ));
    frames.publish(edge).unwrap();
    advance(&mut d, &mut m, 10);
    advance(&mut d, &mut m, 15);
    assert!(m.effects.contains(&Effect::Dma(0xc000, 1024)));
    for ch in 0..2 {
        write(&mut d, &mut m, 0xfff1888c + ch * 0x100, 0);
        write(&mut d, &mut m, 0xfff188f4 + ch * 0x100, 1);
        let now = m.now + 2;
        advance(&mut d, &mut m, now);
        assert_eq!(read(&mut d, &mut m, 0xfff1888c + ch * 0x100), 0);
        assert_eq!(read(&mut d, &mut m, 0xfff188f4 + ch * 0x100), 0);
    }
    images.0.lock().unwrap().radiance = None;
    let edge = FrameEdge { index: 2, tick: 20 };
    images.publish(edge, &[7; 16]).unwrap();
    frames.update(0, &[9; 4]).unwrap();
    frames.publish(edge).unwrap();
    advance(&mut d, &mut m, 20);
    m.effects.clear();
    advance(&mut d, &mut m, 25);
    assert_eq!(
        m.effects,
        [
            Effect::Dma(0x4000, 4),
            Effect::Dma(0x8000, 4),
            Effect::Irq(0, 0x2e)
        ]
    );
    assert_eq!(&m.bytes[0x4000..0x4004], &[9; 4]);
}

#[test]
fn external_statistics_preserve_a_span_overlapping_the_histogram_destination() {
    let images = ImageFrames::from_pixels(
        ImageLayout {
            width: 4,
            height: 2,
            stride: 8,
        },
        vec![5; 16],
    )
    .unwrap();
    let spans = vec![
        StatisticsSpan {
            destination: 0,
            offset: 0,
            bytes: vec![3; 4],
        },
        StatisticsSpan {
            destination: 8,
            offset: 512,
            bytes: vec![0x5a; 1024],
        },
    ];
    let frames = StatisticsFrames::new(spans).unwrap();
    let mut d = Block18000::new(
        fujimod_emulator_imaging::exr2_isp::options::producer::Options {
            pixels_per_tick: None,
            contrast: None,
            metering: None,
            clock: fujimod_emulator_imaging::exr2_isp::dma::Clock::Fixed { ticks: 5 },
            ack_ticks: 2,
            stop_ticks: 3,
            histogram: true,
            external_statistics: true,
            diagnostic_reads: true,
        },
        producer::Wiring {
            grid: None,
            frames: frames.clone(),
            images: images.clone(),
            routes: routes(0x2e),
        },
    )
    .unwrap();
    let mut m = Memory::default();
    for index in 0..9 {
        write(
            &mut d,
            &mut m,
            0xfff18500 + index * 4,
            0x4000 + index * 0x4000,
        );
    }
    write(&mut d, &mut m, 0xfff18520, 0xa000);
    for (offset, value) in [
        (0x524, 0x200),
        (0x88c, 0),
        (0x8f4, 1),
        (0x8a0, 0x80003000),
        (0x8f8, 0),
        (0x8c4, 2),
        (0x8f8, 0x10000000),
        (0x884, 1),
        (4, 0x33173317),
        (0x604, 3),
        (0x608, 0x10001),
        (0x700, 0xa000),
        (0x704, 0x200),
        (0x984, 1),
        (0x9f8, 0x10000000),
        (8, 1),
    ] {
        write(&mut d, &mut m, 0xfff18000 + offset, value);
    }
    advance(&mut d, &mut m, 2);
    let edge = FrameEdge { index: 1, tick: 10 };
    images.publish(edge, &[5; 16]).unwrap();
    frames.publish(edge).unwrap();
    advance(&mut d, &mut m, 10);
    advance(&mut d, &mut m, 15);

    assert!(m.effects.contains(&Effect::Dma(0xa200, 1024)));
    assert!(!m.effects.contains(&Effect::Dma(0xa000, 1024)));
    assert_eq!(&m.bytes[0xa200..0xa600], &[0x5a; 1024]);
}

#[test]
fn histogram_samples_the_guest_window_and_latches_pixels_before_dma() {
    // A guest-window workload reads the programmed crop and count. No pixel
    // fixture or supplied histogram bins authorize hardware completion.
    let (mut d, frames, images, _, mut m) = producer_with_layout(true, 2496, 853);
    let pixels = vec![77; 2496 * 853 * 2];
    write(&mut d, &mut m, 0xfff18604, 0x0024095f);
    write(&mut d, &mut m, 0xfff18608, 0x0028031f);
    let edge = FrameEdge { index: 1, tick: 10 };
    images.publish(edge, &pixels).unwrap();
    frames.publish(edge).unwrap();
    advance(&mut d, &mut m, 10);
    write(&mut d, &mut m, 0xfff18700, 0xb000);
    let state = d.save();
    assert_eq!(state["registers"][0x700 / 4], 0xb000);
    assert_eq!(state["histogram_destination"], 0xa000);
    assert_eq!(state["pending_due"], 15);
    assert_eq!(state, d.save());
    assert!(m.effects.is_empty());
    write(&mut d, &mut m, 0xfff18604, 0x000c098f);
    write(&mut d, &mut m, 0xfff18608, 0x00200328);
    images
        .publish(FrameEdge { index: 2, tick: 11 }, &vec![29; pixels.len()])
        .unwrap();
    advance(&mut d, &mut m, 15);
    let bins: Vec<u32> = m.bytes[0xa000..0xa400]
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    assert_eq!(bins[77], 2400 * 800 / 2);
    assert_eq!(bins.iter().sum::<u32>(), 2400 * 800 / 2);
    assert_eq!(m.effects.last(), Some(&Effect::Irq(0, 0x2e)));
    for ch in 0..2 {
        write(&mut d, &mut m, 0xfff1888c + ch * 0x100, 0);
        write(&mut d, &mut m, 0xfff188f4 + ch * 0x100, 1);
        let now = m.now + 2;
        advance(&mut d, &mut m, now);
        read(&mut d, &mut m, 0xfff1888c + ch * 0x100);
        read(&mut d, &mut m, 0xfff188f4 + ch * 0x100);
    }
    let edge = FrameEdge { index: 3, tick: 30 };
    images.publish(edge, &vec![29; pixels.len()]).unwrap();
    frames.publish(edge).unwrap();
    advance(&mut d, &mut m, 30);
    advance(&mut d, &mut m, 35);
    let bins: Vec<u32> = m.bytes[0xb000..0xb400]
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    assert_eq!(bins[29], 2448 * 809 / 2);
    assert_eq!(bins.iter().sum::<u32>(), 2448 * 809 / 2);
    reject(&mut d, &mut m, 0xfff18704, 0x201);
}

#[test]
fn histogram_crop_clips_present_samples_without_resizing_or_padding() {
    use fujimod_emulator_image_processing::analysis::histogram_luma;
    let layout = ImageLayout {
        width: 4,
        height: 2,
        stride: 12,
    };
    let pixels = [
        10, 20, 0, 0, 30, 40, 0, 0, 255, 255, 255, 255, 50, 60, 0, 0, 70, 80, 0, 0, 255, 255, 255,
        255,
    ];
    let mut bytes = [0; 1024];
    histogram_luma(
        &pixels,
        layout,
        [layout.width, layout.height],
        [1, 0, 5, 3],
        &mut bytes,
    )
    .unwrap();
    let bins: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    for luma in [20, 40, 60, 80] {
        assert_eq!(bins[luma], 1);
    }
    assert_eq!(bins.iter().sum::<u32>(), 4);
    histogram_luma(
        &pixels,
        layout,
        [layout.width, layout.height],
        [4, 0, 2, 2],
        &mut bytes,
    )
    .unwrap();
    assert_eq!(bytes, [0; 1024]);
}
#[test]
fn statistics_stop_masks_alias_overlap_and_route_withdrawal() {
    let (mut d, frames, _, routes, mut m) = producer(false);
    for (o, v) in [
        (0x600, 3),
        (0x308, 6),
        (0x524, 0),
        (0x524, 0x300),
        (0x108, 0x200),
    ] {
        reject(&mut d, &mut m, 0xfff18000 + o, v);
    }
    write(&mut d, &mut m, 0xfff28000, 0);
    advance(&mut d, &mut m, 4);
    assert_eq!(read(&mut d, &mut m, 0xfff28000), 0);
    advance(&mut d, &mut m, 5);
    assert_eq!(read(&mut d, &mut m, 0xfff28000), 0x10000000);
    frames.publish(FrameEdge { index: 1, tick: 10 }).unwrap();
    advance(&mut d, &mut m, 10);
    routes.set(0x2e, None).unwrap();
    advance(&mut d, &mut m, 15);
    assert_eq!(
        m.effects,
        [
            Effect::Dma(0x4000, 4),
            Effect::Dma(0x8000, 4),
            Effect::Irq(0, 0x2e)
        ]
    );
    for ch in 0..2 {
        write(&mut d, &mut m, 0xfff1888c + ch * 0x100, 0);
        write(&mut d, &mut m, 0xfff188f4 + ch * 0x100, 1);
        let now = m.now + 2;
        advance(&mut d, &mut m, now);
        assert_eq!(read(&mut d, &mut m, 0xfff1888c + ch * 0x100), 0);
        assert_eq!(read(&mut d, &mut m, 0xfff188f4 + ch * 0x100), 0);
    }
    m.effects.clear();
    routes.set(0x2e, Some(0)).unwrap();
    write(&mut d, &mut m, 0xfff18504, 0x40004000);
    frames.publish(FrameEdge { index: 2, tick: 20 }).unwrap();
    m.now = 20;
    let mut cx = Cx::new(&mut m, 0, 0, 0);
    d.advance(&mut cx);
    assert!(cx.refusal().unwrap().contains("overlap"));
    assert!(m.effects.is_empty());
}

#[test]
fn pixel_processing_precedes_statistics_dma_and_latches_the_exposure() {
    let (mut d, frames, images, _, mut m) = producer_with_clock(true, 4, 2, Some(2));
    let edge = FrameEdge { index: 1, tick: 10 };
    images.publish(edge, &[5; 16]).unwrap();
    frames.publish(edge).unwrap();
    advance(&mut d, &mut m, 10);
    // Eight pixels / two per tick, followed by five DMA ticks.
    assert_eq!(d.next_deadline(10), Some(19));
    images
        .publish(FrameEdge { index: 2, tick: 11 }, &[9; 16])
        .unwrap();
    advance(&mut d, &mut m, 18);
    assert!(m.effects.is_empty());
    advance(&mut d, &mut m, 19);
    assert_eq!(m.effects.last(), Some(&Effect::Irq(0, 0x2e)));
    assert_eq!(
        u32::from_le_bytes(m.bytes[0xa014..0xa018].try_into().unwrap()),
        4
    );
    assert_eq!(
        u32::from_le_bytes(m.bytes[0xa024..0xa028].try_into().unwrap()),
        0
    );
}

#[test]
fn radiometric_and_packed_metering_refuse_competing_bank_sources() {
    use fujimod_emulator_image_formation::exposure::ExposedFrame;
    use fujimod_emulator_imaging::exposure::{Calibration, Exposure};
    use fujimod_emulator_imaging::scene::{Frame, FramePixels};
    let (mut d, frames, images, _, mut m) = producer_with_metering(
        false,
        4,
        2,
        None,
        Some(fujimod_emulator_imaging::exr2_isp::options::metering::Arithmetic::RgbCellMeans12),
    );
    let edge = FrameEdge { index: 1, tick: 10 };
    images.publish(edge, &[5; 16]).unwrap();
    images.0.lock().unwrap().radiance = Some(std::sync::Arc::new(
        ExposedFrame::from_frame(
            std::sync::Arc::new(Frame::new(1, 1, FramePixels::Rgb24(vec![128; 3].into())).unwrap()),
            Exposure {
                seconds: 0.001,
                analog_gain: 1.,
            },
            8.,
            Calibration {
                full_scale: 16380,
                reference_white: [256; 3],
                saturation_lux_seconds: 1.,
                color_sample_weights: [2, 5, 2],
            },
        )
        .unwrap(),
    ));
    frames.publish(edge).unwrap();
    m.now = edge.tick;
    let mut cx = Cx::new(&mut m, 0, 0, 0);
    d.advance(&mut cx);
    assert!(cx.refusal().unwrap().contains("same statistics bank"));
    assert!(m.effects.is_empty());
}

#[test]
fn histogram_coordinates_follow_the_upstream_extent_instead_of_the_display_raster() {
    use fujimod_emulator_image_processing::analysis::histogram_luma;
    let layout = ImageLayout {
        width: 4,
        height: 4,
        stride: 8,
    };
    let source: Vec<u8> = (0..4)
        .flat_map(|y| {
            [
                10 + y * 20,
                10 + y * 20,
                128,
                128,
                10 + y * 20,
                10 + y * 20,
                128,
                128,
            ]
        })
        .collect();
    let mut bins = [0; 1024];
    histogram_luma(&source, layout, [4, 2], [0, 1, 4, 1], &mut bins).unwrap();
    assert_eq!(
        u32::from_le_bytes(bins[70 * 4..70 * 4 + 4].try_into().unwrap()),
        2
    );
    assert_eq!(
        u32::from_le_bytes(bins[30 * 4..30 * 4 + 4].try_into().unwrap()),
        0
    );
    histogram_luma(&source, layout, [4, 2], [0, 2, 4, 1], &mut bins).unwrap();
    assert!(bins.iter().all(|v| *v == 0));
}

#[test]
fn raster_worker_result_is_frozen_in_fingerprint_and_checkpoint_until_due() {
    fujimod_emulator_devices::work::configure(2).unwrap();
    let (mut d, frames, images, _, mut memory, layout, pixels) = producer_with_raster_banks();
    let edge = FrameEdge { index: 1, tick: 10 };
    images.publish(edge, &pixels).unwrap();
    frames.publish(edge).unwrap();
    advance(&mut d, &mut memory, 10);
    assert_eq!(d.next_deadline(10), Some(15));

    let mut registers = [0; 1024];
    registers[0x100 / 4] = layout.width as u32 - 1;
    registers[0x104 / 4] = layout.height as u32 - 1;
    registers[0x200 / 4] = layout.width as u32 - 1;
    registers[0x204 / 4] = layout.height as u32 - 1;
    let (expected_contrast, expected_bands) = fujimod_emulator_imaging::exr2_isp::contrast::banks(
        &registers,
        &pixels,
        layout,
        &fujimod_emulator_imaging::exr2_isp::contrast::Arithmetic::SquaredHorizontalGradients {
            luma_bits: 8,
        },
    )
    .unwrap();
    let expected_metering = crate::metering::banks(&registers, &pixels, layout).unwrap();

    let before = d.fingerprint().unwrap();
    let saved = d.checkpoint().unwrap();
    assert_eq!(d.fingerprint().unwrap(), before);
    let (mut restored, _, _, _, mut restored_memory, _, _) = producer_with_raster_banks();
    restored_memory.now = 10;
    restored.restore_checkpoint(&saved).unwrap();
    assert_eq!(restored.fingerprint().unwrap(), before);

    let next_edge = FrameEdge { index: 2, tick: 11 };
    images
        .publish(next_edge, &vec![0x91; pixels.len()])
        .unwrap();
    frames.publish(next_edge).unwrap();
    advance(&mut d, &mut memory, 15);
    advance(&mut restored, &mut restored_memory, 15);
    assert_eq!(&memory.bytes[0x4000..0x4000 + 2048], expected_contrast);
    assert_eq!(&memory.bytes[0x8000..0x8000 + 80], expected_bands);
    assert_eq!(&memory.bytes[0xc000..0xc000 + 3072], expected_metering);
    assert_eq!(
        &restored_memory.bytes[0x4000..0x4000 + 2048],
        expected_contrast
    );
    assert_eq!(&restored_memory.bytes[0x8000..0x8000 + 80], expected_bands);
    assert_eq!(
        &restored_memory.bytes[0xc000..0xc000 + 3072],
        expected_metering
    );
    assert_eq!(memory.effects, restored_memory.effects);
    fujimod_emulator_devices::work::configure(0).unwrap();
}

#[test]
fn raster_worker_finishes_while_masked_but_discards_stopped_transfers() {
    fujimod_emulator_devices::work::configure(2).unwrap();
    let (mut d, frames, images, routes, mut memory, _, pixels) = producer_with_raster_banks();
    let edge = FrameEdge { index: 1, tick: 10 };
    images.publish(edge, &pixels).unwrap();
    frames.publish(edge).unwrap();
    advance(&mut d, &mut memory, 10);
    routes.set(0x2e, None).unwrap();
    advance(&mut d, &mut memory, 15);
    let (mut unmasked, frames, images, _, mut expected, _, pixels) = producer_with_raster_banks();
    images.publish(edge, &pixels).unwrap();
    frames.publish(edge).unwrap();
    advance(&mut unmasked, &mut expected, 10);
    advance(&mut unmasked, &mut expected, 15);
    assert_eq!(memory.bytes, expected.bytes);
    assert_eq!(memory.effects, expected.effects);
    assert_eq!(memory.effects.last(), Some(&Effect::Irq(0, 0x2e)));
    assert_eq!(d.save()["pending_due"], serde_json::Value::Null);
    assert_eq!(
        d.fingerprint().unwrap()["raster_result"],
        serde_json::Value::Null
    );

    let (mut stopped, frames, images, _, mut stopped_memory, _, pixels) =
        producer_with_raster_banks();
    images.publish(edge, &pixels).unwrap();
    frames.publish(edge).unwrap();
    advance(&mut stopped, &mut stopped_memory, 10);
    write(&mut stopped, &mut stopped_memory, 0xfff28000, 0);
    advance(&mut stopped, &mut stopped_memory, 15);
    assert!(stopped_memory.effects.is_empty());
    assert_eq!(stopped.save()["pending_due"], serde_json::Value::Null);
    assert_eq!(
        stopped.fingerprint().unwrap()["raster_result"],
        serde_json::Value::Null
    );
    fujimod_emulator_devices::work::configure(0).unwrap();
}
