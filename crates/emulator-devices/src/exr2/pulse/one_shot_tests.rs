//! Channel 10: a one-shot countdown returns four ticks at divisor 128 and
//! stops at zero until the channel is disabled. A channel declared as both an
//! interval timer and a one-shot chooses the form on each arming: reload zero
//! counts down once, reload period - 1 raises its IRQ and re-arms.
use super::*;
use crate::test_host::{TestHost, advance, read, write};

fn unit(one_shot_channels: Vec<u8>, timer_channels: Vec<u8>) -> PulseUnit {
    PulseUnit::new(
        Options {
            counter_channels: vec![],
            timer_channels,
            one_shot_channels,
            counter_readback_channels: vec![],
            free_running_channels: vec![],
            mirrored_irqs: vec![],
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
fn one_shot_channels_cannot_repeat() {
    for channels in [vec![10, 10], vec![12]] {
        assert!(
            PulseUnit::new(
                Options {
                    counter_channels: vec![],
                    timer_channels: vec![],
                    one_shot_channels: channels,
                    counter_readback_channels: vec![],
                    free_running_channels: vec![],
                    mirrored_irqs: vec![],
                    quadrature: false,
                    initial_phase: 0,
                    capture_channel: None,
                },
                Wiring { irqs: [None; 12] },
            )
            .is_err()
        );
    }
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

#[test]
fn a_channel_can_be_a_polled_one_shot_then_an_interval_timer() {
    // Loader form: control 0xd003, reload zero, a polled countdown at divisor
    // 128. Application form: control 0x8001 with reload period - 1, an
    // interval timer on the same channel that re-arms and raises IRQ30.
    let mut irqs = [None; 12];
    irqs[10] = Some(InterruptLine::Core { core: 0, line: 30 });
    let mut device = PulseUnit::new(
        Options {
            counter_channels: vec![],
            timer_channels: vec![10],
            one_shot_channels: vec![10],
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
    let channel = CHANNEL_BASE + 10 * 0x100;
    for (offset, value) in [(0, 0xd003), (4, 0), (8, 100), (12, 0), (16, 0)] {
        write(&mut device, &mut host, channel + offset, value);
    }
    write(&mut device, &mut host, BASE, 0x0400_0400);
    let once = 100 * DIVISORS[(0xd003 & 3) as usize];
    assert_eq!(read(&mut device, &mut host, channel + 8), 100);
    advance(&mut device, &mut host, 100 + once / 2);
    assert!(read(&mut device, &mut host, channel + 8) < 100);
    advance(&mut device, &mut host, 100 + once);
    assert_eq!(read(&mut device, &mut host, channel + 8), 0);
    // The polled countdown expires once and leaves no deadline behind. The
    // channel's physical line is wired, so that single expiry is the one edge
    // on it; the interval phase below then counts only the edges it raises.
    assert_eq!(device.next_deadline(host.now), None);
    assert_eq!(host.irqs, [(0, 30)]);
    host.irqs.clear();

    write(&mut device, &mut host, BASE, 0x0400_0000);
    for (offset, value) in [(0, 0x8001), (4, 0), (8, 50), (12, 49), (16, 0)] {
        write(&mut device, &mut host, channel + offset, value);
    }
    let interval = 50 * DIVISORS[(0x8001 & 3) as usize];
    write(&mut device, &mut host, BASE, 0x0400_0400);
    let armed = host.now;
    // An interval arming records no start tick, so +08 stays the period.
    assert_eq!(read(&mut device, &mut host, channel + 8), 50);
    assert!(host.irqs.is_empty());
    assert_eq!(device.next_deadline(armed), Some(armed + interval));
    advance(&mut device, &mut host, armed + interval - 1);
    assert!(host.irqs.is_empty());
    advance(&mut device, &mut host, armed + interval);
    assert_eq!(host.irqs, [(0, 30)]);
    // The interval form re-arms instead of staying expired.
    assert_eq!(device.next_deadline(host.now), Some(armed + 2 * interval));
    advance(&mut device, &mut host, armed + 2 * interval - 1);
    assert_eq!(host.irqs, [(0, 30)]);
    advance(&mut device, &mut host, armed + 2 * interval);
    assert_eq!(host.irqs, [(0, 30), (0, 30)]);
}
