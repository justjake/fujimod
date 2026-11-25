//! The in-body stabilizer controller behind SPI2 (0xfff34000). The camera
//! powers it (GPIO 0.12 and 1.14), pulses its reset (12.6), waits for its
//! ready pad (0.4) to fall, which line 171 (active low) reports, then runs
//! four-byte frames over the link by DMA (channels 5 and 6) with chip select
//! 12.3 held low. The first frame drives the ready pad high, which line 155
//! (active high) reports.
use fujimod_emulator_cpu::arm;
mod support;
use support::Board;
use serde_json::{Value, json};
const SPI2: u32 = 0xfff3_4000;
const DATA: u32 = SPI2 + 8;
const BANK: u32 = 0xfff4_1000;
const CBAR: u32 = 0xffff_0000;
const RX: u32 = 0xfffe_5000;
const TX: u32 = 0xfffe_6000;
const TX_BUFFER: usize = 0xf_0000;
const RX_BUFFER: usize = 0xf_0100;
const READY_LOW: u32 = 171;
const READY_HIGH: u32 = 155;
const BOOT: u64 = 3000;

fn input() -> Value {
    let mut levels = [0u16; 16];
    levels[0] = 0x7553;
    json!({"ram_size":0x10_0000,"cores":1,"discovery":false,"cbar":CBAR,
        "gpio":levels,"gpio_base":BANK,
        "gpio_interrupts":[{"port":0,"pin":4,"line":READY_LOW,"active_high":false},
                           {"port":0,"pin":4,"line":READY_HIGH,"active_high":true}],
        "stabilizer_link":{"base":SPI2,"idle":false,
            "select":{"port":12,"pin":3,"active_high":false},
            "power":[{"port":0,"pin":12},{"port":1,"pin":14}],
            "reset":{"port":12,"pin":6},"ready":{"port":0,"pin":4},
            "boot_ticks":BOOT,"replies":{"20":0x1234}},
        "standalone_dma":{"irq_routes":(207..223).collect::<Vec<_>>(),
            "reset_profile":"experimental-zero-registers-v1",
            "timing_profile":"functional-16-setup-16-bytes-v1"}})
}
fn board(value: Value) -> Result<Board, String> {
    support::board(value)
}
fn step(session: &Board, instruction: u32, address: u32, value: u32) -> u32 {
    let pc = 0x1000 + session.instructions() as usize * 4;
    session.write(pc, &instruction.to_le_bytes()).unwrap();
    session.write_register(0, arm::PC, pc as u64).unwrap();
    session
        .write_register(0, arm::CORE[0], address.into())
        .unwrap();
    session
        .write_register(0, arm::CORE[1], value.into())
        .unwrap();
    session.run(0, 1).unwrap_or_else(|error| {
        panic!(
            "{address:#x}: {}",
            session.failure().unwrap_or(error)
        )
    });
    session.read_register(0, arm::CORE[1]).unwrap() as u32
}
fn ww(session: &Board, address: u32, value: u32) {
    step(session, 0xe580_1000, address, value);
}
fn rw(session: &Board, address: u32) -> u32 {
    step(session, 0xe590_1000, address, 0)
}
fn ticks(session: &Board) -> u64 {
    session.now()
}
fn run_to(session: &Board, tick: u64) {
    session.run_until(tick).unwrap();
}
/// A masked write of one output bit.
fn output(session: &Board, port: u32, pin: u32, high: bool) {
    ww(
        session,
        BANK + port * 64,
        1 << (pin + 16) | u32::from(high) << pin,
    );
}
fn ready_pad(session: &Board) -> bool {
    rw(session, BANK + 0x14) & 1 << 4 != 0
}
fn pending(session: &Board, irq: u32) -> bool {
    session.interrupt_state().unwrap()[0].pending[(irq / 32) as usize] & 1 << (irq % 32) != 0
}
fn bytes(session: &Board, address: usize) -> [u8; 4] {
    let mut out = [0; 4];
    session.read(address, &mut out).unwrap();
    out
}

/// Powers the controller and pulses its reset; returns the tick the ready
/// pad is due to fall.
fn power_up(session: &Board) -> u64 {
    output(session, 12, 3, true);
    output(session, 0, 12, true);
    output(session, 1, 14, true);
    output(session, 12, 6, true);
    let before = ticks(session);
    output(session, 12, 6, false);
    let after = ticks(session);
    let due = session.next_device_tick().unwrap();
    assert!(
        (before + BOOT..=after + BOOT).contains(&due),
        "{before} {after} {due}"
    );
    due
}
/// SPI2 as a driver opens it: 8-bit words, divider 0x24, both DMA requests,
/// run; chip select asserted.
fn open_link(session: &Board) {
    ww(session, SPI2, 0xc7);
    ww(session, SPI2 + 4, 8);
    ww(session, SPI2 + 0x10, 0x24);
    ww(session, SPI2 + 0x24, 3);
    ww(session, SPI2 + 0x14, 0xf);
    ww(session, SPI2 + 4, 0xa);
    output(session, 12, 3, false);
}
fn program(session: &Board, channel: u32, source: u32, destination: u32, transmit: bool) {
    let (source_config, destination_config) = if transmit { (0, 4) } else { (4, 0) };
    for (offset, value) in [
        (0x10, 3),
        (0x14, source),
        (0x18, destination),
        (0x1c, source_config),
        (0x20, destination_config),
        (0x24, 0),
        (0x28, 0x0020_1011),
    ] {
        ww(session, channel + offset, value);
    }
}
/// One frame by DMA, as the observed driver runs it; returns the reply.
fn frame(session: &Board, camera: [u8; 4]) -> [u8; 4] {
    session.write(TX_BUFFER, &camera).unwrap();
    ww(session, SPI2 + 0x20, 3);
    ww(session, SPI2 + 0x24, 3);
    program(session, RX, DATA, RX_BUFFER as u32, false);
    program(session, TX, TX_BUFFER as u32, DATA, true);
    for channel in [RX, TX] {
        ww(session, channel + 0x30, 0);
        ww(session, channel + 0x28, 0x1020_1011);
    }
    let end = session.next_device_tick().unwrap();
    run_to(session, end);
    assert_eq!(rw(session, RX + 0x30), 1, "receive channel done");
    ww(session, SPI2 + 0x24, 0);
    for channel in [RX, TX] {
        ww(session, channel + 0x30, 0);
    }
    assert_eq!(rw(session, SPI2 + 0xc) & 0x10, 0);
    bytes(session, RX_BUFFER)
}

