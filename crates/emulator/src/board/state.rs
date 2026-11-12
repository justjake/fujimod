//! Complete stopped-board state for direct comparison. Device state is the
//! same typed checkpoint used by keyframes; no content digest is computed.
use super::{bus, memory, Board};
use fujimod_emulator_cpu::Model;
use serde_json::Value;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessorState {
    pub model: Model,
    pub architecture: Vec<u8>,
    pub backings: Vec<usize>,
    pub regions: Vec<(u64, u64, u32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrameState {
    pub output: usize,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, PartialEq)]
pub struct BoardState {
    pub(super) memory: memory::Checkpoint,
    pub processors: Vec<ProcessorState>,
    pub bus: bus::State,
    pub runtime: Value,
    pub records: Vec<String>,
    pub console: Vec<String>,
    pub frames: Vec<FrameState>,
    pub pc_entries: Value,
}

impl fmt::Debug for BoardState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoardState")
            .field("devices", &self.bus.devices.len())
            .field("processors", &self.processors.len())
            .field("frames", &self.frames.len())
            .finish_non_exhaustive()
    }
}

impl BoardState {
    /// First structural difference between two complete board states.
    pub fn diff(&self, other: &Self) -> Option<String> {
        if let Some(message) = self.memory.diff(&other.memory) {
            return Some(format!("memory/{message}"));
        }
        if self.processors.len() != other.processors.len() {
            return Some(format!(
                "processors: count {} differs from {}",
                self.processors.len(),
                other.processors.len()
            ));
        }
        for (index, (left, right)) in self.processors.iter().zip(&other.processors).enumerate() {
            if left.model != right.model {
                return Some(format!("processors[{index}]: model differs"));
            }
            if let Some(offset) = first_difference(&left.architecture, &right.architecture) {
                if let (Ok(left), Ok(right)) = (
                    serde_json::from_slice::<Value>(&left.architecture),
                    serde_json::from_slice::<Value>(&right.architecture),
                ) && let Some(message) = diff_value(&left, &right, "architecture")
                {
                    return Some(format!("processors[{index}]: {message}"));
                }
                return Some(format!(
                    "processors[{index}]: architectural state differs at byte {offset}"
                ));
            }
            if left.backings != right.backings {
                return Some(format!("processors[{index}]: backing geometry differs"));
            }
            if left.regions != right.regions {
                return Some(format!("processors[{index}]: memory regions differ"));
            }
        }
        if let Some(message) = diff_bus(&self.bus, &other.bus) {
            return Some(format!("bus/{message}"));
        }
        if let Some(message) = diff_value(&self.runtime, &other.runtime, "runtime") {
            return Some(message);
        }
        if self.records != other.records {
            return Some("records differ".into());
        }
        if self.console != other.console {
            return Some("console differs".into());
        }
        if self.frames.len() != other.frames.len() {
            return Some("frames: count differs".into());
        }
        for (index, (left, right)) in self.frames.iter().zip(&other.frames).enumerate() {
            if left.output != right.output
                || left.sequence != right.sequence
                || left.width != right.width
                || left.height != right.height
            {
                return Some(format!("frames[{index}]: metadata differs"));
            }
            if let Some(offset) = first_difference(&left.rgba, &right.rgba) {
                return Some(format!("frames[{index}]: pixels differ at byte {offset}"));
            }
        }
        diff_value(&self.pc_entries, &other.pc_entries, "pc_entries")
    }

