use super::{device::*, options::Options};
use fujimod_emulator_devices::{Cx, Device, Host, Record, exr2::irq};
use serde_json::json;
#[derive(Default)]
struct Services {
    now: u64,
    irqs: Vec<(usize, u32)>,
    records: Vec<Record>,
}
impl Host for Services {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {
        panic!("uses exact native deadlines")
    }
    fn set_irq(&mut self, _: u32, _: bool) {
        panic!("per-core IRQ only")
    }
    fn raise_irq(&mut self, core: usize, line: u32) -> Result<(), String> {
        self.irqs.push((core, line));
        Ok(())
    }
    fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
        panic!("I2C has no DMA")
    }
    fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
        panic!("I2C has no DMA")
    }
    fn record(&mut self, record: Record) {
        self.records.push(record);
    }
}
fn write(
    device: &mut dyn Device,
    host: &mut Services,
    address: u64,
    value: u64,
) -> Result<(), String> {
    let mut cx = Cx::new(host, 0, 0xdeadbeef, 0);
    device.write(&mut cx, address, 4, value);
    cx.take_refusal().map_or(Ok(()), Err)
}
fn advance(device: &mut dyn Device, host: &mut Services) -> Result<(), String> {
    let mut cx = Cx::new(host, 0, 0x1234, 0);
    device.advance(&mut cx);
    cx.take_refusal().map_or(Ok(()), Err)
}
fn controllers(
    host: &mut Services,
    line: u32,
) -> ([irq::InterruptController; 2], Vec<(usize, irq::Inputs)>) {
    let inputs = [irq::Inputs::default(), irq::Inputs::default()];
    let mut units = std::array::from_fn(|core| {
        irq::InterruptController::new(
            irq::Options {
                arbitration: irq::Arbitration::Unique,
            },
            irq::Wiring {
                base: 0xfffb3000 + core as u64 * 0x1000,
                inputs: inputs[core].clone(),
                enables: None,
                software: vec![],
            },
        )
        .unwrap()
    });
    let base = 0xfffb4000;
    for (offset, value) in [
        (0x80 + line / 32 * 4, 1u32 << (line % 32)),
        (0x400 + line * 4, 0x1000),
        (0x800 + line * 4, 13),
    ] {
        write(&mut units[1], host, base + u64::from(offset), value.into()).unwrap();
    }
    (units, inputs.into_iter().enumerate().collect())
}
fn primary_options() -> Options {
    serde_json::from_value(
        json!({"base":0xfff85000u32,"irq":101,"modes":[1,9],"transfer_ticks":4,
        "profile":"declared-register-status-pair-latched-irq-unmeasured",
        "parts":{"kind":"primary","sensor_address":null,"prefix_address":null,"sensor_fields":[],
        "prefix_high":28,"heartbeat_register":7174,"topology":[68],"stationary":false,
        "status":[{"address":68,"writable_registers":[1,2,3,4,6],"status":[0,1],
        "transitions":[{"register":2,"data":64,"frame":[0,0]}]}],"commands":[]}}),
    )
    .unwrap()
}
fn finish(bus: &mut I2cController, host: &mut Services) {
    let before = bus.save();
    let due = bus.next_deadline(host.now).unwrap();
    assert_eq!(bus.save(), before);
    host.now = due - 1;
    advance(bus, host).unwrap();
    assert_eq!(bus.save(), before);
    host.now = due;
    advance(bus, host).unwrap();
    assert_eq!(bus.next_deadline(host.now), None);
}
#[test]
fn full_word_configuration_bytes_commit_only_at_stop_and_route_to_the_other_core() {
    let mut host = Services::default();
    let (_units, controllers) = controllers(&mut host, 101);
    let mut bus = I2cController::new(
        primary_options(),
        Wiring {
            controllers,
            cable: None,
            bridge: None,
        },
    )
    .unwrap();
    write(&mut bus, &mut host, 0xfff85010, 9).unwrap();
    write(&mut bus, &mut host, 0xfff85000, 1).unwrap();
    write(&mut bus, &mut host, 0xfff85008, 0x19).unwrap();
    finish(&mut bus, &mut host);
    write(&mut bus, &mut host, 0xfff85008, 0xa).unwrap();
    for byte in [0x88, 2, 0x4d] {
        write(&mut bus, &mut host, 0xfff85004, byte).unwrap();
        finish(&mut bus, &mut host);
    }
    assert_eq!(
        bus.save()["engine"]["parts"]["status"][0]["registers"],
        json!([[1, 0], [2, 0], [3, 0], [4, 0], [6, 0]])
    );
    write(&mut bus, &mut host, 0xfff85008, 0xb).unwrap();
    assert_eq!(
        bus.save()["engine"]["parts"]["status"][0]["frame"],
        json!([0, 1])
    );
    finish(&mut bus, &mut host);
    let state = bus.save();
    assert_eq!(
        state["engine"]["parts"]["status"][0]["registers"][1],
        json!([2, 0x4d])
    );
    assert_eq!(
        state["engine"]["parts"]["status"][0]["frame"],
        json!([0, 1])
    );
    assert_eq!(host.irqs, vec![(1, 101); 5]);
    assert!(host.records.iter().any(|r| r.name == "Wire transaction"));
}
#[test]
fn latched_binding_survives_masking_but_refuses_a_changed_vector() {
    for changed in [false, true] {
        let mut host = Services::default();
        let (mut units, controllers) = controllers(&mut host, 101);
        let mut bus = I2cController::new(
            primary_options(),
            Wiring {
                controllers,
                cable: None,
                bridge: None,
            },
        )
        .unwrap();
        write(&mut bus, &mut host, 0xfff85010, 9).unwrap();
        write(&mut bus, &mut host, 0xfff85000, 1).unwrap();
        write(&mut bus, &mut host, 0xfff85008, 0x19).unwrap();
        write(&mut units[1], &mut host, 0xfffb40ac, 1 << 5).unwrap();
        if changed {
            write(&mut units[1], &mut host, 0xfffb4594, 0x2000).unwrap();
        }
        host.now = 4;
        let result = advance(&mut bus, &mut host);
        if changed {
            assert!(result.unwrap_err().contains("binding changed"));
            assert!(host.irqs.is_empty());
        } else {
            result.unwrap();
            assert_eq!(host.irqs, [(1, 101)]);
        }
    }
}
#[test]
fn secondary_samples_the_shared_cable_on_completion_and_early_ticks_are_pure() {
    let mut host = Services::default();
    let (_units, controllers) = controllers(&mut host, 102);
    let options = serde_json::from_value(
        json!({"base":0xfff85040u32,"irq":102,"modes":[9],"transfer_ticks":4,
        "profile":"declared-bus1-register-replies-unmeasured","parts":{"kind":"secondary",
        "register_address":10,"pwic_address":null,"register_modes":[9],"pwic_modes":[1],
        "register_values":[[2,0]],"register_masks":[],"pwic_values":[],"pwic_masks":[],
        "pwic_read_prefixes":[],"topology":[10],"connected":false}}),
    )
    .unwrap();
    let cable = fujimod_emulator_devices::signals::Level::new(false);
    let mut bus = I2cController::new(
        options,
        Wiring {
            controllers,
            cable: Some(cable.clone()),
            bridge: None,
        },
    )
    .unwrap();
    write(&mut bus, &mut host, 0xfff85050, 9).unwrap();
    write(&mut bus, &mut host, 0xfff85040, 1).unwrap();
    write(&mut bus, &mut host, 0xfff85048, 0x19).unwrap();
    let before = bus.save();
    host.now = 3;
    advance(&mut bus, &mut host).unwrap();
    assert_eq!(bus.save(), before);
    cable.drive(true);
    let observed = bus.save();
    assert_eq!(observed["engine"], before["engine"]);
    assert_eq!(observed["due"], before["due"]);
    assert_eq!(observed["cable"], true);
    assert_eq!(bus.save(), observed);
    host.now = 4;
    assert!(
        advance(&mut bus, &mut host)
            .unwrap_err()
            .contains("cable input changed")
    );
    assert!(host.irqs.is_empty());
}
#[test]
fn unknown_framing_and_duplicate_connections_refuse_without_scheduling_work() {
    let mut host = Services::default();
    let (_units, controllers) = controllers(&mut host, 101);
    let bus = I2cController::new(
        primary_options(),
        Wiring {
            controllers: controllers.clone(),
            cable: None,
            bridge: None,
        },
    )
    .unwrap();
    for (address, size, value) in [(0xfff85004, 2, 0x4d), (0xfff85010, 4, 3)] {
        let before = bus.save();
        assert!(bus.access_contract(address, size, Some(value)).is_some());
        assert_eq!(bus.save(), before);
    }
    assert!(
        I2cController::new(
            primary_options(),
            Wiring {
                controllers: vec![controllers[0].clone(), controllers[0].clone()],
                cable: None,
                bridge: None
            }
        )
        .is_err()
    );
}

