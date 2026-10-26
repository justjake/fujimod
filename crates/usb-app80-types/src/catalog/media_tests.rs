use super::*;
use fujimod_catalog_firmware_identity::FirmwareIdentity;
fn catalog(reverse: bool) -> Catalog {
    let entries:Vec<_>=[("display_option",4),("display_mode",1),("sound",3),("hdmi",19),("parameter_service",1),("charge_control",3),("charge_status",2),("audio_capture",3),("audio_playback",2),("erase_storage",2),("storage_status",1),("format_storage",0),("display_setup",0)].into_iter().enumerate().map(|(index,(codec,count))|{
  let slots:Vec<_>=(0..count).map(|slot|if reverse {31-slot}else{slot}).collect();
  serde_json::json!({"code":210+index,"label":codec,"codec":codec,"slots":slots,"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0})
 }).collect();
    let json = serde_json::json!({"schema":1,"firmware":{"id":"media-v2","model":"Synthetic","version":"2","sha256":"ab".repeat(32)},"mailbox":{"page":3,"offset":576,"configuration_limit":512},"entries":entries});
    Catalog::new(
        Definition::from_json(
            &json.to_string(),
            &FirmwareIdentity::new("media-v2", &"ab".repeat(32)).unwrap(),
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
        .map(|&slot| p.request.arguments()[usize::from(slot)])
        .collect()
}
fn response(p: &PreparedCommand, index: usize, value: u8) -> Reply {
    let mut raw = p.request.encode();
    raw[1 + usize::from(p.slots[index])] = value;
    Reply::decode(&raw).unwrap()
}
#[test]
fn display_forwarding_modes_and_parameter_boolean_have_distinct_typed_choices() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (command, expected) in [
            (
                Command::DisplayOption {
                    action: DisplayOptionAction::IdlePins,
                },
                vec![0, 0, 0, 0],
            ),
            (
                Command::DisplayOption {
                    action: DisplayOptionAction::ForwardPair {
                        first: 7,
                        second: 8,
                    },
                },
                vec![0, 1, 7, 8],
            ),
            (
                Command::DisplayMode {
                    mode: DisplayMode::Option0,
                },
                vec![0],
            ),
            (
                Command::DisplayMode {
                    mode: DisplayMode::Option1,
                },
                vec![1],
            ),
            (
                Command::ParameterService {
                    enabled: Toggle::Disabled,
                },
                vec![0],
            ),
            (
                Command::ParameterService {
                    enabled: Toggle::Enabled,
                },
                vec![1],
            ),
        ] {
            let p = prepare(&c, command);
            assert_eq!(logical(&p), expected);
            assert_eq!(
                p.decode(Reply::decode(&p.request.encode()).unwrap())
                    .unwrap()
                    .value(),
                &ReplyValue::NoPayload
            );
        }
    }
}
#[test]
fn sound_clip_and_track_variants_choose_canonical_codes_and_formats() {
    assert!(SoundClip::new(23).is_err());
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (format, wire) in [
            (SoundFormat::Format1, 0),
            (SoundFormat::Format2, 1),
            (SoundFormat::Format3, 2),
        ] {
            for clip in [0, 22] {
                let p = prepare(
                    &c,
                    Command::Sound {
                        action: SoundAction::Clip {
                            clip: SoundClip::new(clip).unwrap(),
                            format,
                        },
                    },
                );
                assert_eq!(logical(&p), [1, clip, wire]);
            }
            for (track, base) in [
                (SoundTrack::Track2, 23),
                (SoundTrack::Track3, 26),
                (SoundTrack::Track4, 29),
            ] {
                for (operation, offset) in [
                    (SoundOperation::First, 0),
                    (SoundOperation::Second, 1),
                    (SoundOperation::Third, 2),
                ] {
                    let p = prepare(
                        &c,
                        Command::Sound {
                            action: SoundAction::Track {
                                track,
                                operation,
                                format,
                            },
                        },
                    );
                    assert_eq!(p.request.code(), CommandCode(212));
                    assert_eq!(logical(&p), [1, base + offset, wire]);
                }
            }
        }
        assert_eq!(
            logical(&prepare(
                &c,
                Command::Sound {
                    action: SoundAction::Noop
                }
            )),
            [0, 0, 0]
        );
    }
}
#[test]
fn hdmi_packet_is_owned_reversed_and_exactly_sixteen_bytes_with_checked_triplets() {
    for length in [0, 15, 17] {
        assert!(HdmiPacket::new(&vec![0; length]).is_err());
    }
    assert!(HdmiValue::new(4096).is_err());
    assert_eq!(HdmiValue::new(4095).unwrap().value(), 4095);
    let mut original = std::array::from_fn::<_, 16, _>(|n| n as u8);
    let packet = HdmiPacket::new(&original).unwrap();
    original.fill(255);
    assert_eq!(packet.data()[0], 0);
    for reverse in [false, true] {
        let c = catalog(reverse);
        let p = prepare(
            &c,
            Command::Hdmi {
                action: HdmiAction::Packet { packet },
            },
        );
        let mut expected = vec![0, 1, 0];
        expected.extend((0..16).rev());
        assert_eq!(logical(&p), expected);
        let p = prepare(
            &c,
            Command::Hdmi {
                action: HdmiAction::Triplet {
                    first: HdmiValue::new(0x123).unwrap(),
                    second: HdmiValue::new(0x456).unwrap(),
                    third: HdmiValue::new(4095).unwrap(),
                },
            },
        );
        assert_eq!(&logical(&p)[..9], &[1, 0, 0, 1, 0x23, 4, 0x56, 15, 255]);
        assert!(
            logical(&prepare(
                &c,
                Command::Hdmi {
                    action: HdmiAction::ResetStub
                }
            ))
            .iter()
            .all(|&value| value == 0)
        );
    }
}
#[test]
fn charge_results_are_application_enums_and_absent_output_stays_unknown() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (action, expected) in [
            (
                ChargeAction::Control {
                    enabled: Toggle::Disabled,
                },
                [0, 0, 255],
            ),
            (
                ChargeAction::Control {
                    enabled: Toggle::Enabled,
                },
                [0, 1, 255],
            ),
            (
                ChargeAction::EnableAndSetFlag {
                    enabled: Toggle::Disabled,
                },
                [1, 0, 255],
            ),
            (
                ChargeAction::EnableAndSetFlag {
                    enabled: Toggle::Enabled,
                },
                [1, 1, 255],
            ),
        ] {
            let p = prepare(&c, Command::ChargeControl { action });
            assert_eq!(logical(&p), expected);
            for (raw, status) in [
                (0, ChargeResult::Success),
                (1, ChargeResult::Failed),
                (42, ChargeResult::Unknown(42)),
                (255, ChargeResult::Unknown(255)),
            ] {
                assert_eq!(
                    p.decode(response(&p, 2, raw)).unwrap().value(),
                    &ReplyValue::ChargeApplied { status }
                );
            }
        }
        let p = prepare(
            &c,
            Command::ChargeStatus {
                query: ChargeQuery::Presence,
            },
        );
        assert_eq!(logical(&p), [0, 255]);
        for (raw, value) in [
            (0, ChargePresence::Failed),
            (1, ChargePresence::Absent),
            (2, ChargePresence::Present),
            (3, ChargePresence::Unknown(3)),
        ] {
            assert_eq!(
                p.decode(response(&p, 1, raw)).unwrap().value(),
                &ReplyValue::ChargePresence { status: value }
            );
        }
        let p = prepare(
            &c,
            Command::ChargeStatus {
                query: ChargeQuery::State,
            },
        );
        assert_eq!(logical(&p), [1, 255]);
        for (raw, value) in [
            (0, ReportedChargeState::Failed),
            (1, ReportedChargeState::Unknown(1)),
            (2, ReportedChargeState::Unknown(2)),
            (3, ReportedChargeState::Value0),
            (4, ReportedChargeState::Values2To5),
            (5, ReportedChargeState::Value6),
            (6, ReportedChargeState::OtherValue),
            (255, ReportedChargeState::Unknown(255)),
        ] {
            assert_eq!(
                p.decode(response(&p, 1, raw)).unwrap().value(),
                &ReplyValue::ChargeState { status: value }
            );
        }
    }
}

