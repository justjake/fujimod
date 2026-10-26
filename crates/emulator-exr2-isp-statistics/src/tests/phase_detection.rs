use fujimod_emulator_devices::exr2::irq_routes::IrqRoutes;
use crate::*;
use fujimod_emulator_exr2_isp_testing::*;
use fujimod_emulator_devices::Cx;
use fujimod_emulator_devices::Device;
fn phase(arithmetic: fujimod_emulator_imaging::exr2_isp::options::phase_detection::Arithmetic) -> (PhaseDetection, Memory, IrqRoutes) {
    let routes = routes(0x33);
    let d = PhaseDetection::new(
        fujimod_emulator_imaging::exr2_isp::options::phase_detection::Options {
            arithmetic,
            data_ticks: 10,
            stop_ticks: 2,
        },
        phase_detection::Wiring {
            routes: routes.clone(),
        },
    )
    .unwrap();
    (d, Memory::default(), routes)
}
fn phase_config(d: &mut PhaseDetection, m: &mut Memory, packed: bool) {
    let base = 0xfff24000;
    for (o, v) in [
        (4, 0x80),
        (8, 0x4000),
        (0xc, if packed { 0x4000 } else { 0x10000 }),
        (0x14, if packed { 192 } else { 96 }),
        (0x18, 0),
        (0x1c, 0x60000 | if packed { 190 } else { 95 }),
        (0x28, 96),
        (0x2c, (20 << 24) | if packed { 0x100001 } else { 0 }),
    ] {
        write(d, m, base + o, v);
    }
}
#[test]
fn paired_phase_sad_result_widths_deadline_and_cleanup() {
    let (mut d, mut m, _) = phase(fujimod_emulator_imaging::exr2_isp::options::phase_detection::Arithmetic::PairedU16Sad);
    phase_config(&mut d, &mut m, false);
    for y in 0..8 {
        for x in 0usize..96 {
            let a = (1000 + x * 17 + y * 9) as u16;
            let b = (1000 + x.saturating_sub(3) * 17 + y * 9) as u16;
            let i = 0x4000 + (y * 96 + x) * 2;
            m.bytes[i..i + 2].copy_from_slice(&a.to_le_bytes());
            let i = 0x10000 + (y * 96 + x) * 2;
            m.bytes[i..i + 2].copy_from_slice(&b.to_le_bytes());
        }
    }
    write(&mut d, &mut m, 0xfff24004, 0x81);
    assert_eq!(d.next_deadline(0), Some(10));
    reject(&mut d, &mut m, 0xfff24014, 96);
    advance(&mut d, &mut m, 9);
    assert!(m.effects.is_empty());
    m.bytes[0x4000..0x8000].fill(0);
    advance(&mut d, &mut m, 10);
    assert_eq!(m.effects, [Effect::Irq(0, 0x33)]);
    let curve: Vec<_> = (10..=30)
        .map(|i| read(&mut d, &mut m, 0xfff25004 + i * 4))
        .collect();
    assert_eq!(
        curve.iter().enumerate().min_by_key(|(_, v)| *v).unwrap().0 as i32 - 10,
        3
    );
    assert!(read(&mut d, &mut m, 0xfff240b4) > 0);
    write(&mut d, &mut m, 0xfff24000, 0);
    write(&mut d, &mut m, 0xfff24004, 0x81);
    advance(&mut d, &mut m, 11);
    assert_eq!(read(&mut d, &mut m, 0xfff24000), 0);
    advance(&mut d, &mut m, 12);
    assert_eq!(read(&mut d, &mut m, 0xfff24000), 1);
    write(&mut d, &mut m, 0xfff24000, 0);
    advance(&mut d, &mut m, 14);
    assert_eq!(read(&mut d, &mut m, 0xfff24000), 1);
    assert_eq!(m.effects.len(), 1);
}
#[test]
fn packed12_aperture_and_unselected_arithmetic_refusal() {
    let (mut d, mut m, _) = phase(fujimod_emulator_imaging::exr2_isp::options::phase_detection::Arithmetic::ApertureSad);
    phase_config(&mut d, &mut m, true);
    for y in 0..8 {
        for x in 0usize..96 {
            let a = 500 + x * 9;
            let b = 500 + x.saturating_sub(3) * 9;
            let i = 0x4000 + y * 288 + x * 3;
            m.bytes[i] = a as u8;
            m.bytes[i + 1] = ((a >> 8) & 15) as u8 | ((b & 15) << 4) as u8;
            m.bytes[i + 2] = (b >> 4) as u8;
        }
    }
    write(&mut d, &mut m, 0xfff24004, 0x20000081);
    advance(&mut d, &mut m, 10);
    let curve: Vec<_> = (0..41)
        .map(|i| read(&mut d, &mut m, 0xfff25004 + i * 4))
        .collect();
    assert_eq!(
        curve.iter().enumerate().min_by_key(|(_, v)| *v).unwrap().0 as i32 - 20,
        3
    );
    let decoded = fujimod_emulator_image_processing::raw::RawView {
        bytes: &[0xbc, 0x3a, 0x12],
        width: 2,
        height: 1,
        pitch: 3,
        packing: fujimod_emulator_image_processing::raw::Packing::Packed12,
    };
    decoded.validate().unwrap();
    assert_eq!(decoded.phase_sample(0, 0), 0x2af0);
    assert_eq!(decoded.phase_sample(1, 0), 0x48c);
    let (mut d, mut m, _) = phase(fujimod_emulator_imaging::exr2_isp::options::phase_detection::Arithmetic::Disabled);
    phase_config(&mut d, &mut m, true);
    reject(&mut d, &mut m, 0xfff24004, 0x81);
    assert_eq!(d.next_deadline(0), None);
}
#[test]
fn phase_refuses_invalid_samples_and_route_changes_without_completion() {
    let (mut d, mut m, routes) = phase(fujimod_emulator_imaging::exr2_isp::options::phase_detection::Arithmetic::PairedU16Sad);
    phase_config(&mut d, &mut m, false);
    m.bytes[0x4000..0x4002].copy_from_slice(&0x4000u16.to_le_bytes());
    reject(&mut d, &mut m, 0xfff24004, 0x81);
    m.bytes[0x4000..0x4002].fill(0);
    write(&mut d, &mut m, 0xfff24004, 0x81);
    routes.set(0x33, Some(1)).unwrap();
    m.now = 10;
    let mut cx = Cx::new(&mut m, 0, 0, 0);
    d.advance(&mut cx);
    assert!(cx.refusal().unwrap().contains("owner changed"));
    assert!(m.effects.is_empty());
}
