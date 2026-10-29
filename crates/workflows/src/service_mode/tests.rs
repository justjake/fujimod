use super::*;
use crate::bulk::tests::{exchange, exchange_max, info, limits, opened, read};
use fujimod_catalog_workflows::{ServiceSetup, WorkflowDefinition};
use fujimod_usb::{
    ff80::commands::configuration::{ConfigurationLocation, ReadConfiguration, WriteConfiguration},
    testing::Exchange,
};
use futures_lite::future::block_on;
use std::num::NonZeroU32;

fn camera() -> ServiceSetup {
    let json = r#"{
        "$schema":"https://fujimod.com/schemas/service-workflow.generated.schema.json", "schema":1,"firmware_id":"sample-1","firmware_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "identity":{"products":["X70"],"internal_versions":["1.0"],"code_checks":[]},"setup":{"ram_alias":0,
        "configuration":{"address":4096,"cached_address":4096,"size":256,"page_bytes":64,"memory_read_bytes":256},
        "debug_offset":247,
        "guard":{"offset":163,"value":1,"aliases":[4259],"code_checks":[],"state_words":[],
            "callback_table":{"address":8192,"words":[0]},"output_state":{"address":8200,"size":4},"payloads":[]}}, "persistence":null
    }"#;
    WorkflowDefinition::from_json(
        json,
        &fujimod_catalog_firmware_identity::FirmwareIdentity::new("sample-1", &"a".repeat(64))
            .unwrap(),
    )
    .unwrap()
    .setup()
    .unwrap()
    .clone()
}
fn byte(offset: u32, value: u8, write: bool) -> Vec<Exchange> {
    let location = ConfigurationLocation {
        view: ConfigurationView::Ram,
        page: 0,
        offset,
    };
    let request = if write {
        WriteConfiguration::new(location, vec![value], NonZeroU32::new(64).unwrap())
            .unwrap()
            .encode()
    } else {
        ReadConfiguration::new(location, NonZeroU32::MIN, NonZeroU32::new(64).unwrap())
            .unwrap()
            .encode()
    };
    let mut reply = request[..36].to_vec();
    if !write {
        reply.push(value);
    }
    exchange(request, reply)
}
fn steps(before: u8, fail: bool, leave: bool) -> Vec<Exchange> {
    let mut s = info(4, "X70");
    let request = vec![
        2, 0, 26, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0,
    ];
    let mut reply = request.clone();
    reply[28..32].copy_from_slice(&256u32.to_le_bytes());
    reply[32..36].copy_from_slice(&64u32.to_le_bytes());
    s.extend(exchange_max(request, reply, 48));
    s.extend(byte(247, before, false));
    s.extend(byte(163, 0, false));
    if before == 0 {
        s.extend(byte(247, 1, true));
        s.extend(byte(247, 1, false));
    }
    s.extend(read(8192, if fail { &[1, 0, 0, 0] } else { &[0; 4] }));
    if !fail {
        s.extend(read(8200, &[0; 4]));
        s.extend(byte(163, 1, true));
        s.extend(byte(163, 1, false));
        s.extend(read(4259, &[1]));
        s.extend(read(8192, &[0; 4]));
        s.extend(read(8200, &[0; 4]));
    }
    let target = if fail { before } else { u8::from(leave) };
    s.extend(byte(247, 1, false));
    if target == 0 {
        s.extend(byte(247, 0, true));
        s.extend(byte(247, 0, false));
    }
    s
}
#[test]
fn session_leaves_debug_enabled_and_reports_both_original_values() {
    for before in [0, 1] {
        let (mut session, script) = opened(steps(before, false, true));
        let result = block_on(prepare_service_session(
            &mut session,
            &camera(),
            limits(),
            &mut |_| {},
        ))
        .unwrap();
        assert_eq!(result.debug_flag_before, before);
        assert!(!result.guard_was_set);
        assert_eq!(script.remaining(), 0);
    }
}
#[test]
fn failure_restores_debug_and_legacy_guard_still_disables_it() {
    for before in [0, 1] {
        let (mut session, script) = opened(steps(before, true, true));
        assert!(
            block_on(prepare_service_session(
                &mut session,
                &camera(),
                limits(),
                &mut |_| {}
            ))
            .is_err()
        );
        assert_eq!(script.remaining(), 0);
    }
    let (mut session, script) = opened(steps(0, false, false));
    block_on(arm_reset_guard(
        &mut session,
        &camera(),
        limits(),
        &mut |_| {},
    ))
    .unwrap();
    assert_eq!(script.remaining(), 0);
}

#[test]
fn setup_transfer_limits_bound_session_limits() {
    let mut setup = camera();
    setup.configuration.memory_read_bytes = 248;
    let mut session_limits = limits();
    session_limits.memory.read = NonZeroU32::new(256).unwrap();
    session_limits.configuration = NonZeroU32::new(256).unwrap();
    let bounds = session_limits.for_setup(&setup).unwrap();
    assert_eq!(bounds.memory.read.get(), 248);
    assert_eq!(bounds.configuration.get(), 64);
}
