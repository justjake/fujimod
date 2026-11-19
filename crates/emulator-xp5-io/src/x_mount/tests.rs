use super::*;
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::test_host::recording::RecordingHost;
use fujimod_emulator_devices::xp5::pins::Pins;
use fujimod_emulator_ip_arm::pl022::{Clock, Controller};
use fujimod_emulator_parts_io::serial::InputLevel;

const BASE: u64 = 0xff910000;
const MS: u64 = 1_000_000;

fn options() -> Options {
    toml::from_str(
        r#"
        lens = "xf18-55"
        detect = { port = 0, pin = 11 }
        sense = [{ port = 10, pin = 0 }, { port = 10, pin = 1 }, { port = 10, pin = 2 }]
        sense_unpowered = [true, false, true]
        supplies = [{ port = 15, pin = 9 }, { port = 15, pin = 10 }]
        ready = { port = 14, pin = 1 }
        ready_mux = 3
        ready_low_line = 131
        ready_high_line = 115
        startup_us = 2000
        busy_us = 20000
        "#,
    )
    .unwrap()
}
fn connector(pins: SharedPins, choice: &str) -> Connector {
    let options = options();
    let lens = build_lens(&options, choice, 1_000_000_000).unwrap();
    let wire: Box<dyn fujimod_emulator_parts_io::serial::Wire> = match &lens {
        Some(port) => Box::new(port.clone()),
        None => Box::new(InputLevel(true)),
    };
    let controller = Controller::new(
        BASE,
        Clock { source_hz: 48_000_000, ticks_per_second: 1_000_000_000 },
        vec![InterruptLine::Shared(36)],
        wire,
    )
    .unwrap();
    Connector::new(options, choice.into(), lens, pins, Box::new(controller)).unwrap()
}
fn level(pins: &SharedPins, port: usize, pin: usize) -> bool {
    pins.lock().unwrap().level(port, pin) == 1
}
fn advance(device: &mut Connector, host: &mut RecordingHost, now: u64) {
    host.now = now;
    let mut cx = Cx::new(host, 0, 0, 0);
    device.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}
fn latch(pins: &SharedPins, port: usize, pin: usize, high: bool) {
    let mut p = pins.lock().unwrap();
    if high {
        p.ports[port].registers[0] |= 1 << pin;
    } else {
        p.ports[port].registers[0] &= !(1 << pin);
    }
}

#[test]
fn detect_sense_power_and_ready_interrupt_sequence() {
    let pins = Pins::shared();
    let mut device = connector(pins.clone(), "xf18-55");
    let mut host = RecordingHost::default();
    // Attached and unpowered: detect low, pattern A, ready high.
    assert!(!level(&pins, 0, 11));
    assert_eq!([level(&pins, 10, 0), level(&pins, 10, 1), level(&pins, 10, 2)], [true, false, true]);
    assert!(level(&pins, 14, 1));
    advance(&mut device, &mut host, 1000);
    assert!(host.irqs.is_empty(), "ready is not routed before its pad function is selected");
    // The body enables the first supply: still unpowered, pattern unchanged.
    latch(&pins, 15, 9, true);
    assert_eq!(device.next_deadline(2000), None);
    // Second supply and the ready pad function.
    latch(&pins, 15, 10, true);
    pins.lock().unwrap().ports[14].registers[3] |= 3 << 2;
    assert_eq!(device.next_deadline(3000), Some(3000));
    advance(&mut device, &mut host, 3000);
    assert!(device.lens().unwrap().powered());
    assert_eq!(host.irqs, [(115, true)], "ready high while starting");
    // Startup: ready falls (INTID 131), then the busy period ends (INTID 115).
    let start = device.next_deadline(3001).unwrap();
    assert_eq!(start, 3000 + 2 * MS);
    advance(&mut device, &mut host, start);
    assert!(!level(&pins, 14, 1));
    assert_eq!(host.irqs[1..], [(131, true), (115, false)]);
    advance(&mut device, &mut host, start + 20 * MS);
    assert!(level(&pins, 14, 1));
    assert_eq!(host.irqs[3..], [(131, false), (115, true)]);
    // Supply loss powers the lens off and restores the sense pattern.
    pins.lock().unwrap().set_input(10, 1, true);
    latch(&pins, 15, 10, false);
    advance(&mut device, &mut host, start + 21 * MS);
    assert!(!device.lens().unwrap().powered());
    assert!(!level(&pins, 10, 1));
}

