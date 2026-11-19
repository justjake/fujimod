use super::*;

const MS: u64 = 1_000_000;

fn timing() -> Timing {
    Timing {
        ticks_per_ms: MS,
        startup_ticks: 2 * MS,
        busy_ticks: 20 * MS,
        min_operation_ticks: MS,
    }
}
fn lens() -> Port {
    Port::new(Spec::preset("xf18-55").unwrap(), timing()).unwrap()
}
fn frame(byte: u8) -> Frame {
    Frame { bits: 8, mode: 3, word: byte as u16 }
}
fn transfer(port: &mut Port, input: &[u8], time: &mut u64) -> Vec<u8> {
    input
        .iter()
        .map(|byte| {
            let token = port.begin(frame(*byte)).unwrap();
            *time += 5_000;
            port.finish(token, frame(*byte), *time).unwrap() as u8
        })
        .collect()
}
fn word(port: &mut Port, value: u32, time: &mut u64) -> u32 {
    u32::from_be_bytes(transfer(port, &value.to_be_bytes(), time).try_into().unwrap())
}
/// One exchange: the body's header, then its acknowledgment of what the lens sent.
/// Returns the lens's header word.
fn exchange(port: &mut Port, header: u32, time: &mut u64) -> u32 {
    let sent = word(port, header, time);
    let ack = Packet::encode(0x0800, Packet(sent).family(), true).0;
    let lens_ack = word(port, ack, time);
    assert!(Packet(lens_ack).ack(), "lens acknowledgment {lens_ack:08x}");
    assert_eq!(Packet(lens_ack).family(), Packet(header).family());
    sent
}
/// Send a request and collect its reply with an idle header.
fn request(port: &mut Port, family: u16, payload: u16, time: &mut u64) -> u16 {
    exchange(port, Packet::encode(payload, family, false).0, time);
    let reply = exchange(port, 0, time);
    assert_eq!(Packet(reply).family(), family, "reply {reply:08x}");
    Packet(reply).payload()
}
fn powered(port: &Port, time: &mut u64) {
    port.tick(*time, true);
    *time += 2 * MS;
    port.tick(*time, true);
    assert!(port.serving());
}

#[test]
fn packets_use_the_x_mount_framing_and_check() {
    // The sync request of the EXR II startup trace.
    let sync = Packet::encode(0, SYNC, false);
    assert!(valid_word(sync.0));
    assert_eq!(sync.family(), SYNC);
    assert!(!valid_word(sync.0 ^ 0x2), "a flipped check bit fails");
    assert!(!valid_word(sync.0 | 1), "bit 0 must be clear");
    let ack = Packet::encode(0x0a00, 0x21, true);
    assert!(ack.ack() && valid_word(ack.0));
    assert_eq!((ack.payload(), ack.family()), (0x0a00, 0x21));
    for payload in [0u16, 1, 0x7fff, 0xffff] {
        for family in [0u16, 0x14, 0x54, 0x11a, 0x1ff] {
            let p = Packet::encode(payload, family, false);
            assert_eq!((p.payload(), p.family(), p.ack()), (payload, family, false));
            assert!(valid_word(p.0));
        }
    }
}

#[test]
fn identification_record_is_registered_and_sums_to_zero() {
    for name in ["xf18-55", "prime"] {
        let spec = Spec::preset(name).unwrap();
        let record = &spec.identification;
        assert_eq!(record.len(), 128);
        assert!(record[8..16].iter().any(|b| *b != 0xff), "registered lens");
        assert_eq!(record.iter().fold(0u8, |s, b| s.wrapping_add(*b)), 0);
        assert!(record[0x20..0x40].starts_with(spec.name.as_bytes()));
    }
    assert!(Spec::preset("none").is_none());
}

#[test]
fn power_ready_sequence_and_undriven_frames() {
    let mut port = lens();
    let mut time = 0;
    assert!(port.ready_high() && !port.serving());
    // Unpowered: frames read the idle-high line.
    assert_eq!(transfer(&mut port, &[0x12], &mut time), [0xff]);
    port.tick(time, true);
    assert_eq!(port.next_event_tick(time, true), Some(time + 2 * MS));
    assert!(port.ready_high(), "ready stays high while starting");
    time += 2 * MS;
    port.tick(time, true);
    assert!(port.serving() && !port.ready_high(), "startup pulls ready low");
    time += 20 * MS;
    port.tick(time, true);
    assert!(port.ready_high(), "busy period ends with ready released high");
    port.tick(time, false);
    assert!(!port.serving() && port.ready_high());
    assert!(port.take_logs().iter().any(|l| l.contains("before the lens was ready")));
}

