use super::super::{
    ArgumentKind, Catalog, Command, CommandCode, CommandError, CommandReply, PreparedCommand,
    Reply, ReplyValue,
};
use super::{
    AdjustmentPacketAction, AdjustmentPacketPayload2, AdjustmentPacketPayload3,
    AdjustmentPacketPayload8, AdjustmentPacketRead, AdjustmentPacketReadData,
    AdjustmentPacketWrite, ExposureDuration, ExposureModeOption, ExposureOption, ExposureReadiness,
    ExposureStatusOption,
};
use fujimod_catalog_firmware_identity::FirmwareIdentity;
use fujimod_catalog_service::{Definition, ServiceModes};

fn catalog(reverse: bool) -> Catalog {
    let entries: Vec<_> = [
        ("adjustment_status_option", 1),
        ("adjustment_exposure", 4),
        ("adjustment_packet", 11),
        ("adjustment_ready_option", 1),
        ("report_service_success", 0),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (codec, count))| {
        let slots: Vec<_> = (0..count)
            .map(|slot| if reverse { 31 - slot } else { slot })
            .collect();
        serde_json::json!({
            "code": 0x90 + index,
            "codec": codec,
            "label": codec,
            "slots": slots,
            "enabled_modes": 4,
            "next_mode": 0,
            "firmware_kind": 1,
            "handler": 0,
            "parameter_table": 0
        })
    })
    .collect();
    let doc = serde_json::json!({
        "schema": 1,
        "firmware": {
            "id": "exposure-b",
            "model": "Synthetic",
            "version": "2",
            "sha256": "ab".repeat(32)
        },
        "mailbox": {"page": 3, "offset": 576, "configuration_limit": 512},
        "entries": entries
    });
    Catalog::new(
        Definition::from_json(
            &doc.to_string(),
            &FirmwareIdentity::new("exposure-b", &"ab".repeat(32)).unwrap(),
        )
        .unwrap(),
    )
}

fn prepare(catalog: &Catalog, command: Command) -> PreparedCommand {
    catalog
        .prepare(ServiceModes::new(4).unwrap(), command)
        .unwrap()
}

fn logical(prepared: &PreparedCommand) -> Vec<u8> {
    prepared
        .slots
        .iter()
        .map(|&slot| prepared.request.arguments()[usize::from(slot)])
        .collect()
}

fn decode(prepared: &PreparedCommand, logical_reply: &[u8]) -> CommandReply {
    assert_eq!(prepared.slots.len(), logical_reply.len());
    let mut raw = prepared.request.encode();
    for (&slot, &value) in prepared.slots.iter().zip(logical_reply) {
        raw[1 + usize::from(slot)] = value;
    }
    prepared.decode(Reply::decode(&raw).unwrap()).unwrap()
}

#[test]
fn status_and_ready_option_reads_use_one_output_slot_and_preserve_raw_values() {
    for reverse in [false, true] {
        let catalog = catalog(reverse);
        let status = prepare(&catalog, Command::ReadAdjustmentStatusOption);
        assert_eq!(status.request.code(), CommandCode(0x90));
        assert_eq!(logical(&status), [u8::MAX]);
        for raw in 0..=255 {
            let value = match raw {
                0 => ExposureStatusOption::Option0,
                1 => ExposureStatusOption::Option1,
                value => ExposureStatusOption::Unknown { value },
            };
            assert_eq!(
                decode(&status, &[raw]).value(),
                &ReplyValue::AdjustmentStatusOption { value }
            );
        }

        let ready = prepare(&catalog, Command::ReadAdjustmentReadyOption);
        assert_eq!(ready.request.code(), CommandCode(0x93));
        assert_eq!(logical(&ready), [u8::MAX]);
        for raw in 0..=255 {
            let value = match raw {
                0 => ExposureReadiness::NotReady,
                1 => ExposureReadiness::Ready,
                value => ExposureReadiness::Unknown { value },
            };
            assert_eq!(
                decode(&ready, &[raw]).value(),
                &ReplyValue::AdjustmentReadyOption { value }
            );
        }
    }
}

#[test]
fn exposure_request_splits_duration_in_firmware_byte_order_without_host_wait_semantics() {
    for reverse in [false, true] {
        let catalog = catalog(reverse);
        for (mode, mode_byte) in [
            (ExposureModeOption::Default, 0),
            (ExposureModeOption::Mode1, 1),
            (ExposureModeOption::Mode2, 2),
        ] {
            for (option, option_byte) in
                [(ExposureOption::Disabled, 0), (ExposureOption::Enabled, 1)]
            {
                for duration_value in [0, 1, 0x1234, u16::MAX] {
                    let duration = ExposureDuration::new(duration_value);
                    assert_eq!(duration.value(), duration_value);
                    let prepared = prepare(
                        &catalog,
                        Command::RequestExposure {
                            mode,
                            duration,
                            option,
                        },
                    );
                    assert_eq!(prepared.request.code(), CommandCode(0x91));
                    assert_eq!(
                        logical(&prepared),
                        [
                            mode_byte,
                            (duration_value >> 8) as u8,
                            duration_value as u8,
                            option_byte
                        ]
                    );
                    assert_eq!(
                        decode(&prepared, &logical(&prepared)).value(),
                        &ReplyValue::NoPayload
                    );
                }
            }
        }
    }
}

