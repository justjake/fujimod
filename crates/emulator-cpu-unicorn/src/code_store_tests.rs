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
    cpu.write_register(arm::CPSR, &0xd3u32.to_le_bytes())
        .unwrap();
    if hooks {
        let data = std::ptr::null_mut();
        let range = HookRange::Inclusive {
            begin: 0,
            end: 0x7fff,
        };
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
        &[
            0xe59f_1008,
            0xe59f_2008,
            0xe582_1000,
            0xe12f_ff12,
            0xe3a0_0002,
            0,
        ],
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

#[test]
fn mprofile_shared_ram_stores_report_only_translated_guest_bytes() {
    #[repr(align(4096))]
    struct Ram([u8; 8192]);

    unsafe fn record(data: *mut c_void, address: u64, bytes: usize) {
        unsafe { &mut *data.cast::<Vec<(u64, usize)>>() }.push((address, bytes));
    }

    let mut ram = Box::new(Ram([0; 8192]));
    let mut notifications: Vec<(u64, usize)> = Vec::new();
    let cpu = test_support::library().open(Model::CortexM4, 1 << 20).unwrap();
    // Guest mapping is deliberately unrelated to Unicorn's backing offset.
    unsafe {
        cpu.map_backing(
            0x10000,
            ram.0.len(),
            7,
            std::ptr::NonNull::from(&mut ram.0[0]),
        )
        .unwrap();
        cpu.observe_code_stores(
            Some(record),
            std::ptr::from_mut(&mut notifications).cast(),
        )
        .unwrap();
    }
    // STRH R0,[R1].
    cpu.write_memory(0x10000, &0x8008u16.to_le_bytes()).unwrap();
    cpu.write_memory(0x10002, &0xe7feu16.to_le_bytes()).unwrap();
    cpu.write_memory(0x11002, &0xe7feu16.to_le_bytes()).unwrap();
    let step = |address: u64| {
        cpu.run(Run {
            begin: address | 1,
            until: None,
            watchdog_us: 1_000_000,
            instructions: Some(1),
        })
        .unwrap();
    };
    let store = |address: u32, value: u32| {
        cpu.write_register(arm::CORE[0], &value.to_le_bytes()).unwrap();
        cpu.write_register(arm::CORE[1], &address.to_le_bytes()).unwrap();
        step(0x10000);
    };

    // Fill the write TLB as data, then execute the destination and protect it.
    store(0x11000, 0x2001);
    step(0x11000);
    // The first store sharing that page must leave the live translation alone.
    store(0x11004, 0x42);
    store(0x11000, 0x2002);
    assert_eq!(notifications, [(0x11000, 2)]);
    step(0x11000);
    let mut result = [0; 4];
    cpu.read_register(arm::CORE[0], &mut result).unwrap();
    assert_eq!(u32::from_le_bytes(result), 2);
}
