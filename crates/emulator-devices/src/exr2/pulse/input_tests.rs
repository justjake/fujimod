//! Capture period measurement and running quadrature counter updates.
use super::*;
use crate::test_host::{TestHost, read, write};

fn unit(quadrature: bool) -> PulseUnit {
    let mut irqs = [None; 12];
    irqs[1] = Some(InterruptLine::Core { core: 0, line: 21 });
    PulseUnit::new(
        Options {
            counter_channels: vec![],
            timer_channels: vec![],
            quadrature,
            initial_phase: 0,
            capture_channel: (!quadrature).then_some(1),
        },
        Wiring { irqs },
    )
    .unwrap()
}

fn edge(pins: &PulsePins, host: &mut TestHost, level: bool, at: u64) {
    let mut cx = host.cx();
    pins.capture_level(&mut cx, level, at).unwrap();
}

#[test]
fn armed_capture_counts_ticks_and_latches_each_edge_period_with_its_irq() {
    let mut device = unit(false);
    let pins = device.pins();
    let mut host = TestHost::new();
    let channel = CHANNEL_BASE + 0x100;
    write(&mut device, &mut host, BASE + 0x10, 0x10);
    for (offset, value) in [(0, 0x438b), (4, 0), (8, 0), (12, 0), (16, 0)] {
        write(&mut device, &mut host, channel + offset, value);
    }
    // Edges before the arm only set the input level.
    edge(&pins, &mut host, true, 0);
    host.now = 100;
    write(&mut device, &mut host, BASE, 0x0002_0002);
    host.now = 350;
    assert_eq!(read(&mut device, &mut host, channel + 8), 250);
    // The first armed edge restarts the count without a period or IRQ.
    edge(&pins, &mut host, false, 400);
    assert!(host.irqs.is_empty());
    assert_eq!(read(&mut device, &mut host, channel + 16), 0);
    host.now = 450;
    assert_eq!(read(&mut device, &mut host, channel + 8), 50);
    edge(&pins, &mut host, true, 1400);
    assert_eq!(host.irqs, [(0, 21)]);
    host.now = 1400;
    assert_eq!(read(&mut device, &mut host, channel + 16), 1000);
    assert_eq!(read(&mut device, &mut host, channel + 8), 0);
    // An unchanged level is not an edge.
    edge(&pins, &mut host, true, 1500);
    edge(&pins, &mut host, false, 1650);
    assert_eq!(host.irqs, [(0, 21), (0, 21)]);
    assert_eq!(read(&mut device, &mut host, channel + 16), 250);
    // A disabled capture ignores edges; re-arming discards the previous edge.
    write(&mut device, &mut host, BASE, 0x0002_0000);
    edge(&pins, &mut host, true, 1700);
    host.now = 2000;
    write(&mut device, &mut host, channel + 16, 0);
    write(&mut device, &mut host, BASE, 0x0002_0002);
    edge(&pins, &mut host, false, 2100);
    assert_eq!(host.irqs.len(), 2);
    let mut cx = host.cx();
    assert!(pins.capture_level(&mut cx, true, 2050).is_err());
}

#[test]
fn running_quadrature_counters_accept_preload_and_terminal_updates() {
    let mut device = unit(true);
    let pins = device.pins();
    let mut host = TestHost::new();
    write(&mut device, &mut host, BASE + 0x14, 0x100);
    write(&mut device, &mut host, CHANNEL_BASE, 0x870c);
    write(&mut device, &mut host, CHANNEL_BASE + 0x100, 0x2089);
    write(&mut device, &mut host, CHANNEL_BASE + 0x110, 5);
    write(&mut device, &mut host, BASE, 0x0003_0003);
    let mut cx = host.cx();
    for phase in [1, 2, 3, 0] {
        pins.quadrature_phase(&mut cx, phase).unwrap();
    }
    drop(cx);
    assert_eq!(read(&mut device, &mut host, CHANNEL_BASE + 8), 1);
    // A running preload write re-anchors the cycle count at the current phase.
    write(&mut device, &mut host, CHANNEL_BASE + 8, 40);
    write(&mut device, &mut host, CHANNEL_BASE + 0x110, 1);
    let mut cx = host.cx();
    for phase in [1, 2, 3, 0] {
        pins.quadrature_phase(&mut cx, phase).unwrap();
    }
    drop(cx);
    assert_eq!(read(&mut device, &mut host, CHANNEL_BASE + 8), 41);
    // Channel 1 counts one rising A edge per cycle; the second exceeds terminal 1.
    assert_eq!(host.irqs, [(0, 21)]);
    assert_eq!(read(&mut device, &mut host, CHANNEL_BASE + 0x10c), 2);
}
