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
                sets: vec![],
            },
            Ladder {
                wire: Wire::PowerControllerRegister {
                    register: 0x34,
                    released: 300,
                },
                levels: 1024,
                rungs: vec![rung(Key::Menu, 0, 200, 0)],
                sets: vec![],
            },
            Ladder {
                wire: Wire::PowerControllerRegister {
                    register: 0x35,
                    released: 1023,
                },
                levels: 1024,
                rungs: vec![rung(Key::Down, 850, 850, 850), rung(Key::Up, 400, 400, 400)],
                sets: vec![],
            },
        ],
        dials: vec![],
        switches: vec![],
        rear_dial: None,
        front_dial: None,
        gpio_keys: vec![],
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

// Synthetic fixture for contact sets, position dials and command dials. The
// bands are test inputs shaped like a shared ADC wire, not camera values.
fn set(keys: &[Key], position: Option<&str>, low: u16, high: u16) -> ContactSet {
    ContactSet {
        keys: keys.to_vec(),
        position: position.map(str::to_owned),
        band_low: low,
        band_high: high,
        sample: (low + high) / 2,
        basis: "test fixture".into(),
    }
}

fn position(label: &str, low: u16, high: u16, contact: bool) -> Position {
    Position {
        label: label.into(),
        aliases: vec![],
        band_low: low,
        band_high: high,
        sample: (low + high) / 2,
        contact,
    }
}

fn shared_options() -> Options {
    let mut options = options();
    options.ladders.push(Ladder {
        wire: Wire::Adc { channel: 2 },
        levels: 4096,
        rungs: vec![rung(Key::Fn1, 2600, 3000, 2800), rung(Key::Quick, 2100, 2300, 2200)],
        sets: vec![
            set(&[Key::Fn1], Some("MOVIE"), 2310, 2590),
            set(&[Key::Quick], Some("MOVIE"), 1900, 2090),
            set(&[Key::Fn1, Key::Quick], None, 1700, 1899),
            set(&[Key::Fn1, Key::Quick], Some("MOVIE"), 100, 1699),
        ],
    });
    options.ladders[2].sets.push(set(&[Key::Down, Key::Up], None, 600, 700));
    options.dials = vec![
        PositionDial {
            name: "mode".into(),
            aliases: vec!["lever".into()],
            channel: 2,
            positions: vec![position("MOVIE", 3001, 3750, true), position("STILL", 3751, 4095, false)],
            basis: "test fixture".into(),
        },
        PositionDial {
            name: "iso".into(),
            aliases: vec![],
            channel: 12,
            positions: vec![
                Position { aliases: vec!["AUTO".into()], ..position("A", 4000, 4095, false) },
                position("200", 400, 600, false),
                position("+0.3", 1000, 1100, false),
            ],
            basis: "test fixture".into(),
        },
    ];
    options.switches = vec![RegisterSwitch {
        name: "power".into(),
        register: 0,
        bit: 10,
        set: "OFF".into(),
        clear: "ON".into(),
        basis: "test fixture".into(),
    }];
    options.rear_dial = Some(CountingDial { register: 0x36, basis: "test fixture".into() });
    options.front_dial = Some(QuadratureDial {
        pins: [Pin { port: 0, pin: 9 }, Pin { port: 0, pin: 10 }],
        rest_phase: 3,
        basis: "test fixture".into(),
    });
    options.gpio_keys = vec![GpioKey {
        key: Key::RearPush,
        pin: Pin { port: 0, pin: 13 },
        active_high: false,
        basis: "test fixture".into(),
    }];
    options
}

use fujimod_emulator_devices::parts::pins::{DriveInput, Pin};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecordedPins(Mutex<std::collections::BTreeMap<(u8, u8), bool>>);
impl DriveInput for RecordedPins {
    fn set_input(&self, pin: Pin, high: bool) {
        self.0.lock().unwrap().insert((pin.port, pin.pin), high);
    }
}
impl RecordedPins {
    fn level(&self, port: u8, pin: u8) -> Option<bool> {
        self.0.lock().unwrap().get(&(port, pin)).copied()
    }
}

fn shared() -> (LadderKeys, Arc<RecordedPins>) {
    let pins = Arc::new(RecordedPins::default());
    (LadderKeys::with_pins(shared_options(), pins.clone()).unwrap(), pins)
}

#[test]
fn shared_wire_reads_the_band_midpoint_of_the_held_set() {
    let (keys, _) = shared();
    press(&keys, 10, Key::Fn1, true).unwrap();
    press(&keys, 20, Key::Quick, true).unwrap();
    keys.advance(10);
    assert_eq!(keys.sample(2, 4095), 2800);
    keys.advance(20);
    assert_eq!(keys.sample(2, 4095), (1700 + 1899) / 2);
    // Two keys of a register ladder with a decoded pair band.
    press(&keys, 30, Key::Down, true).unwrap();
    press(&keys, 30, Key::Up, true).unwrap();
    keys.advance(30);
    assert_eq!(keys.register_sample(0x35), Some(650));
}

