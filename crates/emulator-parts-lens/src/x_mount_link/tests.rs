use super::*;

fn configuration() -> Configuration {
    Configuration {
        peer_parameters: 0xb801,
        parameters: 0xb801,
        identification: (0..128).collect(),
        status: 0x04c1,
        aperture_ring: 0,
        focus_ring: 0xffff,
        detail_status: 0x14,
        read_catalogs: vec![],
    }
}
fn frame(byte: u8) -> Frame {
    Frame {
        bits: 8,
        mode: 3,
        word: byte as u16,
    }
}
fn transfer(link: &mut Link, input: &[u8], time: &mut u64) -> Result<Vec<u8>, String> {
    input
        .iter()
        .map(|byte| {
            let frame = frame(*byte);
            let token = link.begin(frame)?;
            *time += 64;
            link.finish(token, frame, *time).map(|v| v as u8)
        })
        .collect()
}
fn word(link: &mut Link, input: u32, time: &mut u64) -> u32 {
    u32::from_be_bytes(
        transfer(link, &input.to_be_bytes(), time)
            .unwrap()
            .try_into()
            .unwrap(),
    )
}
fn startup(link: &mut Link, time: &mut u64) {
    let sync = Packet::encode(0, SYNC, false);
    assert_eq!(word(link, sync.0, time), 0);
    assert_eq!(
        word(link, Packet::encode(0x0810, 0, true).0, time),
        Packet::encode(0x0800, SYNC, true).0
    );

    let idle = Packet::encode(0, 0, false);
    assert_eq!(word(link, idle.0, time), sync.0);
    assert_eq!(
        word(link, Packet::encode(0x0800, SYNC, true).0, time),
        Packet::encode(0x0800, 0, true).0
    );

    let parameters = Packet::encode(link.configuration.peer_parameters, PARAMETERS, false);
    assert_eq!(word(link, parameters.0, time), 0);
    assert_eq!(
        word(link, Packet::encode(0x0910, 0, true).0, time),
        Packet::encode(0x0800, PARAMETERS, true).0
    );

    let identification = Packet::encode(0, IDENTIFICATION, false);
    assert_eq!(word(link, identification.0, time), parameters.0);
    assert_eq!(
        word(link, Packet::encode(0x0a00, PARAMETERS, true).0, time),
        Packet::encode(0x0900, IDENTIFICATION, true).0
    );

    assert_eq!(word(link, idle.0, time), identification.0);
    assert_eq!(
        word(link, Packet::encode(0x0a00, IDENTIFICATION, true).0, time),
        Packet::encode(0x0a00, 0, true).0
    );
}
fn bulk(header: [u8; 2]) -> Vec<u8> {
    let mut bytes = header.to_vec();
    bytes.extend([0; 128]);
    bytes.push(0x5a);
    bytes
}

#[test]
fn startup_handshake_then_register_independent_identification_data() {
    let mut link = Link::new(configuration()).unwrap();
    let mut time = 0;
    startup(&mut link, &mut time);
    for (input, output_header) in [([255, 255], [0, 0]), ([255, 0], [255, 255])] {
        let output = transfer(&mut link, &bulk(input), &mut time).unwrap();
        assert_eq!(&output[..2], &output_header);
        assert_eq!(&output[2..130], configuration().identification);
        assert_eq!(
            output[130],
            output[..130]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
        );
    }
    // An unimplemented aperture-control family still refuses.
    assert!(
        transfer(
            &mut link,
            &Packet::encode(0, 0x60, false).bytes(),
            &mut time
        )
        .unwrap_err()
        .contains("request is unmodeled")
    );
}

