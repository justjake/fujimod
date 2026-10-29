//! The device interface every runtime uses.
//!
//! Register reads and writes use a context; deadline events carry tokens,
//! and saved state uses integer guest virtual time, structured records and
//! modeled refusals.

use crate::record::Record;
use std::sync::{Arc, atomic::{AtomicU16, AtomicU32}};

/// One device-owned register read. `offset` is
/// relative to one declared device window. Clocked readers may decline a
/// read when their published state is no longer valid at the supplied tick.
#[derive(Clone)]
pub struct FastReadWord {
    pub offset: u64,
    pub value: FastReadValue,
}

/// Opaque device work that must be completed by a host runtime before guest
/// execution resumes. Device-specific request bytes stay outside checkpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRequest {
    pub id: u64,
    pub kind: String,
    pub payload: Vec<u8>,
}
#[derive(Clone)]
pub enum FastReadValue {
    AtomicU16(Arc<AtomicU16>),
    AtomicU32(Arc<AtomicU32>),
    ClockedU32(Arc<dyn Fn(u64) -> Option<u32> + Send + Sync>),
    /// Consume only prepublished data; decline before IRQ, DMA, deadline or
    /// other framework-visible effects. The device owns the shared cursor and
    /// includes it in checkpoints. Returning None must leave it unchanged.
    ConsumingU32(Arc<dyn Fn() -> Option<u32> + Send + Sync>),
}
impl FastReadValue {
    pub fn width(&self) -> u64 {
        match self {
            Self::AtomicU16(_) => 2,
            _ => 4,
        }
    }
}
impl FastReadWord {
    pub fn atomic_u16(offset: u64, value: Arc<AtomicU16>) -> Self {
        Self {
            offset,
            value: FastReadValue::AtomicU16(value),
        }
    }
    pub fn atomic_u32(offset: u64, value: Arc<AtomicU32>) -> Self {
        Self {
            offset,
            value: FastReadValue::AtomicU32(value),
        }
    }

    pub fn consuming_u32(offset: u64, read: Arc<dyn Fn() -> Option<u32> + Send + Sync>) -> Self {
        Self {
            offset,
            value: FastReadValue::ConsumingU32(read),
        }
    }

    pub fn clocked_u32(
        offset: u64,
        read: Arc<dyn Fn(u64) -> Option<u32> + Send + Sync>,
    ) -> Self {
        Self {
            offset,
            value: FastReadValue::ClockedU32(read),
        }
    }
}

/// A board-wired per-core interrupt controller's physical inputs and output.
/// Delivery is serialized with guest execution. Changing an input wakes its
/// owning device; observing the IRQ output never acknowledges a source.
pub trait CoreInterrupts {
    fn pulse(&self, line: u32) -> Result<(), String>;
    fn level(&self, line: u32, high: bool) -> Result<(), String>;
    fn irq(&self) -> bool;
}

/// A physical level output connected by the checked board graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub enum InterruptLine {
    Shared(u32),
    Core { core: usize, line: u32 },
}
impl InterruptLine {
    pub fn drive(self, cx: &mut Cx<'_>, high: bool) -> Result<(), String> {
        match self {
            Self::Shared(line) => {
                cx.set_irq(line, high);
                Ok(())
            }
            Self::Core { core, line } => cx.set_core_irq(core, line, high),
        }
    }
}

