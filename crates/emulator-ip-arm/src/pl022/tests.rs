use super::*;
use fujimod_emulator_parts_io::serial::Unconnected;
use fujimod_emulator_devices::test_host::recording::RecordingHost;
const BASE: u64 = 0x100000;

fn controller() -> Controller {
    Controller::new(
        BASE,
        Clock {
            source_hz: 48_000_000,
            ticks_per_second: 1_000_000_000,
        },
        vec![InterruptLine::Shared(77)],
        Box::new(Unconnected),
    )
    .unwrap()
}
fn write(c: &mut Controller, h: &mut RecordingHost, offset: u64, value: u64) {
    let mut cx = Cx::new(h, 0, 0x1234, 0);
    c.write(&mut cx, BASE + offset, 2, value);
    assert_eq!(cx.refusal(), None);
}
fn read(c: &mut Controller, h: &mut RecordingHost, offset: u64) -> u64 {
    let mut cx = Cx::new(h, 0, 0x1234, 0);
    let value = c.read(&mut cx, BASE + offset, 2);
    assert_eq!(cx.refusal(), None);
    value
}
fn tick(c: &mut Controller, h: &mut RecordingHost, now: u64) {
    h.now = now;
    let mut cx = Cx::new(h, 0, 0, 0);
    c.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}
