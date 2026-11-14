//! `ladder_keys` with synthetic fixtures. The bands and samples below are test
//! inputs shaped like an ADC ladder plus a power-controller register ladder;
//! none is a measurement of any camera.
use crate::ladder_keys::*;
use crate::resistor_keys::Key;
use fujimod_emulator_devices::parts::controls::Controls;
use std::time::Duration;

fn rung(key: Key, low: u16, high: u16, sample: u16) -> Rung {
    Rung {
        key,
        band_low: low,
        band_high: high,
        sample,
        basis: "test fixture".into(),
    }
}

fn options() -> Options {
    Options {
        ladders: vec![
            Ladder {
                wire: Wire::Adc { channel: 1 },
                levels: 4096,
                rungs: vec![
                    rung(Key::Left, 100, 200, 150),
                    rung(Key::Right, 300, 400, 350),
                ],
            },
            Ladder {
                wire: Wire::PowerControllerRegister {
                    register: 0x34,
                    released: 300,
                },
                levels: 1024,
                rungs: vec![rung(Key::Menu, 0, 200, 0)],
            },
            Ladder {
                wire: Wire::PowerControllerRegister {
                    register: 0x35,
                    released: 1023,
                },
                levels: 1024,
                rungs: vec![rung(Key::Down, 850, 850, 850), rung(Key::Up, 400, 400, 400)],
            },
        ],
    }
}

fn keys() -> LadderKeys {
    LadderKeys::new(options()).unwrap()
}

fn press(keys: &LadderKeys, ns: u64, key: Key, pressed: bool) -> Result<(), String> {
    keys.enqueue(Duration::from_nanos(ns), key, pressed)
}

#[test]
fn adc_and_register_ladders_follow_held_keys() {
    let keys = keys();
    assert_eq!(keys.register_ladders(), [0x34, 0x35]);
    assert_eq!(keys.sample(1, 4095), 4095);
    assert_eq!(keys.register_sample(0x34), Some(300));
    assert_eq!(keys.register_sample(0x35), Some(1023));
    assert_eq!(keys.register_sample(0x36), None);
    press(&keys, 10, Key::Left, true).unwrap();
    press(&keys, 10, Key::Down, true).unwrap();
    press(&keys, 10, Key::Menu, true).unwrap();
    assert_eq!(keys.next_deadline(), Some(10));
    assert_eq!(keys.advance(9), Vec::<String>::new());
    assert_eq!(keys.advance(10).len(), 3);
    assert_eq!(keys.sample(1, 4095), 150);
    assert_eq!(keys.sample(2, 4095), 4095);
    assert_eq!(keys.register_sample(0x34), Some(0));
    assert_eq!(keys.register_sample(0x35), Some(850));
    press(&keys, 20, Key::Down, false).unwrap();
    press(&keys, 20, Key::Up, true).unwrap();
    keys.advance(20);
    assert_eq!(keys.register_sample(0x35), Some(400));
    press(&keys, 30, Key::Left, false).unwrap();
    press(&keys, 30, Key::Menu, false).unwrap();
    keys.advance(30);
    assert_eq!(keys.sample(1, 4095), 4095);
    assert_eq!(keys.register_sample(0x34), Some(300));
    assert_eq!(keys.next_deadline(), None);
}

#[test]
fn keys_without_a_declared_rung_are_refused() {
    let keys = keys();
    for key in [Key::ShutterHalf, Key::ShutterFull, Key::Back, Key::RearPlus] {
        let error = press(&keys, 5, key, true).unwrap_err();
        assert!(error.contains("no declared ladder rung"), "{error}");
    }
    assert!(!keys.used());
    assert_eq!(keys.next_deadline(), None);
}

#[test]
fn one_wire_holds_one_key_but_separate_wires_combine() {
    let keys = keys();
    press(&keys, 10, Key::Left, true).unwrap();
    let error = press(&keys, 15, Key::Right, true).unwrap_err();
    assert!(error.contains("holds the same ladder"), "{error}");
    press(&keys, 15, Key::Menu, true).unwrap();
    press(&keys, 20, Key::Left, false).unwrap();
    press(&keys, 20, Key::Right, true).unwrap();
    keys.advance(20);
    assert_eq!(keys.sample(1, 4095), 350);
    assert_eq!(keys.register_sample(0x34), Some(0));
    // A held key blocks a press queued after the checkpoint too.
    assert!(press(&keys, 30, Key::Left, true).is_err());
}

