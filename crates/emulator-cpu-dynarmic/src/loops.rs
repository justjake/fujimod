//! Run-limit bounded fast-forward of side-effect-free ARM loops.
use super::{Core, NativeAccess, ffi};
use fujimod_emulator_cpu::{
    Access, Cpu, Hook, arm, armv7a,
    loop_proof::{self, Isa, Proof, RegisterLoop, ShiftCountLoop, State},
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

pub(super) extern "C" fn is_shift_count_loop(shift: u32, increment: u32, compare: u32) -> bool {
    ShiftCountLoop::decode([shift, increment, compare]).is_some()
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
        cached_fetch: bool,
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
        if cached_fetch
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
}