#[test]
fn ring_and_detail_reads_keep_their_tags_and_external_data_in_any_order() {
    let mut config = configuration();
    config.aperture_ring = 0x1234;
    config.focus_ring = 0x9876;
    config.detail_status = 0x4321;
    let mut link = Link::new(config.clone()).unwrap();
    let mut time = 0;
    let values = [
        (DETAIL_STATUS, config.detail_status),
        (FOCUS_RING, config.focus_ring),
        (APERTURE_RING, config.aperture_ring),
    ];
    for sequence in 0..18 {
        let counter = sequence % 8;
        let (family, value) = values[sequence as usize % values.len()];
        assert_eq!(
            word(&mut link, Packet::encode(0, family, false).0, &mut time),
            0
        );
        assert_eq!(
            word(
                &mut link,
                Packet::encode(0x0810 | counter << 8, 0, true).0,
                &mut time
            ),
            Packet::encode(0x0800, family, true).0
        );
        let reply = word(&mut link, 0, &mut time);
        assert_eq!(reply >> 16, value as u32);
        assert_eq!((reply & 0x7fc0) >> 6, family as u32);
        assert_eq!(
            word(&mut link, Packet::encode(0x0800, family, true).0, &mut time),
            Packet::encode(0x0800 | counter << 8, 0, true).0
        );
    }
}

#[test]
fn catalog_reads_use_declared_banks_and_continue_a_partial_block_after_restore() {
    let mut config = configuration();
    config.read_catalogs = vec![
        ReadCatalog {
            selector: 6,
            attributes: 3,
            data: (0..4096)
                .map(|i| ((i * 17 + i / 2048) & 255) as u8)
                .collect(),
        },
        ReadCatalog {
            selector: 1,
            attributes: 0,
            data: vec![0xa7; 2048],
        },
    ];
    let mut link = Link::new(config.clone()).unwrap();
    let mut time = 0;
    for (sequence, catalog) in config.read_catalogs.iter().enumerate() {
        assert_eq!(
            word(
                &mut link,
                Packet::encode(catalog.selector as u16, READ_CATALOG, false).0,
                &mut time
            ),
            0
        );
        assert_eq!(
            word(
                &mut link,
                Packet::encode(0x0810 | (sequence as u16) << 8, 0, true).0,
                &mut time
            ),
            Packet::encode(0x0800, READ_CATALOG, true).0
        );
        assert_eq!(
            word(&mut link, 0, &mut time),
            Packet::encode(catalog.word(), READ_CATALOG, false).0
        );
        assert_eq!(
            word(
                &mut link,
                Packet::encode(0x0800, READ_CATALOG, true).0,
                &mut time
            ),
            Packet::encode(0x0800 | (sequence as u16) << 8, 0, true).0
        );
        let count = catalog.data.len() / 2048;
        for block in 0..=count {
            let requested: u16 = if block < count - 1 {
                (block + 1) as u16
            } else if block == count - 1 {
                0xffff
            } else {
                0xff00
            };
            let returned: u16 = if block < count { block as u16 } else { 0xffff };
            let mut request = requested.to_be_bytes().to_vec();
            request.extend([0; 2048]);
            request.push(0x5a);
            let mut output = transfer(&mut link, &request[..37], &mut time).unwrap();
            let state = link.save();
            assert_eq!(link.save(), state);
            let mut restored = Link::new(config.clone()).unwrap();
            restored.restore(&state).unwrap();
            let mut restored_time = time;
            let suffix = transfer(&mut link, &request[37..], &mut time).unwrap();
            assert_eq!(
                transfer(&mut restored, &request[37..], &mut restored_time).unwrap(),
                suffix
            );
            assert_eq!(restored.save(), link.save());
            output.extend(suffix);
            let offset = block.min(count - 1) * 2048;
            assert_eq!(&output[..2], &returned.to_be_bytes());
            assert_eq!(&output[2..2050], &catalog.data[offset..offset + 2048]);
            assert_eq!(
                output[2050],
                output[..2050]
                    .iter()
                    .fold(0u8, |sum, b| sum.wrapping_add(*b))
            );
        }
    }
    assert!(
        transfer(
            &mut link,
            &Packet::encode(7, READ_CATALOG, false).bytes(),
            &mut time
        )
        .unwrap_err()
        .contains("not supplied")
    );
}

