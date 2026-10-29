//! CPU execution mechanisms. The runtime owns memory, devices and guest time;
//! a backend owns only its processors and translation/execution machinery.
//!
//! Backend handles and ABI register numbers never cross this boundary. Host
//! watchdogs only cancel execution; their elapsed time is not guest time.
use std::any::Any;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};

pub mod arm;
pub mod arm64;
pub mod exclusive;
pub mod loops;
pub mod memory;
pub mod timeline;
#[cfg(target_os = "emscripten")]
mod wasm_backing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Architecture {
    Arm32,
    Arm64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum Model {
    #[serde(rename = "cortex-r5f")]
    CortexR5F,
    #[serde(rename = "cortex-a7")]
    CortexA7,
    #[serde(rename = "cortex-m4")]
    CortexM4,
    #[serde(rename = "armv8a")]
    ArmV8A,
}
impl Model {
    pub fn architecture(self) -> Architecture {
        match self {
            Self::ArmV8A => Architecture::Arm64,
            _ => Architecture::Arm32,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identity(u64);
impl Identity {
    pub fn allocate() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookId {
    pub owner: Identity,
    pub key: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum Diagnostic {
    ExceptionSyndrome,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum Register {
    Diagnostic(Diagnostic),
    Arm32(arm::Register),
    X(u8),
    Pc,
    Sp,
    Pstate,
    Fpcr,
    Fpsr,
    V(u8),
    Arm64System(arm::SystemRegister),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    pub begin: u64,
    /// Last mapped address, inclusive.
    pub end: u64,
    pub permissions: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct Run {
    pub begin: u64,
    pub until: Option<u64>,
    /// Host cancellation only. Zero disables the watchdog.
    pub watchdog_us: u64,
    /// None runs until a requested stop or an architectural exit. Some(0)
    /// performs no execution and leaves the processor's PC unchanged.
    pub instructions: Option<usize>,
}
/// One bounded asynchronous run, scoped to the processor that started it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunToken {
    owner: Identity,
    sequence: u64,
}
impl RunToken {
    pub const fn new(owner: Identity, sequence: u64) -> Self {
        Self { owner, sequence }
    }
    pub const fn owner(self) -> Identity {
        self.owner
    }
    pub const fn sequence(self) -> u64 {
        self.sequence
    }
}
/// Nonblocking state for a run started with [`Cpu::start_run`].
#[derive(Clone, Debug)]
pub enum RunStatus {
    Running,
    /// A guest callback is held until the owner explicitly services it.
    CallbackPending,
    /// Completion remains observable until the next run starts.
    Complete(Result<Exit, ExecutionError>),
}
#[derive(Default)]
struct WakeupState {
    generation: Mutex<u64>,
    changed: Condvar,
}
/// Lossless event notification shared by the owner and asynchronous CPUs.
#[derive(Clone, Default)]
pub struct RunWakeup(Arc<WakeupState>);
impl RunWakeup {
    pub fn generation(&self) -> u64 {
        *self
            .0
            .generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn notify(&self) {
        let mut generation = self
            .0
            .generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *generation = generation.wrapping_add(1);
        self.0.changed.notify_all();
    }
    pub fn wait_for_change(&self, observed: u64) {
        let mut generation = self
            .0
            .generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *generation == observed {
            generation = self
                .0
                .changed
                .wait(generation)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}
/// Instruction accounting behavior selected by the execution owner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExecutionPolicy {
    /// Stop at the requested instruction boundary and preserve exact progress.
    #[default]
    Exact,
    /// Permit backend-defined block-boundary overshoot for interactive runs.
    Interactive,
}
impl ExecutionPolicy {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Interactive => "interactive",
        }
    }
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "exact" => Ok(Self::Exact),
            "interactive" => Ok(Self::Interactive),
            _ => Err(format!("unknown CPU execution policy: {value}")),
        }
    }
}
/// Guest-clock anchor for architectural registers implemented by a backend.
/// Its phase and ratio are supplied by the execution owner, never host time.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub now: u64,
    pub phase: u64,
    pub ratio: crate::timeline::Ratio,
}
impl Clock {
    pub fn after(self, entries: u64) -> Result<Self, String> {
        let (ticks, phase) = self.ratio.advance_phase(self.phase, entries)?;
        let now = self
            .now
            .checked_add(u64::try_from(ticks).map_err(|_| "CPU guest clock span overflow")?)
            .ok_or("CPU guest clock exhausted")?;
        Ok(Self { now, phase, ..self })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Fetch,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interrupt {
    Irq,
    Fiq,
}
/// Guest accesses to a mapped device. Offsets are relative to the mapping;
/// widths are in bytes. Host inspection never invokes these callbacks.
#[derive(Clone, Copy)]
pub struct Mmio {
    pub read: unsafe fn(u64, u32, *mut c_void) -> u64,
    pub write: unsafe fn(u64, u32, u64, *mut c_void),
    pub data: *mut c_void,
}
/// A device-published register value or bounded FIFO word that can be read
/// directly by an execution backend. Clocked values are evaluated against an
/// approximate guest tick and may decline reads outside their valid interval.
#[derive(Clone)]
pub struct FastMmioRead {
    pub register_offset: u64,
    kind: FastMmioReadKind,
    enabled: Arc<AtomicBool>,
}
#[derive(Clone)]
enum FastMmioReadKind {
    AtomicU16(Arc<AtomicU16>),
    AtomicU32(Arc<AtomicU32>),
    ClockedU32(Arc<dyn Fn(u64) -> Option<u32> + Send + Sync>),
    ConsumingU32(Arc<dyn Fn() -> Option<u32> + Send + Sync>),
}
impl FastMmioRead {
    pub fn atomic_u16(register_offset: u64, value: Arc<AtomicU16>) -> Self {
        Self {
            register_offset,
            kind: FastMmioReadKind::AtomicU16(value),
            enabled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn atomic_u32(register_offset: u64, value: Arc<AtomicU32>) -> Self {
        Self {
            register_offset,
            kind: FastMmioReadKind::AtomicU32(value),
            enabled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn clocked_u32(
        register_offset: u64,
        read: Arc<dyn Fn(u64) -> Option<u32> + Send + Sync>,
    ) -> Self {
        Self {
            register_offset,
            kind: FastMmioReadKind::ClockedU32(read),
            enabled: Arc::new(AtomicBool::new(false)),
        }
    }
    /// Consume a prepublished word; the device retains all completion effects.
    pub fn consuming_u32(
        register_offset: u64,
        read: Arc<dyn Fn() -> Option<u32> + Send + Sync>,
    ) -> Self {
        Self {
            register_offset,
            kind: FastMmioReadKind::ConsumingU32(read),
            enabled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }
    /// Return a constant snapshot only for the exact declared access width.
    /// Clocked and consuming reads are excluded from instruction-loop proofs.
    pub fn read(&self, offset: u64, size: u32) -> Option<u64> {
        if offset != self.register_offset || size != self.width() {
            return None;
        }
        match &self.kind {
            FastMmioReadKind::AtomicU16(value) => Some(u64::from(value.load(Ordering::Acquire))),
            FastMmioReadKind::AtomicU32(value) => Some(u64::from(value.load(Ordering::Acquire))),
            FastMmioReadKind::ClockedU32(_) | FastMmioReadKind::ConsumingU32(_) => None,
        }
    }
    fn width(&self) -> u32 {
        match self.kind {
            FastMmioReadKind::AtomicU16(_) => 2,
            _ => 4,
        }
    }
    /// Return a fast value for the declared access width in the enabled policy.
    /// Clocked descriptors receive the backend's approximate guest tick.
    pub fn read_at(&self, offset: u64, size: u32, tick: Option<u64>) -> Option<u64> {
        if !self.enabled.load(Ordering::Acquire)
            || offset != self.register_offset
            || size != self.width()
        {
            return None;
        }
        match &self.kind {
            FastMmioReadKind::AtomicU16(value) => Some(u64::from(value.load(Ordering::Acquire))),
            FastMmioReadKind::AtomicU32(value) => Some(u64::from(value.load(Ordering::Acquire))),
            FastMmioReadKind::ClockedU32(read) => read(tick?).map(u64::from),
            FastMmioReadKind::ConsumingU32(read) => read().map(u64::from),
        }
    }
}

#[cfg(test)]
mod fast_mmio_read_tests {
    use super::*;

    #[test]
    fn halfword_snapshots_require_the_declared_width_and_policy() {
        let value = Arc::new(AtomicU16::new(0x1234));
        let read = FastMmioRead::atomic_u16(2, value.clone());
        assert_eq!(read.read(2, 2), Some(0x1234));
        assert_eq!(read.read_at(2, 2, None), None);
        read.set_enabled(true);
        for size in [1, 4] { assert_eq!(read.read_at(2, size, None), None); }
        assert_eq!(read.read_at(0, 2, None), None);
        value.store(0xabcd, Ordering::Release);
        assert_eq!(read.read_at(2, 2, None), Some(0xabcd));
    }

    #[test]
    fn consuming_reads_never_run_for_inspection_disabled_policies_or_other_widths() {
        let consumed = Arc::new(AtomicU32::new(0));
        let count = consumed.clone();
        let read = FastMmioRead::consuming_u32(0x20, Arc::new(move || Some(count.fetch_add(1, Ordering::Relaxed))));
        assert_eq!(read.read(0x20, 4), None);
        assert_eq!(read.read_at(0x20, 4, None), None);
        read.set_enabled(true);
        assert_eq!(read.read_at(0x20, 2, None), None);
        assert_eq!(read.read_at(0x21, 4, None), None);
        assert_eq!(read.read_at(0x20, 4, None), Some(0));
        assert_eq!(read.read(0x20, 4), None);
        read.set_enabled(false);
        assert_eq!(read.read_at(0x20, 4, None), None);
        assert_eq!(consumed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn clocked_reads_are_not_constant_loop_inputs_and_decline_past_validity() {
        let read = FastMmioRead::clocked_u32(
            0x20,
            Arc::new(|tick| (tick < 20).then_some(tick as u32)),
        );
        assert_eq!(read.read(0x20, 4), None);
        assert_eq!(read.read_at(0x20, 4, Some(19)), None);
        read.set_enabled(true);
        assert_eq!(read.read(0x20, 4), None);
        assert_eq!(read.read_at(0x20, 4, None), None);
        assert_eq!(read.read_at(0x20, 4, Some(19)), Some(19));
        assert_eq!(read.read_at(0x20, 4, Some(20)), None);
        assert_eq!(read.read_at(0x21, 4, Some(19)), None);
        assert_eq!(read.read_at(0x20, 2, Some(19)), None);
    }

    #[test]
    fn atomic_snapshots_remain_constant_inputs_for_exact_loop_proofs() {
        let value = Arc::new(AtomicU32::new(7));
        let read = FastMmioRead::atomic_u32(0x30, value);
        assert_eq!(read.read(0x30, 4), Some(7));
        assert_eq!(read.read_at(0x30, 4, None), None);
        read.set_enabled(true);
        assert_eq!(read.read_at(0x30, 4, None), Some(7));
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    ReadUnmapped,
    WriteUnmapped,
    FetchUnmapped,
    ReadProtected,
    WriteProtected,
    FetchProtected,
}
#[derive(Clone, Copy, Debug)]
pub struct Trap {
    pub syndrome: u64,
    pub virtual_address: u64,
    pub physical_address: u64,
}
/// Architectural hint completed by the backend. PC already names the next
/// instruction, and the hint contributes one entered instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
    WaitForInterrupt,
    WaitForEvent,
    SendEvent,
    SendEventLocal,
}
#[derive(Clone, Copy, Debug)]
pub enum Exit {
    /// A framework-owned instruction has not entered. PC names its unchanged
    /// entry and progress excludes it; the owner completes and charges it.
    BeforeInstruction(u32),
    Stopped,
    Hint(Hint),
    Trap(Trap),
    Timer,
}
#[derive(Clone, Debug)]
pub enum ExecutionError {
    InvalidInstruction(String),
    Memory { fault: Fault, message: String },
    Backend(String),
}
impl ExecutionError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidInstruction(_) => "invalid-instruction",
            Self::Memory { fault, .. } => match fault {
                Fault::ReadUnmapped => "read-unmapped",
                Fault::WriteUnmapped => "write-unmapped",
                Fault::FetchUnmapped => "fetch-unmapped",
                Fault::ReadProtected => "read-protected",
                Fault::WriteProtected => "write-protected",
                Fault::FetchProtected => "fetch-protected",
            },
            Self::Backend(_) => "backend",
        }
    }
}
impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInstruction(message)
            | Self::Memory { message, .. }
            | Self::Backend(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for ExecutionError {}
impl From<String> for ExecutionError {
    fn from(message: String) -> Self {
        Self::Backend(message)
    }
}
impl From<&str> for ExecutionError {
    fn from(message: &str) -> Self {
        Self::Backend(message.into())
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Features {
    /// Runs can be started, polled and paused without blocking the owner.
    pub asynchronous_runs: bool,
    /// Architectural counters consume an explicit guest-clock anchor per run.
    pub guest_clock: bool,
    pub instruction_limit: bool,
    /// RAM map/unmap/protect operations affect every CPU in the owning VM.
    /// MMIO callback installation remains per processor.
    pub shared_physical_maps: bool,
    /// Bounded runs expose their entered-instruction count through
    /// `instruction_progress`, including while stopped in a callback.
    pub instruction_progress: bool,
    /// Framework-owned architectural operations exit directly from run;
    /// the scheduler does not need an instruction hook to discover them.
    pub architectural_exits: bool,
    /// A callback can lower the current run's total entered-instruction cap.
    pub tighten_instruction_limit: bool,
    pub translation_filter: bool,
    pub block_accounting: bool,
    pub watched_stores: bool,
    pub backing_observation: bool,
    pub store_pc_control: bool,
}
#[derive(Clone, Copy, Debug)]
pub enum HookRange {
    All,
    Inclusive { begin: u64, end: u64 },
}
#[derive(Clone, Copy, Debug)]
pub struct Block {
    pub address: u64,
    pub bytes: u32,
    pub thumb: Option<bool>,
}
/// The externally selected input phase used by optional guest block profiles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuestProfilePhase {
    /// Guest execution while no physical key or touch input is active.
    #[default]
    Idle,
    /// Guest execution while a physical key or touch input is active.
    Key,
}
/// Exact instruction totals accumulated at executed guest block starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct GuestBlockCount {
    pub address: u64,
    pub entries: u64,
    pub idle_instructions: u64,
    pub key_instructions: u64,
}
#[derive(Clone, Copy)]
pub enum Hook {
    Instruction {
        callback: unsafe fn(u64, u32, *mut c_void),
        data: *mut c_void,
        range: HookRange,
    },
    Block {
        callback: unsafe fn(Block, *mut c_void),
        data: *mut c_void,
        range: HookRange,
    },
    Exception {
        callback: unsafe fn(arm::Exception, *mut c_void),
        data: *mut c_void,
    },
    /// Request an owner barrier before a processor operation that cannot
    /// overlap another guest core safely, such as a watched store or exclusive.
    /// This boundary is used by Interactive execution, where guest cores may
    /// overlap. Exact execution serializes processor runs and skips the hook.
    Synchronization {
        callback: unsafe fn(*mut c_void),
        data: *mut c_void,
    },
    /// Publish a store after its instruction completes, before another guest
    /// instruction enters. Access is always Write; failed stores do not publish.
    CompletedStore {
        callback: unsafe fn(Access, u64, i32, i64, *mut c_void),
        data: *mut c_void,
        range: HookRange,
    },
    Memory {
        callback: unsafe fn(Access, u64, i32, i64, *mut c_void),
        data: *mut c_void,
        read: bool,
        write: bool,
        range: HookRange,
    },
    Fault {
        callback: unsafe fn(Fault, u64, i32, i64, *mut c_void) -> bool,
        data: *mut c_void,
    },
}
#[derive(Clone, Copy, Debug)]
pub enum InstructionDisposition {
    Execute,
    Observe,
    ObserveAndEndBlock,
    /// The instruction observer performs the instruction, including a failed
    /// condition's no-op, and leaves the PC at the instruction only to stop
    /// before it. The backend translates no body and ends the block after it.
    Replace,
}
#[derive(Clone, Copy, Debug)]
pub struct Translation {
    pub instruction: InstructionDisposition,
    pub observe_block: bool,
}
#[derive(Clone, Copy)]
pub struct TranslationHooks {
    pub classify: unsafe fn(*mut c_void, u64, u32) -> Translation,
    pub restart: unsafe fn(*mut c_void, u64),
    pub data: *mut c_void,
}
/// Optional ARM32 block-budget accelerator. The instruction-span ring and its
/// lifetime belong to the runtime. Fields match the published accelerator ABI;
/// the runtime accesses this record only while execution is stopped/in a hook.
/// `enabled` selects exact ring accounting (1), coarse interactive accounting
/// without ring writes (2), or disabled accounting (0).
#[repr(C)]
pub struct BlockAccounting {
    pub budget: i64,
    pub head: u64,
    pub ring: *mut u64,
    pub mask: u64,
    pub enabled: u64,
    pub pause_requested: *const AtomicU32,
}
/// A checkpoint associated with an owning processor. Durable export carries
/// data only; `Cpu::import_snapshot` binds that data to a receiving processor.
pub trait Snapshot: Any {
    fn owner(&self) -> Identity;
    fn as_any(&self) -> &dyn Any;
    fn export(&self) -> Result<Vec<u8>, String> {
        Err("processor snapshot has no durable encoding".into())
    }
}
/// One thread-affine processor. This trait is deliberately not Send/Sync.
/// The owner serializes execution, inspection, maps and callback installation.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackingObserver {
    pub backing: NonNull<u8>,
    pub bytes: usize,
    pub bitmap: NonNull<AtomicU64>,
    pub page_shift: u32,
}

pub trait Cpu {
    fn identity(&self) -> Identity;
    /// Start optional per-block guest instruction profiling. Backends without
    /// block entry observations may leave the profile unavailable.
    fn start_guest_profile(&self) -> Result<(), String> {
        Ok(())
    }
    /// Mark the phase for subsequent guest block entries.
    fn guest_profile_phase(&self, _phase: GuestProfilePhase) -> Result<(), String> {
        Ok(())
    }
    /// Finish an active guest profile and return its per-block totals.
    fn finish_guest_profile(&self) -> Result<Option<Vec<GuestBlockCount>>, String> {
        Ok(None)
    }
    /// Configure instruction-proven loops; statistics remain observational.
    fn set_loop_mode(&self, mode: loops::LoopMode) -> Result<(), String> {
        if mode == loops::LoopMode::Disabled {
            Ok(())
        } else {
            Err("backend has no exact loop acceleration".into())
        }
    }
    /// The execution owner guarantees no RAM/input changes before this run's
    /// instruction cap: stop at device, DMA, interrupt and runnable-peer events.
    /// The permission expires when the bounded run returns.
    fn set_loop_window(&self, _stable: bool) -> Result<(), String> {
        Ok(())
    }
    /// Select how a bounded run handles its instruction target. Backends that
    /// do not support coarse interactive execution keep exact behavior only.
    fn set_execution_policy(&self, policy: ExecutionPolicy) -> Result<(), String> {
        match policy {
            ExecutionPolicy::Exact => Ok(()),
            ExecutionPolicy::Interactive => {
                Err("processor backend does not support interactive execution policy".into())
            }
        }
    }
    /// External mutable backing covered by a whole-board keyframe. Private
    /// memory retained by the processor snapshot is excluded. Unknown owners
    /// refuse, so a board cannot silently omit independently mapped RAM.
    fn checkpoint_backings(&self) -> Result<Vec<(NonNull<u8>, usize)>, String> {
        Err("processor cannot prove keyframe backing ownership".into())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Err("this processor has no complete state fingerprint".into())
    }
    /// Logical reservations shared by processors in one physical address space.
    /// Framework architectural adapters join this domain through checked RAM views.
    fn exclusive_domain(&self) -> Option<exclusive::Shared> {
        None
    }
    fn model(&self) -> Model;
    fn features(&self) -> Features;
    fn set_guest_clock(&self, _clock: Clock) -> Result<(), String> {
        Err("backend does not consume a guest-clock anchor".into())
    }
    fn read_register(&self, register: Register, out: &mut [u8]) -> Result<usize, String>;
    fn write_register(&self, register: Register, bytes: &[u8]) -> Result<(), String>;
    fn coprocessor(&self, register: arm::Coprocessor, value: Option<u64>) -> Result<u64, String>;
    fn read_memory(&self, address: u64, bytes: &mut [u8]) -> Result<(), String>;
    /// Translate using this CPU's architectural MMU and access permissions.
    /// Host inspection refuses device accesses, including page-table reads.
    fn translate(&self, address: u64, access: Access) -> Result<u64, String>;
    /// Read through the architectural MMU with the specified access rights.
    /// This is observational: a range touching MMIO returns an error.
    fn read_virtual(&self, address: u64, access: Access, bytes: &mut [u8]) -> Result<(), String>;
    fn write_memory(&self, address: u64, bytes: &[u8]) -> Result<(), String>;
    /// Notify a write through shared external backing (DMA or a peer backend).
    /// Required for backends with logical exclusive monitors. Prefer
    /// write_memory for host writes; it performs this notification itself.
    fn memory_written(&self, _address: u64, _bytes: usize) -> Result<(), String> {
        Err("backend does not support external memory-write notification".into())
    }
    /// A committed write to physical backing, bypassing CPU-private mappings.
    /// # Safety
    /// `backing` must address `bytes` live bytes of the declared physical RAM,
    /// with execution serialized as for map_backing. The range may be hidden
    /// by private processor RAM and need not be mapped at `physical` on this CPU.
    unsafe fn backing_written(
        &self,
        physical: u64,
        _backing: NonNull<u8>,
        bytes: usize,
    ) -> Result<(), String> {
        if self.features().shared_physical_maps {
            self.memory_written(physical, bytes)?;
        }
        self.invalidate_code(None)
    }
    fn memory_regions(&self) -> Result<Vec<MemoryRegion>, String>;
    /// # Safety
    /// The backing must stay allocated at this address, and guest writes must
    /// be permitted by its Rust ownership, until this mapping is removed or
    /// the backend address space is destroyed. An owning VM can extend that
    /// lifetime beyond its CPUs. Mappings must not race execution/inspection
    /// on any processor sharing the address space. External writers must use
    /// the backend's memory_written notification when it maintains logical
    /// exclusive monitors; stopping CPUs alone does not invalidate a monitor.
    unsafe fn map_backing(
        &self,
        address: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String>;
    /// Map backing with its canonical physical RAM identity. Address aliases
    /// must preserve that identity for code writes and exclusive reservations.
    /// # Safety
    /// The lifetime, ownership and serialization requirements of map_backing
    /// apply. `physical` must identify these same backing bytes on every core.
    unsafe fn map_physical(
        &self,
        address: u64,
        physical: u64,
        bytes: usize,
        permissions: u32,
        backing: NonNull<u8>,
    ) -> Result<(), String> {
        if address != physical && self.exclusive_domain().is_some() {
            return Err("backend does not support canonical physical RAM aliases".into());
        }
        unsafe { self.map_backing(address, bytes, permissions, backing) }
    }
    /// Replace an existing external RAM window while retaining private CPU
    /// overlays. All processors sharing the memory must be stopped.
    /// # Safety
    /// The backing and identity requirements of map_physical apply. Both
    /// the old and new backing must remain alive through this transaction.
    unsafe fn replace_physical(
        &self,
        _address: u64,
        _physical: u64,
        _bytes: usize,
        _permissions: u32,
        _backing: NonNull<u8>,
    ) -> Result<(), String> {
        Err("backend does not support physical RAM window replacement".into())
    }
    fn map_zeroed(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String>;
    /// # Safety
    /// Callback data must remain valid until the whole mapping is removed or
    /// the CPU is destroyed. Callbacks must not unwind and must obey the
    /// owner's bounded reentrancy rules. Partial unmaps retain this lifetime.
    unsafe fn map_mmio(&self, address: u64, bytes: usize, callbacks: Mmio) -> Result<(), String>;
    /// Map MMIO with device-published register reads. Other reads and every
    /// write continue through `callbacks`.
    /// # Safety
    /// The callback requirements of `map_mmio` apply. Published reads must
    /// remain valid at every guest-time boundary. Consuming readers must decline
    /// before a read needs owner callbacks, deadlines, DMA or IRQ effects.
    unsafe fn map_mmio_fast(
        &self,
        address: u64,
        bytes: usize,
        callbacks: Mmio,
        _fast_reads: Vec<FastMmioRead>,
    ) -> Result<(), String> {
        unsafe { self.map_mmio(address, bytes, callbacks) }
    }
    fn unmap(&self, address: u64, bytes: usize) -> Result<(), String>;
    fn protect(&self, address: u64, bytes: usize, permissions: u32) -> Result<(), String>;
    fn invalidate_code(&self, range: Option<(u64, u64)>) -> Result<(), String>;
    fn run(&self, request: Run) -> Result<Exit, ExecutionError>;
    /// Start a run without waiting for its next callback or completion.
    fn start_run(&self, _request: Run) -> Result<RunToken, ExecutionError> {
        Err(ExecutionError::Backend(
            "backend does not support asynchronous runs".into(),
        ))
    }
    /// Observe one asynchronous run without dispatching pending callbacks.
    fn poll_run(&self, _run: RunToken) -> Result<RunStatus, ExecutionError> {
        Err(ExecutionError::Backend(
            "backend does not support asynchronous runs".into(),
        ))
    }
    /// Request a pause at the next backend block boundary.
    fn request_pause(&self, _run: RunToken) -> Result<(), ExecutionError> {
        Err(ExecutionError::Backend(
            "backend does not support asynchronous pauses".into(),
        ))
    }
    /// Dispatch exactly one callback parked by [`Cpu::poll_run`].
    fn service_pending_callback(&self, _run: RunToken) -> Result<(), ExecutionError> {
        Err(ExecutionError::Backend(
            "backend does not support owner-serviced callbacks".into(),
        ))
    }
    /// Reject one callback parked by [`Cpu::poll_run`] without invoking its
    /// owner callback. This lets a failed cluster barrier drain worker threads
    /// without permitting additional device or memory effects.
    fn cancel_pending_callback(&self, _run: RunToken) -> Result<(), ExecutionError> {
        Err(ExecutionError::Backend(
            "backend does not support callback cancellation".into(),
        ))
    }
    /// Install the owner's shared wakeup before starting asynchronous runs.
    fn set_run_wakeup(&self, _wakeup: RunWakeup) -> Result<(), String> {
        Err("backend does not support asynchronous run wakeups".into())
    }
    /// Lower the current run's total entered-instruction limit, including
    /// entries already measured in that run. Never extends the original cap.
    fn tighten_instruction_limit(&self, _total_entries: u64) -> Result<(), String> {
        Err("backend cannot tighten an active instruction limit".into())
    }
    /// Instructions entered in the current/most recent run, when measured.
    /// None means this run has no backend count; the runtime may maintain its
    /// own count through execution observations. Reading never changes it.
    fn instruction_progress(&self) -> Option<u64> {
        None
    }
    fn stop(&self) -> Result<(), String>;
    /// Explicit isolated-call interception: omit the current instruction entry.
    /// Valid only inside an instruction observer, before any guest effects.
    fn stop_before_instruction(&self) -> Result<(), String> {
        Err("backend cannot intercept an instruction entry".into())
    }
    /// Declared R-profile CP14 identity input, changed only while paused.
    fn rprofile_identity(&self, _value: Option<u32>) -> Result<u32, String> {
        Err("backend has no R-profile identity input".into())
    }
    fn set_interrupt(&self, _line: Interrupt, _asserted: bool) -> Result<(), String> {
        Err("this backend does not expose architectural interrupt inputs".into())
    }
    /// `previous` permits sharing immutable private-memory pages between
    /// keyframes. It must belong to this CPU; backends without private RAM
    /// may ignore it. Physical board RAM is captured by the execution owner.
    fn snapshot(&self, previous: Option<&dyn Snapshot>) -> Result<Box<dyn Snapshot>, String>;
    fn restore(&self, snapshot: &dyn Snapshot) -> Result<(), String>;
    /// Decode and validate detached checkpoint data without changing live state.
    /// The returned checkpoint belongs to this CPU and is applied by `restore`.
    fn import_snapshot(&self, _bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        Err("processor cannot import durable snapshots".into())
    }
    /// # Safety
    /// Callback data must remain valid until the hook is removed or the CPU
    /// is destroyed; callbacks must not unwind and must obey the owner's
    /// bounded reentrancy rules.
    unsafe fn install_hook(&self, hook: Hook) -> Result<HookId, String>;
    fn remove_hook(&self, hook: HookId) -> Result<(), String>;
    /// Install one observer group without running the processor between hooks.
    /// On failure, remove the observers already installed by this call.
    ///
    /// # Safety
    /// Every callback context must satisfy install_hook's lifetime contract.
    unsafe fn install_hooks(&self, hooks: &[Hook]) -> Result<Vec<HookId>, String> {
        let mut installed = Vec::with_capacity(hooks.len());
        for &hook in hooks {
            match unsafe { self.install_hook(hook) } {
                Ok(id) => installed.push(id),
                Err(error) => {
                    self.remove_hooks(&installed)?;
                    return Err(error);
                }
            }
        }
        Ok(installed)
    }
    /// Retire an observer group before its callback contexts are released.
    fn remove_hooks(&self, hooks: &[HookId]) -> Result<(), String> {
        for &hook in hooks {
            self.remove_hook(hook)?;
        }
        Ok(())
    }
    /// # Safety
    /// The callback context must outlive installation, as for install_hook.
    unsafe fn translation_hooks(&self, hooks: Option<TranslationHooks>) -> Result<(), String>;
    fn retry_block(&self, instructions: u32) -> Result<(), String>;
    fn request_restart(&self) -> Result<(), String>;
    fn accounting(&self) -> Option<NonNull<BlockAccounting>>;
    /// # Safety
    /// The bitmap is writable and covers every backing page; both buffers
    /// outlive observation and are accessed only under the owner's CPU rules.
    unsafe fn observe_backings(&self, ranges: &[BackingObserver]) -> Result<(), String>;
    fn watch_stores(&self, address: u64, bytes: u64) -> Result<(), String>;
    fn hook_watched(&self, hook: HookId, watched: bool) -> Result<(), String>;
    fn precise_store_pc(&self, on: bool) -> Result<(), String>;
    fn exit_counters(&self) -> Result<std::collections::BTreeMap<&'static str, u64>, String>;
}
