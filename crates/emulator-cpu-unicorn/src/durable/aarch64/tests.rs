use super::*;
use crate::{durable::Document, test_support::library};

fn processor(library: &Library) -> Core {
    let cpu = library.open(Model::ArmV8A, 1 << 20).unwrap();
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    // mov x0, #0; add x0, x0, #1; b 0x1004
    cpu.write_memory(
        0x1000,
        &[
            0x00, 0x00, 0x80, 0xd2, 0x00, 0x04, 0x00, 0x91, 0xff, 0xff, 0xff, 0x17,
        ],
    )
    .unwrap();
    cpu
}
fn run(cpu: &Core, count: usize) {
    let mut pc = [0; 8];
    cpu.read_register(Register::Pc, &mut pc).unwrap();
    cpu.run(Run {
        begin: u64::from_le_bytes(pc),
        until: None,
        watchdog_us: 0,
        instructions: Some(count),
    })
    .unwrap();
}
fn state(cpu: &Core) -> Vec<u8> {
    cpu.snapshot(None).unwrap().export().unwrap()
}

#[test]
fn registers_simd_and_system_state_resume_in_a_fresh_engine() {
    let library = library();
    let (bytes, expected) = {
        let cpu = processor(&library);
        put32(&cpu, Register::Pstate, 0x3c5).unwrap();
        put64(&cpu, Register::Sp, 0x1800).unwrap();
        for index in 0..31u8 {
            put64(&cpu, Register::X(index), 0x100 + u64::from(index) * 8).unwrap();
        }
        for index in 0..32u8 {
            let value: [u8; 16] = std::array::from_fn(|i| (usize::from(index) * 16 + i) as u8);
            cpu.write_register(Register::V(index), &value).unwrap();
        }
        put32(&cpu, Register::Fpcr, 0x01400000).unwrap();
        put32(&cpu, Register::Fpsr, 0x10).unwrap();
        for (encoding, value) in [
            (0xc600, 0x2000),      // VBAR_EL1
            (0xc681, 7),           // CONTEXTIDR_EL1
            (0xc684, 0x5678),      // TPIDR_EL1
            (0xc290, 0x96000004),  // ESR_EL1
            (0xc300, 0xdead0000),  // FAR_EL1
            (0xc208, 0x1f00),      // SP_EL0
            (0xe208, 0x1e00),      // SP_EL1
            (0xde82, 0x1234),      // TPIDR_EL0
        ] {
            put64(&cpu, system(encoding), value).unwrap();
        }
        put64(&cpu, Register::Pc, 0x1000).unwrap();
        run(&cpu, 37);
        let bytes = state(&cpu);
        run(&cpu, 101);
        (bytes, state(&cpu))
    };
    let target = processor(&library);
    let imported = target.import_snapshot(&bytes).unwrap();
    assert_eq!(imported.owner(), target.identity());
    target.restore(imported.as_ref()).unwrap();
    assert_eq!(state(&target), bytes);
    run(&target, 101);
    assert_eq!(state(&target), expected);
}

#[test]
fn invalid_architecture_fails_import_without_changing_the_receiver() {
    let library = library();
    let cpu = processor(&library);
    put32(&cpu, Register::Pstate, 0x3c5).unwrap();
    let original = state(&cpu);
    for corrupt in 0..3 {
        let mut document: Document<Architecture> = decode(&original).unwrap();
        let Architecture::ArmV8A(saved) = &mut document.architecture else {
            unreachable!()
        };
        match corrupt {
            0 => document.schema = 2,
            1 => saved.midr ^= 1,
            _ => saved.pstate |= 0x2,
        }
        assert!(cpu.import_snapshot(&encode(&document).unwrap()).is_err());
        assert_eq!(state(&cpu), original);
    }
    let a7 = library.open(Model::CortexA7, 1 << 20).unwrap();
    assert!(a7.import_snapshot(&original).is_err());
}

#[test]
fn exception_levels_above_el1_refuse_capture_and_import() {
    let library = library();
    for pstate in [0x3c8, 0x3cc, 0x3cd] {
        let cpu = processor(&library);
        put32(&cpu, Register::Pstate, pstate).unwrap();
        assert!(
            cpu.snapshot(None).unwrap().export().is_err(),
            "PSTATE {pstate:#x} captured"
        );
    }
    let cpu = processor(&library);
    put32(&cpu, Register::Pstate, 0x3c5).unwrap();
    let original = state(&cpu);
    let mut document: Document<Architecture> = decode(&original).unwrap();
    for pstate in [0x3c8, 0x3cc, 0x3cd] {
        {
            let Architecture::ArmV8A(saved) = &mut document.architecture else {
                unreachable!()
            };
            saved.pstate = pstate;
        }
        let bytes = encode(&document).unwrap();
        assert!(cpu.import_snapshot(&bytes).is_err());
    }
}
