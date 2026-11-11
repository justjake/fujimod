//! Channel 11: a 600-microsecond period uses 14400 ticks and IRQ31.
use super::*;
use crate::test_host::{TestHost, advance, read, write};
fn configured(core: usize, control: u32, period: u32) -> (PulseUnit, TestHost) {
    let mut irqs = [None; 12];
    irqs[11] = Some(InterruptLine::Core { core, line: 31 });
    let mut device = PulseUnit::new(
        Options {
            counter_channels: vec![],
            timer_channels: vec![11],
            one_shot_channels: vec![],
            counter_readback_channels: vec![],
            free_running_channels: vec![],
            mirrored_irqs: vec![],
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
                one_shot_channels: vec![],
                counter_readback_channels: vec![],
                free_running_channels: vec![],
                mirrored_irqs: vec![],
                quadrature: false,
                initial_phase: 0,
                capture_channel: None
            },
            Wiring { irqs: [None; 12] }
        )
        .is_err()
    );
}
#[test]
fn counter_readback_reports_the_running_down_count_only_when_declared() {
    let mut irqs = [None; 12];
    irqs[7] = Some(InterruptLine::Core { core: 0, line: 27 });
    for readback in [true, false] {
        let mut device = PulseUnit::new(
            Options {
                counter_channels: vec![],
                timer_channels: vec![7],
                one_shot_channels: vec![],
                counter_readback_channels: if readback { vec![7] } else { vec![] },
                free_running_channels: vec![],
                mirrored_irqs: vec![],
                quadrature: false,
                initial_phase: 0,
                capture_channel: None,
            },
            Wiring { irqs },
        )
        .unwrap();
        let mut host = TestHost::new();
        host.now = 100;
        let channel = CHANNEL_BASE + 7 * 0x100;
        for (offset, value) in [(0, 0x9001), (4, 0), (8, 0x7530), (12, 0x752f), (16, 0)] {
            write(&mut device, &mut host, channel + offset, value);
        }
        write(&mut device, &mut host, BASE, 0x0080_0080);
        advance(&mut device, &mut host, 100 + 8 * 10);
        let expected = if readback { 0x7530 - 10 } else { 0x7530 };
        assert_eq!(read(&mut device, &mut host, channel + 8), expected);
    }
}
#[test]
fn mirrored_irq_raises_the_second_core_with_the_primary_output() {
    let mut irqs = [None; 12];
    irqs[11] = Some(InterruptLine::Core { core: 0, line: 31 });
    let options = |mirrored_irqs| Options {
        counter_channels: vec![],
        timer_channels: vec![11],
        one_shot_channels: vec![],
        counter_readback_channels: vec![],
        free_running_channels: vec![],
        mirrored_irqs,
        quadrature: false,
        initial_phase: 0,
        capture_channel: None,
    };
    let mirror = MirroredIrq { channel: 11, core: 1, line: 31 };
    let mut device = PulseUnit::new(options(vec![mirror]), Wiring { irqs }).unwrap();
    let mut host = TestHost::new();
    host.now = 100;
    for (offset, value) in [(0, 0xc000), (4, 0), (8, 6000), (12, 5999), (16, 0)] {
        write(&mut device, &mut host, CHANNEL_BASE + 11 * 0x100 + offset, value);
    }
    write(&mut device, &mut host, BASE, 0x08000800);
    advance(&mut device, &mut host, 6100);
    assert_eq!(host.irqs, [(0, 31), (1, 31)]);
    // A mirror needs a declared interval-timer channel.
    let stray = MirroredIrq { channel: 3, core: 1, line: 23 };
    assert!(PulseUnit::new(options(vec![stray]), Wiring { irqs }).is_err());
    // Existing configurations without the field still deserialize.
    let o: Options = serde_json::from_str(
        r#"{"counter_channels":[],"timer_channels":[11],"quadrature":false,"initial_phase":0,"capture_channel":null}"#,
    )
    .unwrap();
    assert!(o.mirrored_irqs.is_empty());
}
