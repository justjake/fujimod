use super::*;
use fujimod_catalog_firmware_identity::FirmwareIdentity;
fn catalog(reverse: bool) -> Catalog {
    let layouts = [
        ("lens_operation6", 0),
        ("publish_lens_completion_parameter", 2),
        ("prepare_adjustment_output", 0),
        ("prepare_alternate_adjustment_output", 0),
        ("lens_bounded_position", 3),
        ("lens_fixed_position", 1),
        ("invoke_primary_parameter_callback", 2),
        ("invoke_secondary_parameter_callback", 2),
        ("query_adjustment_option", 4),
    ];
    let entries:Vec<_>=layouts.into_iter().enumerate().map(|(n,(codec,count))| {
        let slots:Vec<_>=(0..count).map(|i|if reverse {31-i}else{i}).collect();
        serde_json::json!({"code":180+n,"codec":codec,"label":codec,"slots":slots,"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0})
    }).collect();
    let doc = serde_json::json!({"schema":1,"firmware":{"id":"adjustment-b","model":"Synthetic","version":"2","sha256":"ab".repeat(32)},"mailbox":{"page":3,"offset":576,"configuration_limit":512},"entries":entries});
    Catalog::new(
        Definition::from_json(
            &doc.to_string(),
            &FirmwareIdentity::new("adjustment-b", &"ab".repeat(32)).unwrap(),
        )
        .unwrap(),
    )
}
fn prepare(c: &Catalog, command: Command) -> PreparedCommand {
    c.prepare(ServiceModes::new(4).unwrap(), command).unwrap()
}
fn logical(p: &PreparedCommand) -> Vec<u8> {
    p.slots
        .iter()
        .map(|&i| p.request.arguments()[usize::from(i)])
        .collect()
}
fn decode(p: &PreparedCommand, values: &[u8]) -> CommandReply {
    let mut raw = p.request.encode();
    for (&slot, &value) in p.slots.iter().zip(values) {
        raw[1 + usize::from(slot)] = value;
    }
    p.decode(Reply::decode(&raw).unwrap()).unwrap()
}
#[test]
fn request_preparation_and_lens6_have_no_payload_or_hidden_host_parameters() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        for command in [
            Command::LensOperation6,
            Command::PrepareAdjustmentOutput,
            Command::PrepareAlternateAdjustmentOutput,
        ] {
            let p = prepare(&c, command);
            assert!(p.slots.is_empty());
            assert_eq!(p.request.arguments(), &[0; 32]);
            assert_eq!(decode(&p, &[]).value(), &ReplyValue::NoPayload);
        }
    }
}
#[test]
fn completion_publisher_and_bounded_position_keep_flags_separate_from_completion() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        let completion = prepare(&c, Command::PublishLensCompletionParameter);
        assert_eq!(logical(&completion), [255, 255]);
        for raw in 0..=255 {
            let flag = match raw {
                0 => LensCompletionParameterFlag::Other,
                1 => LensCompletionParameterFlag::Parameter80,
                n => LensCompletionParameterFlag::Unknown(n),
            };
            assert_eq!(
                decode(&completion, &[raw, 255]).value(),
                &ReplyValue::LensCompletionParameter {
                    flag,
                    encoded_value: 255
                }
            );
        }
        for (position, value) in [(0, 255), (255, 0), (18, 52)] {
            let p = prepare(&c, Command::LensBoundedPosition { position, value });
            assert_eq!(logical(&p), [position, value, 255]);
            for raw in 0..=255 {
                let range = match raw {
                    0 => LensPositionRange::NotRejected,
                    1 => LensPositionRange::OutOfRange,
                    n => LensPositionRange::Unknown(n),
                };
                assert_eq!(
                    decode(&p, &[position, value, raw]).value(),
                    &ReplyValue::LensPosition { range }
                );
            }
        }
    }
}
#[test]
fn callback_and_option_snapshots_preserve_raw_bytes_including_reported_failures() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        let p = prepare(&c, Command::LensFixedPosition);
        assert_eq!(logical(&p), [255]);
        assert_eq!(
            decode(&p, &[0]).value(),
            &ReplyValue::LensOperation { raw_value: 0 }
        );
        for parameter in [0, 255, 18] {
            for command in [
                Command::InvokePrimaryParameterCallback { parameter },
                Command::InvokeSecondaryParameterCallback { parameter },
            ] {
                let p = prepare(&c, command);
                assert_eq!(logical(&p), [parameter, 255]);
                for raw in [0, 1, 255] {
                    assert_eq!(
                        decode(&p, &[parameter, raw]).value(),
                        &ReplyValue::AdjustmentCallback { raw_value: raw }
                    );
                }
            }
        }
        for value in [0, 255] {
            let option = AdjustmentOptionByte::new(value);
            assert_eq!(option.value(), value);
            let p = prepare(&c, Command::QueryAdjustmentOption { option });
            assert_eq!(logical(&p), [value, 255, 255, 255]);
            for raw in 0..=255 {
                let status = match raw {
                    0 => AdjustmentOptionStatus::ReportedSuccess,
                    1 => AdjustmentOptionStatus::ReportedFailure,
                    n => AdjustmentOptionStatus::Unknown(n),
                };
                assert_eq!(
                    decode(&p, &[value, 0x89, 0xab, raw]).value(),
                    &ReplyValue::AdjustmentOption {
                        status,
                        raw_value: 0x89ab
                    }
                );
            }
        }
    }
}
