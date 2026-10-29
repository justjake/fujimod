//! Durable board state. Engine snapshots and physical memory are detached data;
//! hooks, shared reservations and observation wiring bind to the receiving board.
use super::*;
use fujimod_emulator_devices::checkpoint::Checkpoint as Encoded;
use serde::{Deserialize, Serialize};
#[cfg(test)]
mod tests;

const KIND: &str = "board-keyframe-v2";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header<S, D, E> {
    runtime: S,
    decoders: D,
    entries: E,
    processors: usize,
    next_core: usize,
    slice_left: usize,
    quantum: Option<usize>,
    host_pause: bool,
    isolated_core: Option<usize>,
    hooks: Vec<usize>,
    mmio: Vec<(usize, u64, usize)>,
    controllers: Vec<Option<usize>>,
    observations: Vec<(usize, Vec<u64>)>,
}
impl Keyframe {
    /// Read execution counters from a portable archive without attaching it to a board.
    pub fn portable_metadata(encoded: &Encoded) -> Result<(u64, Vec<u64>), String> {
        let header: Header<Saved, decoder::Checkpoint, observe::Entries> = encoded.decode(KIND)?;
        if encoded.blobs().len() != header.processors + 1
            || encoded.children().len() != 2
            || header.processors != header.runtime.instructions.len()
        {
            return Err("snapshot processor or child count differs".into());
        }
        Ok((header.runtime.timeline.now(), header.runtime.instructions))
    }
}
impl Keyframe {
    pub fn export(&self) -> Result<Encoded, String> {
        if let Some(reason) = &self.durable_blocker {
            return Err(reason.clone());
        }
        let header = Header {
            runtime: &self.saved,
            decoders: &self.decoders,
            entries: &self.entries,
            processors: self.processors.len(),
            next_core: self.next_core,
            slice_left: self.slice_left,
            quantum: self.quantum,
            host_pause: self.host_pause,
            isolated_core: self.isolated_core,
            hooks: self.hooks.iter().map(|(core, _)| *core).collect(),
            mmio: self.mmio.clone(),
            controllers: self.controllers.clone(),
            observations: self.observations.clone(),
        };
        let mut blobs = self
            .processors
            .iter()
            .map(|cpu| cpu.export().map(Into::into))
            .collect::<Result<Vec<_>, _>>()?;
        blobs.push(self.exclusive.export_checkpoint().into());
        Ok(Encoded::encode(KIND, &header)?
            .with_blobs(blobs)
            .with_children(vec![self.memory.export()?, self.devices.export()?]))
    }
}
impl Board {
    pub fn import_keyframe(&self, encoded: &Encoded) -> Result<Keyframe, String> {
        if let Some(reason) = self.durable_blocker() {
            return Err(reason);
        }
        let state = self
            .state
            .try_borrow()
            .map_err(|_| "snapshot import requires a stopped board")?;
        state.checkpoint_boundary()?;
        let header: Header<Saved, decoder::Checkpoint, observe::Entries> = encoded.decode(KIND)?;
        let n = self.cpus.len();
        if header.processors != n
            || encoded.blobs().len() != n + 1
            || encoded.children().len() != 2
            || header.hooks != self.hooks.iter().map(|(core, _)| *core).collect::<Vec<_>>()
            || header.mmio != self.mmio
            || header.controllers
                != state
                    .core_interrupts
                    .iter()
                    .map(|c| c.as_ref().map(|c| c.device))
                    .collect::<Vec<_>>()
            || header.observations != self.observation_topology()
            || header.isolated_core.is_some()
            || n == 0
            || header.next_core >= n
            || header.quantum == Some(0)
        {
            return Err("snapshot processor/execution wiring differs".into());
        }
        let saved = &header.runtime;
        if [
            &saved.interrupt_changed,
            &saved.released,
            &saved.waiting,
            &saved.waiting_event,
            &saved.events,
            &saved.interrupt_delivery,
        ]
        .iter()
        .any(|v| v.len() != n)
            || saved.pins.len() != n
            || saved.instructions.len() != n
            || saved.accounted.len() != n
            || saved.mprofile.len() != n
            || saved.aprofile.len() != n
            || saved.processor_deadlines.len() != n
            || saved.processor_generations.len() != n
            || saved.code_run.is_some_and(|id| id >= n)
            || saved.deadlines.len() != state.bus.len()
            || saved.generations.len() != state.bus.len()
            || saved.timeline.ratio() != state.timeline.ratio()
        {
            return Err("snapshot scheduler geometry or clock differs".into());
        }
        saved.timeline.validate_targets(state.bus.len(), n)?;
        for (profile, saved) in state.mprofile.iter().zip(&saved.mprofile) {
            match (profile, saved) {
                (Some(profile), Some(saved)) => profile.validate_checkpoint(saved)?,
                (None, None) => (),
                _ => return Err("snapshot M-profile wiring differs".into()),
            }
        }
        for (profile, saved_profile) in state.aprofile.iter().zip(&saved.aprofile) {
            match (profile, saved_profile) {
                (Some(profile), Some(saved_profile)) => {
                    profile.validate_checkpoint(saved_profile, saved.timeline.now())?
                }
                (None, None) => (),
                _ => return Err("snapshot A-profile wiring differs".into()),
            }
        }
        match (&state.interrupts, &saved.gic) {
            (Some(current), Some(saved)) => current.gic.boxed_clone().restore(saved)?,
            (None, None) => (),
            _ => return Err("snapshot GIC wiring differs".into()),
        }
        state.decoders.validate_checkpoint(&header.decoders)?;
        let memory = state.memory.import_checkpoint(&encoded.children()[0])?;
        let devices = state.bus.import_checkpoint(&encoded.children()[1])?;
        let exclusive = state
            .exclusive
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .import_checkpoint(&encoded.blobs()[n])?;
        let processors = self
            .cpus
            .iter()
            .zip(&encoded.blobs()[..n])
            .map(|(cpu, bytes)| cpu.import_snapshot(bytes))
            .collect::<Result<_, _>>()?;
        Ok(Keyframe {
            owner: Rc::downgrade(&self.state),
            processors,
            devices,
            memory,
            decoders: header.decoders,
            exclusive,
            saved: header.runtime,
            entries: header.entries,
            next_core: header.next_core,
            slice_left: header.slice_left,
            quantum: header.quantum,
            host_pause: header.host_pause,
            isolated_core: header.isolated_core,
            hooks: self.hooks.clone(),
            mmio: header.mmio,
            controllers: header.controllers,
            observations: header.observations,
            durable_blocker: None,
        })
    }
}

