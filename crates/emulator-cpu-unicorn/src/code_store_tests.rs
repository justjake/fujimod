//! A CPU store over translated code takes effect at the next fetch, also on
//! an engine whose embedder installs instruction and store hooks but declares
//! no code watches (page-hooks mode). QEMU keeps translated code coherent with
//! guest stores; the I-cache maintenance a program issues before running
//! rewritten code is then architecturally satisfied.
use crate::{Core, test_support};
use fujimod_emulator_cpu::{Access, Cpu, Hook, HookRange, Model, Register, Run, arm};
use std::ffi::c_void;

fn words(cpu: &Core, address: u64, words: &[u32]) {
    let bytes = words
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect::<Vec<_>>();
    cpu.write_memory(address, &bytes).unwrap();
}
unsafe fn instruction(_: u64, _: u32, _: *mut c_void) {}
unsafe fn store(_: Access, _: u64, _: i32, _: i64, _: *mut c_void) {}

fn store_over_translated_code(hooks: bool) {
    let cpu = test_support::library()
        .open(Model::CortexA7, 1 << 20)
        .unwrap();
    cpu.map_zeroed(0, 0x8000, 7).unwrap();
    cpu.write_register(arm::CPSR, &0xd3u32.to_le_bytes()).unwrap();
    if hooks {
        let data = std::ptr::null_mut();
        let range = HookRange::Inclusive { begin: 0, end: 0x7fff };
        for hook in [
            Hook::Instruction {
                callback: instruction,
                data,
                range: HookRange::All,
            },
            Hook::Memory {
                callback: store,
                data,
                read: false,
                write: true,
                range,
            },
        ] {
            unsafe { cpu.install_hook(hook) }.unwrap();
        }
    }
    // 0x0: mov r0, #1; b 0x2000
    words(&cpu, 0, &[0xe3a0_0001, 0xea00_07fd]);
    // 0x2000: ldr r1, [pc, #8]; ldr r2, [pc, #8]; str r1, [r2]; bx r2;
    // literals mov r0, #2 and 0
    words(
        &cpu,
        0x2000,
        &[0xe59f_1008, 0xe59f_2008, 0xe582_1000, 0xe12f_ff12, 0xe3a0_0002, 0],
    );
    cpu.run(Run {
        begin: 0,
        until: None,
        instructions: Some(7),
        watchdog_us: 0,
    })
    .unwrap();
    let read = |register| {
        let mut out = [0; 4];
        cpu.read_register(register, &mut out).unwrap();
        u32::from_le_bytes(out)
    };
    assert_eq!((read(arm::CORE[0]), read(Register::Pc)), (2, 4));
}

#[test]
fn a32_store_over_translated_code_runs_the_new_code() {
    store_over_translated_code(false);
}

#[test]
fn a32_store_over_translated_code_runs_the_new_code_under_instruction_and_store_hooks() {
    store_over_translated_code(true);
}
