//! Exercise the X-T3's catalog-wired X-mount lens on the shared board.
use crate::support::Board;
use fujimod_catalog_configuration::RunEngine;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_cpu::arm;
use fujimod_emulator_parts_lens::x_mount_model::family::{
    APERTURE_RING, IDENTIFICATION, STATE_REQUEST, STATUS,
};
use fujimod_emulator_xp4_board::{catalog::Catalog, shared};
use serde_json::{Value, json};
use std::cell::RefCell;

const SPI0: u32 = 0xfff3_2000;
const DATA: u32 = SPI0 + 8;
const BANK: u32 = 0xfff4_1000;
const READY_IRQ: u32 = 55;
const MS: u64 = 54_000;

fn input(name: &str) -> Value {
    let resolved = ResolvedConfiguration::bundled(name).unwrap();
    let catalog = Catalog::new(&resolved).unwrap();
    let fields = catalog.body_fields().unwrap();
    let gpio_base = &catalog.processor_fields().unwrap()["gpio_base"];
    // These programs need only the body's lens, GPIO bank and the shared GIC.
    let mut input = json!({"ram_size": 0x1_0000, "cores": 1, "discovery": false,
        "cbar": 0xffff_0000u32, "gpio": fields["gpio"], "gpio_base": gpio_base});
    if let Some(lens) = fields.get("xp4_lens_link") {
        input["xp4_lens_link"] = lens.clone();
    }
    input
}

fn board(input: Value) -> (Board, shared::Inputs) {
    let (session, controls) = shared::from_definition(
        serde_json::from_value(input).unwrap(),
        None,
        vec![],
        vec![],
        RunEngine::Unicorn,
    )
    .unwrap();
    (Board(RefCell::new(session)), controls)
}

fn step(board: &Board, instruction: u32, address: u32, value: u32) -> u32 {
    const PC: usize = 0x1000;
    board.write(PC, &instruction.to_le_bytes()).unwrap();
    board.write_register(0, arm::PC, PC as u64).unwrap();
    board
        .write_register(0, arm::CORE[0], address.into())
        .unwrap();
    board.write_register(0, arm::CORE[1], value.into()).unwrap();
    board
        .run(0, 1)
        .unwrap_or_else(|error| panic!("{address:#x}: {error}"));
    board.read_register(0, arm::CORE[1]).unwrap() as u32
}

fn store(board: &Board, address: u32, value: u32) {
    step(board, 0xe580_1000, address, value);
}

fn pads(board: &Board, port: u32) -> u32 {
    step(board, 0xe590_1000, BANK + port * 64 + 0x14, 0)
}

fn ready(board: &Board) -> bool {
    pads(board, 15) & 2 != 0
}

fn irq_asserted(board: &Board) -> bool {
    board.interrupt_state().unwrap()[0].pending[(READY_IRQ / 32) as usize] & (1 << (READY_IRQ % 32))
        != 0
}

fn wait_at(board: &Board, tick: u64) {
    const LOOP: usize = 0x800;
    board.write(LOOP, &0xeaff_fffeu32.to_le_bytes()).unwrap();
    board.write_register(0, arm::PC, LOOP as u64).unwrap();
    board.run_until(tick).unwrap();
}

fn byte(board: &Board, tx: u8) -> u8 {
    store(board, DATA, tx.into());
    let due = board.next_device_tick().unwrap();
    wait_at(board, due);
    step(board, 0xe590_1000, DATA, 0) as u8
}

fn word(board: &Board, tx: u32) -> u32 {
    u32::from_be_bytes(tx.to_be_bytes().map(|tx| byte(board, tx)))
}

// The checksum sums the packet's five 5-bit groups and its last data bit.
fn packet(data: u32) -> u32 {
    let check = (7..32)
        .step_by(5)
        .map(|shift| (data >> shift) & 31)
        .sum::<u32>()
        + ((data >> 6) & 1);
    data | ((check & 31) << 1)
}

fn exchange(board: &Board, header: u32) -> u32 {
    let sent = word(board, header);
    let ack = packet(0x0800_8000 | (sent & 0x7fc0));
    let lens_ack = word(board, ack);
    assert_eq!(lens_ack & 0xffc0, 0x8000 | (header & 0x7fc0));
    assert_eq!(
        lens_ack,
        packet(lens_ack & !63),
        "lens acknowledgement checksum"
    );
    sent
}

