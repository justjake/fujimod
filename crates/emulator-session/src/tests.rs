use super::*;
mod portable;
use fujimod_emulator::board::{
    bus::{Bus, Mode},
    factory,
};
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_ip::sdhci::host::{self as sdhc, SdHost};
use fujimod_emulator_storage::fat;

fn card_session() -> Session {
    let contacts = fujimod_emulator_devices::parts::card_contacts::Contacts::new(true, false);
    let card = fat::format(
        &format(),
        &[("/DCIM/100_FUJI/A.JPG".into(), b"card bytes".to_vec())],
    )
    .unwrap();
    let host = SdHost::new(
        sdhc::Options {
            base: 0xfff50000,
            protocol: sdhc::Protocol::Sdhc,
            transfer_ticks: 2,
            clock_ticks: 2,
            rca: 0x1234,
            ram_bytes: 0x1000,
        },
        sdhc::Wiring {
            card,
            contacts: contacts.clone(),
            irq: InterruptLine::Core { core: 0, line: 80 },
        },
    )
    .unwrap();
    let mut bus = Bus::new(Mode::Strict);
    let id = bus.register(0xfff50000, 0x108, Box::new(host)).unwrap();
    let analog = fujimod_emulator_exr2_io::adc::controller::Converter::new(
        fujimod_emulator_exr2_io::adc::controller::Options {
            conversion_ticks: 7,
            samples: [1023; 12],
        },
    )
    .unwrap();
    let adc = bus.register(0xfff8a000, 0x60, Box::new(analog)).unwrap();
    use fujimod_emulator_devices::{exr2::gpio, parts::pins::Pin};
    use fujimod_emulator_exr2_resources::CardPins;
    let gpio = gpio::Gpio::new(
        gpio::Options {
            input_levels: [0; 16],
            output_levels: Vec::new(),
        },
        gpio::Wiring { irqs: Vec::new() },
    )
    .unwrap();
    let pads = gpio.pads();
    let gpio_id = bus.register(0xfff89000, 0x480, Box::new(gpio)).unwrap();
    let mut resources = Resources::default();
    resources.card_pins.insert(
        "sd".into(),
        CardPins {
            gpio: gpio_id,
            pads,
            detect: Pin { port: 12, pin: 7 },
            present_high: false,
            protect: Pin { port: 12, pin: 6 },
            protected_high: true,
        },
    );
    let graph:factory::Graph=serde_json::from_value(serde_json::json!({
        "processors":[{"model":"cortex-r5f","registers":[],"released":true,"mprofile":null,"rprofile":{"cp14_identity":21930,"tcm":{"btcm":{"bytes":4096,"reset":1879048193},"atcm":{"bytes":8192,"reset":4294934529u32}}}}],
        "ram":[{"base":0,"size":4096}],"dma_mappings":[{"address":0,"physical":0,"size":4096,"permissions":3}],
        "mappings":[{"core":0,"address":0,"physical":0,"size":4096,"permissions":7}],
        "peripherals":[],"decoders":[],"instructions_per_tick":[24,1]
    })).unwrap();
    let board = graph
        .build(
            factory::Options::new(factory::Engine::Unicorn)
                .with_translation_cache_size_bytes(1 << 20),
            bus,
            None,
        )
        .unwrap();
    Session {
        definition: serde_json::json!({"fixture":"card-session"}),
        board,
        controls: None,
        contact_driver: None,
        resources,
        nodes: BTreeMap::from([("sd".into(), id), ("adc".into(), adc)]),
        contacts: BTreeMap::from([("sd".into(), contacts)]),
        recording_frames: Default::default(),
    }
}
fn format() -> fat::Options {
    fat::Options {
        sectors: 0x800000,
        max_dirty_sectors: 131072,
        partition_start: 2048,
        sectors_per_cluster: 64,
        label: "TEST".into(),
        read_only_files: Vec::new(),
    }
}
#[test]
fn card_observations_revision_retention_and_format_are_shared_session_operations() {
    let mut session = card_session();
    let before = session.board.state();
    let state = session.card_state("sd").unwrap();
    let revision = state.revision.unwrap();
    assert_eq!(
        session.card_tree("sd", Some(revision)).unwrap().entries[0].path,
        "/DCIM"
    );
    assert_eq!(
        session
            .card_read("sd", "/DCIM/100_FUJI/A.JPG", Some(revision), 100)
            .unwrap(),
        b"card bytes"
    );
    assert_eq!(session.board.state(), before);
    assert!(
        session
            .card_tree("sd", Some(revision + 1))
            .unwrap_err()
            .contains("revision")
    );
    session.card_contacts("sd", false, false).unwrap();
    assert!(
        session
            .card_read("sd", "/DCIM/100_FUJI/A.JPG", None, 100)
            .is_err()
    );
    session.card_contacts("sd", true, false).unwrap();
    let medium = session.card_medium("sd").unwrap();
    let mut cold = card_session();
    cold.retain_card("sd", medium, true, false).unwrap();
    assert_eq!(cold.card_state("sd").unwrap(), state);
    cold.card_format("sd", &format(), &[("/B.TXT".into(), b"new bytes".to_vec())])
        .unwrap();
    assert!(cold.card_state("sd").unwrap().revision.unwrap() > revision);
    assert_eq!(
        cold.card_read("sd", "/B.TXT", None, 100).unwrap(),
        b"new bytes"
    );
    assert!(cold.card_read("sd", "/B.TXT", Some(revision), 100).is_err());
    assert_eq!(
        session
            .card_read("sd", "/DCIM/100_FUJI/A.JPG", Some(revision), 100)
            .unwrap(),
        b"card bytes"
    );
    assert!(
        session
            .card_read("sd", "/DCIM/100_FUJI/A.JPG", None, 0)
            .is_err()
    );
}