    /// Canonical typed encoding for clients that compare states as bytes.
    pub fn to_checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        use fujimod_emulator_devices::checkpoint::Checkpoint;
        let mut processors = Vec::new();
        for processor in &self.processors {
            processors.push(
                Checkpoint::encode(
                    "processor",
                    &serde_json::json!({
                        "model":processor.model,
                        "backings":processor.backings,
                        "regions":processor.regions,
                    }),
                )?
                .with_blobs(vec![processor.architecture.clone().into()]),
            );
        }
        let mut devices = Vec::new();
        for device in &self.bus.devices {
            devices.push(
                Checkpoint::encode(
                    "device",
                    &serde_json::json!({
                        "kind":device.kind,
                        "mappings":device.mappings,
                        "native_dependencies":device.native_dependencies,
                    }),
                )?
                .with_children(vec![device.checkpoint.clone()]),
            );
        }
        let mut frames = Vec::new();
        for frame in &self.frames {
            frames.push(
                Checkpoint::encode(
                    "frame",
                    &serde_json::json!({
                        "output":frame.output,
                        "sequence":frame.sequence,
                        "width":frame.width,
                        "height":frame.height,
                    }),
                )?
                .with_blobs(vec![frame.rgba.clone().into()]),
            );
        }
        Ok(Checkpoint::encode(
            "board-state",
            &serde_json::json!({
                "bus":{
                    "mode":self.bus.mode.name(),
                    "unmodeled":self.bus.unmodeled,
                    "discovery_shadow":self.bus.discovery_shadow,
                    "continue_refusals":self.bus.continue_refusals,
                    "frontier":self.bus.frontier,
                    "skipped_deadlines":self.bus.skipped_deadlines,
                    "touched":self.bus.touched,
                    "dirty":self.bus.dirty,
                    "wake_queue":self.bus.wake_queue,
                },
                "runtime":self.runtime,
                "records":self.records,
                "console":self.console,
                "pc_entries":self.pc_entries,
            }),
        )?
        .with_children(vec![
            self.memory.export()?,
            Checkpoint::encode("processors", &())?.with_children(processors),
            Checkpoint::encode("devices", &())?.with_children(devices),
            Checkpoint::encode("frames", &())?.with_children(frames),
        ]))
    }

    /// Readable direct-state JSON; binary content stays in the typed checkpoint.
    pub fn to_json(&self) -> serde_json::Value {
        use fujimod_emulator_devices::state::bytes as state_bytes;
        serde_json::json!({
            "memory":self.memory.to_json(),
            "processors":self.processors.iter().map(|processor| serde_json::json!({
                "model":processor.model,
                "architecture":state_bytes(&processor.architecture),
                "backings":processor.backings,
                "regions":processor.regions,
            })).collect::<Vec<_>>(),
            "bus":{
                "mode":self.bus.mode.name(),
                "unmodeled":self.bus.unmodeled,
                "discovery_shadow":self.bus.discovery_shadow,
                "continue_refusals":self.bus.continue_refusals,
                "frontier":self.bus.frontier,
                "skipped_deadlines":self.bus.skipped_deadlines,
                "wakeups":{
                    "touched":self.bus.touched,
                    "dirty":self.bus.dirty,
                    "queue":self.bus.wake_queue,
                },
                "devices":self.bus.devices.iter().map(|device| serde_json::json!({
                    "kind":device.kind,
                    "windows":device.mappings,
                    "native_dependencies":device.native_dependencies,
                    "state":device.checkpoint.state(),
                })).collect::<Vec<_>>(),
            },
            "runtime":self.runtime,
            "records":self.records,
            "console":self.console,
            "frames":self.frames.iter().map(|frame| serde_json::json!({
                "output":frame.output,
                "sequence":frame.sequence,
                "width":frame.width,
                "height":frame.height,
                "rgba":state_bytes(&frame.rgba),
            })).collect::<Vec<_>>(),
            "pc_entries":self.pc_entries,
        })
    }
}

