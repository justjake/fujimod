use super::*;

const DIGEST: &str = "7dd5b358a66f38d1fc7dad899a77d35fe4e6ed0edd0ed1d04fbca512d5dc8fd6";
fn json(guard_offset: &str) -> String {
    format!(
        r#"{{"schema":1,"firmware_id":"x70-110","firmware_sha256":"{DIGEST}","ram_alias":"0x40000000",
        "identity":{{"products":["X70"],"internal_versions":["1.39"],"sections":[]}},
        "configuration":{{"address":"0x42068000","cached_address":"0x02068000","size":"0x420000","max_read":"0x100"}},
        "guard":{{"offset":"{guard_offset}","value":1,"aliases":["0x420680a3"],
          "code_checks":[{{"name":"reset_guard","address":"0x948e98","size":"0x120","sha256":"{DIGEST}"}}],
          "state_words":[{{"name":"ui_mode","address":"0xcbd3e0","value":"0xc"}}],
          "callback_table":{{"address":"0x17d2910","hex":"74ee0e01"}},
          "output_state":{{"address":"0x17d548c","size":8}},
          "payloads":[{{"name":"main","address":"0xd08368","size":"0xe74"}}]}},
        "debug":{{"flag_offset":"0xf7"}},
        "debug_mode":{{"callback":{{"address":"0x17d291c","stock":"0x10ef130"}},"output_fields":"0x17d548c",
          "adapter":{{"address":"0x2068080","hex":"0100a0e3"}},
          "cache":{{"clean":"0x4263c","instruction_invalidate":"0x42720","barrier":"0x43b34","instruction_barrier":"0x42624"}},
          "code_checks":[{{"name":"configuration_range_wrapper","address":"0x945a44","size":"0x28","sha256":"{DIGEST}"}}],
          "storage":{{"geometry":{{"address":"0x40aeab94","words":["0x28","0x3a3",null,"0x1","0x3cb"]}},
            "banks":[{{"address":"0x439c4000","size":"0x500000"}}]}},
          "preferences":{{"address":"0x40d08354","size":"0x4c74"}}}},
        "bulk":{{"ignored":true}}}}"#
    )
}
fn identity() -> FirmwareIdentity {
    FirmwareIdentity::new("x70-110", DIGEST).unwrap()
}

#[test]
fn camera_tables_load_hex_and_integer_fields_for_the_selected_firmware() {
    let camera = CameraDefinition::from_json(&json("0xa3"), &identity()).unwrap();
    let guard = camera.guard().unwrap();
    assert_eq!((guard.offset, guard.value), (0xa3, 1));
    assert_eq!(guard.state_words[0].value, 12);
    assert_eq!(
        guard.callback_table,
        (0x17d2910, vec![0x74, 0xee, 0x0e, 0x01])
    );
    assert_eq!(
        guard.output_state,
        Range {
            address: 0x17d548c,
            size: 8
        }
    );
    assert_eq!(guard.code_checks[0].range.size, 0x120);
    assert_eq!(camera.debug().unwrap().flag_offset, 0xf7);
    assert_eq!(camera.configuration.size, 0x420000);
    let mode = camera.debug_mode().unwrap();
    assert_eq!(mode.adapter, (0x2068080, vec![1, 0, 0xa0, 0xe3]));
    assert_eq!(
        mode.storage_geometry.1,
        [Some(0x28), Some(0x3a3), None, Some(1), Some(0x3cb)]
    );
    assert_eq!(mode.storage_banks[0].size, 0x500000);
}

#[test]
fn camera_tables_refuse_other_firmware_and_out_of_range_offsets() {
    let other = FirmwareIdentity::new("x70-100", DIGEST).unwrap();
    assert!(CameraDefinition::from_json(&json("0xa3"), &other).is_err());
    assert!(CameraDefinition::from_json(&json("0x420000"), &identity()).is_err());
    assert!(CameraDefinition::from_json(&json("a3"), &identity()).is_err());
}
