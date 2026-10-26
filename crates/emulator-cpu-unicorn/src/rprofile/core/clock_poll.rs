//! A32 PMCCNTR; CMP; BLO loops over the processor's explicit clock.
//! No device register, program identity, or firmware address participates.
use super::*;
use fujimod_emulator_cpu::loops::LoopMode;

impl Execution {
    pub(super) fn clock_poll(&self, address: u64, word: u32, cpsr: u32) -> Result<bool, String> {
        if word & 0xffff_0fff != 0xee19_0f1d
            || address > u64::from(u32::MAX) - 11
            || self.loops.mode.get() != LoopMode::Accelerate
            || !self.loops.window.get()
            || self.memory.flush_pending()
            || self.pause_requested.load(Ordering::Acquire) != 0
            || cpsr & ((1 << 9) | (1 << 24)) != 0
            || self.irq.get() && cpsr & (1 << 7) == 0
        {
            return Ok(false);
        }
        let dst = ((word >> 12) & 15) as usize;
        if dst == 15 {
            return Ok(false);
        }
        let mut bytes = [0; 8];
        if self
            .cpu
            .read_virtual(address + 4, Access::Fetch, &mut bytes)
            .is_err()
        {
            return Ok(false);
        }
        let compare = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        let branch = u32::from_le_bytes(bytes[4..].try_into().unwrap());
        // AL CMP of the counter register, then unsigned-lower back to MRC.
        if branch != 0x3aff_fffc || (compare >> 16) & 15 != dst as u32 {
            return Ok(false);
        }
        let limit = if compare & 0xfff0_f000 == 0xe350_0000 {
            (compare & 255).rotate_right(((compare >> 8) & 15) * 2)
        } else if compare & 0xfff0_fff0 == 0xe150_0000 {
            let rhs = (compare & 15) as usize;
            if rhs == 15 || rhs == dst {
                return Ok(false);
            }
            self.scalar(arm::CORE[rhs])?
        } else {
            return Ok(false);
        };
        let observers = self.observer_index.borrow();
        if observers.instructions.any_in(address, address + 11)
            || observers.blocks.any_in(address, address + 11)
        {
            return Ok(false);
        }
        drop(observers);
        let Some(clock) = self.clock.get() else {
            return Ok(false);
        };
        // The MRC has already been admitted by instruction accounting. Each
        // skipped iteration includes that read, its CMP and its branch.
        let current = self.count.get();
        let baseline = current
            .checked_sub(1)
            .ok_or("clock poll has no admitted read")?;
        let room = self.cap.get().saturating_sub(baseline);
        let maximum = room / 3;
        if maximum < 2 {
            return Ok(false);
        }
        let counter = self.pmu.get();
        let at = |iteration: u64| -> Result<(u64, u64), String> {
            let now = clock.after(current + iteration * 3)?.now;
            Ok((now, counter.count(now)))
        };
        let (_, first) = at(0)?;
        let (_, last) = at(maximum - 1)?;
        // A wrap can make the predicate true again. Leave such windows to
        // ordinary execution; all accepted reads are monotonic in one epoch.
        if last < first || first >> 32 != last >> 32 || first as u32 >= limit {
            return Ok(false);
        }
        let mut iterations = maximum;
        let exits = last as u32 >= limit;
        if exits {
            let (mut low, mut high) = (0, maximum - 1);
            while low < high {
                let middle = low + (high - low) / 2;
                if at(middle)?.1 as u32 >= limit {
                    high = middle;
                } else {
                    low = middle + 1;
                }
            }
            iterations = low + 1;
        }
        let (now, _) = at(iterations - 1)?;
        let mut counter = counter;
        let value = counter.access(13, 0, None, now)?;
        let result = value.wrapping_sub(limit);
        let flags = (cpsr & 0x0fff_ffff)
            | (result & 0x8000_0000)
            | (u32::from(result == 0) << 30)
            | (u32::from(value >= limit) << 29)
            | (((value ^ limit) & (value ^ result)) & 0x8000_0000) >> 3;
        self.set(arm::CORE[dst], value)?;
        self.set(arm::CPSR, flags)?;
        self.pmu.set(counter);
        let skipped = iterations * 3;
        self.count.set(baseline + skipped);
        self.loops.clock_poll_instructions.set(
            self.loops
                .clock_poll_instructions
                .get()
                .saturating_add(skipped),
        );
        self.loops.accelerated_instructions.set(
            self.loops
                .accelerated_instructions
                .get()
                .saturating_add(skipped),
        );
        self.reset_budget();
        self.set(arm::PC, (address + if exits { 12 } else { 0 }) as u32)?;
        self.loops.resume.set(true);
        self.cpu.stop()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
