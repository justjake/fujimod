use fujimod_emulator_devices::exr2::irq_routes::Irq;
use crate::*;
use fujimod_emulator_devices::test_host::*;
use fujimod_emulator_devices::Device;
#[test]
fn scaler_dma_padding_reset_and_unsupported_transform() {
    let mut h = TestHost::new();
    h.ram[0x8000..0x8010].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    h.ram[0x9000..0x9020].fill(0xa5);
    let mut d = Scaler::new(
        fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
            completion_ticks: 4,
        },
        Irq { core: 0, line: 67 },
    )
    .unwrap();
    for (o, v) in [
        (0x10, 0x8000),
        (0x14, 8),
        (0x18, 1 << 16 | 8),
        (0x20, 0x9000),
        (0x24, 16),
        (0x28, 1 << 16 | 8),
        (0x40, 65536),
        (0x54, 65536),
        (0x7c, 0x1000),
        (4, 1),
    ] {
        write(&mut d, &mut h, 0xfff47000 + o, v);
    }
    advance(&mut d, &mut h, 3);
    assert!(h.irqs.is_empty());
    advance(&mut d, &mut h, 4);
    assert_eq!(&h.ram[0x9000..0x9010], &h.ram[0x8000..0x8010]);
    assert_eq!(h.ram[0x9010], 0xa5);
    assert_eq!(read(&mut d, &mut h, 0xfff47074), 1);
    write(&mut d, &mut h, 0xfff47078, 1);
    assert_eq!(read(&mut d, &mut h, 0xfff47074), 0);
    write(&mut d, &mut h, 0xfff47044, 1);
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff47004, 4, 1);
    assert!(cx.refusal().is_some());
}
