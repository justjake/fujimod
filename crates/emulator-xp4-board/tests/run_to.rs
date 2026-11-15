//! Bounded debugger stops on synthetic ARM/Thumb programs.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};

const CODE: usize = 0x1000;
const DATA: usize = 0x4000;
const CBAR: u32 = 0x2000_0000;

fn input(cores: usize) -> Value {
    json!({"ram_size":65536,"cores":cores,"discovery":false,"cbar":CBAR,
        "initial_registers":vec![json!([
            ["Pc",CODE], [{"Arm32":{"R":0}},DATA], [{"Arm32":{"R":1}},42]
        ]);cores]})
}

fn session(value: Value) -> Session {
    let definition: Definition = serde_json::from_value(value).unwrap();
    Session::new(definition, None, vec![]).unwrap()
}

fn program(s: &Session, address: usize, words: &[u32]) {
    s.write(address, &words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>()).unwrap();
}

fn word(s: &Session, address: usize) -> u32 {
    let mut bytes = [0; 4];
    s.read(address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

fn state(s: &Session) -> Value {
    serde_json::to_value(s.state(true)).unwrap()
}

#[test]
fn conditional_branch_stops_before_store_and_ordinary_run_resumes() {
    let s = Rc::new(session(input(1)));
    program(&s, CODE, &[
        0xe3a0_2000, // MOV r2,#0.
        0xe352_0000, // CMP r2,#0.
        0x0a00_0000, // BEQ CODE+16: skip the first store.
        0xe580_1000, // STR r1,[r0].
        0xe281_1001, // ADD r1,r1,#1.
        0xe580_1000, // Target: STR r1,[r0].
        0xe1a0_0000,
    ]);
    let target = (CODE + 20) as u64;
    s.instruction_points(0, vec![(CODE + 16) as u64, target]).unwrap();
    let observed = Rc::new(RefCell::new(Vec::new()));
    let seen = observed.clone();
    let weak = Rc::downgrade(&s);
    s.set_instruction_observer(Some(Rc::new(move |core, pc| {
        let s = weak.upgrade().ok_or("observer lost its session")?;
        // Even an already-reached target must not bypass owner reentrancy.
        let error = s.run_to(core, 1, pc).unwrap_err();
        if !error.contains("Reentrant") {
            return Err(error);
        }
        seen.borrow_mut().push((core, pc));
        Ok(())
    }))).unwrap();
    s.run_to(0, 100, target).unwrap();
    assert_eq!(s.read_register(0, arm::PC).unwrap(), target);
    assert_eq!(s.instructions(), [4]);
    assert_eq!(s.now(), 4);
    assert_eq!(word(&s, DATA), 0);
    assert_eq!(*observed.borrow(), [(0, (CODE + 16) as u64)]);
    s.run(0, 1).unwrap();
    assert_eq!(word(&s, DATA), 43);
    assert_eq!(s.read_register(0, arm::PC).unwrap(), target + 4);
    assert_eq!(*observed.borrow(), [(0, (CODE + 16) as u64), (0, target)]);
    assert!(s.state(false).failure.is_none());
}

#[test]
fn budget_is_exact_when_target_is_not_reached_and_exit_can_move() {
    let s = session(input(1));
    program(&s, CODE, &[0xe282_2001, 0xeaff_fffd]); // ADD r2,#1; B CODE.
    s.run_to(0, 5, (CODE + 8) as u64).unwrap();
    assert_eq!(s.instructions(), [5]);
    assert_eq!(s.read_register(0, arm::CORE[2]).unwrap(), 3);
    assert_eq!(s.read_register(0, arm::PC).unwrap(), (CODE + 4) as u64);
    s.run_to(0, 8, CODE as u64).unwrap();
    assert_eq!(s.instructions(), [6]);
    assert_eq!(s.read_register(0, arm::PC).unwrap(), CODE as u64);
    s.run(0, 2).unwrap();
    assert_eq!(s.instructions(), [8]);
    assert_eq!(s.read_register(0, arm::CORE[2]).unwrap(), 4);
    assert!(s.state(false).failure.is_none());
}

#[test]
fn zero_is_a_real_target_and_an_already_reached_target_is_a_noop() {
    let s = session(input(1));
    program(&s, CODE, &[0xe12f_ff12]); // BX r2, initially zero.
    program(&s, 0, &[0xe580_1000]); // The target would write RAM.
    s.run_to(0, 8, 0).unwrap();
    assert_eq!(s.read_register(0, arm::PC).unwrap(), 0);
    assert_eq!(s.instructions(), [1]);
    assert_eq!(word(&s, DATA), 0);
    let before = state(&s);
    let devices = s.device_state_json();
    s.run_to(0, 8, 0).unwrap();
    assert_eq!(state(&s), before);
    assert_eq!(s.device_state_json(), devices);
    assert_eq!(word(&s, DATA), 0);
    s.run(0, 1).unwrap();
    assert_eq!(word(&s, DATA), 42);
}

#[test]
fn thumb_targets_are_even_halfword_addresses_without_a_mode_bit() {
    let mut value = input(1);
    value["initial_registers"] = json!([[
        ["Pstate",0xf3], ["Pc",CODE | 1], [{"Arm32":{"R":1}},DATA]
    ]]);
    let s = session(value);
    // MOVS r0,#1; ADDS r0,#1; STR r0,[r1].
    s.write(CODE, &[0x01, 0x20, 0x01, 0x30, 0x08, 0x60]).unwrap();
    s.run_to(0, 8, (CODE + 2) as u64).unwrap();
    assert_eq!(s.instructions(), [1]);
    assert_eq!(s.read_register(0, arm::PC).unwrap(), (CODE + 2) as u64);
    assert_eq!(s.read_register(0, arm::CORE[0]).unwrap(), 1);
    assert_ne!(s.read_register(0, arm::CPSR).unwrap() & 0x20, 0);
    s.run_to(0, 8, (CODE + 4) as u64).unwrap();
    assert_eq!(s.instructions(), [2]);
    assert_eq!(s.read_register(0, arm::CORE[0]).unwrap(), 2);
    assert_eq!(word(&s, DATA), 0);
    s.run(0, 1).unwrap();
    assert_eq!(word(&s, DATA), 2);
}

#[test]
fn stop_and_budget_are_per_processor() {
    let s = session(input(2));
    program(&s, CODE, &[0xe282_2001, 0xe282_2001, 0xe282_2001]);
    s.run_to(0, 8, (CODE + 4) as u64).unwrap();
    assert_eq!(s.instructions(), [1, 0]);
    assert_eq!(s.read_register(1, arm::PC).unwrap(), CODE as u64);
    assert_eq!(s.read_register(1, arm::CORE[2]).unwrap(), 0);
    s.run_to(1, 8, (CODE + 8) as u64).unwrap();
    assert_eq!(s.instructions(), [1, 2]);
    assert_eq!(s.read_register(0, arm::PC).unwrap(), (CODE + 4) as u64);
    assert_eq!(s.read_register(0, arm::CORE[2]).unwrap(), 1);
    assert_eq!(s.read_register(1, arm::CORE[2]).unwrap(), 2);
}

#[test]
fn wfi_and_guest_sgi_irq_yield_before_target_and_resume_through_vector() {
    let mut value = input(2);
    value["initial_registers"] = json!([
        [["Pc",0x2000], [{"Arm32":{"R":0}},CBAR+0x1f00],
            [{"Arm32":{"R":1}},0x01000003]],
        [["Pc",CODE], [{"Arm32":{"R":0}},DATA],
            [{"Arm32":{"R":6}},CBAR+0x200c], [{"Arm32":{"R":7}},CBAR+0x2010]]
    ]);
    value["gic_initial"] = json!({"distributor_control":1,"cpu_interfaces":[
        {"control":1,"priority_mask":255,"binary_point":0},
        {"control":1,"priority_mask":255,"binary_point":0}
    ]});
    let s = session(value);
    program(&s, CODE, &[
        0xf108_0080, // CPSIE i.
        0xe320_f003, // WFI.
        0xe3a0_402a, 0xe580_4000, // MOV r4,#42; STR r4,[r0].
        0xe1a0_0000,
    ]);
    program(&s, 0x2000, &[0xe580_1000]); // Guest SGIR write.
    program(&s, 0x18, &[
        0xe596_5000, // LDR r5,[r6]: acknowledge IAR.
        0xe587_5000, // STR r5,[r7]: matching EOI.
        0xe25e_f004, // SUBS pc,lr,#4: architectural IRQ return.
    ]);
    let target = (CODE + 16) as u64;
    s.run_to(1, 64, target).unwrap();
    assert_eq!(s.read_register(1, arm::PC).unwrap(), (CODE + 8) as u64);
    assert!(s.state(false).waiting[1]);
    assert_eq!(word(&s, DATA), 0);
    s.run_to(0, 8, 0x2004).unwrap();
    assert!(!s.state(false).waiting[1]);
    let before = s.instructions();
    s.run_to(1, 64, target).unwrap();
    assert_eq!(s.instructions(), before); // IRQ redirect does not enter the old instruction.
    assert_eq!(s.state(false).irq_entries, [0, 1]);
    assert_eq!(s.read_register(1, arm::PC).unwrap(), 0x18);
    s.run_to(1, 8, 0x1c).unwrap();
    assert_eq!(s.read_register(1, arm::CORE[5]).unwrap(), 3);
    s.run_to(1, 8, 0x20).unwrap();
    s.run_to(1, 64, target).unwrap();
    assert_eq!(s.read_register(1, arm::PC).unwrap(), target);
    assert_eq!(word(&s, DATA), 42);
    assert_eq!(s.read_register(1, arm::CPSR).unwrap() & 0x9f, 0x13);
    assert_eq!(s.state(false).irq_entries, [0, 1]);
    assert!(s.state(false).failure.is_none());
}

#[test]
fn strict_mmio_failure_is_sticky_even_when_its_pc_is_the_next_target() {
    let mut value = input(1);
    // ADC owns [BASE, BASE+0x48), but its aligned word at +0x44 is
    // explicitly unmodeled. Ownership maps the page before this refusal.
    value["adc"] = json!([vec![0u16; 12], 16]);
    value["initial_registers"][0].as_array_mut().unwrap()
        .push(json!([{"Arm32":{"R":2}},0xfff40044u32]));
    let s = session(value);
    program(&s, CODE, &[0xe592_3000, 0xe580_1000]); // Refused LDR; unreachable store.
    let error = s.run_to(0, 8, (CODE + 8) as u64).unwrap_err();
    assert!(error.contains("Unsupported MMIO 0xfff40044"), "{error}");
    assert_eq!(s.read_register(0, arm::PC).unwrap(), CODE as u64);
    assert_eq!(word(&s, DATA), 0);
    let before = state(&s);
    assert_eq!(s.run_to(0, 8, CODE as u64).unwrap_err(), error);
    assert_eq!(s.run(0, 1).unwrap_err(), error);
    assert_eq!(state(&s), before);
}

#[test]
fn invalid_arguments_do_not_mutate_or_poison_the_session() {
    let s = session(input(1));
    program(&s, CODE, &[0xe282_2001]);
    let before = state(&s);
    let devices = s.device_state_json();
    for (core, budget, target) in [
        (1, 1, CODE as u64), (0, 0, CODE as u64),
        (0, 1, CODE as u64 + 1), (0, 1, 1u64 << 32),
    ] {
        assert!(s.run_to(core, budget, target).is_err());
        assert_eq!(state(&s), before);
        assert_eq!(s.device_state_json(), devices);
        assert_eq!(s.read_register(0, arm::PC).unwrap(), CODE as u64);
    }
    s.run_to(0, 1, (CODE + 4) as u64).unwrap();
    assert_eq!(s.instructions(), [1]);
    assert_eq!(s.read_register(0, arm::CORE[2]).unwrap(), 1);
    assert!(s.state(false).failure.is_none());
}
