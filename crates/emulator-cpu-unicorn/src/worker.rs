//! Native owner-thread proxy for one Unicorn processor.
//!
//! Unicorn and its `Rc` callback graph are created
//! and destroyed on the dedicated processor thread. The proxy stays on its
//! board thread and services guest callbacks there through synchronous RPC.
#![cfg(not(target_os = "emscripten"))]

use crate::Library;
use crate::memory::SharedMemory;
use crate::{aprofile, rprofile::core};
use fujimod_emulator_cpu::*;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
#[path = "rprofile/worker/durable.rs"]
mod durable;

/// Inputs needed to construct one native R-profile processor on its owner
/// thread.
pub struct Config {
    pub translation_cache_size_bytes: u32,
    pub options: core::Options,
    pub memory: SharedMemory,
}

/// Owner-thread CPU proxy. This value is intentionally thread-affine because
/// it holds callback pointers into the board and pumps them on its creator.
pub struct CpuWorker {
    #[cfg(test)]
    rpc_calls: Cell<usize>,
    // Host diagnostics stay outside the emulated CPU and its snapshots.
    rpc_counts: [Cell<u64>; WorkerRpcCounter::COUNT],
    callback_events: Cell<u64>,
    callback_counts: [Cell<u64>; WorkerCallbackCounter::COUNT],
    commands: Sender<Message>,
    events: RefCell<Receiver<Event>>,
    worker: RefCell<Option<JoinHandle<()>>>,
    identity: Identity,
    model: Model,
    owner_accounting: bool,
    execution_policy: Cell<ExecutionPolicy>,
    features: Features,
    thumb_accounting: Option<NonNull<ThumbBlockAccounting>>,
    thumb_recovery: Option<NonNull<ThumbBlockRecoveryAccounting>>,
    translation_callback: Cell<Option<usize>>,
    /// Context of the installed code-store observer, kept while it is
    /// installed: the engine holds its address until the observer is removed.
    memory: SharedMemory,
    pause_requested: Arc<AtomicU32>,
    callbacks: RefCell<BTreeMap<usize, CallbackEntry>>,
    hooks: RefCell<BTreeMap<usize, RemoteHook>>,
    next_callback: Cell<usize>,
    next_run: Cell<u64>,
    running: Cell<bool>,
    ordered_run: Cell<bool>,
    retried_block: Cell<bool>,
    active_run: Cell<Option<RunToken>>,
    stopped_progress: Cell<Option<u64>>,
    stopped_pc: Cell<Option<u64>>,
    stopped_pstate: Cell<Option<u64>>,
    loop_window: Cell<Option<bool>>,
    pending_callback: RefCell<
        Option<(
            u64,
            usize,
            CallbackCall,
            Option<u64>,
            Option<u64>,
            Option<u64>,
        )>,
    >,
    run_result: RefCell<Option<Result<Exit, ExecutionError>>>,
    callback_depth: Cell<u32>,
    deferred_instruction_limit: Cell<Option<u64>>,
    _owner: PhantomData<Rc<()>>,
}

struct RemoteHook {
    remote: HookId,
    callback: usize,
}

/// The code-store observer as the engine thread needs it: the callback and the
/// owner's user pointer, which is all `crate::code_store` reads.
#[derive(Clone, Copy)]
struct CodeStoreContext {
    callback: unsafe fn(*mut c_void, u64, usize),
    data: *mut c_void,
}
unsafe impl Send for CodeStoreContext {}

enum CallbackEntry {
    Mmio(Mmio),
    Hook(Hook),
    Translation(TranslationHooks),
}

struct RemoteSnapshot {
    owner: Identity,
    token: u64,
    commands: Sender<Message>,
}
impl Snapshot for RemoteSnapshot {
    fn owner(&self) -> Identity {
        self.owner
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn export(&self) -> Result<Vec<u8>, String> {
        self.export_bytes()
    }
}
impl Drop for RemoteSnapshot {
    fn drop(&mut self) {
        let _ = self.commands.send(Message::ReleaseSnapshot(self.token));
    }
}

/// Start a dedicated owner thread and construct the Unicorn-backed Core there.
pub fn spawn(config: Config) -> Result<CpuWorker, String> {
    spawn_engine(
        EngineConfig::Rprofile(config.options),
        config.translation_cache_size_bytes,
        config.memory,
    )
}

#[cfg(test)]
pub fn spawn_armv7a(
    translation_cache_size_bytes: u32,
    memory: SharedMemory,
) -> Result<CpuWorker, String> {
    spawn_engine(
        EngineConfig::Armv7a,
        translation_cache_size_bytes,
        memory,
    )
}

pub fn spawn_aprofile(
    translation_cache_size_bytes: u32,
    memory: SharedMemory,
) -> Result<CpuWorker, String> {
    spawn_engine(EngineConfig::Aprofile, translation_cache_size_bytes, memory)
}

/// A board that provides its own mixed-width instruction accounting can
/// retain that accounting while moving the native M-profile CPU to a worker.
pub fn spawn_mprofile(
    translation_cache_size_bytes: u32,
    owner_accounting: bool,
) -> Result<CpuWorker, String> {
    spawn_engine(
        EngineConfig::Mprofile { owner_accounting },
        translation_cache_size_bytes,
        SharedMemory::default(),
    )
}

enum EngineConfig {
    Rprofile(core::Options),
    /// An A32 core without block accounting, for tests that need a 32-bit
    /// worker with only the code-store observer installed.
    #[cfg(test)]
    Armv7a,
    Aprofile,
    Mprofile { owner_accounting: bool },
}

enum Engine {
    #[cfg(test)]
    Raw(crate::Core),
    Rprofile(core::Core),
    Aprofile(aprofile::Core),
    Mprofile {
        raw: crate::Core,
        owner_accounting: bool,
    },
}

impl Engine {
    fn cpu(&self) -> &dyn Cpu {
        match self {
            #[cfg(test)]
            Self::Raw(cpu) => cpu,
            Self::Rprofile(cpu) => cpu,
            Self::Aprofile(cpu) => cpu,
            Self::Mprofile { raw, .. } => raw,
        }
    }

    fn callback_parked(&self, parked: bool) -> Result<(), String> {
        match self {
            #[cfg(test)]
            Self::Raw(_) => Ok(()),
            Self::Rprofile(cpu) => cpu.owner_callback_parked(parked),
            Self::Aprofile(cpu) => cpu.owner_callback_parked(parked),
            Self::Mprofile { .. } => Ok(()),
        }
    }

    fn progress(&self) -> Option<u64> {
        match self {
            Self::Mprofile {
                raw,
                owner_accounting: false,
            } => raw.instruction_counter().ok(),
            Self::Mprofile { .. } => None,
            _ => self.cpu().instruction_progress(),
        }
    }

    fn run(&self, request: Run) -> Result<Exit, ExecutionError> {
        if let Self::Mprofile {
            raw,
            owner_accounting: false,
        } = self
        {
            raw.reset_instruction_counter()?;
        }
        self.cpu().run(request)
    }
}

struct Ready {
    identity: Identity,
    features: Features,
    pause_requested: Arc<AtomicU32>,
    thumb_accounting: Option<usize>,
    thumb_recovery: Option<usize>,
}

fn spawn_engine(
    config: EngineConfig,
    cache_bytes: u32,
    memory: SharedMemory,
) -> Result<CpuWorker, String> {
    let model = match &config {
        EngineConfig::Rprofile(_) => Model::CortexR5F,
        #[cfg(test)]
        EngineConfig::Armv7a => Model::CortexA7,
        EngineConfig::Aprofile => Model::ArmV8A,
        EngineConfig::Mprofile { .. } => Model::CortexM4,
    };
    let owner_accounting = matches!(
        config,
        EngineConfig::Mprofile {
            owner_accounting: true
        }
    );
    let worker_memory = memory.clone();
    let (commands, command_rx) = mpsc::channel();
    let (event_tx, events) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name(format!("unicorn-{model:?}"))
        .spawn(move || {
            serve(
                config,
                model,
                cache_bytes,
                worker_memory,
                command_rx,
                event_tx,
                ready_tx,
            )
        })
        .map_err(|error| format!("start Unicorn CPU worker: {error}"))?;
    let ready = match ready_rx.recv() {
        Ok(ready) => ready,
        Err(_) => {
            let _ = worker.join();
            return Err("Unicorn CPU worker disconnected during construction".into());
        }
    };
    match ready {
        Ok(ready) => {
            let mut features = ready.features;
            features.asynchronous_runs = true;
            if model == Model::CortexM4 {
                features.instruction_progress = !owner_accounting;
            }
            Ok(CpuWorker {
                #[cfg(test)]
                rpc_calls: Cell::new(0),
                rpc_counts: std::array::from_fn(|_| Cell::new(0)),
                callback_events: Cell::new(0),
                callback_counts: std::array::from_fn(|_| Cell::new(0)),
                commands,
                events: RefCell::new(events),
                worker: RefCell::new(Some(worker)),
                identity: ready.identity,
                model,
                owner_accounting,
                execution_policy: Cell::new(ExecutionPolicy::Exact),
                features,
                memory,
                pause_requested: ready.pause_requested,
                thumb_accounting: ready
                    .thumb_accounting
                    .and_then(|p| NonNull::new(p as *mut _)),
                thumb_recovery: ready.thumb_recovery.and_then(|p| NonNull::new(p as *mut _)),
                translation_callback: Cell::new(None),
                callbacks: RefCell::new(BTreeMap::new()),
                hooks: RefCell::new(BTreeMap::new()),
                next_callback: Cell::new(1),
                next_run: Cell::new(1),
                running: Cell::new(false),
                ordered_run: Cell::new(false),
                retried_block: Cell::new(false),
                active_run: Cell::new(None),
                stopped_progress: Cell::new(None),
                stopped_pc: Cell::new(None),
                stopped_pstate: Cell::new(None),
                loop_window: Cell::new(Some(false)),
                pending_callback: RefCell::new(None),
                run_result: RefCell::new(None),
                callback_depth: Cell::new(0),
                deferred_instruction_limit: Cell::new(None),
                _owner: PhantomData,
            })
        }
        Err(error) => {
            let _ = worker.join();
            Err(error)
        }
    }
}

enum Message {
    Request {
        operation: Operation,
        reply: Sender<Result<Value, String>>,
    },
    Run {
        id: u64,
        request: Run,
    },
    CallbackResponse {
        id: u64,
        result: CallbackResult,
        tighten: Option<u64>,
        applied: Option<Sender<Result<(), String>>>,
    },
    ReleaseSnapshot(u64),
    SetRunWakeup(RunWakeup),
    Shutdown,
}

enum Event {
    Callback {
        id: u64,
        callback: usize,
        call: CallbackCall,
        progress: Option<u64>,
        pc: Option<u64>,
        pstate: Option<u64>,
    },
    RunComplete {
        id: u64,
        result: Result<Exit, ExecutionError>,
        progress: Option<u64>,
        pc: Option<u64>,
        pstate: Option<u64>,
    },
}

enum CallbackCall {
    MmioRead {
        offset: u64,
        size: u32,
    },
    MmioWrite {
        offset: u64,
        size: u32,
        value: u64,
    },
    Instruction {
        address: u64,
        size: u32,
    },
    Block(Block),
    Exception(arm::Exception),
    Synchronization,
    Classify {
        address: u64,
        word: u32,
    },
    Restart(u64),
    Memory {
        access: Access,
        address: u64,
        size: i32,
        value: i64,
    },
    Fault {
        fault: Fault,
        address: u64,
        size: i32,
        value: i64,
    },
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum WorkerCallbackCounter {
    MmioRead,
    MmioWrite,
    Instruction,
    Block,
    Exception,
    Synchronization,
    Memory,
    Fault,
    Classify,
    Restart,
}

impl WorkerCallbackCounter {
    const COUNT: usize = Self::Restart as usize + 1;
    const ALL: [Self; Self::COUNT] = [
        Self::MmioRead,
        Self::MmioWrite,
        Self::Instruction,
        Self::Block,
        Self::Exception,
        Self::Synchronization,
        Self::Memory,
        Self::Fault,
        Self::Classify,
        Self::Restart,
    ];

