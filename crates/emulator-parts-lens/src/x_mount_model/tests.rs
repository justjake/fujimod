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
    for name in ["xf18-55", "prime", "xf23"] {
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
    // The target is signed and clamps at the range ends.
    request(&mut port, FOCUS_TARGET, (-400i16) as u16, &mut time);
    time += 200 * MS;
    assert_eq!(request(&mut port, FEEDBACK, 0, &mut time), 0);
    request(&mut port, FOCUS_TARGET, 0x7fff, &mut time);
    time += 200 * MS;
    assert_eq!(request(&mut port, FEEDBACK, 0, &mut time), 4000);
    // 0x54 sets the speed of the next move (units per second); 0x7fff is the lens's own.
    request(&mut port, FOCUS_SPEED, 20000, &mut time);
    let start = time;
    request(&mut port, FOCUS_TARGET, 3800, &mut time);
    let end = port.next_event_tick(time, true).unwrap();
    assert!(end - start >= 10 * MS, "200 units at 20 units/ms take 10 ms");
    request(&mut port, FOCUS_SPEED, 0x7fff, &mut time);
    // A target equal to the position still pulses ready low, so the body sees completion.
    time = end + MS;
    port.tick(time, true);
    assert!(port.ready_high());
    request(&mut port, FOCUS_TARGET, 3800, &mut time);
    port.tick(time, true);
    assert!(!port.ready_high(), "a zero-length move still holds ready low");
    assert_eq!(request(&mut port, ZOOM_STATE, 0, &mut time), 0, "zoom step 0");
}

#[test]
fn aperture_setpoint_moves_the_index() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    request(&mut port, APERTURE_TARGET, 6, &mut time);
    assert_eq!(port.aperture(time).1, 6);
    time += 10 * MS;
    // Family 0x22 reports the code in the high byte.
    assert_eq!(request(&mut port, APERTURE_STATE, 0, &mut time), 6 << 8);
    // Family 0x62 carries the code times 64 in 12 bits (0x480 = code 18).
    request(&mut port, APERTURE_DRIVE, 0x480 | 0x5000, &mut time);
    time += 20 * MS;
    assert_eq!(port.aperture(time), (18, 18));
    request(&mut port, APERTURE_DRIVE, 6 << 6, &mut time);
    time += 20 * MS;
    assert_eq!(port.aperture(time), (6, 6));
    request(&mut port, APERTURE_STEP, (-2i16) as u16, &mut time);
    time += 10 * MS;
    assert_eq!(port.aperture(time), (4, 4));
    request(&mut port, APERTURE_TARGET, 99, &mut time);
    time += 100 * MS;
    assert_eq!(port.aperture(time), (18, 18), "clamped to the smallest aperture");
}

