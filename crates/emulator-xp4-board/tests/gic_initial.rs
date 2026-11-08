use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const CBAR: u32 = 0x2000_0000;
fn input() -> Value {
    json!({"ram_size":65536,"cores":2,"discovery":false,"cbar":CBAR,
        "initial_registers":[[["Pc",0x2000]],[["Pc",0x1000]]]})
}
fn initial(distributor: u32, control: u32, priority_mask: u32) -> Value {
    json!({"distributor_control":distributor,"cpu_interfaces":[
        {"control":0,"priority_mask":0,"binary_point":0},
        {"control":control,"priority_mask":priority_mask,"binary_point":3}]})
}
fn session(value: Value) -> Result<Session, String> {
    Session::new(serde_json::from_value::<Definition>(value).unwrap(),None,vec![])
}
fn program(s: &Session, address: usize, words: &[u32]) {
    s.write(address,&words.iter().flat_map(|w|w.to_le_bytes()).collect::<Vec<_>>()).unwrap();
}
#[test]
fn omitted_initializer_preserves_exact_reset_observations() {
    let absent=session(input()).unwrap();
    let mut explicit=input();explicit["gic_initial"]=json!({"distributor_control":0,
        "cpu_interfaces":[{"control":0,"priority_mask":0,"binary_point":0},
            {"control":0,"priority_mask":0,"binary_point":0}]});
    let explicit=session(explicit).unwrap();
    let a=serde_json::to_value(absent.interrupt_state().unwrap()).unwrap();
    let b=serde_json::to_value(explicit.interrupt_state().unwrap()).unwrap();
    assert_eq!(a,b);
    for core in a.as_array().unwrap() {
        assert_eq!(core["distributor_control"],json!([0]));
        assert_eq!(core["cpu_control"],json!([0,0,0]));
        assert_eq!(core["pending"],json!(vec![0;16]));
    }
    assert_eq!(absent.state(false).instructions,0);
    assert_eq!(explicit.state(false).instructions,0);
}
#[test]
fn selected_registers_are_banked_and_visible_before_execution() {
    let mut value=input();value["gic_initial"]=json!({"distributor_control":1,
        "cpu_interfaces":[{"control":1,"priority_mask":128,"binary_point":7},
            {"control":0,"priority_mask":255,"binary_point":2}]});
    let s=session(value).unwrap();
    let observed=serde_json::to_value(s.interrupt_state().unwrap()).unwrap();
    assert_eq!(observed[0]["cpu_control"],json!([1,128,7]));
    assert_eq!(observed[1]["cpu_control"],json!([0,255,2]));
    assert_eq!(observed[0]["distributor_control"],json!([1]));
    assert_eq!(observed[1]["distributor_control"],json!([1]));
    assert_eq!(s.state(false).instructions,0);
}
#[test]
fn unsupported_bits_counts_and_fields_refuse() {
    let mut unaddressed=input();unaddressed["discovery"]=json!(true);
    unaddressed.as_object_mut().unwrap().remove("cbar");
    unaddressed["gic_initial"]=initial(1,1,255);
    assert!(session(unaddressed).is_err());
    for (field,bad) in [("control",2),("control",5),("priority_mask",256),("binary_point",8)] {
        let mut value=input();value["gic_initial"]=initial(1,1,255);
        value["gic_initial"]["cpu_interfaces"][1][field]=json!(bad);
        assert!(session(value).is_err());
    }
    for bad in [2,u32::MAX] {
        let mut value=input();value["gic_initial"]=initial(bad,1,255);
        assert!(session(value).is_err());
    }
    for count in [0,1,3] {
        let mut value=input();value["gic_initial"]=initial(1,1,255);
        value["gic_initial"]["cpu_interfaces"]=json!(vec![json!({"control":1,"priority_mask":255,"binary_point":0});count]);
        assert!(session(value).is_err());
    }
    for nested in [false,true] {
        let mut value=input();value["gic_initial"]=initial(1,1,255);
        if nested { value["gic_initial"]["cpu_interfaces"][0]["extra"]=json!(0); }
        else { value["gic_initial"]["extra"]=json!(0); }
        assert!(serde_json::from_value::<Definition>(value).is_err());
    }
}
#[test]
fn initial_eligibility_controls_masked_wfi_sgi_wake() {
    for (dist,control,pmr,wakes) in [(1,1,255,true),(0,1,255,false),(1,0,255,false),(1,1,0,false)] {
        let mut value=input();value["gic_initial"]=initial(dist,control,pmr);
        let s=session(value).unwrap();
        // Receiver masks IRQ, waits, and publishes only after architectural wake.
        program(&s,0x1000,&[0xf10c0080,0xe320f003,0xe3a0102a,0xe5801000,0xeafffffe]);
        s.write_register(1,arm::CORE[0],0x4000).unwrap();
        // Sender issues SGI 3 to the other CPU through the normal guest register.
        program(&s,0x2000,&[0xe5801000,0xe320f003]);
        s.write_register(0,arm::CORE[0],u64::from(CBAR+0x1f00)).unwrap();
        s.write_register(0,arm::CORE[1],0x01000003).unwrap();
        s.run(1,8).unwrap();assert!(s.state(false).waiting[1]);
        s.run(0,8).unwrap();s.run_board(2,2).unwrap();
        let mut bytes=[0;4];s.read(0x4000,&mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes),if wakes {42} else {0});
        assert_ne!(s.read_register(1,arm::CPSR).unwrap()&0x80,0);
        assert_eq!(s.state(false).irq_entries,[0,0]);
        assert!(s.state(false).failure.is_none());
    }
}
