use super::*;

fn core() -> Core {
    let cpu = crate::test_support::library()
        .open(Model::ArmV8A, 1 << 20)
        .unwrap();
    cpu.map_zeroed(0x1000, 0x3000, 7).unwrap();
    cpu.write_register(Register::Pstate, &0x3cdu64.to_le_bytes())
        .unwrap();
    cpu
}
fn value(cpu: &Core, register: Register) -> u64 {
    let mut bytes = [0; 8];
    cpu.read_register(register, &mut bytes).unwrap();
    u64::from_le_bytes(bytes)
}
fn words(cpu: &Core, address: u64, code: &[u32]) {
    cpu.write_memory(
        address,
        &code
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap();
}
fn run(cpu: &Core, begin: u64, until: Option<u64>, instructions: Option<usize>) {
    cpu.run(Run {
        begin,
        until,
        instructions,
        watchdog_us: 1_000_000,
    })
    .unwrap();
}
unsafe fn observed(pc: u64, _: u32, data: *mut c_void) {
    unsafe { &mut *data.cast::<Vec<u64>>() }.push(pc);
}

#[test]
fn native_budget_stops_at_every_offset_and_preserves_instruction_observers() {
    for observed_entries in [false, true] {
        let cpu = core();
        // Four ADDs then a backward branch: exercise both block interiors and ends.
        words(
            &cpu,
            0x1000,
            &[0x91000400, 0x91000400, 0x91000400, 0x91000400, 0x17fffffc],
        );
        let mut entries = Vec::<u64>::new();
        if observed_entries {
            unsafe {
                cpu.install_hook(Hook::Instruction {
                    range: HookRange::All,
                    callback: observed,
                    data: (&mut entries as *mut Vec<u64>).cast(),
                })
                .unwrap();
            }
        }
        for budget in 0..37 {
            entries.clear();
            cpu.write_register(Register::Pc, &0x1000u64.to_le_bytes())
                .unwrap();
            cpu.write_register(Register::X(0), &0u64.to_le_bytes())
                .unwrap();
            run(&cpu, 0x1000, None, Some(budget));
            assert_eq!(
                value(&cpu, Register::Pc),
                0x1000 + (budget % 5) as u64 * 4,
                "budget {budget}"
            );
            assert_eq!(value(&cpu, Register::X(0)), (budget - budget / 5) as u64);
            if observed_entries {
                assert_eq!(entries.len(), budget);
                for (n, pc) in entries.iter().enumerate() {
                    assert_eq!(*pc, 0x1000 + (n % 5) as u64 * 4);
                }
            }
        }
        // Removing and reinstalling the counter must invalidate cached hook code.
        run(&cpu, 0x1000, Some(0x100c), None);
        run(&cpu, 0x1000, None, Some(2));
        assert_eq!(value(&cpu, Register::Pc), 0x1008);
    }
}

struct Restart<'a> {
    cpu: &'a Core,
    writes: usize,
}
unsafe fn restart_store(_: Access, _: u64, _: i32, _: i64, data: *mut c_void) {
    let data = unsafe { &mut *data.cast::<Restart<'_>>() };
    data.writes += 1;
    if data.writes == 1 {
        data.cpu.request_restart().unwrap();
    }
}
#[test]
fn memory_hook_restart_refunds_the_unentered_block_tail() {
    let cpu = core();
    words(
        &cpu,
        0x1000,
        &[0x91000400, 0xb9000022, 0x91000400, 0x91000400],
    );
    cpu.write_register(Register::X(1), &0x2000u64.to_le_bytes())
        .unwrap();
    cpu.write_register(Register::X(2), &123u64.to_le_bytes())
        .unwrap();
    let mut data = Restart {
        cpu: &cpu,
        writes: 0,
    };
    unsafe {
        cpu.install_hook(Hook::Memory {
            range: HookRange::All,
            read: false,
            write: true,
            callback: restart_store,
            data: (&mut data as *mut Restart<'_>).cast(),
        })
        .unwrap();
    }
    run(&cpu, 0x1000, None, Some(3));
    assert_eq!(data.writes, 2);
    assert_eq!(value(&cpu, Register::Pc), 0x100c);
    assert_eq!(value(&cpu, Register::X(0)), 2);
    let mut bytes = [0; 4];
    cpu.read_memory(0x2000, &mut bytes).unwrap();
    assert_eq!(u32::from_le_bytes(bytes), 123);
}

struct Budget<'a> {
    cpu: &'a Core,
    callbacks: usize,
}
unsafe fn classify_execute(_: *mut c_void, _: u64, _: u32) -> Translation {
    Translation {
        instruction: InstructionDisposition::Execute,
        observe_block: false,
    }
}
unsafe fn restarted(_: *mut c_void, _: u64) {}
unsafe fn exhausted(block: Block, data: *mut c_void) {
    let data = unsafe { &mut *data.cast::<Budget<'_>>() };
    data.callbacks += 1;
    let remaining = unsafe { data.cpu.accounting().unwrap().as_ref().budget };
    if remaining == 0 {
        data.cpu.stop().unwrap();
    } else {
        assert!(remaining > 0 && remaining < i64::from(block.bytes / 4));
        data.cpu.retry_block(remaining as u32).unwrap();
    }
}
#[test]
fn translation_filter_and_embedder_accounting_debit_exact_a64_block_budgets() {
    let cpu = core();
    assert!(cpu.features().translation_filter && cpu.features().block_accounting);
    words(
        &cpu,
        0x1000,
        &[0x91000400, 0x91000400, 0x91000400, 0x91000400, 0x17fffffc],
    );
    let mut data = Budget {
        cpu: &cpu,
        callbacks: 0,
    };
    let mut unexpected_entries = Vec::<u64>::new();
    unsafe {
        cpu.translation_hooks(Some(TranslationHooks {
            classify: classify_execute,
            restart: restarted,
            data: std::ptr::null_mut(),
        }))
        .unwrap();
        cpu.install_hook(Hook::Instruction {
            range: HookRange::All,
            callback: observed,
            data: (&mut unexpected_entries as *mut Vec<u64>).cast(),
        })
        .unwrap();
        cpu.install_hook(Hook::Block {
            range: HookRange::All,
            callback: exhausted,
            data: (&mut data as *mut Budget<'_>).cast(),
        })
        .unwrap();
    }
    let mut accounting = cpu.accounting().unwrap();
    for budget in 1..37 {
        cpu.write_register(Register::X(0), &0u64.to_le_bytes())
            .unwrap();
        unsafe {
            accounting.as_mut().budget = budget;
            accounting.as_mut().enabled = 2;
        }
        run(&cpu, 0x1000, None, None);
        unsafe {
            accounting.as_mut().enabled = 0;
        }
        assert_eq!(value(&cpu, Register::Pc), 0x1000 + (budget % 5) as u64 * 4);
        assert_eq!(value(&cpu, Register::X(0)), (budget - budget / 5) as u64);
    }
    assert!(unexpected_entries.is_empty());
    assert!(data.callbacks > 0);
}

unsafe fn classify_replace(_: *mut c_void, _: u64, _: u32) -> Translation {
    Translation {
        instruction: InstructionDisposition::Replace,
        observe_block: true,
    }
}
unsafe fn replace(_: u64, _: u32, data: *mut c_void) {
    let cpu = unsafe { &*data.cast::<Core>() };
    cpu.write_register(Register::X(0), &42u64.to_le_bytes())
        .unwrap();
}
unsafe fn block_seen(_: Block, data: *mut c_void) {
    unsafe { *data.cast::<usize>() += 1 };
}
#[test]
fn replacement_and_final_branch_observation_are_not_bypassed_by_accounting() {
    let cpu = core();
    // If translated, the branch would loop forever. Its observer replaces it.
    words(&cpu, 0x1000, &[0x14000000]);
    let mut blocks = 0usize;
    unsafe {
        cpu.translation_hooks(Some(TranslationHooks {
            classify: classify_replace,
            restart: restarted,
            data: std::ptr::null_mut(),
        }))
        .unwrap();
        cpu.install_hook(Hook::Instruction {
            range: HookRange::All,
            callback: replace,
            data: (&cpu as *const Core).cast_mut().cast(),
        })
        .unwrap();
        cpu.install_hook(Hook::Block {
            range: HookRange::All,
            callback: block_seen,
            data: (&mut blocks as *mut usize).cast(),
        })
        .unwrap();
        let mut accounting = cpu.accounting().unwrap();
        accounting.as_mut().budget = 10;
        accounting.as_mut().enabled = 2;
    }
    run(&cpu, 0x1000, Some(0x1004), None);
    assert_eq!(blocks, 1);
    assert_eq!(value(&cpu, Register::X(0)), 42);
    unsafe {
        cpu.accounting().unwrap().as_mut().enabled = 0;
    }
}

unsafe fn classify_branch(_: *mut c_void, _: u64, _: u32) -> Translation {
    Translation {
        instruction: InstructionDisposition::Observe,
        observe_block: true,
    }
}
#[test]
fn final_branch_honors_block_observation_with_available_budget() {
    let cpu = core();
    words(&cpu, 0x1000, &[0x14000001]);
    let mut blocks = 0usize;
    unsafe {
        cpu.translation_hooks(Some(TranslationHooks {
            classify: classify_branch,
            restart: restarted,
            data: std::ptr::null_mut(),
        }))
        .unwrap();
        cpu.install_hook(Hook::Block {
            range: HookRange::All,
            callback: block_seen,
            data: (&mut blocks as *mut usize).cast(),
        })
        .unwrap();
        let mut accounting = cpu.accounting().unwrap();
        accounting.as_mut().budget = 10;
        accounting.as_mut().enabled = 2;
    }
    run(&cpu, 0x1000, Some(0x1004), None);
    assert_eq!(blocks, 1);
    unsafe {
        cpu.accounting().unwrap().as_mut().enabled = 0;
    }
}

unsafe fn collect_block(block: Block, data: *mut c_void) {
    unsafe { &mut *data.cast::<Vec<Block>>() }.push(block);
}
#[test]
fn native_counter_metadata_is_hidden_from_block_observers() {
    let cpu = core();
    words(
        &cpu,
        0x1000,
        &[0x91000400, 0x91000400, 0x91000400, 0x14000000],
    );
    let mut blocks = Vec::<Block>::new();
    unsafe {
        cpu.install_hook(Hook::Block {
            range: HookRange::All,
            callback: collect_block,
            data: (&mut blocks as *mut Vec<Block>).cast(),
        })
        .unwrap();
    }
    run(&cpu, 0x1000, Some(0x100c), Some(2));
    assert_eq!(value(&cpu, Register::Pc), 0x1008);
    assert!(
        blocks
            .iter()
            .all(|block| block.bytes <= 16 && block.thumb.is_none())
    );
    run(&cpu, 0x1000, Some(0x100c), None);
    assert_eq!(value(&cpu, Register::Pc), 0x100c);
    run(&cpu, 0x1000, Some(0x100c), Some(1));
    assert_eq!(value(&cpu, Register::Pc), 0x1004);
}

unsafe fn stop_block(_: Block, data: *mut c_void) {
    unsafe { &*data.cast::<Core>() }.stop().unwrap();
}
#[test]
fn exact_ring_does_not_truncate_a_64_bit_block_address() {
    let cpu = core();
    let base = 0x1_0000_0000;
    cpu.map_zeroed(base, 0x1000, 7).unwrap();
    words(&cpu, base, &[0x91000400, 0x14000000]);
    let mut ring = [0u64; 2];
    let mut accounting = cpu.accounting().unwrap();
    unsafe {
        cpu.translation_hooks(Some(TranslationHooks {
            classify: classify_execute,
            restart: restarted,
            data: std::ptr::null_mut(),
        }))
        .unwrap();
        cpu.install_hook(Hook::Block {
            range: HookRange::All,
            callback: stop_block,
            data: (&cpu as *const Core).cast_mut().cast(),
        })
        .unwrap();
        accounting.as_mut().enabled = 1;
        accounting.as_mut().budget = 10;
        accounting.as_mut().ring = ring.as_mut_ptr();
        accounting.as_mut().mask = 1;
    }
    run(&cpu, base, None, None);
    assert_eq!(value(&cpu, Register::Pc), base);
    unsafe {
        assert_eq!(accounting.as_ref().budget, 10);
        assert_eq!(accounting.as_ref().head, 0);
        accounting.as_mut().enabled = 0;
    }
    assert_eq!(ring, [0, 0]);
}