#[test]
fn startup_link_then_identification_bulk_read() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    assert_eq!(exchange(&mut port, Packet::encode(0, SYNC, false).0, &mut time), 0);
    let params = Packet::encode(0xb801, PARAMETERS, false).0;
    assert_eq!(exchange(&mut port, params, &mut time), Packet::encode(0, SYNC, false).0);
    let identification = Packet::encode(0, IDENTIFICATION, false).0;
    assert_eq!(exchange(&mut port, identification, &mut time), params);
    // The lens sends its identification reply; a bulk read follows.
    assert_eq!(exchange(&mut port, 0, &mut time), identification);
    let spec = Spec::preset("xf18-55").unwrap();
    for (input, header) in [([0xff, 0xff], [0, 0]), ([0xff, 0x00], [0xff, 0xff])] {
        let mut request = input.to_vec();
        request.extend([0; 128]);
        request.push(0x5a);
        let reply = transfer(&mut port, &request, &mut time);
        assert_eq!(&reply[..2], &header);
        assert_eq!(&reply[2..130], spec.identification.as_slice());
        assert_eq!(reply[130], reply[..130].iter().fold(0u8, |s, b| s.wrapping_add(*b)));
    }
    assert_eq!(port.identification_reads(), 1);
    // Back in the header phase: status reads work.
    assert_eq!(request(&mut port, STATUS, 0, &mut time), 0x04c0);
}

#[test]
fn focus_moves_to_its_target_and_feedback_reports_it() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    time += 20 * MS;
    port.tick(time, true);
    assert!(port.ready_high());
    assert_eq!(request(&mut port, FEEDBACK, 0, &mut time), 2000);
    // Absolute target 2400 at 40 units/ms: 10 ms of motion.
    request(&mut port, FOCUS_TARGET, 2400, &mut time);
    port.tick(time, true);
    assert!(!port.ready_high(), "motion holds ready low");
    let (position, target) = port.focus(time);
    assert_eq!(target, 2400);
    assert!(position > 2000 && position < 2400, "{position}");
    let end = port.next_event_tick(time, true).unwrap();
    time = end;
    port.tick(time, true);
    assert!(port.ready_high(), "settled motion releases ready");
    assert_eq!(request(&mut port, FEEDBACK, 0, &mut time), 2400);
    // A signed step and clamping at the range end.
    request(&mut port, FOCUS_STEP, (-400i16) as u16, &mut time);
    time += 50 * MS;
    assert_eq!(request(&mut port, FEEDBACK, 0, &mut time), 2000);
    request(&mut port, FOCUS_TARGET, 0xffff, &mut time);
    time += 200 * MS;
    assert_eq!(request(&mut port, FEEDBACK, 0, &mut time), 4000);
    assert_eq!(request(&mut port, MOTION_STATE, 0, &mut time), 0);
}

#[test]
fn aperture_setpoint_moves_the_index() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    request(&mut port, APERTURE_TARGET, 6, &mut time);
    assert_eq!(port.aperture(time).1, 6);
    time += 10 * MS;
    assert_eq!(request(&mut port, APERTURE_STATE, 0, &mut time), 6);
    request(&mut port, APERTURE_STEP, (-2i16) as u16, &mut time);
    time += 10 * MS;
    assert_eq!(port.aperture(time), (4, 4));
    request(&mut port, APERTURE_TARGET, 99, &mut time);
    time += 100 * MS;
    assert_eq!(port.aperture(time), (18, 18), "clamped to the smallest aperture");
}

#[test]
fn every_family_answers_and_unknown_families_log_once() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    port.take_logs();
    assert_eq!(request(&mut port, 0x177, 5, &mut time), 0);
    assert_eq!(request(&mut port, 0x177, 6, &mut time), 0);
    let logs = port.take_logs();
    assert_eq!(logs.iter().filter(|l| l.contains("unmodeled family")).count(), 1);
    assert_eq!(request(&mut port, 0xa8, 3, &mut time), 3, "settings echo");
    assert_eq!(request(&mut port, 0x10, 0, &mut time), 0);
    assert_eq!(request(&mut port, DETAIL_STATUS, 0, &mut time), 0x14);
    assert_eq!(request(&mut port, FOCUS_RING, 0, &mut time), 0xffff);
    assert!(!port.take_logs().iter().any(|l| l.contains("unmodeled family")));
}

#[test]
fn declared_replies_override_default_answers() {
    let mut spec = Spec::preset("prime").unwrap();
    spec.replies = vec![FixedReply { family: 0x11, payload: 0x1234 }];
    let mut port = Port::new(spec.clone(), timing()).unwrap();
    let mut time = 0;
    powered(&port, &mut time);
    assert_eq!(request(&mut port, 0x11, 0, &mut time), 0x1234);
    spec.replies = vec![FixedReply { family: FEEDBACK, payload: 1 }];
    assert!(Port::new(spec, timing()).is_err(), "state-driven families cannot be declared");
}

#[test]
fn malformed_frames_resynchronize() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    assert_eq!(word(&mut port, 0xdeadbeef, &mut time), 0);
    assert_eq!(request(&mut port, STATUS, 0, &mut time), 0x04c0);
    assert!(port.take_logs().iter().any(|l| l.contains("invalid header")));
}