fn configure(c: &mut Controller, h: &mut RecordingHost, loopback: bool) {
    write(c, h, 0, 0xc7); // 8 bits, Motorola mode3.
    write(c, h, 0x10, 6);
    write(c, h, 0x24, 3);
    write(c, h, 4, if loopback { 3 } else { 2 });
}
#[test]
fn fifo_bytes_and_request_edges_follow_serial_clocks() {
    let mut h = RecordingHost::default();
    let mut c = controller();
    configure(&mut c, &mut h, true);
    let p = c.port();
    assert_eq!(read(&mut c, &mut h, 0xc), 3);
    assert_eq!(p.lock().unwrap().request(Direction::Receive, false), None);
    assert_eq!(
        p.lock().unwrap().request(Direction::Transmit, false),
        Some(0)
    );
    for value in 0..9 {
        p.lock().unwrap().transmit(0, 1, value).unwrap();
    }
    assert_eq!(p.lock().unwrap().request(Direction::Transmit, false), None);
    assert_eq!(c.next_deadline(0), Some(1000));
    let saved = c.save();
    tick(&mut c, &mut h, 999);
    assert_eq!(saved, c.save());
    tick(&mut c, &mut h, 1000);
    assert_eq!(
        p.lock().unwrap().request(Direction::Receive, false),
        Some(1000)
    );
    assert_eq!(
        p.lock().unwrap().request(Direction::Transmit, false),
        Some(1000)
    );
    assert_eq!(p.lock().unwrap().request(Direction::Receive, true), None);
    assert_eq!(p.lock().unwrap().request(Direction::Transmit, true), None);
    tick(&mut c, &mut h, 4000);
    assert_eq!(
        p.lock().unwrap().request(Direction::Receive, true),
        Some(4000)
    );
    assert_eq!(
        p.lock().unwrap().request(Direction::Transmit, true),
        Some(4000)
    );
    for value in 0..4 {
        assert_eq!(p.lock().unwrap().receive(4000, 1).unwrap(), value);
    }
    assert_eq!(p.lock().unwrap().request(Direction::Receive, false), None);
    assert_eq!(p.lock().unwrap().request(Direction::Receive, true), None);
    tick(&mut c, &mut h, 9000);
    for value in 4..9 {
        assert_eq!(read(&mut c, &mut h, 8), value);
    }
    assert_eq!(read(&mut c, &mut h, 0xc), 3);
}
#[test]
fn interrupt_masks_timeout_clear_and_host_inspection_are_distinct() {
    let mut h = RecordingHost::default();
    let mut c = controller();
    configure(&mut c, &mut h, true);
    write(&mut c, &mut h, 0x14, 2);
    write(&mut c, &mut h, 8, 0x1234);
    tick(&mut c, &mut h, 1000);
    assert_eq!(read(&mut c, &mut h, 0x14), 2);
    assert_eq!(read(&mut c, &mut h, 0x18), 8);
    assert_eq!(c.next_deadline(1000), Some(5000));
    let before = c.save();
    for _ in 0..20 {
        assert_eq!(c.save(), before);
        assert_eq!(c.next_deadline(1000), Some(5000));
    }
    tick(&mut c, &mut h, 4999);
    assert_eq!(c.save(), before);
    tick(&mut c, &mut h, 5000);
    assert_eq!(read(&mut c, &mut h, 0x1c), 2);
    assert_eq!(h.irqs, vec![(77, true)]);
    write(&mut c, &mut h, 0x20, 2);
    assert_eq!(h.irqs, vec![(77, true), (77, false)]);
    assert_eq!(c.next_deadline(5000), Some(9000));
    tick(&mut c, &mut h, 9000);
    assert_eq!(read(&mut c, &mut h, 8), 0x34);
    assert_eq!(h.irqs.last(), Some(&(77, false)));
    assert_eq!(c.next_deadline(9000), None);
}
#[test]
fn overrun_latches_and_acknowledges_without_fabricated_data() {
    let mut h = RecordingHost::default();
    let mut c = controller();
    configure(&mut c, &mut h, true);
    write(&mut c, &mut h, 0x14, 1);
    for value in 0..9 {
        write(&mut c, &mut h, 8, value);
    }
    tick(&mut c, &mut h, 9000);
    assert_eq!(read(&mut c, &mut h, 0x1c), 1);
    assert_eq!(h.irqs, vec![(77, true)]);
    write(&mut c, &mut h, 0x20, 1);
    assert_eq!(read(&mut c, &mut h, 0x1c), 0);
    for value in 0..8 {
        assert_eq!(read(&mut c, &mut h, 8), value);
    }
    let mut cx = Cx::new(&mut h, 0, 0, 0);
    assert_eq!(c.read(&mut cx, BASE + 8, 2), 0);
    assert!(cx.is_unmodeled());
}
#[test]
fn pending_frame_checkpoint_restores_fresh_and_early_ticks_are_noops() {
    let mut h = RecordingHost::default();
    let mut c = controller();
    configure(&mut c, &mut h, true);
    write(&mut c, &mut h, 8, 0x67);
    tick(&mut c, &mut h, 500);
    let state = c.save();
    let mut restored = controller();
    restored.restore(&state).unwrap();
    assert_eq!(restored.save(), state);
    tick(&mut c, &mut h, 1000);
    tick(&mut restored, &mut RecordingHost::default(), 1000);
    assert_eq!(restored.save(), c.save());
    assert_eq!(read(&mut c, &mut h, 8), 0x67);
    let mut bad = state;
    bad["state"]["pending"]["frame"]["bits"] = json!(0);
    let before = restored.save();
    assert!(restored.restore(&bad).is_err());
    assert_eq!(restored.save(), before);
}
#[test]
fn fractional_serial_clock_preserves_phase_across_contiguous_frames() {
    let mut c = controller();
    let mut h = RecordingHost::default();
    write(&mut c, &mut h, 0, 3); // Four-bit frames.
    write(&mut c, &mut h, 0x10, 2);
    write(&mut c, &mut h, 4, 3);
    for value in 0..9 {
        write(&mut c, &mut h, 8, value);
    }
    for n in 1..=9 {
        let due = (n * 500u64).div_ceil(3);
        assert_eq!(c.next_deadline(h.now), Some(due));
        let before = c.save();
        tick(&mut c, &mut h, due - 1);
        assert_eq!(c.save(), before);
        tick(&mut c, &mut h, due);
        assert_eq!(read(&mut c, &mut h, 8), n - 1);
    }
}
#[test]
fn unconnected_wire_and_unsupported_modes_refuse_without_rx_or_completion() {
    for pc in [0x10, 0xfeed0000] {
        let mut h = RecordingHost::default();
        let mut c = controller();
        configure(&mut c, &mut h, false);
        let before = c.save();
        let mut cx = Cx::new(&mut h, 0, pc, 0);
        c.write(&mut cx, BASE + 8, 2, 0x55);
        assert!(cx.is_unmodeled());
        assert_eq!(c.save(), before);
        assert_eq!(c.next_deadline(0), None);
        assert!(h.irqs.is_empty());
    }
    let mut h = RecordingHost::default();
    let mut c = controller();
    for (offset, value) in [(0, 0x37), (4, 4), (0x24, 4)] {
        let before = c.save();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        c.write(&mut cx, BASE + offset, 2, value);
        assert!(cx.is_unmodeled());
        assert_eq!(c.save(), before);
    }
    // Reserved interrupt mask bits read as zero; writes to them are ignored.
    write(&mut c, &mut h, 0x14, 0xfff0 | 2);
    assert_eq!(read(&mut c, &mut h, 0x14), 2);
}
#[test]
fn explicit_fixed_wire_input_is_sampled_only_after_the_programmed_frame() {
    for high in [false, true] {
        let mut c = Controller::new(
            BASE,
            Clock {
                source_hz: 48_000_000,
                ticks_per_second: 1_000_000_000,
            },
            vec![],
            Box::new(fujimod_emulator_parts_io::serial::InputLevel(high)),
        )
        .unwrap();
        let mut h = RecordingHost::default();
        configure(&mut c, &mut h, false);
        let p = c.port();
        p.lock().unwrap().transmit(0, 1, 0x55).unwrap();
        assert_eq!(p.lock().unwrap().request(Direction::Receive, false), None);
        tick(&mut c, &mut h, 999);
        assert_eq!(p.lock().unwrap().request(Direction::Receive, false), None);
        tick(&mut c, &mut h, 1000);
        assert_eq!(
            p.lock().unwrap().receive(1000, 1).unwrap(),
            if high { 255 } else { 0 }
        );
    }
}

