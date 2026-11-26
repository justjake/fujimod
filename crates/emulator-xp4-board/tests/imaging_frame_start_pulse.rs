//! Imaging frame start pulse admission only; no guest instructions execute.
mod support;
use serde_json::{Value, json};
fn input() -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0x20000000u32})
}
fn imaging() -> Value {
    json!([
        [
            0xfff6c000u64,
            0xfff69000u64,
            [300, 301],
            [302, 303],
            [304, 305],
            null
        ],
        [1_000_000, 500_000, 20_000, 100],
        [0, 0],
        null
    ])
}
#[test]
fn a_frame_start_pulse_needs_the_imaging_wiring() {
    let mut pulse = input();
    pulse["imaging_frame_start_pulse"] = json!(101);
    let error = support::open(serde_json::from_value(pulse.clone()).unwrap())
        .err()
        .unwrap();
    assert!(error.contains("requires the imaging wiring"), "{error}");
    pulse["imaging"] = imaging();
    assert!(support::open(serde_json::from_value(pulse).unwrap()).is_ok());
}
#[test]
fn a_frame_start_pulse_outside_the_interrupt_range_refuses() {
    for line in [31, 512] {
        let mut pulse = input();
        pulse["imaging"] = imaging();
        pulse["imaging_frame_start_pulse"] = json!(line);
        let error = support::open(serde_json::from_value(pulse).unwrap())
            .err()
            .unwrap();
        assert!(error.contains("Invalid XP4 imaging map"), "{error}");
    }
}
