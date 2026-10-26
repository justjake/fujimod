use super::*;
use fujimod_catalog_firmware_identity::FirmwareIdentity;
#[test]
fn signed_positions_and_notifier_snapshots_keep_all_bits_with_reordered_slots() {
    for slots in [[0u8, 1, 2], [31, 17, 4]] {
        let doc = serde_json::json!({"schema":1,"firmware":{"id":"lens","model":"Synthetic","version":"1","sha256":"ab".repeat(32)},"mailbox":{"page":3,"offset":576,"configuration_limit":512},"entries":[
            {"code":96,"codec":"set_lens_position","label":"set","slots":slots,"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0},
            {"code":97,"codec":"request_lens_position","label":"request","slots":[slots[2]],"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0},
            {"code":98,"codec":"read_lens_parameter","label":"read","slots":[slots[0],slots[1]],"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0}]});
        let c = Catalog::new(
            Definition::from_json(
                &doc.to_string(),
                &FirmwareIdentity::new("lens", &"ab".repeat(32)).unwrap(),
            )
            .unwrap(),
        );
        let mode = ServiceModes::new(4).unwrap();
        for position in [i16::MIN, -1, 0, 1, i16::MAX] {
            let p = c
                .prepare(mode, Command::SetLensPosition { position })
                .unwrap();
            let [hi, lo] = position.to_be_bytes();
            assert_eq!(p.request.arguments()[usize::from(slots[0])], hi);
            assert_eq!(p.request.arguments()[usize::from(slots[1])], lo);
            assert_eq!(p.request.arguments()[usize::from(slots[2])], 255);
            assert_eq!(
                p.decode(Reply::decode(&p.request.encode()).unwrap())
                    .unwrap()
                    .value(),
                &ReplyValue::NoPayload
            );
            let p = c.prepare(mode, Command::ReadLensParameter).unwrap();
            let mut raw = p.request.encode();
            raw[1 + usize::from(slots[0])] = hi;
            raw[1 + usize::from(slots[1])] = lo;
            assert_eq!(
                p.decode(Reply::decode(&raw).unwrap()).unwrap().value(),
                &ReplyValue::LensParameter {
                    raw_value: position
                }
            );
        }
        let p = c.prepare(mode, Command::RequestLensPosition).unwrap();
        assert_eq!(p.request.arguments()[usize::from(slots[2])], 255);
    }
}

#[test]
fn explicit_constant_requests_preserve_firmware_selection_and_transition_metadata() {
    use fujimod_catalog_service::ModeTransition;
    let entry = |code, codec, next| serde_json::json!({"code":code,"codec":codec,"label":codec,"slots":[],"enabled_modes":5,"next_mode":next,"firmware_kind":1,"handler":0,"parameter_table":0});
    let doc = serde_json::json!({"schema":1,"firmware":{"id":"constants","model":"Synthetic","version":"1","sha256":"ab".repeat(32)},"mailbox":{"page":0,"offset":128,"configuration_limit":256},"entries":[entry(5,"report_service_success",0),entry(8,"report_service_success",0),entry(192,"report_service_failure",64),entry(9,"return_without_signal",0)]});
    let c = Catalog::new(
        Definition::from_json(
            &doc.to_string(),
            &FirmwareIdentity::new("constants", &"ab".repeat(32)).unwrap(),
        )
        .unwrap(),
    );
    let modes = ServiceModes::new(1).unwrap();
    assert!(matches!(
        c.prepare(modes, Command::ReportServiceSuccess),
        Err(CommandError::Ambiguous(Codec::ReportServiceSuccess))
    ));
    for (code, command) in [
        (5, Command::ReportServiceSuccess),
        (8, Command::ReportServiceSuccess),
        (192, Command::ReportServiceFailure),
        (9, Command::ReturnWithoutSignal),
    ] {
        let p = c.prepare_code(CommandCode(code), modes, command).unwrap();
        assert!(p.request().arguments().iter().all(|&v| v == 0));
        assert_eq!(p.request().code(), CommandCode(code));
        assert!(
            c.prepare_code(CommandCode(code), ServiceModes::new(8).unwrap(), command)
                .is_err()
        );
    }
    assert_eq!(
        c.definition().select(192, modes).unwrap().transition(),
        ModeTransition::Set(ServiceModes::new(64).unwrap())
    );
}

#[test]
fn scan_names_keep_all_thirty_two_bytes_and_untouched_tail_is_zero() {
    for reverse in [false, true] {
        let slots: Vec<_> = (0u8..33)
            .map(|i| if reverse { 32 - i } else { i })
            .collect();
        let doc = serde_json::json!({"schema":1,"firmware":{"id":"scan","model":"Synthetic","version":"1","sha256":"ab".repeat(32)},"mailbox":{"page":3,"offset":128,"configuration_limit":512},"entries":[{"code":72,"codec":"read_scan","label":"scan","slots":slots,"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0}]});
        let c = Catalog::new(
            Definition::from_json(
                &doc.to_string(),
                &FirmwareIdentity::new("scan", &"ab".repeat(32)).unwrap(),
            )
            .unwrap(),
        );
        for size in [0, 1, 31, 32] {
            let p = c
                .prepare(
                    ServiceModes::new(4).unwrap(),
                    Command::ReadScan {
                        selection: ScanSelection::new(63, 64).unwrap(),
                    },
                )
                .unwrap();
            assert_eq!(p.request().encoded_len(), 34);
            assert_eq!(p.request().arguments()[usize::from(slots[0])], 63);
            let mut raw = p.request().encode();
            let mut data = [0; 32];
            for i in 0..size {
                data[i] = (i + 128) as u8;
                raw[1 + usize::from(slots[i + 1])] = data[i];
            }
            assert_eq!(
                p.decode_bytes(&raw).unwrap().value(),
                &ReplyValue::ScanName { data }
            );
        }
    }
}
