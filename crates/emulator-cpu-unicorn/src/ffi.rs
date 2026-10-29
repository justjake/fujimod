//! Minimal, version-pinned Unicorn 2.1.4 public C ABI. No Python binding handles.
//! Definitions checked against include/unicorn/{unicorn,arm}.h at tag 2.1.4.
use fujimod_emulator_cpu::BackingObserver;
use std::ffi::{c_char, c_int, c_void};

pub type Engine = *mut c_void;
pub type Context = *mut c_void;
pub type Error = c_int;
pub const FEATURE_PAUSE_ACCOUNTING_LAYOUT: u32 = 1 << 3;
pub const FEATURE_PARALLEL_TCG: u32 = 1 << 4;
pub const FEATURE_REPLACED_INSTRUCTIONS: u32 = 1 << 5;
pub const FEATURE_AARCH64_ACCOUNTING: u32 = 1 << 6;
pub const FEATURE_INLINE_INSTRUCTION_COUNTER: u32 = 1 << 7;
pub const fn ctl(kind: u32, count: u32, direction: u32) -> u32 {
    kind | (count << 26) | (direction << 30)
}

#[repr(C)]
pub struct CpReg {
    pub cp: u32,
    pub is64: u32,
    pub sec: u32,
    pub crn: u32,
    pub crm: u32,
    pub opc1: u32,
    pub opc2: u32,
    pub val: u64,
}
#[repr(C)]
pub struct MemRegion {
    pub begin: u64,
    pub end: u64,
    pub perms: u32,
}

/// Acceleration exports added by the pinned Unicorn source build.
/// Missing exports require the explicit slow-Unicorn opt-in before loading.
pub struct Extensions {
    pub replace_backing: Option<unsafe extern "C" fn(Engine, u64, u64, u32, *mut c_void) -> Error>,
    pub write_hook_pc_sync: Option<unsafe extern "C" fn(Engine, bool) -> Error>,
    pub page_hooks: Option<unsafe extern "C" fn(Engine, bool) -> Error>,
    pub observe_backings:
        Option<unsafe extern "C" fn(Engine, *const BackingObserver, usize) -> Error>,
    pub hook_watched: Option<unsafe extern "C" fn(Engine, usize, bool) -> Error>,
    pub watch: Option<unsafe extern "C" fn(Engine, u64, u64) -> Error>,
    pub insn_filter: Option<
        unsafe extern "C" fn(Engine, Option<InsnFilter>, Option<Restart>, *mut c_void) -> Error,
    >,
    pub retry_block: Option<unsafe extern "C" fn(Engine, u32) -> Error>,
    pub request_restart: Option<unsafe extern "C" fn(Engine) -> Error>,
    pub set_parallel_cpus: Option<unsafe extern "C" fn(Engine, bool) -> Error>,
    /// Test exit requests inline at block entry and after memory accesses.
    pub set_block_exits: Option<unsafe extern "C" fn(Engine, bool) -> Error>,
    pub set_instruction_counter: Option<unsafe extern "C" fn(Engine, bool) -> Error>,
    pub reset_instruction_counter: Option<unsafe extern "C" fn(Engine) -> Error>,
    pub instruction_counter: Option<unsafe extern "C" fn(Engine) -> u64>,
    // uc_embedder_features bits (0 without the export): 1 = block hook size bit 31
    // is the Thumb state while a translation filter is installed.
    // Bit 4: stoppable Thumb instruction entries with synchronized ITSTATE.
    // Bit 32: filter decision 3 replaces an ARM instruction with its code hook.
    // Bit 128: translated instruction entries are counted in CPU state.
    pub features: u32,
    // uc_embedder_accounting (features bit 2): the engine's accounting record.
    pub accounting: Option<unsafe extern "C" fn(Engine) -> *mut Accounting>,
    pub exit_counters: Option<unsafe extern "C" fn(Engine) -> *const u64>,
}