/// Services a runtime gives its devices.
pub trait Host {
    /// Guest virtual time in the board's unit (X70: virtual timer ticks, so the
    /// exact runtime and the HVF port agree).
    fn now(&self) -> u64;
    /// Stop the current guest run after its active instruction retires.
    /// Runtimes without an instruction boundary refuse asynchronous work.
    fn request_pause(&mut self) -> Result<(), String> {
        Err("host cannot pause guest execution for asynchronous work".into())
    }
    /// Worker count for pure host computation. Zero selects the exact serial
    /// path; workers never receive guest state or publish device effects.
    fn compute_workers(&self) -> usize {
        0
    }
    /// Call `device.event(cx, token)` at board time `at`.
    fn schedule(&mut self, device: usize, at: u64, token: u64);
    /// Assert or release a level interrupt line (GIC-style boards).
    fn set_irq(&mut self, line: u32, level: bool);
    /// Latch interrupt `line` for core `cpu` (per-core controllers, X70).
    fn raise_irq(&mut self, cpu: usize, line: u32) -> Result<(), String>;
    /// Drive a level input on a particular processor's interrupt controller.
    fn set_core_irq(&mut self, _cpu: usize, _line: u32, _level: bool) -> Result<(), String> {
        Err("per-core level interrupt input is not connected".into())
    }
    /// Drive a processor's hardware reset input through the engine.
    fn reset_core(&mut self, _cpu: usize, _asserted: bool) -> Result<(), String> {
        Err("processor reset input is not connected".into())
    }
    /// Select an explicitly wired memory decode. The runtime commits an MMIO
    /// request after the accessing instruction, with all processors stopped.
    fn select_memory(&mut self, _decoder: usize, _choice: usize) -> Result<(), String> {
        Err("memory decoder output is not connected".into())
    }
    /// Guest RAM by physical address (DMA).
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String>;
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String>;
    /// Deliver captured pixels to host output transport. This cannot change
    /// guest time, device state or the model's image sequence.
    fn scanout(&mut self, _frame: crate::graphics::scanout::Frame) -> Result<(), String> {
        Err("scanout output is not connected".into())
    }
    /// Observational output status for host tooling.
    fn output_status(&mut self, _output: usize, _state: serde_json::Value) -> Result<(), String> {
        Ok(())
    }
    /// Append to the board's event log.
    fn record(&mut self, record: Record);
    /// Diagnostic output is observational; it must not advance guest time.
    fn log(&mut self, _line: &str) {}
    fn console(&mut self, _line: &str) {}
}

/// One device call's context: who accessed it, the host, and the outcome.
pub struct Cx<'a> {
    /// The accessing core; `usize::MAX` identifies a host event or input.
    pub cpu: usize,
    /// The guest PC of the access, when known.
    pub pc: u64,
    /// This device's index in the runtime's device table.
    pub device: usize,
    host: &'a mut dyn Host,
    refusal: Option<String>,
    unmodeled: bool,
    tick_override: Option<u64>,
}

impl<'a> Cx<'a> {
    pub fn new(host: &'a mut dyn Host, cpu: usize, pc: u64, device: usize) -> Self {
        Cx {
            cpu,
            pc,
            device,
            host,
            refusal: None,
            unmodeled: false,
            tick_override: None,
        }
    }
    pub fn now(&self) -> u64 {
        self.tick_override.unwrap_or_else(|| self.host.now())
    }
    pub fn request_pause(&mut self) -> Result<(), String> {
        self.host.request_pause()
    }
    /// Bounded strategy for pure, deterministic raster work. The owner still
    /// applies every resulting device effect on this context's thread.
    pub fn compute_workers(&self) -> usize {
        self.host.compute_workers()
    }
    /// Service a composite device's already-due effect at its declared tick.
    /// This changes only the child clock view; it never advances the runtime.
    pub fn at_tick(&mut self, tick: u64, effect: impl FnOnce(&mut Self)) {
        if tick > self.host.now() {
            self.fail("a device cannot service a future event early");
            return;
        }
        let previous = self.tick_override.replace(tick);
        effect(self);
        self.tick_override = previous;
    }
    pub fn schedule(&mut self, at: u64, token: u64) {
        self.host.schedule(self.device, at, token);
    }
    pub fn set_irq(&mut self, line: u32, level: bool) {
        self.host.set_irq(line, level);
    }
    pub fn raise_irq(&mut self, cpu: usize, line: u32) -> Result<(), String> {
        self.host.raise_irq(cpu, line)
    }
    pub fn set_core_irq(&mut self, cpu: usize, line: u32, level: bool) -> Result<(), String> {
        self.host.set_core_irq(cpu, line, level)
    }
    pub fn reset_core(&mut self, cpu: usize, asserted: bool) -> Result<(), String> {
        self.host.reset_core(cpu, asserted)
    }
    pub fn select_memory(&mut self, decoder: usize, choice: usize) -> Result<(), String> {
        self.host.select_memory(decoder, choice)
    }
    pub fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        self.host.ram_read(address, out)
    }
    /// DMA data whose consumer needs an owned buffer.
    pub fn ram_bytes(&self, address: u64, size: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0; size];
        self.host.ram_read(address, &mut bytes)?;
        Ok(bytes)
    }
    pub fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        self.host.ram_write(address, bytes)
    }
    pub fn scanout(&mut self, frame: crate::graphics::scanout::Frame) -> Result<(), String> {
        self.host.scanout(frame)
    }
    pub fn output_status(&mut self, output: usize, state: serde_json::Value) -> Result<(), String> {
        self.host.output_status(output, state)
    }
    pub fn record(&mut self, record: Record) {
        self.host.record(record);
    }
    pub fn log(&mut self, line: &str) {
        self.host.log(line);
    }
    pub fn console(&mut self, line: &str) {
        self.host.console(line);
    }
    /// An unsupported register is distinct from a failed modeled operation.
    /// The framework may continue it only in explicitly selected discovery.
    pub fn unmodeled(&mut self) {
        self.unsupported("unmodeled register");
    }
    /// A typed unsupported access, with evidence retained by discovery.
    /// Operational failures still use `fail` and stop in every mode.
    pub fn unsupported(&mut self, reason: impl Into<String>) {
        if self.refusal.is_none() {
            self.unmodeled = true;
            self.fail(reason);
        }
    }
    pub fn is_unmodeled(&self) -> bool {
        self.unmodeled
    }
    /// Refuse this access: the runtime stops with `reason`.
    /// The first refusal wins.
    pub fn fail(&mut self, reason: impl Into<String>) {
        if self.refusal.is_none() {
            self.refusal = Some(reason.into());
        }
    }
    pub fn refusal(&self) -> Option<&str> {
        self.refusal.as_deref()
    }
    pub fn take_refusal(&mut self) -> Option<String> {
        self.refusal.take()
    }
}