#[test]
fn initial_revision_is_external_metadata_only_before_guest_execution() {
    let mut session = card_session();
    session.initialize_card_revision("sd", 123).unwrap();
    assert_eq!(session.card_state("sd").unwrap().revision, Some(123));
    session
        .board
        .write_memory(0, 0, &0xeafffffeu32.to_le_bytes())
        .unwrap();
    session
        .board
        .write_register(0, fujimod_emulator_cpu::Register::Pstate, 0xd3)
        .unwrap();
    session
        .board
        .write_register(0, fujimod_emulator_cpu::Register::Pc, 0)
        .unwrap();
    session.board.run_for_instructions(1, 8).unwrap();
    assert!(session.initialize_card_revision("sd", 456).is_err());
    assert_eq!(session.card_state("sd").unwrap().revision, Some(123));
}

#[test]
fn scene_rgb_prepares_only_the_next_sensor_exposure() {
    use fujimod_emulator_imaging::scene::{Frame, FramePixels, SensorFrames};
    use std::sync::{Arc, Mutex};
    let mut session = card_session();
    let input = Arc::new(Mutex::new(SensorFrames::new(
        Frame::new(1, 1, FramePixels::Rgb24(vec![1, 2, 3].into())).unwrap(),
    )));
    fujimod_emulator_exr2_images::images_mut(&mut session.resources).unwrap().sensors
        .insert("sensor".into(), input.clone());
    let saved = session.checkpoint(None).unwrap();
    let mut history = super::history::History::new(2, 128 << 20).unwrap();
    history
        .apply(
            &mut session,
            super::history::Input::SensorRgb {
                source: "sensor".into(),
                width: 1,
                height: 1,
                bytes: vec![4, 5, 6],
                white_luminance: 1000.,
            },
        )
        .unwrap();
    let straight = session.fingerprint().unwrap();
    let log = fujimod_emulator_devices::checkpoint::Checkpoint::encode(
        "scene-input-log",
        &history.entries(),
    )
    .unwrap();
    let replay: Vec<super::history::Entry> = log.decode("scene-input-log").unwrap();
    session.restore(&saved).unwrap();
    super::history::History::replay(&mut session, &replay).unwrap();
    assert_eq!(session.fingerprint().unwrap(), straight);
    assert!(session.sensor_rgb("sensor", 1, 1, &[7], 10000.).is_err());
    assert!(
        session
            .sensor_rgb("absent", 1, 1, &[4, 5, 6], 10000.)
            .is_err()
    );
    for luminance in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(
            session
                .sensor_rgb("sensor", 1, 1, &[7, 8, 9], luminance)
                .is_err()
        );
    }
    let input = input.lock().unwrap();
    assert_eq!(input.current.white_luminance, 1000.);
    assert_eq!(input.latched.white_luminance, 10000.);
    assert!(matches!(&input.current.pixels,FramePixels::Rgb24(bytes) if bytes.as_ref()==&[4,5,6]));
    assert!(matches!(&input.latched.pixels,FramePixels::Rgb24(bytes) if bytes.as_ref()==&[1,2,3]));
    assert_eq!(input.edge, None);
    assert_eq!(session.board.now(), 0);
}