#[test]
fn audio_storage_requests_and_results_use_remapped_slots_without_completion_instructions() {
    assert!(AudioCount::new(11).is_err());
    assert!(AudioCount::new(255).is_err());
    for reverse in [false, true] {
        let c = catalog(reverse);
        for count in [0, 10] {
            for (route, wire) in [(AudioRoute::Request82, 0), (AudioRoute::Request81, 1)] {
                let p = prepare(
                    &c,
                    Command::AudioCapture {
                        count: AudioCount::new(count).unwrap(),
                        route,
                    },
                );
                assert_eq!(logical(&p), [count, wire, 255]);
                for raw in 0..=255 {
                    let expected = match raw {
                        0 => AudioResult::Success,
                        1 => AudioResult::Failed,
                        n => AudioResult::Unknown(n),
                    };
                    assert_eq!(
                        p.decode(response(&p, 2, raw)).unwrap().value(),
                        &ReplyValue::AudioCaptured { status: expected }
                    );
                }
            }
        }
        for (mode, wire) in [
            (AudioPlayback::AllLanes, 0),
            (AudioPlayback::ZeroSecondLane, 1),
            (AudioPlayback::ZeroFirstLane, 2),
        ] {
            let p = prepare(&c, Command::AudioPlayback { mode });
            assert_eq!(logical(&p), [wire, 255]);
            for raw in 0..=255 {
                let expected = match raw {
                    0 => AudioResult::Success,
                    1 => AudioResult::Failed,
                    n => AudioResult::Unknown(n),
                };
                assert_eq!(
                    p.decode(response(&p, 1, raw)).unwrap().value(),
                    &ReplyValue::AudioPlayed { status: expected }
                );
            }
        }
        for (action, wire) in [
            (StorageErase::Kind1Regions, 1),
            (StorageErase::Kind0And2Regions, 2),
            (StorageErase::Kind0And1And2Regions, 3),
            (StorageErase::AuxiliaryRegion, 4),
            (StorageErase::ResetProfiles, 5),
        ] {
            let p = prepare(&c, Command::EraseStorage { action });
            assert_eq!(logical(&p), [wire, 255]);
            for raw in 0..=255 {
                let expected = match raw {
                    0 => StorageEraseResult::Success,
                    1 => StorageEraseResult::Failed,
                    n => StorageEraseResult::Unknown(n),
                };
                assert_eq!(
                    p.decode(response(&p, 1, raw)).unwrap().value(),
                    &ReplyValue::StorageErased { status: expected }
                );
            }
        }
        let p = prepare(&c, Command::StorageStatus);
        assert_eq!(logical(&p), [255]);
        for raw in 0..=255 {
            let expected = match raw {
                0 => StorageStatus::Zero,
                4 => StorageStatus::Value4,
                8 => StorageStatus::Value8,
                16 => StorageStatus::Value16,
                32 => StorageStatus::Value32,
                n => StorageStatus::Unknown(n),
            };
            assert_eq!(
                p.decode(response(&p, 0, raw)).unwrap().value(),
                &ReplyValue::StorageStatus { status: expected }
            );
        }
        for command in [Command::FormatStorage, Command::DisplaySetup] {
            let p = prepare(&c, command);
            assert_eq!(p.request.arguments(), &[0; 32]);
            assert!(p.slots.is_empty());
            assert_eq!(
                p.decode(Reply::decode(&p.request.encode()).unwrap())
                    .unwrap()
                    .value(),
                &ReplyValue::NoPayload
            );
        }
    }
}