pub(super) mod policy {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &ExecutionPolicy,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.name().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ExecutionPolicy, D::Error> {
        ExecutionPolicy::parse(&String::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}
pub(super) mod timeline {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &Timeline,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.save().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Timeline, D::Error> {
        Timeline::restore(&Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
pub(super) mod frames {
    use super::*;
    use fujimod_emulator_devices::graphics::scanout::Frame;
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Image<B> {
        output: usize,
        sequence: u64,
        width: u32,
        height: u32,
        rgba: B,
    }
    #[derive(Serialize)]
    struct Bytes<'a>(#[serde(with = "fujimod_emulator_devices::checkpoint::bytes")] &'a [u8]);
    #[derive(Deserialize)]
    struct Owned(#[serde(with = "fujimod_emulator_devices::checkpoint::bytes")] Vec<u8>);
    pub fn serialize<S: serde::Serializer>(
        frames: &BTreeMap<usize, Frame>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        frames
            .iter()
            .map(|(id, frame)| {
                (
                    *id,
                    Image {
                        output: frame.output,
                        sequence: frame.sequence,
                        width: frame.width,
                        height: frame.height,
                        rgba: Bytes(&frame.rgba),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<usize, Frame>, D::Error> {
        BTreeMap::<usize, Image<Owned>>::deserialize(deserializer)?
            .into_iter()
            .map(|(id, frame)| {
                let len = (frame.width as usize)
                    .checked_mul(frame.height as usize)
                    .and_then(|n| n.checked_mul(4));
                if len != Some(frame.rgba.0.len()) {
                    return Err(serde::de::Error::custom("snapshot frame extent differs"));
                }
                Ok((
                    id,
                    Frame {
                        output: frame.output,
                        sequence: frame.sequence,
                        width: frame.width,
                        height: frame.height,
                        rgba: frame.rgba.0,
                    },
                ))
            })
            .collect()
    }
}

pub(super) mod records {
    use super::*;
    use fujimod_emulator_devices::record::Value as RecordValue;
    use std::borrow::Cow;

    #[derive(Serialize)]
    #[serde(tag = "kind", content = "value")]
    enum ValueRef<'a> {
        None,
        Int(i128),
        Float(f64),
        Bool(bool),
        Str(&'a str),
        Static(&'a str),
        List(Vec<ValueRef<'a>>),
        Tuple(Vec<ValueRef<'a>>),
        Bytes(&'a [u8]),
        Dict(Vec<(ValueRef<'a>, ValueRef<'a>)>),
        Hex(u128),
    }

    impl<'a> From<&'a RecordValue> for ValueRef<'a> {
        fn from(value: &'a RecordValue) -> Self {
            match value {
                RecordValue::None => Self::None,
                RecordValue::Int(value) => Self::Int(*value),
                RecordValue::Float(value) => Self::Float(*value),
                RecordValue::Bool(value) => Self::Bool(*value),
                RecordValue::Str(value) => Self::Str(value),
                RecordValue::Static(value) => Self::Static(value),
                RecordValue::List(values) => Self::List(values.iter().map(Into::into).collect()),
                RecordValue::Tuple(values) => Self::Tuple(values.iter().map(Into::into).collect()),
                RecordValue::Bytes(value) => Self::Bytes(value),
                RecordValue::Dict(values) => Self::Dict(
                    values
                        .iter()
                        .map(|(key, value)| (key.into(), value.into()))
                        .collect(),
                ),
                RecordValue::Hex(value) => Self::Hex(*value),
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(tag = "kind", content = "value", deny_unknown_fields)]
    enum SavedValue {
        None,
        Int(i128),
        Float(f64),
        Bool(bool),
        Str(String),
        Static(String),
        List(Vec<SavedValue>),
        Tuple(Vec<SavedValue>),
        Bytes(Vec<u8>),
        Dict(Vec<(SavedValue, SavedValue)>),
        Hex(u128),
    }

    impl From<SavedValue> for RecordValue {
        fn from(value: SavedValue) -> Self {
            match value {
                SavedValue::None => Self::None,
                SavedValue::Int(value) => Self::Int(value),
                SavedValue::Float(value) => Self::Float(value),
                SavedValue::Bool(value) => Self::Bool(value),
                SavedValue::Str(value) => Self::Str(value),
                SavedValue::Static(value) => Self::Static(Cow::Owned(value)),
                SavedValue::List(values) => {
                    Self::List(values.into_iter().map(Into::into).collect())
                }
                SavedValue::Tuple(values) => {
                    Self::Tuple(values.into_iter().map(Into::into).collect())
                }
                SavedValue::Bytes(value) => Self::Bytes(value),
                SavedValue::Dict(values) => Self::Dict(
                    values
                        .into_iter()
                        .map(|(key, value)| (key.into(), value.into()))
                        .collect(),
                ),
                SavedValue::Hex(value) => Self::Hex(value),
            }
        }
    }

    #[derive(Serialize)]
    struct RecordRef<'a> {
        name: &'a str,
        fields: Vec<(&'a str, ValueRef<'a>)>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SavedRecord {
        name: String,
        fields: Vec<(String, SavedValue)>,
    }

    pub fn serialize<S: serde::Serializer>(
        records: &[Record],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        records
            .iter()
            .map(|record| RecordRef {
                name: record.name.as_ref(),
                fields: record
                    .fields
                    .iter()
                    .map(|(key, value)| (key.as_ref(), value.into()))
                    .collect(),
            })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Record>, D::Error> {
        Ok(Vec::<SavedRecord>::deserialize(deserializer)?
            .into_iter()
            .map(|record| Record {
                name: Cow::Owned(record.name),
                fields: record
                    .fields
                    .into_iter()
                    .map(|(key, value)| (Cow::Owned(key), value.into()))
                    .collect(),
            })
            .collect())
    }
}
