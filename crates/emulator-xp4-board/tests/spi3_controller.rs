//! The X-T30 firmware configures SPI3 (0xfff35000) during application start.
//! Declaring the controller answers its control-register read; undeclared, the
//! same read stays a strict refusal.
use fujimod_emulator_cpu::arm;
mod support;
use fujimod_emulator_xp4_board::Definition;
use serde_json::{Value, json};
const SPI3: u32 = 0xfff3_5000;
const CBAR: u32 = 0xffff_0000;

fn input(pin: bool) -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":CBAR,
        "spi_inputs":[[SPI3,pin]]})
}
fn board(value: Value) -> Result<support::Board, String> {
    support::open(serde_json::from_value::<Definition>(value).map_err(|e| e.to_string())?)
}
fn step(session: &support::Board, instruction: u32, address: u32, value: u32) -> u32 {
    let pc = 0x1000 + session.instructions() as usize * 4;
    session.write(pc, &instruction.to_le_bytes()).unwrap();
    session.write_register(0, arm::PC, pc as u64).unwrap();
    session
        .write_register(0, arm::CORE[0], address.into())
        .unwrap();
    session
        .write_register(0, arm::CORE[1], value.into())
        .unwrap();
    session
        .run(0, 1)
        .unwrap_or_else(|error| panic!("{instruction:#x} at {address:#x}: {error}"));
    session.read_register(0, arm::CORE[1]).unwrap() as u32
}
fn ww(session: &support::Board, address: u32, value: u32) {
    step(session, 0xe580_1000, address, value);
}
fn rw(session: &support::Board, address: u32) -> u32 {
    step(session, 0xe590_1000, address, 0)
}

#[test]
fn declared_spi3_answers_the_control_read() {
    let session = board(input(false)).expect("a declared SPI3 controller is strict hardware");
    assert_eq!(rw(&session, SPI3), 0, "control register reset value");
    ww(&session, SPI3, 0x07);
    ww(&session, SPI3 + 0x10, 2);
    ww(&session, SPI3 + 0x04, 0x08);
    ww(&session, SPI3 + 0x20, 0x03);
    assert_eq!(rw(&session, SPI3), 0x07);
    assert_eq!(rw(&session, SPI3 + 0x10), 2);
    assert_eq!(rw(&session, SPI3 + 0x0c) & 0xf, 0x3, "both FIFOs empty");
    assert!(session.failure().is_none());
    assert!(session.device_state_json().contains_key("xp4-spi3"));
}

#[test]
fn spi3_shift_returns_the_declared_miso_level() {
    for (pin, expected) in [(false, 0x00), (true, 0xff)] {
        let session = board(input(pin)).unwrap();
        ww(&session, SPI3, 0x07);
        ww(&session, SPI3 + 0x10, 2);
        ww(&session, SPI3 + 0x04, 0x0a);
        ww(&session, SPI3 + 0x08, 0x5a);
        session.run_board(64, 1).unwrap();
        assert_eq!(rw(&session, SPI3 + 0x08), expected, "{pin}");
        assert!(session.failure().is_none());
    }
}

#[test]
fn undeclared_spi3_stays_a_strict_refusal() {
    let session = board(json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":CBAR})).unwrap();
    session
        .write(0x1000, &0xe590_1000u32.to_le_bytes())
        .unwrap();
    session.write_register(0, arm::PC, 0x1000).unwrap();
    session
        .write_register(0, arm::CORE[0], SPI3.into())
        .unwrap();
    assert!(session.run(0, 1).is_err());
    assert!(session.failure().is_some());
}
