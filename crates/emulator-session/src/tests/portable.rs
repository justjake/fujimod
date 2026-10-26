use super::*;
use crate::contact_driver::{Actuator, Check, ControlInput, Pose, Spec as DriverSpec};
use fujimod_emulator_cpu::{Register, arm};
use fujimod_emulator_devices::parts::switch_panel::{Case, Circuit, Control, Signal, Spec};
use fujimod_emulator_imaging::scene::{Frame, FramePixels, SensorFrames};
use std::sync::{Arc, Mutex};

fn session() -> Session {
    let mut session = card_session();
    session
        .board
        .write_register(0, Register::Pstate, 0xd3)
        .unwrap();
    session.board.write_register(0, Register::Pc, 0).unwrap();
    session
        .board
        .write_physical(0, &[1, 0, 0x80, 0xe2, 0xfd, 0xff, 0xff, 0xea])
        .unwrap();
    session.board.write_physical(0x201, &[1]).unwrap();
    session
        .configure_controls(Spec {
            controls: vec![Control {
                id: "menu".into(),
                label: "MENU/OK".into(),
                kind: "button".into(),
                positions: vec!["released".into(), "pressed".into()],
                initial: "released".into(),
                unavailable: None,
            }],
            circuits: vec![Circuit {
                controls: vec!["menu".into()],
                cases: [("released", 1023), ("pressed", 21)]
                    .into_iter()
                    .map(|(position, value)| Case {
                        positions: vec![position.into()],
                        signals: vec![Signal::Analog {
                            node: "adc".into(),
                            channel: 8,
                            value,
                        }],
                    })
                    .collect(),
            }],
        })
        .unwrap();
    session
        .configure_contact_driver(DriverSpec {
            poll_ticks: 1,
            max_phase_ticks: 1000,
            catalog: vec![ControlInput {
                id: "menu".into(),
                label: "MENU/OK".into(),
                kind: "button".into(),
                supported: true,
                unavailable: None,
                positions: None,
                initial: None,
                relative: None,
                width: None,
                height: None,
                stages: None,
            }],
            actuators: vec![Actuator {
                id: "menu".into(),
                momentary: true,
                states: [("released", 0), ("pressed", 1)]
                    .into_iter()
                    .map(|(position, ack)| Pose {
                        position: position.into(),
                        ready: vec![Check {
                            address: 0x201,
                            bytes: vec![1],
                            pointer: None,
                        }],
                        expected: vec![Check {
                            address: 0x200,
                            bytes: vec![ack],
                            pointer: None,
                        }],
                    })
                    .collect(),
            }],
            interfaces: vec![],
        })
        .unwrap();
    fujimod_emulator_exr2_images::images_mut(&mut session.resources).unwrap().sensors.insert(
        "scene".into(),
        Arc::new(Mutex::new(SensorFrames::new(
            Frame::new(1, 1, FramePixels::Rgb24(vec![1, 2, 3].into())).unwrap(),
        ))),
    );
    session
}
fn continue_session(session: &mut Session) {
    session.board.write_physical(0x200, &[1]).unwrap();
    session.pump_contacts().unwrap();
    assert_eq!(session.adc_samples("adc").unwrap()[8], 1023);
    session.board.write_physical(0x200, &[0]).unwrap();
    session.pump_contacts().unwrap();
    session.board.run_for_instructions(200, 17).unwrap();
    assert!(!session.contact_driver.as_ref().unwrap().busy());
}

#[test]
fn durable_session_restores_media_sensor_inputs_contacts_and_queued_actions_then_resumes() {
    let (bytes, before, continued) = {
        let mut source = session();
        let mut card = source.card_medium("sd").unwrap();
        card.write_bytes(100000 * 512, b"captured card sector")
            .unwrap();
        source.retain_card("sd", card, false, true).unwrap();
        fujimod_emulator_exr2_images::images_mut(&mut source.resources).unwrap().sensors["scene"]
            .lock()
            .unwrap()
            .latch(0)
            .unwrap();
        source.sensor_rgb("scene", 1, 1, &[9, 8, 7], 1000.).unwrap();
        source.board.run_for_instructions(13, 17).unwrap();
        source.contact_input("menu", None, false).unwrap();
        source.contact_input("menu", None, true).unwrap();
        assert_eq!(source.adc_samples("adc").unwrap()[8], 21);
        assert!(source.contact_driver.as_ref().unwrap().busy());
        let saved = source.checkpoint(None).unwrap();
        let bytes = saved.export().unwrap();
        let before = source.fingerprint().unwrap();
        assert_eq!(saved.export().unwrap(), bytes);
        assert_eq!(source.fingerprint().unwrap(), before);
        continue_session(&mut source);
        (bytes, before, source.fingerprint().unwrap())
    };
    let mut target = session();
    let sensor = fujimod_emulator_exr2_images::images_mut(&mut target.resources).unwrap().sensors["scene"].clone();
    let initial = target.fingerprint().unwrap();
    let saved = target.import_snapshot(&bytes).unwrap();
    assert_eq!(target.fingerprint().unwrap(), initial);
    target.restore(&saved).unwrap();
    assert_eq!(target.fingerprint().unwrap(), before);
    assert!(Arc::ptr_eq(
        &fujimod_emulator_exr2_images::images_mut(&mut target.resources).unwrap().sensors["scene"],
        &sensor
    ));
    let mut sector = [0; 20];
    target
        .card_medium("sd")
        .unwrap()
        .read_bytes(100000 * 512, &mut sector)
        .unwrap();
    assert_eq!(&sector, b"captured card sector");
    continue_session(&mut target);
    assert_eq!(target.fingerprint().unwrap(), continued);
    assert_eq!(target.board.read_register(0, arm::CORE[0]).unwrap(), 107);
}

#[test]
fn incompatible_session_or_corrupt_archive_is_refused_without_changing_live_state() {
    let source = session();
    let bytes = source.checkpoint(None).unwrap().export().unwrap();
    let mut target = session();
    let before = target.fingerprint().unwrap();
    let mut bad = bytes.clone();
    bad.push(0);
    assert!(target.import_snapshot(&bad).is_err());
    assert_eq!(target.fingerprint().unwrap(), before);
    target.definition["fixture"] = serde_json::json!("different-constructor");
    let before = target.fingerprint().unwrap();
    assert!(target.import_snapshot(&bytes).is_err());
    assert_eq!(target.fingerprint().unwrap(), before);
    let mut target = session();
    target.nodes.remove("adc");
    let before = target.fingerprint().unwrap();
    assert!(target.import_snapshot(&bytes).is_err());
    assert_eq!(target.fingerprint().unwrap(), before);
}
