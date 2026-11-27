//! Run-limit bounded fast-forward of side-effect-free ARM loops.
use super::{Core, ffi};
use fujimod_emulator_cpu::{
    Access, Cpu, Hook, armv7a,
    loop_proof::{self, Isa, Proof, RegisterLoop, State},
    loops::LoopMode,
};
use std::cell::Cell;

#[derive(Default)]
pub(super) struct Loops {
    pub stable: Cell<bool>,
    pub mode: Cell<LoopMode>,
    pub skipped: Cell<u64>,
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
        let bytes = &mut bytes[..size];
        if self.read_virtual(start, Access::Fetch, bytes).is_err() {
            return Ok(false);
        }
        // A device read can mutate state even if successive values agree. The
        // observational read API admits RAM only.
        let state = || State {
            registers: std::array::from_fn(|reg| unsafe {
                ffi::dynarmic_get_reg(self.inst, reg as u32)
            }),
            cpsr,
        };
        let (before, after, skipped) = if word >> 28 == 10 {
            let Some(proof) = RegisterLoop::decode(start, bytes, |word| {
                cpsr & 31 != 0x10 && armv7a::data_cache_set_way(word)
            }) else {
                return Ok(false);
            };
            let before = state();
            let Some((after, skipped)) = proof.advance(before, remaining) else {
                return Ok(false);
            };
            (before, after, skipped)
        } else {
            let Some(proof) = Proof::decode(Isa::A32, start, address, bytes) else {
                return Ok(false);
            };
            let before = state();
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
            (before, advanced.state, advanced.iterations * proof.length())
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