#[test]
fn configuration_packet_writes_use_exact_payload_lengths_and_reads_decode_published_bytes() {
    for reverse in [false, true] {
        let catalog = catalog(reverse);
        let writes = [
            (
                AdjustmentPacketWrite::Kind0 {
                    payload: AdjustmentPacketPayload8::new(&[1, 2, 3, 4, 5, 6, 7, 8]).unwrap(),
                },
                0,
                vec![1, 2, 3, 4, 5, 6, 7, 8],
                8,
            ),
            (
                AdjustmentPacketWrite::Kind1 { value: 0xa1 },
                1,
                vec![0xa1],
                1,
            ),
            (
                AdjustmentPacketWrite::Kind2 {
                    payload: AdjustmentPacketPayload3::new(&[0xb1, 0xb2, 0xb3]).unwrap(),
                },
                2,
                vec![0xb1, 0xb2, 0xb3],
                3,
            ),
            (
                AdjustmentPacketWrite::Kind3 { value: 0xc1 },
                3,
                vec![0xc1],
                1,
            ),
            (
                AdjustmentPacketWrite::Kind4 {
                    payload: AdjustmentPacketPayload2::new(&[0xd1, 0xd2]).unwrap(),
                },
                4,
                vec![0xd1, 0xd2],
                2,
            ),
        ];
        for (packet, kind, data, length) in writes {
            let prepared = prepare(
                &catalog,
                Command::AdjustmentPacket {
                    action: AdjustmentPacketAction::Write { packet },
                },
            );
            assert_eq!(prepared.request.code(), CommandCode(0x92));
            assert_eq!(
                logical(&prepared),
                [vec![kind], data, vec![0; 10 - length]].concat()
            );
            assert_eq!(
                decode(&prepared, &logical(&prepared)).value(),
                &ReplyValue::NoPayload
            );
        }

        let reads = [
            (
                AdjustmentPacketRead::Kind20,
                0x20,
                vec![0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0, 0, 0],
                AdjustmentPacketReadData::Kind20 {
                    data: [0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17],
                },
            ),
            (
                AdjustmentPacketRead::Kind21,
                0x21,
                vec![0x21, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                AdjustmentPacketReadData::Kind21 { value: 0x21 },
            ),
            (
                AdjustmentPacketRead::Kind22,
                0x22,
                vec![0x31, 0x32, 0x33, 0, 0, 0, 0, 0, 0, 0],
                AdjustmentPacketReadData::Kind22 {
                    data: [0x31, 0x32, 0x33],
                },
            ),
            (
                AdjustmentPacketRead::Kind23,
                0x23,
                vec![0x41, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                AdjustmentPacketReadData::Kind23 { value: 0x41 },
            ),
        ];
        for (action, kind, output, data) in reads {
            let prepared = prepare(
                &catalog,
                Command::AdjustmentPacket {
                    action: AdjustmentPacketAction::Read { kind: action },
                },
            );
            assert_eq!(prepared.request.code(), CommandCode(0x92));
            assert_eq!(logical(&prepared), [vec![kind], vec![u8::MAX; 10]].concat());
            assert_eq!(
                decode(&prepared, &[vec![kind], output].concat()).value(),
                &ReplyValue::AdjustmentPacketRead { data }
            );
        }
    }
}

#[test]
fn configuration_packet_payload_wrappers_require_exact_lengths() {
    let payload8 = AdjustmentPacketPayload8::new(&[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
    let payload3 = AdjustmentPacketPayload3::new(&[9, 10, 11]).unwrap();
    let payload2 = AdjustmentPacketPayload2::new(&[12, 13]).unwrap();
    assert_eq!(payload8.data(), &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(payload3.data(), &[9, 10, 11]);
    assert_eq!(payload2.data(), &[12, 13]);
    assert!(matches!(
        AdjustmentPacketPayload8::new(&[1; 7]),
        Err(CommandError::Argument(ArgumentKind::AdjustmentPacketLength))
    ));
    assert!(matches!(
        AdjustmentPacketPayload3::new(&[1; 4]),
        Err(CommandError::Argument(ArgumentKind::AdjustmentPacketLength))
    ));
    assert!(matches!(
        AdjustmentPacketPayload2::new(&[1]),
        Err(CommandError::Argument(ArgumentKind::AdjustmentPacketLength))
    ));
}

#[test]
fn report_success_has_no_arguments_and_no_implicit_result_policy() {
    for reverse in [false, true] {
        let catalog = catalog(reverse);
        let prepared = prepare(&catalog, Command::ReportServiceSuccess);
        assert_eq!(prepared.request.code(), CommandCode(0x94));
        assert!(prepared.slots.is_empty());
        assert_eq!(prepared.request.arguments(), &[0; 32]);
        assert_eq!(decode(&prepared, &[]).value(), &ReplyValue::NoPayload);
    }
}