#[test]
fn constructor_rejects_unproven_controller_modes_and_duplicate_words() {
    for modes in [vec![], vec![17], vec![9, 9]] {
        let mut options = primary_options();
        options.modes = modes;
        assert!(options.build().is_err());
    }
}

#[test]
fn binary_profiles_preserve_variants_instead_of_reparsing_constructor_labels() {
    use super::Profile;
    for original in [
        Profile::PrefixWrite,
        Profile::SensorWrite,
        Profile::Stationary,
        Profile::Heartbeat,
        Profile::LatchedHeartbeat,
        Profile::RegisterStatus,
        Profile::UsbDisconnected,
        Profile::Pwic61,
        Profile::PwicStartup,
        Profile::DeclaredSecondary,
        Profile::Other("explicit unknown".into()),
    ] {
        let saved = fujimod_emulator_devices::checkpoint::Checkpoint::encode("i2c-profile", &original).unwrap();
        assert_eq!(saved.decode::<Profile>("i2c-profile").unwrap(), original);
    }
}

#[test]
fn fingerprint_observes_cable_admission_and_bridge_without_settling_work() {
    let mut host = Services::default();
    let (_, controllers) = controllers(&mut host, 102);
    let options = |connected| {
        serde_json::from_value(json!({
        "base":0xfff85040u32,"irq":102,"modes":[9],"transfer_ticks":4,
        "profile":"declared-bus1-register-replies-unmeasured","parts":{"kind":"secondary",
        "register_address":10,"pwic_address":null,"register_modes":[9],"pwic_modes":[1],
        "register_values":[[2,0]],"register_masks":[],"pwic_values":[],"pwic_masks":[],
        "pwic_read_prefixes":[],"topology":[10],"connected":connected}}))
        .unwrap()
    };
    let cable = fujimod_emulator_devices::signals::Level::new(false);
    let bridge = fujimod_emulator_devices::parts::lens_bridge::Registers::default();
    let wiring = || Wiring {
        controllers: controllers.clone(),
        cable: Some(cable.clone()),
        bridge: Some(bridge.clone()),
    };
    let mut fixed = I2cController::new(options(json!(false)), wiring()).unwrap();
    let dynamic = I2cController::new(options(json!(null)), wiring()).unwrap();
    assert_eq!(fixed.save(), dynamic.save());
    assert_ne!(fixed.fingerprint().unwrap(), dynamic.fingerprint().unwrap());
    write(&mut fixed, &mut host, 0xfff85050, 9).unwrap();
    write(&mut fixed, &mut host, 0xfff85040, 1).unwrap();
    write(&mut fixed, &mut host, 0xfff85048, 0x19).unwrap();
    host.now = 4;
    let before = fixed.save();
    let fingerprint = fixed.fingerprint().unwrap();
    assert_eq!(fixed.fingerprint().unwrap(), fingerprint);
    assert_eq!(fixed.save(), before);
    assert_eq!(fixed.next_deadline(host.now), Some(4));
    bridge.write_register(2, 0x41).unwrap();
    assert_ne!(fixed.fingerprint().unwrap(), fingerprint);
    assert_eq!(fixed.save(), before);
    assert!(host.irqs.is_empty());
}

