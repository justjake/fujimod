use fujimod_emulator_cpu::arm;
mod support;
use fujimod_emulator_xp4_board::Definition;
use serde_json::{Value, json};

const SPI1: u32 = 0xfff3_3000;
const GPIO: u32 = 0xfff4_1000;

fn definition() -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "gpio":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
        "power_spi":[SPI1,false,12,5,true,
            {"0":0,"1":0,"2":0,"3":0,"16":4,"32":0,"34":0,"35":0},54000000],
        "power_spi_bank_inputs":[[true,16,2]],
        "power_spi_eye_commit":true})
}

fn session(value: Value) -> Result<support::Board, String> {
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

fn word(session: &support::Board, tx: u32) -> u32 {
    ww(session, SPI1 + 0x08, tx);
    session.run_board(64, 1).unwrap();
    rw(session, SPI1 + 0x08)
}

fn power_peer(value: Value) -> support::Board {
    let session = session(value).expect("a declared eye sensor is strict hardware");
    ww(&session, GPIO + 12 * 64, (1 << 21) | (1 << 5));
    ww(&session, SPI1, 7);
    ww(&session, SPI1 + 0x10, 2);
    ww(&session, SPI1 + 0x04, 0x0a);
    session
}

#[test]
fn eye_bank_inputs_and_commit_reach_the_power_peer() {
    for bank_inputs in [
        json!([[true, 32, 0]]),
        json!([[true, 3, 1024]]),
        json!([[true, 16, 1], [true, 16, 2]]),
    ] {
        let mut value = definition();
        value["power_spi_bank_inputs"] = bank_inputs;
        assert!(session(value).is_err());
    }

    let session = power_peer(definition());
    // Bank-one register 16 is the eye status word, distinct from the flat
    // bank-zero boot reason 4. Dropping bank inputs lowers this read to 4.
    assert_eq!(
        [
            word(&session, 16 << 2 | 2),
            word(&session, 0),
            word(&session, 0),
        ],
        [0, 0, 2]
    );
    // The eye commit is a bank-zero register 38 bit-15 write. Dropping the
    // option makes the peer refuse the low byte instead of accepting it.
    assert_eq!(
        [
            word(&session, 38 << 2 | 1),
            word(&session, 0x80),
            word(&session, 0),
        ],
        [0, 0, 0]
    );
    assert_eq!(
        [
            word(&session, 38 << 2),
            word(&session, 0),
            word(&session, 0),
        ],
        [0, 0x80, 0]
    );
    assert!(session.failure().is_none());
}

#[test]
fn eye_options_require_the_power_spi_peer() {
    for field in ["power_spi_bank_inputs", "power_spi_eye_commit"] {
        let mut value = definition();
        value.as_object_mut().unwrap().remove("power_spi");
        value
            .as_object_mut()
            .unwrap()
            .remove("power_spi_bank_inputs");
        value["power_spi_bank_inputs"] = if field == "power_spi_bank_inputs" {
            json!([[true, 16, 2]])
        } else {
            json!([])
        };
        value["power_spi_eye_commit"] = json!(field == "power_spi_eye_commit");
        let error = session(value).err().unwrap();
        assert!(error.contains("require a power SPI peer"), "{error}");
    }
}