#[test]
fn keys_at_a_contact_position_read_the_set_naming_it() {
    let (keys, _) = shared();
    // The window's idle sample puts the lever at MOVIE (no position set).
    assert_eq!(keys.sample(2, 3400), 3400);
    press(&keys, 10, Key::Quick, true).unwrap();
    keys.advance(10);
    assert_eq!(keys.sample(2, 3400), (1900 + 2090) / 2);
    assert_eq!(keys.sample(2, 4095), 2200, "at STILL Q reads its own rung");
    // An applied position overrides the idle sample.
    keys.set_position(Duration::from_nanos(20), "lever", "still").unwrap();
    keys.advance(20);
    assert_eq!(keys.sample(2, 3400), 2200);
    press(&keys, 30, Key::Quick, false).unwrap();
    keys.advance(30);
    assert_eq!(keys.sample(2, 3400), (3751 + 4095) / 2);
    keys.set_position(Duration::from_nanos(40), "mode", "MOVIE").unwrap();
    press(&keys, 50, Key::Fn1, true).unwrap();
    press(&keys, 50, Key::Quick, true).unwrap();
    keys.advance(50);
    assert_eq!(keys.sample(2, 4095), (100 + 1699) / 2);
}

#[test]
fn undecoded_sets_are_refused() {
    let (keys, _) = shared();
    press(&keys, 10, Key::Left, true).unwrap();
    let error = press(&keys, 15, Key::Right, true).unwrap_err();
    assert!(error.contains("holds the same ladder"), "{error}");
    press(&keys, 10, Key::Menu, true).unwrap();
    // A position change that would leave a held set undecoded is refused too:
    // the fixture declares Fn1 at MOVIE, so take Down+Up which has no position.
    let (keys, _) = shared();
    let mut options = shared_options();
    options.ladders[3].sets.retain(|set| set.keys != [Key::Quick]);
    let keys2 = LadderKeys::with_pins(options, Arc::new(RecordedPins::default())).unwrap();
    press(&keys2, 10, Key::Quick, true).unwrap();
    let error = keys2.set_position(Duration::from_nanos(20), "mode", "MOVIE").unwrap_err();
    assert!(error.contains("no band is declared"), "{error}");
    // Unknown dials and positions name what exists.
    let error = keys.set_position(Duration::from_nanos(5), "zoom", "1").unwrap_err();
    assert!(error.contains("mode, iso, power"), "{error}");
    let error = keys.set_position(Duration::from_nanos(5), "iso", "123").unwrap_err();
    assert!(error.contains("A|200|+0.3"), "{error}");
}

#[test]
fn dedicated_dials_change_at_their_time_and_accept_aliases_and_numbers() {
    let (keys, _) = shared();
    keys.set_position(Duration::from_nanos(10), "ISO", "auto").unwrap();
    keys.set_position(Duration::from_nanos(20), "iso", "0.3").unwrap();
    assert_eq!(keys.next_deadline(), Some(10));
    assert_eq!(keys.sample(12, 100), 100);
    assert_eq!(keys.advance(10), ["physical input iso A at 0.000s"]);
    assert_eq!(keys.sample(12, 100), (4000 + 4095) / 2);
    keys.advance(20);
    assert_eq!(keys.sample(12, 100), 1050);
    assert_eq!(keys.next_deadline(), None);
}

#[test]
fn counting_dial_register_clears_on_read() {
    let (keys, _) = shared();
    assert_eq!(keys.take_register(0x36), Some(0));
    assert_eq!(keys.take_register(0x35), None);
    for n in 0..10 {
        press(&keys, 10 + n, Key::RearPlus, true).unwrap();
        // The release from --press is accepted and ignored.
        press(&keys, 10 + n, Key::RearPlus, false).unwrap();
    }
    keys.advance(100);
    assert_eq!(keys.take_register(0x36), Some(7));
    assert_eq!(keys.take_register(0x36), Some(3));
    assert_eq!(keys.take_register(0x36), Some(0));
    for n in 0..9 {
        press(&keys, 200 + n, Key::RearMinus, true).unwrap();
    }
    keys.advance(300);
    assert_eq!(keys.take_register(0x36), Some(0x8), "-8 in four bits");
    assert_eq!(keys.take_register(0x36), Some(0xf), "-1 in four bits");
    assert_eq!(keys.take_register(0x36), Some(0));
}

