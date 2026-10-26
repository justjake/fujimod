use super::*;

#[test]
fn finite_store_observers_do_not_accumulate_native_hooks_on_a_parked_core() {
    unsafe fn observed(_: Access, address: u64, _: i32, value: i64, data: *mut c_void) {
        unsafe { &*data.cast::<RefCell<Vec<(u64, i64)>>>() }
            .borrow_mut()
            .push((address, value));
    }
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe5810000, 0xe5810004, 0xe320f003]);
    set(&cpu, arm::CPSR, 0xd3);
    set(&cpu, arm::CORE[0], 7);
    set(&cpu, arm::CORE[1], 0x2000);
    set(&cpu, arm::PC, 0x1000);
    let calls = RefCell::new(Vec::<(u64, i64)>::new());
    let hook = Hook::CompletedStore {
        callback: observed,
        data: (&calls as *const RefCell<Vec<(u64, i64)>>).cast_mut().cast(),
        range: HookRange::Inclusive {
            begin: 0x2000,
            end: 0x2003,
        },
    };
    let native_hooks = cpu.execution.cpu.hooks.borrow().len();
    let before = cpu.fingerprint().unwrap();
    // No intervening execution drains Unicorn's deferred deletion list.
    for _ in 0..1024 {
        let id = unsafe { cpu.install_hook(hook) }.unwrap();
        assert_eq!(cpu.execution.cpu.hooks.borrow().len(), native_hooks);
        cpu.remove_hook(id).unwrap();
    }
    assert_eq!(cpu.fingerprint().unwrap(), before);
    let id = unsafe { cpu.install_hook(hook) }.unwrap();
    assert!(matches!(
        resume(&cpu, 100).unwrap(),
        Exit::Hint(Hint::WaitForInterrupt)
    ));
    assert_eq!(*calls.borrow(), vec![(0x2000, 7)]);
    cpu.remove_hook(id).unwrap();
    set(&cpu, arm::PC, 0x1000);
    resume(&cpu, 100).unwrap();
    assert_eq!(*calls.borrow(), vec![(0x2000, 7)]);
    assert_eq!(cpu.execution.cpu.hooks.borrow().len(), native_hooks);
}

#[test]
fn completed_store_observers_see_written_ram_before_the_next_instruction() {
    unsafe fn stop(_: Access, address: u64, _: i32, _: i64, data: *mut c_void) {
        let cpu = unsafe { &*data.cast::<Core>() };
        let mut bytes = [0; 4];
        cpu.read_memory(address, &mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes), 42);
        cpu.tighten_instruction_limit(cpu.instruction_progress().unwrap())
            .unwrap();
    }
    for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
        let cpu = processor(&Memory::default());
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        words(
            &cpu,
            0x1000,
            &[0xe3a0002a, 0xe5810000, 0xe3a00063, 0xeafffffe],
        );
        cpu.set_execution_policy(policy).unwrap();
        set(&cpu, arm::CPSR, 0xd3);
        set(&cpu, arm::CORE[1], 0x2000);
        set(&cpu, arm::PC, 0x1000);
        let id = unsafe {
            cpu.install_hook(Hook::CompletedStore {
                callback: stop,
                data: (&cpu as *const Core).cast_mut().cast(),
                range: HookRange::Inclusive {
                    begin: 0x2000,
                    end: 0x2003,
                },
            })
        }
        .unwrap();
        resume(&cpu, 20).unwrap();
        match policy {
            ExecutionPolicy::Exact => assert_eq!(cpu.instruction_progress(), Some(2)),
            ExecutionPolicy::Interactive => assert!(cpu.instruction_progress().unwrap() >= 2),
        }
        assert_eq!(get(&cpu, arm::PC), 0x1008);
        assert_eq!(get(&cpu, arm::CORE[0]), 42);
        cpu.remove_hook(id).unwrap();
    }
}

#[test]
fn sticky_pause_stops_instead_of_retrying_a_pending_restart() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    words(&cpu, 0x1000, &[0xeaff_fffe]); // b .

    let execution = &cpu.execution;
    execution.cap.set(10);
    execution.restarted.set(Some(0x1000));
    execution.pause_requested.store(1, Ordering::Release);
    let _run = execution
        .memory
        .reserve_run(
            execution.id.get(),
            execution.policy.get(),
            execution.parallel_tcg,
        )
        .unwrap();

    let exit = cpu
        .execute(
            Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(10),
            },
            0x1000,
        )
        .unwrap();

    assert!(matches!(exit, Exit::Stopped));
    // The C block-entry pause returns before the Rust block callback can clear
    // this marker, so the worker must not retry it while the pause is sticky.
    assert_eq!(execution.restarted.get(), Some(0x1000));
    assert!(!execution.active.get());
}

#[test]
fn execution_policy_cannot_change_during_a_run_reservation() {
    let cpu = processor(&Memory::default());
    let execution = &cpu.execution;
    let run = execution
        .memory
        .reserve_run(
            execution.id.get(),
            ExecutionPolicy::Exact,
            execution.parallel_tcg,
        )
        .unwrap();
    assert!(
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap_err()
            .contains("requires a stopped processor")
    );
    drop(run);
    cpu.set_execution_policy(ExecutionPolicy::Interactive).unwrap();
}

#[test]
fn only_parallel_worker_construction_opts_into_parallel_tcg() {
    let memory = Memory::default();
    let serialized = processor(&memory);
    serialized
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    assert!(!serialized.execution.parallel_tcg);

    let worker = parallel_worker_processor(&memory);
    worker
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    assert!(worker.execution.parallel_tcg);
    worker.set_execution_policy(ExecutionPolicy::Exact).unwrap();
    assert!(worker.execution.parallel_tcg);
}

#[test]
fn parallel_worker_policy_is_restored_with_its_snapshot() {
    let worker = parallel_worker_processor(&Memory::default());
    worker
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    let snapshot = worker.snapshot(None).unwrap();

    worker.set_execution_policy(ExecutionPolicy::Exact).unwrap();
    assert_eq!(worker.execution.policy.get(), ExecutionPolicy::Exact);
    worker.restore(snapshot.as_ref()).unwrap();
    assert_eq!(worker.execution.policy.get(), ExecutionPolicy::Interactive);
    assert!(worker.execution.parallel_tcg);
}

#[test]
fn serialized_core_rejects_an_overlapping_interactive_run_before_execution() {
    let memory = Memory::default();
    let first = processor(&memory);
    let second = processor(&memory);
    for cpu in [&first, &second] {
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        words(cpu, 0x1000, &[0xe320f003]);
        set(cpu, arm::CPSR, 0xd3);
        set(cpu, arm::PC, 0x1000);
    }

    let reservation = first
        .execution
        .memory
        .reserve_run(
            first.execution.id.get(),
            first.execution.policy.get(),
            first.execution.parallel_tcg,
        )
        .unwrap();
    let error = run(&second, 0x1000, 1).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("parallel TCG on every processor")
    );
    assert_eq!(get(&second, arm::PC), 0x1000);
    drop(reservation);
}