    fn for_call(call: &CallbackCall) -> Self {
        match call {
            CallbackCall::MmioRead { .. } => Self::MmioRead,
            CallbackCall::MmioWrite { .. } => Self::MmioWrite,
            CallbackCall::Instruction { .. } => Self::Instruction,
            CallbackCall::Block(_) => Self::Block,
            CallbackCall::Exception(_) => Self::Exception,
            CallbackCall::Synchronization => Self::Synchronization,
            CallbackCall::Classify { .. } => Self::Classify,
            CallbackCall::Restart(_) => Self::Restart,
            CallbackCall::Memory { .. } => Self::Memory,
            CallbackCall::Fault { .. } => Self::Fault,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::MmioRead => "worker_callback_mmio_read",
            Self::MmioWrite => "worker_callback_mmio_write",
            Self::Instruction => "worker_callback_instruction",
            Self::Block => "worker_callback_block",
            Self::Exception => "worker_callback_exception",
            Self::Synchronization => "worker_callback_synchronization",
            Self::Memory => "worker_callback_memory",
            Self::Fault => "worker_callback_fault",
            Self::Classify => "worker_callback_classify",
            Self::Restart => "worker_callback_restart",
        }
    }
}

#[derive(Clone, Copy)]
enum CallbackResult {
    Unit,
    U64(u64),
    Bool(bool),
    Translation(Translation),
    Error,
}

enum Operation {
    SetLoopMode(loops::LoopMode),
    SetLoopWindow(bool),
    SetExecutionPolicy(ExecutionPolicy),
    CheckpointBackings,
    SetGuestClock(Clock),
    ReadRegister(Register, usize),
    WriteRegister(Register, Vec<u8>),
    Coprocessor(arm::Coprocessor, Option<u64>),
    ReadMemory(u64, usize),
    Translate(u64, Access),
    ReadVirtual(u64, Access, usize),
    WriteMemory(u64, Vec<u8>),
    MemoryWritten(u64, usize),
    BackingWritten(u64, usize, usize),
    MemoryRegions,
    MapBacking(u64, usize, u32, usize),
    MapPhysical(u64, u64, usize, u32, usize),
    ReplacePhysical(u64, u64, usize, u32, usize),
    MapZeroed(u64, usize, u32),
    MapMmio(u64, usize, usize, Vec<FastMmioRead>),
    Unmap(u64, usize),
    Protect(u64, usize, u32),
    InvalidateCode(Option<(u64, u64)>),
    TightenInstructionLimit(u64),
    Progress,
    Stop,
    StopBeforeInstruction,
    RprofileIdentity(Option<u32>),
    SetInterrupt(Interrupt, bool),
    Snapshot(Option<u64>),
    Restore(u64),
    ExportSnapshot(u64),
    ImportSnapshot(Vec<u8>),
    InstallHooks(Vec<(usize, HookKind)>),
    RemoveHooks(Vec<(HookId, usize)>),
    ObserveBackings(Vec<BackingRange>),
    WatchStores(u64, u64),
    HookWatched(HookId, bool),
    PreciseStorePc(bool),
    BackingPointer(u64, usize),
    MemoryRestartBoundary(HookRange),
    TranslationHooks(Option<usize>),
    CodeStore(Option<CodeStoreContext>),
    RetryBlock(u32),
    Restart,
    StopOnEl3Transitions,
    ExitCounters,
    #[cfg(test)]
    CallbackContexts,
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum WorkerRpcCounter {
    SetLoopMode,
    SetLoopWindow,
    SetExecutionPolicy,
    CheckpointBackings,
    SetGuestClock,
    ReadRegister,
    WriteRegister,
    Coprocessor,
    ReadMemory,
    Translate,
    ReadVirtual,
    WriteMemory,
    MemoryWritten,
    BackingWritten,
    MemoryRegions,
    MapBacking,
    MapPhysical,
    ReplacePhysical,
    MapZeroed,
    MapMmio,
    Unmap,
    Protect,
    InvalidateCode,
    TightenInstructionLimit,
    Progress,
    Stop,
    StopBeforeInstruction,
    RprofileIdentity,
    SetInterrupt,
    Snapshot,
    Restore,
    InstallHooks,
    RemoveHooks,
    ObserveBackings,
    WatchStores,
    HookWatched,
    PreciseStorePc,
}

impl WorkerRpcCounter {
    const COUNT: usize = Self::PreciseStorePc as usize + 1;
    const ALL: [Self; Self::COUNT] = [
        Self::SetLoopMode,
        Self::SetLoopWindow,
        Self::SetExecutionPolicy,
        Self::CheckpointBackings,
        Self::SetGuestClock,
        Self::ReadRegister,
        Self::WriteRegister,
        Self::Coprocessor,
        Self::ReadMemory,
        Self::Translate,
        Self::ReadVirtual,
        Self::WriteMemory,
        Self::MemoryWritten,
        Self::BackingWritten,
        Self::MemoryRegions,
        Self::MapBacking,
        Self::MapPhysical,
        Self::ReplacePhysical,
        Self::MapZeroed,
        Self::MapMmio,
        Self::Unmap,
        Self::Protect,
        Self::InvalidateCode,
        Self::TightenInstructionLimit,
        Self::Progress,
        Self::Stop,
        Self::StopBeforeInstruction,
        Self::RprofileIdentity,
        Self::SetInterrupt,
        Self::Snapshot,
        Self::Restore,
        Self::InstallHooks,
        Self::RemoveHooks,
        Self::ObserveBackings,
        Self::WatchStores,
        Self::HookWatched,
        Self::PreciseStorePc,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::SetLoopMode => "worker_rpc_set_loop_mode",
            Self::SetLoopWindow => "worker_rpc_set_loop_window",
            Self::SetExecutionPolicy => "worker_rpc_set_execution_policy",
            Self::CheckpointBackings => "worker_rpc_checkpoint_backings",
            Self::SetGuestClock => "worker_rpc_set_guest_clock",
            Self::ReadRegister => "worker_rpc_read_register",
            Self::WriteRegister => "worker_rpc_write_register",
            Self::Coprocessor => "worker_rpc_coprocessor",
            Self::ReadMemory => "worker_rpc_read_memory",
            Self::Translate => "worker_rpc_translate",
            Self::ReadVirtual => "worker_rpc_read_virtual",
            Self::WriteMemory => "worker_rpc_write_memory",
            Self::MemoryWritten => "worker_rpc_memory_written",
            Self::BackingWritten => "worker_rpc_backing_written",
            Self::MemoryRegions => "worker_rpc_memory_regions",
            Self::MapBacking => "worker_rpc_map_backing",
            Self::MapPhysical => "worker_rpc_map_physical",
            Self::ReplacePhysical => "worker_rpc_replace_physical",
            Self::MapZeroed => "worker_rpc_map_zeroed",
            Self::MapMmio => "worker_rpc_map_mmio",
            Self::Unmap => "worker_rpc_unmap",
            Self::Protect => "worker_rpc_protect",
            Self::InvalidateCode => "worker_rpc_invalidate_code",
            Self::TightenInstructionLimit => "worker_rpc_tighten_instruction_limit",
            Self::Progress => "worker_rpc_progress",
            Self::Stop => "worker_rpc_stop",
            Self::StopBeforeInstruction => "worker_rpc_stop_before_instruction",
            Self::RprofileIdentity => "worker_rpc_rprofile_identity",
            Self::SetInterrupt => "worker_rpc_set_interrupt",
            Self::Snapshot => "worker_rpc_snapshot",
            Self::Restore => "worker_rpc_restore",
            Self::InstallHooks => "worker_rpc_install_hooks",
            Self::RemoveHooks => "worker_rpc_remove_hooks",
            Self::ObserveBackings => "worker_rpc_observe_backings",
            Self::WatchStores => "worker_rpc_watch_stores",
            Self::HookWatched => "worker_rpc_hook_watched",
            Self::PreciseStorePc => "worker_rpc_precise_store_pc",
        }
    }
}

impl Operation {
    fn counter(&self) -> Option<WorkerRpcCounter> {
        Some(match self {
            Self::SetLoopMode(_) => WorkerRpcCounter::SetLoopMode,
            Self::SetLoopWindow(_) => WorkerRpcCounter::SetLoopWindow,
            Self::SetExecutionPolicy(_) => WorkerRpcCounter::SetExecutionPolicy,
            Self::CheckpointBackings => WorkerRpcCounter::CheckpointBackings,
            Self::SetGuestClock(_) => WorkerRpcCounter::SetGuestClock,
            Self::ReadRegister(_, _) => WorkerRpcCounter::ReadRegister,
            Self::WriteRegister(_, _) => WorkerRpcCounter::WriteRegister,
            Self::Coprocessor(_, _) => WorkerRpcCounter::Coprocessor,
            Self::ReadMemory(_, _) => WorkerRpcCounter::ReadMemory,
            Self::Translate(_, _) => WorkerRpcCounter::Translate,
            Self::ReadVirtual(_, _, _) => WorkerRpcCounter::ReadVirtual,
            Self::WriteMemory(_, _) => WorkerRpcCounter::WriteMemory,
            Self::MemoryWritten(_, _) => WorkerRpcCounter::MemoryWritten,
            Self::BackingWritten(_, _, _) => WorkerRpcCounter::BackingWritten,
            Self::MemoryRegions => WorkerRpcCounter::MemoryRegions,
            Self::MapBacking(_, _, _, _) => WorkerRpcCounter::MapBacking,
            Self::MapPhysical(_, _, _, _, _) => WorkerRpcCounter::MapPhysical,
            Self::ReplacePhysical(_, _, _, _, _) => WorkerRpcCounter::ReplacePhysical,
            Self::MapZeroed(_, _, _) => WorkerRpcCounter::MapZeroed,
            Self::MapMmio(_, _, _, _) => WorkerRpcCounter::MapMmio,
            Self::Unmap(_, _) => WorkerRpcCounter::Unmap,
            Self::Protect(_, _, _) => WorkerRpcCounter::Protect,
            Self::InvalidateCode(_) => WorkerRpcCounter::InvalidateCode,
            Self::TightenInstructionLimit(_) => WorkerRpcCounter::TightenInstructionLimit,
            Self::Progress => WorkerRpcCounter::Progress,
            Self::Stop => WorkerRpcCounter::Stop,
            Self::StopBeforeInstruction => WorkerRpcCounter::StopBeforeInstruction,
            Self::RprofileIdentity(_) => WorkerRpcCounter::RprofileIdentity,
            Self::SetInterrupt(_, _) => WorkerRpcCounter::SetInterrupt,
            Self::Snapshot(_) => WorkerRpcCounter::Snapshot,
            Self::Restore(_) => WorkerRpcCounter::Restore,
            Self::ExportSnapshot(_) | Self::ImportSnapshot(_) => return None,
            Self::InstallHooks(_) => WorkerRpcCounter::InstallHooks,
            Self::RemoveHooks(_) => WorkerRpcCounter::RemoveHooks,
            Self::ObserveBackings(_) => WorkerRpcCounter::ObserveBackings,
            Self::WatchStores(_, _) => WorkerRpcCounter::WatchStores,
            Self::HookWatched(_, _) => WorkerRpcCounter::HookWatched,
            Self::PreciseStorePc(_) => WorkerRpcCounter::PreciseStorePc,
            Self::BackingPointer(_, _)
            | Self::MemoryRestartBoundary(_)
            | Self::TranslationHooks(_)
            | Self::CodeStore(_)
            | Self::RetryBlock(_)
            | Self::Restart
            | Self::StopOnEl3Transitions
            | Self::ExitCounters => return None,
            #[cfg(test)]
            Self::CallbackContexts => return None,
        })
    }
}

#[derive(Clone, Copy)]
enum HookKind {
    Instruction(HookRange),
    Block(HookRange),
    Exception,
    Synchronization,
    CompletedStore(HookRange),
    Memory {
        read: bool,
        write: bool,
        range: HookRange,
    },
    Fault,
}

#[derive(Clone, Copy)]
struct BackingRange {
    backing: usize,
    bytes: usize,
    bitmap: usize,
    page_shift: u32,
    summary: Option<usize>,
}

enum Value {
    Unit,
    U64(u64),
    OptionalU64(Option<u64>),
    Bytes(Vec<u8>),
    Backings(Vec<(usize, usize)>),
    MemoryRegions(Vec<MemoryRegion>),
    Hooks(Vec<HookId>),
    Snapshot(u64),
    Identity(u32),
    Counters(Vec<(&'static str, u64)>),
}

struct CallbackContext {
    loop_state: *const WorkerLoop,
    callback: usize,
}
struct WorkerLoop {
    commands: RefCell<Receiver<Message>>,
    events: Sender<Event>,
    core: Cell<*const Engine>,
    pause_requested: Arc<AtomicU32>,
    translation_callback: Cell<Option<usize>>,
    /// Context of the installed code-store observer, kept while it is
    /// installed: the engine holds its address until the observer is removed.
    code_store: RefCell<Option<Box<CodeStoreContext>>>,
    virtual_hooks: RefCell<std::collections::BTreeSet<usize>>,
    snapshots: RefCell<HashMap<u64, Box<dyn Snapshot>>>,
    callback_contexts: RefCell<BTreeMap<usize, Box<CallbackContext>>>,
    next_snapshot: Cell<u64>,
    next_event: Cell<u64>,
    callback_error: RefCell<Option<String>>,
    wakeup: RefCell<Option<RunWakeup>>,
    shutdown: Cell<bool>,
}
fn read_scalar(cpu: &dyn Cpu, register: Register) -> Option<u64> {
    let mut bytes = [0; 8];
    match cpu.read_register(register, &mut bytes).ok()? {
        4 | 8 => Some(u64::from_le_bytes(bytes)),
        _ => None,
    }
}
impl WorkerLoop {
    fn notify_wakeup(&self) {
        if let Some(wakeup) = self.wakeup.borrow().as_ref() {
            wakeup.notify();
        }
    }
    fn callback_context(&self, callback: usize) -> *mut std::ffi::c_void {
        let mut contexts = self.callback_contexts.borrow_mut();
        contexts
            .entry(callback)
            .or_insert_with(|| {
                Box::new(CallbackContext {
                    loop_state: self,
                    callback,
                })
            })
            .as_mut() as *mut CallbackContext as *mut std::ffi::c_void
    }
    fn wait_callback(&self, callback: usize, call: CallbackCall) -> CallbackResult {
        let Some(engine) = (unsafe { self.core.get().as_ref() }) else {
            return CallbackResult::Error;
        };
        let cpu = engine.cpu();
        if let Err(error) = engine.callback_parked(true) {
            *self.callback_error.borrow_mut() = Some(error);
            cpu.stop().ok();
            return CallbackResult::Error;
        }
        let progress = engine.progress();
        let pc = read_scalar(cpu, Register::Pc);
        let pstate = read_scalar(cpu, Register::Pstate);
        let is_synchronization = matches!(call, CallbackCall::Synchronization);
        let result = self.wait_callback_parked(engine, callback, call, progress, pc, pstate);
        if is_synchronization
            && !matches!(result, CallbackResult::Error)
            && let Engine::Rprofile(cpu) = engine
        {
            cpu.acknowledge_owner_barrier();
        }
        if let Err(error) = engine.callback_parked(false) {
            *self.callback_error.borrow_mut() = Some(error);
            cpu.stop().ok();
            return CallbackResult::Error;
        }
        result
    }

    fn wait_callback_parked(
        &self,
        engine: &Engine,
        callback: usize,
        call: CallbackCall,
        progress: Option<u64>,
        pc: Option<u64>,
        pstate: Option<u64>,
    ) -> CallbackResult {
        let cpu = engine.cpu();
        let id = self.next_event.get();
        self.next_event.set(id.wrapping_add(1).max(1));
        if self
            .events
            .send(Event::Callback {
                id,
                callback,
                call,
                progress,
                pc,
                pstate,
            })
            .is_err()
        {
            return CallbackResult::Error;
        }
        self.notify_wakeup();
        loop {
            let message = match self.commands.borrow_mut().recv() {
                Ok(message) => message,
                Err(_) => return CallbackResult::Error,
            };
            match message {
                Message::CallbackResponse {
                    id: response_id,
                    result,
                    tighten,
                    applied,
                } if response_id == id => {
                    let apply = if !matches!(result, CallbackResult::Error) {
                        tighten.map_or(Ok(()), |total| cpu.tighten_instruction_limit(total))
                    } else {
                        Ok(())
                    };
                    if let Some(applied) = applied {
                        let _ = applied.send(apply.clone());
                    }
                    if let Err(error) = apply {
                        *self.callback_error.borrow_mut() = Some(error);
                        cpu.stop().ok();
                        return CallbackResult::Error;
                    }
                    if matches!(result, CallbackResult::Error) {
                        *self.callback_error.borrow_mut() =
                            Some("owner callback failed while servicing Unicorn CPU".into());
                        cpu.stop().ok();
                    }
                    return result;
                }
                Message::Request { operation, reply } => {
                    let result = dispatch(self, engine, operation);
                    let _ = reply.send(result);
                }
                Message::ReleaseSnapshot(token) => {
                    self.snapshots.borrow_mut().remove(&token);
                }
                Message::SetRunWakeup(wakeup) => {
                    *self.wakeup.borrow_mut() = Some(wakeup);
                }
                Message::Run { id, .. } => {
                    let _ = self.events.send(Event::RunComplete {
                        id,
                        result: Err(ExecutionError::Backend(
                            "nested Unicorn worker execution is refused".into(),
                        )),
                        progress: engine.progress(),
                        pc: read_scalar(cpu, Register::Pc),
                        pstate: read_scalar(cpu, Register::Pstate),
                    });
                    self.notify_wakeup();
                }
                Message::Shutdown => {
                    self.shutdown.set(true);
                    cpu.stop().ok();
                    return CallbackResult::Error;
                }
                Message::CallbackResponse { .. } => {
                    return CallbackResult::Error;
                }
            }
        }
    }
}

unsafe fn callback_context(data: *mut std::ffi::c_void) -> &'static CallbackContext {
    unsafe { &*data.cast::<CallbackContext>() }
}
unsafe fn callback_loop(data: *mut std::ffi::c_void) -> &'static WorkerLoop {
    unsafe { &*callback_context(data).loop_state }
}
unsafe fn mmio_read(offset: u64, size: u32, data: *mut std::ffi::c_void) -> u64 {
    let context = unsafe { callback_context(data) };
    match unsafe {
        callback_loop(data).wait_callback(context.callback, CallbackCall::MmioRead { offset, size })
    } {
        CallbackResult::U64(value) => value,
        _ => 0,
    }
}
unsafe fn mmio_write(offset: u64, size: u32, value: u64, data: *mut std::ffi::c_void) {
    let context = unsafe { callback_context(data) };
    unsafe {
        callback_loop(data).wait_callback(
            context.callback,
            CallbackCall::MmioWrite {
                offset,
                size,
                value,
            },
        );
    }
}
unsafe fn instruction(address: u64, size: u32, data: *mut std::ffi::c_void) {
    let context = unsafe { callback_context(data) };
    unsafe {
        callback_loop(data).wait_callback(
            context.callback,
            CallbackCall::Instruction { address, size },
        );
    }
}
unsafe fn block(block: Block, data: *mut std::ffi::c_void) {
    let context = unsafe { callback_context(data) };
    unsafe {
        callback_loop(data).wait_callback(context.callback, CallbackCall::Block(block));
    }
}
unsafe fn exception(exception: arm::Exception, data: *mut std::ffi::c_void) {
    let context = unsafe { callback_context(data) };
    unsafe {
        callback_loop(data).wait_callback(context.callback, CallbackCall::Exception(exception));
    }
}
unsafe fn synchronization(data: *mut std::ffi::c_void) {
    let context = unsafe { callback_context(data) };
    unsafe {
        callback_loop(data).wait_callback(context.callback, CallbackCall::Synchronization);
    }
}
unsafe fn memory(access: Access, address: u64, size: i32, value: i64, data: *mut std::ffi::c_void) {
    let context = unsafe { callback_context(data) };
    unsafe {
        callback_loop(data).wait_callback(
            context.callback,
            CallbackCall::Memory {
                access,
                address,
                size,
                value,
            },
        );
    }
}
unsafe fn fault(
    fault: Fault,
    address: u64,
    size: i32,
    value: i64,
    data: *mut std::ffi::c_void,
) -> bool {
    let context = unsafe { callback_context(data) };
    match unsafe {
        callback_loop(data).wait_callback(
            context.callback,
            CallbackCall::Fault {
                fault,
                address,
                size,
                value,
            },
        )
    } {
        CallbackResult::Bool(value) => value,
        _ => false,
    }
}

unsafe fn classify(data: *mut std::ffi::c_void, address: u64, word: u32) -> Translation {
    let context = unsafe { callback_context(data) };
    match unsafe {
        callback_loop(data)
            .wait_callback(context.callback, CallbackCall::Classify { address, word })
    } {
        CallbackResult::Translation(translation) => translation,
        _ => Translation {
            instruction: InstructionDisposition::ObserveAndEndBlock,
            observe_block: true,
        },
    }
}

unsafe fn restart(data: *mut std::ffi::c_void, pc: u64) {
    let context = unsafe { callback_context(data) };
    unsafe {
        callback_loop(data).wait_callback(context.callback, CallbackCall::Restart(pc));
    }
}

unsafe fn pause_at_block(_: Block, data: *mut std::ffi::c_void) {
    let context = unsafe { &*data.cast::<WorkerLoop>() };
    if context.pause_requested.load(Ordering::Acquire) != 0
        && let Some(engine) = unsafe { context.core.get().as_ref() }
    {
        engine.cpu().stop().ok();
    }
}

fn serve(
    config: EngineConfig,
    model: Model,
    cache_bytes: u32,
    memory: SharedMemory,
    commands: Receiver<Message>,
    events: Sender<Event>,
    ready: Sender<Result<Ready, String>>,
) {
    let mut context = Box::new(WorkerLoop {
        commands: RefCell::new(commands),
        events: events.clone(),
        core: Cell::new(std::ptr::null()),
        pause_requested: Arc::new(AtomicU32::new(0)),
        translation_callback: Cell::new(None),
        virtual_hooks: RefCell::default(),
        snapshots: RefCell::new(HashMap::new()),
        callback_contexts: RefCell::new(BTreeMap::new()),
        next_snapshot: Cell::new(1),
        next_event: Cell::new(1),
        callback_error: RefCell::new(None),
        wakeup: RefCell::new(None),
        shutdown: Cell::new(false),
            code_store: RefCell::new(None),
    });
    let engine = (|| -> Result<Engine, String> {
        let raw = Library::linked()?.open(model, cache_bytes)?;
        Ok(match config {
            #[cfg(test)]
            EngineConfig::Armv7a => Engine::Raw(raw),
            EngineConfig::Rprofile(options) => Engine::Rprofile(core::Core::new_parallel_worker(
                raw,
                options,
                core::Wiring {
                    memory: memory.processor(),
                },
            )?),
            EngineConfig::Aprofile => Engine::Aprofile(aprofile::Core::new_parallel_worker(
                raw,
                memory.processor(),
                context.pause_requested.clone(),
            )?),
            EngineConfig::Mprofile { owner_accounting } => {
                let mut record = raw
                    .accounting()
                    .ok_or("M-profile worker requires native block accounting")?;
                unsafe {
                    record.as_mut().pause_requested = Arc::as_ptr(&context.pause_requested);
                    raw.install_hook(Hook::Block {
                        range: HookRange::All,
                        callback: pause_at_block,
                        data: (&raw const *context).cast_mut().cast(),
                    })?;
                }
                if !owner_accounting {
                    raw.set_instruction_counter(true)?;
                }
                Engine::Mprofile {
                    raw,
                    owner_accounting,
                }
            }
        })
    })();
    let engine = match engine {
        Ok(engine) => engine,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if let Engine::Rprofile(cpu) = &engine {
        context.pause_requested = cpu.pause_handle();
    }
    context.core.set(&engine);
    let cpu = engine.cpu();
    if ready
        .send(Ok(Ready {
            identity: cpu.identity(),
            features: cpu.features(),
            pause_requested: context.pause_requested.clone(),
            thumb_accounting: cpu.thumb_accounting().map(|p| p.as_ptr() as usize),
            thumb_recovery: cpu.thumb_recovery_accounting().map(|p| p.as_ptr() as usize),
        }))
        .is_err()
    {
        return;
    }
    loop {
        let message = match context.commands.borrow_mut().recv() {
            Ok(message) => message,
            Err(_) => break,
        };
        match message {
            Message::Request { operation, reply } => {
                let _ = reply.send(dispatch(&context, &engine, operation));
            }
            Message::Run { id, request } => {
                context.callback_error.borrow_mut().take();
                let mut result = engine.run(request);
                if let Some(error) = context.callback_error.borrow_mut().take() {
                    result = Err(ExecutionError::Backend(error));
                }
                let progress = engine.progress();
                let pc = read_scalar(cpu, Register::Pc);
                let pstate = read_scalar(cpu, Register::Pstate);
                let _ = events.send(Event::RunComplete {
                    id,
                    result,
                    progress,
                    pc,
                    pstate,
                });
                context.notify_wakeup();
                if context.shutdown.get() {
                    break;
                }
            }
            Message::ReleaseSnapshot(token) => {
                context.snapshots.borrow_mut().remove(&token);
            }
            Message::SetRunWakeup(wakeup) => {
                *context.wakeup.borrow_mut() = Some(wakeup);
            }
            Message::Shutdown => break,
            Message::CallbackResponse { .. } => break,
        }
    }
    context.core.set(std::ptr::null());
}

fn dispatch(context: &WorkerLoop, engine: &Engine, operation: Operation) -> Result<Value, String> {
    let cpu = engine.cpu();
    let result = match operation {
        #[cfg(test)]
        Operation::CallbackContexts => Value::U64(context.callback_contexts.borrow().len() as u64),
        Operation::SetLoopMode(mode) => {
            cpu.set_loop_mode(mode)?;
            Value::Unit
        }
        Operation::SetLoopWindow(stable) => {
            cpu.set_loop_window(stable)?;
            Value::Unit
        }
        Operation::SetExecutionPolicy(policy) => {
            if let Engine::Mprofile { raw, .. } = engine {
                raw.set_parallel_cpus(policy == ExecutionPolicy::Interactive)?;
            }
            cpu.set_execution_policy(policy)?;
            Value::Unit
        }
        Operation::CheckpointBackings => Value::Backings(
            cpu.checkpoint_backings()?
                .into_iter()
                .map(|(pointer, bytes)| (pointer.as_ptr() as usize, bytes))
                .collect(),
        ),
        Operation::SetGuestClock(clock) => {
            cpu.set_guest_clock(clock)?;
            Value::Unit
        }
        Operation::ReadRegister(register, bytes) => {
            let mut out = vec![0; bytes];
            let read = cpu.read_register(register, &mut out)?;
            out.truncate(read);
            Value::Bytes(out)
        }
        Operation::WriteRegister(register, bytes) => {
            cpu.write_register(register, &bytes)?;
            Value::Unit
        }
        Operation::Coprocessor(register, value) => Value::U64(cpu.coprocessor(register, value)?),
        Operation::ReadMemory(address, bytes) => {
            let mut out = vec![0; bytes];
            cpu.read_memory(address, &mut out)?;
            Value::Bytes(out)
        }
        Operation::Translate(address, access) => Value::U64(cpu.translate(address, access)?),
        Operation::ReadVirtual(address, access, bytes) => {
            let mut out = vec![0; bytes];
            cpu.read_virtual(address, access, &mut out)?;
            Value::Bytes(out)
        }
        Operation::WriteMemory(address, bytes) => {
            cpu.write_memory(address, &bytes)?;
            Value::Unit
        }
        Operation::MemoryWritten(address, bytes) => {
            cpu.memory_written(address, bytes)?;
            Value::Unit
        }
        Operation::BackingWritten(physical, pointer, bytes) => {
            unsafe {
                cpu.backing_written(
                    physical,
                    NonNull::new(pointer as *mut u8).ok_or("null backing pointer")?,
                    bytes,
                )?;
            }
            Value::Unit
        }
        Operation::MemoryRegions => Value::MemoryRegions(cpu.memory_regions()?),
        Operation::MapBacking(address, bytes, permissions, pointer) => {
            unsafe {
                cpu.map_backing(
                    address,
                    bytes,
                    permissions,
                    NonNull::new(pointer as *mut u8).ok_or("null backing pointer")?,
                )?;
            }
            Value::Unit
        }
        Operation::MapPhysical(address, physical, bytes, permissions, pointer) => {
            unsafe {
                cpu.map_physical(
                    address,
                    physical,
                    bytes,
                    permissions,
                    NonNull::new(pointer as *mut u8).ok_or("null backing pointer")?,
                )?;
            }
            Value::Unit
        }
        Operation::ReplacePhysical(address, physical, bytes, permissions, pointer) => {
            unsafe {
                cpu.replace_physical(
                    address,
                    physical,
                    bytes,
                    permissions,
                    NonNull::new(pointer as *mut u8).ok_or("null backing pointer")?,
                )?;
            }
            Value::Unit
        }
        Operation::MapZeroed(address, bytes, permissions) => {
            cpu.map_zeroed(address, bytes, permissions)?;
            Value::Unit
        }
        Operation::MapMmio(address, bytes, callback, fast_reads) => {
            let data = context.callback_context(callback);
            let callbacks = Mmio {
                read: mmio_read,
                write: mmio_write,
                data,
            };
            unsafe {
                if !fast_reads.is_empty() {
                    cpu.map_mmio_fast(address, bytes, callbacks, fast_reads)?;
                } else {
                    cpu.map_mmio(address, bytes, callbacks)?;
                }
            }
            Value::Unit
        }
        Operation::Unmap(address, bytes) => {
            cpu.unmap(address, bytes)?;
            Value::Unit
        }
        Operation::Protect(address, bytes, permissions) => {
            cpu.protect(address, bytes, permissions)?;
            Value::Unit
        }
        Operation::InvalidateCode(range) => {
            cpu.invalidate_code(range)?;
            Value::Unit
        }
        Operation::TightenInstructionLimit(total) => {
            cpu.tighten_instruction_limit(total)?;
            Value::Unit
        }
        Operation::Progress => Value::OptionalU64(engine.progress()),
        Operation::Stop => {
            cpu.stop()?;
            Value::Unit
        }
        Operation::StopBeforeInstruction => {
            cpu.stop_before_instruction()?;
            Value::Unit
        }
        Operation::RprofileIdentity(value) => Value::Identity(cpu.rprofile_identity(value)?),
        Operation::SetInterrupt(line, asserted) => {
            cpu.set_interrupt(line, asserted)?;
            Value::Unit
        }
        Operation::Snapshot(previous) => {
            let snapshots = context.snapshots.borrow();
            let previous = previous.and_then(|token| snapshots.get(&token).map(Box::as_ref));
            let snapshot = cpu.snapshot(previous)?;
            drop(snapshots);
            context.retain_snapshot(snapshot)?
        }
        Operation::ImportSnapshot(bytes) => {
            context.retain_snapshot(cpu.import_snapshot(&bytes)?)?
        }
        Operation::ExportSnapshot(token) => Value::Bytes(
            context
                .snapshots
                .borrow()
                .get(&token)
                .ok_or("unknown worker snapshot token")?
                .export()?,
        ),
        Operation::Restore(token) => {
            let snapshots = context.snapshots.borrow();
            let snapshot = snapshots
                .get(&token)
                .ok_or("unknown worker snapshot token")?;
            cpu.restore(snapshot.as_ref())?;
            Value::Unit
        }
        Operation::InstallHooks(hooks) => {
            let mut installed = Vec::with_capacity(hooks.len());
            for (callback, kind) in hooks {
                if matches!(kind, HookKind::Synchronization) && cpu.model() == Model::CortexM4 {
                    // Raw M-profile has no separate barrier event. The proxy
                    // serves these observers with its owner-serviced writes.
                    let key = usize::MAX - callback;
                    context.virtual_hooks.borrow_mut().insert(key);
                    installed.push((
                        HookId {
                            owner: cpu.identity(),
                            key,
                        },
                        callback,
                    ));
                    continue;
                }
                let data = context.callback_context(callback);
                let hook = match kind {
                    HookKind::Instruction(range) => Hook::Instruction {
                        callback: instruction,
                        data,
                        range,
                    },
                    HookKind::Block(range) => Hook::Block {
                        callback: block,
                        data,
                        range,
                    },
                    HookKind::Exception => Hook::Exception {
                        callback: exception,
                        data,
                    },
                    HookKind::Synchronization => Hook::Synchronization {
                        callback: synchronization,
                        data,
                    },
                    HookKind::Memory { read, write, range } => Hook::Memory {
                        callback: memory,
                        data,
                        read,
                        write,
                        range,
                    },
                    HookKind::CompletedStore(range) => Hook::CompletedStore {
                        callback: memory,
                        data,
                        range,
                    },
                    HookKind::Fault => Hook::Fault {
                        callback: fault,
                        data,
                    },
                };
                match unsafe { cpu.install_hook(hook) } {
                    Ok(hook) => installed.push((hook, callback)),
                    Err(error) => {
                        context.callback_contexts.borrow_mut().remove(&callback);
                        for (hook, callback) in installed {
                            if !context.virtual_hooks.borrow_mut().remove(&hook.key) {
                                cpu.remove_hook(hook)?;
                            }
                            context.callback_contexts.borrow_mut().remove(&callback);
                        }
                        return Err(error);
                    }
                }
            }
            Value::Hooks(installed.into_iter().map(|(hook, _)| hook).collect())
        }
        Operation::RemoveHooks(hooks) => {
            for (hook, callback) in hooks {
                if !context.virtual_hooks.borrow_mut().remove(&hook.key) {
                    cpu.remove_hook(hook)?;
                }
                context.callback_contexts.borrow_mut().remove(&callback);
            }
            Value::Unit
        }
        Operation::ObserveBackings(ranges) => {
            let ranges = ranges
                .into_iter()
                .map(|range| BackingObserver {
                    backing: NonNull::new(range.backing as *mut u8).expect("checked backing"),
                    bytes: range.bytes,
                    bitmap: NonNull::new(range.bitmap as *mut std::sync::atomic::AtomicU64)
                        .expect("checked bitmap"),
                    page_shift: range.page_shift,
                    summary: range
                        .summary
                        .and_then(|address| NonNull::new(address as *mut _)),
                })
                .collect::<Vec<_>>();
            unsafe { cpu.observe_backings(&ranges)? };
            Value::Unit
        }
        Operation::WatchStores(address, bytes) => {
            cpu.watch_stores(address, bytes)?;
            Value::Unit
        }
        Operation::HookWatched(hook, watched) => {
            cpu.hook_watched(hook, watched)?;
            Value::Unit
        }
        Operation::PreciseStorePc(on) => {
            cpu.precise_store_pc(on)?;
            Value::Unit
        }
        Operation::BackingPointer(address, bytes) => Value::OptionalU64(
            cpu.backing_pointer(address, bytes)
                .map(|p| p.as_ptr() as usize as u64),
        ),
        Operation::MemoryRestartBoundary(range) => {
            Value::Hooks(vec![cpu.install_memory_restart_boundary(range)?])
        }
        Operation::TranslationHooks(callback) => {
            let hooks = callback.map(|id| TranslationHooks {
                classify,
                restart,
                data: context.callback_context(id),
            });
            if let Err(error) = unsafe { cpu.translation_hooks(hooks) } {
                if let Some(id) = callback {
                    context.callback_contexts.borrow_mut().remove(&id);
                }
                return Err(error);
            }
            if let Some(old) = context.translation_callback.replace(callback) {
                context.callback_contexts.borrow_mut().remove(&old);
            }
            Value::Unit
        }
        Operation::CodeStore(installed) => {
            // The engine keeps the user pointer until the observer is removed,
            // so the boxed context lives in the loop for exactly that long.
            let mut held = context.code_store.borrow_mut();
            // Box first so the pointer handed to the engine is the boxed
            // context, not a stack copy that dies with this call.
            *held = installed.map(|context| {
                Box::new(CodeStoreContext {
                    callback: context.callback,
                    data: context.data,
                })
            });
            let (trampoline, user) = match held.as_mut() {
                Some(context) => (
                    Some(crate::code_store_trampoline as unsafe fn(*mut c_void, u64, usize)),
                    (&mut **context as *mut CodeStoreContext).cast::<c_void>(),
                ),
                None => (None, std::ptr::null_mut()),
            };
            unsafe { cpu.observe_code_stores(trampoline, user)? };
            drop(held);
            Value::Unit
        }
        Operation::RetryBlock(instructions) => {
            cpu.retry_block(instructions)?;
            Value::Unit
        }
        Operation::Restart => {
            cpu.request_restart()?;
            Value::Unit
        }
        Operation::StopOnEl3Transitions => {
            cpu.stop_on_el3_transitions()?;
            Value::Unit
        }
        Operation::ExitCounters => Value::Counters(cpu.exit_counters()?.into_iter().collect()),
    };
    Ok(result)
}

impl CpuWorker {
    fn accounting_accessible(&self) -> bool {
        !self.running.get()
            || self.callback_depth.get() != 0
            || self.pending_callback.borrow().is_some()
    }
    fn rpc(&self, operation: Operation) -> Result<Value, String> {
        #[cfg(test)]
        self.rpc_calls.set(self.rpc_calls.get() + 1);
        let counter = operation.counter();
        let (reply, response) = mpsc::channel();
        self.commands
            .send(Message::Request { operation, reply })
            .map_err(|_| "Unicorn CPU worker disconnected")?;
        if let Some(counter) = counter {
            let count = &self.rpc_counts[counter as usize];
            count.set(count.get().saturating_add(1));
        }
        response
            .recv()
            .map_err(|_| "Unicorn CPU worker reply disconnected")?
    }
    fn unit(&self, operation: Operation) -> Result<(), String> {
        match self.rpc(operation)? {
            Value::Unit => Ok(()),
            _ => Err("Unicorn CPU worker returned an unexpected reply".into()),
        }
    }
    fn next_callback(&self) -> Result<usize, String> {
        let id = self.next_callback.get();
        self.next_callback
            .set(id.checked_add(1).ok_or("callback identity overflow")?);
        Ok(id)
    }
    fn service_callback(&self, callback: usize, call: CallbackCall) -> CallbackResult {
        let entry = self
            .callbacks
            .borrow()
            .get(&callback)
            .map(|entry| match entry {
                CallbackEntry::Mmio(mmio) => CallbackEntry::Mmio(*mmio),
                CallbackEntry::Hook(hook) => CallbackEntry::Hook(*hook),
                CallbackEntry::Translation(hooks) => CallbackEntry::Translation(*hooks),
            });
        let Some(entry) = entry else {
            return CallbackResult::Error;
        };
        self.callback_depth
            .set(self.callback_depth.get().saturating_add(1));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            if self.model == Model::CortexM4
                && self.execution_policy.get() == ExecutionPolicy::Interactive
                && matches!(
                    call,
                    CallbackCall::Memory {
                        access: Access::Write,
                        ..
                    } | CallbackCall::MmioWrite { .. }
                )
            {
                let barriers: Vec<_> = self
                    .callbacks
                    .borrow()
                    .values()
                    .filter_map(|entry| match entry {
                        CallbackEntry::Hook(Hook::Synchronization { callback, data }) => {
                            Some((*callback, *data))
                        }
                        _ => None,
                    })
                    .collect();
                for (barrier, data) in barriers {
                    barrier(data);
                }
            }
            match (entry, call) {
                (CallbackEntry::Mmio(mmio), CallbackCall::MmioRead { offset, size }) => {
                    CallbackResult::U64((mmio.read)(offset, size, mmio.data))
                }
                (
                    CallbackEntry::Mmio(mmio),
                    CallbackCall::MmioWrite {
                        offset,
                        size,
                        value,
                    },
                ) => {
                    (mmio.write)(offset, size, value, mmio.data);
                    CallbackResult::Unit
                }
                (
                    CallbackEntry::Hook(Hook::Instruction { callback, data, .. }),
                    CallbackCall::Instruction { address, size },
                ) => {
                    callback(address, size, data);
                    CallbackResult::Unit
                }
                (
                    CallbackEntry::Hook(Hook::Block { callback, data, .. }),
                    CallbackCall::Block(block),
                ) => {
                    callback(block, data);
                    CallbackResult::Unit
                }
                (
                    CallbackEntry::Hook(Hook::Exception { callback, data }),
                    CallbackCall::Exception(exception),
                ) => {
                    callback(exception, data);
                    CallbackResult::Unit
                }
                (
                    CallbackEntry::Hook(Hook::Synchronization { callback, data }),
                    CallbackCall::Synchronization,
                ) => {
                    callback(data);
                    CallbackResult::Unit
                }
                (
                    CallbackEntry::Hook(
                        Hook::Memory { callback, data, .. }
                        | Hook::CompletedStore { callback, data, .. },
                    ),
                    CallbackCall::Memory {
                        access,
                        address,
                        size,
                        value,
                    },
                ) => {
                    callback(access, address, size, value, data);
                    CallbackResult::Unit
                }
                (
                    CallbackEntry::Hook(Hook::Fault { callback, data }),
                    CallbackCall::Fault {
                        fault,
                        address,
                        size,
                        value,
                    },
                ) => CallbackResult::Bool(callback(fault, address, size, value, data)),
                (CallbackEntry::Translation(hooks), CallbackCall::Classify { address, word }) => {
                    CallbackResult::Translation((hooks.classify)(hooks.data, address, word))
                }
                (CallbackEntry::Translation(hooks), CallbackCall::Restart(pc)) => {
                    (hooks.restart)(hooks.data, pc);
                    CallbackResult::Unit
                }
                _ => CallbackResult::Error,
            }
        }));
        self.callback_depth
            .set(self.callback_depth.get().saturating_sub(1));
        result.unwrap_or(CallbackResult::Error)
    }

