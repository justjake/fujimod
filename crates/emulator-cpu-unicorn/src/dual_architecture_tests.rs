//! ARM and AArch64 in one linked archive.
//!
//! `fujimod-unicorn-sys` configures the pinned Unicorn build for `arm;aarch64`,
//! and every executable and extension links that one archive. These tests open
//! both architectures at once in a single process and step them alternately, so
//! a build that carries only one architecture fails here instead of in a board
//! run.
use crate::{Core, test_support};
use fujimod_emulator_cpu::{Cpu, Model, Register, Run, arm};

/// `mov r0, #7` then `mov r1, #9` in A32.
const ARM_CODE: [u32; 2] = [0xe3a0_0007, 0xe3a0_1009];
/// `mov x0, #7` then `mov x1, #9` in A64.
const AARCH64_CODE: [u32; 2] = [0xd280_00e0, 0xd280_0121];

fn words(code: &[u32]) -> Vec<u8> {
    code.iter().copied().flat_map(u32::to_le_bytes).collect()
}
fn reg32(cpu: &Core, register: Register) -> u32 {
    let mut out = [0; 4];
    assert_eq!(cpu.read_register(register, &mut out).unwrap(), 4);
    u32::from_le_bytes(out)
}
fn reg64(cpu: &Core, register: Register) -> u64 {
    let mut out = [0; 8];
    assert_eq!(cpu.read_register(register, &mut out).unwrap(), 8);
    u64::from_le_bytes(out)
}
fn step(cpu: &Core, begin: u64) {
    cpu.run(Run { begin, until: None, instructions: Some(1), watchdog_us: 0 })
        .unwrap();
}
fn engine(library: &crate::Library, model: Model, code: &[u32]) -> Core {
    let cpu = library.open(model, 1 << 20).unwrap();
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    cpu.write_memory(0x1000, &words(code)).unwrap();
    cpu
}

#[test]
fn arm_and_aarch64_engines_run_side_by_side() {
    let library = test_support::library();
    let arm = engine(&library, Model::CortexA7, &ARM_CODE);
    let aarch64 = engine(&library, Model::ArmV8A, &AARCH64_CODE);
    for word in 0..ARM_CODE.len() as u64 {
        let address = 0x1000 + 4 * word;
        step(&arm, address);
        step(&aarch64, address);
    }
    assert_eq!(reg32(&arm, arm::CORE[0]), 7);
    assert_eq!(reg32(&arm, arm::CORE[1]), 9);
    assert_eq!(reg32(&arm, arm::PC), 0x1000 + 4 * ARM_CODE.len() as u32);
    assert_eq!(reg64(&aarch64, Register::X(0)), 7);
    assert_eq!(reg64(&aarch64, Register::X(1)), 9);
    assert_eq!(reg64(&aarch64, Register::Pc), 0x1000 + 4 * AARCH64_CODE.len() as u64);
}

#[test]
fn a_fresh_engine_of_each_architecture_opens_after_both_ran() {
    let library = test_support::library();
    drop(engine(&library, Model::CortexA7, &ARM_CODE));
    drop(engine(&library, Model::ArmV8A, &AARCH64_CODE));
    let arm = engine(&library, Model::CortexA7, &ARM_CODE);
    let aarch64 = engine(&library, Model::ArmV8A, &AARCH64_CODE);
    step(&arm, 0x1000);
    step(&aarch64, 0x1000);
    assert_eq!(reg32(&arm, arm::CORE[0]), 7);
    assert_eq!(reg64(&aarch64, Register::X(0)), 7);
}