struct StopBeforeProgress {
    cpu: *const Core,
    before: Cell<Option<u64>>,
    after: Cell<Option<u64>>,
}
unsafe fn stop_before_and_record_progress(_: u64, _: u32, data: *mut c_void) {
    let p = unsafe { &*data.cast::<StopBeforeProgress>() };
    let cpu = unsafe { &*p.cpu };
    p.before.set(cpu.instruction_progress());
    cpu.stop_before_instruction().unwrap();
    p.after.set(cpu.instruction_progress());
}
#[test]
fn interactive_arm_stop_before_preserves_coarse_progress() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe2800001]);
    cpu.set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    set(&cpu, arm::CPSR, 0xd3);
    set(&cpu, arm::PC, 0x1000);
    let mut progress = StopBeforeProgress {
        cpu: &cpu,
        before: Cell::new(None),
        after: Cell::new(None),
    };
    let hook = unsafe {
        cpu.install_hook(Hook::Instruction {
            callback: stop_before_and_record_progress,
            data: (&mut progress as *mut StopBeforeProgress).cast(),
            range: HookRange::Inclusive {
                begin: 0x1000,
                end: 0x1000,
            },
        })
    }
    .unwrap();

    assert!(matches!(run(&cpu, 0x1000, 10).unwrap(), Exit::Stopped));
    assert_eq!(progress.before.get(), Some(1));
    assert_eq!(progress.after.get(), progress.before.get());
    assert_eq!(cpu.instruction_progress(), progress.before.get());
    assert_eq!(get(&cpu, arm::PC), 0x1000);
    cpu.remove_hook(hook).unwrap();
}

fn processor(memory: &Memory) -> Core {
    processor_with_options(
        memory,
        Options {
            cp14_identity: 0x55aa,
            ..Options::default()
        },
    )
}
fn parallel_worker_processor(memory: &Memory) -> Core {
    Core::new_parallel_worker(
        crate::test_support::library()
            .open(Model::CortexR5F, 1024 * 1024)
            .unwrap(),
        Options::default(),
        Wiring {
            memory: memory.clone(),
        },
    )
    .unwrap()
}
#[test]
fn disabled_vfp_enters_the_guest_undefined_vector_and_retries() {
    for access in [0, 0x500000, 0xf00000] {
        for quantum in [1, 17, 1000] {
            let memory = Memory::default();
            let cpu = processor(&memory);
            cpu.map_zeroed(0, 0x4000, 7).unwrap();
            // VPUSH {d8-d9}; observe SP; VPOP; WFI. The guest handler grants
            // CP10/11, enables FPEXC and retries the faulting instruction.
            words(
                &cpu,
                0x1000,
                &[0xed2d8b04, 0xe1a0a00d, 0xecbd8b04, 0xe320f003],
            );
            words(
                &cpu,
                4,
                &[
                    0xe14f4000, 0xe1a0500e, 0xe3a0360f, 0xee013f50, 0xe3a03101, 0xeee83a10,
                    0xe2888001, 0xe25ef004,
                ],
            );
            set(&cpu, arm::CPSR, 0x53);
            set(&cpu, Register::Arm32(arm::Register::Cpacr), access);
            set(&cpu, arm::CORE[13], 0x3000);
            set(&cpu, arm::PC, 0x1000);
            let mut entries = 0;
            for _ in 0..100 {
                let exit = resume(&cpu, quantum).unwrap();
                entries += cpu.instruction_progress().unwrap();
                if matches!(exit, Exit::Hint(Hint::WaitForInterrupt)) {
                    break;
                }
            }
            assert_eq!(get(&cpu, arm::PC), 0x1010);
            assert_eq!(get(&cpu, arm::CORE[4]), 0x53);
            assert_eq!(get(&cpu, arm::CORE[5]), 0x1004);
            assert_eq!(get(&cpu, arm::CORE[8]), 1);
            assert_eq!(get(&cpu, arm::CORE[10]), 0x2ff0);
            assert_eq!(get(&cpu, arm::CORE[13]), 0x3000);
            assert_eq!(entries, 13);
        }
    }
}
#[test]
fn inactive_vfp_conditions_and_other_invalid_instructions_keep_their_meaning() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    // CMP r0,r0; VPUSHNE; WFI. The failed condition does not access VFP.
    words(&cpu, 0x1000, &[0xe1500000, 0x1d2d8b04, 0xe320f003]);
    set(&cpu, arm::CPSR, 0x53);
    assert!(matches!(
        run(&cpu, 0x1000, 10).unwrap(),
        Exit::Hint(Hint::WaitForInterrupt)
    ));
    assert_eq!(cpu.instruction_progress(), Some(3));
    assert_eq!(get(&cpu, arm::CPSR) & 31, 0x13);
    // UDF is not a disabled-VFP access and retains the backend's refusal.
    words(&cpu, 0x1100, &[0xe7f000f0]);
    assert!(matches!(
        run(&cpu, 0x1100, 1),
        Err(ExecutionError::InvalidInstruction(_))
    ));
}
fn mpu(cpu: &Core, region: u32, base: u32, size: u32, access: u32) {
    let transfer = |crn, crm, opc2, value| {
        cpu.coprocessor(
            arm::Coprocessor {
                cp: 15,
                opc1: 0,
                crn,
                crm,
                opc2,
                secure: 0,
                wide: false,
            },
            Some(value as u64),
        )
        .unwrap()
    };
    transfer(6, 2, 0, region);
    transfer(6, 1, 0, base);
    transfer(6, 1, 2, size);
    transfer(6, 1, 4, access);
}
fn system(cpu: &Core, crn: u32, crm: u32, opc2: u32, value: Option<u64>) -> u64 {
    cpu.coprocessor(
        arm::Coprocessor {
            cp: 15,
            opc1: 0,
            crn,
            crm,
            opc2,
            secure: 0,
            wide: false,
        },
        value,
    )
    .unwrap()
}
#[test]
fn mpu_prefetch_abort_runs_the_guest_permission_handler_and_retries() {
    for quantum in [1, 17, 1000] {
        let cpu = processor(&Memory::default());
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        words(&cpu, 0x2000, &[0xe3a0002a, 0xe320f003]);
        // Save ABT return state, grant access to the selected region, retry.
        words(
            &cpu,
            0xc,
            &[
                0xe1a0400e, 0xe14f5000, 0xe3a02003, 0xe3822c03, 0xee062f91, 0xe25ef004,
            ],
        );
        mpu(&cpu, 0, 0, 0x3b, 0x303);
        mpu(&cpu, 5, 0x2000, 0x17, 3);
        system(&cpu, 1, 0, 0, Some(0x1005));
        set(&cpu, arm::CPSR, 0x53);
        set(&cpu, arm::PC, 0x2000);
        for _ in 0..100 {
            if matches!(
                resume(&cpu, quantum).unwrap(),
                Exit::Hint(Hint::WaitForInterrupt)
            ) {
                break;
            }
        }
        assert_eq!(get(&cpu, arm::CORE[0]), 42);
        assert_eq!(get(&cpu, arm::CORE[4]), 0x2004);
        assert_eq!(get(&cpu, arm::CORE[5]), 0x53);
        assert_eq!(get(&cpu, arm::PC), 0x2008);
        assert_eq!(system(&cpu, 5, 0, 1, None), 0xd);
        assert_eq!(system(&cpu, 6, 0, 2, None), 0x2000);
        assert_eq!(system(&cpu, 6, 2, 0, None), 5);
    }
}
#[test]
fn mpu_data_abort_reports_read_or_write_and_only_guest_code_skips_the_transfer() {
    for (instruction, status) in [(0xe5910000, 0xd), (0xe5810000, 0x80d)] {
        let cpu = processor(&Memory::default());
        cpu.map_zeroed(0, 0x4000, 7).unwrap();
        words(&cpu, 0x1000, &[instruction, 0xe320f003]);
        words(&cpu, 0x10, &[0xe3a00063, 0xe25ef004]);
        words(&cpu, 0x2000, &[0x12345678]);
        mpu(&cpu, 0, 0, 0x3b, 0x303);
        mpu(&cpu, 5, 0x2000, 0x17, 3);
        system(&cpu, 1, 0, 0, Some(0x1005));
        set(&cpu, arm::CPSR, 0x53);
        set(&cpu, arm::CORE[1], 0x2000);
        set(&cpu, arm::PC, 0x1000);
        for _ in 0..100 {
            if matches!(
                resume(&cpu, 17).unwrap(),
                Exit::Hint(Hint::WaitForInterrupt)
            ) {
                break;
            }
        }
        assert_eq!(get(&cpu, arm::CORE[0]), 99);
        assert_eq!(system(&cpu, 5, 0, 0, None), status);
        assert_eq!(system(&cpu, 6, 0, 0, None), 0x2000);
        assert_eq!(system(&cpu, 6, 1, 4, None), 3);
        let mut data = [0; 4];
        cpu.read_memory(0x2000, &mut data).unwrap();
        assert_eq!(u32::from_le_bytes(data), 0x12345678);
    }
}
#[test]
fn unexplained_permission_abort_preserves_registers_and_mpu_selector() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    mpu(&cpu, 0, 0, 0x3b, 0x303);
    mpu(&cpu, 5, 0x2000, 0x17, 3);
    // A higher matching region grants access; a lower denial cannot explain
    // an abort. The classification temporarily selects regions, then restores.
    mpu(&cpu, 7, 0x2000, 0x17, 0x303);
    system(&cpu, 1, 0, 0, Some(0x1005));
    set(&cpu, arm::CPSR, 0x53);
    set(&cpu, arm::PC, 0x2000);
    set(&cpu, arm::CORE[14], 0x1234);
    assert!(
        crate::rprofile::abort::enter(&cpu, arm::Exception::PrefetchAbort)
            .unwrap_err()
            .contains("actual MPU permission denial")
    );
    assert_eq!(system(&cpu, 6, 2, 0, None), 7);
    assert_eq!(get(&cpu, arm::PC), 0x2000);
    assert_eq!(get(&cpu, arm::CPSR), 0x53);
    assert_eq!(get(&cpu, arm::CORE[14]), 0x1234);
}
pub(super) fn processor_with_options(memory: &Memory, options: Options) -> Core {
    Core::new(
        crate::test_support::library()
            .open(Model::CortexR5F, 1024 * 1024)
            .unwrap(),
        options,
        Wiring {
            memory: memory.clone(),
        },
    )
    .unwrap()
}

