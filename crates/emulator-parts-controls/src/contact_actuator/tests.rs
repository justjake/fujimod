use super::*;
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::{exr2::gpio, test_host::TestHost};
const MS: u64 = 12_000;
const P9: Pin = Pin { port: 0, pin: 9 };
const P6: Pin = Pin { port: 0, pin: 6 };
fn model() -> (ContactActuator, gpio::Gpio, gpio::Pads, Level, TestHost) {
    let mut levels = [0; 16];
    levels[0] = (1 << 9) | (1 << 6);
    let gpio = gpio::Gpio::new(
        gpio::Options {
            input_levels: levels,
            output_levels: vec![],
            high_port_input_levels: None,
        },
        gpio::Wiring {
            irqs: vec![
                gpio::InputIrq {
                    pin: P9,
                    edge: gpio::Edge::Rising,
                    outputs: vec![InterruptLine::Core { core: 0, line: 17 }],
                },
                gpio::InputIrq {
                    pin: P6,
                    edge: gpio::Edge::Falling,
                    outputs: vec![InterruptLine::Core { core: 0, line: 17 }],
                },
            ],
        },
    )
    .unwrap();
    let pads = gpio.pads();
    let release = Level::new(false);
    // The 12 MHz positions of the catalog part `acquisition-contacts`.
    let position = |ms: f64, contacts| Position {
        run_ticks: (ms * MS as f64) as u64,
        contacts,
    };
    let model = ContactActuator::new(
        Options {
            drive: [Pin { port: 1, pin: 1 }, Pin { port: 1, pin: 2 }],
            contacts: [P9, P6],
            run_drive: 1,
            hold_drives: vec![0, 3],
            positions: vec![
                position(0., [false, false]),
                position(27.5, [false, true]),
                position(43.75, [true, true]),
                position(62.5, [false, true]),
            ],
            initial_run_ticks: 525_000,
            latch_overrun_ticks: 30_000,
            release_ticks: 75_000,
        },
        Wiring {
            pins: Box::new(pads.clone()),
            release: Some(release.clone()),
        },
    )
    .unwrap();
    (model, gpio, pads, release, TestHost::new())
}
fn drive(gpio: &mut gpio::Gpio, h: &mut TestHost, code: u32) {
    let mut cx = h.cx();
    gpio.write(
        &mut cx,
        gpio::BASE + 0x40,
        4,
        ((6 << 16) | (code << 1)).into(),
    );
    assert!(cx.take_refusal().is_none());
}
fn tick(model: &mut ContactActuator, h: &mut TestHost) -> Result<(), String> {
    let mut cx = h.cx();
    model.advance(&mut cx);
    cx.take_refusal().map_or(Ok(()), Err)
}
/// Run the part to `at`, stopping at each of its deadlines on the way.
fn run_to(model: &mut ContactActuator, h: &mut TestHost, at: u64) {
    while let Some(due) = model.next_deadline(h.now).filter(|&due| due <= at) {
        h.now = due;
        tick(model, h).unwrap();
    }
    h.now = at;
}
fn contacts(pads: &gpio::Pads) -> (bool, bool) {
    (pads.input_level(P9).unwrap(), pads.input_level(P6).unwrap())
}
/// X-E2S camera, 1/15 single and continuous stills sampled every 2.5 ms:
/// from charged, drive 1 opens pin 9 after 17.5-20 ms; pin 6 opens 5-7.5 ms
/// after comparator 0's edge; from exposed, drive 1 closes pin 6 after 27.5 ms
/// and pin 9 after 42.5-45 ms. Only pin 9 rising and pin 6 falling interrupt.
#[test]
fn xe2s_card46_release_curtain_and_charge_cycle() {
    let (mut m, mut gpio, pads, release, mut h) = model();
    assert_eq!(m.next_deadline(0), None);
    assert_eq!(contacts(&pads), (true, true));
    drive(&mut gpio, &mut h, 1);
    run_to(&mut m, &mut h, 18 * MS);
    assert_eq!(contacts(&pads), (true, true));
    run_to(&mut m, &mut h, 19 * MS);
    assert_eq!(contacts(&pads), (false, true));
    assert!(h.irqs.is_empty());
    drive(&mut gpio, &mut h, 3);
    run_to(&mut m, &mut h, 40 * MS);
    drive(&mut gpio, &mut h, 0);
    run_to(&mut m, &mut h, 100 * MS);
    assert_eq!(m.next_deadline(h.now), None);
    release.drive(true);
    run_to(&mut m, &mut h, 106 * MS);
    assert_eq!(contacts(&pads), (false, true));
    run_to(&mut m, &mut h, 107 * MS);
    assert_eq!(contacts(&pads), (false, false));
    assert_eq!(h.irqs, vec![(0, 17)]);
    release.drive(false);
    drive(&mut gpio, &mut h, 1);
    run_to(&mut m, &mut h, 134 * MS);
    assert_eq!(contacts(&pads), (false, false));
    run_to(&mut m, &mut h, 135 * MS);
    assert_eq!(contacts(&pads), (false, true));
    run_to(&mut m, &mut h, 150 * MS);
    assert_eq!(contacts(&pads), (false, true));
    run_to(&mut m, &mut h, 151 * MS);
    assert_eq!(contacts(&pads), (true, true));
    assert_eq!(h.irqs, vec![(0, 17), (0, 17)]);
    drive(&mut gpio, &mut h, 3);
    run_to(&mut m, &mut h, 166 * MS);
    drive(&mut gpio, &mut h, 1);
    run_to(&mut m, &mut h, 184 * MS);
    assert_eq!(contacts(&pads), (true, true));
    run_to(&mut m, &mut h, 185 * MS);
    assert_eq!(contacts(&pads), (false, true));
    assert_eq!(m.save()["state"]["transitions"], 5);
}
#[test]
fn held_drive_stops_the_cam_and_keyframes_resume_mid_run() {
    let (mut m, mut gpio, pads, _, mut h) = model();
    drive(&mut gpio, &mut h, 1);
    run_to(&mut m, &mut h, 10 * MS);
    drive(&mut gpio, &mut h, 0);
    run_to(&mut m, &mut h, 500 * MS);
    let saved = m.checkpoint().unwrap();
    let before = m.fingerprint().unwrap();
    drive(&mut gpio, &mut h, 1);
    run_to(&mut m, &mut h, 509 * MS);
    assert_eq!(contacts(&pads), (false, true));
    m.restore_checkpoint(&saved).unwrap();
    assert_eq!(m.fingerprint().unwrap(), before);
    let invalid = fujimod_emulator_devices::checkpoint::Checkpoint::encode(
        m.name(),
        &(
            &m.options,
            State {
                run: u64::MAX,
                ..m.state.clone()
            },
        ),
    )
    .unwrap();
    assert!(m.restore_checkpoint(&invalid).is_err());
    assert_eq!(m.fingerprint().unwrap(), before);
}
#[test]
fn unobserved_motion_refuses() {
    let (mut m, mut gpio, _, _, mut h) = model();
    drive(&mut gpio, &mut h, 2);
    assert!(
        tick(&mut m, &mut h)
            .unwrap_err()
            .contains("no declared motion")
    );
    let (mut m, mut gpio, _, _, mut h) = model();
    drive(&mut gpio, &mut h, 1);
    tick(&mut m, &mut h).unwrap();
    h.now = 60 * MS;
    assert!(
        tick(&mut m, &mut h)
            .unwrap_err()
            .contains("past its release latch")
    );
    let (mut m, _, _, release, mut h) = model();
    release.drive(true);
    assert!(
        tick(&mut m, &mut h)
            .unwrap_err()
            .contains("outside its latch")
    );
}
