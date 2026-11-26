//! SPI2 (0xfff34000) moving words by DMA. With the controller's transmit and
//! receive requests enabled (`+0x24` = 3), a driver starts a receive channel
//! of the storage DMA whose fixed source is the data register, then a
//! transmit channel of the same length whose fixed destination is it. Each
//! transmitted word shifts through the controller's declared input pin and
//! the received word lands in the receive buffer; both channels complete
//! when the last word has shifted.
use fujimod_emulator_cpu::arm;
mod support;
use serde_json::{Value, json};
use support::Board;
const SPI2: u32 = 0xfff3_4000;
const DATA: u32 = SPI2 + 8;
const CBAR: u32 = 0xffff_0000;
/// Storage DMA channels 5 (receive) and 6 (transmit) and their IRQ routes.
const RX: u32 = 0xfffe_5000;
const TX: u32 = 0xfffe_6000;
const RX_IRQ: u32 = 207 + 5;
const TX_IRQ: u32 = 207 + 6;
const TX_BUFFER: usize = 0xc000;
const RX_BUFFER: usize = 0xc100;
/// 8-bit words at divider 0x24 and prescaler 0.
const WORD_TICKS: u64 = 8 * 0x24;

fn input(pin: bool) -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":CBAR,
        "spi_inputs":[[SPI2,pin]],
        "standalone_dma":{"irq_routes":(207..223).collect::<Vec<_>>(),
            "reset_profile":"experimental-zero-registers-v1",
            "timing_profile":"functional-16-setup-16-bytes-v1"}})
}
fn board(value: Value) -> Result<Board, String> {
    support::board(value)
}
/// One instruction at a fresh address with `r0` = address and `r1` = value;
/// a refusal comes back as the board's failure.
fn try_step(session: &Board, instruction: u32, address: u32, value: u32) -> Result<u32, String> {
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
        .map_err(|error| session.failure().unwrap_or(error))?;
    Ok(session.read_register(0, arm::CORE[1]).unwrap() as u32)
}
fn try_ww(session: &Board, address: u32, value: u32) -> Result<(), String> {
    try_step(session, 0xe580_1000, address, value).map(|_| ())
}
fn ww(session: &Board, address: u32, value: u32) {
    try_ww(session, address, value)
        .unwrap_or_else(|error| panic!("{address:#x} = {value:#x}: {error}"));
}
fn rw(session: &Board, address: u32) -> u32 {
    try_step(session, 0xe590_1000, address, 0)
        .unwrap_or_else(|error| panic!("{address:#x}: {error}"))
}
fn bytes(session: &Board, address: usize) -> [u8; 4] {
    let mut out = [0; 4];
    session.read(address, &mut out).unwrap();
    out
}
fn pending(session: &Board, irq: u32) -> bool {
    session.interrupt_state().unwrap()[0].pending[(irq / 32) as usize] & 1 << (irq % 32) != 0
}
fn records(session: &Board, name: &str) -> Vec<Value> {
    session
        .device_records()
        .into_iter()
        .filter(|r| r["name"] == name)
        .collect()
}
fn field(record: &Value, key: &str) -> u64 {
    record[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key}: {record}"))
}

