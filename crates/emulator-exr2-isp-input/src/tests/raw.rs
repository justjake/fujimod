use fujimod_emulator_devices::exr2::irq_routes::Irq;
use crate::*;
use fujimod_emulator_devices::test_host::*;
use fujimod_emulator_devices::Device;
#[test]
fn prefilter_deadlines_reset_ack_and_busy_refusal() {
    let mut h = TestHost::new();
    let mut d = Prefilter::new(
        fujimod_emulator_imaging::exr2_isp::options::raw::Options {
            completion_ticks: 10,
            reset_ticks: 2,
        },
        Irq { core: 0, line: 49 },
    )
    .unwrap();
    write(&mut d, &mut h, 0xfff1a000, 0);
    advance(&mut d, &mut h, 1);
    assert_eq!(read(&mut d, &mut h, 0xfff1a000), 0);
    advance(&mut d, &mut h, 2);
    assert_eq!(read(&mut d, &mut h, 0xfff1a000), 0x10000);
    for (o, v) in [(4, 0x8000), (8, 1), (12, 0x9000), (16, 1), (0, 1)] {
        write(&mut d, &mut h, 0xfff1a000 + o, v);
    }
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff1a004, 4, 0);
    assert!(cx.refusal().is_some());
    advance(&mut d, &mut h, 11);
    assert!(h.irqs.is_empty());
    advance(&mut d, &mut h, 12);
    assert_eq!(h.irqs, [(0, 49)]);
    assert_eq!(read(&mut d, &mut h, 0xfff1a018), 1);
    write(&mut d, &mut h, 0xfff1a01c, 1);
    assert_eq!(read(&mut d, &mut h, 0xfff1a018), 0);
}
#[test]
fn statistics_latches_only_addressed_roi_and_publishes_at_due() {
    let mut h = TestHost::new();
    for b in h.ram[0x8000..0x8048].chunks_exact_mut(2) {
        b.copy_from_slice(&100u16.to_le_bytes());
    }
    let mut d = RawStatistics::new(
        fujimod_emulator_imaging::exr2_isp::options::raw::Options {
            completion_ticks: 5,
            reset_ticks: 2,
        },
        Irq { core: 1, line: 50 },
    )
    .unwrap();
    for (o, v) in [
        (8, 0x8000),
        (0x14, 6),
        (0x18, 0),
        (0x1c, 5 << 16 | 5),
        (0xcc, 1),
        (4, 2),
    ] {
        write(&mut d, &mut h, 0xfff1c000 + o, v);
    }
    h.ram[0x8000..0x8048].fill(0);
    advance(&mut d, &mut h, 4);
    assert_eq!(read(&mut d, &mut h, 0xfff1c600), 0);
    advance_from_checkpoint(&mut d, &mut h, 5);
    assert_eq!(read(&mut d, &mut h, 0xfff1c600), 100);
    assert_eq!(read(&mut d, &mut h, 0xfff1c720), 1);
    assert_eq!(h.irqs, [(1, 50)]);
    write(&mut d, &mut h, 0xfff1c0c8, 1);
    assert_eq!(read(&mut d, &mut h, 0xfff1c0c8), 0);
}

#[test]
fn both_stock_prefilter_packing_fields_are_passive_configuration() {
    // The input stores 10000|(packing<<8), source and stride;
    // neither packing selector starts processing or creates a completion.
    let mut h = TestHost::new();
    h.ram[0x8000..0x8100].fill(0xa5);
    let mut d = Prefilter::new(
        fujimod_emulator_imaging::exr2_isp::options::raw::Options {
            completion_ticks: 10,
            reset_ticks: 2,
        },
        Irq { core: 0, line: 49 },
    )
    .unwrap();
    for packing in [1, 0] {
        for (o, v) in [(0, 0x10000 | packing << 8), (4, 0x8000), (8, 640)] {
            write(&mut d, &mut h, 0xfff1a000 + o, v);
        }
        assert_eq!(
            read(&mut d, &mut h, 0xfff1a000),
            u64::from(0x10000 | packing << 8)
        );
        assert_eq!(d.next_deadline(0), None);
        let before = d.fingerprint().unwrap();
        advance(&mut d, &mut h, 10000);
        assert_eq!(d.fingerprint().unwrap(), before);
        assert!(h.effects.is_empty());
        assert_eq!(&h.ram[0x8000..0x8100], &[0xa5; 256]);
    }
}
