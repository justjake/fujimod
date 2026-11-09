use super::*;
use crate::power_cycle::{ColdBoot, PowerCycle};
use fujimod_emulator_cpu::arm;
use fujimod_emulator_devices::exr2::{clock, wake_alarm::WakeAlarm};
use std::sync::{Arc, Mutex};

#[test]
fn physical_ram_fill_is_applied_before_execution() {
    let graph: factory::Graph = serde_json::from_value(serde_json::json!({
        "processors": [{"model":"cortex-r5f","registers":[],"released":true,"mprofile":null,"rprofile":null}],
        "ram": [{"base":0,"size":4096}], "dma_mappings": [], "decoders": [],
        "mappings": [{"core":0,"address":0,"physical":0,"size":4096,"permissions":7}],
        "peripherals": [], "instructions_per_tick": [1,1]
    })).unwrap();
    let board = graph.build(factory::Options::new(factory::Engine::Unicorn)
        .with_translation_cache_size_bytes(1 << 20).with_ram_fill(0xff),
        Bus::new(Mode::Strict), None).unwrap();
    let mut bytes = [0; 16];
    board.read_physical(0x300, &mut bytes).unwrap();
    assert_eq!(bytes, [0xff; 16]);
}

#[test]
fn selected_ram_survives_a_modeled_restart() {
    struct Builder;
    impl ColdBoot for Builder {
        fn build(&mut self, tick: u64, counts: Vec<u64>, packet: [u8; 16]) -> Result<Session, String> {
            let power = Arc::new(Mutex::new(WakeAlarm::new(1_000_000, clock::POWER_HOLD_RELEASE_LIMIT_TICKS)?));
            power.lock().unwrap().initialize_packet(packet)?;
            Ok(card_session_with_power(Some((tick, counts)), Some(power)))
        }
    }
    let power = Arc::new(Mutex::new(WakeAlarm::new(1_000_000, clock::POWER_HOLD_RELEASE_LIMIT_TICKS).unwrap()));
    {
        let mut p = power.lock().unwrap();
        p.initialize_packet([0x5a; 16]).unwrap();
        p.write(0, 0x69, 1).unwrap();
        p.write(0, 0x6a, 0).unwrap();
        p.write(0, 0x68, 1).unwrap();
    }
    let mut session = card_session_with_power(None, Some(power));
    session.configure_power_cycle(PowerCycle::new("clocks".into(),
        crate::durable::Spec { cards: vec!["sd".into()], nands: vec![] }, Box::new(Builder))
        .with_retained_ram(vec![(0, 4096)])).unwrap();
    let witness = [0xff, 0xff, 0, 0x35, 0xaa, 0xff];
    session.board.write_physical(0x200, &witness).unwrap();
    // Release power hold, then spin until the scheduled wake.
    session.board.write_physical(0, &0xe5801000u32.to_le_bytes()).unwrap();
    session.board.write_physical(4, &0xeafffffeu32.to_le_bytes()).unwrap();
    session.board.write_register(0, arm::CORE[0], 0xfff88084).unwrap();
    session.board.write_register(0, arm::CORE[1], 0).unwrap();
    session.run_with_limits(10_000_000, 128, None, Some(200_000)).unwrap();
    assert!(session.board.is_powered_off());
    session.run_with_limits(10_000_000, 128, None, Some(1_000_000)).unwrap();
    assert!(session.board.epoch_is_unexecuted());
    let mut actual = [0; 6];
    session.board.read_physical(0x200, &mut actual).unwrap();
    assert_eq!(actual, witness);
}
