use super::*;
use crate::board::keyframe::tests::{board, board_mode};

#[test]
fn fresh_board_resumes_the_full_archive_with_pending_dma_and_fractional_slices() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        let (bytes, before, later) = {
            let mut source = board();
            source.set_execution_policy(policy).unwrap();
            source.run_until(13, 17).unwrap();
            let point = source.checkpoint(None).unwrap();
            let bytes = point.export().unwrap().to_bytes().unwrap();
            let before = source.fingerprint().unwrap();
            assert_eq!(point.export().unwrap().to_bytes().unwrap(), bytes);
            assert_eq!(source.fingerprint().unwrap(), before);
            source.run_until(120, 17).unwrap();
            (bytes, before, source.fingerprint().unwrap())
        };
        let encoded = Encoded::from_bytes(&bytes).unwrap();
        let mut target = board();
        let initial = target.fingerprint().unwrap();
        let point = target.import_keyframe(&encoded).unwrap();
        assert_eq!(target.fingerprint().unwrap(), initial);
        target.restore_keyframe(&point).unwrap();
        assert_eq!(target.fingerprint().unwrap(), before);
        assert_eq!(point.export().unwrap().to_bytes().unwrap(), bytes);
        target.checkpoint(Some(&point)).unwrap();
        target.run_until(120, 17).unwrap();
        assert_eq!(target.fingerprint().unwrap(), later);
    }
}

#[test]
fn scheduler_frames_console_and_outputs_are_detached_captured_data() {
    use fujimod_emulator_devices::graphics::scanout::Frame;
    let (bytes, expected) = {
        let mut source = board();
        source.run_until(13, 17).unwrap();
        let mut state = source.state.borrow_mut();
        state.frames.insert(
            0,
            Frame {
                output: 0,
                sequence: 7,
                width: 2,
                height: 1,
                rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
            },
        );
        state.console.push("captured serial output".into());
        state
            .outputs
            .insert(1, json!({"sequence":9,"status":"ready"}));
        drop(state);
        (
            source
                .checkpoint(None)
                .unwrap()
                .export()
                .unwrap()
                .to_bytes()
                .unwrap(),
            source.fingerprint().unwrap(),
        )
    };
    let mut target = board();
    let point = target
        .import_keyframe(&Encoded::from_bytes(&bytes).unwrap())
        .unwrap();
    target.restore_keyframe(&point).unwrap();
    assert_eq!(target.fingerprint().unwrap(), expected);
    assert_eq!(
        target.state.borrow().outputs[&1],
        json!({"sequence":9,"status":"ready"})
    );
}

#[test]
fn event_record_hex_values_keep_their_variant_in_portable_archives() {
    let (bytes, expected) = {
        let source = board();
        source
            .state
            .borrow_mut()
            .records
            .push(fujimod_emulator_devices::Record::new(
                "portable-record",
                [(
                    "address",
                    fujimod_emulator_devices::record::Value::Hex(0x1234),
                )],
            ));
        (
            source
                .checkpoint(None)
                .unwrap()
                .export()
                .unwrap()
                .to_bytes()
                .unwrap(),
            source.fingerprint().unwrap(),
        )
    };
    let mut target = board();
    let checkpoint = target
        .import_keyframe(&Encoded::from_bytes(&bytes).unwrap())
        .unwrap();
    target.restore_keyframe(&checkpoint).unwrap();
    assert_eq!(target.fingerprint().unwrap(), expected);
}

#[test]
fn invalid_topology_scheduler_or_mode_is_refused_without_mutating_the_receiver() {
    let mut source = board();
    source.run_until(13, 17).unwrap();
    let encoded = source.checkpoint(None).unwrap().export().unwrap();
    let target = board();
    let before = target.fingerprint().unwrap();
    for corrupt in 0..7 {
        let mut header: Header<Saved, decoder::Checkpoint, observe::Entries> =
            encoded.decode(KIND).unwrap();
        match corrupt {
            0 => {
                header.runtime.released.pop();
            }
            1 => header.next_core = 2,
            2 => header.quantum = Some(0),
            3 => header
                .runtime
                .timeline
                .schedule(Target::DeviceEvent(999), header.runtime.timeline.now(), 1)
                .unwrap(),
            4 => header.controllers[0] = Some(999),
            5 => header.runtime.timeline = Timeline::new(Ratio::new(4, 3).unwrap()).unwrap(),
            _ => header.observations.clear(),
        }
        let bad = Encoded::encode(KIND, &header)
            .unwrap()
            .with_blobs(encoded.blobs().to_vec())
            .with_children(encoded.children().to_vec());
        assert!(target.import_keyframe(&bad).is_err());
        assert_eq!(target.fingerprint().unwrap(), before);
    }
    assert!(
        target
            .import_keyframe(&encoded.clone().with_blobs(vec![]))
            .is_err()
    );
    assert_eq!(target.fingerprint().unwrap(), before);
    let discovery = board_mode(bus::Mode::Discovery);
    let before = discovery.fingerprint().unwrap();
    assert!(discovery.import_keyframe(&encoded).is_err());
    assert_eq!(discovery.fingerprint().unwrap(), before);
}