/// The observed open: 8-bit words, master, divider 0x24, both DMA requests,
/// causes cleared, then run.
fn open(session: &Board, requests: u32) {
    let control = rw(session, SPI2);
    ww(session, SPI2, (control & 0xffff_00c0) | 0xc7);
    let enable = rw(session, SPI2 + 4);
    ww(session, SPI2 + 4, (enable & !7) | 8);
    ww(session, SPI2 + 0x10, 0x24);
    let enabled = rw(session, SPI2 + 0x24);
    ww(session, SPI2 + 0x24, enabled | requests);
    ww(session, SPI2 + 0x14, 0xf);
    let enable = rw(session, SPI2 + 4);
    ww(session, SPI2 + 4, enable | 2);
}
/// The observed frame preamble: clear both FIFOs, enable both requests and
/// route request 4 to channel 5 (receive) and request 5 to channel 6
/// (transmit).
fn prepare(session: &Board, requests: u32) {
    ww(session, SPI2 + 0x20, 3);
    let enabled = rw(session, SPI2 + 0x24);
    ww(session, SPI2 + 0x24, enabled | requests);
    ww(session, 0xfff4_a508, 0x0100_042c);
    ww(session, 0xfff4_a50c, 0x0001_2c05);
}
/// `+0x1c`/`+0x20`: fixed flag in bit 2, unit 1, byte width.
fn program(
    session: &Board,
    channel: u32,
    bytes: u32,
    source: u32,
    destination: u32,
    transmit: bool,
) {
    let (source_config, destination_config) = if transmit { (0, 4) } else { (4, 0) };
    for (offset, value) in [
        (0x10, bytes - 1),
        (0x14, source),
        (0x18, destination),
        (0x1c, source_config),
        (0x20, destination_config),
        (0x24, 0),
        (0x28, 0x0020_1011),
    ] {
        assert_eq!(rw(session, channel + 0x30) & 0x1_0000, 0);
        ww(session, channel + offset, value);
    }
}
/// The observed start: acknowledge, clear the enable bit, wait for idle, set it.
fn try_start(session: &Board, channel: u32) -> Result<(), String> {
    ww(session, channel + 0x30, 0);
    let control = rw(session, channel + 0x28);
    ww(session, channel + 0x28, control & !0x1000_0000);
    assert_eq!(rw(session, channel + 0x30) & 0x1_0000, 0);
    let control = rw(session, channel + 0x28);
    try_ww(session, channel + 0x28, control | 0x1000_0000)
}
fn frame(session: &Board) {
    prepare(session, 3);
    program(session, RX, 4, DATA, RX_BUFFER as u32, false);
    program(session, TX, 4, TX_BUFFER as u32, DATA, true);
    try_start(session, RX).unwrap();
    try_start(session, TX).unwrap();
}
/// The observed completion: requests off, both channels stopped once idle,
/// routing back to idle (0x2c).
fn finish(session: &Board) {
    let enabled = rw(session, SPI2 + 0x24);
    ww(session, SPI2 + 0x24, enabled & !3);
    for channel in [RX, TX] {
        assert_eq!(rw(session, channel + 0x30) & 0x1_0000, 0);
        ww(session, channel + 0x30, 0);
    }
    ww(session, 0xfff4_a508, 0x0100_2c2c);
    ww(session, 0xfff4_a50c, 0x0001_2c2c);
}
/// The exchange start record of the latest frame: its start and end ticks.
fn exchange(session: &Board) -> (u64, u64) {
    let begun = records(session, "XP4 SPI DMA exchange start")
        .pop()
        .unwrap();
    assert_eq!(field(&begun, "receive_channel"), 5);
    assert_eq!(field(&begun, "transmit_channel"), 6);
    assert_eq!(field(&begun, "receive_request"), 4);
    assert_eq!(field(&begun, "transmit_request"), 5);
    assert_eq!(field(&begun, "word_ticks"), WORD_TICKS);
    (field(&begun, "tick"), field(&begun, "end"))
}

#[test]
fn a_full_duplex_frame_exchanges_each_word_with_the_declared_pin() {
    for (pin, expected) in [(false, 0x00u8), (true, 0xff)] {
        let session = board(input(pin)).unwrap();
        session.write(TX_BUFFER, &[0x12, 0x34, 0x56, 0x78]).unwrap();
        session.write(RX_BUFFER, &[0xaa; 4]).unwrap();
        open(&session, 3);
        frame(&session);
        let (start, end) = exchange(&session);
        // The controller's clock paces the whole exchange: four word times
        // from the transmit start, with no DMA setup charge.
        assert_eq!(end, start + 4 * WORD_TICKS);
        assert_eq!(session.next_device_tick(), Some(end));
        assert_eq!(
            rw(&session, SPI2 + 0xc) & 0x10,
            0x10,
            "busy while the words shift"
        );
        assert_eq!(rw(&session, RX + 0x30), 0x1_0000);
        session.run_until(end - 1).unwrap();
        assert_eq!(bytes(&session, RX_BUFFER), [0xaa; 4]);
        assert!(!pending(&session, RX_IRQ));
        session.run_until(end).unwrap();
        assert_eq!(rw(&session, RX + 0x30), 1, "receive channel done");
        assert_eq!(rw(&session, TX + 0x30), 1, "transmit channel done");
        assert_eq!(bytes(&session, RX_BUFFER), [expected; 4], "{pin}");
        assert!(pending(&session, RX_IRQ) && pending(&session, TX_IRQ));
        let words: Vec<_> = records(&session, "XP4 SPI DMA word")
            .iter()
            .map(|r| (field(r, "tx"), field(r, "rx"), field(r, "tick")))
            .collect();
        let shifted: Vec<_> = [0x12, 0x34, 0x56, 0x78]
            .into_iter()
            .zip(1..)
            .map(|(tx, n)| (tx, u64::from(expected), start + n * WORD_TICKS))
            .collect();
        assert_eq!(words, shifted);
        let completed = records(&session, "XP4 DMA complete");
        assert_eq!(
            completed
                .iter()
                .map(|r| (field(r, "channel"), field(r, "tick")))
                .collect::<Vec<_>>(),
            [(6, end), (5, end)]
        );
        finish(&session);
        assert!(!pending(&session, RX_IRQ) && !pending(&session, TX_IRQ));
        assert_eq!(
            rw(&session, SPI2 + 0xc) & 0x10,
            0,
            "close sees the controller idle"
        );
        // The next frame runs the same way.
        session.write(RX_BUFFER, &[0x55; 4]).unwrap();
        frame(&session);
        let (_, end) = exchange(&session);
        session.run_until(end).unwrap();
        assert_eq!(rw(&session, RX + 0x30), 1);
        assert_eq!(bytes(&session, RX_BUFFER), [expected; 4]);
        finish(&session);
        assert!(session.failure().is_none());
    }
}