    fn validate_run(&self, run: RunToken) -> Result<(), ExecutionError> {
        if run.owner() != self.identity || self.active_run.get() != Some(run) {
            return Err(ExecutionError::Backend(
                "run token belongs to a different or inactive Unicorn run".into(),
            ));
        }
        Ok(())
    }

    fn record_event(&self, run: RunToken, event: Event) -> Result<RunStatus, ExecutionError> {
        match event {
            Event::Callback {
                id,
                callback,
                call,
                progress,
                pc,
                pstate,
            } => {
                if self.pending_callback.borrow().is_some() {
                    return Err(ExecutionError::Backend(
                        "Unicorn worker sent a second callback before the first was serviced"
                            .into(),
                    ));
                }
                self.callback_events
                    .set(self.callback_events.get().saturating_add(1));
                let callback_counter = WorkerCallbackCounter::for_call(&call);
                let count = &self.callback_counts[callback_counter as usize];
                count.set(count.get().saturating_add(1));
                self.stopped_progress.set(progress);
                self.stopped_pc.set(pc);
                self.stopped_pstate.set(pstate);
                *self.pending_callback.borrow_mut() =
                    Some((id, callback, call, progress, pc, pstate));
                Ok(RunStatus::CallbackPending)
            }
            Event::RunComplete {
                id,
                result,
                progress,
                pc,
                pstate,
            } if id == run.sequence() => {
                self.running.set(false);
                self.stopped_progress.set(progress);
                self.stopped_pc.set(pc);
                self.stopped_pstate.set(pstate);
                self.loop_window.set(Some(false));
                *self.run_result.borrow_mut() = Some(result.clone());
                Ok(RunStatus::Complete(result))
            }
            Event::RunComplete { .. } => Ok(RunStatus::Running),
        }
    }

    fn receive_run_event(&self, run: RunToken, wait: bool) -> Result<RunStatus, ExecutionError> {
        self.validate_run(run)?;
        if self.pending_callback.borrow().is_some() {
            return Ok(RunStatus::CallbackPending);
        }
        if let Some(result) = self.run_result.borrow().as_ref() {
            return Ok(RunStatus::Complete(result.clone()));
        }
        if !self.running.get() {
            return Err(ExecutionError::Backend(
                "Unicorn worker lost an active run without a completion event".into(),
            ));
        }
        loop {
            let event = if wait {
                self.events.borrow_mut().recv().map_err(|_| {
                    ExecutionError::Backend("Unicorn CPU worker disconnected".into())
                })?
            } else {
                match self.events.borrow_mut().try_recv() {
                    Ok(event) => event,
                    Err(mpsc::TryRecvError::Empty) => return Ok(RunStatus::Running),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let error =
                            ExecutionError::Backend("Unicorn CPU worker disconnected".into());
                        self.running.set(false);
                        *self.run_result.borrow_mut() = Some(Err(error.clone()));
                        return Ok(RunStatus::Complete(Err(error)));
                    }
                }
            };
            match self.record_event(run, event)? {
                RunStatus::Running => continue,
                status => return Ok(status),
            }
        }
    }
}