#[test]
fn inconsistent_physical_ram_identity_refuses_before_installing_a_mapping() {
    let backing = Backing::aligned_zeros(0x1000, 4096).unwrap();
    let other = Backing::aligned_zeros(0x1000, 4096).unwrap();
    let memory = Memory::default();
    let a = processor(&memory);
    unsafe {
        a.map_physical(
            0x1000,
            0,
            0x1000,
            7,
            NonNull::new(backing.as_ptr()).unwrap(),
        )
        .unwrap();
    }
    let b = processor(&memory);
    assert!(
        unsafe { b.map_physical(0x1000, 0, 0x1000, 7, NonNull::new(other.as_ptr()).unwrap()) }
            .is_err()
    );
    assert!(b.memory_regions().unwrap().is_empty());
    let c = processor(&memory);
    assert!(
        unsafe {
            c.map_physical(
                0x1000,
                0x4000,
                0x1000,
                7,
                NonNull::new(backing.as_ptr()).unwrap(),
            )
        }
        .is_err()
    );
    assert!(c.memory_regions().unwrap().is_empty());
}

#[test]
fn mapping_replacement_uses_its_current_physical_identity_for_exclusives() {
    let memory = Memory::default();
    let first = Backing::aligned_zeros(0x1000, 4096).unwrap();
    let second = Backing::aligned_zeros(0x1000, 4096).unwrap();
    let cpu = processor(&memory);
    cpu.map_zeroed(0x9000, 0x1000, 7).unwrap();
    unsafe {
        cpu.map_physical(0x1000, 0, 0x1000, 7, NonNull::new(first.as_ptr()).unwrap())
            .unwrap();
        cpu.map_physical(0x5000, 0, 0x1000, 7, NonNull::new(first.as_ptr()).unwrap())
            .unwrap();
    }
    words(&cpu, 0x1000, &[5]);
    unsafe {
        cpu.replace_physical(
            0x1000,
            0x8000,
            0x1000,
            7,
            NonNull::new(second.as_ptr()).unwrap(),
        )
        .unwrap();
    }
    words(&cpu, 0x1000, &[8]);
    words(&cpu, 0x9000, &[0xe1910f9f, 0xe1813f92]); // ldrex; strex
    set(&cpu, arm::CORE[1], 0x1000);
    set(&cpu, arm::CORE[2], 9);
    run(&cpu, 0x9000, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[0]), 8);
    words(&cpu, 0x5000, &[6]); // The previous physical RAM no longer aliases this reservation.
    resume(&cpu, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 0);
    run(&cpu, 0x9000, 1).unwrap();
    unsafe {
        std::ptr::write_unaligned(second.as_ptr().cast::<u32>(), 9); // ABA DMA to the current RAM.
        cpu.backing_written(0x8000, NonNull::new(second.as_ptr()).unwrap(), 4)
            .unwrap();
    }
    resume(&cpu, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 1);
    for physical in [1, 1 << 32] {
        assert!(
            unsafe {
                cpu.map_physical(
                    0xd000,
                    physical,
                    0x1000,
                    7,
                    NonNull::new(second.as_ptr()).unwrap(),
                )
            }
            .is_err()
        );
    }
}

