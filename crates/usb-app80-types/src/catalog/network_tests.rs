use super::*;
use fujimod_catalog_firmware_identity::FirmwareIdentity;
fn catalog(reverse: bool) -> Catalog {
    let layouts = [
        ("nand_read_blocks", 3),
        ("option41", 1),
        ("wlan_control", 1),
        ("radio_control", 4),
        ("signal_strength", 7),
        ("iperf", 1),
        ("wps", 8),
        ("station", 4),
        ("network_reconfigure", 14),
    ];
    let entries:Vec<_>=layouts.into_iter().enumerate().map(|(n,(codec,count))|{
  let slots:Vec<_>=(0..count).map(|i|if reverse {31-i}else{i}).collect();
  serde_json::json!({"code":200+n,"codec":codec,"label":codec,"slots":slots,"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0})
 }).collect();
    let json = serde_json::json!({"schema":1,"firmware":{"id":"radio-b","model":"Synthetic","version":"2","sha256":"ab".repeat(32)},"mailbox":{"page":3,"offset":576,"configuration_limit":512},"entries":entries});
    Catalog::new(
        Definition::from_json(
            &json.to_string(),
            &FirmwareIdentity::new("radio-b", &"ab".repeat(32)).unwrap(),
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
fn decode(p: &PreparedCommand, index: usize, byte: u8) -> CommandReply {
    let mut raw = p.request.encode();
    raw[usize::from(p.slots[index]) + 1] = byte;
    p.decode(Reply::decode(&raw).unwrap()).unwrap()
}
#[test]
fn radio_fields_remain_distinct_checked_and_firmware_selected() {
    assert!(RadioChannel::new(15).is_err());
    assert!(RadioPower::new(128).is_err());
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (mode, m) in [(RadioMode::Mode0, 0), (RadioMode::Mode1, 1)] {
            for channel in [0, 14] {
                for power in [0, 11, 12, 127] {
                    for (option, bit) in [(Toggle::Disabled, 0), (Toggle::Enabled, 128)] {
                        let p = prepare(
                            &c,
                            Command::RadioControl {
                                action: RadioAction::Primary {
                                    mode,
                                    channel: RadioChannel::new(channel).unwrap(),
                                    power: RadioPower::new(power).unwrap(),
                                    option,
                                },
                            },
                        );
                        assert_eq!(logical(&p), [m, channel, power | bit, 0]);
                        assert_eq!(p.request.code(), CommandCode(203));
                        assert_eq!(
                            p.decode(Reply::decode(&p.request.encode()).unwrap())
                                .unwrap()
                                .value(),
                            &ReplyValue::NoPayload
                        );
                    }
                }
            }
            let p = prepare(
                &c,
                Command::RadioControl {
                    action: RadioAction::Secondary {
                        mode,
                        channel: RadioChannel::new(14).unwrap(),
                    },
                },
            );
            assert_eq!(logical(&p), [m, 14, 0, 1]);
        }
        for (enabled, wire) in [(Toggle::Disabled, 0), (Toggle::Enabled, 1)] {
            assert_eq!(
                logical(&prepare(&c, Command::WlanControl { enabled })),
                [wire]
            );
        }
    }
}
#[test]
fn nand_and_option_results_are_data_not_callback_dispositions() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        for (spare, wire) in [(NandSpare::Omit, 0), (NandSpare::Include, 1)] {
            for block_group in [0, 255] {
                let p = prepare(&c, Command::NandReadBlocks { spare, block_group });
                assert_eq!(logical(&p), [wire, block_group, 255]);
                for raw in 0..=255 {
                    let expected = match raw {
                        0 => NandReadResult::Success,
                        1 => NandReadResult::RejectedOrWaitError,
                        2 => NandReadResult::OtherError,
                        n => NandReadResult::Unknown(n),
                    };
                    assert_eq!(
                        decode(&p, 2, raw).value(),
                        &ReplyValue::NandRead { status: expected }
                    );
                }
            }
        }
        let p = prepare(&c, Command::Option41);
        assert_eq!(logical(&p), [255]);
        for raw in 0..=255 {
            let expected = match raw {
                1 => Option41State::Zero,
                0 => Option41State::NonZero,
                n => Option41State::Unknown(n),
            };
            assert_eq!(
                decode(&p, 0, raw).value(),
                &ReplyValue::Option41 { state: expected }
            );
        }
    }
}
#[test]
fn peer_address_is_owned_and_opaque_observations_preserve_every_byte() {
    for n in [0, 5, 7] {
        assert!(PeerAddress::new(&vec![0; n]).is_err());
    }
    let mut input = [1, 2, 3, 4, 5, 6];
    let peer = PeerAddress::new(&input).unwrap();
    input.fill(0);
    assert_eq!(peer.data(), &[1, 2, 3, 4, 5, 6]);
    for reverse in [false, true] {
        let c = catalog(reverse);
        let p = prepare(&c, Command::SignalStrength { address: peer });
        assert_eq!(logical(&p), [1, 2, 3, 4, 5, 6, 255]);
        let iperf = prepare(&c, Command::Iperf);
        assert_eq!(logical(&iperf), [255]);
        for raw in 0..=255 {
            assert_eq!(
                decode(&p, 6, raw).value(),
                &ReplyValue::SignalStrength { raw_value: raw }
            );
            assert_eq!(
                decode(&iperf, 0, raw).value(),
                &ReplyValue::Iperf { raw_result: raw }
            );
        }
    }
}

#[test]
fn wps_targets_are_exact_and_callback_snapshots_never_instruct_host_polling() {
    for data in [vec![], vec![1; 5], vec![1; 7], vec![0, 1, 2, 3, 4, 5]] {
        assert!(WpsPeerAddress::new(&data).is_err());
    }
    for reverse in [false, true] {
        let c = catalog(reverse);
        let peer = WpsPeerAddress::new(&[2, 1, 2, 3, 4, 5]).unwrap();
        for (target, expected) in [
            (WpsTarget::Broadcast, [1, 0, 0, 0, 0, 0, 0, 255]),
            (
                WpsTarget::Peer { address: peer },
                [1, 2, 1, 2, 3, 4, 5, 255],
            ),
        ] {
            let p = prepare(
                &c,
                Command::Wps {
                    action: WpsAction::Start { target },
                },
            );
            assert_eq!(p.request.code(), CommandCode(206));
            assert_eq!(logical(&p), expected);
            for raw in 0..=255 {
                let expected = match raw {
                    0 => WpsCallbackResult::Success,
                    1 => WpsCallbackResult::Timeout,
                    2 => WpsCallbackResult::Failed,
                    3 => WpsCallbackResult::Overlap,
                    n => WpsCallbackResult::Unknown(n),
                };
                assert_eq!(
                    decode(&p, 7, raw).value(),
                    &ReplyValue::WpsCallback { snapshot: expected }
                );
            }
        }
        for (action, wire) in [(WpsAction::Stop, 0), (WpsAction::Connect, 2)] {
            let p = prepare(&c, Command::Wps { action });
            assert_eq!(logical(&p), [wire, 0, 0, 0, 0, 0, 0, 0]);
            assert_eq!(decode(&p, 7, 1).value(), &ReplyValue::NoPayload);
        }
        let p = prepare(
            &c,
            Command::Wps {
                action: WpsAction::PublishProgress,
            },
        );
        assert_eq!(logical(&p), [3, 0, 0, 0, 0, 0, 0, 255]);
        let phases = [
            WpsPhase::Idle,
            WpsPhase::PushButton,
            WpsPhase::Pin,
            WpsPhase::CredentialsReceived,
            WpsPhase::Timeout,
            WpsPhase::Failed,
            WpsPhase::Overlap,
            WpsPhase::Error,
        ];
        for raw in 0..=255 {
            let expected = if raw & 15 < 8 && raw >> 4 < 2 {
                WpsProgress::State {
                    phase: phases[usize::from(raw & 15)],
                    connection: if raw >> 4 == 0 {
                        ConnectionState::Disconnected
                    } else {
                        ConnectionState::Connected
                    },
                }
            } else {
                WpsProgress::Unknown { raw_value: raw }
            };
            assert_eq!(
                decode(&p, 7, raw).value(),
                &ReplyValue::WpsProgress { snapshot: expected }
            );
        }
    }
}
#[test]
fn station_controls_preserve_opaque_callback_and_scan_count_snapshots() {
    for reverse in [false, true] {
        let c = catalog(reverse);
        let stop = prepare(
            &c,
            Command::Station {
                action: StationAction::Stop,
            },
        );
        assert_eq!(logical(&stop), [0, 0, 0, 0]);
        assert_eq!(decode(&stop, 2, 255).value(), &ReplyValue::NoPayload);
        let p = prepare(
            &c,
            Command::Station {
                action: StationAction::StartStored,
            },
        );
        assert_eq!(logical(&p), [0, 1, 255, 0]);
        assert_eq!(p.request.code(), CommandCode(207));
        for raw in 0..=255 {
            assert_eq!(
                decode(&p, 2, raw).value(),
                &ReplyValue::StationConnection { raw_result: raw }
            );
        }
        for (mode, wire) in [(StationScanMode::Mode0, 0), (StationScanMode::Mode1, 1)] {
            let p = prepare(
                &c,
                Command::Station {
                    action: StationAction::Scan { mode },
                },
            );
            assert_eq!(logical(&p), [1, wire, 255, 255]);
            for raw in 0..=255 {
                let mut reply = p.request.encode();
                reply[1 + usize::from(p.slots[2])] = raw;
                reply[1 + usize::from(p.slots[3])] = 255 - raw;
                assert_eq!(
                    p.decode(Reply::decode(&reply).unwrap()).unwrap().value(),
                    &ReplyValue::StationScan {
                        raw_result: raw,
                        raw_count: 255 - raw
                    }
                );
            }
        }
    }
}

#[test]
fn access_point_requests_keep_country_bytes_channel_and_security_separate() {
    for data in [vec![], vec![1], vec![1, 2, 3]] {
        assert!(CountryBytes::new(&data).is_err());
    }
    let country = CountryBytes::new(b"US").unwrap();
    for reverse in [false, true] {
        let c = catalog(reverse);
        let stop = prepare(
            &c,
            Command::NetworkReconfigure {
                action: NetworkReconfigureAction::StopServer,
            },
        );
        assert_eq!(logical(&stop), [0; 14]);
        assert_eq!(decode(&stop, 8, 255).value(), &ReplyValue::NoPayload);
        let read = prepare(
            &c,
            Command::NetworkReconfigure {
                action: NetworkReconfigureAction::ReadConfiguration,
            },
        );
        assert_eq!(
            logical(&read),
            [1, 0, 255, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255]
        );
        for (radio, wire) in [
            (AccessPointRadioMode::Mode2, 0),
            (AccessPointRadioMode::Mode3, 1),
        ] {
            for (security, sec) in [
                (AccessPointSecurity::Option0, 0),
                (AccessPointSecurity::Option2, 2),
                (AccessPointSecurity::Option3, 3),
            ] {
                for channel in [0, 14] {
                    let p = prepare(
                        &c,
                        Command::NetworkReconfigure {
                            action: NetworkReconfigureAction::Apply {
                                number: 255,
                                channel: RadioChannel::new(channel).unwrap(),
                                radio,
                                security,
                                country,
                            },
                        },
                    );
                    assert_eq!(p.request.code(), CommandCode(208));
                    assert_eq!(
                        logical(&p),
                        [
                            1, 255, channel, 0, wire, sec, b'U', b'S', 255, 255, 255, 255, 255, 255
                        ]
                    );
                    let mut raw = p.request.encode();
                    for (i, byte) in [0, 1, 128, 254, 255, 42].into_iter().enumerate() {
                        raw[1 + usize::from(p.slots[i + 8])] = byte;
                    }
                    assert_eq!(
                        p.decode(Reply::decode(&raw).unwrap()).unwrap().value(),
                        &ReplyValue::NetworkConfiguration {
                            data: [0, 1, 128, 254, 255, 42]
                        }
                    );
                }
            }
        }
        assert_eq!(
            read.decode(Reply::decode(&read.request.encode()).unwrap())
                .unwrap()
                .value(),
            &ReplyValue::NetworkConfiguration { data: [255; 6] }
        );
    }
}