impl Cpu for CpuWorker {
    fn set_loop_mode(&self, mode: loops::LoopMode) -> Result<(), String> {
        self.unit(Operation::SetLoopMode(mode))
    }
    fn set_loop_window(&self, stable: bool) -> Result<(), String> {
        if self.loop_window.get() == Some(stable) {
            return Ok(());
        }
        self.unit(Operation::SetLoopWindow(stable))?;
        self.loop_window.set(Some(stable));
        Ok(())
    }
    fn hook_count(&self) -> Option<usize> {
        (self.model == Model::CortexM4).then(|| {
            self.hooks
                .borrow()
                .values()
                .filter(|hook| {
                    !matches!(
                        self.callbacks.borrow().get(&hook.callback),
                        Some(CallbackEntry::Hook(Hook::Synchronization { .. }))
                    )
                })
                .count()
        })
    }
    fn backing_pointer(&self, address: u64, bytes: usize) -> Option<NonNull<u8>> {
        match self.rpc(Operation::BackingPointer(address, bytes)).ok()? {
            Value::OptionalU64(address) => NonNull::new(address? as usize as *mut u8),
            _ => None,
        }
    }
    fn stop_on_el3_transitions(&self) -> Result<(), String> {
        self.unit(Operation::StopOnEl3Transitions)
    }
    fn identity(&self) -> Identity {
        self.identity
    }
    fn set_execution_policy(&self, policy: ExecutionPolicy) -> Result<(), String> {
        if self.running.get() {
            return Err("execution policy requires a stopped processor".into());
        }
        self.unit(Operation::SetExecutionPolicy(policy))?;
        self.execution_policy.set(policy);
        Ok(())
    }
    fn checkpoint_backings(&self) -> Result<Vec<(NonNull<u8>, usize)>, String> {
        match self.rpc(Operation::CheckpointBackings)? {
            Value::Backings(backings) => backings
                .into_iter()
                .map(|(pointer, bytes)| {
                    Ok((
                        NonNull::new(pointer as *mut u8).ok_or("null backing pointer")?,
                        bytes,
                    ))
                })
                .collect(),
            _ => Err("Unicorn CPU worker returned an unexpected backing list".into()),
        }
    }
    fn exclusive_domain(&self) -> Option<exclusive::Shared> {
        (self.model == Model::CortexR5F).then(|| self.memory.exclusive_domain())
    }
    fn model(&self) -> Model {
        self.model
    }
    fn features(&self) -> Features {
        self.features
    }
    fn set_guest_clock(&self, clock: Clock) -> Result<(), String> {
        self.unit(Operation::SetGuestClock(clock))
    }
    fn read_register(&self, register: Register, out: &mut [u8]) -> Result<usize, String> {
        let pc_width = if self.model == Model::ArmV8A { 8 } else { 4 };
        if matches!(register, Register::Pc | arm::PC)
            && out.len() >= pc_width
            && (self.callback_depth.get() != 0
                || self.pending_callback.borrow().is_some()
                || self.run_result.borrow().is_some())
            && let Some(pc) = self.stopped_pc.get()
        {
            out[..pc_width].copy_from_slice(&pc.to_le_bytes()[..pc_width]);
            return Ok(pc_width);
        }
        if matches!(register, Register::Pstate | arm::CPSR)
            && out.len() >= 4
            && (self.callback_depth.get() != 0
                || self.pending_callback.borrow().is_some()
                || self.run_result.borrow().is_some())
            && let Some(pstate) = self.stopped_pstate.get()
        {
            out[..4].copy_from_slice(&pstate.to_le_bytes()[..4]);
            return Ok(4);
        }
        match self.rpc(Operation::ReadRegister(register, out.len()))? {
            Value::Bytes(bytes) => {
                if bytes.len() > out.len() {
                    return Err("worker register width exceeds output buffer".into());
                }
                out[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            _ => Err("Unicorn CPU worker returned unexpected register bytes".into()),
        }
    }
    fn write_register(&self, register: Register, bytes: &[u8]) -> Result<(), String> {
        if matches!(register, Register::Pc | arm::PC) {
            self.stopped_pc.set(None);
            self.stopped_pstate.set(None);
        }
        if matches!(register, Register::Pstate | arm::CPSR) {
            self.stopped_pstate.set(None);
        }
        self.unit(Operation::WriteRegister(register, bytes.to_vec()))
    }
    fn coprocessor(&self, register: arm::Coprocessor, value: Option<u64>) -> Result<u64, String> {
        match self.rpc(Operation::Coprocessor(register, value))? {
            Value::U64(value) => Ok(value),
            _ => Err("Unicorn CPU worker returned an unexpected coprocessor value".into()),
        }
    }
    fn read_memory(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        match self.rpc(Operation::ReadMemory(address, out.len()))? {
            Value::Bytes(bytes) if bytes.len() == out.len() => {
                out.copy_from_slice(&bytes);
                Ok(())
            }
            Value::Bytes(_) => Err("worker memory read returned an unexpected length".into()),
            _ => Err("Unicorn CPU worker returned unexpected memory bytes".into()),
        }
    }
    fn translate(&self, address: u64, access: Access) -> Result<u64, String> {
        match self.rpc(Operation::Translate(address, access))? {
            Value::U64(address) => Ok(address),
            _ => Err("Unicorn CPU worker returned an unexpected translation".into()),
        }
    }
    fn read_virtual(&self, address: u64, access: Access, out: &mut [u8]) -> Result<(), String> {
        match self.rpc(Operation::ReadVirtual(address, access, out.len()))? {
            Value::Bytes(bytes) if bytes.len() == out.len() => {
                out.copy_from_slice(&bytes);
                Ok(())
            }
            Value::Bytes(_) => Err("worker virtual read returned an unexpected length".into()),
            _ => Err("Unicorn CPU worker returned unexpected virtual bytes".into()),
        }
    }
    fn write_memory(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        self.unit(Operation::WriteMemory(address, bytes.to_vec()))
    }
    fn memory_written(&self, address: u64, bytes: usize) -> Result<(), String> {
        self.unit(Operation::MemoryWritten(address, bytes))
    }
    unsafe fn backing_written(
        &self,
        physical: u64,
        backing: NonNull<u8>,
        bytes: usize,
    ) -> Result<(), String> {
        self.unit(Operation::BackingWritten(
            physical,
            backing.as_ptr() as usize,
            bytes,
        ))
    }
    fn memory_regions(&self) -> Result<Vec<MemoryRegion>, String> {
        match self.rpc(Operation::MemoryRegions)? {
            Value::MemoryRegions(regions) => Ok(regions),
            _ => Err("Unicorn CPU worker returned unexpected memory regions".into()),
        }
    }
    unsafe fn map_backing(
        &self,
        address: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        self.unit(Operation::MapBacking(
            address,
            bytes,
            permissions,
            backing.as_ptr() as usize,
        ))
    }
    unsafe fn map_physical(
        &self,
        address: u64,
        physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        self.unit(Operation::MapPhysical(
            address,
            physical,
            bytes,
            permissions,
            backing.as_ptr() as usize,
        ))
    }
    unsafe fn replace_physical(
        &self,
        address: u64,
        physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        self.unit(Operation::ReplacePhysical(
            address,
            physical,
            bytes,
            permissions,
            backing.as_ptr() as usize,
        ))
    }
    fn map_zeroed(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        self.unit(Operation::MapZeroed(address, bytes, permissions))
    }
    unsafe fn map_mmio(&self, address: u64, bytes: usize, callbacks: Mmio) -> Result<(), String> {
        let id = self.next_callback()?;
        self.callbacks
            .borrow_mut()
            .insert(id, CallbackEntry::Mmio(callbacks));
        if let Err(error) = self.unit(Operation::MapMmio(address, bytes, id, Vec::new())) {
            self.callbacks.borrow_mut().remove(&id);
            return Err(error);
        }
        Ok(())
    }
    unsafe fn map_mmio_fast(
        &self,
        address: u64,
        bytes: usize,
        callbacks: Mmio,
        fast_reads: Vec<FastMmioRead>,
    ) -> Result<(), String> {
        let id = self.next_callback()?;
        self.callbacks
            .borrow_mut()
            .insert(id, CallbackEntry::Mmio(callbacks));
        if let Err(error) = self.unit(Operation::MapMmio(address, bytes, id, fast_reads)) {
            self.callbacks.borrow_mut().remove(&id);
            return Err(error);
        }
        Ok(())
    }
    fn unmap(&self, address: u64, bytes: usize) -> Result<(), String> {
        self.unit(Operation::Unmap(address, bytes))
    }
    fn protect(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String> {
        self.unit(Operation::Protect(address, bytes, permissions))
    }
    fn invalidate_code(&self, range: Option<(u64, u64)>) -> Result<(), String> {
        self.unit(Operation::InvalidateCode(range))
    }
    fn start_run(&self, request: Run) -> Result<RunToken, ExecutionError> {
        if self.running.get() {
            return Err(ExecutionError::Backend(
                "nested Unicorn worker execution is refused".into(),
            ));
        }
        let id = self.next_run.get();
        self.next_run.set(id.wrapping_add(1).max(1));
        let run = RunToken::new(self.identity, id);
        self.pause_requested.store(0, Ordering::Release);
        self.stopped_progress.set(None);
        self.stopped_pc.set(None);
        self.stopped_pstate.set(None);
        self.loop_window.set(None);
        self.pending_callback.borrow_mut().take();
        self.run_result.borrow_mut().take();
        if self.commands.send(Message::Run { id, request }).is_err() {
            return Err(ExecutionError::Backend(
                "Unicorn CPU worker disconnected".into(),
            ));
        }
        self.active_run.set(Some(run));
        self.running.set(true);
        Ok(run)
    }

    fn poll_run(&self, run: RunToken) -> Result<RunStatus, ExecutionError> {
        self.receive_run_event(run, false)
    }

    fn request_pause(&self, run: RunToken) -> Result<(), ExecutionError> {
        self.validate_run(run)?;
        if self.running.get() {
            self.pause_requested.store(1, Ordering::Release);
            // The accounting flag is only sampled by the hook and at a block
            // entry. A core looping through translated blocks notices the
            // engine's own wakeup instead, which every block entry tests.
            Library::request_pause(self.identity).map_err(ExecutionError::Backend)?;
        }
        Ok(())
    }

    fn service_pending_callback(&self, run: RunToken) -> Result<(), ExecutionError> {
        self.validate_run(run)?;
        let Some((id, callback, call, progress, pc, pstate)) =
            self.pending_callback.borrow_mut().take()
        else {
            return Err(ExecutionError::Backend(
                "Unicorn worker has no parked callback to service".into(),
            ));
        };
        self.stopped_progress.set(progress);
        self.stopped_pc.set(pc);
        self.stopped_pstate.set(pstate);
        self.deferred_instruction_limit.set(None);
        self.retried_block.set(false);
        let yields_after_entry = self.owner_accounting
            && !self.ordered_run.get()
            && matches!(
                call,
                CallbackCall::Block(_)
                    | CallbackCall::Instruction { .. }
                    | CallbackCall::MmioRead { .. }
                    | CallbackCall::MmioWrite { .. }
            );
        let result = self.service_callback(callback, call);
        // A concurrent M-profile run can park during translation or while
        // its owner shortens a block. It must be able to reach the next
        // admitted entry. After that entry, return the processor to the
        // cohort before it runs ahead of a peer waiting at a callback.
        if yields_after_entry && !self.retried_block.get() {
            self.pause_requested.store(1, Ordering::Release);
        }
        let deferred = self.deferred_instruction_limit.take();
        let tighten = (!matches!(result, CallbackResult::Error))
            .then_some(deferred)
            .flatten();
        let (applied, applied_rx) = if tighten.is_some() {
            let (tx, rx) = mpsc::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        if self
            .commands
            .send(Message::CallbackResponse {
                id,
                result,
                tighten,
                applied,
            })
            .is_err()
        {
            let error =
                ExecutionError::Backend("Unicorn CPU worker disconnected during callback".into());
            self.stopped_progress.set(None);
            self.stopped_pc.set(None);
            self.stopped_pstate.set(None);
            self.running.set(false);
            *self.run_result.borrow_mut() = Some(Err(error.clone()));
            return Err(error);
        }
        let applied_result = if let Some(applied) = applied_rx {
            match applied.recv() {
                Ok(result) => result.map_err(ExecutionError::Backend),
                Err(_) => Err(ExecutionError::Backend(
                    "Unicorn CPU worker disconnected while applying callback limit".into(),
                )),
            }
        } else {
            Ok(())
        };
        self.stopped_progress.set(None);
        self.stopped_pc.set(None);
        self.stopped_pstate.set(None);
        applied_result
    }

    fn cancel_pending_callback(&self, run: RunToken) -> Result<(), ExecutionError> {
        self.validate_run(run)?;
        self.pause_requested.store(1, Ordering::Release);
        let Some((id, _, _, _, _, _)) = self.pending_callback.borrow_mut().take() else {
            return Ok(());
        };
        if self
            .commands
            .send(Message::CallbackResponse {
                id,
                result: CallbackResult::Error,
                tighten: None,
                applied: None,
            })
            .is_err()
        {
            let error = ExecutionError::Backend(
                "Unicorn CPU worker disconnected while canceling callback".into(),
            );
            self.stopped_progress.set(None);
            self.stopped_pc.set(None);
            self.stopped_pstate.set(None);
            self.running.set(false);
            *self.run_result.borrow_mut() = Some(Err(error.clone()));
            return Err(error);
        }
        self.stopped_progress.set(None);
        self.stopped_pc.set(None);
        self.stopped_pstate.set(None);
        Ok(())
    }

    fn set_run_wakeup(&self, wakeup: RunWakeup) -> Result<(), String> {
        if self.running.get() {
            return Err("cannot replace Unicorn run wakeup during an active run".into());
        }
        self.commands
            .send(Message::SetRunWakeup(wakeup))
            .map_err(|_| "Unicorn CPU worker disconnected".into())
    }

    fn run(&self, request: Run) -> Result<Exit, ExecutionError> {
        let previous = self.ordered_run.replace(true);
        let result = (|| {
            let run = self.start_run(request)?;
            loop {
                match self.receive_run_event(run, true)? {
                    RunStatus::Running => continue,
                    RunStatus::CallbackPending => self.service_pending_callback(run)?,
                    RunStatus::Complete(result) => return result,
                }
            }
        })();
        self.ordered_run.set(previous);
        result
    }
    fn tighten_instruction_limit(&self, total: u64) -> Result<(), String> {
        if self.callback_depth.get() != 0 {
            let total = self
                .deferred_instruction_limit
                .get()
                .map_or(total, |current| current.min(total));
            self.deferred_instruction_limit.set(Some(total));
            return Ok(());
        }
        self.unit(Operation::TightenInstructionLimit(total))
    }
    fn instruction_progress(&self) -> Option<u64> {
        if self.owner_accounting {
            return None;
        }
        if (self.callback_depth.get() != 0
            || self.pending_callback.borrow().is_some()
            || self.run_result.borrow().is_some())
            && let Some(progress) = self.stopped_progress.get()
        {
            return Some(progress);
        }
        match self.rpc(Operation::Progress).ok()? {
            Value::OptionalU64(progress) => progress,
            _ => None,
        }
    }
    fn stop(&self) -> Result<(), String> {
        self.unit(Operation::Stop)
    }
    fn stop_before_instruction(&self) -> Result<(), String> {
        self.unit(Operation::StopBeforeInstruction)
    }
    fn rprofile_identity(&self, value: Option<u32>) -> Result<u32, String> {
        match self.rpc(Operation::RprofileIdentity(value))? {
            Value::Identity(value) => Ok(value),
            _ => Err("Unicorn CPU worker returned unexpected identity".into()),
        }
    }
    fn set_interrupt(&self, line: Interrupt, asserted: bool) -> Result<(), String> {
        self.unit(Operation::SetInterrupt(line, asserted))
    }
    fn snapshot(&self, previous: Option<&dyn Snapshot>) -> Result<Box<dyn Snapshot>, String> {
        let previous = previous
            .map(|snapshot| -> Result<u64, String> {
                if snapshot.owner() != self.identity {
                    return Err("CPU snapshot belongs to a different processor".into());
                }
                snapshot
                    .as_any()
                    .downcast_ref::<RemoteSnapshot>()
                    .map(|snapshot| snapshot.token)
                    .ok_or_else(|| "CPU snapshot belongs to a different backend".into())
            })
            .transpose()?;
        let token = match self.rpc(Operation::Snapshot(previous))? {
            Value::Snapshot(token) => token,
            _ => return Err("Unicorn CPU worker returned unexpected snapshot".into()),
        };
        Ok(Box::new(RemoteSnapshot {
            owner: self.identity,
            token,
            commands: self.commands.clone(),
        }))
    }
    fn restore(&self, snapshot: &dyn Snapshot) -> Result<(), String> {
        if snapshot.owner() != self.identity {
            return Err("CPU snapshot belongs to a different processor".into());
        }
        let snapshot = snapshot
            .as_any()
            .downcast_ref::<RemoteSnapshot>()
            .ok_or("CPU snapshot belongs to a different backend")?;
        self.unit(Operation::Restore(snapshot.token))?;
        self.stopped_progress.set(None);
        self.stopped_pc.set(None);
        self.stopped_pstate.set(None);
        self.loop_window.set(None);
        Ok(())
    }
    fn import_snapshot(&self, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        self.import_bytes(bytes)
    }
    unsafe fn install_hook(&self, hook: Hook) -> Result<HookId, String> {
        Ok(unsafe { self.install_hooks(&[hook]) }?.remove(0))
    }
    fn remove_hook(&self, hook: HookId) -> Result<(), String> {
        self.remove_hooks(&[hook])
    }
    unsafe fn install_hooks(&self, hooks: &[Hook]) -> Result<Vec<HookId>, String> {
        if hooks.is_empty() {
            return Ok(vec![]);
        }
        if hooks
            .iter()
            .any(|hook| matches!(hook, Hook::HostWrite { .. }))
        {
            return Err("host-write hooks require the R-profile backend boundary".into());
        }
        let mut pending = Vec::with_capacity(hooks.len());
        for &hook in hooks {
            let callback = self.next_callback()?;
            let kind = match hook {
                Hook::Instruction { range, .. } => HookKind::Instruction(range),
                Hook::Block { range, .. } => HookKind::Block(range),
                Hook::Exception { .. } => HookKind::Exception,
                Hook::Synchronization { .. } => HookKind::Synchronization,
                Hook::Memory {
                    read, write, range, ..
                } => HookKind::Memory { read, write, range },
                Hook::CompletedStore { range, .. } => HookKind::CompletedStore(range),
                Hook::HostWrite { .. } => {
                    return Err("host-write hooks require the R-profile backend boundary".into());
                }
                Hook::Fault { .. } => HookKind::Fault,
            };
            pending.push((callback, kind, hook));
        }
        let remote = match self.rpc(Operation::InstallHooks(
            pending
                .iter()
                .map(|&(callback, kind, _)| (callback, kind))
                .collect(),
        ))? {
            Value::Hooks(hooks) if hooks.len() == pending.len() => hooks,
            _ => return Err("Unicorn CPU worker returned unexpected hooks".into()),
        };
        for (&remote, (callback, _, hook)) in remote.iter().zip(pending) {
            self.callbacks
                .borrow_mut()
                .insert(callback, CallbackEntry::Hook(hook));
            self.hooks
                .borrow_mut()
                .insert(remote.key, RemoteHook { remote, callback });
        }
        Ok(remote)
    }
    fn remove_hooks(&self, hooks: &[HookId]) -> Result<(), String> {
        if hooks.is_empty() {
            return Ok(());
        }
        let mut seen = std::collections::BTreeSet::new();
        let remote = hooks
            .iter()
            .map(|hook| {
                self.hooks
                    .borrow()
                    .get(&hook.key)
                    .filter(|remote| {
                        hook.owner == self.identity
                            && remote.remote == *hook
                            && seen.insert(hook.key)
                    })
                    .map(|remote| (remote.remote, remote.callback))
                    .ok_or_else(|| "hook belongs to a different CPU or was removed".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.unit(Operation::RemoveHooks(remote.clone()))?;
        for (hook, callback) in remote {
            self.hooks.borrow_mut().remove(&hook.key);
            self.callbacks.borrow_mut().remove(&callback);
        }
        Ok(())
    }
    fn install_memory_restart_boundary(&self, range: HookRange) -> Result<HookId, String> {
        let callback = self.next_callback()?;
        let Value::Hooks(mut hooks) = self.rpc(Operation::MemoryRestartBoundary(range))? else {
            return Err("worker did not return a restart boundary".into());
        };
        let remote = hooks
            .pop()
            .ok_or("worker did not install a restart boundary")?;
        self.hooks
            .borrow_mut()
            .insert(remote.key, RemoteHook { remote, callback });
        Ok(remote)
    }
    unsafe fn translation_hooks(&self, hooks: Option<TranslationHooks>) -> Result<(), String> {
        let callback = hooks
            .map(|hooks| {
                self.next_callback().map(|id| {
                    self.callbacks
                        .borrow_mut()
                        .insert(id, CallbackEntry::Translation(hooks));
                    id
                })
            })
            .transpose()?;
        if let Err(error) = self.unit(Operation::TranslationHooks(callback)) {
            if let Some(id) = callback {
                self.callbacks.borrow_mut().remove(&id);
            }
            return Err(error);
        }
        if let Some(old) = self.translation_callback.replace(callback) {
            self.callbacks.borrow_mut().remove(&old);
        }
        Ok(())
    }
    unsafe fn observe_code_stores(
        &self,
        callback: Option<unsafe fn(*mut c_void, u64, usize)>,
        data: *mut c_void,
    ) -> Result<(), String> {
        // The engine calls back through `crate::code_store`, which reads the
        // callback and its user pointer from `InstalledCodeStore` on the engine
        // thread and then calls that callback. Only those two fields cross the
        // boundary, so the worker sends them directly instead of holding a box
        // of this crate's own type; that box would be a different type on the
        // engine side and `crate::code_store` would read it wrongly.
        self.unit(Operation::CodeStore(callback.map(|callback| {
            CodeStoreContext { callback, data }
        })))
    }
    fn retry_block(&self, instructions: u32) -> Result<(), String> {
        self.unit(Operation::RetryBlock(instructions))?;
        self.retried_block.set(true);
        Ok(())
    }
    fn request_restart(&self) -> Result<(), String> {
        self.unit(Operation::Restart)
    }
    fn thumb_accounting(&self) -> Option<NonNull<ThumbBlockAccounting>> {
        self.accounting_accessible()
            .then_some(self.thumb_accounting)
            .flatten()
    }
    fn thumb_recovery_accounting(&self) -> Option<NonNull<ThumbBlockRecoveryAccounting>> {
        self.accounting_accessible()
            .then_some(self.thumb_recovery)
            .flatten()
    }
    fn accounting(&self) -> Option<NonNull<BlockAccounting>> {
        None
    }
    unsafe fn observe_backings(&self, ranges: &[BackingObserver]) -> Result<(), String> {
        let ranges = ranges
            .iter()
            .map(|range| BackingRange {
                backing: range.backing.as_ptr() as usize,
                bytes: range.bytes,
                bitmap: range.bitmap.as_ptr() as usize,
                page_shift: range.page_shift,
                summary: range.summary.map(|ptr| ptr.as_ptr() as usize),
            })
            .collect();
        self.unit(Operation::ObserveBackings(ranges))
    }
    fn watch_stores(&self, address: u64, bytes: u64) -> Result<(), String> {
        self.unit(Operation::WatchStores(address, bytes))
    }
    fn hook_watched(&self, hook: HookId, watched: bool) -> Result<(), String> {
        if hook.owner != self.identity || !self.hooks.borrow().contains_key(&hook.key) {
            return Err("hook belongs to a different CPU or was removed".into());
        }
        self.unit(Operation::HookWatched(hook, watched))
    }
    fn precise_store_pc(&self, on: bool) -> Result<(), String> {
        self.unit(Operation::PreciseStorePc(on))
    }
    fn exit_counters(&self) -> Result<BTreeMap<&'static str, u64>, String> {
        match self.rpc(Operation::ExitCounters)? {
            Value::Counters(counters) => {
                let mut counters = counters.into_iter().collect::<BTreeMap<_, _>>();
                for counter in WorkerRpcCounter::ALL {
                    counters.insert(counter.name(), self.rpc_counts[counter as usize].get());
                }
                counters.insert("worker_callback_events", self.callback_events.get());
                for counter in WorkerCallbackCounter::ALL {
                    counters.insert(counter.name(), self.callback_counts[counter as usize].get());
                }
                Ok(counters)
            }
            _ => Err("Unicorn CPU worker returned unexpected exit counters".into()),
        }
    }
}

impl Drop for CpuWorker {
    fn drop(&mut self) {
        self.pause_requested.store(1, Ordering::Release);
        let _ = self.commands.send(Message::Shutdown);
        if let Some(worker) = self.worker.get_mut().take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod architecture_tests {
    use super::*;
    use std::time::{Duration, Instant};

    unsafe fn unexpected_read(_: u64, _: u32, _: *mut std::ffi::c_void) -> u64 {
        0xdead
    }

    unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

    // Wait for the native guest read to publish its value before pausing.
    // The read bypasses callbacks, so the request interrupts guest execution.
    fn pause_after_read(cpu: &CpuWorker, begin: u64, seen: &AtomicU32) {
        let run = cpu
            .start_run(Run {
                begin,
                until: None,
                watchdog_us: 0,
                instructions: Some(100_000_000),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while seen.load(Ordering::Acquire) == 0 {
            assert!(
                Instant::now() < deadline,
                "worker did not enter its guest read"
            );
            assert!(matches!(cpu.poll_run(run).unwrap(), RunStatus::Running));
            std::thread::yield_now();
        }
        cpu.request_pause(run).unwrap();
        assert!(matches!(
            cpu.receive_run_event(run, true).unwrap(),
            RunStatus::Complete(Ok(Exit::Stopped))
        ));
        assert!((1..100_000_000).contains(&cpu.instruction_progress().unwrap()));
    }

    #[test]
    fn worker_forwards_the_code_store_observer() {
        // A worker advertises `code_store_observation` only because it forwards
        // the observer; the board installs it whenever block execution starts.
        // Without the forwarding the backend reports a capability it refuses,
        // and block mode fails with "no code-store observer".
        let cpu = spawn_mprofile(1 << 20, false).unwrap();
        assert_eq!(cpu.model(), Model::CortexM4);
        assert!(cpu.features().code_store_observation);

        let seen = Arc::new(AtomicU32::new(0));
        let counter = seen.clone();
        unsafe fn record(_: *mut c_void, _: u64, _: usize) {}
        unsafe {
            cpu.observe_code_stores(Some(record), std::ptr::null_mut())
                .unwrap();
            cpu.observe_code_stores(None, std::ptr::null_mut()).unwrap();
        }
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_code_store_through_a_worker_reaches_the_installed_observer() {
        // The board installs this observer so a store covering translated bytes
        // is reported on its own. On a worker the engine calls back on the
        // engine thread, so the callback and its user pointer must both cross
        // the boundary; a pointer to the wrong struct there would deliver a
        // wrong callback with a wrong context.
        //
        // The guest rewrites its own code page: `str r1,[r2]` writes the two
        // bytes at 0x2004, which are translated instructions.
        let cpu = spawn_armv7a(1 << 20, SharedMemory::default()).unwrap();
        assert_eq!(cpu.model(), Model::CortexA7);
        cpu.map_zeroed(0, 0x8000, 7).unwrap();
        cpu.write_register(arm::CPSR, &0xd3u32.to_le_bytes())
            .unwrap();
        // 0x2000: ldr r1,[pc,#8]; ldr r2,[pc,#8]; str r1,[r2]; b .
        let code: [u32; 6] = [0xe59f_1008, 0xe59f_2008, 0xe582_1000, 0xeaff_fffe, 0, 0x2004];
        let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
        cpu.write_memory(0x2000, &bytes).unwrap();

        struct Counters {
            seen: AtomicU32,
            address: AtomicU64,
        }
        unsafe fn record(data: *mut c_void, address: u64, _: usize) {
            // SAFETY: the test installs a pointer to the `Counters` below and
            // keeps it alive while the observer is installed.
            let counters = unsafe { &*data.cast::<Counters>() };
            counters.address.store(address, Ordering::Release);
            counters.seen.store(1, Ordering::Release);
        }
        let counters = Box::new(Counters {
            seen: AtomicU32::new(0),
            address: AtomicU64::new(0),
        });
        let data = (&*counters as *const Counters).cast_mut().cast();
        unsafe { cpu.observe_code_stores(Some(record), data).unwrap() };

        let run = cpu
            .start_run(Run {
                begin: 0x2000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1_000_000),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if counters.seen.load(Ordering::Acquire) != 0 {
                break;
            }
            assert!(Instant::now() < deadline, "no code-store notification");
            match cpu.poll_run(run).unwrap() {
                RunStatus::Running => std::thread::yield_now(),
                RunStatus::CallbackPending => cpu.service_pending_callback(run).unwrap(),
                RunStatus::Complete(result) => {
                    panic!("worker run ended before any code store: {result:?}")
                }
            }
        }
        // The store writes 0x2004, so the report names that address and the
        // owner passed its own user pointer through to the callback.
        assert_eq!(counters.address.load(Ordering::Acquire), 0x2004);
        // Remove the observer and stop the engine before the counters go out of
        // scope: the worker may deliver one more notification as it shuts down.
        unsafe { cpu.observe_code_stores(None, std::ptr::null_mut()).unwrap() };
        let _ = cpu.request_pause(run);
        drop(cpu);
    }

    #[test]
    fn mprofile_worker_pauses_at_a_thumb_block() {
        let cpu = spawn_mprofile(1 << 20, false).unwrap();
        assert_eq!(cpu.model(), Model::CortexM4);
        assert!(cpu.features().asynchronous_runs);
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        cpu.write_memory(0x1000, &[0x08, 0x68, 0xfe, 0xe7]).unwrap(); // ldr r0,[r1]; b .
        cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        let seen = Arc::new(AtomicU32::new(0));
        let read_seen = seen.clone();
        let read = FastMmioRead::consuming_u32(
            0,
            Arc::new(move || {
                read_seen.store(1, Ordering::Release);
                Some(77)
            }),
        );
        read.set_enabled(true);
        unsafe {
            cpu.map_mmio_fast(
                0x8000,
                0x1000,
                Mmio {
                    read: unexpected_read,
                    write,
                    data: std::ptr::null_mut(),
                },
                vec![read],
            )
            .unwrap();
        }
        pause_after_read(&cpu, 0x1001, &seen);
        assert_eq!(read_scalar(&cpu, Register::Pc), Some(0x1002));
        assert_eq!(read_scalar(&cpu, arm::CORE[0]), Some(77));
    }

    #[test]
    fn aprofile_worker_uses_the_clock_and_keeps_the_full_pc_at_pause() {
        let cpu = spawn_aprofile(1 << 20, SharedMemory::default()).unwrap();
        assert_eq!(cpu.model(), Model::ArmV8A);
        assert!(cpu.features().asynchronous_runs);
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        let address = 0x1_0000_0000;
        cpu.map_zeroed(address, 0x4000, 7).unwrap();
        let code = [0xb940_0020_u32, 0x1400_0000]; // ldr w0,[x1]; b .
        let bytes: Vec<_> = code.into_iter().flat_map(u32::to_le_bytes).collect();
        cpu.write_memory(address, &bytes).unwrap();
        cpu.write_register(Register::X(1), &0x8000u64.to_le_bytes())
            .unwrap();
        let tick = Arc::new(AtomicU32::new(0));
        let observed_tick = tick.clone();
        unsafe {
            cpu.map_mmio_fast(
                0x8000,
                0x1000,
                Mmio {
                    read: unexpected_read,
                    write,
                    data: std::ptr::null_mut(),
                },
                vec![FastMmioRead::clocked_u32(
                    0,
                    Arc::new(move |now| {
                        let now = u32::try_from(now).ok()?;
                        observed_tick.store(now, Ordering::Release);
                        Some(now)
                    }),
                )],
            )
            .unwrap();
        }
        cpu.set_guest_clock(Clock {
            now: 555,
            phase: 0,
            ratio: timeline::Ratio::new(1, 1).unwrap(),
        })
        .unwrap();
        pause_after_read(&cpu, address, &tick);
        assert_eq!(read_scalar(&cpu, Register::Pc), Some(address + 4));
        assert_eq!(read_scalar(&cpu, Register::X(0)), Some(555));
        assert_eq!(tick.load(Ordering::Acquire), 555);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_cpu::memory::Backing;

    fn worker(memory: SharedMemory) -> CpuWorker {
        spawn(Config {
            translation_cache_size_bytes: 1024 * 1024,
            options: core::Options::default(),
            memory,
        })
        .expect("start Unicorn worker")
    }

    fn read_u32(cpu: &CpuWorker, register: Register) -> u32 {
        let mut bytes = [0; 4];
        assert_eq!(cpu.read_register(register, &mut bytes).unwrap(), 4);
        u32::from_le_bytes(bytes)
    }

    fn write_words(cpu: &CpuWorker, address: u64, words: &[u32]) {
        let bytes = words
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect::<Vec<_>>();
        cpu.write_memory(address, &bytes).unwrap();
    }

    fn one_step(cpu: &CpuWorker, begin: u64) -> Result<Exit, ExecutionError> {
        cpu.run(Run {
            begin,
            until: None,
            watchdog_us: 0,
            instructions: Some(1),
        })
    }

    fn poll_to_completion(cpu: &CpuWorker, run: RunToken) -> Result<Exit, ExecutionError> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match cpu.poll_run(run)? {
                RunStatus::Running => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "worker run did not finish"
                    );
                    std::thread::yield_now();
                }
                RunStatus::CallbackPending => cpu.service_pending_callback(run)?,
                RunStatus::Complete(result) => return result,
            }
        }
    }

    #[test]
    fn worker_rpc_counters_are_observational_and_survive_restore() {
        for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
            let cpu = worker(SharedMemory::default());
            cpu.set_execution_policy(policy).unwrap();
            let before = cpu.exit_counters().unwrap();

            cpu.map_zeroed(0x1000, 4096, 7).unwrap();
            cpu.write_register(arm::CORE[0], &0x1122_3344u32.to_le_bytes())
                .unwrap();
            let snapshot = cpu.snapshot(None).unwrap();
            cpu.write_register(arm::CORE[0], &0x5566_7788u32.to_le_bytes())
                .unwrap();
            cpu.restore(snapshot.as_ref()).unwrap();
            assert_eq!(read_u32(&cpu, arm::CORE[0]), 0x1122_3344, "{policy:?}");

            let after = cpu.exit_counters().unwrap();
            assert_eq!(
                after["worker_rpc_map_zeroed"] - before["worker_rpc_map_zeroed"],
                1,
                "{policy:?}"
            );
            assert_eq!(
                after["worker_rpc_write_register"] - before["worker_rpc_write_register"],
                2,
                "{policy:?}"
            );
            assert_eq!(
                after["worker_rpc_snapshot"] - before["worker_rpc_snapshot"],
                1,
                "{policy:?}"
            );
            assert_eq!(
                after["worker_rpc_restore"] - before["worker_rpc_restore"],
                1,
                "{policy:?}"
            );
            assert_eq!(
                after["worker_rpc_read_register"] - before["worker_rpc_read_register"],
                1,
                "{policy:?}"
            );
            assert_eq!(after["worker_callback_events"], 0, "{policy:?}");
            for counter in WorkerCallbackCounter::ALL {
                assert_eq!(after[counter.name()], 0, "{policy:?}: {}", counter.name());
            }
            assert!(!after.contains_key("worker_rpc_exit_counters"));
            assert_eq!(after, cpu.exit_counters().unwrap(), "{policy:?}");
        }
    }

    #[test]
    fn observer_groups_use_one_rpc_and_retire_callback_contexts() {
        unsafe fn stored(_: Access, _: u64, _: i32, _: i64, data: *mut std::ffi::c_void) {
            let calls = unsafe { &*data.cast::<Cell<usize>>() };
            calls.set(calls.get() + 1);
        }
        let cpu = worker(SharedMemory::default());
        cpu.map_zeroed(0x1000, 8192, 7).unwrap();
        write_words(&cpu, 0x1000, &[0xe5810000, 0xe5810000]);
        cpu.write_register(arm::CORE[1], &0x2000u32.to_le_bytes())
            .unwrap();
        let calls = Cell::new(0usize);
        let hook = |address| Hook::CompletedStore {
            callback: stored,
            data: (&calls as *const Cell<usize>).cast_mut().cast(),
            range: HookRange::Inclusive {
                begin: address,
                end: address + 3,
            },
        };
        let observers = (0..64).map(|i| hook(0x2000 + i * 4)).collect::<Vec<_>>();
        let before = cpu.rpc_calls.get();
        let installed = unsafe { cpu.install_hooks(&observers) }.unwrap();
        assert_eq!(cpu.rpc_calls.get() - before, 1);
        one_step(&cpu, 0x1000).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(read_u32(&cpu, arm::PC), 0x1004);
        // Repeated or foreign tokens refuse before retiring any member.
        assert!(cpu.remove_hooks(&[installed[0], installed[0]]).is_err());
        let before = cpu.rpc_calls.get();
        cpu.remove_hooks(&installed).unwrap();
        assert_eq!(cpu.rpc_calls.get() - before, 1);
        assert!(cpu.callbacks.borrow().is_empty());
        assert!(cpu.hooks.borrow().is_empty());
        assert!(matches!(
            cpu.rpc(Operation::CallbackContexts).unwrap(),
            Value::U64(0)
        ));
        one_step(&cpu, 0x1004).unwrap();
        assert_eq!(calls.get(), 1);

        // A failed group rolls back its earlier installations on both threads.
        let invalid = Hook::CompletedStore {
            callback: stored,
            data: (&calls as *const Cell<usize>).cast_mut().cast(),
            range: HookRange::Inclusive { begin: 2, end: 1 },
        };
        assert!(unsafe { cpu.install_hooks(&[hook(0x2000), invalid]) }.is_err());
        assert!(cpu.callbacks.borrow().is_empty());
        assert!(cpu.hooks.borrow().is_empty());
        assert!(matches!(
            cpu.rpc(Operation::CallbackContexts).unwrap(),
            Value::U64(0)
        ));
    }

    #[test]
    fn two_worker_cores_share_backing_and_keep_snapshots_worker_local() {
        let memory = SharedMemory::default();
        let backing = Backing::aligned_zeros(4096, 4096).unwrap();
        let pointer = NonNull::new(backing.as_ptr()).unwrap();
        let first = worker(memory.clone());
        let second = worker(memory);
        unsafe {
            first
                .map_physical(0x1000, 0x2000, 4096, 7, pointer)
                .unwrap();
            second
                .map_physical(0x1000, 0x2000, 4096, 7, pointer)
                .unwrap();
        }
        write_words(&first, 0x1000, &[0xe3a00007, 0xe2800001, 0xe320f003]);

        one_step(&first, 0x1000).unwrap();
        assert_eq!(read_u32(&first, arm::CORE[0]), 7);
        let snapshot = first.snapshot(None).unwrap();
        first
            .run(Run {
                begin: 0x1004,
                until: None,
                watchdog_us: 0,
                instructions: Some(2),
            })
            .unwrap();
        assert_eq!(read_u32(&first, arm::CORE[0]), 8);
        assert_eq!(first.instruction_progress(), Some(2));
        first.restore(snapshot.as_ref()).unwrap();
        assert_eq!(read_u32(&first, arm::CORE[0]), 7);
        assert_eq!(read_u32(&first, arm::PC), 0x1004);
        assert_eq!(first.instruction_progress(), Some(1));
        assert!(second.restore(snapshot.as_ref()).is_err());

        one_step(&second, 0x1000).unwrap();
        assert_eq!(read_u32(&second, arm::CORE[0]), 7);
    }

    #[test]
    fn owner_mmio_callback_can_make_a_nested_worker_request() {
        struct Probe {
            cpu: *const CpuWorker,
            calls: u32,
            progress: Option<u64>,
            pc: Option<u64>,
        }
        unsafe fn read(_: u64, size: u32, data: *mut std::ffi::c_void) -> u64 {
            assert_eq!(size, 4);
            let probe = unsafe { &mut *data.cast::<Probe>() };
            let cpu = unsafe { &*probe.cpu };
            probe.calls += 1;
            probe.progress = Some(cpu.instruction_progress().unwrap());
            let mut pc = [0; 4];
            assert_eq!(cpu.read_register(arm::PC, &mut pc).unwrap(), 4);
            probe.pc = Some(u64::from(u32::from_le_bytes(pc)));
            0x1234_5678
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

        let cpu = worker(SharedMemory::default());
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        write_words(&cpu, 0x1000, &[0xe5910000, 0xe320f003]); // ldr r0,[r1]; wfi
        cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        let mut probe = Probe {
            cpu: &cpu,
            calls: 0,
            progress: None,
            pc: None,
        };
        unsafe {
            cpu.map_mmio(
                0x8000,
                4096,
                Mmio {
                    read,
                    write,
                    data: (&mut probe as *mut Probe).cast(),
                },
            )
            .unwrap();
        }
        one_step(&cpu, 0x1000).unwrap();
        assert_eq!(read_u32(&cpu, arm::CORE[0]), 0x1234_5678);
        assert_eq!(probe.calls, 1);
        assert!(probe.progress.is_some());
        assert!(probe.pc.is_some());
        assert_eq!(cpu.exit_counters().unwrap()["worker_callback_events"], 1);
    }

    #[test]
    fn callback_progress_and_limit_updates_are_batched_for_both_policies() {
        struct Probe {
            cpu: *const CpuWorker,
            progress: Cell<Option<u64>>,
            rpc_deltas: Cell<Option<(usize, usize)>>,
        }
        unsafe fn read(_: u64, size: u32, data: *mut std::ffi::c_void) -> u64 {
            assert_eq!(size, 4);
            let probe = unsafe { &*data.cast::<Probe>() };
            let cpu = unsafe { &*probe.cpu };
            let before = cpu.rpc_calls.get();
            let progress = cpu.instruction_progress().unwrap();
            let after_progress = cpu.rpc_calls.get();
            cpu.tighten_instruction_limit(progress).unwrap();
            let after_tighten = cpu.rpc_calls.get();
            probe.progress.set(Some(progress));
            probe.rpc_deltas.set(Some((
                after_progress - before,
                after_tighten - after_progress,
            )));
            0x7654_3210
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

        for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
            let cpu = worker(SharedMemory::default());
            cpu.set_execution_policy(policy).unwrap();
            cpu.map_zeroed(0x1000, 4096, 7).unwrap();
            let mut code = vec![0xe2822001; 8];
            code.extend([0xe5910000]); // ldr r0,[r1]
            code.extend([0xe2833001; 8]);
            code.push(0xe320f003); // wfi
            write_words(&cpu, 0x1000, &code);
            cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
                .unwrap();
            let mut probe = Probe {
                cpu: &cpu,
                progress: Cell::new(None),
                rpc_deltas: Cell::new(None),
            };
            unsafe {
                cpu.map_mmio(
                    0x8000,
                    4096,
                    Mmio {
                        read,
                        write,
                        data: (&mut probe as *mut Probe).cast(),
                    },
                )
                .unwrap();
            }

            let exit = cpu
                .run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(100),
                })
                .unwrap();
            let progress = probe.progress.get().expect("callback progress snapshot");
            assert!(progress >= 8, "{policy:?}: {progress}");
            assert_eq!(probe.rpc_deltas.get(), Some((0, 0)), "{policy:?}");
            assert_eq!(read_u32(&cpu, arm::CORE[0]), 0x7654_3210, "{policy:?}");
            assert_eq!(read_u32(&cpu, arm::CORE[2]), 8, "{policy:?}");
            assert_eq!(read_u32(&cpu, arm::CORE[3]), 0, "{policy:?}");
            assert_eq!(read_u32(&cpu, arm::PC), 0x1024, "{policy:?}");
            assert!(matches!(exit, Exit::Stopped), "{policy:?}: {exit:?}");
            let after = cpu.rpc_calls.get();
            assert_eq!(cpu.instruction_progress(), Some(progress), "{policy:?}");
            assert_eq!(cpu.rpc_calls.get(), after, "{policy:?}");
            let counters = cpu.exit_counters().unwrap();
            assert_eq!(counters["worker_callback_events"], 1, "{policy:?}");
            assert_eq!(counters["worker_callback_mmio_read"], 1, "{policy:?}");
            assert_eq!(counters["worker_callback_mmio_write"], 0, "{policy:?}");
            assert_eq!(counters["worker_callback_synchronization"], 0, "{policy:?}");
        }
    }

    #[test]
    fn callback_kind_counters_distinguish_exact_and_interactive_barriers() {
        unsafe fn barrier(data: *mut std::ffi::c_void) {
            let calls = unsafe { &*data.cast::<Cell<u32>>() };
            calls.set(calls.get() + 1);
        }

        for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
            let memory = SharedMemory::default();
            let cpu = worker(memory);
            cpu.set_execution_policy(policy).unwrap();
            cpu.map_zeroed(0x1000, 4096, 7).unwrap();
            let data = Backing::aligned_zeros(4096, 4096).unwrap();
            unsafe {
                cpu.map_physical(
                    0x8000,
                    0x8000,
                    4096,
                    7,
                    NonNull::new(data.as_ptr()).unwrap(),
                )
                .unwrap();
            }
            write_words(&cpu, 0x1000, &[0xe1910f9f]); // ldrex r0,[r1]
            cpu.write_memory(0x8000, &0x7654_3210u32.to_le_bytes())
                .unwrap();
            cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
                .unwrap();
            let calls = Cell::new(0u32);
            unsafe {
                cpu.install_hook(Hook::Synchronization {
                    callback: barrier,
                    data: (&calls as *const Cell<u32>).cast_mut().cast(),
                })
                .unwrap();
            }

            let snapshot = cpu.snapshot(None).unwrap();
            let before = cpu.exit_counters().unwrap();
            assert!(matches!(
                cpu.run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(1),
                })
                .unwrap(),
                Exit::Stopped
            ));
            let after = cpu.exit_counters().unwrap();
            let expected = u64::from(policy == ExecutionPolicy::Interactive);
            assert_eq!(calls.get() as u64, expected, "{policy:?}");
            assert_eq!(
                after["worker_callback_synchronization"]
                    - before["worker_callback_synchronization"],
                expected,
                "{policy:?}"
            );
            assert_eq!(
                after["worker_callback_events"] - before["worker_callback_events"],
                expected,
                "{policy:?}"
            );
            assert_eq!(
                after["worker_callback_mmio_read"] - before["worker_callback_mmio_read"],
                0,
                "{policy:?}"
            );
            cpu.restore(snapshot.as_ref()).unwrap();
            let restored = cpu.exit_counters().unwrap();
            for counter in WorkerCallbackCounter::ALL {
                assert_eq!(
                    restored[counter.name()],
                    after[counter.name()],
                    "{policy:?}"
                );
            }
            assert_eq!(
                restored["worker_callback_events"], after["worker_callback_events"],
                "{policy:?}"
            );
        }
    }

    #[test]
    fn stopped_pc_snapshots_match_mmio_state_and_invalidate_for_writes_and_restore() {
        struct Probe {
            cpu: *const CpuWorker,
            cached_pc: Cell<Option<u64>>,
            cached_read_rpcs: Cell<Option<usize>>,
            cached_pstate: Cell<Option<u64>>,
            cached_pstate_rpcs: Cell<Option<usize>>,
        }
        unsafe fn read(_: u64, size: u32, data: *mut std::ffi::c_void) -> u64 {
            assert_eq!(size, 4);
            let probe = unsafe { &*data.cast::<Probe>() };
            let cpu = unsafe { &*probe.cpu };
            let before = cpu.rpc_calls.get();
            let mut cached = [0; 4];
            assert_eq!(cpu.read_register(Register::Pc, &mut cached).unwrap(), 4);
            let after_cached = cpu.rpc_calls.get();
            probe
                .cached_pc
                .set(Some(u64::from(u32::from_le_bytes(cached))));
            probe.cached_read_rpcs.set(Some(after_cached - before));
            let mut pstate = [0; 4];
            let mut cpsr = [0; 4];
            assert_eq!(cpu.read_register(Register::Pstate, &mut pstate).unwrap(), 4);
            assert_eq!(cpu.read_register(arm::CPSR, &mut cpsr).unwrap(), 4);
            assert_eq!(pstate, cpsr);
            probe
                .cached_pstate
                .set(Some(u64::from(u32::from_le_bytes(pstate))));
            probe
                .cached_pstate_rpcs
                .set(Some(cpu.rpc_calls.get() - after_cached));
            0x7654_3210
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

        for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
            let cpu = worker(SharedMemory::default());
            cpu.set_execution_policy(policy).unwrap();
            cpu.map_zeroed(0x1000, 4096, 7).unwrap();
            write_words(&cpu, 0x1000, &[0xe5910000, 0xe320f003]);
            cpu.write_register(Register::Pc, &0x1000u32.to_le_bytes())
                .unwrap();
            cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
                .unwrap();
            cpu.write_register(arm::CPSR, &0xd3u32.to_le_bytes())
                .unwrap();
            let mut probe = Probe {
                cpu: &cpu,
                cached_pc: Cell::new(None),
                cached_read_rpcs: Cell::new(None),
                cached_pstate: Cell::new(None),
                cached_pstate_rpcs: Cell::new(None),
            };
            unsafe {
                cpu.map_mmio(
                    0x8000,
                    4096,
                    Mmio {
                        read,
                        write,
                        data: (&mut probe as *mut Probe).cast(),
                    },
                )
                .unwrap();
            }
            let snapshot = cpu.snapshot(None).unwrap();

            let run = cpu
                .start_run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(8),
                })
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                match cpu.poll_run(run).unwrap() {
                    RunStatus::CallbackPending => break,
                    RunStatus::Running => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "MMIO callback did not park"
                        );
                        std::thread::yield_now();
                    }
                    RunStatus::Complete(result) => {
                        panic!("run completed before MMIO: {result:?}")
                    }
                }
            }
            let pending_pc_rpcs = cpu.rpc_calls.get();
            let mut pending_pc = [0; 4];
            assert_eq!(cpu.read_register(Register::Pc, &mut pending_pc).unwrap(), 4);
            assert_eq!(cpu.rpc_calls.get(), pending_pc_rpcs, "{policy:?}");

