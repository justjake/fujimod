//! Channel 11: a 600-microsecond period uses 14400 ticks and IRQ31.
use super::*;
use crate::test_host::{TestHost, advance, write};
fn configured(core: usize, control: u32, period: u32) -> (PulseUnit, TestHost) {
    let mut irqs = [None; 12];
    irqs[11] = Some(InterruptLine::Core { core, line: 31 });
    let mut device = PulseUnit::new(
        Options {
            counter_channels: vec![],
            timer_channels: vec![11],
            quadrature: false,
            initial_phase: 0,
            capture_channel: None,
        },
        Wiring { irqs },
    )
    .unwrap();
    let mut host = TestHost::new();
    host.now = 100;
    for (offset, value) in [(0, control), (4, 0), (8, period), (12, period - 1), (16, 0)] {
        write(
            &mut device,
            &mut host,
            CHANNEL_BASE + 11 * 0x100 + offset,
            value,
        );
    }
    (device, host)
}
#[test]
fn channel11_deadline_physical_core_route_cancellation_and_rearm() {
    for core in 0..2 {
        let (mut device, mut host) = configured(core, 0xc000, 6000);
        write(&mut device, &mut host, BASE, 0x08000800);
        let before = device.fingerprint().unwrap();
        for now in [100, 101, 6099] {
            advance(&mut device, &mut host, now);
            assert_eq!(device.fingerprint().unwrap(), before);
            assert_eq!(device.next_deadline(now), Some(6100));
            assert!(host.irqs.is_empty());
        }
        advance(&mut device, &mut host, 6100);
        advance(&mut device, &mut host, 6100);
        assert_eq!(host.irqs, [(core, 31)]);
        assert_eq!(device.next_deadline(6100), None);
        write(&mut device, &mut host, BASE, 0x08000000);
        write(&mut device, &mut host, BASE, 0x08000800);
        assert_eq!(device.next_deadline(6100), Some(12100));
        write(&mut device, &mut host, BASE, 0x08000000);
        advance(&mut device, &mut host, 13000);
        assert_eq!(host.irqs, [(core, 31)]);
        assert_eq!(device.next_deadline(13000), None);
    }
}
#[test]
fn periodic_channel11_phase_is_independent_of_service_frequency() {
    let mut snapshots = vec![];
    for ticks in [vec![48100, 96100, 144100], vec![144100]] {
        let (mut device, mut host) = configured(0, 0x8001, 6000);
        write(&mut device, &mut host, BASE, 0x08000800);
        for tick in ticks {
            advance(&mut device, &mut host, tick);
        }
        assert!(host.irqs.iter().all(|irq| *irq == (0, 31)));
        assert!(!host.irqs.is_empty());
        assert_eq!(device.next_deadline(144100), Some(192100));
        snapshots.push(device.fingerprint().unwrap());
    }
    assert_eq!(snapshots[0], snapshots[1]);
}
#[test]
fn invalid_channel11_configuration_or_absent_irq_refuses_before_enable() {
    let (mut device, mut host) = configured(0, 0xc000, 6000);
    write(&mut device, &mut host, CHANNEL_BASE + 11 * 0x100 + 12, 12);
    let before = device.fingerprint().unwrap();
    let mut cx = host.cx();
    device.write(&mut cx, BASE, 4, 0x08000800);
    assert_eq!(
        cx.refusal(),
        Some("incomplete interval timer configuration or IRQ wiring")
    );
    assert_eq!(device.fingerprint().unwrap(), before);
    assert!(host.irqs.is_empty());
    assert!(
        PulseUnit::new(
            Options {
                counter_channels: vec![],
                timer_channels: vec![11],
                quadrature: false,
                initial_phase: 0,
                capture_channel: None
            },
            Wiring { irqs: [None; 12] }
        )
        .is_err()
    );
}
