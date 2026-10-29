use super::*;
use crate::contact_driver::{Actuator, Check, ControlInput, Pose, Spec as DriverSpec};
use fujimod_emulator_cpu::{Register, arm};
use fujimod_emulator_devices::parts::switch_panel::{Case, Circuit, Control, Signal, Spec};
use fujimod_emulator_imaging::scene::{Frame, FramePixels, SensorFrames};
use std::sync::{Arc, Mutex};

#[test]
fn durable_card_uses_the_new_hosts_allocation_limit_without_changing_saved_files() {
    let mut source = card_session();
    let mut card = source.card_medium("sd").unwrap();
    let old_limit = card.dirty_sectors();
    card.set_allocation_limit(old_limit).unwrap();
    let revision = card.revision();
    let capacity = card.sectors();
    source.retain_card("sd", card, true, false).unwrap();
    let spec = crate::durable::Spec {
        cards: vec!["sd".into()],
        nands: vec![],
    };
    let saved = crate::durable::Media::capture(&source, &spec).unwrap();
    let mut cold = card_session();
    let limit = cold.card_medium("sd").unwrap().allocation_limit();
    assert!(limit > old_limit);
    saved.apply(&mut cold, &spec).unwrap();
    let retained = cold.card_medium("sd").unwrap();
    assert_eq!(retained.allocation_limit(), limit);
    assert_eq!(retained.sectors(), capacity);
    assert_eq!(retained.revision(), revision);
    assert_eq!(
        cold.card_read("sd", "/DCIM/100_FUJI/A.JPG", Some(revision), 100)
            .unwrap(),
        b"card bytes"
    );
    retained
        .reserve_write(0x100000, old_limit as u32 + 1)
        .unwrap();
}

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

#[test]
fn serialized_checkpoint_resumes_a_fresh_session_and_checks_image_and_graph_identity() {
    let definition = serde_json::json!({
        "inputs": {
            "update": {"size": 8, "sha256": "1111111111111111111111111111111111111111111111111111111111111111"},
            "boot_rom": {"content": {"size": 4, "sha256": "2222222222222222222222222222222222222222222222222222222222222222"}}
        },
        "execution": {"processors": [{"model": "test-core"}]},
        "devices": {"nodes": [{"id": "timer", "kind": "timer"}]},
        "scene": {"images": []}
    });
    let (bytes, before, after) = {
        let mut source = session();
        source.definition = definition.clone();
        source.board.run_for_instructions(13, 17).unwrap();
        let checkpoint = source.checkpoint(None).unwrap();
        let bytes = checkpoint.export().unwrap();
        let (ticks, counts) = crate::checkpoint::Snapshot::inspect(&bytes).unwrap();
        assert_eq!((ticks, counts), (checkpoint.now(), checkpoint.instruction_counts().to_vec()));
        let before = source.fingerprint().unwrap();
        source.board.run_for_instructions(200, 17).unwrap();
        (bytes, before, source.fingerprint().unwrap())
    };

    let mut target = session();
    target.definition = definition.clone();
    let checkpoint = target.import_snapshot(&bytes).unwrap();
    target.restore(&checkpoint).unwrap();
    assert_eq!(target.fingerprint().unwrap(), before);
    target.board.run_for_instructions(200, 17).unwrap();
    assert_eq!(target.fingerprint().unwrap(), after);

    let mut different_image = session();
    different_image.definition = definition.clone();
    different_image.definition["inputs"]["update"]["sha256"] =
        serde_json::json!("3333333333333333333333333333333333333333333333333333333333333333");
    assert!(different_image.import_snapshot(&bytes).is_err());

    let mut different_graph = session();
    different_graph.definition = definition;
    different_graph.definition["devices"]["nodes"][0]["kind"] = serde_json::json!("other");
    assert!(different_graph.import_snapshot(&bytes).is_err());
}