impl Extensions {
    /// Direct native clients must acknowledge the same slow path as the
    /// Python selector.
    fn require_acceleration(&self) -> Result<(), String> {
        if self.features & FEATURE_PAUSE_ACCOUNTING_LAYOUT == 0 {
            return Err(
                "The Unicorn accounting ABI predates asynchronous pause support; rebuild the matching Unicorn library".into(),
            );
        }
        let missing: Vec<_> = [
            (
                self.write_hook_pc_sync.is_some(),
                "uc_embedder_set_write_hook_pc_sync",
            ),
            (self.page_hooks.is_some(), "uc_embedder_set_page_hooks"),
            (
                self.observe_backings.is_some(),
                "uc_embedder_observe_backings",
            ),
            (self.hook_watched.is_some(), "uc_embedder_set_hook_watched"),
            (self.watch.is_some(), "uc_embedder_watch"),
            (self.insn_filter.is_some(), "uc_embedder_set_insn_filter"),
            (self.retry_block.is_some(), "uc_embedder_retry_block"),
            (
                self.request_restart.is_some(),
                "uc_embedder_request_restart",
            ),
            (
                self.set_parallel_cpus.is_some() && self.features & FEATURE_PARALLEL_TCG != 0,
                "uc_embedder_set_parallel_cpus",
            ),
            (self.accounting.is_some(), "uc_embedder_accounting"),
            (
                self.set_block_exits.is_some(),
                "uc_embedder_set_block_exits",
            ),
            (
                self.set_instruction_counter.is_some()
                    && self.reset_instruction_counter.is_some()
                    && self.instruction_counter.is_some()
                    && self.features & FEATURE_INLINE_INSTRUCTION_COUNTER != 0,
                "uc_embedder_instruction_counter",
            ),
            (
                self.features & 3 == 3,
                "uc_embedder_features (Thumb block state and budgeted accounting)",
            ),
            (
                self.features & FEATURE_REPLACED_INSTRUCTIONS != 0,
                "uc_embedder_features (replaced ARM instructions)",
            ),
        ]
        .into_iter()
        .filter_map(|(present, name)| (!present).then_some(name))
        .collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(format!(
            "The linked pinned Unicorn build is missing required embedder features: {}.",
            missing.join(", ")
        ))
    }
}
/// Unicorn's embedder budgeted block accounting record (uc_priv.h embedder_acct).
/// `enabled` selects exact ring accounting (1), coarse interactive accounting
/// without ring writes (2), or disabled accounting (0).
#[repr(C)]
pub struct Accounting {
    pub budget: i64,
    pub head: u64,
    pub ring: *mut u64,
    pub mask: u64,
    pub enabled: u64,
    pub pause_requested: *const u32,
}
pub type InsnFilter = unsafe extern "C" fn(*mut c_void, u64, u32) -> c_int;
pub type Restart = unsafe extern "C" fn(*mut c_void, u64);
macro_rules! api {
    ($($field:ident : unsafe extern "C" fn($($args:tt)*) -> $result:ty),+ $(,)?) => {
        pub struct Api {
            $(pub $field: unsafe extern "C" fn($($args)*) -> $result,)+
            pub extensions: Extensions,
        }
        unsafe extern "C" { $(fn $field($($args)*) -> $result;)+ }
        impl Api {
            pub fn linked() -> Result<Self, String> {
                let api = Self { $($field,)+ extensions: linked::extensions() };
                api.extensions.require_acceleration()?;
                Ok(api)
            }
        }
    }
}
api! {
    uc_version: unsafe extern "C" fn(_: *mut u32, _: *mut u32) -> u32,
    uc_open: unsafe extern "C" fn(_: c_int, _: c_int, _: *mut Engine) -> Error,
    uc_close: unsafe extern "C" fn(_: Engine) -> Error,
    uc_strerror: unsafe extern "C" fn(_: Error) -> *const c_char,
    uc_ctl: unsafe extern "C" fn(_: Engine, _: u32,  ...) -> Error,
    uc_reg_read: unsafe extern "C" fn(_: Engine, _: c_int, _: *mut c_void) -> Error,
    uc_reg_write: unsafe extern "C" fn(_: Engine, _: c_int, _: *const c_void) -> Error,
    uc_reg_read2: unsafe extern "C" fn(_: Engine, _: c_int, _: *mut c_void, _: *mut usize) -> Error,
    uc_reg_write2: unsafe extern "C" fn(_: Engine, _: c_int, _: *const c_void, _: *mut usize) -> Error,
    uc_mem_read: unsafe extern "C" fn(_: Engine, _: u64, _: *mut c_void, _: u64) -> Error,
    uc_mem_write: unsafe extern "C" fn(_: Engine, _: u64, _: *const c_void, _: u64) -> Error,
    uc_vmem_translate: unsafe extern "C" fn(_: Engine, _: u64, _: u32, _: *mut u64) -> Error,
    uc_mem_map: unsafe extern "C" fn(_: Engine, _: u64, _: u64, _: u32) -> Error,
    uc_mem_map_ptr: unsafe extern "C" fn(_: Engine, _: u64, _: u64, _: u32, _: *mut c_void) -> Error,
    uc_mmio_map: unsafe extern "C" fn(_: Engine, _: u64, _: u64, _: *mut c_void, _: *mut c_void, _: *mut c_void, _: *mut c_void) -> Error,
    uc_mem_unmap: unsafe extern "C" fn(_: Engine, _: u64, _: u64) -> Error,
    uc_mem_protect: unsafe extern "C" fn(_: Engine, _: u64, _: u64, _: u32) -> Error,
    uc_mem_regions: unsafe extern "C" fn(_: Engine, _: *mut *mut MemRegion, _: *mut u32) -> Error,
    uc_free: unsafe extern "C" fn(_: *mut c_void) -> Error,
    uc_emu_start: unsafe extern "C" fn(_: Engine, _: u64, _: u64, _: u64, _: usize) -> Error,
    uc_emu_stop: unsafe extern "C" fn(_: Engine) -> Error,
    uc_hook_add: unsafe extern "C" fn(_: Engine, _: *mut usize, _: c_int, _: *mut c_void, _: *mut c_void, _: u64, _: u64,  ...) -> Error,
    uc_hook_del: unsafe extern "C" fn(_: Engine, _: usize) -> Error,
    uc_context_alloc: unsafe extern "C" fn(_: Engine, _: *mut Context) -> Error,
    uc_context_reg_read2: unsafe extern "C" fn(_: Context, _: c_int, _: *mut c_void, _: *mut usize) -> Error,
    uc_context_reg_write2: unsafe extern "C" fn(_: Context, _: c_int, _: *const c_void, _: *mut usize) -> Error,
    uc_context_save: unsafe extern "C" fn(_: Engine, _: Context) -> Error,
    uc_context_restore: unsafe extern "C" fn(_: Engine, _: Context) -> Error,
    uc_context_free: unsafe extern "C" fn(_: Context) -> Error,
}

