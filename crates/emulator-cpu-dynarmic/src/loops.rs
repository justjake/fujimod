//! Run-limit bounded fast-forward of ARM loops.
use super::{Core, NativeAccess, ffi};
use fujimod_emulator_cpu::{
    Access, Cpu, Hook, arm, armv7a,
    loop_proof::{self, Isa, Proof, RegisterLoop, ShiftCountLoop, State, WordFillLoop},
    loops::LoopMode,
};
use std::cell::{Cell, RefCell};

const PROOF_CACHE_SLOTS: usize = 16;

/// A direct A32 B to its own address. Its condition is evaluated by the CPU.
pub(super) fn is_self_branch(word: u32) -> bool {
    word >> 28 != 15 && word & 0x0fff_ffff == 0x0aff_fffe
}

pub(super) fn is_shift_count_branch(word: u32) -> bool {
    word == 0x8aff_fffb // BHI to the head of a four-instruction loop.
}

pub(super) fn is_word_fill_branch(word: u32) -> bool {
    word == 0x1aff_fffc // BNE to the head of a three-instruction loop.
}

pub(super) extern "C" fn is_counted_loop(branch: u32, first: u32, second: u32, third: u32) -> bool {
    if is_word_fill_branch(branch) {
        WordFillLoop::decode([first, second]).is_some()
    } else {
        is_shift_count_branch(branch) && ShiftCountLoop::decode([first, second, third]).is_some()
    }
}

#[derive(Default)]
pub(super) struct Loops {
    pub stable: Cell<bool>,
    pub mode: Cell<LoopMode>,
    pub skipped: Cell<u64>,
    proofs: [RefCell<Option<DecodedLoop>>; PROOF_CACHE_SLOTS],
}

enum Advance {
    Counter(RegisterLoop),
    ShiftCount(ShiftCountLoop),
    WordFill(WordFillLoop),
    General(Proof),
}

struct DecodedLoop {
    address: u64,
    privileged: bool,
    bytes: [u8; 64],
    advance: Option<Advance>,
}