#[test]
fn catalog_configuration_rejects_unknown_geometry_metadata_and_duplicate_selectors() {
    let bank = ReadCatalog {
        selector: 6,
        attributes: 0,
        data: vec![0; 2048],
    };
    let mut cases = vec![
        ReadCatalog {
            data: vec![],
            ..bank.clone()
        },
        ReadCatalog {
            data: vec![0; 2047],
            ..bank.clone()
        },
        ReadCatalog {
            selector: 16,
            ..bank.clone()
        },
        ReadCatalog {
            attributes: 32,
            ..bank.clone()
        },
    ];
    for catalog in cases.drain(..) {
        let mut config = configuration();
        config.read_catalogs = vec![catalog];
        assert!(Link::new(config).is_err());
    }
    let mut config = configuration();
    config.read_catalogs = vec![bank.clone(), bank];
    assert!(Link::new(config).is_err());
}

#[test]
fn bounded_control20_writes_echo_and_latch_only_at_an_acknowledged_sync() {
    let mut link = Link::new(configuration()).unwrap();
    let mut time = 0;
    assert_eq!(link.save()["state"]["control20"], Value::Null);
    for (sequence, family, value) in [
        (0, CONTROL20, 1),
        (1, SYNC, 0),
        (2, CONTROL20, 0),
        (3, STATUS, 0),
        (4, SYNC, 0),
    ] {
        let before = link.save()["state"]["control20"].clone();
        assert_eq!(
            word(&mut link, Packet::encode(value, family, false).0, &mut time),
            0
        );
        assert_eq!(link.save()["state"]["control20"], before);
        assert_eq!(
            word(
                &mut link,
                Packet::encode(0x0810 | sequence << 8, 0, true).0,
                &mut time
            ),
            Packet::encode(0x0800, family, true).0
        );
        let reply = word(&mut link, 0, &mut time);
        assert_eq!(
            reply,
            Packet::encode(if family == STATUS { 0x04c1 } else { value }, family, false).0
        );
        assert_eq!(
            word(&mut link, Packet::encode(0x0800, family, true).0, &mut time),
            Packet::encode(0x0800 | sequence << 8, 0, true).0
        );
        if family == CONTROL20 {
            assert_eq!(link.save()["state"]["control20"], before);
            assert_eq!(link.save()["state"]["staged_control20"], value);
            let saved = link.save();
            let mut restored = Link::new(configuration()).unwrap();
            restored.restore(&saved).unwrap();
            assert_eq!(restored.save(), saved);
        } else if family == SYNC {
            assert_eq!(
                link.save()["state"]["control20"],
                if sequence == 1 { 1 } else { 0 }
            );
            assert_eq!(link.save()["state"]["staged_control20"], Value::Null);
        }
    }
    let saved = link.save();
    for field in ["control20", "staged_control20"] {
        let mut bad = saved.clone();
        bad["state"][field] = json!(2);
        assert!(link.restore(&bad).is_err());
        assert_eq!(link.save(), saved);
    }
    assert!(
        transfer(
            &mut link,
            &Packet::encode(2, CONTROL20, false).bytes(),
            &mut time
        )
        .unwrap_err()
        .contains("unmodeled")
    );
    link.reset();
    assert_eq!(link.save()["state"]["control20"], Value::Null);
    assert_eq!(link.save()["state"]["staged_control20"], Value::Null);
}