            cpu.service_pending_callback(run).unwrap();
            assert_eq!(cpu.stopped_pc.get(), None, "{policy:?}");
            assert!(matches!(
                poll_to_completion(&cpu, run).unwrap(),
                Exit::Hint(_) | Exit::Stopped
            ));
            assert_eq!(read_u32(&cpu, arm::CORE[0]), 0x7654_3210, "{policy:?}");
            let cached_pc = probe.cached_pc.get().expect("callback PC snapshot");
            assert_eq!(cached_pc, 0x1000, "{policy:?}");
            assert_eq!(probe.cached_read_rpcs.get(), Some(0), "{policy:?}");
            assert_eq!(probe.cached_pstate.get(), Some(0xd3), "{policy:?}");
            assert_eq!(probe.cached_pstate_rpcs.get(), Some(0), "{policy:?}");

            let completed_pc_rpcs = cpu.rpc_calls.get();
            let mut completed_pc = [0; 4];
            assert_eq!(
                cpu.read_register(Register::Pc, &mut completed_pc).unwrap(),
                4
            );
            assert_ne!(u32::from_le_bytes(completed_pc), 0x1000, "{policy:?}");
            assert_eq!(cpu.rpc_calls.get(), completed_pc_rpcs, "{policy:?}");

            let completed_pstate_rpcs = cpu.rpc_calls.get();
            let mut completed_pstate = [0; 4];
            assert_eq!(
                cpu.read_register(Register::Pstate, &mut completed_pstate)
                    .unwrap(),
                4
            );
            assert_eq!(u32::from_le_bytes(completed_pstate), 0xd3, "{policy:?}");
            let mut completed_cpsr = [0; 4];
            assert_eq!(
                cpu.read_register(arm::CPSR, &mut completed_cpsr).unwrap(),
                4
            );
            assert_eq!(completed_pstate, completed_cpsr, "{policy:?}");
            assert_eq!(cpu.rpc_calls.get(), completed_pstate_rpcs, "{policy:?}");

