use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc};

const ENTRY: u64 = 0x1000;
const REQUEST: u32 = 0x1000_0000;

fn session(request_ticks: Option<u64>) -> Session {
    let mut input = json!({
        "ram_size": 65536, "cores": 1, "discovery": false,
        "cbar": 0x2000_0000u32,
        "initial_registers": [[["Pc", ENTRY]]],
    });
    if let Some(ticks) = request_ticks {
        input["control_requests"] = json!([[REQUEST, ticks]]);
    }
    let definition: Definition = serde_json::from_value(input).unwrap();
    Session::new(definition, None, vec![]).unwrap()
}

fn program(session: &Session, words: &[u32]) {
    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    session.write(ENTRY as usize, &bytes).unwrap();
}

fn state(session: &Session) -> Value {
    serde_json::to_value(session.state(true)).unwrap()
}

fn dacr() -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15, wide: false, secure: 0, crn: 3, crm: 0, opc1: 0, opc2: 0,
    }
}

#[test]
fn guest_fault_is_sticky_and_does_not_complete_a_due_device_request() {
    let session = session(Some(1));
    // STR r1,[r0]; LDR r3,[r2]. The second access has no physical backing.
    program(&session, &[0xe580_1000, 0xe592_3000]);
    session.write_register(0, arm::CORE[0], REQUEST.into()).unwrap();
    session.write_register(0, arm::CORE[1], 2).unwrap();
    session.write_register(0, arm::CORE[2], 0x20000).unwrap();

    let error = session.run(0, 8).unwrap_err();
    assert!(error.contains("Unmapped/protected ARMv7-A access"), "{error}");
    assert_eq!(session.read_register(0, arm::PC).unwrap(), ENTRY + 4);
    assert_eq!(session.state(false).failure.as_deref(), Some(error.as_str()));
    let before = state(&session);
    let devices = session.device_state_json();
    let requests: Value = serde_json::from_str(&devices["xp4-control-requests"]).unwrap();
    let word = &requests["words"][REQUEST.to_string()];
    assert_eq!(word["value"], 2);
    assert_eq!(word["next"][0], session.now());
    assert_eq!(word["next"][1], 1);
    assert!(session.device_records().is_empty());
    let mut ram = vec![0; 65536];
    session.read(0, &mut ram).unwrap();
    let registers: Vec<_> = arm::CORE.iter()
        .map(|&register| session.read_register(0, register).unwrap()).collect();
    let control = session.coprocessor(0, dacr(), None).unwrap();

    assert_eq!(session.run(0, 1).unwrap_err(), error);
    assert_eq!(session.run_board(8, 1).unwrap_err(), error);
    assert_eq!(session.run_board(0, 1).unwrap_err(), error);
    assert!(!session.idle());
    assert_eq!(session.write(0x4000, &[1]).unwrap_err(), error);
    assert_eq!(session.write_register(0, arm::CORE[0], 0).unwrap_err(), error);
    assert_eq!(session.coprocessor(0, dacr(), Some(0)).unwrap_err(), error);
    assert_eq!(session.instruction_points(0, vec![]).unwrap_err(), error);
    assert_eq!(session.memory_watch_begin(vec![(0, 65536)], 8).unwrap_err(), error);
    assert_eq!(session.set_gpio_input(0, 0, true).unwrap_err(), error);
    assert_eq!(session.set_usb_connected(true).unwrap_err(), error);
    assert_eq!(state(&session), before);
    assert_eq!(session.device_state_json(), devices);
    assert!(session.device_records().is_empty());
    let mut after = vec![0; 65536];
    session.read(0, &mut after).unwrap();
    assert_eq!(after, ram);
    for (&register, expected) in arm::CORE.iter().zip(registers) {
        assert_eq!(session.read_register(0, register).unwrap(), expected);
    }
    assert_eq!(session.coprocessor(0, dacr(), None).unwrap(), control);
}

#[test]
fn invalid_run_arguments_leave_the_board_available_and_unchanged() {
    let session = session(Some(8));
    program(&session, &[0xe1a0_0000]); // MOV r0,r0.
    let before = state(&session);
    let devices = session.device_state_json();
    assert!(session.run(1, 1).unwrap_err().contains("Invalid ARMv7-A core"));
    assert!(session.run_board(1, 0).unwrap_err().contains("quantum"));
    assert_eq!(state(&session), before);
    assert_eq!(session.device_state_json(), devices);
    assert_eq!(session.read_register(0, arm::PC).unwrap(), ENTRY);
    session.run(0, 1).unwrap();
    assert_eq!(session.read_register(0, arm::PC).unwrap(), ENTRY + 4);
    assert!(session.state(false).failure.is_none());
}