#[test]
fn status_reads_use_external_data_and_normal_queue_counters() {
    for status in [0x04c1, 0x0080, 0x0010] {
        let mut config = configuration();
        config.status = status;
        let mut link = Link::new(config).unwrap();
        let mut time = 0;
        // Polling is allowed in any message order, including before sync/ID.
        for sequence in 0..18 {
            let counter = sequence % 8;
            assert_eq!(word(&mut link, 0x00000820, &mut time), 0);
            let ack = Packet::encode(0x0810 | counter << 8, 0, true);
            assert_eq!(word(&mut link, ack.0, &mut time), 0x08008832);
            let output = word(&mut link, 0, &mut time);
            assert_eq!(output >> 16, status as u32);
            assert_eq!(output & 0xffc0, 0x0800);
            let ack = Packet::encode(0x0800, STATUS, true);
            assert_eq!(
                word(&mut link, ack.0, &mut time),
                Packet::encode(0x0800 | counter << 8, 0, true).0
            );
        }
    }
}

#[test]
fn configured_part_data_and_message_order_determine_replies() {
    let mut config = configuration();
    config.parameters = 0x1234;
    let mut link = Link::new(config).unwrap();
    let mut time = 0;
    // This request can be first, repeat after acknowledgment, and return a
    // different part's declared parameters. No transaction index is matched.
    for count in 0..10 {
        let seq = count & 7;
        assert_eq!(word(&mut link, 0xb8010626, &mut time), 0);
        let ack = Packet::encode(0x0800, PARAMETERS, true).0;
        let peer_ack = Packet::encode(0x0810 | seq << 8, 0, true).0;
        assert_eq!(word(&mut link, peer_ack, &mut time), ack);
        assert_eq!(
            word(&mut link, 0, &mut time),
            Packet::encode(0x1234, PARAMETERS, false).0
        );
        let idle_ack = Packet::encode(0x0800 | seq << 8, 0, true).0;
        assert_eq!(word(&mut link, 0x0800862a, &mut time), idle_ack);
    }
    link.reset();
    assert_eq!(link.save()["state"]["frames"], 0);
    assert_eq!(word(&mut link, 0xb8010626, &mut time), 0);
    assert_eq!(word(&mut link, 0x08108022, &mut time), 0x0800862a);
}

#[test]
fn partial_packet_and_record_restore_continue_without_host_effects() {
    let mut source = Link::new(configuration()).unwrap();
    let mut time = 0;
    startup(&mut source, &mut time);
    let request = bulk([255, 255]);
    transfer(&mut source, &request[..37], &mut time).unwrap();
    let saved = source.save();
    for _ in 0..3 {
        assert_eq!(source.save(), saved);
    }
    let mut resumed = Link::new(configuration()).unwrap();
    resumed.restore(&saved).unwrap();
    let mut other_time = time;
    assert_eq!(
        transfer(&mut source, &request[37..], &mut time).unwrap(),
        transfer(&mut resumed, &request[37..], &mut other_time).unwrap()
    );
    assert_eq!(source.save(), resumed.save());
    let request = bulk([255, 0]);
    assert_eq!(
        transfer(&mut source, &request, &mut time).unwrap(),
        transfer(&mut resumed, &request, &mut other_time).unwrap()
    );
    assert_eq!(source.save(), resumed.save());
    transfer(&mut source, &[0xb8, 1], &mut time).unwrap();
    resumed.restore(&source.save()).unwrap();
    other_time = time;
    assert_eq!(
        transfer(&mut source, &[6, 0x26], &mut time).unwrap(),
        transfer(&mut resumed, &[6, 0x26], &mut other_time).unwrap()
    );
    assert_eq!(source.save(), resumed.save());
}

