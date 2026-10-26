use super::*;
use fujimod_catalog_firmware_identity::FirmwareIdentity;
fn catalog(reverse: bool) -> Catalog {
    let layouts = [
        ("transfer_prepare", 1),
        ("transfer_select_operation", 2),
        ("transfer_ready", 1),
        ("transfer_select_channel", 2),
        ("transfer_start", 1),
        ("transfer_word", 10),
        ("transfer_pair", 7),
        ("transfer_bitfields", 5),
        ("transfer_byte", 3),
        ("transfer_mode_byte", 3),
        ("transfer_halfword", 7),
        ("transfer_option_word", 6),
        ("transfer_selected_word", 4),
        ("transfer_word7", 3),
        ("transfer_word16", 3),
        ("transfer_load_test", 1),
    ];
    let entries:Vec<_>=layouts.into_iter().enumerate().map(|(n,(codec,count))|{
        let slots:Vec<_>=(0..count).map(|i|if reverse {31-i}else{i}).collect();
        serde_json::json!({"code":180+n,"codec":codec,"label":codec,"slots":slots,"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0})
    }).collect();
    let json = serde_json::json!({"schema":1,"firmware":{"id":"transfer-b","model":"Synthetic","version":"2","sha256":"ab".repeat(32)},"mailbox":{"page":3,"offset":576,"configuration_limit":512},"entries":entries});
    Catalog::new(
        Definition::from_json(
            &json.to_string(),
            &FirmwareIdentity::new("transfer-b", &"ab".repeat(32)).unwrap(),
        )
        .unwrap(),
    )
}
#[test]
fn canonical_transfer_selectors_and_outputs_are_firmware_selected() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (command, code, expected, index) in [
            (Command::TransferPrepare, 180, vec![255], 0),
            (Command::TransferReady, 182, vec![255], 0),
            (Command::TransferStart, 184, vec![255], 0),
            (
                Command::TransferSelectOperation {
                    operation: TransferOperation::Operation0,
                },
                181,
                vec![0, 255],
                1,
            ),
            (
                Command::TransferSelectOperation {
                    operation: TransferOperation::Operation1,
                },
                181,
                vec![1, 255],
                1,
            ),
            (
                Command::TransferSelectOperation {
                    operation: TransferOperation::Operation2,
                },
                181,
                vec![2, 255],
                1,
            ),
            (
                Command::TransferSelectOperation {
                    operation: TransferOperation::Operation3,
                },
                181,
                vec![3, 255],
                1,
            ),
            (
                Command::TransferSelectOperation {
                    operation: TransferOperation::Operation6,
                },
                181,
                vec![255, 255],
                1,
            ),
            (
                Command::TransferSelectChannel {
                    channel: TransferChannel::Operation1,
                },
                183,
                vec![0, 255],
                1,
            ),
            (
                Command::TransferSelectChannel {
                    channel: TransferChannel::Operation3,
                },
                183,
                vec![1, 255],
                1,
            ),
            (
                Command::TransferSelectChannel {
                    channel: TransferChannel::Operation4,
                },
                183,
                vec![2, 255],
                1,
            ),
        ] {
            let p = c.prepare(ServiceModes::new(4).unwrap(), command).unwrap();
            assert_eq!(p.request.code(), CommandCode(code));
            let logical: Vec<_> = p
                .slots
                .iter()
                .map(|&s| p.request.arguments()[usize::from(s)])
                .collect();
            assert_eq!(logical, expected);
            for raw in 0..=255 {
                let mut bytes = p.request.encode();
                bytes[1 + usize::from(p.slots[index])] = raw;
                let reply = p.decode(Reply::decode(&bytes).unwrap()).unwrap();
                assert_eq!(reply.raw().raw(), &bytes);
                let result = match raw {
                    0 => TransferResult::Success,
                    1 => TransferResult::Failed,
                    n => TransferResult::Unknown(n),
                };
                let value = match command {
                    Command::TransferPrepare => ReplyValue::TransferPrepared {
                        result: match raw {
                            0 => TransferPrepareResult::Success,
                            2 => TransferPrepareResult::PreparationError,
                            3 => TransferPrepareResult::Status7,
                            n => TransferPrepareResult::Unknown(n),
                        },
                    },
                    Command::TransferReady => ReplyValue::TransferReadiness { result },
                    Command::TransferStart => ReplyValue::TransferStarted { result },
                    Command::TransferSelectOperation { .. } => {
                        ReplyValue::TransferOperationSelected { result }
                    }
                    Command::TransferSelectChannel { .. } => {
                        ReplyValue::TransferChannelSelected { result }
                    }
                    _ => unreachable!(),
                };
                assert_eq!(reply.value(), &value);
            }
        }
    }
}

