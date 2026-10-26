use super::*;

fn board() -> Board {
    let graph: factory::Graph = serde_json::from_value(json!({
        "processors":[{"model":"cortex-r5f","released":true,"registers":[["Pc",65536]],
            "mprofile":null,"rprofile":{"cp14_identity":16387,"tcm":{"atcm":{"bytes":0,"reset":0},"btcm":{"bytes":0,"reset":0}}}}],
        "ram":[{"base":65536,"size":4096}],
        "mappings":[{"core":0,"address":65536,"physical":65536,"size":4096,"permissions":7}],
        "dma_mappings":[{"address":65536,"physical":65536,"size":4096,"permissions":3}],
        "peripherals":[],"decoders":[],"instructions_per_tick":[1,1]
    })).unwrap();
    let mut board = graph
        .build(
            factory::Options::new(factory::Engine::Unicorn)
                .with_translation_cache_size_bytes(1 << 20),
            Bus::new(bus::Mode::Strict),
            None,
        )
        .unwrap();
    board
        .write_memory(0, 0x10000, &0xeafffffeu32.to_le_bytes())
        .unwrap();
    board
}

#[test]
fn worker_preference_is_host_only_and_exact_stays_inline() {
    let mut board = board();
    assert_eq!(board.device_worker_preference(), 0);
    assert_eq!(board.device_worker_count(), 0);
    let before = board.fingerprint().unwrap();
    board.set_device_workers(2).unwrap();
    assert_eq!(board.device_worker_preference(), 2);
    assert_eq!(board.device_worker_count(), 0);
    assert_eq!(fujimod_emulator_devices::work::worker_count(), 0);
    assert_eq!(board.fingerprint().unwrap(), before);
    assert!(board.set_device_workers(1).is_err());
    board
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    assert_eq!(board.device_worker_count(), 2);
    assert_eq!(fujimod_emulator_devices::work::worker_count(), 2);
    let before = board.fingerprint().unwrap();
    let saved = board.checkpoint(None).unwrap();
    board.set_device_workers(0).unwrap();
    board.restore_keyframe(&saved).unwrap();
    assert_eq!(board.device_worker_preference(), 0);
    assert_eq!(board.fingerprint().unwrap(), before);
}

#[test]
fn alternating_boards_apply_their_preference_before_each_run() {
    let mut inline = board();
    let mut threaded = board();
    assert_eq!(inline.device_worker_preference(), 0);
    threaded.set_device_workers(2).unwrap();
    inline
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    assert_eq!(inline.device_worker_count(), 0);
    threaded
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    for tick in [5, 10] {
        inline.run_until(tick, 8).unwrap();
        assert_eq!(fujimod_emulator_devices::work::worker_count(), 0);
        threaded.run_until(tick, 8).unwrap();
        assert_eq!(fujimod_emulator_devices::work::worker_count(), 2);
        assert_eq!(
            inline.fingerprint().unwrap(),
            threaded.fingerprint().unwrap()
        );
    }
    fujimod_emulator_devices::work::configure(0).unwrap();
}