#[test]
fn replacing_executable_backing_flushes_the_replaced_window_and_preserves_other_aliases() {
    let first = Backing::aligned_zeros(0x1000, 4096).unwrap();
    let second = Backing::aligned_zeros(0x2000, 4096).unwrap();
    let cpu = processor(&Memory::default());
    unsafe {
        std::ptr::write_unaligned(second.as_ptr().cast::<u32>(), 0xe2844009);
        cpu.map_physical(0x1000, 0, 0x1000, 7, NonNull::new(first.as_ptr()).unwrap())
            .unwrap();
        cpu.map_physical(0x5000, 0, 0x1000, 7, NonNull::new(first.as_ptr()).unwrap())
            .unwrap();
    }
    words(&cpu, 0x1000, &[0xe2844001]);
    run(&cpu, 0x1000, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[4]), 1);
    unsafe {
        cpu.replace_physical(
            0x1000,
            0x8000,
            0x1000,
            7,
            NonNull::new(second.as_ptr()).unwrap(),
        )
        .unwrap();
    }
    run(&cpu, 0x1000, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[4]), 10);
    run(&cpu, 0x5000, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[4]), 11);
    for (address, physical, bytes, permissions) in [
        (0x9000, 0, 0x1000, 7),
        (0x1000, 0x8000, 0x2000, 7),
        (0x1000, 0x8000, 0x1000, 8),
    ] {
        assert!(
            unsafe {
                cpu.replace_physical(
                    address,
                    physical,
                    bytes,
                    permissions,
                    NonNull::new(second.as_ptr()).unwrap(),
                )
            }
            .is_err()
        );
    }
    run(&cpu, 0x1000, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[4]), 20);
}

#[test]
fn exclusive_monitors_follow_physical_aliases_peer_stores_dma_and_word_granules() {
    let backing = Backing::aligned_zeros(0x2000, 4096).unwrap();
    let memory = Memory::default();
    let a = processor(&memory);
    let b = processor(&memory);
    for (cpu, address) in [(&a, 0x1000), (&a, 0x5000), (&b, 0x9000)] {
        unsafe {
            cpu.map_physical(
                address,
                0,
                0x2000,
                7,
                NonNull::new(backing.as_ptr()).unwrap(),
            )
            .unwrap();
        }
    }
    words(&a, 0x1000, &[0xe1910f9f, 0xe1813f92, 0xe5812000]); // ldrex; strex; str
    words(&a, 0x1800, &[5]);
    set(&a, arm::CORE[1], 0x1800);
    set(&a, arm::CORE[2], 9);
    run(&a, 0x1000, 1).unwrap();
    let observed = a.fingerprint().unwrap();
    assert!(observed["reservation"].is_array());
    assert_eq!(a.fingerprint().unwrap(), observed);
    words(&a, 0x5804, &[99]); // adjacent word preserves this 4-byte reservation
    resume(&a, 1).unwrap();
    assert_eq!((get(&a, arm::CORE[0]), get(&a, arm::CORE[3])), (5, 0));
    run(&a, 0x1000, 1).unwrap();
    set(&b, arm::CORE[1], 0x9800);
    set(&b, arm::CORE[2], 9);
    run(&b, 0x9008, 1).unwrap(); // a peer ABA store through its alias
    resume(&a, 1).unwrap();
    assert_eq!(get(&a, arm::CORE[3]), 1);
    run(&a, 0x1000, 1).unwrap();
    unsafe {
        std::ptr::write_unaligned(backing.as_ptr().add(0x800).cast::<u32>(), 9);
    }
    b.memory_written(0x9800, 4).unwrap();
    resume(&a, 1).unwrap();
    assert_eq!(get(&a, arm::CORE[3]), 1);
    set(&a, arm::CORE[1], 0x1801);
    assert!(
        run(&a, 0x1000, 1)
            .unwrap_err()
            .to_string()
            .contains("unaligned")
    );
}

#[test]
fn exclusive_code_store_flushes_aliases_once_and_thumb_and_irq_clear_the_monitor() {
    let backing = Backing::aligned_zeros(0x2000, 4096).unwrap();
    let cpu = processor(&Memory::default());
    for address in [0, 0x4000] {
        unsafe {
            cpu.map_physical(
                address,
                0,
                0x2000,
                7,
                NonNull::new(backing.as_ptr()).unwrap(),
            )
            .unwrap();
        }
    }
    words(&cpu, 0x1800, &[0xe2844001]);
    run(&cpu, 0x1800, 1).unwrap();
    words(&cpu, 0x1000, &[0xe1910f9f, 0xe1813f92, 0xe12fff11]);
    set(&cpu, arm::CORE[1], 0x5800);
    set(&cpu, arm::CORE[2], 0xe2844009);
    run(&cpu, 0x1000, 4).unwrap();
    assert_eq!(
        (
            cpu.instruction_progress(),
            get(&cpu, arm::CORE[3]),
            get(&cpu, arm::CORE[4])
        ),
        (Some(4), 0, 10)
    );
    words(&cpu, 0x1000, &[0xe1910f9f, 0xf57ff01f, 0xe1813f92]);
    run(&cpu, 0x1000, 3).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 1);
    cpu.write_memory(0x1000, &[0x51, 0xe8, 0x00, 0x0f, 0x41, 0xe8, 0x00, 0x23])
        .unwrap();
    run(&cpu, 0x1001, 2).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 0);
    words(&cpu, 0x1000, &[0xe1910f9f, 0xe1813f92]);
    words(&cpu, 0x18, &[0xe28aa001, 0xe25ef004]);
    set(&cpu, arm::CPSR, 0x53);
    run(&cpu, 0x1000, 1).unwrap();
    cpu.set_interrupt(Interrupt::Irq, true).unwrap();
    resume(&cpu, 2).unwrap();
    cpu.set_interrupt(Interrupt::Irq, false).unwrap();
    resume(&cpu, 1).unwrap();
    assert_eq!((get(&cpu, arm::CORE[10]), get(&cpu, arm::CORE[3])), (1, 1));
}
pub(super) fn get(cpu: &Core, register: Register) -> u32 {
    cpu.execution.scalar(register).unwrap()
}
pub(super) fn set(cpu: &Core, register: Register, value: u32) {
    cpu.write_register(register, &value.to_le_bytes()).unwrap();
}
pub(super) fn words(cpu: &Core, address: u64, values: &[u32]) {
    cpu.write_memory(
        address,
        &values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap();
}
pub(super) fn run(cpu: &Core, begin: u64, n: usize) -> Result<Exit, ExecutionError> {
    cpu.run(Run {
        begin,
        until: None,
        watchdog_us: 0,
        instructions: Some(n),
    })
}
pub(super) fn resume(cpu: &Core, n: usize) -> Result<Exit, ExecutionError> {
    run(
        cpu,
        get(cpu, arm::PC) as u64 | u64::from(get(cpu, arm::CPSR) & 32 != 0),
        n,
    )
}

fn counter_register(crm: u32, opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn: 9,
        crm,
        opc2,
        secure: 0,
        wide: false,
    }
}
#[test]
fn guest_pmu_counts_fractional_time_across_host_pauses_and_idle_inputs() {
    let simulate = |pause| {
        let cpu = processor(&Memory::default());
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        cpu.set_guest_clock(Clock {
            now: 100,
            phase: 7,
            ratio: fujimod_emulator_cpu::timeline::Ratio::new(24, 1).unwrap(),
        })
        .unwrap();
        let mut code = vec![0xee090f1c, 0xee091f3c]; // mcr pmcr,r0; mcr pmcntenset,r1
        code.extend([0xe320f000; 24]);
        code.push(0xee192f1d); // mrc pmccntr,r2
        words(&cpu, 0x1000, &code);
        set(&cpu, arm::CORE[0], 1);
        set(&cpu, arm::CORE[1], 0x80000000);
        if pause {
            run(&cpu, 0x1000, 10).unwrap();
            resume(&cpu, 17).unwrap();
        } else {
            run(&cpu, 0x1000, 27).unwrap();
        }
        assert_eq!(get(&cpu, arm::CORE[2]), 1);
        let control = cpu.coprocessor(counter_register(12, 0), None).unwrap();
        let count = cpu.coprocessor(counter_register(13, 0), None).unwrap();
        assert_eq!(
            cpu.coprocessor(counter_register(13, 0), None).unwrap(),
            count
        );
        cpu.set_guest_clock(Clock {
            now: 1000,
            phase: 0,
            ratio: fujimod_emulator_cpu::timeline::Ratio::new(24, 1).unwrap(),
        })
        .unwrap();
        assert_eq!(cpu.coprocessor(counter_register(13, 0), None).unwrap(), 900);
        assert!(
            cpu.set_guest_clock(Clock {
                now: 999,
                phase: 0,
                ratio: fujimod_emulator_cpu::timeline::Ratio::new(24, 1).unwrap()
            })
            .is_err()
        );
        (get(&cpu, arm::PC), control, count)
    };
    assert_eq!(simulate(false), simulate(true));
}

