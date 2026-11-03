//! Channel 10: a one-shot countdown returns four ticks at divisor 128 and
//! stops at zero until the channel is disabled. No IRQ is wired.
use super::*;
use crate::test_host::{TestHost, advance, read, write};

fn unit(one_shot_channels: Vec<u8>, timer_channels: Vec<u8>) -> PulseUnit {
    PulseUnit::new(
        Options {
            counter_channels: vec![],
            timer_channels,
            one_shot_channels,
            counter_readback_channels: vec![],
            quadrature: false,
            initial_phase: 0,
            capture_channel: None,
        },
        Wiring { irqs: [None; 12] },
    )
    .unwrap()
}

fn configured(period: u32, reload: u32) -> (PulseUnit, TestHost) {
    let mut device = unit(vec![10], vec![]);
    let mut host = TestHost::new();
    host.now = 100;
    for (offset, value) in [(0, 0xd003), (4, 0), (8, period), (12, reload), (16, 0)] {
        write(
            &mut device,
            &mut host,
            CHANNEL_BASE + 10 * 0x100 + offset,
            value,
        );
    }
    (device, host)
}

#[test]
fn channel10_one_shot_counts_down_at_its_divisor_and_stays_expired() {
    let (mut device, mut host) = configured(4, 0);
    write(&mut device, &mut host, BASE, 0x0400_0400);
    let channel = CHANNEL_BASE + 10 * 0x100 + 8;
    assert_eq!(read(&mut device, &mut host, channel), 4);
    advance(&mut device, &mut host, 100 + 128 * 2);
    assert_eq!(read(&mut device, &mut host, channel), 2);
    advance(&mut device, &mut host, 100 + 128 * 4);
    assert_eq!(read(&mut device, &mut host, channel), 0);
    advance(&mut device, &mut host, 100 + 128 * 10);
    assert_eq!(read(&mut device, &mut host, channel), 0);
    assert!(host.irqs.is_empty());
    // The countdown is one shot: no deadline remains until it is re-armed.
    assert_eq!(device.next_deadline(host.now), None);
}

#[test]
fn one_shot_channel_refuses_a_reload_before_enable() {
    let (mut device, mut host) = configured(4, 3);
    let before = device.fingerprint().unwrap();
    let mut cx = host.cx();
    device.write(&mut cx, BASE, 4, 0x0400_0400);
    assert_eq!(
        cx.refusal(),
        Some("incomplete one-shot timer configuration")
    );
    assert_eq!(device.fingerprint().unwrap(), before);
    assert!(host.irqs.is_empty());
}

#[test]
fn one_shot_channels_cannot_overlap_timers_or_repeat() {
    for channels in [vec![10, 10], vec![12]] {
        assert!(
            PulseUnit::new(
                Options {
                    counter_channels: vec![],
                    timer_channels: vec![],
                    one_shot_channels: channels,
                    counter_readback_channels: vec![],
                    quadrature: false,
                    initial_phase: 0,
                    capture_channel: None,
                },
                Wiring { irqs: [None; 12] },
            )
            .is_err()
        );
    }
    assert!(
        PulseUnit::new(
            Options {
                counter_channels: vec![],
                timer_channels: vec![10],
                one_shot_channels: vec![10],
                counter_readback_channels: vec![],
                quadrature: false,
                initial_phase: 0,
                capture_channel: None,
            },
            Wiring {
                irqs: [Some(InterruptLine::Core { core: 0, line: 30 }); 12]
            },
        )
        .is_err()
    );
    assert!(unit(vec![10], vec![]).pins.one_shots == 1 << 10);
}

#[test]
fn configurations_without_one_shot_channels_still_deserialize() {
    let options: Options = serde_json::from_str(
        r#"{"counter_channels":[0,4],"timer_channels":[],"quadrature":false,
            "initial_phase":0,"capture_channel":null}"#,
    )
    .unwrap();
    assert!(options.one_shot_channels.is_empty());
    let device = PulseUnit::new(options, Wiring { irqs: [None; 12] }).unwrap();
    assert_eq!(device.pins.one_shots, 0);
}
