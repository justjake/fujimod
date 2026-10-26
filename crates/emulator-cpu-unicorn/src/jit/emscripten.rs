//! Owner-thread engine adapter. Browser imports load bytes and table handles;
//! the Rust tier owns every compilation and retirement decision.
use super::{
    Environment, Helper, HelperKind, HelperLayout, Host, JitOptions, MemoryLayout, Metadata,
    Module, Statistics, Tier,
};
use std::{
    cell::RefCell,
    ffi::{CStr, c_char, c_void},
};

unsafe extern "C" {
    fn uc_embedder_tci_wasm_configure(
        threshold: u32,
        notice: unsafe extern "C" fn(*mut c_void, u32, u32, *mut u32, *const u8, u32),
        dispatcher: u32,
    ) -> u32;
    fn uc_embedder_tci_wasm_dispatch_layout() -> *const u32;
    fn uc_embedder_tci_wasm_layout(engine: *mut c_void) -> *const u32;
    fn uc_embedder_tci_wasm_helper_layout() -> *const u32;
    fn uc_embedder_tci_wasm_opname(engine: *mut c_void, opcode: u32) -> *const c_char;
    fn uc_embedder_tci_wasm_helper_name(function: u32) -> *const c_char;
    fn uc_embedder_tci_wasm_helper_target(function: u32) -> u32;
    fn uc_embedder_tci_wasm_count(counter: u32) -> u64;
    fn unicorn_jit_host_grow(count: u32) -> u32;
    fn unicorn_jit_host_install(
        bytes: *const u8,
        len: u32,
        exports: *const Export,
        count: u32,
    ) -> i32;
    fn unicorn_jit_host_clear(slot: u32);
    fn unicorn_jit_host_error(buffer: *mut u8, capacity: u32) -> u32;
    fn unicorn_jit_host_now() -> f64;
}
#[repr(C)]
struct Export {
    name: *const u8,
    len: u32,
    slot: u32,
}
struct Browser;
enum Runtime {
    Unconfigured,
    Disabled,
    Enabled(Tier<Browser>),
}
thread_local! {static RUNTIME:RefCell<Runtime>=const {RefCell::new(Runtime::Unconfigured)};}