#[test]
fn checkpoint_restores_held_and_pending_keys() {
    let first = keys();
    press(&first, 10, Key::Down, true).unwrap();
    press(&first, 50, Key::Down, false).unwrap();
    first.advance(10);
    let saved = first.save();
    let checkpoint = first.checkpoint().unwrap();
    let second = keys();
    second.restore(&saved).unwrap();
    assert_eq!(second.register_sample(0x35), Some(850));
    assert_eq!(second.next_deadline(), Some(50));
    second.advance(50);
    assert_eq!(second.register_sample(0x35), Some(1023));
    let third = keys();
    third.validate_checkpoint(&checkpoint).unwrap();
    third.restore_checkpoint(&checkpoint).unwrap();
    assert_eq!(third.register_sample(0x35), Some(850));
    let mut foreign = saved.clone();
    foreign["held"] = serde_json::json!(["ShutterHalf"]);
    assert!(keys().restore(&foreign).is_err());
}

#[test]
fn scripts_queue_declared_keys_only() {
    let keys = keys();
    keys.script_at("t=1:MENU;t=2:-MENU", Duration::from_secs(10)).unwrap();
    assert_eq!(keys.next_deadline(), Some(11_000_000_000));
    assert!(keys.script("t=1:S1").is_err());
}

#[test]
fn undeclared_or_inconsistent_ladders_do_not_open() {
    let refuses = |edit: &dyn Fn(&mut Options), needle: &str| {
        let mut options = options();
        edit(&mut options);
        let error = LadderKeys::new(options).err().expect(needle);
        assert!(error.contains(needle), "{error}");
    };
    refuses(&|o| o.ladders.clear(), "no ladder");
    refuses(&|o| o.ladders[0].rungs.clear(), "no rung");
    refuses(&|o| o.ladders[0].rungs[0].sample = 99, "low <= sample");
    refuses(&|o| o.ladders[0].rungs[0].band_high = 4096, "low <= sample");
    refuses(&|o| o.ladders[0].rungs[0].basis = " ".into(), "no basis");
    refuses(&|o| o.ladders[0].rungs[1] = rung(Key::Right, 150, 250, 200), "overlaps");
    refuses(&|o| o.ladders[0].rungs[1].key = Key::Left, "two rungs");
    refuses(&|o| o.ladders[0].rungs[1].key = Key::FrontPlus, "dial");
    refuses(&|o| o.ladders[0].wire = Wire::Adc { channel: 15 }, "does not exist");
    refuses(&|o| o.ladders[2].wire = o.ladders[1].wire, "two ladders");
    refuses(&|o| o.ladders[0].levels = 1024, "the model reads 4096");
    refuses(&|o| o.ladders[1].levels = 4096, "the model reads 1024");
    refuses(
        &|o| o.ladders[1].wire = Wire::PowerControllerRegister { register: 0x80, released: 300 },
        "does not exist",
    );
    refuses(
        &|o| o.ladders[1].wire = Wire::PowerControllerRegister { register: 0x34, released: 100 },
        "lies in the",
    );
    refuses(
        &|o| o.ladders[2].rungs[0] = rung(Key::Down, 850, 1024, 900),
        "low <= sample",
    );
}

#[test]
fn declarations_deserialize_from_the_manifest_shape() {
    let options: Options = serde_json::from_value(serde_json::json!({
        "ladders": [{
            "wire": {"kind": "power-controller-register", "register": 52, "released": 300},
            "levels": 1024,
            "rungs": [{"key": "Menu", "band_low": 0, "band_high": 200, "sample": 0, "basis": "test"}]
        }]
    }))
    .unwrap();
    assert_eq!(LadderKeys::new(options).unwrap().register_ladders(), [52]);
    // Unknown fields and undeclared wire kinds do not open.
    assert!(serde_json::from_value::<Options>(serde_json::json!({"ladders": [], "extra": 1})).is_err());
    assert!(serde_json::from_value::<Options>(serde_json::json!({
        "ladders": [{"wire": {"kind": "gpio", "pin": 1}, "levels": 2, "rungs": []}]
    }))
    .is_err());
}
