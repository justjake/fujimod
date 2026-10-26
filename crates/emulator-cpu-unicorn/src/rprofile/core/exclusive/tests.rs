use super::super::tests::{get, processor_with_options, resume, run, set, words};
use super::*;

fn processor() -> (Core, Backing) {
    let backing = Backing::aligned_zeros(0x4000, 4096).unwrap();
    let cpu = processor_with_options(
        &Memory::default(),
        Options {
            cp14_identity: 0x55aa,
            ..Options::default()
        },
    );
    unsafe {
        cpu.map_physical(0, 0, 0x4000, 7, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
    }
    set(&cpu, arm::CPSR, 0xd3);
    set(&cpu, arm::CORE[1], 0x2000);
    set(&cpu, arm::CORE[2], 0x1234_5678);
    (cpu, backing)
}
fn word(cpu: &Core, address: u64) -> u32 {
    let mut bytes = [0; 4];
    cpu.read_memory(address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}
fn exits(cpu: &Core) -> u64 {
    cpu.exit_counters().unwrap()["check_exit_no_sync"]
}

#[test]
fn arm_exclusives_identity_and_pmu_reads_replace_their_translated_bodies() {
    let (cpu, _backing) = processor();
    for word in [
        0xe1910f9f, 0xe1813f92, 0xf57ff01f, 0xee124e10, 0xee190f1d, 0x11910f9f,
    ] {
        assert!(
            matches!(
                cpu.execution.classify(0x1000, word).unwrap().instruction,
                InstructionDisposition::Replace
            ),
            "{word:#x}"
        );
    }
    // A TCM remap stops the run; nonword exclusives refuse when executed.
    for word in [0xee091f31, 0xe1d00f9f, 0xfe124e10] {
        assert!(
            matches!(
                cpu.execution.classify(0x1000, word).unwrap().instruction,
                InstructionDisposition::ObserveAndEndBlock
            ),
            "{word:#x}"
        );
    }
}

#[test]
fn replaced_instructions_run_without_leaving_translated_code() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        let (cpu, _backing) = processor();
        cpu.set_execution_policy(policy).unwrap();
        // ldrex r0,[r1]; strex r3,r2,[r1]; mrc p14 identity r4; clrex;
        // add r5,r5,#1; wfi
        words(
            &cpu,
            0x1000,
            &[
                0xe1910f9f, 0xe1813f92, 0xee124e10, 0xf57ff01f, 0xe2855001, 0xe320f003,
            ],
        );
        cpu.write_memory(0x2000, &7u32.to_le_bytes()).unwrap();
        let before = exits(&cpu);
        assert!(matches!(
            run(&cpu, 0x1000, 100).unwrap(),
            Exit::Hint(Hint::WaitForInterrupt)
        ));
        // Only the WFI hint leaves translated code through its PC write.
        assert_eq!(exits(&cpu) - before, 1);
        assert_eq!(
            (
                get(&cpu, arm::CORE[0]),
                get(&cpu, arm::CORE[3]),
                get(&cpu, arm::CORE[4]),
                get(&cpu, arm::CORE[5]),
                word(&cpu, 0x2000),
                get(&cpu, arm::PC),
            ),
            (7, 0, 0x55aa, 1, 0x1234_5678, 0x1018)
        );
        if policy == ExecutionPolicy::Exact {
            assert_eq!(cpu.instruction_progress(), Some(6));
        }
    }
}

#[test]
fn a_failed_condition_is_the_replaced_instruction_s_no_op() {
    let (cpu, _backing) = processor();
    // ldrexne r0,[r1] with Z set; strex r3,r2,[r1]; wfi
    words(&cpu, 0x1000, &[0x11910f9f, 0xe1813f92, 0xe320f003]);
    set(&cpu, arm::CPSR, 0x4000_00d3);
    set(&cpu, arm::CORE[0], 99);
    cpu.write_memory(0x2000, &7u32.to_le_bytes()).unwrap();
    run(&cpu, 0x1000, 100).unwrap();
    assert_eq!(
        (
            get(&cpu, arm::CORE[0]),
            get(&cpu, arm::CORE[3]),
            word(&cpu, 0x2000)
        ),
        (99, 1, 7)
    );
    assert_eq!(cpu.instruction_progress(), Some(3));
}