impl Core {
    pub(super) fn advance_loop(
        &self,
        pc: u32,
        cpsr: u32,
        word: u32,
        remaining: u64,
        native_pages: bool,
    ) -> Result<bool, String> {
        if self.shared.device_accessed.get()
            || self.loops.mode.get() != LoopMode::Accelerate
            || cpsr & 0x0700_fe20 != 0
            || !arm::condition_passes(word >> 28, cpsr)
        {
            return Ok(false);
        }
        let address = u64::from(pc);
        let Some(start) = loop_proof::loop_start(Isa::A32, address, word) else {
            return Ok(false);
        };
        let observer = self.scheduler_hook.get().map(|hook| hook.key);
        if self.shared.observers.borrow().iter().any(|&(id, hook)| {
            Some(id) != observer
                && matches!(
                    hook,
                    Hook::Instruction { .. } | Hook::Memory { read: true, .. }
                )
        }) {
            return Ok(false);
        }
        if is_self_branch(word) {
            // The taken branch changes neither registers nor flags. The
            // owner has bounded the window before the next input can change.
            self.charge_loop(remaining);
            return Ok(true);
        }
        let size = (address - start) as usize + 4;
        let mut bytes = [0; 64];
        if native_pages
            && let Some(NativeAccess::Ram(pointer, _)) =
                self.shared.native_access(start as u32, size, Access::Fetch)
        {
            // A checked native page already covers the whole loop in RAM.
            unsafe { std::ptr::copy_nonoverlapping(pointer, bytes.as_mut_ptr(), size) };
        } else if self
            .read_virtual(start, Access::Fetch, &mut bytes[..size])
            .is_err()
        {
            return Ok(false);
        }
        let privileged = cpsr & 31 != 0x10;
        // Recheck the actual instruction bytes even when another core or DMA
        // can write the backing. Only the decode is reused.
        let index = (pc as usize >> 2) & (PROOF_CACHE_SLOTS - 1);
        let mut entry = self.loops.proofs[index].borrow_mut();
        if !entry.as_ref().is_some_and(|entry| {
            entry.address == address && entry.privileged == privileged && entry.bytes == bytes
        }) {
            let advance = if is_shift_count_branch(word) {
                let (words, _) = bytes[..size].as_chunks::<4>();
                let body = std::array::from_fn(|index| u32::from_le_bytes(words[index]));
                ShiftCountLoop::decode(body).map(Advance::ShiftCount)
            } else if is_word_fill_branch(word) {
                let (words, _) = bytes[..size].as_chunks::<4>();
                let body = std::array::from_fn(|index| u32::from_le_bytes(words[index]));
                WordFillLoop::decode(body).map(Advance::WordFill)
            } else if word >> 28 == 10 {
                RegisterLoop::decode(start, &bytes[..size], |word| {
                    privileged && armv7a::data_cache_set_way(word)
                })
                .map(Advance::Counter)
            } else {
                Proof::decode(Isa::A32, start, address, &bytes[..size]).map(Advance::General)
            };
            *entry = Some(DecodedLoop {
                address,
                privileged,
                bytes,
                advance,
            });
        }
        let Some(advance) = entry.as_ref().and_then(|entry| entry.advance.as_ref()) else {
            return Ok(false);
        };
        let mut registers = [0; 15];
        unsafe { ffi::dynarmic_read_gprs(self.inst, registers.as_mut_ptr()) };
        let before = State { registers, cpsr };
        let (after, skipped) = match advance {
            Advance::Counter(proof) => {
                let Some(advanced) = proof.advance(before, remaining) else {
                    return Ok(false);
                };
                advanced
            }
            Advance::ShiftCount(proof) => {
                let Some(advanced) = proof.advance(before, remaining) else {
                    return Ok(false);
                };
                advanced
            }
            Advance::WordFill(proof) => {
                if !native_pages {
                    return Ok(false);
                }
                let Some(advanced) = self.advance_word_fill(proof, before, remaining) else {
                    return Ok(false);
                };
                advanced
            }
            Advance::General(proof) => {
                // Observational reads admit RAM only. A device read may change state.
                let rounds = remaining / proof.length();
                let Ok(advanced) = proof.advance(before, rounds, |address, width| {
                    if !(address as usize).is_multiple_of(width) {
                        return None;
                    }
                    let mut bytes = [0; 4];
                    self.read_virtual(address.into(), Access::Read, &mut bytes[..width])
                        .ok()?;
                    Some(u32::from_le_bytes(bytes))
                }) else {
                    return Ok(false);
                };
                (advanced.state, advanced.iterations * proof.length())
            }
        };
        if skipped == 0 || skipped > remaining {
            return Ok(false);
        }
        for (reg, value) in after.registers.into_iter().enumerate() {
            if value != before.registers[reg] {
                unsafe { ffi::dynarmic_set_reg(self.inst, reg as u32, value) };
            }
        }
        if after.cpsr != cpsr {
            self.set_cpsr(after.cpsr)?;
        }
        self.charge_loop(skipped);
        Ok(true)
    }

    fn charge_loop(&self, skipped: u64) {
        self.last_progress.set(self.last_progress.get() + skipped);
        self.loops
            .skipped
            .set(self.loops.skipped.get().saturating_add(skipped));
    }

    fn advance_word_fill(
        &self,
        proof: &WordFillLoop,
        before: State,
        remaining: u64,
    ) -> Option<(State, u64)> {
        let address = proof.address(&before);
        if address & 3 != 0 || self.shared.observe_exclusive_stores.get() {
            return None;
        }
        let page = self.shared.inline_page(address);
        if page.is_null() || page.addr() & 1 != 0 {
            return None;
        }
        let offset = (address & 4095) as usize;
        let limit = ((4096 - offset) / 4) as u32;
        let advanced = proof.advance(before, remaining, limit)?;
        // A writable inline entry already covers the whole RAM page, has
        // marked it dirty, and excludes watched code and translation tables.
        // The owner's exact window ends before another core or device runs.
        let words = unsafe { page.add(offset).cast::<u32>() };
        let value = advanced.value.to_le();
        for index in 0..advanced.words {
            unsafe { words.add(index).write(value) };
        }
        Some((advanced.state, advanced.entries))
    }
}
