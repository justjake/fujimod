use super::*;
use crate::{durable::Document, test_support::library};

fn processor(library: &Library) -> Core {
    let cpu = library.open(Model::CortexA7, 1 << 20).unwrap();
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    cpu.write_memory(0x1000, &[0x01, 0x00, 0x80, 0xe2, 0xfd, 0xff, 0xff, 0xea])
        .unwrap(); // add r0,1; b 1000
    cpu
}
fn run(cpu: &Core, count: usize) {
    let mut pc = [0; 4];
    cpu.read_register(arm::PC, &mut pc).unwrap();
    cpu.run(Run {
        begin: u64::from(u32::from_le_bytes(pc)),
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
fn registers_banks_and_translation_controls_resume_in_a_fresh_engine() {
    let library = library();
    let (bytes, expected) = {
        let cpu = processor(&library);
        for (i, mode) in MODES.into_iter().enumerate() {
            put(&cpu, arm::CPSR, 0xd0 | mode).unwrap();
            for r in 8..13 {
                put(
                    &cpu,
                    arm::CORE[r],
                    if mode == 0x11 {
                        0xf000 + r as u32
                    } else {
                        r as u32
                    },
                )
                .unwrap();
            }
            put(&cpu, arm::CORE[13], 0x10000 + i as u32 * 4).unwrap();
            put(&cpu, arm::CORE[14], 0x20000 + i as u32 * 4).unwrap();
            if !matches!(mode, 0x10 | 0x1f) {
                put(&cpu, arm::SPSR, 0x600000d3 + i as u32).unwrap();
            }
        }
        put(&cpu, arm::CPSR, 0xd3).unwrap();
        for r in 0..16 {
            let bytes = std::array::from_fn::<_, 16, _>(|i| (r * 16 + i) as u8);
            cpu.write_register(Register::Arm32(arm::Register::Q(r as u8)), &bytes)
                .unwrap();
        }
        put(&cpu, SCALARS[0], 0x00f00000).unwrap();
        put(&cpu, SCALARS[1], 0x40000000).unwrap();
        put(&cpu, SCALARS[2], 0x01400010).unwrap();
        for (index, selector) in CP15.into_iter().enumerate().skip(1) {
            // Alignment-sensitive tables and domain words keep distinct values.
            let value = match index {
                6 | 7 => 0x4000 * (index as u32 - 5),
                _ => 0x100 + index as u32,
            };
            for secure in 0..2 {
                let _ = cp(&cpu, selector, secure, Some(value + secure));
            }
        }
        put(&cpu, arm::PC, 0x1000).unwrap();
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
    let original = state(&cpu);
    for corrupt in 0..4 {
        let mut document: Document<Architecture> = decode(&original).unwrap();
        let Architecture::CortexA7(saved) = &mut document.architecture else {
            unreachable!()
        };
        match corrupt {
            0 => document.schema = 2,
            1 => saved.identity[0] ^= 1,
            2 => saved.cpsr &= !31,
            _ => saved.banks[7].registers[0] ^= 1,
        }
        assert!(cpu.import_snapshot(&encode(&document).unwrap()).is_err());
        assert_eq!(state(&cpu), original);
    }
    let r5 = library.open(Model::CortexR5F, 1 << 20).unwrap();
    assert!(r5.import_snapshot(&original).is_err());
}