#[test]
fn physical_controls_validate_every_target_and_change_only_external_electrical_inputs() {
    use fujimod_emulator_devices::parts::switch_panel::{Case, Circuit, Control, Signal, Spec};
    let mut session = card_session();
    let signals = |node: &str, value| {
        vec![Signal::Analog {
            node: node.into(),
            channel: 8,
            value,
        }]
    };
    let mut spec = Spec {
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
            cases: vec![
                Case {
                    positions: vec!["released".into()],
                    signals: signals("adc", 1023),
                },
                Case {
                    positions: vec!["pressed".into()],
                    signals: signals("absent", 21),
                },
            ],
        }],
    };
    assert!(session.configure_controls(spec.clone()).is_err());
    assert!(session.controls.is_none());
    assert_eq!(session.adc_samples("adc").unwrap()[8], 1023);
    spec.circuits[0].cases[1].signals = signals("adc", 21);
    session.configure_controls(spec).unwrap();
    let before = session.board.now();
    session.set_position("menu", "pressed").unwrap();
    assert_eq!(session.adc_samples("adc").unwrap()[8], 21);
    let handle = session
        .board
        .inspect_device::<fujimod_emulator_exr2_io::adc::controller::Converter, _>(
            session.node("adc").unwrap(),
            |adc| Ok(adc.external_inputs()),
        )
        .unwrap();
    assert_eq!(handle.values()[8], 21);
    assert!(session.set_position("menu", "unknown").is_err());
    assert_eq!(
        session.control_state().unwrap().positions["menu"],
        "pressed"
    );
    let saved = session.checkpoint(None).unwrap();
    session.set_position("menu", "released").unwrap();
    assert_eq!(handle.values()[8], 1023);
    session.restore(&saved).unwrap();
    assert_eq!(handle.values()[8], 21);
    assert_eq!(
        session.control_state().unwrap().positions["menu"],
        "pressed"
    );
    session.set_position("menu", "released").unwrap();
    assert_eq!(session.board.now(), before);
    assert_eq!(session.board.instruction_counts(), vec![0]);
}

#[test]
fn session_keyframe_and_input_replay_preserve_card_adc_and_fractional_cpu_boundary() {
    use super::history::{Boundary, History, Input};
    use fujimod_emulator_cpu::{Register, arm};
    let mut session = card_session();
    session
        .board
        .write_register(0, Register::Pstate, 0xd3)
        .unwrap();
    session.board.write_register(0, Register::Pc, 0).unwrap();
    session
        .board
        .write_physical(0, &[0x01, 0x00, 0x80, 0xe2, 0xfd, 0xff, 0xff, 0xea])
        .unwrap();
    let mut history = History::new(8, usize::MAX).unwrap();
    history.mark(&session).unwrap();
    history
        .apply(
            &mut session,
            Input::Run {
                instructions: 13,
                quantum: 17,
                pause_after: None,
                until: None,
            },
        )
        .unwrap();
    assert_eq!(session.board.now(), 0);
    history
        .apply(
            &mut session,
            Input::Adc {
                node: "adc".into(),
                samples: vec![837; 12],
            },
        )
        .unwrap();
    let before = session.checkpoint(None).unwrap();
    let cursor = history.entries().len();
    history.mark(&session).unwrap();
    history
        .apply(
            &mut session,
            Input::Run {
                instructions: 200,
                quantum: 17,
                pause_after: None,
                until: None,
            },
        )
        .unwrap();
    history
        .apply(
            &mut session,
            Input::CardContacts {
                node: "sd".into(),
                present: false,
                protected: true,
            },
        )
        .unwrap();
    let straight = session.fingerprint().unwrap();
    let encoded = fujimod_emulator_devices::checkpoint::Checkpoint::encode(
        "input-log",
        &&history.entries()[cursor..],
    )
    .unwrap();
    let replay: Vec<super::history::Entry> = encoded.decode("input-log").unwrap();
    session.restore(&before).unwrap();
    assert_eq!(session.board.instruction_counts(), [13]);
    assert!(session.card_state("sd").unwrap().present);
    assert_eq!(session.adc_samples("adc").unwrap(), vec![837; 12]);
    History::replay(&mut session, &replay).unwrap();
    assert_eq!(session.fingerprint().unwrap(), straight);
    history.undo(&mut session).unwrap();
    assert_eq!(session.board.instruction_counts(), [13]);
    assert_eq!(session.board.read_register(0, arm::CORE[0]).unwrap(), 7);
    assert!(History::replay(&mut session, &replay[1..]).is_err());
    let rollback_start = Boundary::capture(&session);
    history
        .apply(
            &mut session,
            Input::Run {
                instructions: 240,
                quantum: 17,
                pause_after: None,
                until: None,
            },
        )
        .unwrap();
    history.rollback(&mut session, 3).unwrap();
    assert_eq!(session.board.now(), 3);
    let rolled = session.fingerprint().unwrap();
    // Repeat the same target directly from the retained earlier state.
    session.restore(&before).unwrap();
    assert_eq!(Boundary::capture(&session), rollback_start);
    session
        .board
        .run_with_limits(240, 17, None, Some(3))
        .unwrap();
    assert_eq!(session.fingerprint().unwrap(), rolled);
}