#[test]
fn quadrature_dial_steps_one_phase_per_detent() {
    let (keys, pins) = shared();
    let phase = |pins: &RecordedPins| {
        (u8::from(pins.level(0, 9).unwrap()) << 1) | u8::from(pins.level(0, 10).unwrap())
    };
    // FRONT+ from rest: 0.10 (bit 0) falls first, then 3 -> 2 -> 0 -> 1 -> 3.
    let mut seen = Vec::new();
    for n in 0..4 {
        press(&keys, 10 + n, Key::FrontPlus, true).unwrap();
        keys.advance(10 + n);
        seen.push(phase(&pins));
    }
    assert_eq!(seen, [2, 0, 1, 3]);
    seen.clear();
    for n in 0..4 {
        press(&keys, 20 + n, Key::FrontMinus, true).unwrap();
        keys.advance(20 + n);
        seen.push(phase(&pins));
    }
    assert_eq!(seen, [1, 0, 2, 3]);
}

#[test]
fn gpio_push_and_register_switch_drive_their_wiring() {
    let (keys, pins) = shared();
    assert_eq!(keys.driven_registers(), [0, 0x34, 0x35]);
    assert_eq!(keys.drive_register(0, 0x200), None, "no position set: register untouched");
    press(&keys, 10, Key::RearPush, true).unwrap();
    keys.set_position(Duration::from_nanos(10), "power", "off").unwrap();
    keys.advance(10);
    assert_eq!(pins.level(0, 13), Some(false), "active low");
    assert_eq!(keys.drive_register(0, 0x280), Some(0x680));
    press(&keys, 20, Key::RearPush, false).unwrap();
    keys.set_position(Duration::from_nanos(20), "power", "ON").unwrap();
    keys.advance(20);
    assert_eq!(pins.level(0, 13), Some(true));
    assert_eq!(keys.drive_register(0, 0x680), Some(0x280));
    assert_eq!(keys.drive_register(0x34, 0), Some(300), "ladders keep their value");
    assert!(LadderKeys::new(shared_options()).is_err(), "GPIO controls need pins");
}

#[test]
fn checkpoints_keep_positions_and_counts_and_old_states_still_load() {
    let (first, _) = shared();
    first.set_position(Duration::from_nanos(10), "iso", "200").unwrap();
    press(&first, 10, Key::RearPlus, true).unwrap();
    press(&first, 50, Key::RearPlus, true).unwrap();
    first.advance(10);
    let checkpoint = first.checkpoint().unwrap();
    let (second, _) = shared();
    second.restore_checkpoint(&checkpoint).unwrap();
    assert_eq!(second.sample(12, 4095), 500);
    assert_eq!(second.next_deadline(), Some(50));
    assert_eq!(second.take_register(0x36), Some(1));

    // The state shape written before positions and dials existed.
    #[derive(serde::Serialize)]
    struct Old {
        held: std::collections::BTreeSet<Key>,
        pending: Vec<()>,
        now: Duration,
        started: bool,
        used: bool,
    }
    let old = fujimod_emulator_devices::checkpoint::Checkpoint::encode(
        "physical-controls",
        &Old { held: [Key::Menu].into(), pending: vec![], now: Duration::from_secs(3), started: true, used: true },
    )
    .unwrap();
    let (third, _) = shared();
    third.validate_checkpoint(&old).unwrap();
    third.restore_checkpoint(&old).unwrap();
    assert_eq!(third.register_sample(0x34), Some(0));
    assert_eq!(third.sample(12, 4095), 4095);
    assert_eq!(third.take_register(0x36), Some(0));
}

#[test]
fn inconsistent_extensions_do_not_open() {
    let refuses = |edit: &dyn Fn(&mut Options), needle: &str| {
        let mut options = shared_options();
        edit(&mut options);
        let error = LadderKeys::with_pins(options, Arc::new(RecordedPins::default()))
            .err()
            .expect(needle);
        assert!(error.contains(needle), "{error}");
    };
    refuses(&|o| o.ladders[3].sets.push(set(&[Key::Fn1], None, 10, 20)), "declare it as a rung");
    refuses(&|o| o.ladders[3].sets.push(set(&[Key::Menu, Key::Fn1], None, 10, 20)), "has no rung");
    refuses(&|o| o.ladders[3].sets.push(set(&[Key::Fn1], Some("STILL"), 10, 20)), "no contact position");
    refuses(&|o| o.ladders[3].sets.push(set(&[Key::Fn1, Key::Quick], None, 10, 20)), "declared twice");
    refuses(&|o| o.ladders[3].sets[0].band_low = 2000, "overlaps");
    refuses(&|o| o.dials[1].channel = 2, "two dials");
    refuses(&|o| o.dials[1].positions[1].band_high = 4050, "overlaps or repeats");
    refuses(&|o| o.dials[1].positions[1].contact = true, "no ladder shares");
    refuses(&|o| o.switches[0].register = 0x34, "driven twice");
    refuses(&|o| o.rear_dial.as_mut().unwrap().register = 0, "unusable");
    refuses(&|o| o.gpio_keys[0].pin = Pin { port: 0, pin: 9 }, "reuses a GPIO pad");
    refuses(&|o| o.gpio_keys[0].key = Key::Menu, "declared twice");
}