            let changed_pstate = 0x8000_00d3u32;
            cpu.write_register(arm::CPSR, &changed_pstate.to_le_bytes())
                .unwrap();
            assert_eq!(cpu.stopped_pstate.get(), None, "{policy:?}");
            let after_cpsr_write = cpu.rpc_calls.get();
            let mut changed_pstate_bytes = [0; 4];
            assert_eq!(
                cpu.read_register(Register::Pstate, &mut changed_pstate_bytes)
                    .unwrap(),
                4
            );
            assert_eq!(
                u32::from_le_bytes(changed_pstate_bytes),
                changed_pstate,
                "{policy:?}"
            );
            assert_eq!(cpu.rpc_calls.get(), after_cpsr_write + 1, "{policy:?}");
            let after_cpsr_alias_read = cpu.rpc_calls.get();
            let mut changed_cpsr = [0; 4];
            assert_eq!(cpu.read_register(arm::CPSR, &mut changed_cpsr).unwrap(), 4);
            assert_eq!(changed_cpsr, changed_pstate.to_le_bytes(), "{policy:?}");
            assert_eq!(cpu.rpc_calls.get(), after_cpsr_alias_read + 1, "{policy:?}");

            let pstate_alias_value = 0x4000_00d3u32;
            cpu.write_register(Register::Pstate, &pstate_alias_value.to_le_bytes())
                .unwrap();
            let after_pstate_alias_write = cpu.rpc_calls.get();
            let mut pstate_alias_read = [0; 4];
            assert_eq!(
                cpu.read_register(arm::CPSR, &mut pstate_alias_read)
                    .unwrap(),
                4
            );
            assert_eq!(
                pstate_alias_read,
                pstate_alias_value.to_le_bytes(),
                "{policy:?}"
            );
            assert_eq!(
                cpu.rpc_calls.get(),
                after_pstate_alias_write + 1,
                "{policy:?}"
            );

            let pc_before_write = cpu.rpc_calls.get();
            cpu.write_register(Register::Pc, &completed_pc).unwrap();
            assert_eq!(cpu.rpc_calls.get(), pc_before_write + 1, "{policy:?}");
            let pc_after_write = cpu.rpc_calls.get();
            let mut actual_pc = [0; 4];
            assert_eq!(cpu.read_register(arm::PC, &mut actual_pc).unwrap(), 4);
            assert_eq!(cpu.rpc_calls.get(), pc_after_write + 1, "{policy:?}");
            assert_eq!(actual_pc, completed_pc, "{policy:?}");
            assert_eq!(cpu.stopped_pstate.get(), None, "{policy:?}");
            let after_pc_write = cpu.rpc_calls.get();
            let mut pstate_after_pc_write = [0; 4];
            assert_eq!(
                cpu.read_register(Register::Pstate, &mut pstate_after_pc_write)
                    .unwrap(),
                4
            );
            assert_eq!(
                u32::from_le_bytes(pstate_after_pc_write),
                pstate_alias_value,
                "{policy:?}"
            );
            assert_eq!(cpu.rpc_calls.get(), after_pc_write + 1, "{policy:?}");

