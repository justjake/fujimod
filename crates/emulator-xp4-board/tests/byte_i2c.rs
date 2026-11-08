//! Synthetic ARM programs exercise the declared byte-controller wiring.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};
const BASE: u32 = 0x3000_0000;
const CBAR: u32 = 0x2000_0000;
fn input() -> Value { json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":CBAR}) }
fn bus() -> Value { json!({"base":BASE,"irq":40,"profile":{"reset":{"ccr":0,"adr":0,"csr":0,"fsr":0},"byte_ticks":16},"peer":"Empty"}) }
fn selected() -> Value { let mut v=input();v["byte_i2c"]=json!([bus()]);v }
fn session(v:Value)->Result<Session,String>{Session::new(serde_json::from_value::<Definition>(v).map_err(|e|e.to_string())?,None,vec![])}
fn step(s:&Session, instruction:u32,address:u32,value:u32)->u32 {
    let pc = 0x1000 + s.state(false).instructions as usize * 4;
    s.write(pc,&instruction.to_le_bytes()).unwrap();
    s.write_register(0,arm::PC,pc as u64).unwrap();
    s.write_register(0,arm::CORE[0],address.into()).unwrap();
    s.write_register(0,arm::CORE[1],value.into()).unwrap();
    let result=s.run(0,1);
    if let Err(error)=result { panic!("instruction {instruction:x} address {address:x}: {error}"); }
    s.read_register(0,arm::CORE[1]).unwrap() as u32
}
fn wb(s:&Session,offset:u32,value:u32){step(s,0xe5c01000,BASE+offset,value);}
fn rb(s:&Session,offset:u32)->u32{step(s,0xe5d01000,BASE+offset,0)}
fn ww(s:&Session,address:u32,value:u32){step(s,0xe5801000,address,value);}
fn rw(s:&Session,address:u32)->u32{step(s,0xe5901000,address,0)}
fn wait(s:&Session){s.write(0x8000,&[0xe1a00000u32;24].iter().flat_map(|w|w.to_le_bytes()).collect::<Vec<_>>()).unwrap();s.write_register(0,arm::PC,0x8000).unwrap();s.run_board(24,3).unwrap();}
#[test]
fn empty_bus_completion_and_interrupt_are_guest_visible(){
 let s=session(selected()).unwrap();
 ww(&s,CBAR+0x1000,1);ww(&s,CBAR+0x2000,1);ww(&s,CBAR+0x2004,255);
 ww(&s,CBAR+0x1104,1<<8);ww(&s,CBAR+0x1828,1);
 wb(&s,8,0x20);wb(&s,16,0x60);wb(&s,4,0x12);
 assert_eq!(rb(&s,4)&1,0);wait(&s);
 assert_eq!(rb(&s,0),0x99);assert_eq!(rb(&s,4)&3,3);
 assert_eq!(rw(&s,CBAR+0x200c),40);
 wb(&s,4,2);wait(&s);ww(&s,CBAR+0x2010,40);
 assert_eq!(rw(&s,CBAR+0x200c),1023);assert_eq!(rb(&s,0),0);
 assert!(s.state(false).failure.is_none());
}
#[test]
fn declared_register_peer_round_trip_uses_byte_instructions(){
 let mut v=selected();v["byte_i2c"][0]["peer"]=json!({"Register":{"address":48,"registers":{"16":165}}});
 let s=session(v).unwrap();wb(&s,8,0x20);
 for (data,control) in [(0x60,0x10),(16,0x10),(0x5a,0x10)] {wb(&s,16,data);wb(&s,4,control);wait(&s);}
 wb(&s,4,0);wait(&s);
 for (data,control) in [(0x60,0x10),(16,0x10),(0x61,0x30)] {wb(&s,16,data);wb(&s,4,control);wait(&s);}
 wb(&s,4,0x10);wait(&s);assert_eq!(rb(&s,16),0x5a);wb(&s,4,0);wait(&s);
 assert!(s.state(false).failure.is_none());
}
#[test]
fn omission_keeps_unknown_mmio_strict_and_reserved_lanes_refuse(){
 for (v,address,instruction) in [(input(),BASE,0xe5d01000u32),(selected(),BASE+1,0xe5d01000),(selected(),BASE,0xe5901000)] {
  let s=session(v).unwrap();s.write(0x1000,&instruction.to_le_bytes()).unwrap();s.write_register(0,arm::PC,0x1000).unwrap();s.write_register(0,arm::CORE[0],address.into()).unwrap();assert!(s.run(0,1).is_err());assert!(s.state(false).failure.is_some());
 }
}
#[test]
fn invalid_definitions_and_all_physical_owner_conflicts_refuse(){
 for (field,value) in [("base",json!(BASE+1)),("base",json!(0xffff_fff0u64)),("irq",json!(31)),("irq",json!(512)),("extra",json!(0))] {let mut v=selected();v["byte_i2c"][0][field]=value;assert!(session(v).is_err());}
 for (field,value) in [("byte_ticks",json!(0)),("unknown",json!(0))] {let mut v=selected();v["byte_i2c"][0]["profile"][field]=value;assert!(session(v).is_err());}
 for (field,value) in [("ccr",128),("adr",1),("csr",64)] {let mut v=selected();v["byte_i2c"][0]["profile"]["reset"][field]=json!(value);assert!(session(v).is_err());}
 for peer in [json!({"Register":{"address":7,"registers":{"16":1}}}),json!({"Register":{"address":48,"registers":{}}}),json!({"Register":{"address":48,"registers":{"16":1},"extra":0}}),json!("Unknown")] {let mut v=selected();v["byte_i2c"][0]["peer"]=peer;assert!(session(v).is_err());}
 let mut v=selected();v["byte_i2c"][0]["peer"]=json!({"Register":{"address":48,"registers":(0..129).map(|i|(i.to_string(),json!(0))).collect::<serde_json::Map<_,_>>()}});assert!(session(v).is_err());
 for field in ["base","irq","profile","peer"] {let mut v=selected();v["byte_i2c"][0].as_object_mut().unwrap().remove(field);assert!(session(v).is_err());}
 for base in [0,CBAR+0x1000,BASE] {let mut v=selected();v["byte_i2c"][0]["base"]=json!(base);if base==BASE {v["control_requests"]=json!([[BASE+1,8]]);}assert!(session(v).is_err());}
 let mut v=selected();v["memory_regions"]=json!([{"base":BASE,"size":4096,"permissions":3}]);assert!(session(v).is_err());
 let mut v=selected();v["dram_alias"]=json!(BASE);assert!(session(v).is_err());
 let mut v=selected();v["byte_i2c"]=json!([bus(),bus()]);assert!(session(v).is_err());
}

#[test]
fn three_declared_buses_retain_independent_observations() {
    let mut value = selected();
    let buses = (0..3).map(|index| {
        let mut config = bus();
        config["base"] = json!(BASE + index * 0x1000);
        config["irq"] = json!(40 + index);
        config
    }).collect::<Vec<_>>();
    value["byte_i2c"] = json!(buses);
    value["control_requests"] = json!([[BASE + 0x10000, 8]]);
    let s = session(value).unwrap();
    for index in 0..3 {
        step(&s, 0xe5c01000, BASE + index * 0x1000 + 24, index + 1);
    }
    let snapshots = s.device_state_json();
    assert_eq!(snapshots.len(), 4);
    assert!(snapshots.contains_key("xp4-control-requests"));
    assert!(!snapshots.contains_key("byte-i2c"));
    for index in 0..3 {
        let snapshot: Value = serde_json::from_str(&snapshots[&format!("byte-i2c#{index}")]).unwrap();
        assert_eq!(snapshot["base"], json!(BASE + index * 0x1000));
        assert_eq!(snapshot["state"]["fsr"], json!(index + 1));
    }
    assert!(s.state(false).failure.is_none());
}