fn host_error() -> String {
    let length = unsafe { unicorn_jit_host_error(std::ptr::null_mut(), 0) };
    let mut bytes = vec![0; length as usize];
    unsafe { unicorn_jit_host_error(bytes.as_mut_ptr(), length) };
    String::from_utf8_lossy(&bytes).into_owned()
}
unsafe fn words(pointer: *const u32, length: usize, version: u32) -> Result<Vec<u32>, String> {
    if pointer.is_null() {
        return Err("engine returned a null JIT layout".into());
    }
    // Check the leading version before reading that version's full extent.
    if unsafe { *pointer } != version {
        return Err("engine returned an unsupported JIT layout version".into());
    }
    Ok(unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec())
}
unsafe fn name(pointer: *const c_char) -> Result<String, String> {
    if pointer.is_null() {
        return Ok(String::new());
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| "non-UTF-8 JIT helper name".into())
}
impl Host for Browser {
    fn dispatch_layout(&mut self) -> Result<Vec<u32>, String> {
        unsafe { words(uc_embedder_tci_wasm_dispatch_layout(), 4, 2) }
    }
    fn metadata(&mut self, engine: u32) -> Result<Metadata, String> {
        let engine = engine as *mut c_void;
        let memory =
            MemoryLayout::new(&unsafe { words(uc_embedder_tci_wasm_layout(engine), 22, 1) }?)?;
        let helper =
            HelperLayout::new(&unsafe { words(uc_embedder_tci_wasm_helper_layout(), 8, 1) }?)?;
        let names = (0..255)
            .map(|opcode| unsafe { name(uc_embedder_tci_wasm_opname(engine, opcode)) })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Metadata {
            names,
            environment: Environment {
                memory: Some(memory),
                helper: Some(helper),
                ..Default::default()
            },
        })
    }
    fn helper(&mut self, pointer: u32) -> Result<Option<Helper>, String> {
        let Some(kind) =
            HelperKind::named(&unsafe { name(uc_embedder_tci_wasm_helper_name(pointer)) }?)
        else {
            return Ok(None);
        };
        let target = unsafe { uc_embedder_tci_wasm_helper_target(pointer) };
        if target == 0 {
            return Err("engine returned no typed target for a known helper".into());
        }
        Ok(Some(Helper { kind, target }))
    }
    fn grow_table(&mut self, count: u32) -> Result<u32, String> {
        match unsafe { unicorn_jit_host_grow(count) } {
            u32::MAX => Err(host_error()),
            first => Ok(first),
        }
    }
    fn install(&mut self, module: &Module, slots: &[u32]) -> Result<(), String> {
        if slots.len() != module.exports.len() {
            return Err("JIT module export count differs from requested slots".into());
        }
        let exports = module
            .exports
            .iter()
            .zip(slots)
            .map(|(name, slot)| Export {
                name: name.as_ptr(),
                len: name.len() as u32,
                slot: *slot,
            })
            .collect::<Vec<_>>();
        let length = module
            .bytes
            .len()
            .try_into()
            .map_err(|_| "JIT module exceeds wasm32 extent")?;
        if unsafe {
            unicorn_jit_host_install(
                module.bytes.as_ptr(),
                length,
                exports.as_ptr(),
                exports.len() as u32,
            )
        } == 0
        {
            Ok(())
        } else {
            Err(host_error())
        }
    }
    fn clear_function(&mut self, slot: u32) {
        unsafe { unicorn_jit_host_clear(slot) };
    }
    fn publish(&mut self, address: u32, slot: u32) {
        // The engine invalidation callback revokes this address before reuse.
        unsafe { (address as *mut u32).write_volatile(slot) };
    }
    fn configure(&mut self, threshold: u32, dispatcher: u32) -> Result<(), String> {
        let code = unsafe { uc_embedder_tci_wasm_configure(threshold, notice, dispatcher) };
        if code == 0 {
            Ok(())
        } else {
            Err(format!(
                "TCG tier must be configured once before guest execution (engine status {code})"
            ))
        }
    }
    fn now_millis(&self) -> f64 {
        unsafe { unicorn_jit_host_now() }
    }
}
unsafe extern "C" fn notice(
    engine: *mut c_void,
    event: u32,
    generation: u32,
    slot: *mut u32,
    code: *const u8,
    length: u32,
) {
    RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        if let Runtime::Enabled(tier) = &mut *runtime {
            let bytes = if event == 1 && !code.is_null() && length != 0 {
                // Engine-generated TCI is copied into typed operations before
                // browser calls can grow memory. No source slice is retained.
                unsafe { std::slice::from_raw_parts(code, length as usize) }
            } else {
                &[]
            };
            let _ = tier.notice(
                event,
                engine as u32,
                generation,
                slot as u32,
                code as u32,
                bytes,
            );
        }
    });
}
pub fn setup(options: Option<JitOptions>) -> Result<(), String> {
    RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        if !matches!(*runtime, Runtime::Unconfigured) {
            return Err("TCG tier is already configured".into());
        }
        *runtime = match options {
            None => Runtime::Disabled,
            Some(options) => Runtime::Enabled(Tier::new(options, Browser)?),
        };
        Ok(())
    })
}
pub fn flush() {
    RUNTIME.with(|runtime| {
        if let Runtime::Enabled(tier) = &mut *runtime.borrow_mut() {
            let _ = tier.flush();
        }
    });
}
pub fn statistics() -> Option<Statistics> {
    RUNTIME.with(|runtime| match &*runtime.borrow() {
        Runtime::Enabled(tier) => Some(Statistics {
            enabled: true,
            stats: tier.stats().clone(),
            installed: tier.installed(),
            retained_bytes: tier.retained_bytes(),
            cached_functions: tier.cached(),
            options: tier.options(),
            chained_executions: unsafe { uc_embedder_tci_wasm_count(2) }.to_string(),
            compiled_executions: unsafe { uc_embedder_tci_wasm_count(0) }.to_string(),
            entries: unsafe { uc_embedder_tci_wasm_count(1) }.to_string(),
        }),
        _ => None,
    })
}