#[test]
fn the_ready_pad_falls_after_boot_and_rises_after_the_first_frame() {
    let session = board(input()).unwrap();
    output(&session, 12, 3, true);
    assert!(ready_pad(&session), "idle high while unpowered");
    assert!(pending(&session, READY_HIGH) && !pending(&session, READY_LOW));
    let due = power_up(&session);
    run_to(&session, due - 2);
    assert!(ready_pad(&session), "still booting");
    assert!(!pending(&session, READY_LOW));
    run_to(&session, due);
    assert!(!ready_pad(&session), "low at r + boot_ticks");
    assert!(pending(&session, READY_LOW) && !pending(&session, READY_HIGH));
    open_link(&session);
    assert_eq!(frame(&session, [0x00, 0x00, 0x05, 0x14]), [0; 4]);
    assert!(ready_pad(&session), "high after the first frame");
    assert!(pending(&session, READY_HIGH) && !pending(&session, READY_LOW));
    assert!(session.failure().is_none());
}

#[test]
fn the_worked_example_and_resync_over_the_link() {
    let session = board(input()).unwrap();
    let due = power_up(&session);
    run_to(&session, due);
    open_link(&session);
    for (camera, peer) in [
        ([0x00, 0x00, 0x05, 0x14], [0x00, 0x00, 0x00, 0x00]),
        ([0x08, 0x10, 0x80, 0x22], [0x08, 0x00, 0x85, 0x26]),
        ([0x00, 0x00, 0x00, 0x00], [0x12, 0x34, 0x05, 0x1c]),
        ([0x09, 0x10, 0x80, 0x2a], [0x09, 0x00, 0x80, 0x1a]),
        ([0xa5, 0x5a, 0x3c, 0xc3], [0xc3, 0x3c, 0xa5, 0x5a]),
    ] {
        assert_eq!(frame(&session, camera), peer, "{camera:02x?}");
    }
    let words = session
        .device_records()
        .iter()
        .filter(|r| r["name"] == "XP4 SPI DMA word")
        .count();
    assert_eq!(words, 20);
}

#[test]
fn a_session_resumes_from_a_snapshot_taken_mid_link() {
    let session = board(input()).unwrap();
    let due = power_up(&session);
    run_to(&session, due);
    open_link(&session);
    assert_eq!(frame(&session, [0x00, 0x00, 0x05, 0x14]), [0; 4]);
    assert_eq!(
        frame(&session, [0x08, 0x10, 0x80, 0x22]),
        [0x08, 0x00, 0x85, 0x26]
    );
    let saved = session.checkpoint().unwrap();
    let resumed = board(input()).unwrap();
    resumed.restore(&saved).unwrap();
    for board in [&session, &resumed] {
        assert_eq!(frame(board, [0, 0, 0, 0]), [0x12, 0x34, 0x05, 0x1c]);
        assert_eq!(
            frame(board, [0x09, 0x10, 0x80, 0x2a]),
            [0x09, 0x00, 0x80, 0x1a]
        );
        // Removing power returns the pad to idle; a new pulse boots again.
        output(board, 1, 14, false);
        assert!(ready_pad(board));
        output(board, 1, 14, true);
        output(board, 12, 6, true);
        output(board, 12, 6, false);
        let due = board.next_device_tick().unwrap();
        run_to(board, due);
        assert!(!ready_pad(board));
        assert!(board.failure().is_none());
    }
    assert_eq!(session.device_state_json(), resumed.device_state_json());
}

#[test]
fn wiring_is_refused_outside_the_model() {
    let mut duplicate_line = input();
    duplicate_line["gpio_interrupts"][1]["line"] = json!(READY_LOW);
    let mut same_level = input();
    same_level["gpio_interrupts"][1]["active_high"] = json!(false);
    for value in [duplicate_line, same_level] {
        let error = board(value).err().unwrap();
        assert!(error.contains("Duplicate GPIO interrupt route"), "{error}");
    }
    let mut no_gpio = input();
    for key in ["gpio", "gpio_base", "gpio_interrupts"] {
        no_gpio.as_object_mut().unwrap().remove(key);
    }
    let error = board(no_gpio).err().unwrap();
    assert!(error.contains("requires physical GPIO wiring"), "{error}");
    let mut shared = input();
    shared["stabilizer_link"]["ready"] = json!({"port":12,"pin":6});
    assert!(board(shared).is_err());
    let mut also_plain = input();
    also_plain["spi_inputs"] = json!([[SPI2, false]]);
    assert!(board(also_plain).is_err());
}
