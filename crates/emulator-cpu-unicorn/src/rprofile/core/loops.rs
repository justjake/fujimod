//! Loop fast-forward for A32 and T32 code. No firmware identities or software
//! structures. A proof is rebuilt from permission-checked instruction bytes at
//! each entry. The execution owner bounds the stable window at every event/peer
//! boundary.
use super::*;
use fujimod_emulator_cpu::loop_proof::{self, Advance, AdvanceFailure, Isa, Proof, State};
use fujimod_emulator_cpu::loops::LoopMode;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Loops {
    pub mode: Cell<LoopMode>,
    pub window: Cell<bool>,
    pub resume: Cell<bool>,
    pub bypass: Cell<Option<u64>>,
    pub poll_instructions: Cell<u64>,
    pub counted_instructions: Cell<u64>,
    pub clock_poll_instructions: Cell<u64>,
    pub accelerated_instructions: Cell<u64>,
    pub candidate_entries: Cell<u64>,
    pub proofs_attempted: Cell<u64>,
    pub proofs_failed_structural: Cell<u64>,
    pub proofs_failed_window_inactive: Cell<u64>,
    pub proofs_failed_memory_flush_pending: Cell<u64>,
    pub proofs_failed_instruction_observer: Cell<u64>,
    pub proofs_failed_block_observer: Cell<u64>,
    pub proofs_failed_read_observer: Cell<u64>,
    pub proofs_failed_loop_bytes_unavailable: Cell<u64>,
    pub proofs_failed_cpsr_e_or_j: Cell<u64>,
    pub proofs_failed_it_block: Cell<u64>,
    pub proofs_failed_inputs_changed: Cell<u64>,
    pub proofs_failed_loop_condition_not_taken: Cell<u64>,
    pub proofs_failed_loop_read_unavailable: Cell<u64>,
    pub proofs_failed_iteration_model_unsupported: Cell<u64>,
    pub proofs_failed_counter_at_boundary: Cell<u64>,
    pub proofs_failed_budget_room: Cell<u64>,
    structural_failures: RefCell<BTreeMap<u64, Vec<u8>>>,
    unsupported_models: RefCell<BTreeMap<u64, Vec<u8>>>,
    unsupported_model_addresses: RefCell<BTreeSet<u64>>,
    #[cfg(debug_assertions)]
    unsupported_model_hits: RefCell<BTreeMap<u64, (Vec<u8>, u64)>>,
}
impl Loops {
    pub(super) fn unsupported_model_pc_count(&self) -> usize {
        self.unsupported_model_addresses.borrow().len()
    }
    #[cfg(debug_assertions)]
    pub(super) fn log_unsupported_models(&self) {
        if std::env::var_os("FUJIMOD_LOOP_MODEL_FAILURES").is_none() {
            return;
        }
        let hits = self.unsupported_model_hits.borrow();
        let mut entries: Vec<_> = hits.iter().collect();
        entries.sort_by_key(|(_, (_, count))| std::cmp::Reverse(*count));
        eprintln!(
            "unsupported loop iteration models: {} distinct PCs",
            hits.len()
        );
        for (&pc, (bytes, count)) in entries.into_iter().take(32) {
            let words = bytes_to_words(bytes);
            eprintln!("  pc=0x{pc:08x} entries={count} words={words:08x?}");
        }
    }
    #[cfg(debug_assertions)]
    pub(super) fn record_unsupported_model_block(&self, block: Block) {
        if block.bytes == 0 {
            return;
        }
        let Some(branch) = block
            .address
            .checked_add(u64::from(block.bytes))
            .and_then(|end| end.checked_sub(4))
        else {
            return;
        };
        let models = self.unsupported_models.borrow();
        let Some(bytes) = models
            .get(&branch)
            .filter(|bytes| bytes.len() == block.bytes as usize)
        else {
            return;
        };
        let mut hits = self.unsupported_model_hits.borrow_mut();
        let entry = hits.entry(branch).or_insert((bytes.clone(), 0));
        if entry.0 != *bytes {
            *entry = (bytes.clone(), 0);
        }
        entry.1 = entry.1.saturating_add(1);
    }
}
pub(super) struct Window<'a>(pub &'a Cell<bool>);
impl Drop for Window<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

fn bytes_to_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect()
}

