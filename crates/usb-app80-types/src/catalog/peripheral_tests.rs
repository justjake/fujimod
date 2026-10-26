use super::*;
use fujimod_catalog_firmware_identity::FirmwareIdentity;

fn catalog(reverse: bool) -> Catalog {
    let entries: Vec<_>=[("register_control",7),("i2c",14),("accumulator",7),("peripheral_report",8),("battery_control",2),("initialize_display",1),("display_pattern",5)]
        .into_iter().enumerate().map(|(index,(codec,count))| {
            let slots:Vec<_>=(0..count).map(|slot| if reverse {31-slot} else {slot}).collect();
            serde_json::json!({"code":150+index,"label":codec,"codec":codec,"slots":slots,"enabled_modes":4,
                "next_mode":0,"firmware_kind":19,"handler":0,"parameter_table":0})
        }).collect();
    let json = serde_json::json!({"schema":1,"firmware":{"id":"synthetic-v2","model":"Synthetic","version":"2","sha256":"ab".repeat(32)},
        "mailbox":{"page":3,"offset":576,"configuration_limit":512},"entries":entries});
    Catalog::new(
        Definition::from_json(
            &json.to_string(),
            &FirmwareIdentity::new("synthetic-v2", &"ab".repeat(32)).unwrap(),
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
        .map(|&s| p.request.arguments()[usize::from(s)])
        .collect()
}
fn response(p: &PreparedCommand, values: &[u8]) -> Reply {
    let mut raw = p.request.encode();
    for (&slot, &byte) in p.slots.iter().zip(values) {
        raw[1 + usize::from(slot)] = byte;
    }
    Reply::decode(&raw).unwrap()
}
#[test]
fn register_variants_encode_scattered_fields_without_claiming_stub_read_values() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        let cases = [
            (
                RegisterAction::AudioValue { value: 0x56 },
                [0, 0, 0, 0, 0x56, 0, 0],
            ),
            (
                RegisterAction::DisplayFirst {
                    selector: 7,
                    value: 0x1234,
                },
                [1, 0, 7, 0x12, 0x34, 0, 0],
            ),
            (
                RegisterAction::DisplayStub {
                    selector: 7,
                    value: 0x1234,
                },
                [2, 0, 7, 0x12, 0x34, 0, 0],
            ),
            (
                RegisterAction::DisplaySecond {
                    selector: 7,
                    value: 0x1234,
                },
                [3, 0, 7, 0x12, 0x34, 0, 0],
            ),
            (
                RegisterAction::PowerWrite {
                    register: 0x1234,
                    value: 0x56,
                },
                [4, 0x12, 0x34, 0, 0x56, 0, 0],
            ),
            (
                RegisterAction::PositionPair {
                    first: 0x1234,
                    second: 0x5678,
                },
                [5, 0x12, 0x34, 0x56, 0x78, 0, 0],
            ),
            (RegisterAction::NoopFirst, [7, 0, 0, 0, 0, 0, 0]),
            (RegisterAction::NoopSecond, [8, 0, 0, 0, 0, 0, 0]),
        ];
        for (action, expected) in cases {
            let p = prepare(&c, Command::RegisterControl { action });
            assert_eq!(p.request.code(), CommandCode(150));
            assert_eq!(logical(&p), expected);
            assert_eq!(
                p.decode(response(&p, &expected)).unwrap().value(),
                &ReplyValue::NoPayload
            );
        }
    }
}
#[test]
fn i2c_requires_bounded_owned_data_and_has_a_distinct_single_write_contract() {
    for n in [0, 11, 255] {
        assert!(I2cLength::new(n).is_err());
    }
    for n in [0, 11, 256] {
        assert!(I2cPayload::new(&vec![0; n]).is_err());
    }
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (port, selector) in [
            (I2cPort::Port0, 0),
            (I2cPort::Port1, 1),
            (I2cPort::Port2, 2),
            (I2cPort::Port4, 4),
            (I2cPort::Port5, 5),
        ] {
            let p = prepare(
                &c,
                Command::I2c {
                    action: I2cAction::Read {
                        port,
                        command: 0xa5,
                        length: I2cLength::new(3).unwrap(),
                    },
                },
            );
            assert_eq!(&logical(&p)[..4], &[0, selector, 3, 0xa5]);
            let values = [0, selector, 3, 0xa5, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0];
            assert_eq!(
                p.decode(response(&p, &values)).unwrap().value(),
                &ReplyValue::I2cData {
                    data: vec![1, 2, 3]
                }
            );
            let mut input = [4, 5, 6];
            let payload = I2cPayload::new(&input).unwrap();
            input.fill(0);
            assert_eq!(payload.data(), &[4, 5, 6]);
            let p = prepare(
                &c,
                Command::I2c {
                    action: I2cAction::Write {
                        port,
                        command: 0x12,
                        payload,
                    },
                },
            );
            assert_eq!(
                logical(&p),
                [1, selector, 3, 0x12, 4, 5, 6, 0, 0, 0, 0, 0, 0, 0]
            );
        }
        let p = prepare(
            &c,
            Command::I2c {
                action: I2cAction::WriteSingle {
                    command: 0xa5,
                    value: 0x7f,
                },
            },
        );
        assert_eq!(
            logical(&p),
            [1, 3, 1, 0xa5, 0x7f, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            p.decode(response(&p, &logical(&p))).unwrap().value(),
            &ReplyValue::NoPayload
        );
    }
}
#[test]
fn accumulator_stop_returns_low_halfwords_and_status_report_stays_opaque() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        let start = prepare(
            &c,
            Command::Accumulator {
                action: AccumulatorAction::Start,
            },
        );
        assert_eq!(logical(&start)[0], 1);
        let stop = prepare(
            &c,
            Command::Accumulator {
                action: AccumulatorAction::StopAndRead,
            },
        );
        assert_eq!(logical(&stop)[0], 0);
        assert_eq!(
            stop.decode(response(&stop, &[0, 0x12, 0x34, 0xab, 0xcd, 0xff, 0xfe]))
                .unwrap()
                .value(),
            &ReplyValue::Accumulated {
                values: [0x1234, 0xabcd, 0xfffe]
            }
        );
        let report = prepare(&c, Command::PeripheralReport);
        assert_eq!(
            report
                .decode(response(&report, &[1, 2, 3, 4, 5, 6, 7, 8]))
                .unwrap()
                .value(),
            &ReplyValue::PeripheralReport {
                data: [1, 2, 3, 4, 5, 6, 7, 8]
            }
        );
    }
}

