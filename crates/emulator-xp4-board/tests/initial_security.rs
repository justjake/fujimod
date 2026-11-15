//! Construction inputs and guest-visible Cortex-A7 register permissions.
use fujimod_emulator_cpu::{Register, arm};
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const CODE: usize = 0x1000;
const NS_SMP: u32 = 1 << 18;
const NS_FP: u32 = (1 << 10) | (1 << 11);
const CPACR: u32 = 0xf << 20;
const PROGRAM: [u32; 6] = [
    0xee11_0f30, // MRC p15,0,r0,c1,c0,1: ACTLR.
    0xe380_0040, // ORR r0,r0,#0x40: request SMP without changing other bits.
    0xee01_0f30, // MCR p15,0,r0,c1,c0,1.
    0xee11_1f30, // MRC p15,0,r1,c1,c0,1: guest-visible result.
    0xee01_2f50, // MCR p15,0,r2,c1,c0,2: request CP10/CP11 access.
    0xee11_3f50, // MRC p15,0,r3,c1,c0,2: guest-visible result.
];

fn input(cores: usize) -> Value {
    json!({"ram_size":65536,"cores":cores,"discovery":false,"cbar":0x20000000,
        "initial_registers":vec![json!([
            ["Pc",CODE], [{"Arm32":{"R":2}},CPACR]
        ]);cores]})
}

fn session(value: Value) -> Result<Session, String> {
    let definition: Definition = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Session::new(definition, None, vec![])
}

fn cp(s: &Session, core: usize, crm: u32, opc2: u32, secure: u32) -> u64 {
    s.coprocessor(core, arm::Coprocessor {
        cp: 15, opc1: 0, crn: 1, crm, opc2, secure, wide: false,
    }, None).unwrap()
}

fn assert_initial(s: &Session, core: usize, scr: u32, nsacr: u32, cpsr: u32) {
    assert_eq!(cp(s, core, 1, 0, 1), u64::from(scr));
    assert_eq!(cp(s, core, 1, 2, 1), u64::from(nsacr));
    assert_eq!(s.read_register(core, arm::CPSR).unwrap(), u64::from(cpsr));
    assert_eq!(cp(s, core, 0, 1, 0), 0x6000);
    assert_eq!(cp(s, core, 0, 2, 0), 0);
    assert_eq!(s.read_register(core, arm::PC).unwrap(), CODE as u64);
    assert_eq!(s.state(false).instructions, 0);
}

fn execute(s: &Session, core: usize, smp: bool, cpacr: bool) {
    let fpexc = s.read_register(core, Register::Arm32(arm::Register::Fpexc)).unwrap();
    s.run(core, PROGRAM.len()).unwrap();
    assert_eq!(s.read_register(core, arm::PC).unwrap(), (CODE + PROGRAM.len() * 4) as u64);
    assert_eq!(s.read_register(core, arm::CORE[1]).unwrap(), if smp { 0x6040 } else { 0x6000 });
    assert_eq!(s.read_register(core, arm::CORE[3]).unwrap(), if cpacr { u64::from(CPACR) } else { 0 });
    assert_eq!(s.read_register(core, Register::Arm32(arm::Register::Fpexc)).unwrap(), fpexc);
    assert!(s.state(false).failure.is_none());
    assert!(s.state(false).undefined_entries.iter().all(|&count| count == 0));
}

fn program(s: &Session) {
    // Initial synthetic code is the only host memory input; no post-construction
    // register, CP15, device or interrupt mutation supplies the tested outcome.
    s.write(CODE, &PROGRAM.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>()).unwrap();
}

#[test]
fn omitted_and_empty_security_preserve_reset_and_guest_denial() {
    for empty in [false, true] {
        let mut value = input(1);
        if empty { value["initial_security"] = json!([]); }
        let s = session(value).unwrap();
        assert_initial(&s, 0, 1, 0, 0xd3);
        program(&s);
        execute(&s, 0, false, false);
    }
}