#[test]
fn state_request_echoes_and_holds_ready_low_for_one_operation() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    time += 20 * MS;
    port.tick(time, true);
    assert!(port.ready_high());
    port.take_logs();
    assert_eq!(request(&mut port, STATE_REQUEST, 1, &mut time), 1, "payload echo");
    port.tick(time, true);
    assert!(!port.ready_high(), "the request holds ready low");
    time = port.next_event_tick(time, true).unwrap();
    port.tick(time, true);
    assert!(port.ready_high(), "ready rises after the minimum operation time");
    assert!(!port.take_logs().iter().any(|l| l.contains("unmodeled family")));
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
    for name in ["xf18-55", "prime", "xf23"] {
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
fn catalog_6_serves_its_blocks_in_the_requested_order_with_the_marker() {
    let mut port = lens();
    let mut time = 0;
    powered(&port, &mut time);
    let query = Packet::encode(6, READ_CATALOG, false).0;
    exchange(&mut port, query, &mut time);
    let reply = exchange(&mut port, 0, &mut time);
    assert_eq!((Packet(reply).family(), Packet(reply).payload()), (READ_CATALOG, 43));
    // The body names the next block it wants (as 0x13ed614 does), then 0xffff, then 0xff00.
    let mut headers = vec![];
    let mut blocks = vec![];
    let requests = (1u16..43).chain([0xffff, 0xff00]);
    for request in requests {
        let mut bytes = request.to_be_bytes().to_vec();
        bytes.extend(vec![0; CATALOG_BLOCK_BYTES]);
        bytes.push(0x5a);
        let out = transfer(&mut port, &bytes, &mut time);
        headers.push(u16::from_be_bytes([out[0], out[1]]));
        let sum = out[..CATALOG_BLOCK_BYTES + 2].iter().fold(0u8, |s, b| s.wrapping_add(*b));
        assert_eq!(out[CATALOG_BLOCK_BYTES + 2], sum);
        blocks.push(out[2..CATALOG_BLOCK_BYTES + 2].to_vec());
    }
    let expected: Vec<u16> = (0u16..43).chain([0xffff]).collect();
    assert_eq!(headers, expected);
    // Block count - 4 (39) is the first block stored at 0x720000; it carries the marker.
    assert_eq!((blocks[39][0x47], blocks[39][0x48]), (0x55, 0xaa), "marker in block 39");
    assert!(blocks[39][0x4a..].starts_with(b"XF18-55mmF2.8-4 R LM OIS\0"));
    assert_eq!(port.catalog_reads(), 1);
    assert_eq!(request(&mut port, STATUS, 0, &mut time), 0x04c0);
    // An undeclared selector serves one block.
    exchange(&mut port, Packet::encode(0, READ_CATALOG, false).0, &mut time);
    assert_eq!(Packet(exchange(&mut port, 0, &mut time)).payload(), 1);
}

/// The catalog as the X-T5 body stores it: configuration offset -> byte.
fn configuration(catalog: &Catalog) -> impl Fn(u32) -> u8 + '_ {
    let lens_blocks = catalog.blocks as u32 - 4;
    move |config: u32| {
        let at = if config >= 0x722000 {
            config - 0x722000
        } else {
            lens_blocks * CATALOG_BLOCK_BYTES as u32 + config - 0x720000
        };
        let (block, offset) = (at / CATALOG_BLOCK_BYTES as u32, at % CATALOG_BLOCK_BYTES as u32);
        assert!(block < catalog.blocks as u32);
        if catalog.marker_block == Some(block as u16) && (offset == 0x47 || offset == 0x48) {
            return [0x55, 0xaa][offset as usize - 0x47];
        }
        catalog.byte(block as u16, offset as usize)
    }
}
fn u16_at(read: &impl Fn(u32) -> u8, config: u32) -> u16 {
    u16::from_le_bytes([read(config), read(config + 1)])
}

#[test]
fn the_lens_data_carries_identity_names_and_serial_where_the_body_reads_them() {
    let spec = Spec::preset("xf23").unwrap();
    let record = &spec.identification;
    let read = configuration(&spec.catalogs[0]);
    let bytes = |from: u32, n: u32| (from..from + n).map(&read).collect::<Vec<u8>>();
    assert_eq!(bytes(0x720000, 8), record[0x10..0x18], "block +0 matches record +0x10");
    assert_eq!(bytes(0x720014, 10), record[0x50..0x5a], "serial matches record +0x50");
    assert_eq!(bytes(0x720038, 12), b"FUJIFILM\0\0\0\0", "Exif LensMake");
    assert_eq!(bytes(0x72004a, 16), b"XF23mmF2.8 R WR\0", "Exif LensModel");
    assert_eq!(read(0x720009), 0, "normal lens kind");
}

