//! Arm PL011 UARTs on the XP4 board (X-T3 5.11: three ports at
//! 0xfff39000 + 0x1000·n, port 0 on interrupt 253): wiring admission, and a
//! port opened and written by guest stores the way the firmware's serial
//! driver opens it.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const CBAR: u32 = 0x2000_0000;
const PORT: u32 = 0xfff3_9000;
const IRQ: u32 = 253;
const CODE: usize = 0x1000;
const RESULT: usize = 0x2000;

fn input() -> Value {
    json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR,
        "initial_registers": [[["Pc", CODE]]]})
}

fn session(value: Value) -> Result<Session, String> {
    let definition: Definition = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Session::new(definition, None, vec![])
}

fn word(session: &Session, address: usize) -> u32 {
    let mut bytes = [0; 4];
    session.read(address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

#[test]
fn the_ports_are_explicit_and_own_their_pages() {
    let absent = session(input()).unwrap();
    assert!(
        !absent
            .device_state_json()
            .keys()
            .any(|k| k.starts_with("pl011"))
    );
    let mut one = input();
    one["pl011_uarts"] = json!([{"base": PORT, "irq": IRQ}]);
    assert!(
        session(one.clone())
            .unwrap()
            .device_state_json()
            .contains_key("pl011")
    );
    let mut three = input();
    three["pl011_uarts"] = json!([
        {"base": PORT, "irq": IRQ},
        {"base": PORT + 0x1000},
        {"base": PORT + 0x2000},
    ]);
    let states = session(three.clone()).unwrap().device_state_json();
    for key in ["pl011#0", "pl011#1", "pl011#2"] {
        assert!(states.contains_key(key), "{key}");
    }
    for address in [PORT, PORT + 0x44, PORT + 0xffc] {
        let mut conflict = one.clone();
        conflict["control_requests"] = json!([[address, 4]]);
        let error = session(conflict).err().unwrap();
        assert!(error.contains("Overlapping"), "{error}");
    }
    let mut twice = input();
    twice["pl011_uarts"] = json!([{"base": PORT}, {"base": PORT}]);
    let error = session(twice).err().unwrap();
    assert!(error.contains("Overlapping PL011 UART"), "{error}");
    let mut extra = one.clone();
    extra["pl011_uarts"][0]["extra"] = json!(true);
    assert!(serde_json::from_value::<Definition>(extra).is_err());
    for uart in [
        json!({"base": PORT + 4}),
        json!({"base": PORT, "irq": 31}),
        json!({"base": PORT, "irq": 512}),
    ] {
        let mut invalid = input();
        invalid["pl011_uarts"] = json!([uart]);
        let error = session(invalid).err().unwrap();
        assert!(error.contains("Invalid PL011 UART"), "{error}");
    }
}

#[test]
fn a_guest_opens_port_0_and_its_transmit_interrupt_reaches_the_declared_line() {
    let mut selected = input();
    selected["pl011_uarts"] = json!([
        {"base": PORT, "irq": IRQ},
        {"base": PORT + 0x1000},
        {"base": PORT + 0x2000},
    ]);
    let board = session(selected).unwrap();

    // The driver's open: CR = 0, the baud rate divisors, a read-modify-write
    // of LCR_H; then enable the port, unmask the transmit interrupt, send one
    // byte and read FR and MIS back.
    let program: [u32; 15] = [
        0xe580_1030, // STR r1, [r0, #0x30]   CR
        0xe580_4024, // STR r4, [r0, #0x24]   IBRD
        0xe580_5028, // STR r5, [r0, #0x28]   FBRD
        0xe590_202c, // LDR r2, [r0, #0x2c]   LCR_H
        0xe382_2060, // ORR r2, r2, #0x60
        0xe580_202c, // STR r2, [r0, #0x2c]
        0xe580_6030, // STR r6, [r0, #0x30]   CR
        0xe580_7038, // STR r7, [r0, #0x38]   IMSC
        0xe5c0_8000, // STRB r8, [r0]         DR
        0xe590_2018, // LDR r2, [r0, #0x18]   FR
        0xe583_2000, // STR r2, [r3]
        0xe590_2040, // LDR r2, [r0, #0x40]   MIS
        0xe583_2004, // STR r2, [r3, #4]
        0xe590_2024, // LDR r2, [r0, #0x24]   IBRD
        0xe583_2008, // STR r2, [r3, #8]
    ];
    let mut bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    bytes.extend(0xeaff_fffe_u32.to_le_bytes()); // B .
    board.write(CODE, &bytes).unwrap();
    for (register, value) in [
        (0, PORT),
        (1, 0),
        (3, RESULT as u32),
        (4, 11),
        (5, 46),
        (6, 0x301),
        (7, 1 << 5),
        (8, 0x41),
    ] {
        board
            .write_register(0, arm::CORE[register], value.into())
            .unwrap();
    }
    board.run_board(4000, 100).unwrap();
    assert!(board.state(false).failure.is_none());

    assert_eq!(word(&board, RESULT), 0x90, "FR: both FIFOs empty");
    assert_eq!(word(&board, RESULT + 4), 1 << 5, "MIS: transmit");
    assert_eq!(word(&board, RESULT + 8), 11, "IBRD");
    let states = board.device_state_json();
    let port0: Value = serde_json::from_str(&states["pl011#0"]).unwrap();
    assert_eq!(port0["sent"], json!([0x41]));
    assert_eq!(port0["lcr_h"], json!(0x60));
    let port1: Value = serde_json::from_str(&states["pl011#1"]).unwrap();
    assert_eq!(port1["sent"], json!([]));
    // The level interrupt stays pending until ICR clears the transmit bit.
    let pending = &board.interrupt_state().unwrap()[0].pending;
    assert_ne!(pending[(IRQ / 32) as usize] & 1 << (IRQ % 32), 0);
}