#[test]
fn observer_register_updates_remain_supported_but_host_work_cannot_reenter() {
    let session = Rc::new(session(None));
    // STR r0,[r1]; WFI. The observer supplies r0 before the store executes.
    program(&session, &[0xe581_0000, 0xe320_f003]);
    session.write_register(0, arm::CORE[1], 0x4000).unwrap();
    session.instruction_points(0, vec![ENTRY]).unwrap();
    let observed = Rc::new(Cell::new(0));
    let seen = observed.clone();
    let weak = Rc::downgrade(&session);
    session.set_instruction_observer(Some(Rc::new(move |core, _| {
        let session = weak.upgrade().ok_or("observer lost its owner")?;
        let refused = [
            session.run(core, 1).is_err(),
            session.run_board(1, 1).is_err(),
            !session.idle(),
            session.write(0x4000, &[99]).is_err(),
            session.instruction_points(core, vec![]).is_err(),
            session.memory_watch_begin(vec![(0, 65536)], 8).is_err(),
            session.function_watch_begin(vec![], 1, 8, 8, None, false).is_err(),
            session.set_gpio_input(0, 0, true).is_err(),
        ];
        if refused.iter().any(|refused| !refused) {
            return Err("a host operation reentered the live board".into());
        }
        session.write_register(core, arm::CORE[0], 42)?;
        session.coprocessor(core, dacr(), Some(0x5555_5555))?;
        seen.set(seen.get() + 1);
        Ok(())
    }))).unwrap();
    session.run(0, 8).unwrap();
    assert_eq!(observed.get(), 1);
    let mut stored = [0; 4];
    session.read(0x4000, &mut stored).unwrap();
    assert_eq!(u32::from_le_bytes(stored), 42);
    assert_eq!(session.coprocessor(0, dacr(), None).unwrap(), 0x5555_5555);
    assert!(session.state(false).failure.is_none());
    assert_eq!(session.state(false).waiting, [true]);
}

#[test]
fn disabled_vfp_still_enters_the_guest_undefined_vector() {
    let session = session(None);
    program(&session, &[0xed2d_8b02]); // VPUSH {d8}; FP access remains disabled.
    session.write_register(0, arm::CORE[13], 0x8000).unwrap();
    session.run(0, 1).unwrap();
    assert!(session.state(false).failure.is_none());
    assert_eq!(session.state(false).undefined_entries, [1]);
    assert_eq!(session.read_register(0, arm::PC).unwrap(), 4);
    assert_eq!(session.read_register(0, arm::CPSR).unwrap() & 31, 0x1b);
    assert_eq!(session.read_register(0, arm::CORE[14]).unwrap(), ENTRY + 4);
    assert_eq!(session.read_register(0, arm::SPSR).unwrap(), 0xd3);
}

#[test]
fn scheduled_device_refusal_stops_before_the_next_guest_instruction() {
    let input = json!({
        "ram_size": 65536, "cores": 1, "discovery": false,
        "cbar": 0x2000_0000u32,
        "initial_registers": [[["Pc", ENTRY]]],
        "storage_jedec": 0x123456,
        "storage_dma_routes": (32u32..48).collect::<Vec<_>>(),
        "storage_geometry": [65536, 256, 100, 10],
        "storage_irq": 48,
    });
    let session = Session::new(serde_json::from_value(input).unwrap(), None, vec![]).unwrap();
    // STR r1,[r0], followed by ADD r2,r2,#1. The requested four-byte
    // identity response is rejected when the controller's deadline expires.
    let mut words = vec![0xe580_1000];
    words.extend([0xe282_2001; 16]);
    program(&session, &words);
    session.write_register(0, arm::CORE[0], 0xfffd_8090).unwrap();
    session.write_register(0, arm::CORE[1], 0x9f30_0001).unwrap();
    session.write_register(0, arm::CORE[2], 0).unwrap();
    session.run(0, 1).unwrap();
    let devices = session.device_state_json();
    let flash: Value = serde_json::from_str(&devices["xp4-serial-flash"]).unwrap();
    assert_eq!(flash["next"], json!([9, 1]));

    let error = session.run(0, 32).unwrap_err();
    assert_eq!(error, "XP4 JEDEC read exceeds three identity bytes");
    assert_eq!(session.state(false).failure.as_deref(), Some(error.as_str()));
    assert_eq!(session.now(), 9);
    assert_eq!(session.instructions(), [9]);
    assert_eq!(session.read_register(0, arm::CORE[2]).unwrap(), 8);
    assert_eq!(session.read_register(0, arm::PC).unwrap(), ENTRY + 36);
    let before = state(&session);
    let devices = session.device_state_json();
    let records = session.device_records();
    assert_eq!(session.run_board(32, 8).unwrap_err(), error);
    assert!(!session.idle());
    assert_eq!(state(&session), before);
    assert_eq!(session.device_state_json(), devices);
    assert_eq!(session.device_records(), records);
}

#[test]
fn invalid_external_arguments_do_not_poison_a_healthy_board() {
    let input = json!({
        "ram_size": 65536, "cores": 1, "discovery": false,
        "cbar": 0x2000_0000u32,
        "initial_registers": [[["Pc", ENTRY]]],
        "adc": (vec![0u16; 12], 1),
    });
    let session = Session::new(serde_json::from_value(input).unwrap(), None, vec![]).unwrap();
    program(&session, &[0xe1a0_0000]);
    let before = state(&session);
    let devices = session.device_state_json();
    for (channel, value) in [(12, 0), (0, 4096)] {
        assert_eq!(session.set_adc_input(channel, value).unwrap_err(),
            "Invalid XP4 external ADC sample");
    }
    assert!(session.set_usb_connected(true).is_err());
    assert_eq!(state(&session), before);
    assert_eq!(session.device_state_json(), devices);
    session.set_adc_input(0, 4095).unwrap();
    session.run(0, 1).unwrap();
    assert!(session.state(false).failure.is_none());
    assert_eq!(session.read_register(0, arm::PC).unwrap(), ENTRY + 4);
}
