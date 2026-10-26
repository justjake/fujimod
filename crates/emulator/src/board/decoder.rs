//! Checked memory decoding choices, committed with every processor stopped.
use super::{
    factory::MemoryMap,
    memory::{BusMapping, Memory},
};
use fujimod_emulator_cpu::Cpu;
use serde_json::{Value, json};
use std::{ptr::NonNull, rc::Rc};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub processors: Vec<MemoryMap>,
    pub dma: Vec<BusMapping>,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decoder {
    pub initial: usize,
    pub choices: Vec<Choice>,
}

#[derive(Default)]
pub(super) struct Decoders {
    slots: Vec<Slot>,
    failure: Option<String>,
}
struct Slot {
    choices: Vec<Choice>,
    selected: usize,
    pending: Option<usize>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Checkpoint {
    slots: Vec<(usize, Option<usize>)>,
    failure: Option<String>,
}
impl Decoders {
    pub(super) fn observed_physical(&self, core: usize, address: u64, size: u64) -> Option<u64> {
        self.slots
            .iter()
            .flat_map(|s| &s.choices[s.selected].processors)
            .find(|m| {
                m.core == core
                    && m.address <= address
                    && address.saturating_add(size) <= m.address + m.size as u64
            })
            .map(|m| m.physical + address - m.address)
    }
    pub(super) fn observation_maps(&self, core: usize) -> Vec<MemoryMap> {
        self.slots
            .iter()
            .flat_map(|s| &s.choices)
            .flat_map(|c| &c.processors)
            .filter(|m| m.core == core)
            .cloned()
            .collect()
    }

    pub(super) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            slots: self.slots.iter().map(|s| (s.selected, s.pending)).collect(),
            failure: self.failure.clone(),
        }
    }
    pub(super) fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        if self.slots.len() != saved.slots.len()
            || self
                .slots
                .iter()
                .zip(&saved.slots)
                .any(|(slot, &(selected, pending))| {
                    selected >= slot.choices.len()
                        || pending.is_some_and(|v| v >= slot.choices.len())
                })
        {
            return Err("checkpoint memory decoder graph differs".into());
        }
        Ok(())
    }
    pub(super) fn restore_checkpoint(
        &mut self,
        saved: &Checkpoint,
        cpus: &[Rc<dyn Cpu>],
        memory: &Memory,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.failure = None;
        for (slot, &(selected, _)) in self.slots.iter_mut().zip(&saved.slots) {
            slot.pending = (slot.selected != selected).then_some(selected);
        }
        self.apply(cpus, memory)?;
        for (slot, &(_, pending)) in self.slots.iter_mut().zip(&saved.slots) {
            slot.pending = pending;
        }
        self.failure.clone_from(&saved.failure);
        Ok(())
    }
    pub fn new(decoders: Vec<Decoder>) -> Self {
        Self {
            slots: decoders
                .into_iter()
                .map(|d| Slot {
                    choices: d.choices,
                    selected: d.initial,
                    pending: None,
                })
                .collect(),
            failure: None,
        }
    }
    pub fn pending(&self) -> bool {
        self.slots.iter().any(|s| s.pending.is_some())
    }
    pub fn select(&mut self, decoder: usize, choice: usize, defer: bool) -> Result<(), String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let slot = self.slots.get_mut(decoder).ok_or("absent memory decoder")?;
        if choice >= slot.choices.len() {
            return Err("absent memory decoder choice".into());
        }
        if let Some(pending) = slot.pending {
            if pending != choice {
                return Err("conflicting memory choices before stopped boundary".into());
            }
        } else if slot.selected != choice {
            if !defer {
                return Err("memory decoding change requires a stopped boundary".into());
            }
            slot.pending = Some(choice);
        }
        Ok(())
    }
    pub fn apply(&mut self, cpus: &[Rc<dyn Cpu>], memory: &Memory) -> Result<(), String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        for slot in &mut self.slots {
            let Some(choice) = slot.pending else {
                continue;
            };
            let result = (|| {
                for map in &slot.choices[choice].processors {
                    let (base, backing) = memory
                        .regions()
                        .iter()
                        .find(|(base, backing)| {
                            *base <= map.physical
                                && map.physical + map.size as u64 <= *base + backing.len() as u64
                        })
                        .ok_or("decoder backing disappeared")?;
                    let pointer = unsafe { backing.as_ptr().add((map.physical - base) as usize) };
                    unsafe {
                        cpus[map.core].replace_physical(
                            map.address,
                            map.physical,
                            map.size,
                            map.permissions,
                            NonNull::new(pointer).ok_or("null decoder backing")?,
                        )?;
                    }
                }
                memory.replace_views(&slot.choices[choice].dma)?;
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                // A backend mapping failure may follow successful peer changes.
                // Keep the board unusable rather than execute a partial decode.
                self.failure = Some(format!("memory decoding failed: {error}"));
                return Err(self.failure.as_ref().unwrap().clone());
            }
            slot.selected = choice;
            slot.pending = None;
        }
        Ok(())
    }
    pub fn state(&self) -> Value {
        json!({"choices":self.slots.iter().map(|s|s.selected).collect::<Vec<_>>(),
            "pending":self.slots.iter().map(|s|s.pending).collect::<Vec<_>>(),"failure":self.failure})
    }
}

impl super::factory::Graph {
    pub(super) fn validate_decoders(&self, spans: &[(u64, u64)]) -> Result<(), String> {
        use std::collections::BTreeSet;
        let mut owned_processors = BTreeSet::new();
        let mut owned_dma = BTreeSet::new();
        for decoder in &self.decoders {
            let initial = decoder
                .choices
                .get(decoder.initial)
                .ok_or("absent initial memory choice")?;
            if initial.processors.is_empty() && initial.dma.is_empty() {
                return Err("memory decoder has no windows".into());
            }
            let processors: BTreeSet<_> = initial
                .processors
                .iter()
                .map(|m| (m.core, m.address, m.size))
                .collect();
            let dma: BTreeSet<_> = initial.dma.iter().map(|m| (m.address, m.size)).collect();
            if processors.len() != initial.processors.len() || dma.len() != initial.dma.len() {
                return Err("duplicate memory decoder window".into());
            }
            for map in &initial.processors {
                if !self.mappings.contains(map) || !owned_processors.insert((map.core, map.address))
                {
                    return Err("initial processor decode must own a unique mapped window".into());
                }
                if !matches!(self.processors[map.core].model,
                    fujimod_emulator_cpu::Model::CortexR5F | fujimod_emulator_cpu::Model::CortexA7) {
                    return Err("processor backend cannot replace memory decoding".into());
                }
            }
            for map in &initial.dma {
                if !self.dma_mappings.contains(map) || !owned_dma.insert(map.address) {
                    return Err("initial DMA decode must own a unique mapped window".into());
                }
            }
            for choice in &decoder.choices {
                if choice.processors.len() != processors.len()
                    || choice.dma.len() != dma.len()
                    || choice
                        .processors
                        .iter()
                        .map(|m| (m.core, m.address, m.size))
                        .collect::<BTreeSet<_>>()
                        != processors
                    || choice
                        .dma
                        .iter()
                        .map(|m| (m.address, m.size))
                        .collect::<BTreeSet<_>>()
                        != dma
                {
                    return Err("memory decoder choices must preserve window geometry".into());
                }
                for map in &choice.processors {
                    self.validate_memory_map(map, spans)?;
                }
                super::memory::validate_mappings(&choice.dma, spans)?;
            }
        }
        Ok(())
    }
}