#[test]
fn history_replay_instruction_boundaries_follow_execution_policy() {
    use super::history::{Boundary, Entry, History, Input};
    use fujimod_emulator_cpu::ExecutionPolicy;

    for mismatch_before in [true, false] {
        for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
            let mut session = card_session();
            session.board.set_execution_policy(policy).unwrap();
            let boundary = Boundary::capture(&session);
            let mut before = boundary.clone();
            let mut after = boundary;
            if mismatch_before {
                before.instructions[0] += 1;
            } else {
                after.instructions[0] += 1;
            }
            let entry = Entry {
                before,
                input: Input::Adc {
                    node: "adc".into(),
                    samples: vec![837; 12],
                },
                after,
                failure: None,
            };

            let result = History::replay(&mut session, &[entry]);
            assert_eq!(
                result.is_ok(),
                policy == ExecutionPolicy::Interactive,
                "policy={policy:?}, mismatch_before={mismatch_before}"
            );
        }
    }

    let mut session = card_session();
    session
        .board
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    let boundary = Boundary::capture(&session);
    let refusal = Entry {
        before: boundary.clone(),
        input: Input::Adc {
            node: "absent".into(),
            samples: vec![837; 12],
        },
        after: boundary,
        failure: None,
    };
    assert!(History::replay(&mut session, &[refusal]).is_err());
}

#[test]
fn history_rollback_bounds_interactive_run_and_keeps_exact_keyframe_restore() {
    use super::history::{Boundary, History, Input};
    use fujimod_emulator_cpu::{ExecutionPolicy, Register};

    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        let mut session = card_session();
        session.board.set_execution_policy(policy).unwrap();
        session
            .board
            .write_register(0, Register::Pstate, 0xd3)
            .unwrap();
        session.board.write_register(0, Register::Pc, 0).unwrap();
        session
            .board
            .write_physical(0, &[0xfe, 0xff, 0xff, 0xea])
            .unwrap();

        let initial = session.fingerprint().unwrap();
        let initial_boundary = Boundary::capture(&session);
        let mut history = History::new(2, usize::MAX).unwrap();
        history.mark(&session).unwrap();
        history
            .apply(
                &mut session,
                Input::Run {
                    instructions: 240,
                    quantum: 17,
                    pause_after: None,
                    until: None,
                },
            )
            .unwrap();
        assert!(session.board.now() > 3);

        let reached = history.rollback(&mut session, 3).unwrap();
        if policy == ExecutionPolicy::Exact {
            assert_eq!(reached.tick, 3);
        } else {
            assert!(reached.tick <= 3);
        }
        assert_eq!(history.entries().len(), 1);
        let replayed = &history.entries()[0];
        assert_eq!(replayed.before, initial_boundary);
        assert_eq!(replayed.after, reached);
        assert!(matches!(&replayed.input, Input::Run { until: Some(3), .. }));

        history.undo(&mut session).unwrap();
        assert_eq!(session.fingerprint().unwrap(), initial);
        assert_eq!(Boundary::capture(&session), initial_boundary);
    }
}

