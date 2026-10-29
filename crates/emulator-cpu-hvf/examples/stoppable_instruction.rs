//! Signed, source-authored regression for interception before instruction entry.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn main() {
    panic!("requires macOS ARM64");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn main() -> Result<(), String> {
    use fujimod_emulator_cpu::*;
    use fujimod_emulator_cpu_hvf::Vm;
    use std::ffi::c_void;
    struct Context<'a> {
        cpu: &'a dyn Cpu,
        calls: usize,
    }
    unsafe fn stop(_: u64, _: u32, data: *mut c_void) {
        let context = unsafe { &mut *data.cast::<Context<'_>>() };
        context.calls += 1;
        context.cpu.stop_before_instruction().unwrap();
    }
    fn get(cpu: &dyn Cpu, r: Register) -> u64 {
        let mut raw = [0; 8];
        cpu.read_register(r, &mut raw).unwrap();
        u64::from_le_bytes(raw)
    }
    let vm = Vm::new()?;
    let cpu = vm.cpu(Model::ArmV8A)?;
    cpu.map_zeroed(0x10000, 0x4000, 7)?;
    cpu.write_memory(0x10000, &0x91000400u32.to_le_bytes())?; // ADD x0,x0,#1
    cpu.write_register(Register::X(0), &41u64.to_le_bytes())?;
    let mut context = Context {
        cpu: &cpu,
        calls: 0,
    };
    assert!(cpu.stop_before_instruction().is_err());
    let hook = unsafe {
        cpu.install_hook(Hook::Instruction {
            callback: stop,
            data: (&mut context as *mut Context<'_>).cast(),
            range: HookRange::Inclusive {
                begin: 0x10000,
                end: 0x10000,
            },
        })?
    };
    cpu.run(Run {
        begin: 0x10000,
        until: Some(0x10004),
        instructions: Some(1),
        watchdog_us: 1_000_000,
    })
    .map_err(|e| e.to_string())?;
    assert_eq!(context.calls, 1);
    assert_eq!(get(&cpu, Register::Pc), 0x10000);
    assert_eq!(get(&cpu, Register::X(0)), 41);
    assert_eq!(cpu.instruction_progress(), Some(0));
    cpu.remove_hook(hook)?;
    cpu.run(Run {
        begin: 0x10000,
        until: Some(0x10004),
        instructions: Some(1),
        watchdog_us: 1_000_000,
    })
    .map_err(|e| e.to_string())?;
    assert_eq!(get(&cpu, Register::Pc), 0x10004);
    assert_eq!(get(&cpu, Register::X(0)), 42);
    assert_eq!(cpu.instruction_progress(), Some(1));
    assert!(cpu.stop_before_instruction().is_err());
    println!(
        "stoppable instruction: unchanged PC/registers and zero entries, then one-entry resume passed"
    );
    Ok(())
}
