use super::*;
use crate::{backend::Backend, testing::facts};
use fujimod_usb::testing::{Exchange, Script};

fn envelope(kind: u8, size: u32) -> [u8; 12] {
    let mut bytes = [0; 12];
    bytes[0] = kind;
    bytes[6..10].copy_from_slice(&size.to_be_bytes());
    bytes[11] = 1;
    bytes
}
fn opened(steps: impl IntoIterator<Item = Exchange>) -> Script {
    let mut open = [0; 12];
    open[0] = 0xc0;
    open[9] = 8;
    Script::new(
        [
            Exchange::Control(open),
            Exchange::Read {
                maximum: 8,
                data: vec![1, 0, 0, 0, 0, 0, 1, 0],
            },
        ]
        .into_iter()
        .chain(steps),
    )
}
fn command(function: u32, parameters: [u8; 16], payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 36];
    bytes[..4].copy_from_slice(&0x200001u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&function.to_le_bytes());
    bytes[20..36].copy_from_slice(&parameters);
    bytes.extend(payload);
    bytes
}
fn exchanges(request: Vec<u8>, reply: Vec<u8>) -> Vec<Exchange> {
    vec![
        Exchange::Control(envelope(0xd0, request.len() as u32)),
        Exchange::Write(request),
        Exchange::Control(envelope(0xd1, reply.len() as u32)),
        Exchange::Read {
            maximum: reply.len(),
            data: reply,
        },
    ]
}
fn options() -> Options {
    Options {
        memory_limits: DebugMemoryLimits {
            read: NonZeroU32::new(4).unwrap(),
            write: NonZeroU32::new(4).unwrap(),
        },
        event_bytes: NonZeroU32::new(64).unwrap(),
        poll_interval: Duration::from_millis(1),
        output: "lcd".into(),
    }
}
#[test]
fn range_reads_use_alias_and_chunk_limits() {
    futures_lite::future::block_on(async {
        let mut steps = vec![];
        for (address, data) in [
            (0x40001000u32, vec![1, 2, 3, 4]),
            (0x40001004u32, vec![5, 6]),
        ] {
            let mut params = [0; 16];
            params[8..12].copy_from_slice(&address.to_le_bytes());
            params[12..16].copy_from_slice(&(data.len() as u32).to_le_bytes());
            steps.extend(exchanges(
                command(2, params, &[]),
                command(2, params, &data),
            ));
        }
        let script = opened(steps);
        let session = script.begin_session(1024, None).await.unwrap();
        let mut facts = facts();
        facts.firmware.uncached_alias = 0x40000000;
        let mut remote = Ff80Remote::new(session, facts, options()).unwrap();
        let bytes = remote
            .backend
            .memory(0x1000, 6, &mut ReadSet::default())
            .await
            .unwrap();
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6]);
        assert_eq!(script.remaining(), 0);
    });
}
#[test]
fn debug_names_are_explicit_and_failed_events_are_not_replayed() {
    futures_lite::future::block_on(async {
        let payload = b"EXE_M ON\n";
        let mut parameters = [0; 16];
        parameters[12..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        let mut reply = command(5, parameters, &[]);
        reply[4..8].copy_from_slice(&1u32.to_le_bytes());
        let script = opened(exchanges(command(5, parameters, payload), reply));
        let session = script.begin_session(1024, None).await.unwrap();
        let mut facts = facts();
        rename_menu_key(&mut facts, "EXE_M");
        let mut remote = Ff80Remote::new(session, facts.clone(), options()).unwrap();
        assert!(
            remote
                .backend
                .input(
                    &Input {
                        control: control("menu_ok_button"),
                        state: InputState::Pressed
                    },
                    &facts
                )
                .await
                .is_err()
        );
        assert_eq!(script.remaining(), 0);
    });
}

#[test]
fn release_sends_off_before_any_state_or_readiness_reads() {
    futures_lite::future::block_on(async {
        let mut steps = vec![];
        for payload in ["FN OFF\n", "FULL OFF\n", "HALF OFF\n"] {
            let mut parameters = [0; 16];
            parameters[12..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            steps.extend(exchanges(
                command(5, parameters, payload.as_bytes()),
                command(5, parameters, &[]),
            ));
        }
        // An asserted contact must be released even when state reads would
        // be slow or unavailable. The wire script permits only OFF events.
        let script = opened(steps);
        let session = script.begin_session(1024, None).await.unwrap();
        let mut remote = Ff80Remote::new(session, facts(), options()).unwrap();
        for id in ["wifi_button", "shutter_button"] {
            remote
                .set_input(
                    &Input {
                        control: control(id),
                        state: InputState::Released,
                    },
                    u64::MAX,
                )
                .await
                .unwrap();
        }
        assert_eq!(script.remaining(), 0);
    });
}

#[test]
fn press_has_no_observation_round_trips_between_on_and_off() {
    futures_lite::future::block_on(async {
        let mut steps = vec![];
        // Synthetic REC readiness, then the state read for control support,
        // before asserting the contact. Eight-byte flags use two bounded reads.
        for (address, value) in [
            (780u32, 0u32),
            (800, 0),
            (804, 0),
            (808, 0),
            (812, 0),
            (816, 0),
            (820, 0),
            (824, 1),
            (768, 3),
            (776, 0),
            (784, 0),
            (520, 0),
        ] {
            let mut parameters = [0; 16];
            parameters[8..12].copy_from_slice(&address.to_le_bytes());
            parameters[12..].copy_from_slice(&4u32.to_le_bytes());
            steps.extend(exchanges(
                command(2, parameters, &[]),
                command(2, parameters, &value.to_le_bytes()),
            ));
        }
        for payload in ["FN ON\n", "FN OFF\n"] {
            let mut parameters = [0; 16];
            parameters[12..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            steps.extend(exchanges(
                command(5, parameters, payload.as_bytes()),
                command(5, parameters, &[]),
            ));
        }
        let script = opened(steps);
        let session = script.begin_session(1024, None).await.unwrap();
        let mut remote = Ff80Remote::new(session, facts(), options()).unwrap();
        remote
            .press(&control("wifi_button"), u64::MAX)
            .await
            .unwrap();
        assert_eq!(script.remaining(), 0);
    });
}

#[test]
fn scanner_names_never_supply_missing_debug_bindings() {
    futures_lite::future::block_on(async {
        let script = opened([]);
        let session = script.begin_session(1024, None).await.unwrap();
        let mut facts = facts();
        facts
            .control_ids
            .inputs
            .iter_mut()
            .find(|input| input.debug_key.as_deref() == Some("MENU"))
            .unwrap()
            .debug_key = None;
        let mut remote = Ff80Remote::new(session, facts.clone(), options()).unwrap();
        let error = remote
            .backend
            .input(
                &Input {
                    control: control("menu_ok_button"),
                    state: InputState::Pressed,
                },
                &facts,
            )
            .await
            .unwrap_err();
        assert!(error.contains("does not declare debug key"), "{error}");
        assert_eq!(script.remaining(), 0);
    });
}

fn rename_menu_key(facts: &mut Facts, name: &str) {
    use fujimod_catalog_controls::{ControlBehavior, InputBinding};
    for key in &mut facts.control_ids.debug_keys {
        if key.name == "MENU" {
            key.name = name.into();
        }
    }
    for input in &mut facts.control_ids.inputs {
        if input.debug_key.as_deref() == Some("MENU") {
            input.debug_key = Some(name.into());
        }
    }
    facts
        .physical_controls
        .controls
        .get_mut("menu_ok_button")
        .unwrap()
        .behavior = Some(ControlBehavior::Button {
        press: Some(InputBinding::DebugKey { name: name.into() }),
    });
}

#[test]
fn shutter_stages_release_all_contacts_and_positions_have_no_release_event() {
    futures_lite::future::block_on(async {
        let mut steps = vec![];
        for payload in [
            "HALF ON\n",
            "HALF ON\n",
            "FULL ON\n",
            "FULL OFF\n",
            "HALF OFF\n",
            "AUTO ON\n",
        ] {
            let mut parameters = [0; 16];
            parameters[12..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            steps.extend(exchanges(
                command(5, parameters, payload.as_bytes()),
                command(5, parameters, &[]),
            ));
        }
        let script = opened(steps);
        let session = script.begin_session(1024, None).await.unwrap();
        let facts = facts();
        let mut remote = Ff80Remote::new(session, facts.clone(), options()).unwrap();
        for state in [
            InputState::Stage("half_press".into()),
            InputState::Stage("full_press".into()),
            InputState::Released,
        ] {
            remote
                .backend
                .input(
                    &Input {
                        control: control("shutter_button"),
                        state,
                    },
                    &facts,
                )
                .await
                .unwrap();
        }
        remote
            .backend
            .input(
                &Input {
                    control: control("position_dial"),
                    state: InputState::Position("A".into()),
                },
                &facts,
            )
            .await
            .unwrap();
        assert_eq!(script.remaining(), 0);
    });
}
#[test]
fn touch_is_refused_before_any_exchange() {
    futures_lite::future::block_on(async {
        let script = opened([]);
        let session = script.begin_session(1024, None).await.unwrap();
        let facts = crate::testing::touch_facts();
        let mut remote = Ff80Remote::new(session, facts.clone(), options()).unwrap();
        for state in [InputState::TouchStart { x: 1, y: 2 }, InputState::TouchEnd] {
            let error = remote
                .backend
                .input(
                    &Input {
                        control: control("touchscreen"),
                        state,
                    },
                    &facts,
                )
                .await
                .unwrap_err();
            assert!(error.contains("cannot inject touch"));
        }
        assert_eq!(script.remaining(), 0);
    });
}