#[test]
fn rust_card_insertion_drives_the_declared_native_gpio_contacts() {
    let mut session = card_session();
    let pins = session.resources.card_pins.get("sd").unwrap();
    let pads = pins.pads.clone();
    let detect = pins.detect;
    let protect = pins.protect;
    let before = (session.board.now(), session.board.instruction_counts());
    session.card_contacts("sd", false, true).unwrap();
    assert_eq!(
        (session.board.now(), session.board.instruction_counts()),
        before
    );
    assert!(!pads.input_level(detect).unwrap());
    assert!(!pads.input_level(protect).unwrap());
    assert_eq!(session.card_state("sd").unwrap().present, false);
    assert_eq!(session.card_state("sd").unwrap().protected, true);
    session
        .board
        .write_memory(0, 0, &0xeafffffeu32.to_le_bytes())
        .unwrap();
    session
        .board
        .write_register(0, fujimod_emulator_cpu::Register::Pc, 0)
        .unwrap();
    session.board.run_for_instructions(1, 8).unwrap();
    assert!(pads.input_level(detect).unwrap());
    assert!(pads.input_level(protect).unwrap());
}

#[test]
fn card_insertion_refuses_full_gpio_queue_before_replacing_media() {
    let mut session = card_session();
    let pins = session.resources.card_pins.get("sd").unwrap();
    pins.pads
        .request_many(&vec![(pins.detect, true); 1024])
        .unwrap();
    let before = session.board.state();
    let replacement = fat::format(&format(), &[("/NEW.TXT".into(), b"new".to_vec())]).unwrap();
    assert!(
        session
            .insert_card("sd", replacement, false, false)
            .err()
            .unwrap()
            .contains("queue")
    );
    assert_eq!(session.board.state(), before);
    assert_eq!(
        session
            .card_read("sd", "/DCIM/100_FUJI/A.JPG", None, 100)
            .unwrap(),
        b"card bytes"
    );
}

#[test]
fn sensor_raw16_replaces_the_current_frame_without_changing_the_latched_frame() {
    use fujimod_emulator_imaging::scene::{Frame, FramePixels, SensorFrames};
    use std::sync::{Arc, Mutex};
    let mut session = card_session();
    let sensor = Arc::new(Mutex::new(SensorFrames::new(
        Frame::new(2, 1, FramePixels::Raw16(vec![1, 2])).unwrap(),
    )));
    fujimod_emulator_exr2_images::images_mut(&mut session.resources).unwrap().sensors
        .insert("sensor".into(), sensor.clone());
    session
        .sensor_raw16("sensor", 3, 1, &[3, 0, 4, 0, 5, 0])
        .unwrap();
    let frames = sensor.lock().unwrap();
    assert!(matches!(&frames.current.pixels, FramePixels::Raw16(p) if p == &[3, 4, 5]));
    assert!(matches!(&frames.latched.pixels, FramePixels::Raw16(p) if p == &[1, 2]));
}

#[test]
fn sensor_radiance_is_retained_by_serialized_input_history_and_undo() {
    use super::history::{Entry, History, Input};
    use fujimod_emulator_imaging::scene::{Frame, FramePixels, SensorFrames};
    use std::sync::{Arc, Mutex};
    let mut session = card_session();
    let sensor = Arc::new(Mutex::new(SensorFrames::new(
        Frame::new(1, 1, FramePixels::Rgb24(vec![1, 2, 3].into())).unwrap(),
    )));
    fujimod_emulator_exr2_images::images_mut(&mut session.resources).unwrap().sensors
        .insert("sensor".into(), sensor.clone());
    let initial = sensor.lock().unwrap().fingerprint();
    let mut history = History::new(2, 128 << 20).unwrap();
    history.mark(&session).unwrap();
    history
        .apply(
            &mut session,
            Input::SensorRgb {
                source: "sensor".into(),
                width: 1,
                height: 1,
                bytes: vec![4, 5, 6],
                white_luminance: 250.,
            },
        )
        .unwrap();
    let changed = sensor.lock().unwrap().fingerprint();
    assert_eq!(sensor.lock().unwrap().current.white_luminance, 250.);
    let recorded = serde_json::to_vec(history.entries()).unwrap();
    history.undo(&mut session).unwrap();
    assert_eq!(sensor.lock().unwrap().fingerprint(), initial);
    let entries: Vec<Entry> = serde_json::from_slice(&recorded).unwrap();
    History::replay(&mut session, &entries).unwrap();
    assert_eq!(sensor.lock().unwrap().fingerprint(), changed);
    assert_eq!(sensor.lock().unwrap().current.white_luminance, 250.);
}