impl Board {
    /// Complete future-affecting state while every processor is stopped.
    pub fn complete_state(&self) -> Result<BoardState, String> {
        let s = self.state.try_borrow().map_err(|_| "state requires a stopped board")?;
        if s.active_slice.is_some() || s.parallel.is_some() || s.memory_access {
            return Err("state requires a stopped board".into());
        }
        let processors = self
            .cpus
            .iter()
            .map(|cpu| {
                let state = cpu.state()?;
                let mut regions = cpu
                    .memory_regions()?
                    .into_iter()
                    .map(|region| (region.begin, region.end, region.permissions))
                    .collect::<Vec<_>>();
                regions.sort_unstable();
                Ok(ProcessorState {
                    model: cpu.model(),
                    architecture: state.architecture,
                    backings: state.backings,
                    regions,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut runtime = self.state_with_bus(Value::Null);
        if let Some(object) = runtime.as_object_mut() {
            object.remove("bus");
        }
        runtime["scheduler"] = serde_json::json!({
            "deadlines":s.deadlines,
            "generations":s.generations,
            "processor_deadlines":s.processor_deadlines,
            "processor_generations":s.processor_generations,
            "interrupt_delivery":s.interrupt_delivery,
            "irq_observations":s.irq_observations,
            "isolated_core":self.isolated_core.get(),
            "host_pause":self.host_pause.get(),
        });
        let records = s.records.iter().map(|record| format!("{record:?}")).collect();
        let console = s.console.clone();
        let frames = s
            .frames
            .iter()
            .map(|(output, frame)| FrameState {
                output: *output,
                sequence: frame.sequence,
                width: frame.width,
                height: frame.height,
                rgba: frame.rgba.clone(),
            })
            .collect();
        Ok(BoardState {
            memory: s.memory.checkpoint_full(),
            processors,
            bus: s.bus.state_capture()?,
            runtime,
            records,
            console,
            frames,
            pc_entries: Value::Array(self.pc_entries()),
        })
    }
}

fn diff_bus(left: &bus::State, right: &bus::State) -> Option<String> {
    if left.mode != right.mode {
        return Some("mode differs".into());
    }
    if left.continue_refusals != right.continue_refusals {
        return Some("continue_refusals differs".into());
    }
    for (name, a, b) in [
        ("unmodeled", &left.unmodeled, &right.unmodeled),
        ("discovery_shadow", &left.discovery_shadow, &right.discovery_shadow),
        ("frontier", &left.frontier, &right.frontier),
    ] {
        if let Some(message) = diff_value(a, b, name) {
            return Some(message);
        }
    }
    if left.skipped_deadlines != right.skipped_deadlines {
        return Some("skipped_deadlines differs".into());
    }
    if left.touched != right.touched {
        return Some("wakeups/touched differs".into());
    }
    if left.dirty != right.dirty {
        return Some("wakeups/dirty differs".into());
    }
    if left.wake_queue != right.wake_queue {
        return Some("wakeups/queue differs".into());
    }
    if left.devices.len() != right.devices.len() {
        return Some(format!(
            "devices: count {} differs from {}",
            left.devices.len(),
            right.devices.len()
        ));
    }
    for (index, (a, b)) in left.devices.iter().zip(&right.devices).enumerate() {
        if a.kind != b.kind || a.mappings != b.mappings || a.native_dependencies != b.native_dependencies {
            return Some(format!("devices[{index}]: wiring or kind differs"));
        }
        if let Some(message) = a.checkpoint.diff(&b.checkpoint) {
            return Some(format!("devices[{index}] ({})/{message}", a.kind));
        }
    }
    None
}

fn diff_value(left: &Value, right: &Value, path: &str) -> Option<String> {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys()) {
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => {
                        if let Some(message) = diff_value(x, y, &format!("{path}.{key}")) {
                            return Some(message);
                        }
                    }
                    (Some(_), None) => return Some(format!("{path}.{key}: only in left state")),
                    (None, Some(_)) => return Some(format!("{path}.{key}: only in right state")),
                    (None, None) => {}
                }
            }
            None
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: length {} differs from {}", a.len(), b.len()));
            }
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                if let Some(message) = diff_value(x, y, &format!("{path}[{index}]")) {
                    return Some(message);
                }
            }
            None
        }
        (Value::Number(a), Value::Number(b)) if a == b => None,
        _ if left == right => None,
        _ => Some(format!("{path}: {left} differs from {right}")),
    }
}

fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
    left.iter()
        .zip(right)
        .position(|(a, b)| a != b)
        .or_else(|| (left.len() != right.len()).then_some(left.len().min(right.len())))
}

impl fmt::Display for BoardState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format!(
            "board state ({} devices, {} processors, {} frames)",
            self.bus.devices.len(),
            self.processors.len(),
            self.frames.len()
        ))
    }
}