            let before_restore = cpu.rpc_calls.get();
            cpu.restore(snapshot.as_ref()).unwrap();
            let after_restore = cpu.rpc_calls.get();
            assert_eq!(after_restore, before_restore + 1, "{policy:?}");
            assert_eq!(cpu.stopped_pc.get(), None, "{policy:?}");
            assert_eq!(cpu.stopped_pstate.get(), None, "{policy:?}");
            let mut restored_pc = [0; 4];
            assert_eq!(
                cpu.read_register(Register::Pc, &mut restored_pc).unwrap(),
                4
            );
            assert_eq!(cpu.rpc_calls.get(), after_restore + 1, "{policy:?}");
            assert_eq!(u32::from_le_bytes(restored_pc), 0x1000, "{policy:?}");
            let mut restored_pstate = [0; 4];
            assert_eq!(
                cpu.read_register(Register::Pstate, &mut restored_pstate)
                    .unwrap(),
                4
            );
            assert_eq!(u32::from_le_bytes(restored_pstate), 0xd3, "{policy:?}");
            assert_eq!(cpu.rpc_calls.get(), after_restore + 2, "{policy:?}");

            let restarted = cpu
                .start_run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(0),
                })
                .unwrap();
            assert_eq!(cpu.stopped_pstate.get(), None, "{policy:?}");
            let before_restart_read = cpu.rpc_calls.get();
            let mut restarted_pstate = [0; 4];
            assert_eq!(
                cpu.read_register(arm::CPSR, &mut restarted_pstate).unwrap(),
                4
            );
            assert_eq!(cpu.rpc_calls.get(), before_restart_read + 1, "{policy:?}");
            assert_eq!(u32::from_le_bytes(restarted_pstate), 0xd3, "{policy:?}");
            assert!(matches!(
                poll_to_completion(&cpu, restarted).unwrap(),
                Exit::Stopped
            ));
            let after_restart_completion = cpu.rpc_calls.get();
            assert_eq!(read_u32(&cpu, Register::Pstate), 0xd3, "{policy:?}");
            assert_eq!(cpu.rpc_calls.get(), after_restart_completion, "{policy:?}");
        }
    }

    #[test]
    fn loop_window_cache_tracks_core_run_reset_without_extra_rpcs() {
        for policy in [ExecutionPolicy::Exact, ExecutionPolicy::Interactive] {
            let cpu = worker(SharedMemory::default());
            cpu.set_execution_policy(policy).unwrap();

            let initial = cpu.rpc_calls.get();
            cpu.set_loop_window(false).unwrap();
            assert_eq!(cpu.rpc_calls.get(), initial, "{policy:?}");

            cpu.set_loop_window(true).unwrap();
            let after_enable = cpu.rpc_calls.get();
            assert_eq!(after_enable, initial + 1, "{policy:?}");
            cpu.set_loop_window(true).unwrap();
            assert_eq!(cpu.rpc_calls.get(), after_enable, "{policy:?}");

            let run = cpu
                .start_run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(0),
                })
                .unwrap();
            assert_eq!(cpu.loop_window.get(), None, "{policy:?}");
            assert!(matches!(
                poll_to_completion(&cpu, run).unwrap(),
                Exit::Stopped
            ));
            assert_eq!(cpu.loop_window.get(), Some(false), "{policy:?}");
            let after_run = cpu.rpc_calls.get();
            cpu.set_loop_window(false).unwrap();
            cpu.set_loop_window(false).unwrap();
            assert_eq!(cpu.rpc_calls.get(), after_run, "{policy:?}");
        }
    }

    #[test]
    fn stopped_progress_snapshots_cover_pending_and_completed_cluster_peers() {
        struct ReadCount(Cell<u32>);
        unsafe fn read(_: u64, size: u32, data: *mut std::ffi::c_void) -> u64 {
            assert_eq!(size, 4);
            unsafe { &*data.cast::<ReadCount>() }.0.set(1);
            0x1234
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

        let memory = SharedMemory::default();
        let backing = Backing::aligned_zeros(4096, 4096).unwrap();
        let pointer = NonNull::new(backing.as_ptr()).unwrap();
        let first = worker(memory.clone());
        let second = worker(memory);
        first
            .set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        second
            .set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        unsafe {
            first
                .map_physical(0x1000, 0x1000, 4096, 7, pointer)
                .unwrap();
            second
                .map_physical(0x1000, 0x1000, 4096, 7, pointer)
                .unwrap();
        }
        write_words(&first, 0x1000, &[0xe5910000, 0xe320f003]);
        write_words(&first, 0x1100, &[0xeafffffe]); // b .
        first
            .write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        let mut read_count = ReadCount(Cell::new(0));
        unsafe {
            first
                .map_mmio(
                    0x8000,
                    4096,
                    Mmio {
                        read,
                        write,
                        data: (&mut read_count as *mut ReadCount).cast(),
                    },
                )
                .unwrap();
        }

        let first_run = first
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        let second_run = second
            .start_run(Run {
                begin: 0x1100,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match first.poll_run(first_run).unwrap() {
                RunStatus::CallbackPending => break,
                RunStatus::Running => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "MMIO callback did not park"
                    );
                    std::thread::yield_now();
                }
                RunStatus::Complete(result) => {
                    panic!("first worker completed before callback: {result:?}")
                }
            }
        }
        assert!(matches!(
            poll_to_completion(&second, second_run).unwrap(),
            Exit::Stopped
        ));
        let first_rpcs = first.rpc_calls.get();
        let second_rpcs = second.rpc_calls.get();
        let first_progress = first
            .instruction_progress()
            .expect("parked callback progress");
        let second_progress = second
            .instruction_progress()
            .expect("completed peer progress");
        assert!(first_progress >= 1);
        assert_eq!(second_progress, 1);
        assert_eq!(first.rpc_calls.get(), first_rpcs);
        assert_eq!(second.rpc_calls.get(), second_rpcs);

        first.service_pending_callback(first_run).unwrap();
        assert!(matches!(
            poll_to_completion(&first, first_run).unwrap(),
            Exit::Stopped
        ));
        assert_eq!(read_count.0.get(), 1);
    }

    #[test]
    fn interactive_fast_mmio_reads_use_declared_atomic_snapshots() {
        struct ReadCount(Cell<u32>);
        unsafe fn read(_: u64, size: u32, data: *mut std::ffi::c_void) -> u64 {
            assert_eq!(size, 4);
            let count = unsafe { &*data.cast::<ReadCount>() };
            count.0.set(count.0.get() + 1);
            0x1111
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

        let cpu = worker(SharedMemory::default());
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        write_words(&cpu, 0x1000, &[0xe5910000, 0xe320f003]);
        cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        let mut count = ReadCount(Cell::new(0));
        let callbacks = Mmio {
            read,
            write,
            data: (&mut count as *mut ReadCount).cast(),
        };
        let value = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0x2222));
        let fast_read = FastMmioRead::atomic_u32(0, value.clone());
        unsafe {
            cpu.map_mmio_fast(0x8000, 4096, callbacks, vec![fast_read.clone()])
                .unwrap();
        }

        value.store(0x3333, std::sync::atomic::Ordering::Release);
        cpu.run(Run {
            begin: 0x1000,
            until: None,
            watchdog_us: 0,
            instructions: Some(1),
        })
        .unwrap();
        assert_eq!(read_u32(&cpu, arm::CORE[0]), 0x3333);
        assert_eq!(read_u32(&cpu, arm::PC), 0x1004);
        assert!(cpu.instruction_progress().unwrap() >= 1);
        assert_eq!(count.0.get(), 0);

        cpu.set_execution_policy(ExecutionPolicy::Exact).unwrap();
        cpu.write_register(arm::PC, &0x1000u32.to_le_bytes())
            .unwrap();
        cpu.run(Run {
            begin: 0x1000,
            until: None,
            watchdog_us: 0,
            instructions: Some(3),
        })
        .unwrap();
        assert_eq!(read_u32(&cpu, arm::CORE[0]), 0x1111);
        assert_eq!(count.0.get(), 1);
    }

    #[test]
    fn consuming_mmio_preserves_read_observers_and_reserves_the_owner_boundary() {
        unsafe fn read(_: u64, _: u32, data: *mut std::ffi::c_void) -> u64 {
            let calls = unsafe { &*data.cast::<Cell<u32>>() };
            calls.set(calls.get() + 1);
            0x44
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}
        unsafe fn observe(
            access: Access,
            address: u64,
            size: i32,
            _: i64,
            data: *mut std::ffi::c_void,
        ) {
            assert_eq!((access, address, size), (Access::Read, 0x8000, 4));
            let calls = unsafe { &*data.cast::<Cell<u32>>() };
            calls.set(calls.get() + 1);
        }
        let cpu = worker(SharedMemory::default());
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        write_words(&cpu, 0x1000, &[0xe5910000, 0xe5912000, 0xe320f003]);
        cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        let consumed = Arc::new(AtomicU32::new(0));
        let cursor = consumed.clone();
        let fast = FastMmioRead::consuming_u32(
            0,
            Arc::new(move || {
                cursor
                    .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                    .ok()
                    .map(|_| 0x33)
            }),
        );
        let mut owner_calls = Cell::new(0u32);
        let mut observer_calls = Cell::new(0u32);
        unsafe {
            cpu.map_mmio_fast(
                0x8000,
                4096,
                Mmio {
                    read,
                    write,
                    data: (&mut owner_calls as *mut Cell<u32>).cast(),
                },
                vec![fast],
            )
            .unwrap();
            cpu.install_hook(Hook::Memory {
                callback: observe,
                data: (&mut observer_calls as *mut Cell<u32>).cast(),
                read: true,
                write: false,
                range: HookRange::Inclusive {
                    begin: 0x8000,
                    end: 0x8003,
                },
            })
            .unwrap();
        }
        assert!(cpu.read_memory(0x8000, &mut [0; 4]).is_err());
        assert_eq!(consumed.load(Ordering::Acquire), 0);
        cpu.run(Run {
            begin: 0x1000,
            until: None,
            watchdog_us: 0,
            instructions: Some(24),
        })
        .unwrap();
        assert_eq!(read_u32(&cpu, arm::CORE[0]), 0x33);
        assert_eq!(read_u32(&cpu, arm::CORE[2]), 0x44);
        assert_eq!(owner_calls.get(), 1);
        assert_eq!(observer_calls.get(), 2);
        assert_eq!(consumed.load(Ordering::Acquire), 1);
    }

    #[test]
    fn asynchronous_completion_is_repeatable_until_the_next_run() {
        let cpu = worker(SharedMemory::default());
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        write_words(&cpu, 0x1000, &[0xe3a00007, 0xe2800001]);

        let run = cpu
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(2),
            })
            .unwrap();
        assert!(matches!(
            poll_to_completion(&cpu, run).unwrap(),
            Exit::Stopped
        ));
        assert!(matches!(
            cpu.poll_run(run).unwrap(),
            RunStatus::Complete(Ok(Exit::Stopped))
        ));

        let next = cpu
            .start_run(Run {
                begin: 0x1008,
                until: None,
                watchdog_us: 0,
                instructions: Some(0),
            })
            .unwrap();
        assert_ne!(run, next);
        assert!(cpu.poll_run(run).is_err());
        assert!(matches!(
            poll_to_completion(&cpu, next).unwrap(),
            Exit::Stopped
        ));
        assert!(matches!(
            cpu.poll_run(next).unwrap(),
            RunStatus::Complete(Ok(Exit::Stopped))
        ));
    }

    #[test]
    fn asynchronous_callbacks_park_until_explicit_owner_service() {
        struct ReadProbe {
            calls: Cell<u32>,
        }
        unsafe fn read(_: u64, size: u32, data: *mut std::ffi::c_void) -> u64 {
            assert_eq!(size, 4);
            let probe = unsafe { &*data.cast::<ReadProbe>() };
            probe.calls.set(probe.calls.get() + 1);
            0x7654_3210
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

        let memory = SharedMemory::default();
        let cpu = worker(memory.clone());
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        write_words(&cpu, 0x1000, &[0xe5910000]);
        cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        let mut probe = ReadProbe {
            calls: Cell::new(0),
        };
        unsafe {
            cpu.map_mmio(
                0x8000,
                4096,
                Mmio {
                    read,
                    write,
                    data: (&mut probe as *mut ReadProbe).cast(),
                },
            )
            .unwrap();
        }

        let wakeup = RunWakeup::default();
        cpu.set_run_wakeup(wakeup.clone()).unwrap();
        let observed = wakeup.generation();
        let run = cpu
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let parked = loop {
            match cpu.poll_run(run).unwrap() {
                RunStatus::CallbackPending => break true,
                RunStatus::Running => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "MMIO callback was not parked"
                    );
                    std::thread::yield_now();
                }
                RunStatus::Complete(result) => {
                    panic!("run completed before callback service: {result:?}")
                }
            }
        };
        assert!(parked);
        assert!(memory.processor().require_stopped().is_ok());
        assert_eq!(probe.calls.get(), 0);
        assert!(wakeup.generation() > observed);
        assert!(matches!(
            cpu.poll_run(run).unwrap(),
            RunStatus::CallbackPending
        ));

        cpu.service_pending_callback(run).unwrap();
        assert!(matches!(
            poll_to_completion(&cpu, run).unwrap(),
            Exit::Stopped
        ));
        assert_eq!(probe.calls.get(), 1);
        assert_eq!(read_u32(&cpu, arm::CORE[0]), 0x7654_3210);
    }

    #[test]
    fn canceling_a_parked_callback_stops_without_invoking_the_owner() {
        struct ReadProbe(Cell<u32>);
        unsafe fn read(_: u64, _: u32, data: *mut std::ffi::c_void) -> u64 {
            unsafe { &*data.cast::<ReadProbe>() }.0.set(1);
            0x7654_3210
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

        let cpu = worker(SharedMemory::default());
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        write_words(&cpu, 0x1000, &[0xe5910000]); // ldr r0,[r1]
        cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        let mut probe = ReadProbe(Cell::new(0));
        unsafe {
            cpu.map_mmio(
                0x8000,
                4096,
                Mmio {
                    read,
                    write,
                    data: (&mut probe as *mut ReadProbe).cast(),
                },
            )
            .unwrap();
        }

        let run = cpu
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match cpu.poll_run(run).unwrap() {
                RunStatus::CallbackPending => break,
                RunStatus::Running => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "MMIO callback was not parked"
                    );
                    std::thread::yield_now();
                }
                RunStatus::Complete(result) => {
                    panic!("run completed before cancellation: {result:?}")
                }
            }
        }
        cpu.cancel_pending_callback(run).unwrap();
        assert_eq!(cpu.stopped_pc.get(), None);
        assert!(matches!(
            poll_to_completion(&cpu, run),
            Err(ExecutionError::Backend(_))
        ));
        assert_eq!(probe.0.get(), 0);
    }

    #[test]
    fn dropping_worker_with_pending_mmio_callback_exits() {
        const CHILD: &str = "FUJIMOD_RPROFILE_PENDING_CALLBACK_DROP_CHILD";
        if std::env::var_os(CHILD).is_some() {
            struct ReadProbe(Cell<u32>);
            unsafe fn read(_: u64, _: u32, data: *mut std::ffi::c_void) -> u64 {
                unsafe { &*data.cast::<ReadProbe>() }.0.set(1);
                0x7654_3210
            }
            unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

            let cpu = worker(SharedMemory::default());
            cpu.map_zeroed(0x1000, 4096, 7).unwrap();
            write_words(&cpu, 0x1000, &[0xe5910000]); // ldr r0,[r1]
            cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
                .unwrap();
            let mut probe = ReadProbe(Cell::new(0));
            unsafe {
                cpu.map_mmio(
                    0x8000,
                    4096,
                    Mmio {
                        read,
                        write,
                        data: (&mut probe as *mut ReadProbe).cast(),
                    },
                )
                .unwrap();
            }
            let run = cpu
                .start_run(Run {
                    begin: 0x1000,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(1),
                })
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                match cpu.poll_run(run).unwrap() {
                    RunStatus::CallbackPending => break,
                    RunStatus::Running => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "MMIO callback was not parked before worker drop"
                        );
                        std::thread::yield_now();
                    }
                    RunStatus::Complete(result) => {
                        panic!("run completed before callback park: {result:?}")
                    }
                }
            }
            assert_eq!(probe.0.get(), 0, "owner callback ran before drop");
            drop(cpu);
            assert_eq!(probe.0.get(), 0, "worker serviced MMIO while shutting down");
            return;
        }

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("worker::tests::dropping_worker_with_pending_mmio_callback_exits")
            .arg("--nocapture")
            .env(CHILD, "1")
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "worker shutdown child failed: {status}");
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("worker drop hung while an MMIO callback was pending");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn exact_worker_runs_refuse_concurrent_peers_before_guest_entry() {
        struct ReadProbe(Cell<u32>);
        unsafe fn read(_: u64, _: u32, data: *mut std::ffi::c_void) -> u64 {
            unsafe { &*data.cast::<ReadProbe>() }.0.set(1);
            0x1234
        }
        unsafe fn write(_: u64, _: u32, _: u64, _: *mut std::ffi::c_void) {}

        let memory = SharedMemory::default();
        let first = worker(memory.clone());
        let second = worker(memory);
        first.map_zeroed(0x1000, 4096, 7).unwrap();
        second.map_zeroed(0x1000, 4096, 7).unwrap();
        write_words(&first, 0x1000, &[0xe5910000]); // ldr r0,[r1]
        write_words(&second, 0x1000, &[0xe3a00007]); // mov r0,#7
        first
            .write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        let mut probe = ReadProbe(Cell::new(0));
        unsafe {
            first
                .map_mmio(
                    0x8000,
                    4096,
                    Mmio {
                        read,
                        write,
                        data: (&mut probe as *mut ReadProbe).cast(),
                    },
                )
                .unwrap();
        }

        let first_run = first
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match first.poll_run(first_run).unwrap() {
                RunStatus::CallbackPending => break,
                RunStatus::Running => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "first exact run did not park"
                    );
                    std::thread::yield_now();
                }
                RunStatus::Complete(result) => {
                    panic!("first exact run completed before its MMIO callback: {result:?}")
                }
            }
        }
        assert!(
            first
                .set_execution_policy(ExecutionPolicy::Interactive)
                .unwrap_err()
                .contains("requires a stopped processor")
        );

        let second_run = second
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        let error = poll_to_completion(&second, second_run).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("require interactive execution policy")
        );

        first.cancel_pending_callback(first_run).unwrap();
        assert!(poll_to_completion(&first, first_run).is_err());
        assert_eq!(probe.0.get(), 0);
        assert_eq!(read_u32(&second, arm::CORE[0]), 0);
    }

    #[test]
    fn synchronization_hook_parks_before_ldrex_and_resumes_the_instruction() {
        struct Probe(Cell<u32>);
        unsafe fn barrier(data: *mut std::ffi::c_void) {
            unsafe { &*data.cast::<Probe>() }.0.set(1);
        }

        let memory = SharedMemory::default();
        let cpu = worker(memory.clone());
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        let data = Backing::aligned_zeros(4096, 4096).unwrap();
        unsafe {
            cpu.map_physical(
                0x8000,
                0x8000,
                4096,
                7,
                NonNull::new(data.as_ptr()).unwrap(),
            )
            .unwrap();
        }
        write_words(&cpu, 0x1000, &[0xe1910f9f]); // ldrex r0,[r1]
        cpu.write_memory(0x8000, &0x7654_3210u32.to_le_bytes())
            .unwrap();
        cpu.write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        let mut probe = Probe(Cell::new(0));
        unsafe {
            cpu.install_hook(Hook::Synchronization {
                callback: barrier,
                data: (&mut probe as *mut Probe).cast(),
            })
            .unwrap();
        }

        let run = cpu
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match cpu.poll_run(run).unwrap() {
                RunStatus::CallbackPending => break,
                RunStatus::Running => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "sync hook was not parked"
                    );
                    std::thread::yield_now();
                }
                RunStatus::Complete(result) => panic!("run completed before barrier: {result:?}"),
            }
        }
        assert_eq!(probe.0.get(), 0);
        assert!(memory.processor().require_stopped().is_ok());
        cpu.service_pending_callback(run).unwrap();
        assert!(matches!(
            poll_to_completion(&cpu, run).unwrap(),
            Exit::Stopped
        ));
        assert_eq!(probe.0.get(), 1);
        assert_eq!(read_u32(&cpu, arm::CORE[0]), 0x7654_3210);
        assert_eq!(read_u32(&cpu, arm::PC), 0x1004);
    }

    #[test]
    fn interactive_store_barrier_coalescing_requires_and_resets_a_parked_cohort() {
        struct Probe(Cell<u32>);
        unsafe fn barrier(data: *mut std::ffi::c_void) {
            let probe = unsafe { &*data.cast::<Probe>() };
            probe.0.set(probe.0.get() + 1);
        }
        fn wait_for_callback(cpu: &CpuWorker, run: RunToken) -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                match cpu.poll_run(run).unwrap() {
                    RunStatus::CallbackPending => return true,
                    RunStatus::Running => {
                        if std::time::Instant::now() >= deadline {
                            return false;
                        }
                        std::thread::yield_now();
                    }
                    RunStatus::Complete(_) => return false,
                }
            }
        }
        fn finish_run(cpu: &CpuWorker, run: RunToken) -> (Result<Exit, ExecutionError>, u32) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut callbacks = 0;
            loop {
                match cpu.poll_run(run).unwrap() {
                    RunStatus::Running => {
                        if std::time::Instant::now() >= deadline {
                            let _ = cpu.request_pause(run);
                            return (
                                Err(ExecutionError::Backend("worker run timed out".into())),
                                callbacks,
                            );
                        }
                        std::thread::yield_now();
                    }
                    RunStatus::CallbackPending => {
                        callbacks += 1;
                        if cpu.service_pending_callback(run).is_err() {
                            let _ = cpu.cancel_pending_callback(run);
                        }
                    }
                    RunStatus::Complete(result) => return (result, callbacks),
                }
            }
        }

        let code = Backing::aligned_zeros(4096, 4096).unwrap();
        let data = Backing::aligned_zeros(4096, 4096).unwrap();
        let memory = SharedMemory::default();
        let first = worker(memory.clone());
        let second = worker(memory.clone());
        unsafe {
            first
                .map_physical(
                    0x1000,
                    0x1000,
                    4096,
                    7,
                    NonNull::new(code.as_ptr()).unwrap(),
                )
                .unwrap();
            first
                .map_physical(
                    0x5000,
                    0x1000,
                    4096,
                    7,
                    NonNull::new(code.as_ptr()).unwrap(),
                )
                .unwrap();
            first
                .map_physical(
                    0x8000,
                    0x8000,
                    4096,
                    7,
                    NonNull::new(data.as_ptr()).unwrap(),
                )
                .unwrap();
            second
                .map_physical(
                    0x8000,
                    0x8000,
                    4096,
                    7,
                    NonNull::new(data.as_ptr()).unwrap(),
                )
                .unwrap();
        }
        first.map_zeroed(0x11000, 4096, 7).unwrap();
        first.map_zeroed(0x12000, 4096, 7).unwrap();
        second.map_zeroed(0x9000, 4096, 7).unwrap();
        write_words(
            &first,
            0x1000,
            &[
                0xe5812000, // str r2, [r1]
                0xe5814000, // str r4, [r1]
                0xe5832000, // str r2, [r3] through a code alias
                0xe320f003, // wfi
            ],
        );
        write_words(&first, 0x11000, &[0xe5812000, 0xe5814000, 0xe320f003]);
        write_words(&first, 0x12000, &[0xe1910f9f]); // ldrex r0, [r1]
        write_words(&second, 0x9000, &[0xe1910f9f]); // ldrex r0, [r1]
        first
            .write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();
        first
            .write_register(arm::CORE[2], &0xe320f003u32.to_le_bytes())
            .unwrap();
        first
            .write_register(arm::CORE[3], &0x5000u32.to_le_bytes())
            .unwrap();
        first
            .write_register(arm::CORE[4], &0x1357_9bdfu32.to_le_bytes())
            .unwrap();
        second
            .write_register(arm::CORE[1], &0x8000u32.to_le_bytes())
            .unwrap();

        // Prime the shared exclusive-watch page before either worker runs in
        // parallel. Later ordinary stores to that page use the synchronization
        // hook without invalidating translated code.
        let prime = second
            .start_run(Run {
                begin: 0x9000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        assert!(matches!(
            poll_to_completion(&second, prime).unwrap(),
            Exit::Stopped
        ));

        for cpu in [&first, &second] {
            cpu.set_execution_policy(ExecutionPolicy::Interactive)
                .unwrap();
        }
        let mut first_probe = Probe(Cell::new(0));
        let mut second_probe = Probe(Cell::new(0));
        for (cpu, probe) in [(&first, &mut first_probe), (&second, &mut second_probe)] {
            unsafe {
                cpu.install_hook(Hook::Synchronization {
                    callback: barrier,
                    data: (probe as *mut Probe).cast(),
                })
                .unwrap();
            }
        }
        let before = first.exit_counters().unwrap();
        let first_run = first
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(8),
            })
            .unwrap();
        let second_run = second
            .start_run(Run {
                begin: 0x9000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        let first_parked = wait_for_callback(&first, first_run);
        let second_parked = wait_for_callback(&second, second_run);
        first.request_pause(first_run).unwrap();
        second.request_pause(second_run).unwrap();
        if first_parked {
            let _ = first.service_pending_callback(first_run);
        }
        let (first_result, first_extra_callbacks) = finish_run(&first, first_run);
        if second_parked {
            let _ = second.service_pending_callback(second_run);
        }
        let (second_result, second_extra_callbacks) = finish_run(&second, second_run);

        let first_exit = first_result.unwrap_or(Exit::Stopped);
        let second_exit = second_result.unwrap_or(Exit::Stopped);
        let after = first.exit_counters().unwrap();
        assert!(
            first_parked && second_parked,
            "both workers must park before the barrier"
        );
        assert!(matches!(first_exit, Exit::Stopped));
        assert!(matches!(second_exit, Exit::Stopped));
        assert_eq!(first_extra_callbacks, 0);
        assert_eq!(second_extra_callbacks, 0);
        assert_eq!(first.pause_requested.load(Ordering::Acquire), 1);
        assert_eq!(second.pause_requested.load(Ordering::Acquire), 1);
        assert_eq!(first_probe.0.get(), 1);
        assert_eq!(second_probe.0.get(), 1); // the concurrent LDREX barrier
        assert_eq!(
            after["worker_callback_synchronization"] - before["worker_callback_synchronization"],
            1
        );
        assert!(after["synchronization_requested"] - before["synchronization_requested"] >= 3);
        assert!(after["synchronization_elided"] - before["synchronization_elided"] >= 1);
        assert_eq!(
            after["synchronization_barriers"] - before["synchronization_barriers"],
            1
        );

        // The peer's LDREX callback was serviced only after the first run's
        // lease ended. Its new run observes the completed ordinary stores.
        let mut observed = [0; 4];
        second.read_memory(0x8000, &mut observed).unwrap();
        assert_eq!(u32::from_le_bytes(observed), 0x1357_9bdf);
        let load = second
            .start_run(Run {
                begin: 0x9000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        let (load_result, _) = finish_run(&second, load);
        assert!(matches!(load_result.unwrap(), Exit::Stopped));
        assert_eq!(read_u32(&second, arm::CORE[0]), 0x1357_9bdf);

        write_words(&second, 0x9004, &[0xe1813f92]); // strex r3, r2, [r1]
        second
            .write_register(arm::CORE[2], &0x2468_ace0u32.to_le_bytes())
            .unwrap();
        let exclusive_run = second
            .start_run(Run {
                begin: 0x9004,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        let (exclusive_result, _) = finish_run(&second, exclusive_run);
        assert!(matches!(exclusive_result.unwrap(), Exit::Stopped));
        assert_eq!(read_u32(&second, arm::CORE[3]), 0);
        let mut exclusive_value = [0; 4];
        second.read_memory(0x8000, &mut exclusive_value).unwrap();
        assert_eq!(u32::from_le_bytes(exclusive_value), 0x2468_ace0);

        // Resume the stopped code-alias store in a fresh run. Its invalidation
        // must still reach every translated alias before the new instruction
        // is fetched.
        let alias_store = first
            .start_run(Run {
                begin: 0x1008,
                until: None,
                watchdog_us: 0,
                instructions: Some(8),
            })
            .unwrap();
        let (alias_result, _) = finish_run(&first, alias_store);
        assert!(matches!(
            alias_result.unwrap(),
            Exit::Hint(Hint::WaitForInterrupt)
        ));
        let mut replaced = [0; 4];
        first.read_memory(0x1000, &mut replaced).unwrap();
        assert_eq!(u32::from_le_bytes(replaced), 0xe320f003);
        let retranslated = first
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: Some(1),
            })
            .unwrap();
        assert!(matches!(
            poll_to_completion(&first, retranslated).unwrap(),
            Exit::Hint(Hint::WaitForInterrupt)
        ));
        assert_eq!(read_u32(&first, arm::CORE[0]), 0);

        let before_next_run = first.exit_counters().unwrap();
        let next_run = first
            .start_run(Run {
                begin: 0x11000,
                until: None,
                watchdog_us: 0,
                instructions: Some(16),
            })
            .unwrap();
        let next_parked = wait_for_callback(&first, next_run);
        first.request_pause(next_run).unwrap();
        if next_parked {
            let _ = first.service_pending_callback(next_run);
        }
        let (next_result, extra_callbacks) = finish_run(&first, next_run);
        let next_callbacks = u32::from(next_parked) + extra_callbacks;
        let after_next_run = first.exit_counters().unwrap();
        assert!(next_parked);
        assert!(matches!(
            next_result.unwrap(),
            Exit::Stopped | Exit::Hint(Hint::WaitForInterrupt)
        ));
        assert_eq!(next_callbacks, 1);
        assert_eq!(
            after_next_run["worker_callback_synchronization"]
                - before_next_run["worker_callback_synchronization"],
            1
        );
        assert_eq!(
            after_next_run["synchronization_elided"] - before_next_run["synchronization_elided"],
            1
        );
        assert_eq!(
            after_next_run["synchronization_barriers"]
                - before_next_run["synchronization_barriers"],
            1
        );
    }

    #[test]
    fn asynchronous_pause_stops_chained_interactive_blocks() {
        let cpu = worker(SharedMemory::default());
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        cpu.map_zeroed(0x1000, 4096, 7).unwrap();
        write_words(&cpu, 0x1000, &[0xeaff_fffe]); // b .
        let run = cpu
            .start_run(Run {
                begin: 0x1000,
                until: None,
                watchdog_us: 0,
                instructions: None,
            })
            .unwrap();
        cpu.request_pause(run).unwrap();
        assert!(matches!(
            poll_to_completion(&cpu, run).unwrap(),
            Exit::Stopped
        ));
    }

    /// Four concurrently running A53 workers must each honor a pause. The owner
    /// services every parked callback only after all of them stop, so one core
    /// that misses the request stalls the whole cohort.
    #[test]
    fn four_concurrent_a53_workers_all_honor_a_pause() {
        let memory = SharedMemory::default();
        let backing = Backing::aligned_zeros(0x4000, 4096).unwrap();
        let cpus: Vec<_> = (0..4)
            .map(|core| {
                let cpu = spawn_aprofile(1 << 20, memory.clone()).unwrap();
                cpu.set_execution_policy(ExecutionPolicy::Interactive)
                    .unwrap();
                let address = 0x1000 + core as u64 * 0x100;
                unsafe {
                    cpu.map_physical(
                        0,
                        0,
                        0x4000,
                        7,
                        NonNull::new(backing.as_ptr()).unwrap(),
                    )
                    .unwrap();
                }
                let code = [0x9100_0400u32, 0x9100_0400, 0x17ff_fffe];
                let bytes: Vec<_> = code.into_iter().flat_map(u32::to_le_bytes).collect();
                cpu.write_memory(address, &bytes).unwrap();
                (cpu, address)
            })
            .collect::<Vec<_>>();
        let runs: Vec<_> = cpus
            .iter()
            .map(|(cpu, address)| {
                cpu.start_run(Run {
                    begin: *address,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(50_000_000),
                })
                .unwrap()
            })
            .collect();
        std::thread::sleep(std::time::Duration::from_millis(10));
        for ((cpu, _), run) in cpus.iter().zip(&runs) {
            cpu.request_pause(*run).unwrap();
        }
        let started = std::time::Instant::now();
        for ((cpu, _), run) in cpus.iter().zip(&runs) {
            loop {
                match cpu.poll_run(*run).unwrap() {
                    RunStatus::Running => {
                        assert!(
                            started.elapsed() < std::time::Duration::from_secs(10),
                            "an A53 worker never reached a pause boundary"
                        );
                        std::thread::yield_now();
                    }
                    RunStatus::CallbackPending => cpu.service_pending_callback(*run).unwrap(),
                    RunStatus::Complete(result) => {
                        assert!(matches!(result.unwrap(), Exit::Stopped));
                        break;
                    }
                }
            }
        }
    }

    /// Every interactive window must start and stop a worker promptly, and a
    /// pause must be honored across repeated windows without the owner's
    /// request outliving the run that observed it.
    #[test]
    fn interactive_windows_start_and_stop_an_a53_worker_promptly() {
        let cpu = spawn_aprofile(1 << 20, SharedMemory::default()).unwrap();
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        let address = 0x1000u64;
        cpu.map_zeroed(address, 0x4000, 7).unwrap();
        let code = [0x9100_0400u32, 0x9100_0400, 0x17ff_fffe]; // add; add; b .
        let bytes: Vec<_> = code.into_iter().flat_map(u32::to_le_bytes).collect();
        cpu.write_memory(address, &bytes).unwrap();
        for window in 0..500u64 {
            let run = cpu
                .start_run(Run {
                    begin: address,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(1024),
                })
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let exit = loop {
                match cpu.poll_run(run).unwrap() {
                    RunStatus::Running => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "window {window} did not stop"
                        );
                        std::thread::yield_now();
                    }
                    RunStatus::CallbackPending => cpu.service_pending_callback(run).unwrap(),
                    RunStatus::Complete(result) => break result.unwrap(),
                }
            };
            assert!(matches!(exit, Exit::Stopped), "window {window}: {exit:?}");
            // The pause handle is cleared before every run; a request left
            // set by a previous window would stop this one before entry.
            assert!(
                cpu.instruction_progress().unwrap() >= 1024,
                "window {window} retired no work"
            );
        }
    }

    /// An interactive window ends when the cohort crosses its boundary. A core
    /// still inside its instruction budget must therefore stop at the next
    /// block boundary, not when its budget runs out: the owner requests a
    /// pause and every parked peer waits for this core to reach it.
    #[test]
    fn aprofile_worker_honors_a_pause_well_before_its_instruction_budget() {
        let cpu = spawn_aprofile(1 << 20, SharedMemory::default()).unwrap();
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        let address = 0x1000u64;
        cpu.map_zeroed(address, 0x4000, 7).unwrap();
        let code = [0x9100_0400u32, 0x9100_0400, 0x17ff_fffe]; // add; add; b .
        let bytes: Vec<_> = code.into_iter().flat_map(u32::to_le_bytes).collect();
        cpu.write_memory(address, &bytes).unwrap();
        // Far more than a fast host retires before the pause arrives.
        let budget = 50_000_000_000usize;
        let run = cpu
            .start_run(Run {
                begin: address,
                until: None,
                watchdog_us: 0,
                instructions: Some(budget),
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        cpu.request_pause(run).unwrap();
        let started = std::time::Instant::now();
        let exit = loop {
            match cpu.poll_run(run).unwrap() {
                RunStatus::Running => {
                    assert!(
                        started.elapsed() < std::time::Duration::from_secs(5),
                        "pause was not honored before the instruction budget ran out"
                    );
                    std::thread::yield_now();
                }
                RunStatus::CallbackPending => cpu.service_pending_callback(run).unwrap(),
                RunStatus::Complete(result) => break result.unwrap(),
            }
        };
        assert!(matches!(exit, Exit::Stopped), "unexpected exit: {exit:?}");
        assert!(
            cpu.instruction_progress().unwrap() < budget as u64,
            "pause waited for the whole budget"
        );
    }

    /// The owner raises the pause from another thread while the worker is
    /// starting its run, which is the window where a request used to be lost:
    /// the run cleared its stop flag just after the request arrived.
    #[test]
    fn a_pause_raised_around_run_start_is_never_lost() {
        let cpu = spawn_aprofile(1 << 20, SharedMemory::default()).unwrap();
        cpu.set_execution_policy(ExecutionPolicy::Interactive)
            .unwrap();
        let address = 0x1000u64;
        cpu.map_zeroed(address, 0x4000, 7).unwrap();
        let code = [0x9100_0400u32, 0x9100_0400, 0x17ff_fffe]; // add; add; b .
        let bytes: Vec<_> = code.into_iter().flat_map(u32::to_le_bytes).collect();
        cpu.write_memory(address, &bytes).unwrap();
        for iteration in 0..10_000 {
            let run = cpu
                .start_run(Run {
                    begin: address,
                    until: None,
                    watchdog_us: 0,
                    instructions: Some(100_000_000),
                })
                .unwrap();
            // Racy by design: the request may land before, during or after the
            // run's own entry.
            cpu.request_pause(run).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                match cpu.poll_run(run).unwrap() {
                    RunStatus::Running => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "iteration {iteration} never observed the pause"
                        );
                        std::thread::yield_now();
                    }
                    RunStatus::CallbackPending => cpu.service_pending_callback(run).unwrap(),
                    RunStatus::Complete(result) => {
                        assert!(
                            matches!(result.unwrap(), Exit::Stopped),
                            "iteration {iteration} ended without stopping"
                        );
                        break;
                    }
                }
            }
        }
    }
}