#[test]
fn active_retiming_and_disabling_unread_data_refuse_without_discarding_state() {
    let mut c = controller();
    let mut h = RecordingHost::default();
    configure(&mut c, &mut h, true);
    write(&mut c, &mut h, 8, 0x67);
    for (offset, value) in [(0, 0x47), (0x10, 4), (4, 0)] {
        let before = c.save();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        c.write(&mut cx, BASE + offset, 2, value);
        assert!(cx.is_unmodeled());
        assert_eq!(c.save(), before);
    }
    tick(&mut c, &mut h, 1000);
    let before = c.save();
    let mut cx = Cx::new(&mut h, 0, 0, 0);
    c.write(&mut cx, BASE + 4, 2, 0);
    assert!(cx.is_unmodeled());
    assert_eq!(c.save(), before);
    assert_eq!(read(&mut c, &mut h, 8), 0x67);
    write(&mut c, &mut h, 4, 0);
    write(&mut c, &mut h, 0x10, 4);
}
#[test]
fn interrupt_fanout_and_checkpoint_wiring_are_explicit() {
    let mut c = controller();
    c.interrupts.push(InterruptLine::Shared(78));
    let mut h = RecordingHost::default();
    configure(&mut c, &mut h, true);
    write(&mut c, &mut h, 0x14, 8);
    assert_eq!(h.irqs, vec![(77, true), (78, true)]);
    let mut other = controller();
    let before = other.save();
    assert!(other.restore(&c.save()).is_err());
    assert_eq!(other.save(), before);
    other.interrupts = c.interrupts.clone();
    other.restore(&c.save()).unwrap();
    assert_eq!(other.save(), c.save());
}
