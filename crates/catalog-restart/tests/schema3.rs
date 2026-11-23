//! Schema 3: the X-Processor 4 message request, which publishes no power-off.
//!
//! The fixture carries the facts the public catalog publishes for the X-T30
//! 2.01 update: the rewritten request word, the queue that carries it, the
//! clock the deadline is compared against and the diagnostic query it leaves
//! through. The catalog also publishes the sender code check, the queue
//! builder, clock and query routines, which these tests do not restate.
use fujimod_catalog_restart::{
    DeadlineUnit, FirmwareRestart, RestartKind, WAKE_DEADLINE_SCHEMA, XP4_WAKE_DEADLINE_SCHEMA,
};
use serde_json::{Value, json};

const DOCUMENT: &str = include_str!("fixtures/xp4-wake-deadline.json");

fn document() -> Value {
    serde_json::from_str(DOCUMENT).expect("fixture is JSON")
}

fn parse(value: &Value) -> FirmwareRestart {
    serde_json::from_value(value.clone()).expect("fixture is a restart document")
}

#[test]
fn the_published_x_processor_4_document_validates() {
    let restart = parse(&document());
    restart.validate().unwrap();
    assert_eq!(restart.schema, XP4_WAKE_DEADLINE_SCHEMA);
    assert_eq!(restart.mechanism.kind, Some(RestartKind::Xp4WakeDeadline));
    assert!(restart.mechanism.power_off.is_none());
    assert_eq!(restart.mechanism.runtime_qualified, Some(false));
    let deadline = restart.mechanism.deadline.as_ref().unwrap();
    assert_eq!(deadline.payload_offset, 8);
    assert_eq!(deadline.unit, DeadlineUnit::EpochSeconds);
    let queue = restart.mechanism.queue.as_ref().unwrap();
    assert_eq!(
        queue.parameters.get(&8),
        Some(&restart.mechanism.deadline_offset.unwrap())
    );
}

#[test]
fn the_document_keeps_its_facts_through_its_own_types() {
    let restart = parse(&document());
    let text = serde_json::to_string(&restart).unwrap();
    let again: FirmwareRestart = serde_json::from_str(&text).unwrap();
    assert_eq!(restart, again);
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["mechanism"]["kind"], "xp4-wake-deadline");
    assert_eq!(value["mechanism"]["queue"]["parameters"]["8"], 4);
    assert!(value["mechanism"]["power_off"].is_null());
}

#[test]
fn a_message_request_needs_the_facts_that_carry_the_deadline() {
    for field in [
        "queue",
        "deadline",
        "clock",
        "request_conversion",
        "configuration_override",
    ] {
        let mut value = document();
        value["mechanism"][field] = Value::Null;
        let error = parse(&value).validate().unwrap_err();
        assert!(error.contains("deadline, queue"), "{field}: {error}");
    }
    let mut value = document();
    value["mechanism"].as_object_mut().unwrap().remove("kind");
    assert!(
        parse(&value)
            .validate()
            .unwrap_err()
            .contains("names its mechanism")
    );
}

#[test]
fn the_queue_layout_must_carry_the_request_and_deadline() {
    let mut value = document();
    value["mechanism"]["queue"]["parameters"] = json!({"0": 0, "12": 8});
    assert!(
        parse(&value)
            .validate()
            .unwrap_err()
            .contains("queue layout")
    );
    let mut value = document();
    value["mechanism"]["queue"]["parameters"] = json!({"0": 0, "8": 4, "12": 8, "20": 0});
    assert!(
        parse(&value)
            .validate()
            .unwrap_err()
            .contains("starts past the payload")
    );
    let mut value = document();
    value["mechanism"]["queue"]["uninitialized_payload_offsets"] = json!([8]);
    assert!(
        parse(&value)
            .validate()
            .unwrap_err()
            .contains("uninitialized payload byte")
    );
}

#[test]
fn an_uninitialized_offset_inside_a_queued_parameter_is_rejected() {
    let mut value = document();
    value["mechanism"]["queue"]["uninitialized_payload_offsets"] = json!([9]);
    let error = parse(&value).validate().unwrap_err();
    assert!(error.contains("uninitialized payload byte"), "{error}");
}

#[test]
fn the_clock_read_order_must_take_both_halves() {
    let restart = parse(&document());
    let registers = restart.mechanism.clock.as_ref().unwrap().registers;
    let mut value = document();
    value["mechanism"]["clock"]["read_order"] = json!([registers.low, registers.low]);
    let error = parse(&value).validate().unwrap_err();
    assert!(error.contains("read order"), "{error}");
}

#[test]
fn a_message_request_publishes_no_power_off_and_no_storage() {
    let mut value = document();
    value["mechanism"]["kind"] = json!("borrowed-query");
    assert!(
        serde_json::from_value::<FirmwareRestart>(value).is_err(),
        "the mechanism name is one of the published kinds"
    );
    let mut value = document();
    value["mechanism"]["parameter_storage"] = json!({"address": 1, "bytes": 12});
    assert!(
        parse(&value)
            .validate()
            .unwrap_err()
            .contains("no parameter storage")
    );
    let mut value = document();
    value["mechanism"]["runtime_qualified"] = Value::Null;
    assert!(
        parse(&value)
            .validate()
            .unwrap_err()
            .contains("runtime qualification")
    );
}

#[test]
fn a_wake_deadline_document_still_validates_and_rejects_message_facts() {
    let mut value = document();
    value["schema"] = json!(WAKE_DEADLINE_SCHEMA);
    let restart = parse(&value);
    assert!(
        restart
            .validate()
            .unwrap_err()
            .contains("carries no message-queue facts")
    );
    let mut value = document();
    value["schema"] = json!(4);
    assert!(
        parse(&value)
            .validate()
            .unwrap_err()
            .contains("unsupported schema")
    );
}

#[test]
fn a_shadow_clock_requires_a_live_reader_and_explanation() {
    let value: Value =
        serde_json::from_str(include_str!("fixtures/xp4-shadow-clock.json")).unwrap();
    parse(&value).validate().unwrap();
    for field in ["live_reader", "absent_reason"] {
        let mut invalid = value.clone();
        invalid["mechanism"]["clock"][field] = Value::Null;
        assert!(parse(&invalid).validate().is_err(), "{field}");
    }
    let mut partial = value.clone();
    partial["mechanism"]["clock"]["writer"] = json!(1);
    assert!(parse(&partial).validate().is_err());
    let mut bus = document();
    bus["mechanism"]["clock"]["transaction"] = Value::Null;
    assert!(parse(&bus).validate().is_err());
}

#[test]
fn an_xp4_callback_timer_must_pass_the_common_timer_checks() {
    let mut value = document();
    value["mechanism"]["deferred_timer"] = json!({
        "kind": "software-timer-callback-v1",
        "period_unit": "milliseconds",
        "one_shot_periodic": 0,
        "periodic_nonzero": true,
        "callback_arguments": 0,
        "start": {"address": 1, "size": 4},
        "cancel": {"address": 2, "size": 4}
    });
    assert!(
        parse(&value)
            .validate()
            .unwrap_err()
            .contains("matching code checks")
    );
}