#[test]
fn exact_block_budgets_ring_wrap_pause_and_thumb_resume() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    let mut code = vec![0xe2800001; 32];
    code.push(0xeaffffde); // b 0x1000
    words(&cpu, 0x1000, &code);
    for cap in [1, 31, 32, 33, 34, 2113, 10037] {
        set(&cpu, arm::CORE[0], 0);
        run(&cpu, 0x1000, cap).unwrap();
        assert_eq!(cpu.instruction_progress(), Some(cap as u64));
        assert_eq!(get(&cpu, arm::CORE[0]), (cap - cap / 33) as u32);
        let pc = get(&cpu, arm::PC);
        run(&cpu, 0x1800, 0).unwrap();
        assert_eq!(get(&cpu, arm::PC), pc);
        assert_eq!(cpu.instruction_progress(), Some(0));
    }
    assert!(cpu.execution.accounting().counters().1 > 64);
    set(&cpu, arm::CORE[0], 0);
    run(&cpu, 0x1000, 34).unwrap();
    resume(&cpu, 10003).unwrap();
    assert_eq!(get(&cpu, arm::CORE[0]), (10037 - 10037 / 33) as u32);
    cpu.write_memory(0x1800, &[0x01, 0x30, 0xfd, 0xe7]).unwrap(); // adds r0,#1; b 0x1800
    set(&cpu, arm::CORE[0], 0);
    run(&cpu, 0x1801, 101).unwrap();
    assert_eq!(
        (cpu.instruction_progress(), get(&cpu, arm::CORE[0])),
        (Some(101), 51)
    );
    resume(&cpu, 3).unwrap();
    assert_eq!(
        (cpu.instruction_progress(), get(&cpu, arm::CORE[0])),
        (Some(3), 52)
    );
}

#[test]
fn interactive_policy_runs_full_blocks_and_rebases_clock_after_bounded_overshoot() {
    let setup = |memory: &Memory| {
        let cpu = processor(memory);
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        words(
            &cpu,
            0x1000,
            &[0xe3a00001, 0xe2811001, 0xe2822001, 0xe320f003],
        );
        cpu
    };

    let exact = setup(&Memory::default());
    run(&exact, 0x1000, 1).unwrap();
    assert_eq!(
        (
            exact.instruction_progress(),
            get(&exact, arm::PC),
            get(&exact, arm::CORE[0]),
        ),
        (Some(1), 0x1004, 1)
    );
    assert!(exact.execution.accounting().counters().1 > 0);

    let interactive = setup(&Memory::default());
    interactive
        .set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    let before_snapshot = interactive.fingerprint().unwrap();
    let snapshot = interactive.snapshot(None).unwrap();
    interactive
        .set_execution_policy(ExecutionPolicy::Exact)
        .unwrap();
    interactive.restore(snapshot.as_ref()).unwrap();
    assert_eq!(interactive.fingerprint().unwrap(), before_snapshot);
    assert_eq!(
        interactive.execution.policy.get(),
        ExecutionPolicy::Interactive
    );
    interactive
        .set_guest_clock(Clock {
            now: 20,
            phase: 0,
            ratio: fujimod_emulator_cpu::timeline::Ratio::new(1, 1).unwrap(),
        })
        .unwrap();

    run(&interactive, 0x1000, 1).unwrap();
    let progress = interactive.instruction_progress().unwrap();
    assert!((3..=4).contains(&progress));
    assert_eq!(get(&interactive, arm::PC), 0x1000 + 4 * progress as u32);
    assert_eq!(
        (
            get(&interactive, arm::CORE[0]),
            get(&interactive, arm::CORE[1]),
            get(&interactive, arm::CORE[2]),
        ),
        (1, 1, 1)
    );
    assert_eq!(interactive.execution.accounting().counters().1, 0);

    // The board may clamp time to its deadline while the CPU has advanced by
    // the final block. That bounded block-time discrepancy is rebased once.
    interactive
        .set_guest_clock(Clock {
            now: 21,
            phase: 0,
            ratio: fujimod_emulator_cpu::timeline::Ratio::new(1, 1).unwrap(),
        })
        .unwrap();
    assert!(
        interactive
            .set_guest_clock(Clock {
                now: 20,
                phase: 0,
                ratio: fujimod_emulator_cpu::timeline::Ratio::new(1, 1).unwrap(),
            })
            .is_err()
    );

    let hint = processor(&Memory::default());
    hint.map_zeroed(0x1000, 0x1000, 7).unwrap();
    words(&hint, 0x1000, &[0xe320f003]);
    hint.set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    assert!(matches!(
        run(&hint, 0x1000, 1).unwrap(),
        Exit::Hint(Hint::WaitForInterrupt)
    ));
    assert_eq!(
        (hint.instruction_progress(), get(&hint, arm::PC)),
        (Some(1), 0x1004)
    );
}

