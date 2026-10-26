use super::*;
use fujimod_catalog_firmware_identity::FirmwareIdentity;
fn catalog(reverse: bool) -> Catalog {
    let layouts = [
        ("lens_select", 5),
        ("lens_operation2", 1),
        ("lens_operation3", 1),
        ("lens_operation4", 0),
        ("lens_operation5", 0),
    ];
    let entries:Vec<_>=layouts.into_iter().enumerate().map(|(n,(codec,count))| {
        let slots:Vec<_>=(0..count).map(|i|if reverse {31-i}else{i}).collect();
        serde_json::json!({"code":180+n,"codec":codec,"label":codec,"slots":slots,"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0})
    }).collect();
    let doc = serde_json::json!({"schema":1,"firmware":{"id":"lens-b","model":"Synthetic","version":"2","sha256":"ab".repeat(32)},"mailbox":{"page":3,"offset":576,"configuration_limit":512},"entries":entries});
    Catalog::new(
        Definition::from_json(
            &doc.to_string(),
            &FirmwareIdentity::new("lens-b", &"ab".repeat(32)).unwrap(),
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
#[test]
fn lens_selection_indices_cannot_alias_initialization_or_overflow() {
    for value in [160, 255] {
        assert!(matches!(
            LensSelectionIndex::new(value),
            Err(CommandError::Argument(ArgumentKind::LensSelectionIndex))
        ));
    }
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (target, selector) in [
            (LensTarget::Initialize, 0),
            (
                LensTarget::Index {
                    index: LensSelectionIndex::new(0).unwrap(),
                },
                1,
            ),
            (
                LensTarget::Index {
                    index: LensSelectionIndex::new(159).unwrap(),
                },
                160,
            ),
            (
                LensTarget::Index {
                    index: LensSelectionIndex::new(161).unwrap(),
                },
                162,
            ),
            (
                LensTarget::Index {
                    index: LensSelectionIndex::new(254).unwrap(),
                },
                255,
            ),
        ] {
            let p = prepare(&c, Command::LensSelect { target });
            assert_eq!(logical(&p), [selector, 0, 255, 255, 255]);
            for raw in 0..=255 {
                let mut data = p.request.encode();
                data[1 + usize::from(p.slots[2])] = raw;
                data[1 + usize::from(p.slots[3])] = 0;
                let range = match raw {
                    0 => LensSelectionRange::NotRejected,
                    1 => LensSelectionRange::OutOfRange,
                    n => LensSelectionRange::Unknown(n),
                };
                assert_eq!(
                    p.decode(Reply::decode(&data).unwrap()).unwrap().value(),
                    &ReplyValue::LensSelection {
                        range,
                        raw_detail: [0, 255]
                    }
                );
            }
        }
    }
}
#[test]
fn lens_operation_outputs_remain_snapshots_without_inferred_callback_success() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        for command in [Command::LensOperation2, Command::LensOperation3] {
            let p = prepare(&c, command);
            assert_eq!(logical(&p), [255]);
            for raw in [0, 1, 255] {
                let mut data = p.request.encode();
                data[1 + usize::from(p.slots[0])] = raw;
                assert_eq!(
                    p.decode(Reply::decode(&data).unwrap()).unwrap().value(),
                    &ReplyValue::LensOperation { raw_value: raw }
                );
            }
        }
        for command in [Command::LensOperation4, Command::LensOperation5] {
            let p = prepare(&c, command);
            assert!(p.slots.is_empty());
            assert_eq!(p.request.arguments(), &[0; 32]);
            assert_eq!(
                p.decode(Reply::decode(&p.request.encode()).unwrap())
                    .unwrap()
                    .value(),
                &ReplyValue::NoPayload
            );
        }
    }
}