fn prepare(c: &Catalog, command: Command) -> PreparedCommand {
    c.prepare(ServiceModes::new(4).unwrap(), command).unwrap()
}
fn logical(p: &PreparedCommand) -> Vec<u8> {
    p.slots
        .iter()
        .map(|&s| p.request.arguments()[usize::from(s)])
        .collect()
}
fn response(p: &PreparedCommand, updates: &[(usize, u8)]) -> CommandReply {
    let mut raw = p.request.encode();
    for &(slot, byte) in updates {
        raw[1 + usize::from(p.slots[slot])] = byte;
    }
    p.decode(Reply::decode(&raw).unwrap()).unwrap()
}
#[test]
fn transfer_word_space_is_bounded_and_results_do_not_accept_failed_payloads() {
    for address in [1, 2, 3, 0x10000, u32::MAX] {
        assert!(TransferWordAddress::new(address).is_err());
    }
    for reverse in [false, true] {
        let c = catalog(reverse);
        for address in [0, 0xfffc] {
            let address = TransferWordAddress::new(address).unwrap();
            let read = prepare(&c, Command::TransferReadWord { address });
            let bytes = address.value().to_be_bytes();
            assert_eq!(
                logical(&read),
                [
                    0, bytes[0], bytes[1], bytes[2], bytes[3], 255, 255, 255, 255, 255
                ]
            );
            let write = prepare(
                &c,
                Command::TransferWriteWord {
                    address,
                    value: 0x89abcdef,
                },
            );
            assert_eq!(
                logical(&write),
                [
                    1, bytes[0], bytes[1], bytes[2], bytes[3], 0x89, 0xab, 0xcd, 0xef, 255
                ]
            );
            for status in 0..=255 {
                let value = match status {
                    0 => TransferWordReadResult::Value {
                        raw_value: 0x89abcdef,
                    },
                    1 => TransferWordReadResult::Rejected,
                    n => TransferWordReadResult::Unknown { raw_status: n },
                };
                assert_eq!(
                    response(
                        &read,
                        &[(5, 0x89), (6, 0xab), (7, 0xcd), (8, 0xef), (9, status)]
                    )
                    .value(),
                    &ReplyValue::TransferWordRead { result: value }
                );
                let result = match status {
                    0 => TransferResult::Success,
                    1 => TransferResult::Failed,
                    n => TransferResult::Unknown(n),
                };
                assert_eq!(
                    response(&write, &[(9, status)]).value(),
                    &ReplyValue::TransferWordWritten { result }
                );
            }
        }
    }
}
#[test]
fn pair_bounds_masks_and_opaque_reply_bytes_stay_separate() {
    assert!(TransferPairFirst::new(512).is_err());
    for reverse in [false, true] {
        let c = catalog(reverse);
        let p = prepare(
            &c,
            Command::TransferPair {
                first: TransferPairFirst::new(511).unwrap(),
                second: 65535,
            },
        );
        assert_eq!(logical(&p), [1, 255, 255, 255, 255, 255, 255]);
        for status in 0..=255 {
            let result = match status {
                0 => TransferPairResult::Values {
                    first: 0x89,
                    second: 0xab,
                },
                3 => TransferPairResult::CallbackError,
                4 => TransferPairResult::Rejected,
                6 => TransferPairResult::WaitError,
                n => TransferPairResult::Unknown { raw_status: n },
            };
            assert_eq!(
                response(&p, &[(4, 0x89), (5, 0xab), (6, status)]).value(),
                &ReplyValue::TransferPair { result }
            );
        }
        let mode = TransferPayloadByte::new(255);
        let p = prepare(
            &c,
            Command::TransferBitfields {
                mode,
                first: 0x1234,
                second: 0xabcd,
            },
        );
        assert_eq!(logical(&p), [255, 0x12, 0x34, 0xab, 0xcd]);
        assert_eq!(response(&p, &[]).value(), &ReplyValue::NoPayload);
        for command in [
            Command::TransferByte { data: mode },
            Command::TransferModeByte { data: mode },
        ] {
            let p = prepare(&c, command);
            assert_eq!(logical(&p), [255, 255, 255]);
            assert_eq!(
                response(&p, &[(1, 0), (2, 255)]).value(),
                &ReplyValue::TransferBytes { data: [0, 255] }
            );
        }
    }
}
#[test]
fn halfword_options_and_selected_words_preserve_full_quantity_widths() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        let address = TransferHalfwordAddress::new(u32::MAX);
        let p = prepare(&c, Command::TransferReadHalfword { address });
        assert_eq!(logical(&p), [0, 255, 255, 255, 255, 255, 255]);
        assert_eq!(
            response(&p, &[(5, 0x89), (6, 0xab)]).value(),
            &ReplyValue::TransferHalfword { raw_value: 0x89ab }
        );
        let p = prepare(
            &c,
            Command::TransferWriteHalfword {
                address,
                value: 0x89ab,
            },
        );
        assert_eq!(logical(&p), [1, 255, 255, 255, 255, 0x89, 0xab]);
        assert_eq!(response(&p, &[]).value(), &ReplyValue::NoPayload);
        for (option, mode, bank) in [
            (TransferOption::Option0, 0, 0),
            (TransferOption::Option1, 1, 0),
            (TransferOption::Option2, 2, 0),
            (TransferOption::Option3, 3, 0),
            (TransferOption::Option4, 0, 1),
            (TransferOption::Option5, 1, 1),
            (TransferOption::Option6, 2, 1),
            (TransferOption::Option7, 3, 1),
        ] {
            let p = prepare(&c, Command::TransferReadOptionWord { option });
            assert_eq!(logical(&p), [mode, bank, 255, 255, 255, 255]);
            assert_eq!(
                response(&p, &[(2, 0x12), (3, 0x34), (4, 0x56), (5, 0x78)]).value(),
                &ReplyValue::TransferOptionWord {
                    raw_value: 0x12345678
                }
            );
        }
        for (kind, a, b) in [
            (TransferSelectedWordKind::Kind9, 2, 1),
            (TransferSelectedWordKind::Kind10, 2, 0),
            (TransferSelectedWordKind::Kind11, 1, 1),
            (TransferSelectedWordKind::Kind12, 1, 0),
            (TransferSelectedWordKind::Kind13, 0, 1),
            (TransferSelectedWordKind::Kind14, 0, 0),
        ] {
            let p = prepare(
                &c,
                Command::TransferSetSelectedWord {
                    kind,
                    value: 0x89ab,
                },
            );
            assert_eq!(logical(&p), [a, b, 0x89, 0xab]);
            assert_eq!(response(&p, &[]).value(), &ReplyValue::NoPayload);
        }
        for (command, bytes) in [
            (
                Command::TransferSetWord7 {
                    kind: TransferWord7Kind::Kind7,
                    value: 0x89ab,
                },
                [1, 0x89, 0xab],
            ),
            (
                Command::TransferSetWord7 {
                    kind: TransferWord7Kind::Kind8,
                    value: 0x89ab,
                },
                [0, 0x89, 0xab],
            ),
            (
                Command::TransferSetWord16 {
                    kind: TransferWord16Kind::Kind16,
                    value: 0x89ab,
                },
                [1, 0x89, 0xab],
            ),
            (
                Command::TransferSetWord16 {
                    kind: TransferWord16Kind::Kind17,
                    value: 0x89ab,
                },
                [0, 0x89, 0xab],
            ),
        ] {
            let p = prepare(&c, command);
            assert_eq!(logical(&p), bytes);
            assert_eq!(response(&p, &[]).value(), &ReplyValue::NoPayload);
        }
        for (mode, wire) in [
            (TransferLoadMode::Setup, 0),
            (TransferLoadMode::Run, 1),
            (TransferLoadMode::Request3One, 3),
            (TransferLoadMode::Request3Zero, 4),
        ] {
            let p = prepare(&c, Command::TransferLoadTest { mode });
            assert_eq!(logical(&p), [wire]);
            assert_eq!(response(&p, &[]).value(), &ReplyValue::NoPayload);
        }
    }
}
