//! One EXR II per-core interrupt controller. Arbitration by ascending group
//! and descending bit, and strict priority nesting, remain explicit model
//! options. FIQ, CPU state and firmware RAM are not part of this device.

use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Arbitration {
    Unique,
    ActiveBitmap,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub arbitration: Arbitration,
}

#[derive(Default)]
struct Pins {
    edges: [AtomicU32; 4],
    levels: [AtomicU32; 4],
    changed: AtomicBool,
    irq: AtomicBool,
    enabled: [AtomicU32; 4],
    pending: [AtomicU32; 4],
    configuration: Mutex<BTreeMap<u32, u32>>,
    active: Mutex<Vec<(u8, u8)>>,
}

/// Physical inputs and one observational IRQ output. Atomics satisfy the
/// shared Device's Send contract without a lock on register accesses.
#[derive(Clone, Default)]
pub struct Inputs(Arc<Pins>);
/// Register-derived source outputs consumed by wired controllers.
#[derive(Clone, Copy, Debug)]
pub struct SourceRegisters {
    pub enabled: bool,
    pub configuration60: Option<bool>,
    pub source_config: bool,
    pub mode140: Option<u8>,
    pub mode180: Option<u8>,
    pub vector: Option<u32>,
    pub priority: Option<u32>,
}
impl Inputs {
    pub fn source(&self, line: u32) -> Result<SourceRegisters, String> {
        Self::bit(line)?;
        let config = self.0.configuration.lock().unwrap();
        let group = line / 32;
        let shift = (line % 16) * 2;
        Ok(SourceRegisters {
            enabled: self.enabled() >> line & 1 != 0,
            configuration60: config
                .get(&(0x60 + group * 4))
                .map(|v| v >> (line % 32) & 1 != 0),
            source_config: config
                .get(&(0x100 + group * 4))
                .is_some_and(|v| v >> (line % 32) & 1 != 0),
            mode140: config
                .get(&(0x140 + line / 16 * 4))
                .map(|v| ((v >> shift) & 3) as u8),
            mode180: config
                .get(&(0x180 + line / 16 * 4))
                .map(|v| ((v >> shift) & 3) as u8),
            vector: config.get(&(0x400 + line * 4)).copied(),
            priority: config.get(&(0x800 + line * 4)).copied(),
        })
    }
    fn bit(line: u32) -> Result<(usize, u32), String> {
        if line >= 128 {
            return Err(format!("interrupt line {line} exceeds 128 sources"));
        }
        Ok(((line / 32) as usize, 1 << (line % 32)))
    }
    pub fn pulse(&self, line: u32) -> Result<(), String> {
        let (group, bit) = Self::bit(line)?;
        self.0.edges[group].fetch_or(bit, Ordering::Relaxed);
        self.0.changed.store(true, Ordering::Relaxed);
        Ok(())
    }
    pub fn level(&self, line: u32, high: bool) -> Result<(), String> {
        let (group, bit) = Self::bit(line)?;
        let previous = if high {
            self.0.levels[group].fetch_or(bit, Ordering::Relaxed)
        } else {
            self.0.levels[group].fetch_and(!bit, Ordering::Relaxed)
        };
        if (previous & bit != 0) != high {
            self.0.changed.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
    pub fn irq(&self) -> bool {
        self.0.irq.load(Ordering::Relaxed)
    }
    /// Register-derived hardware mask output, without consuming an interrupt.
    pub fn enabled(&self) -> u128 {
        self.0
            .enabled
            .iter()
            .enumerate()
            .fold(0, |mask, (group, word)| {
                mask | ((word.load(Ordering::Relaxed) as u128) << (group * 32))
            })
    }
    /// Read-only register outputs for transitional peer adapters. This does
    /// not consume input edges, acknowledge a source or advance the device.
    pub fn configuration(&self) -> serde_json::Value {
        let config = self.0.configuration.lock().unwrap();
        let active = self.0.active.lock().unwrap();
        let source_config = (0..4).fold(0u128, |mask, i| {
            mask | (u128::from(*config.get(&(0x100 + i * 4)).unwrap_or(&0)) << (i * 32))
        });
        let pending = self
            .0
            .pending
            .iter()
            .enumerate()
            .fold(0u128, |mask, (i, v)| {
                mask | (u128::from(v.load(Ordering::Relaxed)) << (i * 32))
            });
        // Decimal strings preserve all 128 source bits across JSON clients.
        serde_json::json!({"config":*config,"enabled":self.enabled().to_string(),"pending":pending.to_string(),"source_config":source_config.to_string(),"active":active.last().map(|p|p.0),"active_stack":active.iter().map(|&(line,priority)|serde_json::json!({"line":line,"priority":priority})).collect::<Vec<_>>()})
    }
}

pub struct Wiring {
    pub base: u64,
    pub inputs: Inputs,
    pub enables: Option<Box<dyn EnableOutput>>,
    pub software: Vec<SoftwareOutput>,
}

/// Latched software-command pulse for a wired one-shot consumer. External
/// interrupt inputs, pending reads and acknowledgments never produce it.
#[derive(Clone, Default)]
pub struct SoftwarePulse(Arc<AtomicBool>);
impl SoftwarePulse {
    pub fn restore_level(&self, asserted: bool) {
        self.0.store(asserted, Ordering::Relaxed);
    }
    pub fn asserted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
pub struct SoftwareOutput {
    pub line: u32,
    pub pulse: SoftwarePulse,
}

/// Register-derived source masks supplied to connected hardware consumers.
pub trait EnableOutput: Send {
    fn publish(&self, enabled: [u32; 4]);
}

impl crate::bus::CoreInterrupts for Inputs {
    fn pulse(&self, line: u32) -> Result<(), String> {
        Inputs::pulse(self, line)
    }
    fn level(&self, line: u32, high: bool) -> Result<(), String> {
        Inputs::level(self, line, high)
    }
    fn irq(&self) -> bool {
        Inputs::irq(self)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    line: u8,
    priority: u8,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    enabled: [u32; 4],
    pending: [u32; 4],
    source_config: [u32; 4],
    configuration60: [u32; 4],
    configuration140: [u32; 8],
    configuration180: [u32; 8],
    vectors: Vec<Option<u32>>,
    priorities: Vec<Option<u8>>,
    active: Vec<Source>,
    reset: [u32; 2],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    arbitration: Arbitration,
    registers: State,
    edges: [u32; 4],
    levels: [u32; 4],
    changed: bool,
    software: Vec<bool>,
}

pub struct InterruptController {
    base: u64,
    arbitration: Arbitration,
    inputs: Inputs,
    enables: Option<Box<dyn EnableOutput>>,
    software: Vec<SoftwareOutput>,
    state: State,
}

enum RegisterError {
    Unmodeled,
    Refusal(String),
}
impl From<String> for RegisterError {
    fn from(value: String) -> Self {
        Self::Refusal(value)
    }
}
impl From<&str> for RegisterError {
    fn from(value: &str) -> Self {
        Self::Refusal(value.into())
    }
}

impl InterruptController {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if !wiring.base.is_multiple_of(0x1000) || wiring.base.checked_add(0x1000).is_none() {
            return Err("interrupt controller requires an aligned 4 KiB window".into());
        }
        if wiring.software.iter().any(|s| s.line >= 128) {
            return Err("software interrupt output exceeds 128 sources".into());
        }
        let controller = Self {
            base: wiring.base,
            arbitration: options.arbitration,
            inputs: wiring.inputs,
            enables: wiring.enables,
            software: wiring.software,
            state: State {
                enabled: [0; 4],
                pending: [0; 4],
                source_config: [0; 4],
                configuration60: [0; 4],
                configuration140: [0; 8],
                configuration180: [0; 8],
                vectors: vec![None; 128],
                priorities: vec![None; 128],
                active: Vec::with_capacity(16),
                reset: [0; 2],
            },
        };
        controller.rebuild_configuration();
        Ok(controller)
    }
    fn rebuild_configuration(&self) {
        let mut output = self.inputs.0.configuration.lock().unwrap();
        output.clear();
        for (base, values) in [
            (0x60, self.state.configuration60.as_slice()),
            (0x100, self.state.source_config.as_slice()),
            (0x140, self.state.configuration140.as_slice()),
            (0x180, self.state.configuration180.as_slice()),
        ] {
            output.extend(
                values
                    .iter()
                    .enumerate()
                    .map(|(i, &word)| (base + i as u32 * 4, word)),
            );
        }
        output.extend(
            self.state
                .vectors
                .iter()
                .enumerate()
                .filter_map(|(i, &v)| v.map(|v| (0x400 + i as u32 * 4, v))),
        );
        output.extend(
            self.state
                .priorities
                .iter()
                .enumerate()
                .filter_map(|(i, &v)| v.map(|v| (0x800 + i as u32 * 4, v as u32))),
        );
    }
    fn publish_state_outputs(&self) {
        for (word, value) in self.inputs.0.enabled.iter().zip(self.state.enabled) {
            word.store(value, Ordering::Relaxed);
        }
        for (word, value) in self.inputs.0.pending.iter().zip(self.state.pending) {
            word.store(value, Ordering::Relaxed);
        }
        let mut active = self.inputs.0.active.lock().unwrap();
        active.clear();
        active.extend(self.state.active.iter().map(|s| (s.line, s.priority)));
    }
    fn offset(&self, address: u64, size: u8) -> Option<u32> {
        let offset = address.checked_sub(self.base)?;
        (size == 4 && offset < 0x1000 && offset.is_multiple_of(4)).then_some(offset as u32)
    }
    fn selection(state: &State, arbitration: Arbitration) -> Result<Option<Source>, String> {
        let mut selected: Option<Source> = None;
        let mut ties = false;
        let ceiling = state.active.last().map(|s| s.priority);
        for group in 0..4 {
            let mut candidates = state.pending[group] & state.enabled[group];
            for active in &state.active {
                if active.line as usize / 32 == group {
                    candidates &= !(1 << (active.line % 32));
                }
            }
            while candidates != 0 {
                // Equal-priority selection keeps the first bit in this order.
                let bit = 31 - candidates.leading_zeros();
                candidates &= !(1 << bit);
                let line = group * 32 + bit as usize;
                let priority = state.priorities[line]
                    .ok_or_else(|| format!("IRQ {line:#x} lacks a programmed priority"))?;
                if let Some(ceiling) = ceiling {
                    if arbitration == Arbitration::Unique {
                        return Err("nested IRQ delivery requires declared arbitration".into());
                    }
                    if priority >= ceiling {
                        continue;
                    }
                }
                match selected {
                    None => {
                        selected = Some(Source {
                            line: line as u8,
                            priority,
                        })
                    }
                    Some(previous) if priority < previous.priority => {
                        selected = Some(Source {
                            line: line as u8,
                            priority,
                        });
                        ties = false;
                    }
                    Some(previous) if priority == previous.priority => ties = true,
                    _ => (),
                }
            }
        }
        if ties && arbitration == Arbitration::Unique {
            return Err("equal-priority IRQ arbitration is not declared".into());
        }
        Ok(selected)
    }
    fn publish(&self, cx: &mut Cx<'_>) {
        self.publish_state_outputs();
        if let Some(output) = &self.enables {
            output.publish(self.state.enabled);
        }
        match Self::selection(&self.state, self.arbitration) {
            Ok(source) => self.inputs.0.irq.store(source.is_some(), Ordering::Relaxed),
            Err(error) => {
                self.inputs.0.irq.store(false, Ordering::Relaxed);
                cx.fail(error);
            }
        }
    }
    fn read_register(&mut self, offset: u32) -> Result<u32, RegisterError> {
        Ok(match offset {
            0x60..=0x6c => self.state.configuration60[((offset - 0x60) / 4) as usize],
            0x80..=0x8c => self.state.enabled[((offset - 0x80) / 4) as usize],
            0x100..=0x10c => self.state.source_config[((offset - 0x100) / 4) as usize],
            0x140..=0x15c => self.state.configuration140[((offset - 0x140) / 4) as usize],
            0x180..=0x19c => self.state.configuration180[((offset - 0x180) / 4) as usize],
            0x1c0 => self
                .state
                .active
                .last()
                .map_or(0xffff, |s| u32::from(s.line)),
            0x1c4 => 0xffff,
            0x200 => {
                if let Some(source) = Self::selection(&self.state, self.arbitration)? {
                    let vector = self.state.vectors[source.line as usize].ok_or_else(|| {
                        format!("IRQ {:#x} lacks a programmed vector", source.line)
                    })?;
                    if self.state.active.len() == 16 {
                        return Err("IRQ nesting exceeds 16 priority levels".into());
                    }
                    self.state.active.push(source);
                    vector
                } else if let Some(source) = self.state.active.last() {
                    self.state.vectors[source.line as usize]
                        .ok_or("active IRQ lacks a programmed vector")?
                } else {
                    0
                }
            }
            0x230..=0x23c => self.state.active.last().map_or(0, |s| {
                if u32::from(s.line) / 32 == (offset - 0x230) / 4 {
                    1 << (s.line % 32)
                } else {
                    0
                }
            }),
            0x400..=0x5fc => self.state.vectors[((offset - 0x400) / 4) as usize].unwrap_or(0),
            0x800..=0x9fc => {
                self.state.priorities[((offset - 0x800) / 4) as usize].map_or(0, u32::from)
            }
            _ => return Err(RegisterError::Unmodeled),
        })
    }
    fn write_register(&mut self, offset: u32, value: u32) -> Result<(), RegisterError> {
        match offset {
            0x60..=0x6c => self.state.configuration60[((offset - 0x60) / 4) as usize] = value,
            0x80..=0x8c => self.state.enabled[((offset - 0x80) / 4) as usize] |= value,
            0xa0..=0xac => self.state.enabled[((offset - 0xa0) / 4) as usize] &= !value,
            0xc0..=0xcc => {
                let group = (offset - 0xc0) / 4;
                self.state.pending[group as usize] |= value;
                for output in &self.software {
                    if output.line / 32 == group && value & (1 << (output.line % 32)) != 0 {
                        output.pulse.0.store(true, Ordering::Relaxed);
                    }
                }
            }
            0xe0..=0xec => self.state.pending[((offset - 0xe0) / 4) as usize] &= !value,
            0x100..=0x10c => self.state.source_config[((offset - 0x100) / 4) as usize] = value,
            0x120..=0x12c => self.state.pending[((offset - 0x120) / 4) as usize] &= !value,
            0x140..=0x15c => self.state.configuration140[((offset - 0x140) / 4) as usize] = value,
            0x180..=0x19c => self.state.configuration180[((offset - 0x180) / 4) as usize] = value,
            0x1c0 | 0x1c4 => {
                if value != 0xffff {
                    return Err("unsupported current-source initialization".into());
                }
            }
            0x1c8 => {
                if value != 1 {
                    return Err("unsupported interrupt-controller reset value".into());
                }
                self.state.reset[0] = value;
            }
            0xc00 => {
                if value != 0 {
                    return Err("unsupported interrupt-controller reset value".into());
                }
                self.state.reset[1] = value;
            }
            0x200 => {
                if value != 0 {
                    return Err("unsupported end-of-interrupt value".into());
                }
                self.state.active.pop();
                for group in 0..4 {
                    self.state.pending[group] |=
                        self.inputs.0.levels[group].load(Ordering::Relaxed);
                }
            }
            0x400..=0x5fc => self.state.vectors[((offset - 0x400) / 4) as usize] = Some(value),
            0x800..=0x9fc => {
                if value > 15 {
                    return Err("unsupported interrupt priority".into());
                }
                let line = ((offset - 0x800) / 4) as usize;
                // Acknowledge latches the active priority in Source. The
                // programmable priority selects future acknowledgments;
                // rewriting it does not reorder the in-service stack.
                self.state.priorities[line] = Some(value as u8);
            }
            _ => return Err(RegisterError::Unmodeled),
        }
        if matches!(offset, 0x60..=0x6c | 0x100..=0x10c | 0x140..=0x15c | 0x180..=0x19c | 0x400..=0x5fc | 0x800..=0x9fc)
        {
            self.inputs
                .0
                .configuration
                .lock()
                .unwrap()
                .insert(offset, value);
        }
        Ok(())
    }
}

impl InterruptController {
    fn capture_state(&self) -> Saved {
        Saved {
            arbitration: self.arbitration,
            registers: self.state.clone(),
            edges: std::array::from_fn(|i| self.inputs.0.edges[i].load(Ordering::Relaxed)),
            levels: std::array::from_fn(|i| self.inputs.0.levels[i].load(Ordering::Relaxed)),
            changed: self.inputs.0.changed.load(Ordering::Relaxed),
            software: self.software.iter().map(|s| s.pulse.asserted()).collect(),
        }
    }
    fn validate_state(&self, saved: &Saved) -> Result<(), String> {
        let s = &saved.registers;
        if saved.arbitration != self.arbitration
            || saved.software.len() != self.software.len()
            || s.vectors.len() != 128
            || s.priorities.len() != 128
            || s.priorities.iter().flatten().any(|&p| p > 15)
            || s.active.len() > 16
            || s.active.iter().enumerate().any(|(i, a)| {
                a.line >= 128
                    || a.priority > 15
                    || s.priorities[a.line as usize].is_none()
                    || s.active[..i].iter().any(|b| b.line == a.line)
                    || i > 0 && s.active[i - 1].priority <= a.priority
            })
        {
            return Err("invalid interrupt-controller state".into());
        }
        Self::selection(&saved.registers, self.arbitration)?;
        Ok(())
    }
    fn restore_state(&mut self, mut saved: Saved) -> Result<(), String> {
        self.validate_state(&saved)?;

        let irq = Self::selection(&saved.registers, self.arbitration)?.is_some();
        saved
            .registers
            .active
            .reserve(16 - saved.registers.active.len());
        self.state = saved.registers;
        self.rebuild_configuration();
        self.publish_state_outputs();
        for (output, asserted) in self.software.iter().zip(saved.software) {
            output.pulse.0.store(asserted, Ordering::Relaxed);
        }
        for group in 0..4 {
            self.inputs.0.edges[group].store(saved.edges[group], Ordering::Relaxed);
            self.inputs.0.levels[group].store(saved.levels[group], Ordering::Relaxed);
        }
        self.inputs
            .0
            .changed
            .store(saved.changed, Ordering::Relaxed);
        self.inputs.0.irq.store(irq, Ordering::Relaxed);
        if let Some(output) = &self.enables {
            output.publish(self.state.enabled);
        }
        Ok(())
    }
}
impl Device for InterruptController {
    fn checkpoint(&self) -> Result<crate::checkpoint::Checkpoint, String> {
        crate::checkpoint::Checkpoint::encode(self.name(), &self.capture_state())
    }
    fn validate_checkpoint(&self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_state(&saved.decode(self.name())?)
    }
    fn restore_checkpoint(&mut self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        self.restore_state(saved.decode(self.name())?)
    }

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-interrupt-controller"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        if cx.refusal().is_some() {
            return 0;
        }
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        match self.read_register(offset) {
            Ok(value) => {
                if offset == 0x200 {
                    self.publish(cx);
                }
                u64::from(value)
            }
            Err(RegisterError::Unmodeled) => {
                cx.unmodeled();
                0
            }
            Err(RegisterError::Refusal(error)) => {
                cx.fail(error);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        match self.write_register(offset, value as u32) {
            Ok(()) => self.publish(cx),
            Err(RegisterError::Unmodeled) => cx.unmodeled(),
            Err(RegisterError::Refusal(error)) => cx.fail(error),
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.inputs.0.changed.load(Ordering::Relaxed).then_some(now)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.inputs.0.changed.swap(false, Ordering::Relaxed) {
            for group in 0..4 {
                self.state.pending[group] |= self.inputs.0.edges[group].swap(0, Ordering::Relaxed)
                    | self.inputs.0.levels[group].load(Ordering::Relaxed);
            }
            self.publish(cx);
        }
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(self.capture_state()).expect("typed interrupt state is serializable")
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let saved: Saved = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.restore_state(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Host, Record};
    struct Services;
    impl Host for Services {
        fn now(&self) -> u64 {
            7
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("unexpected schedule")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("unexpected IRQ")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            panic!("unexpected IRQ")
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("unexpected DMA")
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            panic!("unexpected DMA")
        }
        fn record(&mut self, _: Record) {
            panic!("unexpected record")
        }
    }
    fn model(arbitration: Arbitration) -> (InterruptController, Inputs) {
        let inputs = Inputs::default();
        (
            InterruptController::new(
                Options { arbitration },
                Wiring {
                    base: 0x1000,
                    inputs: inputs.clone(),
                    enables: None,
                    software: vec![],
                },
            )
            .unwrap(),
            inputs,
        )
    }
    fn access(d: &mut InterruptController, offset: u64, value: Option<u32>) -> Result<u32, String> {
        let mut host = Services;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let result = if let Some(value) = value {
            d.write(&mut cx, 0x1000 + offset, 4, value.into());
            0
        } else {
            d.read(&mut cx, 0x1000 + offset, 4) as u32
        };
        if let Some(error) = cx.take_refusal() {
            Err(error)
        } else {
            Ok(result)
        }
    }
    fn source(d: &mut InterruptController, line: u32, priority: u32) {
        access(d, 0x400 + u64::from(line) * 4, Some(0x10000 + line * 4)).unwrap();
        access(d, 0x800 + u64::from(line) * 4, Some(priority)).unwrap();
        access(d, 0x80 + u64::from(line / 32) * 4, Some(1 << (line % 32))).unwrap();
    }
    #[test]
    fn peer_register_outputs_are_observational_and_restore_with_controller_state() {
        let (mut controller, pins) = model(Arbitration::ActiveBitmap);
        source(&mut controller, 120, 3);
        access(&mut controller, 0x8c, Some(1 << 24)).unwrap();
        access(&mut controller, 0x10c, Some(1 << 24)).unwrap();
        let before = controller.save();
        let output = pins.configuration();
        assert_eq!(output["enabled"], (1u128 << 120).to_string());
        assert_eq!(output["source_config"], (1u128 << 120).to_string());
        pins.pulse(120).unwrap();
        assert_eq!(pins.configuration(), output);
        assert_eq!(controller.save()["registers"], before["registers"]);
        let saved = controller.save();
        access(&mut controller, 0x80, Some(1)).unwrap();
        controller.restore(&saved).unwrap();
        assert_eq!(pins.configuration(), output);
    }
    fn advance(d: &mut InterruptController) -> Result<(), String> {
        let mut host = Services;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        d.advance(&mut cx);
        cx.take_refusal().map_or(Ok(()), Err)
    }
    #[test]
    fn physical_edge_has_an_immediate_deadline_and_inspection_does_not_consume_it() {
        let (mut d, pins) = model(Arbitration::Unique);
        source(&mut d, 65, 3);
        pins.pulse(65).unwrap();
        let saved = d.save();
        assert_eq!(d.next_deadline(7), Some(7));
        assert!(!pins.irq());
        assert_eq!(saved, d.save());
        advance(&mut d).unwrap();
        assert!(pins.irq());
        assert_eq!(d.next_deadline(7), None);
        assert_eq!(access(&mut d, 0x200, None).unwrap(), 0x10104);
        assert!(!pins.irq());
        assert_eq!(access(&mut d, 0x1c0, None).unwrap(), 65);
        assert_eq!(access(&mut d, 0x238, None).unwrap(), 2);
        access(&mut d, 0x128, Some(2)).unwrap();
        access(&mut d, 0x200, Some(0)).unwrap();
        assert_eq!(access(&mut d, 0x1c0, None).unwrap(), 0xffff);
        assert!(!pins.irq());
        d.restore(&saved).unwrap();
        assert_eq!(saved, d.save());
        assert!(!pins.irq());
        advance(&mut d).unwrap();
        assert!(pins.irq());
    }
    #[test]
    fn bitmap_arbitration_nests_only_strictly_higher_priorities_and_restores_the_parent() {
        let (mut d, pins) = model(Arbitration::ActiveBitmap);
        for (line, priority) in [(3, 7), (5, 7), (1, 2)] {
            source(&mut d, line, priority);
        }
        access(&mut d, 0xc0, Some((1 << 3) | (1 << 5))).unwrap();
        assert_eq!(access(&mut d, 0x200, None).unwrap(), 0x10014);
        assert!(!pins.irq());
        pins.pulse(1).unwrap();
        advance(&mut d).unwrap();
        assert_eq!(access(&mut d, 0x200, None).unwrap(), 0x10004);
        assert_eq!(access(&mut d, 0x1c0, None).unwrap(), 1);
        access(&mut d, 0x814, Some(1)).unwrap();
        let saved = d.save();
        assert_eq!(saved["registers"]["active"][0]["priority"], 7);
        assert_eq!(access(&mut d, 0x814, None).unwrap(), 1);
        d.restore(&saved).unwrap();
        assert_eq!(d.save(), saved);
        access(&mut d, 0x120, Some(2)).unwrap();
        access(&mut d, 0x200, Some(0)).unwrap();
        assert_eq!(access(&mut d, 0x1c0, None).unwrap(), 5);
        // The parent still has its acknowledged priority 7, so priority 6
        // can nest even though the parent's programmed priority is now 1.
        source(&mut d, 4, 6);
        pins.pulse(4).unwrap();
        advance(&mut d).unwrap();
        assert_eq!(access(&mut d, 0x200, None).unwrap(), 0x10010);
        access(&mut d, 0x120, Some(1 << 4)).unwrap();
        access(&mut d, 0x200, Some(0)).unwrap();
        assert_eq!(access(&mut d, 0x1c0, None).unwrap(), 5);
        access(&mut d, 0x120, Some(1 << 5)).unwrap();
        access(&mut d, 0x200, Some(0)).unwrap();
        assert!(pins.irq());
        assert_eq!(access(&mut d, 0x200, None).unwrap(), 0x1000c);
    }
    #[test]
    fn held_level_retriggers_after_guest_clear_and_end_of_interrupt() {
        let (mut d, pins) = model(Arbitration::ActiveBitmap);
        source(&mut d, 1, 3);
        pins.level(1, true).unwrap();
        advance(&mut d).unwrap();
        access(&mut d, 0x200, None).unwrap();
        access(&mut d, 0x120, Some(2)).unwrap();
        access(&mut d, 0x200, Some(0)).unwrap();
        assert!(pins.irq());
        pins.level(1, false).unwrap();
        advance(&mut d).unwrap();
        access(&mut d, 0x120, Some(2)).unwrap();
        assert!(!pins.irq());
    }
    #[test]
    fn incomplete_sources_undeclared_ties_and_invalid_register_values_refuse() {
        let (mut d, pins) = model(Arbitration::Unique);
        access(&mut d, 0x80, Some(2)).unwrap();
        pins.pulse(1).unwrap();
        assert!(advance(&mut d).unwrap_err().contains("priority"));
        access(&mut d, 0x804, Some(2)).unwrap();
        assert!(access(&mut d, 0x200, None).unwrap_err().contains("vector"));
        source(&mut d, 1, 2);
        source(&mut d, 2, 2);
        assert!(
            access(&mut d, 0xc0, Some(4))
                .unwrap_err()
                .contains("arbitration")
        );
        assert!(access(&mut d, 0x804, Some(16)).is_err());
        assert!(access(&mut d, 0x200, Some(1)).is_err());
        assert!(access(&mut d, 0x1c8, Some(0)).is_err());
        assert!(pins.pulse(128).is_err());
        let saved = d.save();
        let mut corrupt = saved.clone();
        corrupt["registers"]["priorities"] = serde_json::json!([]);
        assert!(d.restore(&corrupt).is_err());
        assert_eq!(saved, d.save());
    }
}