#[test]
fn the_lens_data_gives_the_x_t5_focal_length_and_f_number_tables() {
    // xf23: one zoom step, 23 mm, F2.8 to F16 in 1/3 stops (16 codes).
    let spec = Spec::preset("xf23").unwrap();
    let read = configuration(&spec.catalogs[0]);
    assert_eq!((read(0x720f80), read(0x720f82)), (1, 16));
    assert_eq!(u16_at(&read, 0x722134), 230, "focal length in 0.1 mm (Exif 23.0 mm)");
    assert_eq!(u16_at(&read, 0x7221fd), 230);
    assert_eq!((read(0x7222c8), read(0x72232d)), (0, 15), "widest and smallest code");
    // 0x1537e4c: F x 100 = table[index[31z + c]].
    let f = |z: u32, c: u32| u16_at(&read, 0x722530 + 2 * read(0x734600 + 31 * z + c) as u32);
    assert_eq!(f(0, 0), 280, "F2.8 wide open");
    assert_eq!(f(0, 15), 1600, "F16 smallest");
    assert_eq!(read(0x734600 + 16), 0xff, "no code past the smallest aperture");
    assert_eq!(u16_at(&read, 0x732400) as i16, 3000, "Av 3 at F2.8");
    assert_eq!(u16_at(&read, 0x732400 + 2 * 15) as i16, 8000, "Av 8 at F16");

    // xf18-55: 8 zoom steps from 18 to 55 mm, wide open F2.8 at the short end and F4 at the long end.
    let spec = Spec::preset("xf18-55").unwrap();
    let read = configuration(&spec.catalogs[0]);
    assert_eq!((read(0x720f80), read(0x720f82)), (8, 19));
    assert_eq!(u16_at(&read, 0x722134), 180);
    assert_eq!(u16_at(&read, 0x722134 + 2 * 7), 550);
    let f = |z: u32, c: u32| u16_at(&read, 0x722530 + 2 * read(0x734600 + 31 * z + c) as u32);
    assert_eq!(f(0, read(0x7222c8) as u32), 280);
    assert_eq!(f(7, read(0x7222c8 + 7) as u32), 400);
    assert_eq!(f(7, read(0x72232d + 7) as u32), 2200);
}

#[test]
fn the_lens_data_focus_tables_rise_from_infinity_inside_the_travel() {
    let spec = Spec::preset("xf23").unwrap();
    let read = configuration(&spec.catalogs[0]);
    let infinity = u16_at(&read, 0x720317);
    let offsets: Vec<u16> = [0x7203e0, 0x7204a9, 0x720572, 0x72063b, 0x720704, 0x7207cd]
        .iter()
        .map(|t| u16_at(&read, *t))
        .collect();
    assert!(offsets.windows(2).all(|w| w[0] <= w[1]), "{offsets:?}");
    assert!(infinity > spec.focus.min && infinity + offsets[5] < spec.focus.max);
    // The contrast scan runs from infinity to infinity + [+0x572] (0x153b6b8, 0x1539a30): most
    // of the travel, with margins (0x153e704) that keep infinity - margin inside it.
    let travel = spec.focus.max - spec.focus.min;
    assert!(offsets[2] > travel * 3 / 4, "{offsets:?}");
    for margin in [0x720cd2, 0x720d9b, 0x720b40, 0x720c09] {
        let margin = u16_at(&read, margin);
        assert!(margin > 0 && infinity - margin >= spec.focus.min);
    }
    // Nonzero scan speeds (0x1539fb4) and speed limits (0x1539c98, 0x1539cd0).
    for table in [0x721674, 0x721a84, 0x7215a6, 0x7215a8] {
        assert!(u16_at(&read, table) > 0);
    }
    // 0x1539228 uses the +0x63b and +0x704 tables only for zoom steps below +0x1213.
    assert_eq!(read(0x721213), read(0x720f80));
}

#[test]
fn every_preset_catalog_fits_the_x_t5_lens_data_window() {
    for name in ["xf18-55", "prime", "xf23"] {
        let catalog = &Spec::preset(name).unwrap().catalogs[0];
        // 0x1494e70: the lens-data window ends at 0x735cff.
        assert!(0x722000 + (catalog.blocks as u32 - 4) * CATALOG_BLOCK_BYTES as u32 <= 0x735d00);
        assert_eq!(catalog.marker_block, Some(catalog.blocks - 4));
    }
}
