//! The SD card medium leaves a board and enters the next one. Definition admission only:
//! these tests execute no guest instructions.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

fn definition(sectors: u32) -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0x20000000u32,
        "sd_hosts":[{
            "options":{"base":0xfffc4000u64,"transfer_ticks":8,"clock_ticks":8,"rca":0x1234,
                "ram_bytes":65536,"capabilities":0,"capabilities_2":0,"slot_and_version":0},
            "card":{"sectors":sectors,"max_dirty_sectors":65536,"partition_start":8192,
                "sectors_per_cluster":64,"label":"CAMERA"}}]})
}
fn open(v: Value, files: Vec<(String, Vec<u8>)>, media: Vec<(usize, fujimod_emulator_devices::parts::sd_card::Card)>)
    -> Result<Session, String>
{
    Session::new_with_media(serde_json::from_value::<Definition>(v).unwrap(), None, files, media)
}

#[test]
fn medium_leaves_one_board_and_replaces_the_formatted_card_of_the_next() {
    let first = open(definition(0x800000), vec![("A.TXT".into(), b"hello".to_vec())], vec![]).unwrap();
    let medium = first.card_medium(0).unwrap();
    assert!(medium.dirty_sectors() > 0);
    let second = open(definition(0x800000), vec![], vec![(0, medium.clone())]).unwrap();
    assert_eq!(second.card_medium(0).unwrap(), medium);
}

#[test]
fn undeclared_host_capacity_mismatch_and_double_source_refuse() {
    let medium = open(definition(0x800000), vec![], vec![]).unwrap().card_medium(0).unwrap();
    assert!(open(definition(0x800000), vec![], vec![(1, medium.clone())]).is_err());
    assert!(open(definition(0x1000000), vec![], vec![(0, medium.clone())]).is_err());
    let error = open(definition(0x800000), vec![("A".into(), vec![1])], vec![(0, medium.clone())])
        .err().unwrap();
    assert!(error.contains("not both"), "{error}");
    assert!(open(definition(0x800000), vec![], vec![(0, medium.clone()), (0, medium)]).is_err());
    assert!(open(definition(0x800000), vec![], vec![]).unwrap().card_medium(1).is_err());
}
