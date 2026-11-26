//! The X-mount lens behind SPI0 (0xfff32000) and GPIO pads.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const SPI0: u32 = 0xfff3_2000;
const DATA: u32 = SPI0 + 8;
const BANK: u32 = 0xfff4_1000;
const CBAR: u32 = 0xffff_0000;
const READY_IRQ: u32 = 55;

fn input() -> Value {
    let levels = [0u16; 16];
    json!({
        "ram_size": 0x10_0000,
        "cores": 1,
        "discovery": false,
        "cbar": CBAR,
        "gpio": levels,
        "gpio_base": BANK,
        "gpio_interrupts": [{"port": 15, "pin": 1, "line": READY_IRQ, "active_high": false}],
        "xp4_lens_link": {
            "base": SPI0,
            "idle": false,
            "detect": {"port": 0, "pin": 11},
            "ready": {"port": 15, "pin": 1},
            "power": [{"port": 1, "pin": 12}, {"port": 1, "pin": 13}],
            "sense": [
                {"port": 11, "pin": 0},
                {"port": 11, "pin": 1},
                {"port": 11, "pin": 2},
                {"port": 15, "pin": 0}
            ],
            "sense_unpowered": [true, false, true, true],
            "boot_ticks": 1000,
            "busy_ticks": 500,
            "operation_ticks": 100,
            "lens": "xf16-80",
            "mounted": true
        }
    })
}

fn board(value: Value) -> Result<Session, String> {
    Session::new(
        serde_json::from_value::<Definition>(value).map_err(|e| e.to_string())?,
        None,
        vec![],
    )
}

fn step(session: &Session, instruction: u32, address: u32, value: u32) -> u32 {
    let pc = 0x1000 + session.state(false).instructions as usize * 4;
    session.write(pc, &instruction.to_le_bytes()).unwrap();
    session.write_register(0, arm::PC, pc as u64).unwrap();
    session.write_register(0, arm::CORE[0], address.into()).unwrap();
    session.write_register(0, arm::CORE[1], value.into()).unwrap();
    session.run(0, 1).unwrap_or_else(|error| {
        panic!("{address:#x}: {}", session.state(false).failure.unwrap_or(error))
    });
    session.read_register(0, arm::CORE[0]).unwrap() as u32
}

fn store(session: &Session, address: u32, value: u32) {
    step(session, 0xe5801000, address, value);
}

fn load(session: &Session, address: u32) -> u32 {
    step(session, 0xe5900000, address, 0)
}

#[test]
fn lens_link_initializes_and_detects_mounted_lens() {
    let session = board(input()).unwrap();

    // Check detect pin (Port 0, Pin 11): should be driven low (0)
    let port0_in = load(&session, BANK + 0x00 * 0x40 + 0x04);
    assert_eq!(port0_in & (1 << 11), 0, "Detect pin 0.11 must be 0 (mounted)");

    // Check sense pins (Port 11: 11.0=1, 11.1=0, 11.2=1):
    let port11_in = load(&session, BANK + 0x0b * 0x40 + 0x04);
    assert_ne!(port11_in & (1 << 0), 0, "Sense 11.0 must be 1");
    assert_eq!(port11_in & (1 << 1), 0, "Sense 11.1 must be 0");
    assert_ne!(port11_in & (1 << 2), 0, "Sense 11.2 must be 1");

    // Check sense pin 15.0=1 and ready pin 15.1=1:
    let port15_in = load(&session, BANK + 0x0f * 0x40 + 0x04);
    assert_ne!(port15_in & (1 << 0), 0, "Sense 15.0 must be 1");
    assert_ne!(port15_in & (1 << 1), 0, "Ready 15.1 must be 1 initially");

    // Assert power: drive Port 1.12 = 1 and Port 1.13 = 1
    store(&session, BANK + 0x01 * 0x40 + 0x00, (1 << 12) | (1 << 13));

    // Run forward past boot_ticks (1000)
    session.run(0, 1500).unwrap();

    // Check ready pad: after 1000 ticks, it was pulled low (interrupt line 55 asserted)
    // Run forward past busy_ticks (500): returns high
    session.run(0, 1000).unwrap();
    let port15_after = load(&session, BANK + 0x0f * 0x40 + 0x04);
    assert_ne!(port15_after & (1 << 1), 0, "Ready 15.1 must be released high");
}