/// A register-level device behind one or more address windows.
pub trait Device: Send + std::any::Any {
    fn name(&self) -> &'static str;
    // Capabilities the model offers sessions (`crate::capability`).
    fn analog_inputs(&self) -> Option<&(dyn crate::capability::AnalogInputs + 'static)> {
        None
    }
    fn analog_inputs_mut(&mut self) -> Option<&mut (dyn crate::capability::AnalogInputs + 'static)> {
        None
    }
    fn serial_status(&self) -> Option<&(dyn crate::capability::SerialStatus + 'static)> {
        None
    }
    fn serial_status_mut(&mut self) -> Option<&mut (dyn crate::capability::SerialStatus + 'static)> {
        None
    }
    fn touch_panel(&self) -> Option<&(dyn crate::capability::TouchPanel + 'static)> {
        None
    }
    fn touch_panel_mut(&mut self) -> Option<&mut (dyn crate::capability::TouchPanel + 'static)> {
        None
    }
    fn hdmi_output(&self) -> Option<&(dyn crate::capability::HdmiOutput + 'static)> {
        None
    }
    fn hdmi_output_mut(&mut self) -> Option<&mut (dyn crate::capability::HdmiOutput + 'static)> {
        None
    }
    fn usb_port(&self) -> Option<&(dyn crate::capability::UsbPort + 'static)> {
        None
    }
    fn usb_port_mut(&mut self) -> Option<&mut (dyn crate::capability::UsbPort + 'static)> {
        None
    }
    fn nand_store(&self) -> Option<&(dyn crate::capability::NandStore + 'static)> {
        None
    }
    fn nand_store_mut(&mut self) -> Option<&mut (dyn crate::capability::NandStore + 'static)> {
        None
    }
    fn card_slot(&self) -> Option<&(dyn crate::capability::CardSlot + 'static)> {
        None
    }
    fn card_slot_mut(&mut self) -> Option<&mut (dyn crate::capability::CardSlot + 'static)> {
        None
    }
    fn scanout_source(&self) -> Option<&(dyn crate::capability::ScanoutSource + 'static)> {
        None
    }
    fn scanout_source_mut(&mut self) -> Option<&mut (dyn crate::capability::ScanoutSource + 'static)> {
        None
    }
    fn frame_source(&self) -> Option<&(dyn crate::capability::FrameSource + 'static)> {
        None
    }
    fn frame_source_mut(&mut self) -> Option<&mut (dyn crate::capability::FrameSource + 'static)> {
        None
    }
    /// Optional device-published reads for one register window. This
    /// query is observational: it must not settle time or change modeled
    /// state. Interactive runtimes may bypass `read` only while the declaration
    /// remains valid; ordinary exact execution keeps using the read callback.
    /// Atomic values may also serve as exact loop-proof inputs when they stay
    /// stable through the proof window. Clocked and consuming values never do.
    /// Consuming readers may advance only their device-owned FIFO cursor;
    /// completion and other framework effects must use the ordinary callback.
    fn fast_reads(&self, _window_base: u64, _window_size: u64) -> Vec<FastReadWord> {
        Vec::new()
    }
    /// Full typed mutable state, including pending operations and owned input
    /// parts. Unsupported models refuse; diagnostic `save()` is not a fallback.
    fn checkpoint(&self) -> Result<crate::checkpoint::Checkpoint, String> {
        Err(format!(
            "{} has no exact checkpoint implementation",
            self.name()
        ))
    }
    fn validate_checkpoint(&self, _state: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        Err(format!(
            "{} has no exact checkpoint implementation",
            self.name()
        ))
    }
    /// Called with all processors stopped after the complete graph passes
    /// validation. Restore existing shared wiring in place; never replay MMIO.
    fn restore_checkpoint(&mut self, _state: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        Err(format!(
            "{} has no exact checkpoint implementation",
            self.name()
        ))
    }
    /// Complete future-affecting state for comparison, including pending
    /// payloads and owned external inputs. Diagnostic summaries are separate.
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Err(format!("{} has no complete state fingerprint", self.name()))
    }
    /// A read of `size` bytes at `address` (absolute, so one model can own
    /// several windows).
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64;
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64);
    /// Optional side-effect-free admission check used only by explicit
    /// discovery before choosing a transport continuation. Strict execution
    /// always calls the ordinary register law.
    fn access_contract(&self, _address: u64, _size: u8, _value: Option<u64>) -> Option<String> {
        None
    }
    /// Register-level continuation of an unsupported access. Only the runtime
    /// in explicit discovery may invoke this; an applied result remains
    /// recorded as unsupported and never counts as a modeled operation.
    fn discovery_access(
        &mut self,
        _cx: &mut Cx<'_>,
        _address: u64,
        _size: u8,
        _value: Option<u64>,
    ) -> Option<u64> {
        None
    }
    /// True only for side-effect-free reads. Used for host polling backoff;
    /// checking this predicate must itself be observational.
    fn pollable(&self, _address: u64, _size: u8) -> bool {
        false
    }
    /// Observational checkpoint state; restoring does not perform guest work.
    fn save(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    /// Stopped-board diagnostics, separate from complete checkpoint payloads.
    fn diagnostic_state(&self) -> serde_json::Value {
        self.save()
    }
    /// Read an owned buffer without encoding binary data as JSON integers.
    /// This observation must not advance time or change device behavior.
    fn diagnostic_buffer(&self, name: &str) -> Result<&[u8], String> {
        Err(format!("{} has no diagnostic buffer {name}", self.name()))
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        if state.is_null() {
            Ok(())
        } else {
            Err(format!("{} has no checkpoint state", self.name()))
        }
    }
    /// Explicit scene input, decoded by the framework, independent of firmware.
    /// Returns whether this model consumes the input.
    fn set_scene(&mut self, _scene: &serde_json::Value) -> Result<bool, String> {
        Ok(false)
    }
    /// Drain host artifact work when a run ends; no guest effects are allowed.
    fn finish(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// A deadline set with `cx.schedule(at, token)` arrived.
    fn event(&mut self, _cx: &mut Cx<'_>, _token: u64) {}
    /// Take one opaque host request after the board has stopped its processors.
    fn take_host_request(&mut self) -> Option<HostRequest> {
        None
    }
    /// Whether this device is waiting for a host result before the guest can
    /// execute again.
    fn has_host_request(&self) -> bool {
        false
    }
    /// Apply a host result while the board is stopped. Implementations should
    /// validate the request ID and make the modeled completion visible only
    /// after the result has been accepted.
    fn complete_host_request(
        &mut self,
        _cx: &mut Cx<'_>,
        _id: u64,
        _result: &[u8],
    ) -> Result<(), String> {
        Err(format!("{} has no pending host request", self.name()))
    }
    /// Bring time bookkeeping up to `cx.now()` on guest-driven access or a
    /// due event. A quantum boundary alone must not advance a device.
    fn advance(&mut self, _cx: &mut Cx<'_>) {}
    /// The next board time at which this device has work, if any.
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        None
    }
}