#[test]
fn history_device_budget_expires_old_marks_without_losing_latest_undo() {
    let mut session = card_session();
    session
        .board
        .write_register(0, fujimod_emulator_cpu::Register::Pstate, 0xd3)
        .unwrap();
    session
        .board
        .write_register(0, fujimod_emulator_cpu::Register::Pc, 0)
        .unwrap();
    session
        .board
        .write_physical(0, &[0xfe, 0xff, 0xff, 0xea])
        .unwrap();
    let mut history = super::history::History::new(8, 1).unwrap();
    history.mark(&session).unwrap();
    history
        .apply(
            &mut session,
            super::history::Input::Run {
                instructions: 120,
                quantum: 17,
                pause_after: None,
                until: None,
            },
        )
        .unwrap();
    let expected = session.board.now();
    history.mark(&session).unwrap();
    assert_eq!(history.retained_marks(), 1);
    history
        .apply(
            &mut session,
            super::history::Input::Run {
                instructions: 120,
                quantum: 17,
                pause_after: None,
                until: None,
            },
        )
        .unwrap();
    history.undo(&mut session).unwrap();
    assert_eq!(session.board.now(), expected);
}

#[test]
fn automatic_history_coalesces_interactive_inputs_and_keeps_exact_boundaries() {
    use super::history::{Boundary, History, Input};
    use fujimod_emulator_cpu::{ExecutionPolicy, Register};
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        let mut session = card_session();
        session.board.set_execution_policy(policy).unwrap();
        session
            .board
            .write_register(0, Register::Pstate, 0xd3)
            .unwrap();
        session.board.write_register(0, Register::Pc, 0).unwrap();
        session
            .board
            .write_physical(0, &[0xfe, 0xff, 0xff, 0xea])
            .unwrap();
        let mut history = History::new(8, usize::MAX).unwrap();
        let mut captured = Vec::new();
        for index in 0..40 {
            let input = if index % 2 == 0 {
                Input::Run {
                    instructions: 120,
                    quantum: 17,
                    pause_after: None,
                    until: None,
                }
            } else {
                Input::Adc {
                    node: "adc".into(),
                    samples: vec![index; 12],
                }
            };
            if history.mark_automatic(&session, &input).unwrap() {
                captured.push(Boundary::capture(&session));
            }
            history.apply(&mut session, input).unwrap();
        }
        if policy == ExecutionPolicy::Exact {
            assert_eq!(captured.len(), 40);
            assert_eq!(history.retained_marks(), 8);
        } else {
            assert_eq!(captured.len(), 3);
            assert_eq!(history.entries().len(), 40);
            let entries = history.entries().to_vec();
            let final_state = session.fingerprint().unwrap();
            assert_eq!(history.undo(&mut session).unwrap(), captured[2]);
            History::replay(&mut session, &entries[32..]).unwrap();
            assert_eq!(session.fingerprint().unwrap(), final_state);
            // An explicit mark remains immediate even inside an automatic group.
            let explicit = Boundary::capture(&session);
            history.mark(&session).unwrap();
            assert_eq!(history.retained_marks(), 3);
            history
                .apply(
                    &mut session,
                    Input::Run {
                        instructions: 120,
                        quantum: 17,
                        pause_after: None,
                        until: None,
                    },
                )
                .unwrap();
            assert_eq!(history.undo(&mut session).unwrap(), explicit);
        }
    }
}

#[test]
fn automatic_history_bounds_interactive_log_payloads_and_retains_failures() {
    use super::history::{History, Input};
    use fujimod_emulator_cpu::ExecutionPolicy;
    let mut session = card_session();
    session
        .board
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    let budget = session
        .checkpoint(None)
        .unwrap()
        .encoded_device_bytes()
        .max(4096)
        * 4;
    let mut history = History::new(32, budget).unwrap();
    let mut captures = 0;
    for _ in 0..12 {
        // Invalid external data is still logged with its actual failure. It
        // must not evade retention limits or trigger guest execution.
        let input = Input::Adc {
            node: "adc".into(),
            samples: vec![100; budget / 4],
        };
        captures += usize::from(history.mark_automatic(&session, &input).unwrap());
        assert!(history.apply(&mut session, input).is_err());
    }
    assert!(captures > 1);
    assert!(history.entries().len() <= 2);
    assert!(
        history
            .entries()
            .iter()
            .all(|entry| entry.failure.is_some())
    );
    assert_eq!(session.board.instruction_counts(), vec![0]);
    history.undo(&mut session).unwrap();
    assert!(history.entries().is_empty());
}

