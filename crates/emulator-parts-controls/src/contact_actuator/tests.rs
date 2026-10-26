use super::*;
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::{exr2::gpio, test_host::TestHost};
fn model() -> (ContactActuator, gpio::Gpio, gpio::Pads, TestHost) {
    let mut levels = [0; 16];
    levels[0] = (1 << 9) | (1 << 6);
    let gpio = gpio::Gpio::new(
        gpio::Options {
            input_levels: levels,
            output_levels: vec![],
        },
        gpio::Wiring {
            irqs: vec![
                gpio::InputIrq {
                    pin: Pin { port: 0, pin: 9 },
                    edge: gpio::Edge::Rising,
                    outputs: vec![InterruptLine::Core { core: 0, line: 17 }],
                },
                gpio::InputIrq {
                    pin: Pin { port: 0, pin: 6 },
                    edge: gpio::Edge::Falling,
                    outputs: vec![InterruptLine::Core { core: 0, line: 17 }],
                },
            ],
        },
    )
    .unwrap();
    let pads = gpio.pads();
    let model = ContactActuator::new(
        Options {
            drive: [Pin { port: 1, pin: 1 }, Pin { port: 1, pin: 2 }],
            contacts: [Pin { port: 0, pin: 9 }, Pin { port: 0, pin: 6 }],
            positions: vec![
                Position {
                    drive: 0,
                    contacts: [true, true],
                },
                Position {
                    drive: 1,
                    contacts: [false, false],
                },
                Position {
                    drive: 3,
                    contacts: [true, true],
                },
            ],
            initial_position: 0,
            settle_ticks: 3000,
        },
        Wiring {
            pins: Box::new(pads.clone()),
        },
    )
    .unwrap();
    (model, gpio, pads, TestHost::new())
}
fn drive(gpio: &mut gpio::Gpio, h: &mut TestHost, code: u32) {
    let mut cx = h.cx();
    gpio.write(
        &mut cx,
        gpio::BASE + 0x40,
        4,
        ((6 << 16) | (code << 1)).into(),
    );
    assert!(cx.take_refusal().is_none());
}
fn tick(model: &mut ContactActuator, h: &mut TestHost) -> Result<(), String> {
    let mut cx = h.cx();
    model.advance(&mut cx);
    cx.take_refusal().map_or(Ok(()), Err)
}
#[test]
fn floating_drive_is_idle_and_contacts_change_only_at_the_settled_guest_tick() {
    let (mut model, mut gpio, pads, mut host) = model();
    assert_eq!(model.next_deadline(0), None);
    let stable = model.save();
    tick(&mut model, &mut host).unwrap();
    assert_eq!(stable, model.save());
    host.now = 10;
    drive(&mut gpio, &mut host, 1);
    assert_eq!(model.next_deadline(10), Some(10));
    tick(&mut model, &mut host).unwrap();
    assert_eq!(model.next_deadline(10), Some(3010));
    let pending = model.save();
    assert_eq!(model.fingerprint().unwrap(), pending);
    host.now = 3009;
    tick(&mut model, &mut host).unwrap();
    assert_eq!(pending, model.save());
    assert!(pads.input_level(Pin { port: 0, pin: 6 }).unwrap());
    assert!(host.irqs.is_empty());
    host.now = 3010;
    tick(&mut model, &mut host).unwrap();
    assert_eq!(model.next_deadline(3010), None);
    assert!(!pads.input_level(Pin { port: 0, pin: 9 }).unwrap());
    assert!(!pads.input_level(Pin { port: 0, pin: 6 }).unwrap());
    assert_eq!(host.irqs, vec![(0, 17)]);
    drive(&mut gpio, &mut host, 3);
    tick(&mut model, &mut host).unwrap();
    host.now = 6010;
    tick(&mut model, &mut host).unwrap();
    assert!(pads.input_level(Pin { port: 0, pin: 9 }).unwrap());
    assert_eq!(host.irqs, vec![(0, 17), (0, 17)]);
}
#[test]
fn repeated_identical_drive_does_not_move_a_deadline_and_reversal_cancels_it() {
    let (mut model, mut gpio, pads, mut host) = model();
    drive(&mut gpio, &mut host, 0);
    tick(&mut model, &mut host).unwrap();
    drive(&mut gpio, &mut host, 1);
    tick(&mut model, &mut host).unwrap();
    host.now = 100;
    drive(&mut gpio, &mut host, 1);
    tick(&mut model, &mut host).unwrap();
    assert_eq!(model.next_deadline(100), Some(3000));
    drive(&mut gpio, &mut host, 0);
    tick(&mut model, &mut host).unwrap();
    assert_eq!(model.next_deadline(100), None);
    host.now = 4000;
    tick(&mut model, &mut host).unwrap();
    assert!(pads.input_level(Pin { port: 0, pin: 6 }).unwrap());
    assert!(host.irqs.is_empty());
}
#[test]
fn unadmitted_drive_refuses_without_contact_effects() {
    let (mut model, mut gpio, pads, mut host) = model();
    drive(&mut gpio, &mut host, 2);
    assert!(
        tick(&mut model, &mut host)
            .unwrap_err()
            .contains("no declared motion")
    );
    assert!(pads.input_level(Pin { port: 0, pin: 6 }).unwrap());
    assert!(host.irqs.is_empty());
}

#[test]
fn unchanged_contacts_have_no_clock_and_keyframes_resume_a_real_pending_edge() {
    let (mut part, mut gpio, pads, mut host) = model();
    drive(&mut gpio, &mut host, 3);
    tick(&mut part, &mut host).unwrap();
    assert_eq!(part.next_deadline(0), None);
    assert_eq!(part.save()["transitions"], 0);
    host.now = 10;
    drive(&mut gpio, &mut host, 1);
    tick(&mut part, &mut host).unwrap();
    let saved = part.checkpoint().unwrap();
    let gpio_saved = gpio.checkpoint().unwrap();
    let before = part.fingerprint().unwrap();
    host.now = 3010;
    tick(&mut part, &mut host).unwrap();
    assert!(!pads.input_level(Pin { port: 0, pin: 6 }).unwrap());
    gpio.restore_checkpoint(&gpio_saved).unwrap();
    part.restore_checkpoint(&saved).unwrap();
    assert_eq!(part.fingerprint().unwrap(), before);
    assert!(pads.input_level(Pin { port: 0, pin: 6 }).unwrap());
    host.irqs.clear();
    host.now = 3009;
    tick(&mut part, &mut host).unwrap();
    assert_eq!(part.fingerprint().unwrap(), before);
    host.now = 3010;
    tick(&mut part, &mut host).unwrap();
    assert_eq!(host.irqs, vec![(0, 17)]);
    let invalid = fujimod_emulator_devices::checkpoint::Checkpoint::encode(
        part.name(),
        &(&part.options, None::<u8>, usize::MAX, None::<Motion>, 0u64),
    )
    .unwrap();
    let stable = part.fingerprint().unwrap();
    assert!(part.restore_checkpoint(&invalid).is_err());
    assert_eq!(part.fingerprint().unwrap(), stable);
}
