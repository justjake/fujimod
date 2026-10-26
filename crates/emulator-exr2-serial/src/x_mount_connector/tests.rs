use super::*;
use fujimod_emulator_devices::test_host::{TestHost, read, write};
use fujimod_emulator_parts_io::serial::{Frame, Wire};
fn pad(port: u8, pin: u8, mux: u32) -> Pad {
    Pad {
        pin: Pin { port, pin },
        mux,
    }
}
fn options() -> Options {
    Options {
        supplies: [pad(2, 14, 0), pad(2, 15, 0)],
        reset: pad(2, 12, 0),
        ready: pad(0, 7, 1),
        ready_line: 15,
        clock: pad(13, 8, 1),
        transmit: pad(13, 2, 1),
        receive: pad(13, 5, 1),
    }
}
fn setup() -> (XMountLens, gpio::Gpio, irq::InterruptController, TestHost) {
    let mut levels = [0; 16];
    levels[0] = 0x80;
    let gpio = gpio::Gpio::new(
        gpio::Options {
            input_levels: levels,
            output_levels: vec![],
        },
        gpio::Wiring { irqs: vec![] },
    )
    .unwrap();
    let inputs = irq::Inputs::default();
    let controller = irq::InterruptController::new(
        irq::Options {
            arbitration: irq::Arbitration::ActiveBitmap,
        },
        irq::Wiring {
            base: 0x1000,
            inputs: inputs.clone(),
            enables: None,
            software: vec![],
        },
    )
    .unwrap();
    let lens = XMountLens::new(
        serial_dma::Options {
            serial_base: 0x5000,
            dma_base: 0x4000,
            routing_base: 0x8000,
            channels: [2, 3],
            transmit_request: 9,
            receive_request: 8,
            frame_ticks: 64,
            irq_targets: vec![],
        },
        x_mount_link::Configuration {
            peer_parameters: 0xb801,
            parameters: 0xb801,
            identification: (0..128).collect(),
            status: 0x80,
            aperture_ring: 0,
            focus_ring: 0xffff,
            detail_status: 0x14,
            read_catalogs: vec![],
        },
        100,
        options(),
        gpio.pads(),
        vec![(0, inputs)],
    )
    .unwrap();
    (lens, gpio, controller, TestHost::new())
}
fn configure(g: &mut gpio::Gpio, h: &mut TestHost) {
    for (port, words) in [
        (0, vec![(4, 0x00800000), (8, 0x00800080), (12, 0xc0004000)]),
        (
            2,
            vec![
                (0, 0xd000d000),
                (4, 0xd000d000),
                (8, 0xd000d000),
                (16, 0xf3000000),
            ],
        ),
        (
            13,
            vec![
                (0, 0x01040100),
                (4, 0x01240104),
                (8, 0x01240124),
                (12, 0x0c300410),
                (16, 0x00030001),
            ],
        ),
    ] {
        for (offset, value) in words {
            write(g, h, gpio::BASE + port * 64 + offset, value);
        }
    }
}
fn source(c: &mut irq::InterruptController, h: &mut TestHost, edge: bool, sense: u32) {
    for (offset, value) in [
        (0x43c, 0xdeadbeec),
        (0x83c, 3),
        (0x100, if edge { 1 << 15 } else { 0 }),
        (if edge { 0x140 } else { 0x180 }, sense << 30),
        (0x80, 1 << 15),
    ] {
        write(c, h, 0x1000 + offset, value);
    }
}
fn advance(l: &mut XMountLens, h: &mut TestHost, at: u64) {
    h.now = at;
    let mut cx = h.cx();
    l.advance(&mut cx);
    assert_eq!(cx.take_refusal(), None);
}
#[test]
fn known_pads_start_one_deadline_and_ready_edge_replays_after_partial_restore() {
    let (mut lens, mut gpio, mut controller, mut host) = setup();
    assert_eq!(lens.next_deadline(0), None);
    write(&mut gpio, &mut host, gpio::BASE + 2 * 64, 0xd000d000);
    assert_eq!(lens.next_deadline(0), None);
    configure(&mut gpio, &mut host);
    source(&mut controller, &mut host, true, 2);
    assert_eq!(lens.next_deadline(0), Some(0));
    advance(&mut lens, &mut host, 0);
    assert_eq!(lens.next_deadline(0), Some(100));
    let saved = lens.checkpoint().unwrap();
    let gs = gpio.checkpoint().unwrap();
    let cs = controller.checkpoint().unwrap();
    let before = lens.save();
    for at in [0, 1, 99] {
        advance(&mut lens, &mut host, at);
        assert_eq!(lens.save(), before);
        assert_eq!(lens.next_deadline(at), Some(100));
    }
    advance(&mut lens, &mut host, 100);
    assert!(!gpio.pads().input_level(options().ready.pin).unwrap());
    assert_eq!(lens.next_deadline(100), None);
    let done = lens.save();
    let final_irq = controller.checkpoint().unwrap();
    let (mut other, mut og, mut oc, mut oh) = setup();
    og.restore_checkpoint(&gs).unwrap();
    oc.restore_checkpoint(&cs).unwrap();
    other.restore_checkpoint(&saved).unwrap();
    assert_eq!(other.save(), before);
    advance(&mut other, &mut oh, 100);
    assert_eq!(other.save(), done);
    assert_eq!(oc.checkpoint().unwrap().bytes(), final_irq.bytes());
    assert_eq!(oc.save(), controller.save());
    host.now = 101;
    write(&mut gpio, &mut host, gpio::BASE + 2 * 64, 0x10000000);
    assert_eq!(lens.next_deadline(101), Some(101));
    advance(&mut lens, &mut host, 101);
    assert!(gpio.pads().input_level(options().ready.pin).unwrap());
    assert_eq!(lens.next_deadline(101), None);
}
#[test]
fn physical_level_survives_clear_and_eoi_and_withdraws_when_input_is_disabled() {
    let (mut lens, mut gpio, mut controller, mut host) = setup();
    configure(&mut gpio, &mut host);
    source(&mut controller, &mut host, false, 1);
    advance(&mut lens, &mut host, 0);
    controller.advance(&mut host.cx());
    assert!(lens.controllers[0].1.irq());
    assert_eq!(read(&mut controller, &mut host, 0x1200), 0xdeadbeec);
    write(&mut controller, &mut host, 0x1120, 1 << 15);
    write(&mut controller, &mut host, 0x1200, 0);
    assert!(lens.controllers[0].1.irq());
    assert_eq!(lens.next_deadline(0), Some(100));
    write(&mut gpio, &mut host, gpio::BASE + 8, 0x00800000);
    assert_eq!(lens.next_deadline(0), Some(0));
    advance(&mut lens, &mut host, 0);
    controller.advance(&mut host.cx());
    write(&mut controller, &mut host, 0x1120, 1 << 15);
    assert!(!lens.controllers[0].1.irq());
}
#[test]
fn every_supported_sense_is_pure_and_unknown_enabled_sense_refuses() {
    for edge in [true, false] {
        for sense in 0..=3 {
            for previous in [false, true] {
                for high in [false, true] {
                    let result = signal(edge, sense, previous, high);
                    if (edge && sense != 0) || (!edge && (sense == 1 || sense == 2)) {
                        let expected = if edge {
                            match sense {
                                1 => !previous && high,
                                2 => previous && !high,
                                _ => previous != high,
                            }
                        } else if sense == 1 {
                            high
                        } else {
                            !high
                        };
                        assert_eq!(result.unwrap(), (!edge, expected));
                    } else {
                        assert!(result.is_err());
                    }
                }
            }
        }
    }
    let (mut lens, mut gpio, mut controller, mut host) = setup();
    configure(&mut gpio, &mut host);
    source(&mut controller, &mut host, true, 0);
    assert_eq!(lens.next_deadline(0), Some(0));
    let mut cx = host.cx();
    lens.advance(&mut cx);
    assert!(cx.take_refusal().unwrap().contains("interrupt sense"));
}
#[test]
fn serial_pad_loss_refuses_and_checkpoint_wiring_validation_is_atomic() {
    let (mut lens, mut gpio, _, mut host) = setup();
    configure(&mut gpio, &mut host);
    advance(&mut lens, &mut host, 0);
    advance(&mut lens, &mut host, 100);
    let before = lens.save();
    let saved = lens.checkpoint().unwrap();
    let invalid = Checkpoint::encode(
        lens.name(),
        &Saved {
            options: Options {
                reset: options().supplies[0],
                ..options()
            },
            ..lens.saved()
        },
    )
    .unwrap()
    .with_children(saved.children().to_vec());
    assert!(lens.restore_checkpoint(&invalid).is_err());
    assert_eq!(lens.save(), before);
    write(&mut gpio, &mut host, gpio::BASE + 13 * 64 + 16, 0x00030000);
    assert_eq!(lens.next_deadline(100), Some(100));
    let mut cx = host.cx();
    lens.advance(&mut cx);
    assert!(cx.take_refusal().unwrap().contains("SPI clock/TX/RX"));
    assert_eq!(lens.save(), before);
    let mut port = lens.port.clone();
    let token = port
        .begin(Frame {
            bits: 8,
            mode: 3,
            word: 0,
        })
        .unwrap();
    host.now = 101;
    write(&mut gpio, &mut host, gpio::BASE + 2 * 64, 0x10000000);
    advance(&mut lens, &mut host, 101);
    assert!(
        port.finish(
            token,
            Frame {
                bits: 8,
                mode: 3,
                word: 0
            },
            102
        )
        .unwrap_err()
        .contains("lost power")
    );
}

#[test]
fn in_flight_serial_checkpoint_validates_against_saved_power_epoch_and_link_count() {
    let (mut lens, mut gpio, _, mut host) = setup();
    configure(&mut gpio, &mut host);
    advance(&mut lens, &mut host, 0);
    advance(&mut lens, &mut host, 100);
    for (address, value) in [
        (0x5000, 0x40),
        (0x5004, 8),
        (0x5008, 0x09090000),
        (0x5020, 0x400),
        (0x5000, 0xc0),
        (0x501c, 0),
    ] {
        write(&mut lens, &mut host, address, value);
    }
    let saved = lens.checkpoint().unwrap();
    let before = lens.save();
    advance(&mut lens, &mut host, 164);
    let after = lens.save();
    assert_ne!(before, after);
    lens.restore_checkpoint(&saved).unwrap();
    assert_eq!(lens.save(), before);
    advance(&mut lens, &mut host, 164);
    assert_eq!(lens.save(), after);
    write(&mut gpio, &mut host, gpio::BASE + 2 * 64, 0x10000000);
    advance(&mut lens, &mut host, 164);
    lens.restore_checkpoint(&saved).unwrap();
    assert_eq!(lens.save(), before);
}
