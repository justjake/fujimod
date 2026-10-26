//! A memory hook that declares a watched page flushes the TLB mid-access.
//!
//! `rprofile` memory hooks may call `uc_embedder_watch` (through
//! `Memory::apply_watches`) while Unicorn is inside its load or store helper.
//! The flush may resize the TLB (QEMU frees and reallocates the table), so the
//! helper must look its entry up again after the hooks. Reusing the freed entry
//! read back a zero addend and dereferenced the guest address as a host
//! pointer.
//!
//! Each case reruns this test binary as a child with freed heap memory
//! scribbled (macOS libmalloc, glibc), so a stale entry is caught every time
//! and a host crash fails the parent test instead of the runner.
use crate::ffi::{Api, Engine, Error};
use std::ffi::{c_int, c_void};
use std::process::Command;

const CHILD: &str = "FUJIMOD_HOOK_TLB_FLUSH_CHILD";

struct Hook {
    watch: unsafe extern "C" fn(Engine, u64, u64) -> Error,
    calls: Vec<u64>,
}

unsafe extern "C" fn hook(
    engine: Engine,
    _access: c_int,
    address: u64,
    _size: c_int,
    _value: i64,
    data: *mut c_void,
) {
    // What Memory::apply_watches does from inside a memory hook.
    let hook = unsafe { &mut *data.cast::<Hook>() };
    hook.calls.push(address);
    let page = 0x30_0000 + 0x1000 * hook.calls.len() as u64;
    assert_eq!(unsafe { (hook.watch)(engine, page, 0x1000) }, 0);
}

fn execute(store: bool) {
    let api = Api::linked().unwrap();
    let mut engine: Engine = std::ptr::null_mut();
    unsafe {
        assert_eq!((api.uc_open)(1, 0, &mut engine), 0); // UC_ARCH_ARM, UC_MODE_ARM
        assert_eq!((api.uc_mem_map)(engine, 0, 0x40_0000, 7), 0);
    }
    // 300 pages is more than 70% of the 256-entry default TLB: a flush resizes it.
    let code: [u32; 10] = [
        0xe3a00601, // mov r0, #0x100000
        0xe300112c, // movw r1, #300
        0xe5902000, // loop: ldr r2, [r0]      one TLB entry per 1 KiB page
        0xe2800b01, // add r0, r0, #0x400
        0xe2511001, // subs r1, r1, #1
        0x1afffffb, // bne loop
        0xe3a03808, // mov r3, #0x80000        the hooked page
        0xe30b6eef, // movw r6, #0xbeef
        // str r6, [r3, #0x100] / ldrb r4, [r3, #0x353]
        if store { 0xe5836100 } else { 0xe5d34353 },
        0xe1a00000, // nop (stop address)
    ];
    let image: Vec<u8> = code.iter().flat_map(|word| word.to_le_bytes()).collect();
    let mut data = Hook {
        watch: api.extensions.watch.unwrap(),
        calls: Vec::new(),
    };
    let callback: unsafe extern "C" fn(Engine, c_int, u64, c_int, i64, *mut c_void) = hook;
    let kind = if store { 1 << 11 } else { 1 << 10 }; // UC_HOOK_MEM_WRITE / UC_HOOK_MEM_READ
    let mut handle = 0usize;
    unsafe {
        assert_eq!(
            (api.uc_mem_write)(engine, 0x1000, image.as_ptr().cast(), image.len() as u64),
            0
        );
        assert_eq!(
            (api.uc_mem_write)(engine, 0x80353, [0x5au8].as_ptr().cast(), 1),
            0
        );
        assert_eq!((api.extensions.page_hooks.unwrap())(engine, true), 0);
        assert_eq!(
            (api.uc_hook_add)(
                engine,
                &mut handle,
                kind,
                callback as *mut c_void,
                (&mut data as *mut Hook).cast(),
                0x80000,
                0x80fff,
            ),
            0
        );
        let end = 0x1000 + 4 * (code.len() as u64 - 1);
        assert_eq!((api.uc_emu_start)(engine, 0x1000, end, 0, 0), 0);
    }
    if store {
        assert_eq!(data.calls, [0x80100]);
        let mut word = [0u8; 4];
        assert_eq!(
            unsafe { (api.uc_mem_read)(engine, 0x80100, word.as_mut_ptr().cast(), 4) },
            0
        );
        assert_eq!(u32::from_le_bytes(word), 0xbeef);
    } else {
        assert_eq!(data.calls, [0x80353]);
        let mut r4 = 0u32;
        // UC_ARM_REG_R4
        assert_eq!(
            unsafe { (api.uc_reg_read)(engine, 70, (&mut r4 as *mut u32).cast()) },
            0
        );
        assert_eq!(r4, 0x5a);
    }
    unsafe { (api.uc_close)(engine) };
}

fn run_child(name: &str, store: bool) {
    if std::env::var_os(CHILD).is_some() {
        execute(store);
        return;
    }
    let test = format!(
        "{}::{name}",
        module_path!().trim_start_matches("fujimod_emulator_cpu_unicorn::")
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([test.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("MallocScribble", "1")
        .env("MALLOC_PERTURB_", "85")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "child {:?}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn read_hook_flushing_tlb_keeps_load_on_its_page() {
    run_child("read_hook_flushing_tlb_keeps_load_on_its_page", false);
}

#[test]
fn write_hook_flushing_tlb_keeps_store_on_its_page() {
    run_child("write_hook_flushing_tlb_keeps_store_on_its_page", true);
}
