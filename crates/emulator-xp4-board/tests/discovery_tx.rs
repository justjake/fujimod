use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::json;
fn input(discovery: bool) -> serde_json::Value {
    json!({"ram_size":65536,"cores":1,"discovery":discovery,"cbar":0xffff0000u32,
        "storage_jedec":32,"storage_geometry":[4096,256,8,8],"storage_irq":37,
        "storage_dma_routes":(207..223).collect::<Vec<_>>(),
        "discovery_tx_receiver":{"address":0xfff30044u32,"request":22,"capacity":32,"offer_at":0,"offer_bytes":32}})
}
#[test]
fn discovery_receiver_is_explicit_and_strict_refuses_it() {
    let d:Definition=serde_json::from_value(input(false)).unwrap();
    assert!(Session::new(d,None,vec![]).err().unwrap().contains("requires discovery"));
    let d:Definition=serde_json::from_value(input(true)).unwrap();
    assert!(Session::new(d,Some(vec![255;4096]),vec![]).is_ok());
}
#[test]
fn discovery_receiver_rejects_missing_owner_and_invalid_offer() {
    for (key,value) in [("storage_jedec",json!(null)),("discovery_tx_receiver",json!({"address":0xfff30044u32,"request":22,"capacity":32,"offer_at":0,"offer_bytes":36}))] {
        let mut v=input(true);v[key]=value;
        let d:Definition=serde_json::from_value(v).unwrap();
        assert!(Session::new(d,Some(vec![255;4096]),vec![]).is_err());
    }
}