#[test]
fn declared_nonsecure_permissions_are_independent_per_core() {
    let mut value = input(4);
    let permissions = [0, NS_SMP, NS_FP, NS_SMP | NS_FP];
    value["initial_security"] = json!(permissions.map(|nsacr| json!({"scr":1,"nsacr":nsacr})));
    let s = session(value).unwrap();
    for (core, nsacr) in permissions.into_iter().enumerate() {
        assert_initial(&s, core, 1, nsacr, 0xd3);
    }
    program(&s);
    for (core, nsacr) in permissions.into_iter().enumerate() {
        execute(&s, core, nsacr & NS_SMP != 0, nsacr & NS_FP != 0);
        assert_eq!(cp(&s, core, 1, 2, 1), u64::from(nsacr));
    }
    // A fresh default instance does not inherit another instance's permissions.
    let default = session(input(1)).unwrap();
    assert_initial(&default, 0, 1, 0, 0xd3);
}

#[test]
fn secure_svc_and_later_initial_registers_are_applied_in_order() {
    let mut value = input(1);
    value["initial_security"] = json!([{"scr":0,"nsacr":0}]);
    value["initial_registers"][0].as_array_mut().unwrap().extend([
        json!([{"Arm32":"Cpsr"},0x200000d3]), json!(["Sp",0x8000]),
    ]);
    let s = session(value).unwrap();
    assert_initial(&s, 0, 0, 0, 0x200000d3);
    assert_eq!(s.read_register(0, arm::CORE[13]).unwrap(), 0x8000);
    program(&s);
    execute(&s, 0, true, true);
}

#[test]
fn final_initial_cpsr_mode_and_banked_stack_are_verified() {
    let mut value = input(1);
    value["initial_security"] = json!([{"scr":1,"nsacr":NS_SMP}]);
    value["initial_registers"][0].as_array_mut().unwrap().extend([
        json!(["Sp",0x7000]), json!(["Pstate",0xd2]), json!(["Sp",0x8000]),
        json!(["Pstate",0xd3]),
    ]);
    let s = session(value).unwrap();
    assert_initial(&s, 0, 1, NS_SMP, 0xd3);
    // Restoring SVC selects its earlier SP, not the intervening IRQ bank.
    assert_eq!(s.read_register(0, arm::CORE[13]).unwrap(), 0x7000);
}

#[test]
fn unsupported_bits_incomplete_counts_and_invalid_modes_refuse() {
    for count in [1, 3] {
        let mut value = input(2);
        value["initial_security"] = json!(vec![json!({"scr":1,"nsacr":0});count]);
        assert!(session(value).err().unwrap().contains("every processor"));
    }
    for (scr, nsacr) in [(2, 0), (0x100, 0), (u32::MAX, 0),
                         (1, 1), (1, 1 << 19), (1, 1 << 10),
                         (1, 1 << 11), (1, u32::MAX)] {
        let mut value = input(1);
        value["initial_security"] = json!([{"scr":scr,"nsacr":nsacr}]);
        assert!(session(value).err().unwrap().contains("Initial Cortex-A7"));
    }
    for (scr, cpsr) in [(1, 0xe0), (0, 0xda)] {
        let mut value = input(1);
        value["initial_security"] = json!([{"scr":scr,"nsacr":0}]);
        value["initial_registers"][0].as_array_mut().unwrap()
            .push(json!([{"Arm32":"Cpsr"},cpsr]));
        assert!(session(value).err().unwrap().contains("invalid CPSR mode"));
    }
}

#[test]
fn malformed_records_are_rejected_by_the_typed_schema() {
    for initial in [json!([{"scr":1}]), json!([{"nsacr":0}]),
                    json!([{"scr":-1,"nsacr":0}]),
                    json!([{"scr":1,"nsacr":0x100000000u64}]),
                    json!([{"scr":1,"nsacr":0,"extra":0}]),
                    json!({"scr":1,"nsacr":0}), Value::Null] {
        let mut value = input(1);
        value["initial_security"] = initial;
        assert!(serde_json::from_value::<Definition>(value).is_err());
    }
}
