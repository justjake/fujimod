use super::*;
use crate::Address;
use fujimod_catalog_firmware_identity::FirmwareIdentity;
fn modes() -> ServiceModes {
    ServiceModes::new(1).unwrap()
}
fn document(second: bool) -> serde_json::Value {
    let mut entries = vec![];
    for (i, codec) in [
        "write_pins",
        "read_pins",
        "read_channel",
        "write_channel",
        "profile_byte",
        "download_status",
        "copy_configuration",
        "flush_configuration",
        "memory_word",
    ]
    .into_iter()
    .enumerate()
    {
        let n = match codec {
            "write_pins" => 5,
            "read_pins" | "read_channel" | "write_channel" | "profile_byte" => 3,
            "memory_word" => 10,
            _ => 1,
        };
        let slots: Vec<u8> = (0..n)
            .map(|slot| if second { 31 - slot } else { slot })
            .collect();
        entries.push(serde_json::json!({"code":i+if second{100}else{0},"label":codec,"codec":codec,"slots":slots,"enabled_modes":1,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0}));
    }
    serde_json::json!({"schema":1,"firmware":{"id":if second{"body-b-v2"}else{"body-a-v1"},"model":"Synthetic","version":"test","sha256":"ab".repeat(32)},"mailbox":{"page":0,"offset":if second{384}else{128},"configuration_limit":256},"entries":entries})
}
fn catalog(second: bool) -> Catalog {
    let d = document(second);
    let id = d["firmware"]["id"].as_str().unwrap();
    Catalog::new(
        Definition::from_json(
            &d.to_string(),
            &FirmwareIdentity::new(id, &"ab".repeat(32)).unwrap(),
        )
        .unwrap(),
    )
}
fn bank(n: u8) -> PinBank {
    PinBank::new(n).unwrap()
}
fn response(prepared: &PreparedCommand, fields: &[(usize, u8)]) -> Reply {
    let mut raw = prepared.request().encode();
    for (index, value) in fields {
        raw[1 + usize::from(prepared.slots[*index])] = *value;
    }
    Reply::decode(&raw).unwrap()
}
#[test]
fn separate_firmwares_select_their_own_codes_and_argument_positions() {
    for second in [false, true] {
        let c = catalog(second);
        let p = c
            .prepare(
                modes(),
                Command::WritePins {
                    bank: bank(15),
                    levels: 0x1234,
                    exclude: 0xabcd,
                },
            )
            .unwrap();
        assert_eq!(
            p.request().code(),
            CommandCode(if second { 100 } else { 0 })
        );
        for (i, value) in [15, 0x12, 0x34, 0xab, 0xcd].into_iter().enumerate() {
            assert_eq!(p.request().arguments()[usize::from(p.slots[i])], value);
        }
        let p = c
            .prepare(modes(), Command::ReadPins { bank: bank(2) })
            .unwrap();
        assert_eq!(
            p.decode(response(&p, &[(1, 0xfe), (2, 0xdc)]))
                .unwrap()
                .value(),
            &ReplyValue::Pins { levels: 0xfedc }
        );
        let p = c
            .prepare(modes(), Command::ReadChannel { channel: 42 })
            .unwrap();
        assert_eq!(
            p.decode(response(&p, &[(1, 0x12), (2, 0x34)]))
                .unwrap()
                .value(),
            &ReplyValue::Channel { value: 0x1234 }
        );
    }
}
#[test]
fn profile_actions_are_variants_and_read_results_are_not_host_dispositions() {
    let c = catalog(false);
    for (bank, opcode) in [(ProfileBank::First, 1), (ProfileBank::Second, 2)] {
        let p = c
            .prepare(
                modes(),
                Command::ProfileByte {
                    action: ProfileAction::Read {
                        bank,
                        index: ProfileIndex::new(15).unwrap(),
                    },
                },
            )
            .unwrap();
        assert_eq!(&p.request().arguments()[..2], &[opcode, 15]);
        assert_eq!(
            p.decode(response(&p, &[(2, 1)])).unwrap().value(),
            &ReplyValue::ProfileByte { value: 1 }
        );
        let p = c
            .prepare(
                modes(),
                Command::ProfileByte {
                    action: ProfileAction::Write {
                        bank,
                        index: ProfileIndex::new(3).unwrap(),
                        value: 0x80,
                    },
                },
            )
            .unwrap();
        assert_eq!(&p.request().arguments()[..3], &[opcode + 2, 3, 0x80]);
    }
    let p = c
        .prepare(
            modes(),
            Command::ProfileByte {
                action: ProfileAction::Reset,
            },
        )
        .unwrap();
    assert_eq!(p.request().arguments()[0], 5);
    assert!(PinBank::new(16).is_err());
    assert!(ProfileIndex::new(16).is_err());
}
#[test]
fn unknown_save_and_download_results_are_retained_as_data() {
    let c = catalog(false);
    let p = c.prepare(modes(), Command::FlushConfiguration).unwrap();
    for (byte, result) in [
        (0, SaveStatus::Success),
        (1, SaveStatus::Failed),
        (42, SaveStatus::Unknown(42)),
    ] {
        let raw = response(&p, &[(0, byte)]);
        let r = p.decode(raw.clone()).unwrap();
        assert_eq!(
            r.value(),
            &ReplyValue::ConfigurationSaved { status: result }
        );
        assert_eq!(r.raw(), &raw);
    }
    let p = c.prepare(modes(), Command::DownloadStatus).unwrap();
    assert_eq!(
        p.decode(response(&p, &[(0, 1)])).unwrap().value(),
        &ReplyValue::DownloadStatus { raw_status: 1 }
    );
}
#[test]
fn service_word_bytes_are_big_endian_and_addresses_are_checked_before_use() {
    let c = catalog(false);
    let address = WordAddress::new(Address(0x40001234)).unwrap();
    let p = c
        .prepare(
            modes(),
            Command::WriteWord {
                address,
                value: 0x89abcdef,
            },
        )
        .unwrap();
    assert_eq!(
        &p.request().arguments()[..10],
        &[1, 0, 0x40, 0, 0x12, 0x34, 0x89, 0xab, 0xcd, 0xef]
    );
    assert_eq!(
        p.decode(response(&p, &[(1, 1)])).unwrap().value(),
        &ReplyValue::WriteWord {
            result: WordWriteResult::Confirmed
        }
    );
    let p = c.prepare(modes(), Command::ReadWord { address }).unwrap();
    assert_eq!(
        p.decode(response(
            &p,
            &[(1, 1), (6, 0x89), (7, 0xab), (8, 0xcd), (9, 0xef)]
        ))
        .unwrap()
        .value(),
        &ReplyValue::ReadWord {
            result: WordReadResult::Value { value: 0x89abcdef }
        }
    );
    assert_eq!(
        p.decode(response(&p, &[(1, 42)])).unwrap().value(),
        &ReplyValue::ReadWord {
            result: WordReadResult::Unknown(42)
        }
    );
    assert!(WordAddress::new(Address(1)).is_err());
    assert!(
        c.prepare(
            modes(),
            Command::ReadWord {
                address: WordAddress::new(Address(0xe0000000)).unwrap()
            }
        )
        .is_err()
    );
}
#[test]
fn missing_ambiguous_and_shadowed_commands_never_fall_back() {
    let c = catalog(false);
    assert!(
        c.prepare(
            ServiceModes::new(4).unwrap(),
            Command::ReadPins { bank: bank(0) }
        )
        .is_err()
    );
    let mut d = document(false);
    let mut duplicate = d["entries"][1].clone();
    duplicate["code"] = serde_json::json!(99);
    d["entries"].as_array_mut().unwrap().push(duplicate);
    let id = FirmwareIdentity::new("body-a-v1", &"ab".repeat(32)).unwrap();
    let c = Catalog::new(Definition::from_json(&d.to_string(), &id).unwrap());
    assert!(matches!(
        c.prepare(modes(), Command::ReadPins { bank: bank(0) }),
        Err(CommandError::Ambiguous(_))
    ));
    let mut d = document(false);
    let mut shadow = d["entries"][1].clone();
    shadow["codec"] = serde_json::json!("research");
    shadow["slots"] = serde_json::json!([]);
    d["entries"].as_array_mut().unwrap().insert(0, shadow);
    let c = Catalog::new(Definition::from_json(&d.to_string(), &id).unwrap());
    assert!(matches!(
        c.prepare(modes(), Command::ReadPins { bank: bank(0) }),
        Err(CommandError::Unsupported(_))
    ));
    let p = c.prepare_research(CommandCode(1), modes(), &[42]).unwrap();
    let raw = Reply::decode(&[1; 33]).unwrap();
    assert_eq!(p.decode(raw).unwrap().value(), &ReplyValue::Research);
    assert!(c.prepare_research(CommandCode(255), modes(), &[]).is_err());
}