#[test]
fn touch_reports_use_wire_reads_irq_masks_and_exact_deadlines_and_restore_in_flight() {
    use fujimod_emulator_devices::parts::touch_input::{ContactInput, Phase};
    let mut host = Services::default();
    let (mut units, controllers) = controllers(&mut host, 101);
    let options=serde_json::from_value(json!({"base":0xfff85000u32,"irq":101,"modes":[1,9],"transfer_ticks":4,
        "profile":"endpoint7-stationary-and9-heartbeat-latched-irq-unmeasured", "parts":{"kind":"primary","sensor_address":null,
        "prefix_address":85,"sensor_fields":[],"prefix_high":28,"heartbeat_register":7174,"topology":[85],
        "stationary":false,"status":[],"commands":[],
        "touch":{"register":0,"core":0,"irq":15,"ticks_per_second":12000000}}})).unwrap();
    let mut bus = I2cController::new(
        options,
        Wiring {
            controllers,
            cable: None,
            bridge: None,
        },
    )
    .unwrap();
    bus.touch(
        0,
        &[ContactInput {
            tick: 100,
            contact: 0,
            phase: Phase::Down,
            x: 300,
            y: 200,
        }],
    )
    .unwrap();
    let before = bus.fingerprint().unwrap();
    assert_eq!(bus.next_deadline(0), Some(100));
    host.now = 99;
    advance(&mut bus, &mut host).unwrap();
    assert_eq!(bus.fingerprint().unwrap(), before);
    host.now = 100;
    advance(&mut bus, &mut host).unwrap();
    assert!(host.irqs.is_empty());
    let masked = bus.fingerprint().unwrap();
    assert_eq!(bus.fingerprint().unwrap(), masked);
    write(&mut units[0], &mut host, 0xfffb3080, 1 << 15).unwrap();
    assert_eq!(bus.next_deadline(100), Some(100));
    advance(&mut bus, &mut host).unwrap();
    assert_eq!(host.irqs, [(0, 15)]);
    assert_eq!(
        bus.save()["engine"]["parts"]["prefixed_registers"][0][1],
        json!([1, 2, 1, 44, 200, 1, 128, 0, 0, 0, 0])
    );
    fn shift(bus: &mut I2cController, host: &mut Services, address: u64, value: u64) {
        write(bus, host, address, value).unwrap();
        if let Some(at) = bus.save()["due"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_u64().unwrap())
            .min()
        {
            host.now = at;
            advance(bus, host).unwrap();
        }
    }
    write(&mut bus, &mut host, 0xfff85010, 9).unwrap();
    write(&mut bus, &mut host, 0xfff85000, 1).unwrap();
    shift(&mut bus, &mut host, 0xfff85008, 0x19);
    write(&mut bus, &mut host, 0xfff85008, 0xa).unwrap();
    for byte in [0xaa, 0, 0] {
        shift(&mut bus, &mut host, 0xfff85004, byte);
    }
    shift(&mut bus, &mut host, 0xfff85008, 0xb);
    shift(&mut bus, &mut host, 0xfff85008, 0x19);
    write(&mut bus, &mut host, 0xfff85008, 0xa).unwrap();
    shift(&mut bus, &mut host, 0xfff85004, 0xab);
    write(&mut bus, &mut host, 0xfff85008, 0xe).unwrap();
    let saved = bus.checkpoint().unwrap();
    let at = host.now;
    let mut completed = None;
    for _ in 0..2 {
        bus.restore_checkpoint(&saved).unwrap();
        host.now = at;
        let mut frame = Vec::new();
        for index in 0..11 {
            if index == 10 {
                write(&mut bus, &mut host, 0xfff85008, 0xa).unwrap();
            }
            shift(
                &mut bus,
                &mut host,
                0xfff85008,
                if index == 10 { 0x2a } else { 0x2e },
            );
            let mut cx = Cx::new(&mut host, 0, 0xfeed, 0);
            frame.push(bus.read(&mut cx, 0xfff85004, 4) as u8);
            assert!(cx.take_refusal().is_none());
        }
        assert_eq!(frame, [1, 2, 1, 44, 200, 1, 128, 0, 0, 0, 0]);
        assert_eq!(
            bus.save()["engine"]["parts"]["prefixed_registers"][0][2]
                .as_u64()
                .unwrap(),
            0
        );
        shift(&mut bus, &mut host, 0xfff85008, 0xb);
        assert_eq!(
            bus.save()["engine"]["parts"]["prefixed_registers"][0][2]
                .as_u64()
                .unwrap(),
            1
        );
        let state = bus.fingerprint().unwrap();
        if let Some(previous) = &completed {
            assert_eq!(&state, previous);
        } else {
            completed = Some(state);
        }
    }
}
