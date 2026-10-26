//! `parts::resistor_keys` driven through XP5 parts.
use fujimod_emulator_devices::parts::controls::Controls;
use fujimod_emulator_devices::parts::pins::{DriveInput, Pin};
use serde_json::Value;
use std::{
    sync::Arc,
    time::Duration,
};
use crate::resistor_keys::*;
use fujimod_emulator_devices::xp5::pins::{Pins, SharedPins};

fn options() -> Options {
    Options {
        layout: Layout {
            shutter_half: Pin { port: 0, pin: 0 },
            shutter_full: Pin { port: 0, pin: 1 },
            rear_dial: [Pin { port: 5, pin: 7 }, Pin { port: 5, pin: 9 }],
            front_dial: [Pin { port: 5, pin: 8 }, Pin { port: 13, pin: 5 }],
        },
        initial_levels: [
            (0, 0, true),
            (0, 1, true),
            (5, 7, false),
            (5, 9, false),
            (5, 8, false),
            (13, 5, false),
            (11, 4, false),
            (4, 4, false),
        ]
        .into_iter()
        .map(|(port, pin, high)| InitialLevel {
            pin: Pin { port, pin },
            high,
        })
        .collect(),
        samples: KeySamples {
            menu: 3472,
            back: 2227,
            menu_back: 1999,
            up: 1558,
            down: 3409,
            left: 2829,
            right: 2068,
        },
        detent_return_ns: 60_000_000,
        shutter_half_delay_ns: 30_000_000,
    }
}
fn input() -> (ResistorKeys, SharedPins) {
    let pins = Pins::shared();
    let driver: Arc<dyn DriveInput> = pins.clone();
    (ResistorKeys::new(driver, options()).unwrap(), pins)
}
#[test]
fn resistor_ladders_are_explicit_and_inspection_is_observational() {
    let (input, _) = input();
    for (key, channel, sample) in [
        (Key::Menu, 11, 3472),
        (Key::Back, 11, 2227),
        (Key::Up, 14, 1558),
        (Key::Down, 14, 3409),
        (Key::Left, 14, 2829),
        (Key::Right, 14, 2068),
    ] {
        input.enqueue(Duration::ZERO, key, true).unwrap();
        let before = input.save();
        assert_eq!(input.sample(channel, 4095), 4095);
        assert_eq!(input.save(), before);
        input.advance(0);
        assert_eq!(input.sample(channel, 4095), sample);
        input.enqueue(Duration::ZERO, key, false).unwrap();
        input.advance(0);
    }
    input.script("t=1:MENU;t=1:BACK").unwrap();
    input.advance(1_000_000_000);
    assert_eq!(input.sample(11, 4095), 1999);
}
#[test]
fn shutter_sequence_and_dial_return_use_edge_time_not_service_frequency() {
    let (frequent, frequent_pins) = input();
    let (batched, batched_pins) = input();
    let script = "t=0.1:FULL;t=0.2:-FULL;t=0.3:REAR+;t=0.4:REAR-;t=0.5:FRONT+;t=0.6:FRONT-";
    for input in [&frequent, &batched] {
        input.script(script).unwrap();
    }
    frequent.advance(100_000_000);
    assert_eq!(frequent_pins.lock().unwrap().word(0) & 3, 2); // S1 only
    frequent.advance(129_999_999);
    assert_eq!(frequent_pins.lock().unwrap().word(0) & 3, 2);
    frequent.advance(130_000_000);
    assert_eq!(frequent_pins.lock().unwrap().word(0) & 3, 0); // S1+S2
    frequent.advance(200_000_000);
    assert_eq!(frequent_pins.lock().unwrap().word(0) & 3, 2);
    frequent.advance(230_000_000);
    assert_eq!(frequent_pins.lock().unwrap().word(0) & 3, 3);
    for tick in [
        300_000_000,
        360_000_000,
        400_000_000,
        460_000_000,
        500_000_000,
        560_000_000,
        600_000_000,
        660_000_000,
        1_000_000_000,
    ] {
        frequent.advance(tick);
    }
    batched.advance(1_000_000_000);
    assert_eq!(frequent.save(), batched.save());
    assert_eq!(
        serde_json::to_value(&*frequent_pins.lock().unwrap()).unwrap(),
        serde_json::to_value(&*batched_pins.lock().unwrap()).unwrap()
    );
    let saved = batched.save();
    assert!(batched.restore(&Value::Null).is_err());
    assert_eq!(batched.save(), saved);
}