struct Reads<'a> {
    cpu: &'a Core,
    calls: Cell<u32>,
}
unsafe fn read_and_stop(_: Access, _: u64, _: i32, _: i64, data: *mut c_void) {
    let reads = unsafe { &*data.cast::<Reads>() };
    reads.calls.set(reads.calls.get() + 1);
    reads.cpu.stop().unwrap();
}

#[test]
fn a_stop_requested_inside_a_replaced_instruction_resumes_after_it() {
    let (cpu, _backing) = processor();
    words(&cpu, 0x1000, &[0xe1910f9f, 0xe1813f92, 0xe320f003]);
    cpu.write_memory(0x2000, &7u32.to_le_bytes()).unwrap();
    let reads = Reads {
        cpu: &cpu,
        calls: Cell::new(0),
    };
    let hook = unsafe {
        cpu.install_hook(Hook::Memory {
            callback: read_and_stop,
            data: (&reads as *const Reads).cast_mut().cast(),
            read: true,
            write: false,
            range: HookRange::Inclusive {
                begin: 0x2000,
                end: 0x2003,
            },
        })
    }
    .unwrap();
    assert!(matches!(run(&cpu, 0x1000, 100).unwrap(), Exit::Stopped));
    assert_eq!(
        (
            get(&cpu, arm::PC),
            get(&cpu, arm::CORE[0]),
            cpu.instruction_progress()
        ),
        (0x1004, 7, Some(1))
    );
    resume(&cpu, 100).unwrap();
    assert_eq!(reads.calls.get(), 1);
    assert_eq!(
        (get(&cpu, arm::CORE[3]), word(&cpu, 0x2000)),
        (0, 0x1234_5678)
    );
    cpu.remove_hook(hook).unwrap();
}

struct Stores<'a> {
    cpu: &'a Core,
    seen: RefCell<Vec<(u64, i32, i64, u32)>>,
}
unsafe fn completed(_: Access, address: u64, size: i32, value: i64, data: *mut c_void) {
    let stores = unsafe { &*data.cast::<Stores>() };
    // The store is visible and the next instruction has not entered.
    stores
        .seen
        .borrow_mut()
        .push((address, size, value, word(stores.cpu, address)));
    stores
        .cpu
        .tighten_instruction_limit(stores.cpu.instruction_progress().unwrap())
        .unwrap();
}

#[test]
fn a_successful_strex_publishes_its_completed_store() {
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        let (cpu, _backing) = processor();
        cpu.set_execution_policy(policy).unwrap();
        // ldrex r0,[r1]; strex r3,r2,[r1]; mov r0,#0x63; b .
        words(
            &cpu,
            0x1000,
            &[0xe1910f9f, 0xe1813f92, 0xe3a00063, 0xeafffffe],
        );
        let stores = Stores {
            cpu: &cpu,
            seen: RefCell::new(Vec::new()),
        };
        let hook = unsafe {
            cpu.install_hook(Hook::CompletedStore {
                callback: completed,
                data: (&stores as *const Stores).cast_mut().cast(),
                range: HookRange::Inclusive {
                    begin: 0x2000,
                    end: 0x2003,
                },
            })
        }
        .unwrap();
        run(&cpu, 0x1000, 20).unwrap();
        assert_eq!(
            *stores.seen.borrow(),
            vec![(0x2000, 4, 0x1234_5678, 0x1234_5678)]
        );
        assert_eq!(get(&cpu, arm::CORE[3]), 0);
        if policy == ExecutionPolicy::Exact {
            assert_eq!(get(&cpu, arm::PC), 0x1008);
            assert_eq!(get(&cpu, arm::CORE[0]), 0);
            assert_eq!(cpu.instruction_progress(), Some(2));
        }
        // A failed STREX stores nothing and publishes nothing.
        words(&cpu, 0x1000, &[0xf57ff01f, 0xe1813f92, 0xe320f003]);
        stores.seen.borrow_mut().clear();
        run(&cpu, 0x1000, 20).unwrap();
        assert_eq!(get(&cpu, arm::CORE[3]), 1);
        assert!(stores.seen.borrow().is_empty());
        cpu.remove_hook(hook).unwrap();
    }
}