fn request(board: &Board, family: u16, payload: u16) -> u16 {
    exchange(
        board,
        packet((u32::from(payload) << 16) | (u32::from(family) << 6)),
    );
    let reply = exchange(board, 0);
    assert_eq!((reply & 0x7fc0) >> 6, u32::from(family));
    assert_eq!(reply, packet(reply & !63), "lens reply checksum");
    (reply >> 16) as u16
}

#[test]
fn xt3_lens_startup_interrupt_and_aperture_ring_are_visible_over_spi() {
    let input = input("xt3-511");
    let (camera, controls) = board(input.clone());
    assert_eq!(pads(&camera, 0) & (1 << 11), 0, "detect is active low");
    assert_eq!(pads(&camera, 11) & 7, 5, "lens sense pattern");
    assert_eq!(pads(&camera, 15) & 3, 3, "fourth sense pad and idle ready");

    store(&camera, SPI0, 0xc7); // 8-bit SPI mode 3
    store(&camera, SPI0 + 0x10, 2);
    store(&camera, SPI0 + 4, 2);
    assert_eq!(byte(&camera, 0), 0xff, "unpowered MISO");

    store(&camera, BANK + 64, (1 << 28) | (1 << 12));
    assert!(
        camera.next_device_tick().is_none(),
        "one supply is insufficient"
    );
    let before = camera.now();
    store(&camera, BANK + 64 + 4, (1 << 29) | (1 << 13));
    let start = camera.next_device_tick().unwrap();
    assert!((before + MS..=camera.now() + MS).contains(&start));
    assert!(ready(&camera) && !irq_asserted(&camera));
    wait_at(&camera, start);
    assert!(!ready(&camera) && irq_asserted(&camera));
    wait_at(&camera, start + MS);
    assert!(ready(&camera) && !irq_asserted(&camera));

    assert_eq!(request(&camera, IDENTIFICATION, 0), 0);
    for (request_header, reply_header) in [([0xff, 0xff], [0, 0]), ([0xff, 0], [0xff, 0xff])] {
        let mut request = [0; 131];
        request[..2].copy_from_slice(&request_header);
        let ident = request.map(|tx| byte(&camera, tx));
        assert_eq!(ident[..2], reply_header);
        assert!(ident[2 + 0x10..].starts_with(b"XF16-80mmF4 R OIS WR"));
        assert_eq!(
            ident[2..130]
                .iter()
                .fold(0u8, |sum, b| sum.wrapping_add(*b)),
            0
        );
        assert_eq!(
            ident[130],
            ident[..130].iter().fold(0u8, |sum, b| sum.wrapping_add(*b))
        );
    }

    assert_eq!(request(&camera, APERTURE_RING, 0), 0);
    controls.set_lens_aperture_ring(7).unwrap();
    assert_eq!(request(&camera, STATUS, 0) & 0x10, 0x10);

    let saved = camera.checkpoint().unwrap();
    let (restored, restored_controls) = board(input);
    restored.restore(&saved).unwrap();
    assert_eq!(request(&restored, STATUS, 0) & 0x10, 0x10);
    assert_eq!(request(&restored, APERTURE_RING, 0), 7);
    restored_controls.set_lens_aperture_ring(7).unwrap();
    assert_eq!(request(&restored, STATUS, 0) & 0x10, 0);
    restored_controls.set_lens_aperture_ring(0).unwrap();
    assert_eq!(request(&restored, APERTURE_RING, 0), 0);

    assert_eq!(request(&camera, STATE_REQUEST, 1), 1);
    assert!(!ready(&camera) && irq_asserted(&camera));
    let release = camera.next_device_tick().unwrap();
    wait_at(&camera, release);
    assert!(ready(&camera) && !irq_asserted(&camera));
    store(&camera, BANK + 64, 1 << 28);
    assert_eq!(
        byte(&camera, 0),
        0xff,
        "dropping either supply turns the lens off"
    );
}

#[test]
fn a_body_without_a_lens_mount_has_no_lens_control_or_spi0_peer() {
    let input = input("xt4-212");
    assert!(input.get("xp4_lens_link").is_none());
    let (camera, controls) = board(input);
    assert!(controls.set_lens_aperture_ring(1).is_err());
    camera.write(0x1000, &0xe590_1000u32.to_le_bytes()).unwrap();
    camera.write_register(0, arm::PC, 0x1000).unwrap();
    camera.write_register(0, arm::CORE[0], SPI0.into()).unwrap();
    assert!(
        camera.run(0, 1).is_err(),
        "the absent lens does not add SPI0"
    );
}
