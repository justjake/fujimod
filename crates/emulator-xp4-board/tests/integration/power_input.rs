//! A declared input word of the XP4 power peer moves mid-run on the shared
//! board, as a user moves a switch (X-T3 5.11: word 0 bit 10 is the power
//! switch, 0x8080 on and 0x8480 off); undeclared words and boards without a
//! serial part refuse, and the moved word survives a checkpoint.
use crate::support;
use fujimod_emulator_xp4_board::shared;
use serde_json::{Value, json};

fn definition() -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "gpio":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
        "power_spi":[0xfff33000u32,false,12,5,true,
            {"0":0x8080,"1":0,"2":0,"3":0,"32":0,"34":0,"35":0},54000000]})
}

#[test]
fn the_power_switch_word_moves_on_the_shared_board() {
    let board = support::board(definition()).unwrap();
    let before = board.state("xp4-spi1");
    shared::set_power_input(&mut board.0.borrow_mut(), 0, 0x8480).unwrap();
    let after = board.state("xp4-spi1");
    assert_ne!(after, before);
    assert!(
        after.to_string().contains(r#""moved":{"0":33920}"#),
        "{after}"
    );
    let saved = board.checkpoint().unwrap();
    let restored = support::board(definition()).unwrap();
    restored.restore(&saved).unwrap();
    assert_eq!(restored.state("xp4-spi1"), after);
}

#[test]
fn undeclared_words_and_boards_without_a_serial_part_refuse() {
    let board = support::board(definition()).unwrap();
    let before = board.device_state_json();
    let error = shared::set_power_input(&mut board.0.borrow_mut(), 9, 1).unwrap_err();
    assert!(error.contains("cannot change mid-run"), "{error}");
    // An ADC word holds 10 bits.
    let error = shared::set_power_input(&mut board.0.borrow_mut(), 3, 1024).unwrap_err();
    assert!(error.contains("cannot change mid-run"), "{error}");
    assert_eq!(board.device_state_json(), before);
    assert!(board.failure().is_none());

    let mut bare = definition();
    bare.as_object_mut().unwrap().remove("power_spi");
    let board = support::board(bare).unwrap();
    assert_eq!(
        shared::set_power_input(&mut board.0.borrow_mut(), 0, 0x8480).unwrap_err(),
        "No XP4 serial part declares input word 0"
    );
}
