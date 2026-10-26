//! Conservative A32 loop proofs. No firmware identities or software structures.
//! A proof is rebuilt from permission-checked instruction bytes at each entry.
//! The execution owner bounds the stable window at every event/peer boundary.
use super::*;
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
        eprintln!("unsupported loop iteration models: {} distinct PCs", hits.len());
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
        let Some(bytes) = models.get(&branch).filter(|bytes| bytes.len() == block.bytes as usize)
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

#[derive(Clone, Copy, Debug)]
enum Operand {
    Immediate(u32, Option<bool>),
    Register(usize, u32, u32),
}
#[derive(Clone, Copy, Debug)]
enum Op {
    Data {
        opcode: u32,
        flags: bool,
        dst: usize,
        lhs: usize,
        rhs: Operand,
    },
    Load {
        dst: usize,
        base: usize,
        offset: u32,
        width: usize,
    },
}
#[derive(Clone, Debug)]
struct Proof {
    branch: u64,
    condition: u32,
    body: Vec<Op>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct State {
    registers: [u32; 15],
    cpsr: u32,
}

#[derive(Clone, Copy)]
enum AdvanceFailure {
    BudgetRoom,
    ConditionNotTaken,
    ReadUnavailable,
    IterationModelUnsupported,
    CounterAtBoundary,
}

/// Only a direct, backward A32 B (never BL, BLX or the unconditional space).
fn target(address: u64, word: u32) -> Option<u64> {
    if word >> 28 == 15 || word & 0x0f00_0000 != 0x0a00_0000 {
        return None;
    }
    let displacement = ((word << 8) as i32 >> 6) as i64;
    let start = address.checked_add(8)?.checked_add_signed(displacement)?;
    (start <= address && address - start <= 60).then_some(start)
}
fn decode(word: u32) -> Option<Op> {
    // Conditional bodies, PC, writeback, register-offset loads, atomics,
    // coprocessors, stores, barriers and hints are deliberately unsupported.
    if word >> 28 != 14 {
        return None;
    }
    let dst = ((word >> 12) & 15) as usize;
    let lhs = ((word >> 16) & 15) as usize;
    if dst == 15 || lhs == 15 {
        return None;
    }
    if word & 0x0f30_0000 == 0x0510_0000 {
        let offset = word & 0xfff;
        return Some(Op::Load {
            dst,
            base: lhs,
            offset: if word & (1 << 23) == 0 {
                0u32.wrapping_sub(offset)
            } else {
                offset
            },
            width: if word & (1 << 22) == 0 { 4 } else { 1 },
        });
    }
    if word & 0x0c00_0000 != 0 {
        return None;
    }
    let opcode = (word >> 21) & 15;
    if !matches!(opcode, 0 | 1 | 2 | 4 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15) {
        return None;
    }
    let flags = word & (1 << 20) != 0;
    if (8..=11).contains(&opcode) && (!flags || dst != 0) {
        return None;
    }
    let rhs = if word & (1 << 25) != 0 {
        let rotate = ((word >> 8) & 15) * 2;
        let value = (word & 255).rotate_right(rotate);
        Operand::Immediate(value, (rotate != 0).then_some(value >> 31 != 0))
    } else {
        if word & 16 != 0 || word & 15 == 15 {
            return None;
        }
        Operand::Register((word & 15) as usize, (word >> 5) & 3, (word >> 7) & 31)
    };
    Some(Op::Data {
        opcode,
        flags,
        dst,
        lhs,
        rhs,
    })
}
impl Operand {
    fn value(self, state: State) -> (u32, bool) {
        let carry = state.cpsr & (1 << 29) != 0;
        match self {
            Self::Immediate(value, out) => (value, out.unwrap_or(carry)),
            Self::Register(r, kind, shift) => {
                let value = state.registers[r];
                match (kind, shift) {
                    (0, 0) => (value, carry),
                    (0, n) => (value << n, value & (1 << (32 - n)) != 0),
                    (1, 0) => (0, value >> 31 != 0),
                    (1, n) => (value >> n, value & (1 << (n - 1)) != 0),
                    (2, 0) => (((value as i32) >> 31) as u32, value >> 31 != 0),
                    (2, n) => (((value as i32) >> n) as u32, value & (1 << (n - 1)) != 0),
                    (3, 0) => ((u32::from(carry) << 31) | (value >> 1), value & 1 != 0),
                    (3, n) => (value.rotate_right(n), value & (1 << (n - 1)) != 0),
                    _ => unreachable!(),
                }
            }
        }
    }
}
impl Proof {
    fn decode(start: u64, branch: u64, words: &[u32]) -> Option<Self> {
        let (&word, body) = words.split_last()?;
        if target(branch, word)? != start {
            return None;
        }
        Some(Self {
            branch,
            condition: word >> 28,
            body: body
                .iter()
                .map(|&word| decode(word))
                .collect::<Option<_>>()?,
        })
    }
    fn apply(
        &self,
        mut state: State,
        mut read: impl FnMut(u32, usize) -> Option<u32>,
    ) -> Option<State> {
        for op in &self.body {
            match *op {
                Op::Load {
                    dst,
                    base,
                    offset,
                    width,
                } => {
                    state.registers[dst] = read(state.registers[base].wrapping_add(offset), width)?;
                }
                Op::Data {
                    opcode,
                    flags,
                    dst,
                    lhs,
                    rhs,
                } => {
                    let a = state.registers[lhs];
                    let (b, shifted_carry) = rhs.value(state);
                    let (result, carry, overflow) = match opcode {
                        2 | 10 => {
                            let result = a.wrapping_sub(b);
                            (result, a >= b, ((a ^ b) & (a ^ result)) >> 31 != 0)
                        }
                        4 | 11 => {
                            let (result, carry) = a.overflowing_add(b);
                            (result, carry, (!(a ^ b) & (a ^ result)) >> 31 != 0)
                        }
                        _ => (
                            match opcode {
                                0 | 8 => a & b,
                                1 | 9 => a ^ b,
                                12 => a | b,
                                13 => b,
                                14 => a & !b,
                                15 => !b,
                                _ => unreachable!(),
                            },
                            shifted_carry,
                            state.cpsr & (1 << 28) != 0,
                        ),
                    };
                    if !(8..=11).contains(&opcode) {
                        state.registers[dst] = result;
                    }
                    if flags {
                        state.cpsr = (state.cpsr & 0x0fff_ffff)
                            | (result & 0x8000_0000)
                            | (u32::from(result == 0) << 30)
                            | (u32::from(carry) << 29)
                            | (u32::from(overflow) << 28);
                    }
                }
            }
        }
        Some(state)
    }
    fn taken(&self, state: State) -> bool {
        arm::condition_passes(self.condition, state.cpsr)
    }
    fn length(&self) -> u64 {
        self.body.len() as u64 + 1
    }
    /// A single unit-step counter, followed only by a fixed comparison.
    /// Restricting the algebra keeps its first-exit proof small and auditable.
    fn counter(&self, state: State) -> Option<(usize, i64, u32)> {
        let (&first, rest) = self.body.split_first()?;
        let Op::Data {
            opcode,
            flags,
            dst,
            lhs,
            rhs: Operand::Immediate(1, _),
        } = first
        else {
            return None;
        };
        if dst != lhs || !matches!(opcode, 2 | 4) {
            return None;
        }
        let compare = match rest {
            [] if flags => 0,
            [
                Op::Data {
                    opcode: 10,
                    lhs,
                    rhs,
                    ..
                },
            ] if *lhs == dst => match *rhs {
                Operand::Immediate(value, _) => value,
                Operand::Register(r, 0, 0) if r != dst => state.registers[r],
                _ => return None,
            },
            _ => return None,
        };
        matches!(self.condition, 1 | 2 | 3 | 8 | 9).then_some((
            dst,
            if opcode == 2 { -1 } else { 1 },
            compare,
        ))
    }
    /// The supported iteration pattern is determined by decoded instruction
    /// shape, register roles, and branch condition. Register contents only
    /// determine the comparison value at runtime.
    fn has_iteration_model(&self) -> bool {
        let Some((&first, rest)) = self.body.split_first() else {
            return false;
        };
        let Op::Data {
            opcode,
            flags,
            dst,
            lhs,
            rhs: Operand::Immediate(1, _),
            ..
        } = first
        else {
            return false;
        };
        if dst != lhs || !matches!(opcode, 2 | 4) {
            return false;
        }
        (match rest {
            [] => flags,
            [Op::Data {
                opcode: 10,
                lhs,
                rhs,
                ..
            }] if *lhs == dst => match *rhs {
                Operand::Immediate(_, _) => true,
                Operand::Register(r, 0, 0) => r != dst,
                _ => false,
            },
            _ => false,
        }) && matches!(self.condition, 1 | 2 | 3 | 8 | 9)
    }
    fn advance(
        &self,
        state: State,
        cycles: u64,
        mut read: impl FnMut(u32, usize) -> Option<u32>,
    ) -> Result<(State, u64, bool), AdvanceFailure> {
        if cycles == 0 {
            return Err(AdvanceFailure::BudgetRoom);
        }
        if !self.taken(state) {
            return Err(AdvanceFailure::ConditionNotTaken);
        }
        let next = self
            .apply(state, &mut read)
            .ok_or(AdvanceFailure::ReadUnavailable)?;
        // Fixed point of the complete architectural state. The read callback
        // proves that every addressed byte is stable for this entire window.
        if next == state {
            return Ok((state, cycles, false));
        }
        if !self.has_iteration_model() {
            return Err(AdvanceFailure::IterationModelUnsupported);
        }
        let (counter, step, compare) = self
            .counter(state)
            .ok_or(AdvanceFailure::CounterAtBoundary)?;
        let value = state.registers[counter];
        let distance = if step < 0 {
            u64::from(value)
        } else {
            u64::from(u32::MAX - value)
        };
        let mut limit = cycles.min(distance);
        // NE can become false at equality and true again afterwards.
        if self.condition == 1 {
            let equal = if step < 0 {
                value.checked_sub(compare)
            } else {
                compare.checked_sub(value)
            };
            if let Some(equal) = equal.filter(|&n| n != 0) {
                limit = limit.min(u64::from(equal));
            }
        }
        if limit == 0 {
            return Err(AdvanceFailure::CounterAtBoundary);
        }
        let at = |n: u64| {
            let mut before = state;
            before.registers[counter] = (i64::from(value) + step * (n as i64 - 1)) as u32;
            self.apply(before, |_, _| None).unwrap()
        };
        // Unsigned comparisons are monotonic between the no-wrap boundaries.
        if !self.taken(at(limit)) {
            let (mut low, mut high) = (1, limit);
            while low < high {
                let mid = low + (high - low) / 2;
                if self.taken(at(mid)) {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            limit = low;
        }
        Ok((at(limit), limit, true))
    }
}

fn bytes_to_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
        .collect()
}

impl Execution {
    pub(super) fn loop_candidate(&self, address: u64, word: u32) -> bool {
        if self.policy.get() != ExecutionPolicy::Exact
            || self.loops.mode.get() == LoopMode::Disabled
        {
            return false;
        }
        let Some((start, bytes)) = self.loop_bytes(address, word) else {
            return false;
        };
        if self
            .loops
            .structural_failures
            .borrow()
            .get(&address)
            .is_some_and(|cached| *cached == bytes)
        {
            return false;
        }
        if self
            .loops
            .unsupported_models
            .borrow()
            .get(&address)
            .is_some_and(|cached| *cached == bytes)
        {
            return false;
        }
        self.loops.structural_failures.borrow_mut().remove(&address);
        self.loops
            .proofs_attempted
            .set(self.loops.proofs_attempted.get().saturating_add(1));
        if Proof::decode(start, address, &bytes_to_words(&bytes)).is_some() {
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
    fn loop_bytes(&self, address: u64, word: u32) -> Option<(u64, Vec<u8>)> {
        let start = target(address, word)?;
        let mut bytes = vec![0; (address - start + 4) as usize];
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
        if width == 4 {
            let physical = self.cpu.translate(u64::from(address), Access::Read).ok()?;
            return self.loop_reads.borrow().iter().find_map(|(base, read)| {
                (physical == base + read.register_offset)
                    .then(|| read.read(read.register_offset, 4))
                    .flatten()
                    .map(|value| value as u32)
            });
        }
        None
    }
    fn loop_runtime_failure(&self, counter: &Cell<u64>) {
        counter.set(counter.get().saturating_add(1));
    }
    pub(super) fn execute_loop(&self, address: u64, word: u32) -> Result<bool, String> {
        if self.loops.mode.get() == LoopMode::Disabled
            || target(address, word).is_none()
            || self.thumb.get()
            || self.policy.get() != ExecutionPolicy::Exact
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
        let start = target(address, word).unwrap();
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
        let Some((start, bytes)) = self.loop_bytes(address, word) else {
            self.loop_runtime_failure(&self.loops.proofs_failed_loop_bytes_unavailable);
            return Ok(false);
        };
        let Some(proof) = Proof::decode(start, address, &bytes_to_words(&bytes)) else {
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
        if proof.body.iter().any(|op| matches!(op, Op::Load { .. }))
            && !self.observer_index.borrow().reads.is_empty()
        {
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
        for (r, value) in state.registers.iter_mut().enumerate() {
            *value = self.scalar(arm::CORE[r])?;
        }
        let room = self.cap.get().saturating_sub(self.count.get() - 1);
        let (after, cycles, counted) =
            match proof.advance(state, (room / proof.length()).max(1), |p, n| {
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
        self.loops.bypass.set(Some(proof.branch));
        self.set(arm::PC, proof.branch as u32)?;
        self.cpu.stop()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
