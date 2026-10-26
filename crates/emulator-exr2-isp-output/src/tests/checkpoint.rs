use crate::scaler::Scaler;
use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::test_host::{TestHost, advance, write};

#[test]
fn checkpoint_pending_scaler_restores_pixels_and_original_dma_deadline() {
    let mut host = TestHost::new();
    host.ram[0x8000..0x8040].fill(123);
    let mut device = Scaler::new(
        fujimod_emulator_imaging::exr2_isp::options::scaler::Options {
            completion_ticks: 5,
        },
        Irq { core: 0, line: 47 },
    )
    .unwrap();
    for (offset, value) in [
        (0x10, 0x8000),
        (0x14, 8),
        (0x18, 2 << 16 | 8),
        (0x20, 0x9000),
        (0x24, 8),
        (0x28, 2 << 16 | 8),
        (0x40, 1 << 16),
        (0x54, 1 << 16),
        (0x7c, 0x1000),
        (4, 1),
    ] {
        write(&mut device, &mut host, 0xfff47000 + offset, value);
    }
    let saved = device.checkpoint().unwrap();
    let state = device.fingerprint().unwrap();
    advance(&mut device, &mut host, 5);
    let pixels = host.ram[0x9000..0x9020].to_vec();
    host.ram[0x8000..0x8040].fill(0);
    host.ram[0x9000..0x9020].fill(0);
    host.irqs.clear();
    device.restore_checkpoint(&saved).unwrap();
    assert_eq!(device.fingerprint().unwrap(), state);
    advance(&mut device, &mut host, 4);
    assert!(host.irqs.is_empty());
    advance(&mut device, &mut host, 5);
    assert_eq!(&host.ram[0x9000..0x9020], pixels.as_slice());
    assert_eq!(host.irqs, [(0, 47)]);
}
