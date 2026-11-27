//! Run-limit bounded fast-forward of side-effect-free ARM loops.
use super::{Core, ffi};
use fujimod_emulator_cpu::{
    Access, Cpu, Hook, arm, armv7a,
    loop_proof::{self, Isa, Proof, RegisterLoop, State},
    loops::LoopMode,
};
use std::cell::{Cell, RefCell};

const PROOF_CACHE_SLOTS: usize = 16;

#[derive(Default)]
pub(super) struct Loops {
    pub stable: Cell<bool>,
    pub mode: Cell<LoopMode>,
    pub skipped: Cell<u64>,
    proofs: [RefCell<Option<DecodedLoop>>; PROOF_CACHE_SLOTS],
}

enum Advance {
    Counter(RegisterLoop),
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
        let size = (address - start) as usize + 4;
        let mut bytes = [0; 64];
        if self
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
            let advance = if word >> 28 == 10 {
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
        self.last_progress.set(self.last_progress.get() + skipped);
        self.loops
            .skipped
            .set(self.loops.skipped.get().saturating_add(skipped));
        Ok(true)
    }
}