struct DeviceAccess {
    cpu: *const Core,
    reads: Cell<u32>,
    observed: Cell<u64>,
    change_code: bool,
    stop: bool,
}
unsafe fn device_read(_: u64, size: u32, data: *mut c_void) -> u64 {
    let d = unsafe { &*data.cast::<DeviceAccess>() };
    let cpu = unsafe { &*d.cpu };
    assert_eq!(size, 4);
    d.reads.set(d.reads.get() + 1);
    let progress = cpu.instruction_progress().unwrap();
    assert_eq!(cpu.instruction_progress(), Some(progress));
    d.observed.set(progress);
    if d.change_code {
        words(cpu, 0x1000, &[0xe2833009]);
    } else {
        cpu.tighten_instruction_limit(progress + 3).unwrap();
    }
    if d.stop {
        cpu.stop().unwrap();
    }
    44
}
unsafe fn device_write(_: u64, _: u32, _: u64, _: *mut c_void) {
    panic!("unexpected write");
}
fn map_device(cpu: &Core, d: &mut DeviceAccess) {
    unsafe {
        cpu.map_mmio(
            0x8000,
            0x1000,
            Mmio {
                read: device_read,
                write: device_write,
                data: (d as *mut DeviceAccess).cast(),
            },
        )
        .unwrap();
    }
}

#[test]
fn mmio_is_last_and_tightens_the_running_budget_without_repeating_access() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    let mut d = DeviceAccess {
        cpu: &cpu,
        reads: Cell::new(0),
        observed: Cell::new(0),
        change_code: false,
        stop: false,
    };
    map_device(&cpu, &mut d);
    let mut code = vec![0xe2822001; 10];
    code.push(0xe5910000); // ldr r0,[r1]
    code.extend([0xe2822001; 10]);
    words(&cpu, 0x1000, &code);
    set(&cpu, arm::CORE[1], 0x8000);
    run(&cpu, 0x1000, 500).unwrap();
    assert_eq!((d.reads.get(), d.observed.get()), (1, 11));
    assert_eq!(
        (
            cpu.instruction_progress(),
            get(&cpu, arm::CORE[2]),
            get(&cpu, arm::CORE[0])
        ),
        (Some(14), 13, 44)
    );
    assert_eq!(get(&cpu, arm::PC), 0x1038);
}

#[test]
fn a_dma_code_write_completes_its_mmio_once_then_flushes_before_the_next_instruction() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0x1000, 0x2000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe2833001]);
    run(&cpu, 0x1000, 1).unwrap();
    let mut d = DeviceAccess {
        cpu: &cpu,
        reads: Cell::new(0),
        observed: Cell::new(0),
        change_code: true,
        stop: false,
    };
    map_device(&cpu, &mut d);
    words(&cpu, 0x2000, &[0xe5910000, 0xe12fff12]); // ldr r0,[r1]; bx r2
    set(&cpu, arm::CORE[1], 0x8000);
    set(&cpu, arm::CORE[2], 0x1000);
    run(&cpu, 0x2000, 3).unwrap();
    assert_eq!(
        (d.reads.get(), d.observed.get(), cpu.instruction_progress()),
        (1, 1, Some(3))
    );
    assert_eq!((get(&cpu, arm::CORE[0]), get(&cpu, arm::CORE[3])), (44, 10));
    // A user/device refusal must win over the internal invalidation resume.
    d.stop = true;
    run(&cpu, 0x2000, 100).unwrap();
    assert_eq!(
        (
            d.reads.get(),
            cpu.instruction_progress(),
            get(&cpu, arm::CORE[3])
        ),
        (2, Some(1), 10)
    );
}

struct Observations {
    cpu: *const Core,
    block: Cell<u64>,
    instruction: Cell<u64>,
}
unsafe fn observe_block(_: Block, data: *mut c_void) {
    let p = unsafe { &*data.cast::<Observations>() };
    let cpu = unsafe { &*p.cpu };
    if p.block.get() == 0 {
        assert_eq!(cpu.instruction_progress(), Some(0));
        cpu.tighten_instruction_limit(3).unwrap();
    }
    p.block.set(p.block.get() + 1);
}
unsafe fn observe_instruction(_: u64, _: u32, data: *mut c_void) {
    let p = unsafe { &*data.cast::<Observations>() };
    let cpu = unsafe { &*p.cpu };
    p.instruction.set(p.instruction.get() + 1);
    assert_eq!(cpu.instruction_progress(), Some(p.instruction.get()));
}
#[test]
fn optional_observers_get_exact_progress_and_do_not_replace_the_owned_accelerator() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe2800001; 16]);
    let mut p = Observations {
        cpu: &cpu,
        block: Cell::new(0),
        instruction: Cell::new(0),
    };
    let data = (&mut p as *mut Observations).cast();
    let block = unsafe {
        cpu.install_hook(Hook::Block {
            callback: observe_block,
            data,
            range: HookRange::All,
        })
        .unwrap()
    };
    run(&cpu, 0x1000, 100).unwrap();
    assert_eq!(
        (cpu.instruction_progress(), get(&cpu, arm::CORE[0])),
        (Some(3), 3)
    );
    cpu.remove_hook(block).unwrap();
    let code = unsafe {
        cpu.install_hook(Hook::Instruction {
            callback: observe_instruction,
            data,
            range: HookRange::All,
        })
        .unwrap()
    };
    run(&cpu, 0x1000, 11).unwrap();
    assert_eq!(p.instruction.get(), 11);
    cpu.remove_hook(code).unwrap();
    assert!(cpu.accounting().is_none());
    assert!(!cpu.features().translation_filter);
    let result = cpu
        .run(Run {
            begin: 0x1000,
            until: Some(0x100c),
            instructions: None,
            watchdog_us: 0,
        })
        .unwrap();
    assert!(matches!(result, Exit::Stopped));
    assert_eq!(cpu.instruction_progress(), Some(3));
}

