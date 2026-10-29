use super::*;
use fujimod_emulator_ip_arm::pl022::{Clock, Controller};
use fujimod_emulator_parts_io::serial::Unconnected;
use fujimod_emulator_devices::test_host::recording::RecordingHost;

const BASE: u64 = 0x100000;
const SERIAL: u64 = 0x200000;
fn pair(routing: SharedRouting, connections: BTreeMap<u8, Connection>) -> Pair {
    Pair::new(
        BASE,
        routing,
        connections,
        [
            vec![InterruptLine::Shared(42)],
            vec![InterruptLine::Shared(43)],
        ],
    )
    .unwrap()
}
fn write(d: &mut dyn Device, h: &mut RecordingHost, address: u64, value: u64) {
    let mut cx = Cx::new(h, 0, 0x1234, 0);
    d.write(&mut cx, address, 4, value);
    assert_eq!(cx.refusal(), None, "{address:#x} <- {value:#x}");
}
fn read(d: &mut dyn Device, h: &mut RecordingHost, address: u64) -> u64 {
    let mut cx = Cx::new(h, 0, 0x1234, 0);
    let v = d.read(&mut cx, address, 4);
    assert_eq!(cx.refusal(), None);
    v
}
fn advance(d: &mut dyn Device, h: &mut RecordingHost) {
    let mut cx = Cx::new(h, 0, 0, 0);
    d.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}