#[test]
fn battery_actions_are_not_measurements_and_initialize_uses_canonical_selector() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (action, selector) in [
            (BatteryAction::ResetPending, 0),
            (BatteryAction::CallbackWait, 1),
        ] {
            let p = prepare(&c, Command::BatteryControl { action });
            assert_eq!(p.request.code(), CommandCode(154));
            assert_eq!(logical(&p), [selector, 0]);
            // No payload/status byte is defined; arbitrary returned bytes never
            // become a host completion instruction or a measurement.
            assert_eq!(
                p.decode(response(&p, &[255, 255])).unwrap().value(),
                &ReplyValue::NoPayload
            );
        }
        let p = prepare(&c, Command::InitializeDisplay);
        assert_eq!(p.request.code(), CommandCode(155));
        assert_eq!(logical(&p), [0]);
        assert_eq!(
            p.decode(response(&p, &[255])).unwrap().value(),
            &ReplyValue::NoPayload
        );
    }
}
#[test]
fn display_pattern_preserves_unused_slot_and_packed_color_field_order() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (action, expected) in [
            (DisplayPatternAction::ResetPreset, [0, 0, 0, 0, 0]),
            (DisplayPatternAction::Bars, [1, 0, 0, 0, 0]),
            (
                DisplayPatternAction::Solid {
                    repeated: 0x12,
                    middle: 0x34,
                    low: 0x56,
                },
                [2, 0, 0x12, 0x56, 0x34],
            ),
        ] {
            let p = prepare(&c, Command::DisplayPattern { action });
            assert_eq!(p.request.code(), CommandCode(156));
            assert_eq!(logical(&p), expected);
            assert_eq!(
                p.decode(response(&p, &[255; 5])).unwrap().value(),
                &ReplyValue::NoPayload
            );
        }
        for repeated in 0..=255 {
            let p = prepare(
                &c,
                Command::DisplayPattern {
                    action: DisplayPatternAction::Solid {
                        repeated,
                        middle: 255 - repeated,
                        low: 0,
                    },
                },
            );
            assert_eq!(logical(&p), [2, 0, repeated, 0, 255 - repeated]);
        }
    }
}