#[test]
fn the_request_enable_register_takes_bits_0_and_1_only() {
    let session = board(input(false)).unwrap();
    for value in 0..4 {
        ww(&session, SPI2 + 0x24, value);
        assert_eq!(rw(&session, SPI2 + 0x24), value);
    }
    ww(&session, SPI2 + 0x24, 0);
    let error = try_ww(&session, SPI2 + 0x24, 4).unwrap_err();
    assert!(
        error.contains("XP4 SPI request enable +0x24 is unsupported"),
        "{error}"
    );
}

#[test]
fn a_lone_or_mismatched_direction_is_refused() {
    let session = board(input(false)).unwrap();
    open(&session, 3);
    prepare(&session, 3);
    program(&session, TX, 4, TX_BUFFER as u32, DATA, true);
    let error = try_start(&session, TX).unwrap_err();
    assert!(
        error.contains("transmit needs a running receive channel"),
        "{error}"
    );

    let session = board(input(false)).unwrap();
    open(&session, 3);
    prepare(&session, 3);
    program(&session, RX, 4, DATA, RX_BUFFER as u32, false);
    program(&session, TX, 8, TX_BUFFER as u32, DATA, true);
    try_start(&session, RX).unwrap();
    let error = try_start(&session, TX).unwrap_err();
    assert!(
        error.contains("transmit and receive lengths differ"),
        "{error}"
    );

    // Without its request enabled, a channel on the data register refuses.
    let session = board(input(false)).unwrap();
    open(&session, 0);
    prepare(&session, 1);
    program(&session, RX, 4, DATA, RX_BUFFER as u32, false);
    let error = try_start(&session, RX).unwrap_err();
    assert!(error.contains("receive request is not enabled"), "{error}");

    // The CPU may not use the data register while DMA requests are enabled.
    let session = board(input(false)).unwrap();
    open(&session, 3);
    let error = try_ww(&session, DATA, 0x5a).unwrap_err();
    assert!(error.contains("DMA requests are enabled"), "{error}");

    let session = board(input(false)).unwrap();
    open(&session, 3);
    frame(&session);
    let error = try_ww(&session, DATA, 0x5a).unwrap_err();
    assert!(error.contains("during a DMA exchange"), "{error}");
}

#[test]
fn a_frame_resumes_from_a_snapshot_taken_mid_exchange() {
    let session = board(input(true)).unwrap();
    session.write(TX_BUFFER, &[1, 2, 3, 4]).unwrap();
    open(&session, 3);
    frame(&session);
    let (_, end) = exchange(&session);
    let saved = session.checkpoint().unwrap();
    let resumed = board(input(true)).unwrap();
    resumed.restore(&saved).unwrap();
    for board in [&session, &resumed] {
        assert_eq!(rw(board, SPI2 + 0xc) & 0x10, 0x10);
        board.run_until(end).unwrap();
        assert_eq!(rw(board, RX + 0x30), 1);
        assert_eq!(bytes(board, RX_BUFFER), [0xff; 4]);
        finish(board);
        assert!(board.failure().is_none());
    }
    assert_eq!(session.device_state_json(), resumed.device_state_json());
}