fn configure(
    d: &mut Pair,
    h: &mut RecordingHost,
    lane: u64,
    source: u64,
    destination: u64,
    shapes: [u64; 2],
    bytes: u64,
) {
    for (offset, value) in [
        (0x10, bytes - 1),
        (0x14, source),
        (0x18, destination),
        (0x1c, shapes[0]),
        (0x20, shapes[1]),
        (0x24, 0),
    ] {
        write(d, h, BASE + lane * 0x30 + offset, value);
    }
}
#[test]
fn software_transfer_copies_addressed_bytes_before_status_and_acknowledges_zero() {
    let routing = Arc::new(Mutex::new(Routing::default()));
    routing.lock().unwrap().set_request(0, 63).unwrap();
    let mut d = pair(routing.clone(), BTreeMap::new());
    let mut h = RecordingHost {
        ram: (0..128).collect(),
        ..Default::default()
    };
    configure(&mut d, &mut h, 0, 8, 72, [0x30000; 2], 32);
    write(&mut d, &mut h, BASE, ENABLE as u64);
    write(
        &mut d,
        &mut h,
        BASE + 0x28,
        (ENABLE | SOFTWARE | 0x100000 | 3) as u64,
    );
    let before = d.save();
    for _ in 0..10 {
        assert_eq!(d.next_deadline(0), Some(0));
        assert_eq!(d.save(), before);
    }
    assert_eq!(&h.ram[72..104], &(72..104).collect::<Vec<u8>>());
    for n in 0..4 {
        assert_eq!(
            read(&mut d, &mut h, BASE + 0x30),
            if n == 0 { 0 } else { 0x10000 }
        );
        advance(&mut d, &mut h);
        assert_eq!(&h.ram[72..80 + n * 8], &h.ram[8..16 + n * 8]);
    }
    assert_eq!(read(&mut d, &mut h, BASE + 0x30), 8);
    let mut cx = Cx::new(&mut h, 0, 0, 0);
    d.read(&mut cx, BASE + 0x10, 4);
    assert!(cx.is_unmodeled()); // Exhausted counter encoding is unproved.
    assert_eq!(h.irqs, vec![(42, true)]);
    assert_eq!(d.next_deadline(0), None);
    let mut cx = Cx::new(&mut h, 0, 0, 0);
    d.write(&mut cx, BASE + 0x30, 4, 8);
    assert!(cx.is_unmodeled());
    assert_eq!(read(&mut d, &mut h, BASE + 0x30), 8);
    write(&mut d, &mut h, BASE + 0x30, 0);
    assert_eq!(h.irqs.last(), Some(&(42, false)));
    assert_eq!(read(&mut d, &mut h, BASE + 0x14), 40);
    assert_eq!(read(&mut d, &mut h, BASE + 0x18), 104);
    assert!(routing.lock().unwrap().set_request(0, 3).is_ok());
}
struct SerialDma {
    d: Pair,
    serial: Controller,
    h: RecordingHost,
}
impl SerialDma {
    fn fresh() -> Self {
        let serial = Controller::new(
            SERIAL,
            Clock {
                source_hz: 48_000_000,
                ticks_per_second: 1_000_000_000,
            },
            vec![],
            Box::new(Unconnected),
        )
        .unwrap();
        let routing = Arc::new(Mutex::new(Routing::default()));
        routing.lock().unwrap().set_request(0, 3).unwrap();
        routing.lock().unwrap().set_request(1, 2).unwrap();
        let connections = [(3, Direction::Transmit), (2, Direction::Receive)]
            .into_iter()
            .map(|(request, direction)| {
                (
                    request,
                    Connection {
                        address: SERIAL + 8,
                        direction,
                        port: serial.port(),
                    },
                )
            })
            .collect();
        Self {
            d: pair(routing, connections),
            serial,
            h: RecordingHost {
                ram: (0..128).collect(),
                ..Default::default()
            },
        }
    }
    fn start(&mut self) {
        self.start_with_request_option(0);
    }
    fn start_with_request_option(&mut self, request_option: u32) {
        for (offset, value) in [(0, 0xc7), (0x10, 6), (4, 3), (0x24, 3)] {
            write(&mut self.serial, &mut self.h, SERIAL + offset, value);
        }
        configure(&mut self.d, &mut self.h, 0, 0, SERIAL + 8, [0, 4], 13);
        configure(&mut self.d, &mut self.h, 1, SERIAL + 8, 64, [4, 0], 13);
        write(&mut self.d, &mut self.h, BASE, ENABLE as u64);
        for lane in 0..2 {
            write(
                &mut self.d,
                &mut self.h,
                BASE + 0x28 + lane * 0x30,
                (ENABLE
                    | ((lane as u32 + 2) << 20)
                    | SERIAL_ACCESS
                    | request_option
                    | BLOCK
                    | ACK_WRITE
                    | 1) as u64,
            );
        }
    }
    fn until(&mut self, target: u64) {
        loop {
            let next = self
                .d
                .next_deadline(self.h.now)
                .into_iter()
                .chain(self.serial.next_deadline(self.h.now))
                .min();
            let Some(at) = next.filter(|&at| at <= target) else {
                self.h.now = target;
                break;
            };
            self.h.now = at;
            advance(&mut self.serial, &mut self.h);
            advance(&mut self.d, &mut self.h);
        }
    }
}
#[test]
fn peripheral_request_option_does_not_change_transfer_execution() {
    let mut without_option = SerialDma::fresh();
    without_option.start();
    without_option.until(13000);

    let mut with_option = SerialDma::fresh();
    with_option.start_with_request_option(REQUEST_OPTION);
    with_option.until(13000);

    assert_eq!(with_option.h.now, without_option.h.now);
    assert_eq!(with_option.h.ram, without_option.h.ram);
    assert_eq!(with_option.h.irqs, without_option.h.irqs);
    assert_eq!(with_option.serial.save(), without_option.serial.save());
    let mut channels = with_option.d.state.channels.clone();
    for channel in &mut channels {
        channel.control &= !REQUEST_OPTION;
    }
    assert_eq!(
        serde_json::to_value(channels).unwrap(),
        serde_json::to_value(&without_option.d.state.channels).unwrap()
    );
}