/// The crate links the pinned Unicorn archive at build time; no runtime loader.
mod linked {
    use super::*;
    unsafe extern "C" {
        fn uc_embedder_replace_backing(
            engine: Engine,
            address: u64,
            size: u64,
            permissions: u32,
            backing: *mut c_void,
        ) -> Error;
        fn uc_embedder_set_write_hook_pc_sync(engine: Engine, enabled: bool) -> Error;
        fn uc_embedder_set_page_hooks(engine: Engine, enabled: bool) -> Error;
        fn uc_embedder_observe_backings(
            engine: Engine,
            ranges: *const BackingObserver,
            count: usize,
        ) -> Error;
        fn uc_embedder_set_hook_watched(engine: Engine, hook: usize, watched: bool) -> Error;
        fn uc_embedder_watch(engine: Engine, begin: u64, size: u64) -> Error;
        fn uc_embedder_set_insn_filter(
            engine: Engine,
            filter: Option<InsnFilter>,
            restart: Option<Restart>,
            data: *mut c_void,
        ) -> Error;
        fn uc_embedder_retry_block(engine: Engine, count: u32) -> Error;
        fn uc_embedder_request_restart(engine: Engine) -> Error;
        fn uc_embedder_set_parallel_cpus(engine: Engine, enabled: bool) -> Error;
        fn uc_embedder_set_block_exits(engine: Engine, enabled: bool) -> Error;
        fn uc_embedder_set_instruction_counter(engine: Engine, enabled: bool) -> Error;
        fn uc_embedder_reset_instruction_counter(engine: Engine) -> Error;
        fn uc_embedder_instruction_counter(engine: Engine) -> u64;
        fn uc_embedder_accounting(engine: Engine) -> *mut Accounting;
        fn uc_embedder_exit_counters(engine: Engine) -> *const u64;
        fn uc_embedder_features() -> u32;
    }
    pub fn extensions() -> Extensions {
        Extensions {
            replace_backing: Some(uc_embedder_replace_backing),
            write_hook_pc_sync: Some(uc_embedder_set_write_hook_pc_sync),
            page_hooks: Some(uc_embedder_set_page_hooks),
            observe_backings: Some(uc_embedder_observe_backings),
            hook_watched: Some(uc_embedder_set_hook_watched),
            watch: Some(uc_embedder_watch),
            insn_filter: Some(uc_embedder_set_insn_filter),
            retry_block: Some(uc_embedder_retry_block),
            request_restart: Some(uc_embedder_request_restart),
            set_parallel_cpus: Some(uc_embedder_set_parallel_cpus),
            set_block_exits: Some(uc_embedder_set_block_exits),
            set_instruction_counter: Some(uc_embedder_set_instruction_counter),
            reset_instruction_counter: Some(uc_embedder_reset_instruction_counter),
            instruction_counter: Some(uc_embedder_instruction_counter),
            accounting: Some(uc_embedder_accounting),
            exit_counters: Some(uc_embedder_exit_counters),
            features: unsafe { uc_embedder_features() },
        }
    }
}