#[test]
fn no_lens_leaves_the_pads_alone() {
    let pins = Pins::shared();
    let mut device = connector(pins.clone(), "none");
    let mut host = RecordingHost::default();
    latch(&pins, 15, 9, true);
    latch(&pins, 15, 10, true);
    advance(&mut device, &mut host, 5 * MS);
    assert!(level(&pins, 0, 11) && level(&pins, 10, 1) && level(&pins, 14, 1));
    assert!(host.irqs.is_empty());
    assert!(device.lens().is_none());
    assert!(build_lens(&options(), "zoom9000", 1_000_000_000).is_err());
}

/// Exchange bytes through the PL022 registers: CR0 8-bit mode 3, prescaler 2.
fn spi(device: &mut Connector, host: &mut RecordingHost, now: &mut u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![];
    for chunk in bytes.chunks(4) {
        {
            let mut cx = Cx::new(host, 0, 0, 0);
            for byte in chunk {
                device.write(&mut cx, BASE + 8, 4, *byte as u64);
            }
            assert_eq!(cx.refusal(), None);
        }
        *now += 10_000;
        advance(device, host, *now);
        let mut cx = Cx::new(host, 0, 0, 0);
        for _ in chunk {
            out.push(device.read(&mut cx, BASE + 8, 4) as u8);
        }
        assert_eq!(cx.refusal(), None);
    }
    out
}

#[test]
fn the_serial_controller_carries_lens_frames_and_checkpoints() {
    use fujimod_emulator_parts_lens::x_mount_model::family;
    let pins = Pins::shared();
    let mut device = connector(pins.clone(), "xf18-55");
    let mut host = RecordingHost::default();
    latch(&pins, 15, 9, true);
    latch(&pins, 15, 10, true);
    let mut now = 1000;
    advance(&mut device, &mut host, now);
    now += 3 * MS;
    advance(&mut device, &mut host, now);
    assert!(device.lens().unwrap().serving());
    {
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.write(&mut cx, BASE, 4, 0xc7);
        device.write(&mut cx, BASE + 0x10, 4, 2);
        device.write(&mut cx, BASE + 4, 4, 2);
        assert_eq!(cx.refusal(), None);
    }
    // Feedback request, acknowledgment, then an idle header collects the reply.
    let encode = |payload: u16, fam: u16, ack: bool| {
        // Same framing as x_mount_link::Packet (5-bit additive check).
        let mut word = ((payload as u32) << 16) | ((fam as u32) << 6) | (u32::from(ack) << 15);
        let check = ([27, 22, 17, 12, 7].iter().map(|s| (word >> s) & 31).sum::<u32>()
            + ((word >> 6) & 1))
            & 31;
        word |= check << 1;
        word.to_be_bytes()
    };
    spi(&mut device, &mut host, &mut now, &encode(0, family::FEEDBACK, false));
    spi(&mut device, &mut host, &mut now, &encode(0x0800, 0, true));
    let saved = device.checkpoint().unwrap();
    let reply = spi(&mut device, &mut host, &mut now, &[0, 0, 0, 0]);
    assert_eq!(reply, encode(2000, family::FEEDBACK, false), "focus position 2000");
    // Restore and replay: the same reply comes back.
    device.validate_checkpoint(&saved).unwrap();
    device.restore_checkpoint(&saved).unwrap();
    let again = spi(&mut device, &mut host, &mut now, &[0, 0, 0, 0]);
    assert_eq!(again, reply);
    // A run with another lens refuses the checkpoint.
    let other = connector(Pins::shared(), "prime");
    assert!(other.validate_checkpoint(&saved).is_err());
}