#[test]
fn peripheral_dma_waits_for_fifo_edges_and_rx_completion_follows_real_serial_data() {
    let mut s = SerialDma::fresh();
    s.start();
    s.until(999);
    assert!(s.h.irqs.is_empty());
    assert_eq!(&s.h.ram[64..77], &(64..77).collect::<Vec<_>>());
    assert_eq!(s.d.next_deadline(999), None); // Waiting on a peer, no polling timer.
    s.until(1000);
    assert_eq!(s.h.ram[64], 0);
    assert_eq!(read(&mut s.d, &mut s.h, BASE + 0x60), 0x10000);
    s.until(4000);
    assert_eq!(s.h.irqs, vec![(42, true)]); // TX FIFO accepted the last byte.
    s.until(12999);
    assert_eq!(&s.h.ram[64..76], &s.h.ram[..12]);
    assert_eq!(s.h.ram[76], 76);
    s.until(13000);
    assert_eq!(&s.h.ram[64..77], &s.h.ram[..13]);
    assert_eq!(s.h.irqs, vec![(42, true), (43, true)]);
    assert_eq!(read(&mut s.d, &mut s.h, BASE + 0x60), 8);
    assert_eq!(s.d.next_deadline(13000), None);
    assert_eq!(s.serial.next_deadline(13000), None);
}
#[test]
fn pending_serial_dma_restores_fresh_and_continues_with_identical_data_and_native_state() {
    let mut s = SerialDma::fresh();
    s.start();
    s.until(3500);
    let ds = s.d.save();
    let ss = s.serial.save();
    let mut fresh = SerialDma::fresh();
    fresh.h.now = s.h.now;
    fresh.h.ram = s.h.ram.clone();
    fresh.d.restore(&ds).unwrap();
    fresh.serial.restore(&ss).unwrap();
    s.until(13000);
    fresh.until(13000);
    assert_eq!(fresh.h.ram, s.h.ram);
    assert_eq!(fresh.h.irqs, s.h.irqs);
    assert_eq!(fresh.d.save(), s.d.save());
    assert_eq!(fresh.serial.save(), s.serial.save());
    let before = fresh.d.save();
    let mut invalid = ds;
    invalid["state"]["channels"][0]["remaining"] = json!(0);
    assert!(fresh.d.restore(&invalid).is_err());
    assert_eq!(fresh.d.save(), before);
}
#[test]
fn unknown_control_ports_high_addresses_and_live_reconfiguration_refuse_without_completion() {
    let mut s = SerialDma::fresh();
    s.start();
    assert!(s.d.routing.lock().unwrap().set_request(0, 4).is_err());
    assert!(s.d.routing.lock().unwrap().set_high(0, 1).is_err());
    let before = s.d.save();
    let mut cx = Cx::new(&mut s.h, 0, 0xfeed, 0);
    s.d.write(&mut cx, BASE + 0x14, 4, 16);
    assert!(cx.is_unmodeled());
    assert_eq!(s.d.save(), before);
    for control in [0x10209211, 0x10401011, 0x10200001] {
        let mut fresh = SerialDma::fresh();
        configure(&mut fresh.d, &mut fresh.h, 0, 0, SERIAL + 8, [0, 4], 1);
        let before = fresh.d.save();
        let mut cx = Cx::new(&mut fresh.h, 0, 0, 0);
        fresh.d.write(&mut cx, BASE + 0x28, 4, control);
        assert!(cx.is_unmodeled());
        assert_eq!(fresh.d.save(), before);
        assert!(fresh.h.irqs.is_empty());
        assert_eq!(fresh.d.next_deadline(0), None);
    }
}
#[test]
fn disabling_active_channel_reports_stop_and_never_normal_completion() {
    let mut s = SerialDma::fresh();
    s.start();
    let control = read(&mut s.d, &mut s.h, BASE + 0x28) as u32;
    write(&mut s.d, &mut s.h, BASE + 0x28, (control & !ENABLE) as u64);
    assert_eq!(read(&mut s.d, &mut s.h, BASE + 0x30), 2);
    assert!(s.h.irqs.is_empty()); // Error interrupt was masked.
    write(&mut s.d, &mut s.h, BASE, 0);
    assert_eq!(read(&mut s.d, &mut s.h, BASE + 0x60), 2);
    assert_eq!(s.d.next_deadline(0), None);
    assert_eq!(&s.h.ram[64..77], &(64..77).collect::<Vec<_>>());
}

