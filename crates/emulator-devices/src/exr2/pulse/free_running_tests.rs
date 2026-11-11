//! Channel 7 as a free-running up-count (control 0x900b: bit 3 with the timer bits, divisor 128): +08 reads the
//! counts since the enable, it raises nothing and schedules nothing, and a disable clears it so the channel can be
//! reprogrammed as an interval timer.
use super::*;
use crate::test_host::{TestHost, advance, read, write};

fn unit(free_running_channels: Vec<u8>) -> PulseUnit {
    PulseUnit::new(
        Options {
            counter_channels: vec![],
            timer_channels: vec![7],
            one_shot_channels: vec![],
            counter_readback_channels: vec![],
            free_running_channels,
            mirrored_irqs: vec![],
            quadrature: false,
            initial_phase: 0,
            capture_channel: None,
        },
        Wiring {
            irqs: std::array::from_fn(|ch| (ch == 7).then_some(InterruptLine::Core { core: 0, line: 27 })),
        },
    )
    .unwrap()
}
const CH7: u64 = CHANNEL_BASE + 7 * 0x100;

#[test]
fn free_running_channel_counts_up_at_its_divisor_without_an_irq() {
    let mut device = unit(vec![7]);
    let mut host = TestHost::new();
    host.now = 1000;
    for (offset, value) in [(0, 0x900b), (4, 0), (8, 0), (12, 0), (16, 0)] {
        write(&mut device, &mut host, CH7 + offset, value);
    }
    write(&mut device, &mut host, BASE, 0x0080_0080);
    assert_eq!(read(&mut device, &mut host, CH7 + 8), 0);
    assert_eq!(device.next_deadline(host.now), None);
    advance(&mut device, &mut host, 1000 + 128 * 1875 + 5);
    assert_eq!(read(&mut device, &mut host, CH7 + 8), 1875);
    assert!(host.irqs.is_empty());
    // Disable, reprogram as the interval tick and enable: the count no longer runs.
    write(&mut device, &mut host, BASE, 0x0080_0000);
    for (offset, value) in [(0, 0x9001), (8, 0x7530), (12, 0x752f)] {
        write(&mut device, &mut host, CH7 + offset, value);
    }
    write(&mut device, &mut host, BASE, 0x0080_0080);
    assert_eq!(read(&mut device, &mut host, CH7 + 8), 0x7530);
    assert!(device.next_deadline(host.now).is_some());
}

#[test]
fn free_running_mode_needs_its_declaration_and_an_empty_configuration() {
    let mut host = TestHost::new();
    let mut undeclared = unit(vec![]);
    let mut cx = host.cx();
    undeclared.write(&mut cx, CH7, 4, 0x900b);
    assert_eq!(cx.refusal(), Some("unsupported pulse control mode"));
    let mut declared = unit(vec![7]);
    write(&mut declared, &mut host, CH7, 0x900b);
    write(&mut declared, &mut host, CH7 + 8, 5);
    let mut cx = host.cx();
    declared.write(&mut cx, BASE, 4, 0x0080_0080);
    assert_eq!(cx.refusal(), Some("incomplete free-running count configuration"));
    assert!(
        PulseUnit::new(
            Options {
                counter_channels: vec![],
                timer_channels: vec![],
                one_shot_channels: vec![],
                counter_readback_channels: vec![],
                free_running_channels: vec![12],
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
