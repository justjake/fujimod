use super::*;
use crate::{durable::Document, test_support::library};

fn processor(library: &Library) -> Core {
    let cpu = library.open(Model::CortexR5F, 1 << 20).unwrap();
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    cpu.write_memory(0x1000, &[0x01, 0x00, 0x80, 0xe2, 0xfd, 0xff, 0xff, 0xea])
        .unwrap(); // add r0,1; b 1000
    cpu
}
fn run(cpu: &Core, count: usize) {
    let mut pc = [0; 4];
    let mut cpsr = [0; 4];
    cpu.read_register(arm::PC, &mut pc).unwrap();
    cpu.read_register(arm::CPSR, &mut cpsr).unwrap();
    let thumb = u32::from_le_bytes(cpsr) & 32 != 0;
    cpu.run(Run {
        begin: u64::from(u32::from_le_bytes(pc) | u32::from(thumb)),
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
fn retained_registers_survive_source_destruction_and_resume_in_a_fresh_engine() {
    let library = library();
    let (saved, bytes, expected) = {
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
        put(&cpu, arm::CORE[0], 7).unwrap();
        put(&cpu, SCALARS[0], 0x00f00000).unwrap();
        put(&cpu, SCALARS[1], 0x40000000).unwrap();
        put(&cpu, SCALARS[2], 0x01400010).unwrap();
        put(&cpu, SCALARS[3], 0x12345678).unwrap();
        for r in 0..16 {
            let bytes = std::array::from_fn::<_, 16, _>(|i| (r * 16 + i) as u8);
            cpu.write_register(Register::Arm32(arm::Register::Q(r as u8)), &bytes)
                .unwrap();
        }
        for index in 0..16 {
            cp(&cpu, SELECT, 0, Some(index)).unwrap();
            for (selector, value) in MPU.into_iter().zip([index * 0x1000, 0x1f, 0x300]) {
                cp(&cpu, selector, 0, Some(value)).unwrap();
            }
        }
        cp(&cpu, SELECT, 0, Some(5)).unwrap();
        for secure in 0..2 {
            cp(&cpu, CP15[7], secure, Some(0x8000 + secure * 0x1000)).unwrap();
            cp(&cpu, CP15[10], secure, Some(0x12340000 + secure)).unwrap();
        }
        put(&cpu, arm::PC, 0x1000).unwrap();
        run(&cpu, 37);
        let saved = cpu.snapshot(None).unwrap();
        let bytes = saved.export().unwrap();
        assert_eq!(cp(&cpu, SELECT, 0, None).unwrap(), 5);
        put(&cpu, arm::CORE[0], 0xdeadbeef).unwrap();
        cp(&cpu, CP15[10], 0, Some(0xdead)).unwrap();
        let live = state(&cpu);
        assert_eq!(saved.export().unwrap(), bytes);
        assert_eq!(state(&cpu), live);
        cpu.restore(saved.as_ref()).unwrap();
        run(&cpu, 101);
        (saved, bytes, state(&cpu))
    };
    assert_eq!(saved.export().unwrap(), bytes);
    let target = processor(&library);
    let before = state(&target);
    assert!(target.restore(saved.as_ref()).is_err());
    let imported = target.import_snapshot(&bytes).unwrap();
    assert_eq!(imported.owner(), target.identity());
    assert_ne!(saved.owner(), target.identity());
    assert_eq!(state(&target), before);
    target.restore(imported.as_ref()).unwrap();
    assert_eq!(state(&target), bytes);
    run(&target, 101);
    assert_eq!(state(&target), expected);
}

#[test]
fn thumb_it_state_and_condition_flags_survive_durable_restore() {
    let library = library();
    for z in [0, 1 << 30] {
        let source = processor(&library);
        source
            .write_memory(0x1000, &[0x01, 0x30, 0xfe, 0xe7])
            .unwrap(); // ADDEQ r0,1 at the saved IT boundary; b .
        put(&source, arm::CORE[0], 23).unwrap();
        put(&source, arm::PC, 0x1001).unwrap();
        put(&source, arm::CPSR, z | 0x800 | 0xf3).unwrap();
        let saved = source.snapshot(None).unwrap().export().unwrap();
        run(&source, 2);
        let expected = state(&source);
        let target = processor(&library);
        target
            .write_memory(0x1000, &[0x01, 0x30, 0xfe, 0xe7])
            .unwrap();
        let imported = target.import_snapshot(&saved).unwrap();
        target.restore(imported.as_ref()).unwrap();
        assert_eq!(state(&target), saved);
        run(&target, 2);
        assert_eq!(state(&target), expected);
        let mut r0 = [0; 4];
        target.read_register(arm::CORE[0], &mut r0).unwrap();
        assert_eq!(u32::from_le_bytes(r0), if z == 0 { 23 } else { 24 });
    }
}

#[test]
fn invalid_architecture_and_aliases_fail_import_without_changing_the_receiver() {
    let library = library();
    let cpu = processor(&library);
    let original = state(&cpu);
    for corrupt in 0..7 {
        let mut document: Document<Architecture> = decode(&original).unwrap();
        let Architecture::CortexR5f(saved) = &mut document.architecture;
        match corrupt {
            0 => document.schema = 2,
            1 => saved.identity[0] ^= 1,
            2 => saved.cpsr &= !31,
            3 => saved.mpu_selected = 16,
            4 => saved.banks[6].registers[0] ^= 1,
            5 => saved.banks[2].spsr = None,
            _ => saved.cp15[7][0] |= 1,
        }
        assert!(cpu.import_snapshot(&encode(&document).unwrap()).is_err());
        assert_eq!(state(&cpu), original);
    }
    let mut trailing = original.clone();
    trailing.push(0);
    assert!(cpu.import_snapshot(&trailing).is_err());
    for end in [0, 1, original.len() - 1] {
        assert!(cpu.import_snapshot(&original[..end]).is_err());
    }
    let a7 = library.open(Model::CortexA7, 1 << 20).unwrap();
    assert!(a7.import_snapshot(&original).is_err());
    assert_eq!(state(&cpu), original);
}
