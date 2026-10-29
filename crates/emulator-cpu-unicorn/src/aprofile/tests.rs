use super::*;
fn cpu() -> Core {
    let raw = crate::test_support::library()
        .open(Model::ArmV8A, 1 << 20)
        .unwrap();
    let cpu = Core::new(raw, Memory::default()).unwrap();
    cpu.map_zeroed(0x1000, 0x4000, 7).unwrap();
    cpu.write_register(Register::Pstate, &0x3cdu64.to_le_bytes())
        .unwrap();
    cpu
}
fn value(cpu: &dyn Cpu, r: Register) -> u64 {
    let mut b = [0; 8];
    cpu.read_register(r, &mut b).unwrap();
    u64::from_le_bytes(b)
}
fn code(cpu: &Core, words: &[u32]) {
    cpu.write_memory(
        0x1000,
        &words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap()
}
fn run(cpu: &Core, begin: u64, budget: usize) -> Exit {
    cpu.run(Run {
        begin,
        until: None,
        instructions: Some(budget),
        watchdog_us: 0,
    })
    .unwrap()
}
#[test]
fn interactive_policy_is_forwarded_and_keeps_exact_instruction_limits() {
    let cpu = cpu();
    cpu.set_execution_policy(ExecutionPolicy::Interactive).unwrap();
    code(&cpu, &[0x91000400, 0x17ffffff]);
    assert!(matches!(run(&cpu, 0x1000, 1), Exit::Stopped));
    assert_eq!(cpu.instruction_progress(), Some(1));
    assert_eq!(value(&cpu, Register::X(0)), 1);
}
#[test]
fn every_budget_boundary_matches_instruction_entries() {
    let cpu = cpu();
    code(
        &cpu,
        &[0x91000400, 0x91000400, 0x91000400, 0x91000400, 0x17fffffc],
    );
    for n in 0..83 {
        cpu.write_register(Register::Pc, &0x1000u64.to_le_bytes())
            .unwrap();
        cpu.write_register(Register::X(0), &0u64.to_le_bytes())
            .unwrap();
        run(&cpu, 0x1000, n);
        assert_eq!(cpu.instruction_progress(), Some(n as u64));
        assert_eq!(value(&cpu, Register::Pc), 0x1000 + (n % 5) as u64 * 4);
        assert_eq!(value(&cpu, Register::X(0)), (n - n / 5) as u64);
    }
}
#[test]
fn framework_exit_precedes_entry_and_preserves_exact_budget() {
    let cpu = cpu();
    code(&cpu, &[0x91000400, 0x91000400, 0xd53be040, 0x91000400]); // MRS x0,CNTVCT_EL0
    assert!(matches!(run(&cpu, 0x1000, 2), Exit::Stopped));
    assert_eq!(cpu.instruction_progress(), Some(2));
    assert!(matches!(
        run(&cpu, 0x1000, 3),
        Exit::BeforeInstruction(0xd53be040)
    ));
    assert_eq!(cpu.instruction_progress(), Some(2));
    assert_eq!(value(&cpu, Register::Pc), 0x1008);
}

struct Device<'a> {
    cpu: &'a Core,
    reads: Vec<u64>,
    writes: Vec<u64>,
    tighten: bool,
}
unsafe fn read(_: u64, _: u32, data: *mut c_void) -> u64 {
    let d = unsafe { &mut *data.cast::<Device<'_>>() };
    d.reads.push(d.cpu.instruction_progress().unwrap());
    if d.tighten {
        d.cpu.tighten_instruction_limit(3).unwrap();
    }
    42
}
unsafe fn write(_: u64, _: u32, _: u64, data: *mut c_void) {
    let d = unsafe { &mut *data.cast::<Device<'_>>() };
    d.writes.push(d.cpu.instruction_progress().unwrap());
}
#[test]
fn mmio_entries_and_tightened_deadlines_are_exact() {
    for tighten in [false, true] {
        let cpu = cpu();
        code(
            &cpu,
            &[
                0x91000442, 0x91000442, 0xb9400020, 0x91000442, 0xb9000020, 0x91000442, 0x14000000,
            ],
        );
        cpu.write_register(Register::X(1), &0x8000u64.to_le_bytes())
            .unwrap();
        let mut d = Device {
            cpu: &cpu,
            reads: vec![],
            writes: vec![],
            tighten,
        };
        unsafe {
            cpu.map_mmio(
                0x8000,
                0x1000,
                Mmio {
                    read,
                    write,
                    data: (&mut d as *mut Device<'_>).cast(),
                },
            )
            .unwrap()
        }
        run(&cpu, 0x1000, 6);
        assert_eq!(d.reads, [3]);
        assert_eq!(value(&cpu, Register::X(0)), 42);
        assert_eq!(d.writes, if tighten { vec![] } else { vec![5] });
        assert_eq!(
            cpu.instruction_progress(),
            Some(if tighten { 3 } else { 6 })
        );
        assert_eq!(
            value(&cpu, Register::Pc),
            if tighten { 0x100c } else { 0x1018 }
        );
    }
}
struct HighAddressDevice {
    reads: Vec<(u64, u32)>,
}
unsafe fn high_address_read(offset: u64, size: u32, data: *mut c_void) -> u64 {
    let device = unsafe { &mut *data.cast::<HighAddressDevice>() };
    device.reads.push((offset, size));
    0x1234_5678
}
unsafe fn high_address_write(_: u64, _: u32, _: u64, _: *mut c_void) {}

#[test]
fn x100vi_el1_mmio_read_uses_scr_rw_and_reaches_callback() {
    let raw = crate::test_support::library()
        .open(Model::ArmV8A, 1 << 20)
        .unwrap();
    let cpu = Core::new(raw, Memory::default()).unwrap();
    cpu.map_zeroed(0, 0x68000000, 7).unwrap(); // X100VI DRAM extent.
    let system_value = |encoding| {
        value(
            &cpu,
            Register::Arm64System(arm::SystemRegister::from_encoding(encoding)),
        )
    };
    assert_eq!(system_value(0xf088) & 0x400, 0x400);
    assert_eq!(system_value(0xe088) & 0x8000_0000, 0x8000_0000);
    cpu.write_register(Register::Pstate, &0x3c4u64.to_le_bytes())
        .unwrap();
    let system = |encoding: u16, value: u64| {
        cpu.write_register(
            Register::Arm64System(arm::SystemRegister::from_encoding(encoding)),
            &value.to_le_bytes(),
        )
        .unwrap();
    };
    cpu.write_memory(0x108000, &0x609u64.to_le_bytes()).unwrap();
    cpu.write_memory(0x108018, &0x109003u64.to_le_bytes())
        .unwrap();
    cpu.write_memory(
        0x109000 + 0x1d0 * 8,
        &0x0010_0000_fa00_0601u64.to_le_bytes(),
    )
    .unwrap();
    system(0xc100, 0x108000); // TTBR0_EL1.
    system(0xc102, 0x1a59c251c); // 36-bit VA, 4 KiB granule.
    system(0xc510, 0xff4404); // MAIR_EL1.
    system(0xc080, 0x30d00801); // SCTLR_EL1 at the observed failure.
    // LDR W0, [X0] from the observed boot stop.
    cpu.write_memory(0x9ed88, &0xb9400000u32.to_le_bytes())
        .unwrap();
    cpu.write_register(Register::X(0), &0xfa090400u64.to_le_bytes())
        .unwrap();
    let mut device = HighAddressDevice { reads: vec![] };
    unsafe {
        cpu.map_mmio(
            0xfa000000,
            0x100000,
            Mmio {
                read: high_address_read,
                write: high_address_write,
                data: (&mut device as *mut HighAddressDevice).cast(),
            },
        )
        .unwrap()
    }
    assert_eq!(cpu.translate(0xfa090400, Access::Read).unwrap(), 0xfa090400);
    run(&cpu, 0x9ed88, 1);
    assert_eq!(device.reads, [(0x90400, 4)]);
    assert_eq!(value(&cpu, Register::X(0)), 0x1234_5678);
}
unsafe fn intercept(_: u64, _: u32, data: *mut c_void) {
    unsafe { &*data.cast::<Core>() }
        .stop_before_instruction()
        .unwrap();
}
#[test]
fn observer_intercepts_before_entry_and_effect() {
    let cpu = cpu();
    code(&cpu, &[0x91000400, 0x91000400, 0x91000400]);
    unsafe {
        cpu.install_hook(Hook::Instruction {
            range: HookRange::Inclusive {
                begin: 0x1004,
                end: 0x1004,
            },
            callback: intercept,
            data: (&cpu as *const Core).cast_mut().cast(),
        })
        .unwrap();
    }
    run(&cpu, 0x1000, 3);
    assert_eq!(cpu.instruction_progress(), Some(1));
    assert_eq!(value(&cpu, Register::X(0)), 1);
    assert_eq!(value(&cpu, Register::Pc), 0x1004);
}
#[test]
fn irq_unmask_stops_before_the_following_instruction() {
    let cpu = cpu();
    code(&cpu, &[0xd50342ff, 0x91000400]); // DAIFClr #2; ADD x0,x0,#1.
    let sys = |n| Register::Arm64System(arm::SystemRegister::from_encoding(n));
    cpu.write_register(sys(0xf088), &0x400u64.to_le_bytes())
        .unwrap();
    cpu.write_register(Register::Pstate, &0x3c5u64.to_le_bytes())
        .unwrap();
    cpu.write_register(sys(0xc600), &0x2000u64.to_le_bytes())
        .unwrap();
    cpu.set_interrupt(Interrupt::Irq, true).unwrap();
    run(&cpu, 0x1000, 2);
    assert_eq!(cpu.instruction_progress(), Some(1));
    assert_eq!(value(&cpu, Register::Pc), 0x2280);
    assert_eq!(value(&cpu, sys(0xc201)), 0x1004);
    assert_eq!(value(&cpu, Register::X(0)), 0);
}
#[test]
fn host_code_write_invalidates_every_core_sharing_backing() {
    use fujimod_emulator_cpu::memory::Backing;
    let memory = Memory::default();
    let backing = Backing::aligned_zeros(0x4000, 4096).unwrap();
    let open = || {
        let raw = crate::test_support::library()
            .open(Model::ArmV8A, 1 << 20)
            .unwrap();
        let cpu = Core::new(raw, memory.clone()).unwrap();
        unsafe {
            cpu.map_physical(
                0x1000,
                0x1000,
                0x4000,
                7,
                NonNull::new(backing.as_ptr()).unwrap(),
            )
            .unwrap()
        };
        cpu
    };
    let a = open();
    let b = open();
    code(&a, &[0x91000400, 0x14000000]);
    run(&a, 0x1000, 1);
    run(&b, 0x1000, 1);
    code(&a, &[0x91001400, 0x14000000]);
    run(&b, 0x1000, 1);
    assert_eq!(value(&b, Register::X(0)), 6);
}

#[test]
fn snapshots_restore_pending_interrupt_inputs() {
    let cpu = cpu();
    code(&cpu, &[0xd50342ff, 0x91000400]);
    let sys = |n| Register::Arm64System(arm::SystemRegister::from_encoding(n));
    cpu.write_register(sys(0xf088), &0x400u64.to_le_bytes())
        .unwrap();
    cpu.write_register(sys(0xc600), &0x2000u64.to_le_bytes())
        .unwrap();
    cpu.write_register(Register::Pstate, &0x3c5u64.to_le_bytes())
        .unwrap();
    cpu.set_interrupt(Interrupt::Irq, true).unwrap();
    let saved = cpu.snapshot(None).unwrap();
    cpu.set_interrupt(Interrupt::Irq, false).unwrap();
    cpu.restore(&*saved).unwrap();
    run(&cpu, 0x1000, 2);
    assert_eq!(value(&cpu, Register::Pc), 0x2280);
    assert_eq!(cpu.instruction_progress(), Some(1));
}

#[test]
fn completed_store_observers_see_ram_and_can_stop_before_the_next_entry() {
    unsafe fn stop(_: Access, address: u64, _: i32, _: i64, data: *mut c_void) {
        let cpu = unsafe { &*data.cast::<Core>() };
        let mut bytes = [0; 4];
        cpu.read_memory(address, &mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes), 42);
        cpu.tighten_instruction_limit(cpu.instruction_progress().unwrap())
            .unwrap();
    }
    let cpu = cpu();
    // Two stores to data-only pages ensure cached fast-path stores stay observed.
    code(
        &cpu,
        &[0x52800540, 0xb9000020, 0xb9000020, 0x91000400, 0x14000000],
    );
    cpu.write_register(Register::X(1), &0x3000u64.to_le_bytes())
        .unwrap();
    let hook = unsafe {
        cpu.install_hook(Hook::CompletedStore {
            range: HookRange::Inclusive {
                begin: 0x3000,
                end: 0x3003,
            },
            callback: stop,
            data: (&cpu as *const Core).cast_mut().cast(),
        })
    }
    .unwrap();
    run(&cpu, 0x1000, 20);
    assert_eq!(cpu.instruction_progress(), Some(2));
    assert_eq!(value(&cpu, Register::Pc), 0x1008);
    run(&cpu, 0x1008, 20);
    assert_eq!(cpu.instruction_progress(), Some(1));
    assert_eq!(value(&cpu, Register::Pc), 0x100c);
    cpu.remove_hook(hook).unwrap();
    run(&cpu, 0x100c, 1);
    assert_eq!(value(&cpu, Register::X(0)), 43);
}