#[test]
fn checkpoints_restore_link_motion_and_power_exactly() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    request(&mut port, FOCUS_TARGET, 3000, &mut time);
    // Leave a partial header in flight.
    transfer(&mut port, &Packet::encode(0, FEEDBACK, false).bytes()[..2], &mut time);
    let saved = port.checkpoint().unwrap();
    port.validate_checkpoint(&saved).unwrap();
    let json = port.save();
    let mut copy = lens();
    copy.restore_checkpoint(&saved).unwrap();
    assert_eq!(copy.save(), json);
    // Both continue identically.
    let rest = &Packet::encode(0, FEEDBACK, false).bytes()[2..];
    let (mut ta, mut tb) = (time, time);
    assert_eq!(transfer(&mut port, rest, &mut ta), transfer(&mut copy, rest, &mut tb));
    assert_eq!(port.save(), copy.save());
    // A different lens refuses the checkpoint; JSON restore round-trips.
    let other = Port::new(Spec::preset("prime").unwrap(), timing()).unwrap();
    assert!(other.validate_checkpoint(&saved).is_err());
    let mut json_copy = lens();
    json_copy.restore(&json).unwrap();
    assert_eq!(json_copy.save(), json);
    // The pending-frame token of the saved state validates.
    let token = (1u64 << 32) | (json["state"]["link"]["frames"].as_u64().unwrap());
    port.validate_pending(&saved, token, frame(0)).unwrap();
    assert!(port.validate_pending(&saved, token + 1, frame(0)).is_err());
}

#[test]
fn identification_satisfies_the_x_t5_startup_checks() {
    for name in ["xf18-55", "prime"] {
        let r = Spec::preset(name).unwrap().identification;
        assert!(r[8..16].iter().any(|b| *b != 0xff));
        assert_eq!(r[9], b'L');
        assert!(b"PSZ".contains(&r[0x0a]));
        assert!(b"GNS".contains(&r[0x0b]));
        assert!(b"AFW".contains(&r[0x0c]));
        assert_eq!((r[0x62], r[0x63]), (1, 0), "row 0 of the body's version table");
        assert_ne!(r[0x65] & 2, 0, "extended record announced");
        assert_eq!(r[0x64] & 0x30, 0);
    }
}

#[test]
fn family_0x96_starts_the_extended_record_bulk_read() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    let extended = Packet::encode(0, EXTENDED_IDENTIFICATION, false).0;
    exchange(&mut port, extended, &mut time);
    assert_eq!(exchange(&mut port, 0, &mut time), extended);
    let spec = Spec::preset("xf18-55").unwrap();
    for (input, header) in [([0xff, 0xff], [0, 0]), ([0xff, 0x00], [0xff, 0xff])] {
        let mut request = input.to_vec();
        request.extend([0; 128]);
        request.push(0x5a);
        let reply = transfer(&mut port, &request, &mut time);
        assert_eq!(&reply[..2], &header);
        assert_eq!(&reply[2..130], spec.extended.as_slice());
    }
    assert_eq!((port.identification_reads(), port.extended_reads()), (0, 1));
    assert_eq!(request(&mut port, STATUS, 0, &mut time), 0x04c0);
}

#[test]
fn catalog_6_serves_four_blocks_in_the_requested_order_with_the_marker() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    let query = Packet::encode(6, READ_CATALOG, false).0;
    exchange(&mut port, query, &mut time);
    let reply = exchange(&mut port, 0, &mut time);
    assert_eq!((Packet(reply).family(), Packet(reply).payload()), (READ_CATALOG, 4));
    // The body names the next block it wants (as 0x13ed614 does), then 0xffff, then 0xff00.
    let mut headers = vec![];
    for request in [1u16, 2, 3, 0xffff, 0xff00] {
        let mut bytes = request.to_be_bytes().to_vec();
        bytes.extend(vec![0; CATALOG_BLOCK_BYTES]);
        bytes.push(0x5a);
        let out = transfer(&mut port, &bytes, &mut time);
        headers.push(u16::from_be_bytes([out[0], out[1]]));
        let sum = out[..CATALOG_BLOCK_BYTES + 2].iter().fold(0u8, |s, b| s.wrapping_add(*b));
        assert_eq!(out[CATALOG_BLOCK_BYTES + 2], sum);
        if headers.len() == 1 {
            assert_eq!((out[2 + 0x47], out[2 + 0x48]), (0x55, 0xaa), "marker in block 0");
        }
    }
    assert_eq!(headers, [0, 1, 2, 3, 0xffff]);
    assert_eq!(port.catalog_reads(), 1);
    assert_eq!(request(&mut port, STATUS, 0, &mut time), 0x04c0);
    // An undeclared selector serves one block.
    exchange(&mut port, Packet::encode(0, READ_CATALOG, false).0, &mut time);
    assert_eq!(Packet(exchange(&mut port, 0, &mut time)).payload(), 1);
}
