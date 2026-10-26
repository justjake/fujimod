use fujimod_emulator_devices::exr2::irq_routes::IrqRoutes;
use crate::*;
use fujimod_emulator_exr2_isp_testing::*;
use fujimod_emulator_devices::Cx;
use fujimod_emulator_devices::Device;
fn window() -> (WindowAnalysis, Memory, IrqRoutes) {
    let routes = routes(0x46);
    let d = WindowAnalysis::new(
        fujimod_emulator_imaging::exr2_isp::options::window_analysis::Options {
            arithmetic: fujimod_emulator_imaging::exr2_isp::options::window_analysis::Arithmetic::Y16GradientsAndLocalTracking,
            ticks_per_window: 3,
        },
        window_analysis::Wiring {
            routes: routes.clone(),
        },
    )
    .unwrap();
    (d, Memory::default(), routes)
}
fn window_config(d: &mut WindowAnalysis, m: &mut Memory, kind: u64) {
    for (o, v) in [
        (4, 0x200 | kind),
        (0x10, 0x4000),
        (0x2c, 0x8000),
        (0x48, 0xc000),
        (0x4c, 0x10000),
        (0x50, 0x60006),
        (0x60, 0),
        (0x64, 0x10001),
        (0x68, 0x40004),
        (0x6c, 0),
        (0x9c, 0),
        (0x104, 1),
        (0x184, 0x10001),
        (0x1a8, 32),
        (0x1f8, 0x10000000),
    ] {
        write(d, m, 0xfff42000 + o, v);
    }
}
#[test]
fn window_gradient_dma_status_then_irq_and_cancel() {
    let (mut d, mut m, _) = window();
    window_config(&mut d, &mut m, 1);
    for y in 0..7 {
        for x in 0..7 {
            let v = (x * 17 + y * 13) as u16;
            let i = 0x4000 + (y * 7 + x) * 2;
            m.bytes[i..i + 2].copy_from_slice(&v.to_le_bytes());
        }
    }
    write(&mut d, &mut m, 0xfff42000, 1);
    assert_eq!(d.next_deadline(0), Some(3));
    reject(&mut d, &mut m, 0xfff42050, 0);
    advance(&mut d, &mut m, 2);
    assert!(m.effects.is_empty());
    advance(&mut d, &mut m, 3);
    assert_eq!(m.effects, [Effect::Dma(0xc000, 8), Effect::Irq(0, 0x46)]);
    assert_eq!(read(&mut d, &mut m, 0xfff42000), 0x10000);
    assert_eq!(&m.bytes[0xc000..0xc008], &[1, 0, 1, 0, 26, 0, 0, 0]);
    write(&mut d, &mut m, 0xfff42000, 1);
    write(&mut d, &mut m, 0xfff42000, 0);
    advance(&mut d, &mut m, 6);
    assert_eq!(m.effects.len(), 2);
    assert_eq!(d.next_deadline(6), None);
}
#[test]
fn window_tracking_and_unsupported_geometry() {
    let (mut d, mut m, _) = window();
    window_config(&mut d, &mut m, 2);
    for y in 0..7 {
        for x in 0..7 {
            let v = (x * x * 17 + y * y * 13 + x * y * 3) as u16;
            let i = 0x4000 + (y * 7 + x) * 2;
            m.bytes[i..i + 2].copy_from_slice(&v.to_le_bytes());
        }
    }
    let image = m.bytes[0x4000..0x4000 + 98].to_vec();
    m.bytes[0x8000..0x8000 + 98].copy_from_slice(&image);
    m.bytes[0xc000..0xc008].copy_from_slice(&[3, 0, 3, 0, 100, 0, 0, 0]);
    write(&mut d, &mut m, 0xfff42000, 1);
    advance(&mut d, &mut m, 3);
    assert_eq!(&m.bytes[0x10000..0x10008], &[3, 0, 3, 0, 100, 0, 0, 0]);
    write(&mut d, &mut m, 0xfff42050, 0);
    reject(&mut d, &mut m, 0xfff42000, 1);
}
#[test]
fn window_route_change_refuses_before_result_dma() {
    let (mut d, mut m, routes) = window();
    window_config(&mut d, &mut m, 1);
    write(&mut d, &mut m, 0xfff42000, 1);
    routes.set(0x46, Some(1)).unwrap();
    m.now = 3;
    let mut cx = Cx::new(&mut m, 0, 0, 0);
    d.advance(&mut cx);
    assert!(cx.refusal().unwrap().contains("owner changed"));
    assert!(m.effects.is_empty());
}
#[test]
fn window_masking_preserves_result_dma_status_and_latched_irq() {
    let (mut d, mut m, routes) = window();
    window_config(&mut d, &mut m, 1);
    write(&mut d, &mut m, 0xfff42000, 1);
    routes.set(0x46, None).unwrap();
    advance(&mut d, &mut m, 3);
    assert_eq!(m.effects, [Effect::Dma(0xc000, 8), Effect::Irq(0, 0x46)]);
    assert_eq!(read(&mut d, &mut m, 0xfff42000), 0x10000);
    advance(&mut d, &mut m, 4);
    assert_eq!(m.effects.len(), 2);
}
#[test]
fn maximum_window_count_uses_the_complete_result_extent() {
    let (mut d, mut m, _) = window();
    window_config(&mut d, &mut m, 1);
    for (o, v) in [
        (0x50, (63 << 16) | 31),
        (0x60, (63 << 16) | 31),
        (0x64, 0),
        (0x68, 0),
        (0x6c, 0),
        (0x9c, 2047),
        (0x1a8, 2048 * 8 * 4),
    ] {
        write(&mut d, &mut m, 0xfff42000 + o, v);
    }
    write(&mut d, &mut m, 0xfff42000, 1);
    advance(&mut d, &mut m, 6143);
    assert!(m.effects.is_empty());
    advance(&mut d, &mut m, 6144);
    assert_eq!(
        m.effects,
        [Effect::Dma(0xc000, 16384), Effect::Irq(0, 0x46)]
    );
    assert_eq!(
        &m.bytes[0xc000 + 16384 - 8..0xc000 + 16384],
        &[63, 0, 31, 0, 0, 0, 0, 0]
    );
}