#[test]
fn automatic_history_admits_one_oversized_input_and_expires_its_old_prefix() {
    use super::history::{History, Input};
    use fujimod_emulator_cpu::ExecutionPolicy;
    use fujimod_emulator_imaging::scene::{Frame, FramePixels, SensorFrames};
    use std::sync::{Arc, Mutex};
    let mut session = card_session();
    session
        .board
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    let sensor = Arc::new(Mutex::new(SensorFrames::new(
        Frame::new(1, 1, FramePixels::Rgb24(vec![1, 2, 3].into())).unwrap(),
    )));
    fujimod_emulator_exr2_images::images_mut(&mut session.resources).unwrap().sensors
        .insert("sensor".into(), sensor.clone());
    // A tiny retention budget cannot reject a supported external input. As
    // with keyframes, retain one oversized item and expire it on the next mark.
    let mut history = History::new(8, 1).unwrap();
    for luminance in [250., 500.] {
        let input = Input::SensorRgb {
            source: "sensor".into(),
            width: 1,
            height: 1,
            bytes: vec![4, 5, 6],
            white_luminance: luminance,
        };
        assert!(history.mark_automatic(&session, &input).unwrap());
        history.apply(&mut session, input).unwrap();
        assert_eq!(history.retained_marks(), 1);
        assert_eq!(history.entries().len(), 1);
        assert!(history.entries()[0].failure.is_none());
        assert_eq!(sensor.lock().unwrap().current.white_luminance, luminance);
    }
    history.undo(&mut session).unwrap();
    assert!(history.entries().is_empty());
    assert_eq!(sensor.lock().unwrap().current.white_luminance, 250.);
}

#[test]
fn raf_history_and_replay_share_the_installed_frame_and_checkpoint_counts() {
    use super::history::{History, Input};
    use fujimod_emulator_imaging::scene::{Frame, FramePixels, SensorFrames, mosaic::Radiance};
    use std::sync::{Arc, Mutex};
    fujimod_emulator_exr2_images::install_sensor_frame_codec();
    let mut session = card_session();
    let input = Arc::new(Mutex::new(SensorFrames::new(
        Frame::new(1, 1, FramePixels::Rgb24(vec![0; 3].into())).unwrap(),
    )));
    fujimod_emulator_exr2_images::images_mut(&mut session.resources).unwrap().sensors
        .insert("sensor".into(), input.clone());
    let saved = session.checkpoint(None).unwrap();
    let frame: Arc<Frame> = Arc::new(
        Frame::new(
            7,
            3,
            FramePixels::Mosaic {
                cfa: std::array::from_fn(|y| std::array::from_fn(|x| ((x + y) % 3) as u8)),
                radiance: Radiance::from_counts((0..21).map(|i| i * 1777), 1. / 64800.),
            },
        )
        .unwrap(),
    );
    let mut history = History::new(2, 128 << 20).unwrap();
    history
        .apply(
            &mut session,
            Input::SensorRaf {
                source: "sensor".into(),
                frame: fujimod_emulator_exr2_resources::SensorFrame {
                    frame: frame.clone(),
                    bytes: 0,
                },
            },
        )
        .unwrap();
    assert!(Arc::ptr_eq(&input.lock().unwrap().current, &frame));
    let Input::SensorRaf {
        frame: retained, ..
    } = &history.entries()[0].input
    else {
        panic!("RAF input");
    };
    assert!(Arc::ptr_eq(&retained.frame.clone().downcast::<Frame>().unwrap(), &frame));
    let straight = session.fingerprint().unwrap();
    let encoded =
        fujimod_emulator_devices::checkpoint::Checkpoint::encode("raf-history", &history.entries())
            .unwrap();
    let replay: Vec<super::history::Entry> = encoded.decode("raf-history").unwrap();
    session.restore(&saved).unwrap();
    History::replay(&mut session, &replay).unwrap();
    let Input::SensorRaf {
        frame: replayed, ..
    } = &replay[0].input
    else {
        panic!("RAF input");
    };
    let replayed = replayed.frame.clone().downcast::<Frame>().unwrap();
    assert!(Arc::ptr_eq(&input.lock().unwrap().current, &replayed));
    assert_eq!(session.fingerprint().unwrap(), straight);
    let current = session.checkpoint(None).unwrap();
    session.restore(&saved).unwrap();
    session.restore(&current).unwrap();
    assert!(input.lock().unwrap().current.same_pixels(&replayed));
}