#[test]
fn global_stop_honors_error_mask_and_unknown_transfer_shapes_never_run() {
    let routing = Arc::new(Mutex::new(Routing::default()));
    routing.lock().unwrap().set_request(0, 63).unwrap();
    let mut d = pair(routing.clone(), BTreeMap::new());
    let mut h = RecordingHost {
        ram: vec![0x55; 128],
        ..Default::default()
    };
    configure(&mut d, &mut h, 0, 0, 64, [0; 2], 1);
    write(&mut d, &mut h, BASE, ENABLE as u64);
    write(
        &mut d,
        &mut h,
        BASE + 0x28,
        (ENABLE | SOFTWARE | 0x100000 | 3) as u64,
    );
    write(&mut d, &mut h, BASE, 0);
    assert_eq!(read(&mut d, &mut h, BASE + 0x30), 2);
    assert_eq!(h.irqs, vec![(42, true)]);
    write(&mut d, &mut h, BASE + 0x30, 0);
    assert_eq!(h.irqs.last(), Some(&(42, false)));
    for (register, value) in [(0x1c, 0x100), (0x24, 1)] {
        let mut s = SerialDma::fresh();
        configure(&mut s.d, &mut s.h, 0, 0, SERIAL + 8, [0, 4], 1);
        write(&mut s.d, &mut s.h, BASE + register, value);
        let before = s.d.save();
        let mut cx = Cx::new(&mut s.h, 0, 0, 0);
        s.d.write(&mut cx, BASE + 0x28, 4, (ENABLE | 0x201011) as u64);
        assert!(cx.is_unmodeled());
        assert_eq!(s.d.save(), before);
    }
    routing.lock().unwrap().set_high(0, 1).unwrap();
    let before = d.save();
    let mut cx = Cx::new(&mut h, 0, 0, 0);
    d.write(
        &mut cx,
        BASE + 0x28,
        4,
        (ENABLE | SOFTWARE | 0x100003) as u64,
    );
    assert!(cx.is_unmodeled());
    assert_eq!(d.save(), before);
}

#[test]
fn exhausted_counter_cannot_restart_until_the_guest_programs_a_new_count() {
    let routing = Arc::new(Mutex::new(Routing::default()));
    routing.lock().unwrap().set_request(0, 63).unwrap();
    let mut d = pair(routing, BTreeMap::new());
    let mut h = RecordingHost {
        ram: (0..128).collect(),
        ..Default::default()
    };
    let control = (ENABLE | SOFTWARE | 0x100001) as u64;
    configure(&mut d, &mut h, 0, 8, 64, [0; 2], 1);
    write(&mut d, &mut h, BASE, ENABLE as u64);
    write(&mut d, &mut h, BASE + 0x28, control);
    advance(&mut d, &mut h);
    write(&mut d, &mut h, BASE + 0x30, 0);
    let state = d.save();
    let ram = h.ram.clone();
    let mut cx = Cx::new(&mut h, 0, 0, 0);
    d.write(&mut cx, BASE + 0x28, 4, control);
    assert!(cx.is_unmodeled());
    assert_eq!(d.save(), state);
    assert_eq!(h.ram, ram);
    assert_eq!(d.next_deadline(0), None);
    write(&mut d, &mut h, BASE + 0x10, 1);
    write(&mut d, &mut h, BASE + 0x28, control);
    advance(&mut d, &mut h);
    advance(&mut d, &mut h);
    assert_eq!(&h.ram[64..67], &[8, 9, 10]);
    assert_eq!(read(&mut d, &mut h, BASE + 0x30), 8);
}