impl Execution {
    pub(super) fn loop_candidate(&self, address: u64, word: u32) -> bool {
        if self.policy.get() != ExecutionPolicy::Exact
            || self.loops.mode.get() == LoopMode::Disabled
        {
            return false;
        }
        let Some((start, bytes)) = self.loop_bytes(Isa::A32, address, word) else {
            return false;
        };
        if self.loop_failure_cached(address, &bytes) {
            return false;
        }
        self.loops.structural_failures.borrow_mut().remove(&address);
        self.loops
            .proofs_attempted
            .set(self.loops.proofs_attempted.get().saturating_add(1));
        if Proof::decode(Isa::A32, start, address, &bytes).is_some() {
            true
        } else {
            self.loops
                .proofs_failed_structural
                .set(self.loops.proofs_failed_structural.get().saturating_add(1));
            self.loops
                .structural_failures
                .borrow_mut()
                .insert(address, bytes);
            false
        }
    }
    /// A structural or iteration-model failure already recorded for exactly
    /// these instruction bytes.
    fn loop_failure_cached(&self, address: u64, bytes: &[u8]) -> bool {
        self.loops
            .structural_failures
            .borrow()
            .get(&address)
            .is_some_and(|cached| *cached == bytes)
            || self
                .loops
                .unsupported_models
                .borrow()
                .get(&address)
                .is_some_and(|cached| *cached == bytes)
    }
    fn loop_bytes(&self, isa: Isa, address: u64, word: u32) -> Option<(u64, Vec<u8>)> {
        let start = loop_proof::loop_start(isa, address, word)?;
        let size = loop_proof::branch_size(isa, address, word)?;
        let mut bytes = vec![0; (address - start + size) as usize];
        self.cpu
            .read_virtual(start, Access::Fetch, &mut bytes)
            .ok()?;
        Some((start, bytes))
    }
    pub(super) fn invalidate_loop_failures(&self) {
        self.loops.structural_failures.borrow_mut().clear();
        self.loops.unsupported_models.borrow_mut().clear();
    }
    fn loop_read(&self, address: u32, width: usize) -> Option<u32> {
        if !(address as usize).is_multiple_of(width) {
            return None;
        }
        let mut bytes = [0; 4];
        // Permission-checked and observational. Device reads are refused here.
        if self
            .cpu
            .read_virtual(u64::from(address), Access::Read, &mut bytes[..width])
            .is_ok()
        {
            return Some(u32::from_le_bytes(bytes));
        }
        // A device-declared stable register, at exactly its declared width.
        let physical = self.cpu.translate(u64::from(address), Access::Read).ok()?;
        self.loop_reads.borrow().iter().find_map(|(base, read)| {
            (physical == base + read.register_offset)
                .then(|| read.read(read.register_offset, width as u32))
                .flatten()
                .map(|value| value as u32)
        })
    }
    fn loop_runtime_failure(&self, counter: &Cell<u64>) {
        counter.set(counter.get().saturating_add(1));
    }
    pub(super) fn execute_loop(&self, address: u64, word: u32) -> Result<bool, String> {
        let thumb = self.thumb.get();
        let isa = if thumb { Isa::T32 } else { Isa::A32 };
        if self.loops.mode.get() == LoopMode::Disabled
            || self.policy.get() != ExecutionPolicy::Exact
        {
            return Ok(false);
        }
        let Some(start) = loop_proof::loop_start(isa, address, word) else {
            return Ok(false);
        };
        // Every Thumb instruction reaches this observer, so a backward branch
        // whose bytes already failed is refused here, as translation-time
        // classification refuses A32 branches.
        if thumb
            && self
                .loop_bytes(isa, address, word)
                .is_some_and(|(_, bytes)| self.loop_failure_cached(address, &bytes))
        {
            return Ok(false);
        }
        self.loops
            .candidate_entries
            .set(self.loops.candidate_entries.get().saturating_add(1));
        if !self.loops.window.get() {
            self.loop_runtime_failure(&self.loops.proofs_failed_window_inactive);
            return Ok(false);
        }
        if self.memory.flush_pending() {
            self.loop_runtime_failure(&self.loops.proofs_failed_memory_flush_pending);
            return Ok(false);
        }
        if self.loops.bypass.take() == Some(address) {
            return Ok(false);
        }
        // Instruction and block observers on the loop must see every
        // iteration. The index answers by address instead of scanning every
        // installed observer on each loop entry.
        let index = self.observer_index.borrow();
        if index.instructions.any_in(start, address) {
            self.loop_runtime_failure(&self.loops.proofs_failed_instruction_observer);
            return Ok(false);
        }
        if index.blocks.any_in(start, address) {
            self.loop_runtime_failure(&self.loops.proofs_failed_block_observer);
            return Ok(false);
        }
        drop(index);
        self.loops
            .proofs_attempted
            .set(self.loops.proofs_attempted.get().saturating_add(1));
        let Some((start, bytes)) = self.loop_bytes(isa, address, word) else {
            self.loop_runtime_failure(&self.loops.proofs_failed_loop_bytes_unavailable);
            return Ok(false);
        };
        let Some(proof) = Proof::decode(isa, start, address, &bytes) else {
            self.loops
                .proofs_failed_structural
                .set(self.loops.proofs_failed_structural.get().saturating_add(1));
            self.loops
                .structural_failures
                .borrow_mut()
                .insert(address, bytes);
            return Ok(false);
        };
        // A read observer cannot fire in a register-only loop. Keep the broad
        // observer gate only for proofs whose body actually reads memory.
        if proof.reads_memory() && !self.observer_index.borrow().reads.is_empty() {
            self.loop_runtime_failure(&self.loops.proofs_failed_read_observer);
            return Ok(false);
        }
        let mut state = State {
            registers: [0; 15],
            cpsr: self.scalar(arm::CPSR)?,
        };
        if state.cpsr & ((1 << 9) | (1 << 24)) != 0 {
            self.loop_runtime_failure(&self.loops.proofs_failed_cpsr_e_or_j);
            return Ok(false);
        }
        // The proof models a branch outside any IT block.
        if state.cpsr & 0x0600_fc00 != 0 {
            self.loop_runtime_failure(&self.loops.proofs_failed_it_block);
            return Ok(false);
        }
        for (r, value) in state.registers.iter_mut().enumerate() {
            *value = self.scalar(arm::CORE[r])?;
        }
        let room = self.cap.get().saturating_sub(self.count.get() - 1);
        let Advance {
            state: after,
            iterations: cycles,
            counted,
        } = match proof.advance(state, (room / proof.length()).max(1), |p, n| {
            self.loop_read(p, n)
        }) {
            Ok(advanced) => advanced,
            Err(reason) => {
                let counter = match reason {
                    AdvanceFailure::BudgetRoom => &self.loops.proofs_failed_budget_room,
                    AdvanceFailure::ConditionNotTaken => {
                        &self.loops.proofs_failed_loop_condition_not_taken
                    }
                    AdvanceFailure::ReadUnavailable => {
                        &self.loops.proofs_failed_loop_read_unavailable
                    }
                    AdvanceFailure::InputsChanged => &self.loops.proofs_failed_inputs_changed,
                    AdvanceFailure::IterationModelUnsupported => {
                        self.loops
                            .unsupported_models
                            .borrow_mut()
                            .insert(address, bytes.clone());
                        self.loops
                            .unsupported_model_addresses
                            .borrow_mut()
                            .insert(address);
                        #[cfg(debug_assertions)]
                        {
                            let mut hits = self.loops.unsupported_model_hits.borrow_mut();
                            let entry = hits.entry(address).or_insert((bytes.clone(), 0));
                            if entry.0 != bytes {
                                *entry = (bytes.clone(), 0);
                            }
                        }
                        self.cpu.invalidate_code(Some((address, address + 3)))?;
                        &self.loops.proofs_failed_iteration_model_unsupported
                    }
                    AdvanceFailure::CounterAtBoundary => {
                        &self.loops.proofs_failed_counter_at_boundary
                    }
                };
                self.loop_runtime_failure(counter);
                return Ok(false);
            }
        };
        let counter = if counted {
            &self.loops.counted_instructions
        } else {
            &self.loops.poll_instructions
        };
        counter.set(counter.get().saturating_add(proof.length()));
        if self.loops.mode.get() != LoopMode::Accelerate || cycles * proof.length() > room {
            self.loop_runtime_failure(&self.loops.proofs_failed_budget_room);
            return Ok(false);
        }
        let skipped = cycles * proof.length();
        for (r, value) in after.registers.iter().enumerate() {
            if *value != state.registers[r] {
                self.set(arm::CORE[r], *value)?;
            }
        }
        if after.cpsr != state.cpsr {
            self.set(arm::CPSR, after.cpsr)?;
        }
        self.count.set(self.count.get() - 1 + skipped);
        counter.set(counter.get().saturating_add(skipped - proof.length()));
        self.loops.accelerated_instructions.set(
            self.loops
                .accelerated_instructions
                .get()
                .saturating_add(skipped),
        );
        self.reset_budget();
        self.loops.resume.set(true);
        self.loops.bypass.set(Some(proof.branch()));
        self.set(arm::PC, proof.branch() as u32 | u32::from(thumb))?;
        self.cpu.stop()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