#[test]
fn invalid_wire_packets_and_checkpoints_do_not_publish_partial_state() {
    let mut link = Link::new(configuration()).unwrap();
    let mut time = 0;
    transfer(&mut link, &[0xb8, 1, 6], &mut time).unwrap();
    let saved = link.save();
    assert!(
        transfer(&mut link, &[0x27], &mut time)
            .unwrap_err()
            .contains("checksum/framing")
    );
    assert_eq!(link.save(), saved);
    let mut bad = saved.clone();
    bad["state"]["receive_counter"] = json!(8);
    assert!(link.restore(&bad).is_err());
    assert_eq!(link.save(), saved);
    let mut bad = saved.clone();
    bad["configuration"]["parameters"] = json!(1);
    assert!(link.restore(&bad).is_err());
    assert_eq!(link.save(), saved);
    assert!(
        link.begin(Frame {
            mode: 0,
            ..frame(0)
        })
        .is_err()
    );
    assert!(link.finish(999, frame(0), time).is_err());
    assert_eq!(link.save(), saved);
    transfer(&mut link, &[0x26], &mut time).unwrap();
    assert_eq!(word(&mut link, 0x08108022, &mut time), 0x0800862a);
    word(&mut link, 0, &mut time);
    let prior = link.save();
    assert!(transfer(&mut link, &[0x08, 0, 0x85], &mut time).is_ok());
    let partial = link.save();
    assert!(
        transfer(&mut link, &[0x26], &mut time)
            .unwrap_err()
            .contains("no matching sent header")
    );
    assert_eq!(link.save(), partial);
    link.restore(&prior).unwrap();
    assert_eq!(word(&mut link, 0x0800862a, &mut time), 0x08008012);
}

#[test]
fn header_delivery_waits_for_the_peer_acknowledgment_and_sync_does_not_reset_counters() {
    let mut link = Link::new(configuration()).unwrap();
    let mut time = 0;
    assert_eq!(word(&mut link, 0xb8010626, &mut time), 0);
    let pending = link.save();
    assert_eq!(pending["state"]["transmit_counter"], 0);
    let wrong = Packet::encode(0x0910, 0, true).bytes();
    assert!(
        transfer(&mut link, &wrong, &mut time)
            .unwrap_err()
            .contains("unexpected message counter")
    );
    assert_eq!(link.save()["state"]["transmit_counter"], 0);
    link.restore(&pending).unwrap();
    assert_eq!(word(&mut link, 0x08108022, &mut time), 0x0800862a);
    // While the part sends its parameter reply (sequence zero), the peer
    // sends sync as its second message. The ACK must carry zero, not one.
    assert_eq!(word(&mut link, 0x00003fc6, &mut time), 0xb8010626);
    assert_eq!(
        word(
            &mut link,
            Packet::encode(0x0900, PARAMETERS, true).0,
            &mut time
        ),
        0x0800bfd8
    );
    assert_eq!(word(&mut link, 0, &mut time), 0x00003fc6);
    assert_eq!(
        word(&mut link, 0x0800bfd8, &mut time),
        Packet::encode(0x0900, 0, true).0
    );
    assert_eq!(link.save()["state"]["receive_counter"], 2);
    assert_eq!(link.save()["state"]["transmit_counter"], 2);
}

#[test]
fn typed_checkpoint_restores_partial_headers_and_validates_pending_tokens() {
    let mut link = Link::new(configuration()).unwrap();
    let mut time = 0;
    transfer(&mut link, &[0, 0], &mut time).unwrap();
    let saved = link.checkpoint().unwrap();
    let mut restored = Link::new(configuration()).unwrap();
    restored.restore_checkpoint(&saved).unwrap();
    assert_eq!(restored.save(), link.save());
    assert!(restored.validate_pending(&saved, 2, frame(0x3f)).is_ok());
    assert!(restored.validate_pending(&saved, 3, frame(0x3f)).is_err());
    let mut resumed_time = time;
    assert_eq!(
        transfer(&mut link, &[0x3f, 0xc6], &mut time).unwrap(),
        transfer(&mut restored, &[0x3f, 0xc6], &mut resumed_time).unwrap()
    );
    assert_eq!(link.save(), restored.save());
    let before = restored.save();
    assert!(
        restored
            .restore_checkpoint(&saved.clone().with_children(vec![saved.clone()]))
            .is_err()
    );
    assert_eq!(restored.save(), before);
    let mut different = configuration();
    different.parameters = 0xb800;
    assert!(
        Link::new(different)
            .unwrap()
            .validate_checkpoint(&saved)
            .is_err()
    );
}