#[test]
fn guest_code_store_through_an_alias_restarts_only_the_unentered_store() {
    let backing = Backing::aligned_zeros(0x2000, 4096).unwrap();
    let cpu = processor(&Memory::default());
    unsafe {
        cpu.map_backing(0x1000, 0x2000, 7, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
        cpu.map_backing(0x5000, 0x1000, 7, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
    }
    words(&cpu, 0x1000, &[0xe2800001]);
    run(&cpu, 0x1000, 1).unwrap();
    words(&cpu, 0x2000, &[0xe5812000, 0xe12fff13]); // str r2,[r1]; bx r3
    set(&cpu, arm::CORE[1], 0x5000);
    set(&cpu, arm::CORE[2], 0xe2800009);
    set(&cpu, arm::CORE[3], 0x1000);
    run(&cpu, 0x2000, 3).unwrap();
    assert_eq!(
        (
            cpu.instruction_progress(),
            get(&cpu, arm::CORE[0]),
            get(&cpu, arm::PC)
        ),
        (Some(3), 10, 0x1004)
    );
}

#[test]
fn exact_serial_exclusives_keep_shared_ram_effects_without_sync_callbacks() {
    unsafe fn synchronized(data: *mut c_void) {
        let calls = unsafe { &*data.cast::<Cell<u32>>() };
        calls.set(calls.get() + 1);
    }

    let backing = Backing::aligned_zeros(0x2000, 4096).unwrap();
    let memory = Memory::default();
    let a = processor(&memory);
    let b = processor(&memory);
    for (cpu, address) in [(&a, 0x1000), (&b, 0x9000)] {
        unsafe {
            cpu.map_physical(
                address,
                0,
                0x2000,
                7,
                NonNull::new(backing.as_ptr()).unwrap(),
            )
            .unwrap();
        }
    }
    words(&a, 0x1000, &[0xe1910f9f, 0xe1813f92]); // ldrex r0,[r1]; strex r3,r2,[r1]
    words(&b, 0x9000, &[0xe5812000]); // str r2,[r1]
    a.write_memory(0x1800, &0x7654_3210u32.to_le_bytes())
        .unwrap();
    set(&a, arm::CORE[1], 0x1800);
    set(&a, arm::CORE[2], 7);
    set(&b, arm::CORE[1], 0x9800);
    set(&b, arm::CORE[2], 0x3333);
    let sync_calls = Cell::<u32>::new(0);
    let hook = unsafe {
        a.install_hook(Hook::Synchronization {
            callback: synchronized,
            data: (&sync_calls as *const Cell<u32>).cast_mut().cast(),
        })
    }
    .unwrap();

    run(&a, 0x1000, 1).unwrap();
    run(&b, 0x9000, 1).unwrap();
    resume(&a, 1).unwrap();

    let mut bytes = [0; 4];
    a.read_memory(0x1800, &mut bytes).unwrap();
    assert_eq!(u32::from_le_bytes(bytes), 0x3333);
    assert_eq!(get(&a, arm::CORE[3]), 1);
    assert_eq!(sync_calls.get(), 0);
    a.remove_hook(hook).unwrap();
}

struct SynchronizationProgress {
    cpu: *const Core,
    observed: RefCell<Vec<u64>>,
}
unsafe fn observe_synchronization_progress(data: *mut c_void) {
    let p = unsafe { &*data.cast::<SynchronizationProgress>() };
    p.observed
        .borrow_mut()
        .push(unsafe { &*p.cpu }.instruction_progress().unwrap());
}
#[test]
fn interactive_code_write_restart_does_not_retract_published_coarse_progress() {
    let backing = Backing::aligned_zeros(0x3000, 4096).unwrap();
    let cpu = processor(&Memory::default());
    unsafe {
        cpu.map_backing(0x1000, 0x3000, 7, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
        cpu.map_backing(0x5000, 0x1000, 7, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
    }
    cpu.set_execution_policy(ExecutionPolicy::Interactive)
        .unwrap();
    words(&cpu, 0x1000, &[0xe2800001]);
    run(&cpu, 0x1000, 1).unwrap();

    let mut code = vec![0xe2844001; 8]; // eight independent ADD entries
    code.extend([0xe5812000, 0xe12fff13]); // str r2,[r1]; bx r3
    words(&cpu, 0x2000, &code);
    set(&cpu, arm::CORE[1], 0x5000);
    set(&cpu, arm::CORE[2], 0xe320f003); // replace the watched entry with WFI
    set(&cpu, arm::CORE[3], 0x1000);
    let mut progress = SynchronizationProgress {
        cpu: &cpu,
        observed: RefCell::new(Vec::new()),
    };
    let hook = unsafe {
        cpu.install_hook(Hook::Synchronization {
            callback: observe_synchronization_progress,
            data: (&mut progress as *mut SynchronizationProgress).cast(),
        })
    }
    .unwrap();

    assert!(matches!(
        run(&cpu, 0x2000, 100).unwrap(),
        Exit::Hint(Hint::WaitForInterrupt)
    ));
    let observed = progress.observed.borrow();
    assert_eq!(observed[0], 10);
    assert!(observed.windows(2).all(|pair| pair[1] >= pair[0]));
    assert!(cpu.instruction_progress().unwrap() >= *observed.last().unwrap());
    assert_eq!(get(&cpu, arm::CORE[4]), 8);
    assert_eq!(get(&cpu, arm::PC), 0x1004);
    cpu.remove_hook(hook).unwrap();
}

#[test]
fn code_writes_invalidate_every_peer_and_new_alias_of_the_same_backing() {
    let backing = Backing::aligned_zeros(0x1000, 4096).unwrap();
    let memory = Memory::default();
    let a = processor(&memory);
    let b = processor(&memory);
    for (cpu, address) in [(&a, 0x1000), (&b, 0x9000)] {
        unsafe {
            cpu.map_backing(address, 0x1000, 7, NonNull::new(backing.as_ptr()).unwrap())
                .unwrap();
        }
    }
    words(&a, 0x1000, &[0xe2800001]);
    run(&a, 0x1000, 1).unwrap();
    run(&b, 0x9000, 1).unwrap();
    unsafe {
        a.map_backing(0x5000, 0x1000, 7, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
    }
    words(&a, 0x5000, &[0xe2800009]);
    run(&a, 0x1000, 1).unwrap();
    run(&b, 0x9000, 1).unwrap();
    assert_eq!((get(&a, arm::CORE[0]), get(&b, arm::CORE[0])), (10, 10));
}

#[test]
fn code_ranges_can_cross_adjacent_mappings_with_distinct_backing() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    cpu.map_zeroed(0x2000, 0x1000, 7).unwrap();
    words(&cpu, 0x1ffc, &[0xe2811001, 0xe2822001]);
    run(&cpu, 0x1ffc, 2).unwrap();
    words(&cpu, 0x1ffc, &[0xe2811009, 0xe282200b]);
    run(&cpu, 0x1ffc, 2).unwrap();
    assert_eq!((get(&cpu, arm::CORE[1]), get(&cpu, arm::CORE[2])), (10, 12));
    cpu.write_memory(0x1ffe, &[0xaf, 0xf3, 0x03, 0x80]).unwrap(); // wfi.w across the boundary
    assert!(matches!(
        run(&cpu, 0x1fff, 100).unwrap(),
        Exit::Hint(Hint::WaitForInterrupt)
    ));
    assert_eq!(
        (cpu.instruction_progress(), get(&cpu, arm::PC)),
        (Some(1), 0x2002)
    );
}

#[test]
fn code_watch_marks_exact_bytes_across_words_pages_and_aliases() {
    let backing = Backing::aligned_zeros(0x2000, 4096).unwrap();
    let memory = Memory::default();
    let cpu = processor(&memory);
    unsafe {
        cpu.map_backing(0x1000, 0x2000, 7, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
        cpu.map_backing(0x9000, 0x2000, 7, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
    }
    let core = cpu.execution.id.get();

    // The first range crosses bitmap words; the second crosses backing pages.
    assert!(memory.note_code(core, 0x103f, 2).unwrap());
    assert!(memory.note_code(core, 0x1fff, 2).unwrap());
    memory.apply_watches().unwrap();

    // Reclassification preserves the recorded bytes without re-adding hooks.
    assert!(!memory.note_code(core, 0x103f, 2).unwrap());
    assert!(!memory.written(core, 0x103e, 1).unwrap());
    assert!(memory.written(core, 0x103f, 1).unwrap());
    memory.flush().unwrap();

    // The other byte in the cross-word range is still watched after translation.
    assert!(!memory.note_code(core, 0x103f, 2).unwrap());
    assert!(memory.written(core, 0x1040, 1).unwrap());
    memory.flush().unwrap();

    // Both sides of the cross-page range remain watched through the alias.
    assert!(!memory.note_code(core, 0x1fff, 2).unwrap());
    assert!(memory.written(core, 0x9fff, 1).unwrap());
    memory.flush().unwrap();
    assert!(!memory.note_code(core, 0x1fff, 2).unwrap());
    assert!(memory.written(core, 0xa000, 1).unwrap());
    memory.flush().unwrap();
}

#[test]
fn pending_irq_stops_after_unmask_then_enters_without_an_instruction_charge() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0, 0x2000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe3a00001, 0xf1080080, 0xe2800001]); // mov r0,#1; cpsie i; add r0,#1
    words(&cpu, 0x18, &[0xe28aa001, 0xe25ef004]); // add r10,#1; subs pc,lr,#4
    set(&cpu, arm::CPSR, 0xd3);
    cpu.set_interrupt(Interrupt::Irq, true).unwrap();
    run(&cpu, 0x1000, 100).unwrap();
    assert_eq!(
        (
            cpu.instruction_progress(),
            get(&cpu, arm::PC),
            get(&cpu, arm::CORE[0])
        ),
        (Some(2), 0x1008, 1)
    );
    resume(&cpu, 2).unwrap();
    assert_eq!(
        (
            cpu.instruction_progress(),
            get(&cpu, arm::PC),
            get(&cpu, arm::CORE[10])
        ),
        (Some(2), 0x1008, 1)
    );
    cpu.set_interrupt(Interrupt::Irq, false).unwrap();
    resume(&cpu, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[0]), 2);
}

#[test]
fn hints_and_faults_have_entered_counts_and_unsupported_mechanisms_refuse() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe2800001, 0xe320f003, 0xe2800001]);
    assert!(matches!(
        run(&cpu, 0x1000, 100).unwrap(),
        Exit::Hint(Hint::WaitForInterrupt)
    ));
    assert_eq!(
        (cpu.instruction_progress(), get(&cpu, arm::PC)),
        (Some(2), 0x1008)
    );
    words(&cpu, 0x1000, &[0xe2800001, 0xe5910000]);
    set(&cpu, arm::CORE[1], 0x8000);
    assert!(matches!(
        run(&cpu, 0x1000, 100),
        Err(ExecutionError::Memory {
            fault: Fault::ReadUnmapped,
            ..
        })
    ));
    assert_eq!(cpu.instruction_progress(), Some(2));
    words(&cpu, 0x1000, &[0xe1d00f9f]); // LDREXB
    assert!(
        run(&cpu, 0x1000, 1)
            .unwrap_err()
            .to_string()
            .contains("not implemented")
    );
    assert_eq!(cpu.instruction_progress(), Some(1));
    words(&cpu, 0x1000, &[0xee190fbc]); // MRC PMSELR, an unmodeled PMU selector
    assert!(
        run(&cpu, 0x1000, 1)
            .unwrap_err()
            .to_string()
            .contains("unmodeled")
    );
    words(&cpu, 0x1000, &[0xee120e10]);
    run(&cpu, 0x1000, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[0]), 0x55aa);
    words(&cpu, 0x1000, &[0xfe120e10]); // MRC2 is outside the selected CP14 substitution.
    assert!(
        run(&cpu, 0x1000, 1)
            .unwrap_err()
            .to_string()
            .contains("unmodeled")
    );
    words(&cpu, 0x1000, &[0xf320f003]); // This unconditional encoding is not ARM WFI.
    assert!(run(&cpu, 0x1000, 1).is_err());
    for (hint, halfwords) in [
        (Hint::WaitForEvent, [0xf3af_u16, 0x8002]),
        (Hint::WaitForInterrupt, [0xf3af, 0x8003]),
        (Hint::SendEvent, [0xf3af, 0x8004]),
    ] {
        cpu.write_memory(
            0x1000,
            &halfwords
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(matches!(run(&cpu, 0x1001, 100).unwrap(), Exit::Hint(found) if hint == found));
        assert_eq!(
            (cpu.instruction_progress(), get(&cpu, arm::PC)),
            (Some(1), 0x1004)
        );
    }
    cpu.write_memory(0x1000, &[0xd1, 0xe8, 0x00, 0xf0]).unwrap(); // tbb [r1,r0]
    cpu.write_memory(0x1800, &[4]).unwrap();
    set(&cpu, arm::CORE[0], 0);
    set(&cpu, arm::CORE[1], 0x1800);
    run(&cpu, 0x1001, 1).unwrap();
    assert_eq!(get(&cpu, arm::PC), 0x100c);
    for halfwords in [[0xe8d0, 0x127f], [0xe8d0, 0x1f4f], [0xe8c0, 0x1f42]] {
        cpu.write_memory(
            0x1000,
            &halfwords
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(
            run(&cpu, 0x1001, 1)
                .unwrap_err()
                .to_string()
                .contains("not implemented")
        );
    }
}

unsafe fn sparse_entry(_: u64, _: u32, data: *mut c_void) {
    let calls = unsafe { &*data.cast::<Cell<u64>>() };
    calls.set(calls.get() + 1);
}
#[test]
fn a_narrow_pc_observer_keeps_other_instructions_on_the_block_accelerator() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe2800001; 16]);
    let calls = Cell::new(0u64);
    let hook = unsafe {
        cpu.install_hook(Hook::Instruction {
            callback: sparse_entry,
            data: (&calls as *const Cell<u64>).cast_mut().cast(),
            range: HookRange::Inclusive {
                begin: 0x1004,
                end: 0x1004,
            },
        })
        .unwrap()
    };
    assert!(matches!(
        cpu.execution
            .classify(0x1000, 0xe2800001)
            .unwrap()
            .instruction,
        InstructionDisposition::Execute
    ));
    assert!(matches!(
        cpu.execution
            .classify(0x1004, 0xe2800001)
            .unwrap()
            .instruction,
        InstructionDisposition::ObserveAndEndBlock
    ));
    run(&cpu, 0x1000, 7).unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(cpu.instruction_progress(), Some(7));
    assert_eq!(get(&cpu, arm::CORE[0]), 7);
    cpu.remove_hook(hook).unwrap();
}

#[test]
fn fingerprint_reads_detached_banks_without_changing_live_context() {
    let cpu = processor(&Memory::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe3a0002a, 0xe320f003]);
    set(&cpu, arm::CPSR, 0x53);
    set(&cpu, arm::CORE[13], 0x3000);
    mpu(&cpu, 3, 0x2000, 0x17, 0x303);
    assert!(matches!(
        run(&cpu, 0x1000, 10).unwrap(),
        Exit::Hint(Hint::WaitForInterrupt)
    ));
    let before = cpu.fingerprint().unwrap();
    assert_eq!(get(&cpu, arm::CPSR), 0x53);
    assert_eq!(get(&cpu, arm::CORE[13]), 0x3000);
    assert_eq!(system(&cpu, 6, 2, 0, None), 3);
    assert_eq!(cpu.fingerprint().unwrap(), before);
    set(&cpu, arm::CORE[0], 43);
    assert_ne!(cpu.fingerprint().unwrap(), before);
}
