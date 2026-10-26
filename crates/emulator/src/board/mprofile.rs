//! M-profile architectural state on the board timeline. No firmware policy.
#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod exclusive_tests;
use crate::architecture::mprofile;
use crate::architecture::mprofile::nvic::Nvic;
use fujimod_emulator_cpu::{Access, Cpu, Register, arm, exclusive};
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub clock_hz: u64,
    pub board_ticks_per_second: u64,
    pub vector_base: u32,
}
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryView {
    pub address: u64,
    pub physical: u64,
    pub size: usize,
}
pub(super) struct Profile {
    config: Config,
    pub nvic: Nvic,
    pub levels: [u32; 8],
    pub needs_reset: bool,
    dwt_control: u32,
    dwt_frozen: u32,
    dwt_anchor: u64,
    domain: exclusive::Shared,
    owner: exclusive::Owner,
    views: Vec<MemoryView>,
    alias_write: bool,
}
pub(super) enum Entry {
    Execute,
    Redirect,
    Emulated,
    Wait,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Checkpoint {
    config: Config,
    views: Vec<MemoryView>,
    nvic: Nvic,
    levels: [u32; 8],
    needs_reset: bool,
    dwt_control: u32,
    dwt_frozen: u32,
    dwt_anchor: u64,
    alias_write: bool,
}
impl Drop for Profile {
    fn drop(&mut self) {
        self.domain
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unregister(self.owner);
    }
}
fn special(r: arm::Register) -> Register {
    Register::Arm32(r)
}
fn reg(n: u8) -> Register {
    special(arm::Register::R(n))
}
fn get(cpu: &dyn Cpu, r: Register) -> Result<u32, String> {
    super::scalar(cpu, r).and_then(|v| {
        u32::try_from(v).map_err(|_| "M-profile register width exceeds 32 bits".into())
    })
}
fn set(cpu: &dyn Cpu, r: Register, v: u32) -> Result<(), String> {
    super::set(cpu, r, v as u64)
}
impl Profile {
    pub(super) fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        if saved.config != self.config
            || saved.views != self.views
            || saved.nvic.clock_hz != self.nvic.clock_hz
            || saved.nvic.enabled.len() != self.nvic.enabled.len()
            || saved.nvic.pending.len() != self.nvic.pending.len()
            || saved.nvic.priority.len() != self.nvic.priority.len()
            || saved
                .nvic
                .active
                .iter()
                .any(|id| usize::from(*id) >= saved.nvic.priority.len())
        {
            return Err("M-profile snapshot configuration/state differs".into());
        }
        Ok(())
    }
    pub(super) fn checkpoint(&self) -> Checkpoint {
        let Self {
            config,
            nvic,
            levels,
            needs_reset,
            dwt_control,
            dwt_frozen,
            dwt_anchor,
            domain: _,
            owner: _,
            views,
            alias_write,
        } = self;
        Checkpoint {
            config: *config,
            views: views.clone(),
            nvic: nvic.clone(),
            levels: *levels,
            needs_reset: *needs_reset,
            dwt_control: *dwt_control,
            dwt_frozen: *dwt_frozen,
            dwt_anchor: *dwt_anchor,
            alias_write: *alias_write,
        }
    }
    pub(super) fn restore_checkpoint(&mut self, saved: &Checkpoint) {
        self.nvic.clone_from(&saved.nvic);
        self.levels = saved.levels;
        self.needs_reset = saved.needs_reset;
        self.dwt_control = saved.dwt_control;
        self.dwt_frozen = saved.dwt_frozen;
        self.dwt_anchor = saved.dwt_anchor;
        self.alias_write = saved.alias_write;
    }
    pub fn new(
        config: Config,
        domain: exclusive::Shared,
        views: Vec<MemoryView>,
    ) -> Result<Self, String> {
        if config.clock_hz == 0
            || config.clock_hz > 1_000_000_000
            || config.board_ticks_per_second < config.clock_hz
            || config.vector_base & 127 != 0
        {
            return Err("invalid M-profile clock/vector configuration".into());
        }
        if views.is_empty()
            || views.iter().any(|v| {
                v.size == 0
                    || v.address
                        .checked_add(v.size as u64)
                        .is_none_or(|end| end > 1u64 << 32)
                    || v.physical
                        .checked_add(v.size as u64)
                        .is_none_or(|end| end > 1u64 << 32)
            })
        {
            return Err("invalid M-profile physical RAM views".into());
        }
        for (i, a) in views.iter().enumerate() {
            if views[..i].iter().any(|b| {
                a.address < b.address + b.size as u64 && b.address < a.address + a.size as u64
            }) {
                return Err("overlapping M-profile RAM views".into());
            }
        }
        let owner = domain
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .register(64)?;
        Ok(Self {
            config,
            nvic: Nvic::new(config.clock_hz),
            levels: [0; 8],
            needs_reset: true,
            dwt_control: 0,
            dwt_frozen: 0,
            dwt_anchor: 0,
            domain,
            owner,
            views,
            alias_write: false,
        })
    }
    fn ns(&self, now: u64) -> Result<u64, String> {
        u64::try_from(now as u128 * 1_000_000_000 / self.config.board_ticks_per_second as u128)
            .map_err(|_| "M-profile virtual time exceeds nanosecond range".into())
    }
    fn cycles(&self, now: u64) -> u64 {
        (now as u128 * self.config.clock_hz as u128 / self.config.board_ticks_per_second as u128)
            as u64
    }
    fn traced(&self) -> bool {
        self.dwt_control & 1 != 0
            && self.nvic.regs.get(&0xdfc).copied().unwrap_or(0) & (1 << 24) != 0
    }
    fn counter(&self, now: u64) -> u32 {
        if self.traced() {
            self.dwt_frozen
                .wrapping_add(self.cycles(now).wrapping_sub(self.dwt_anchor) as u32)
        } else {
            self.dwt_frozen
        }
    }
    pub(super) fn physical(&self, address: u64, size: usize) -> Result<u64, String> {
        let end = address
            .checked_add(size as u64)
            .ok_or("M-profile RAM range overflow")?;
        self.views
            .iter()
            .find_map(|v| {
                (address >= v.address && end <= v.address + v.size as u64)
                    .then(|| v.physical + address - v.address)
            })
            .ok_or("M-profile exclusive/write is outside checked RAM views".into())
    }
    pub(super) fn clear_exclusive(&self) -> Result<(), String> {
        self.domain
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .clear(self.owner);
        Ok(())
    }
    pub(super) fn writes_an_alias(&self, address: u64, size: usize) -> bool {
        let Ok(physical) = self.physical(address, size) else {
            return false;
        };
        self.views.iter().any(|v| {
            physical >= v.physical
                && physical + size as u64 <= v.physical + v.size as u64
                && v.address + physical - v.physical != address
        })
    }
    pub(super) fn take_alias_write(&mut self) -> bool {
        std::mem::take(&mut self.alias_write)
    }
    fn enter(&mut self, cpu: &dyn Cpu, exception: u16, pc: u32) -> Result<(), String> {
        mprofile::enter(cpu, &mut self.nvic, exception, pc)?;
        self.clear_exclusive()?;
        let stack = if get(cpu, reg(14))? & 4 != 0 {
            arm::Register::Psp
        } else {
            arm::Register::Msp
        };
        let stack = get(cpu, special(stack))? as u64;
        self.alias_write |= self.writes_an_alias(stack, 32);
        let address = self.physical(stack, 32)?;
        self.domain
            .lock()
            .map_err(|_| "exclusive monitors poisoned")?
            .written(Some((address, 32)))
    }
    fn exclusive(
        &mut self,
        cpu: &dyn Cpu,
        pc: u64,
        instruction: arm::thumb::exclusive::Instruction,
    ) -> Result<(), String> {
        use arm::thumb::exclusive::Instruction;
        match instruction {
            Instruction::Clear => self.clear_exclusive()?,
            Instruction::Transfer {
                load,
                base,
                data,
                status,
                bytes,
                offset,
            } => {
                let address = get(cpu, reg(base))?.wrapping_add(offset as u32) as u64;
                let size = bytes as usize;
                if !address.is_multiple_of(size as u64) {
                    return Err("unaligned M-profile exclusive".into());
                }
                let physical = self.physical(address, size)?;
                let mut value = [0; 4];
                // This checks permissions and refuses MMIO without driving a device.
                cpu.read_virtual(
                    address,
                    if load { Access::Read } else { Access::Write },
                    &mut value[..size],
                )
                .map_err(|e| {
                    format!("M-profile exclusive at {pc:#x}, address {address:#x}: {e}")
                })?;
                let mut domain = self
                    .domain
                    .lock()
                    .map_err(|_| "exclusive monitors poisoned")?;
                if load {
                    domain.load(self.owner, physical, size);
                    set(cpu, reg(data), u32::from_le_bytes(value))?;
                } else {
                    let success = domain.store(self.owner, physical, size);
                    if success {
                        cpu.write_memory(address, &get(cpu, reg(data))?.to_le_bytes()[..size])?;
                        self.alias_write |= self.writes_an_alias(address, size);
                        domain.written(Some((physical, size)))?;
                    }
                    set(cpu, reg(status), u32::from(!success))?;
                }
            }
        }
        let cpsr = get(cpu, special(arm::Register::Cpsr))?;
        set(
            cpu,
            special(arm::Register::Cpsr),
            arm::thumb::advance_it(cpsr),
        )?;
        set(cpu, Register::Pc, (pc as u32).wrapping_add(4) | 1)?;
        cpu.stop()?;
        Ok(())
    }
    pub fn reset(&mut self, cpu: &dyn Cpu) -> Result<(), String> {
        self.alias_write = false;
        let mut vectors = [0; 8];
        cpu.read_memory(self.config.vector_base as u64, &mut vectors)?;
        let sp = u32::from_le_bytes(vectors[..4].try_into().unwrap());
        let pc = u32::from_le_bytes(vectors[4..].try_into().unwrap());
        if sp & 3 != 0 || pc & 1 == 0 {
            return Err("M-profile reset vectors require aligned SP and Thumb entry".into());
        }
        self.nvic = Nvic::new(self.config.clock_hz);
        self.nvic.vtor = self.config.vector_base;
        self.dwt_control = 0;
        self.dwt_frozen = 0;
        self.dwt_anchor = 0;
        for r in [
            arm::Register::Control,
            arm::Register::Primask,
            arm::Register::Basepri,
            arm::Register::Faultmask,
            arm::Register::Psp,
        ] {
            set(cpu, special(r), 0)?;
        }
        set(cpu, special(arm::Register::Cpsr), 0x20)?;
        set(cpu, special(arm::Register::Xpsr), 1 << 24)?;
        set(cpu, special(arm::Register::Msp), sp)?;
        set(cpu, Register::Sp, sp)?;
        set(cpu, reg(14), u32::MAX)?;
        set(cpu, Register::Pc, pc)?;
        self.clear_exclusive()?;
        self.needs_reset = false;
        Ok(())
    }
    pub fn next_deadline(&self) -> Result<Option<u64>, String> {
        if self.needs_reset || self.nvic.control & 1 == 0 {
            return Ok(None);
        }
        let at = (self.nvic.deadline as u128 * self.config.board_ticks_per_second as u128)
            .div_ceil(1_000_000_000);
        Ok(Some(u64::try_from(at).map_err(
            |_| "M-profile deadline exceeds board timeline",
        )?))
    }
    pub fn tick(&mut self, now: u64) -> Result<(), String> {
        self.nvic.tick(self.ns(now)?);
        Ok(())
    }
    pub fn level(&mut self, line: u32, high: bool) -> Result<(), String> {
        if line >= 256 {
            return Err("NVIC input exceeds supported external lines".into());
        }
        let mask = 1 << (line % 32);
        if high {
            self.levels[line as usize / 32] |= mask;
        } else {
            self.levels[line as usize / 32] &= !mask;
        }
        Ok(())
    }
    pub fn pulse(&mut self, line: u32) -> Result<(), String> {
        if line >= 256 {
            return Err("NVIC pulse exceeds supported external lines".into());
        }
        if !self.needs_reset {
            self.nvic.pend(line as u16);
        }
        Ok(())
    }
    pub fn pending(&mut self) -> bool {
        for (bank, levels) in self.levels.into_iter().enumerate() {
            let mut bits = levels;
            while bits != 0 {
                let irq = (bank * 32 + bits.trailing_zeros() as usize) as u16;
                if !self.nvic.active.contains(&(irq + 16)) {
                    self.nvic.pend(irq);
                }
                bits &= bits - 1;
            }
        }
        self.nvic.wake_pending()
    }
    pub fn before(&mut self, cpu: &dyn Cpu, pc: u64) -> Result<Entry, String> {
        self.pending();
        if let Some(exception) = self.nvic.next(
            get(cpu, special(arm::Register::Primask))?,
            get(cpu, special(arm::Register::Basepri))?,
            get(cpu, special(arm::Register::Faultmask))?,
        ) {
            self.enter(
                cpu,
                exception,
                u32::try_from(pc).map_err(|_| "M-profile PC width")?,
            )?;
            return Ok(Entry::Redirect);
        }
        let mut bytes = [0; 2];
        cpu.read_memory(pc, &mut bytes)?;
        let word = u16::from_le_bytes(bytes);
        let cpsr = get(cpu, special(arm::Register::Cpsr))?;
        if !arm::thumb::condition_passed(cpsr) {
            return Ok(Entry::Execute);
        }
        let mut handled = true;
        if word == 0xbf30 {
            set(
                cpu,
                special(arm::Register::Cpsr),
                arm::thumb::advance_it(cpsr),
            )?;
            set(cpu, Register::Pc, (pc as u32).wrapping_add(2) | 1)?;
            cpu.stop()?;
            return Ok(Entry::Wait);
        } else if word == 0xbf20 {
            return Err("M-profile WFE event-register emulation is not installed".into());
        } else if word & 0xff00 == 0xdf00 {
            set(
                cpu,
                special(arm::Register::Cpsr),
                arm::thumb::advance_it(cpsr),
            )?;
            self.enter(cpu, 11, (pc as u32).wrapping_add(2))?;
        } else if word == 0xf3ef {
            cpu.read_memory(pc + 2, &mut bytes)?;
            let suffix = u16::from_le_bytes(bytes);
            if matches!(suffix & 255, 1 | 3 | 5 | 7) {
                let rt = ((suffix >> 8) & 15) as u8;
                if rt == 15 {
                    return Err("unpredictable M-profile MRS destination".into());
                }
                let mask = if suffix & 4 == 0 { 0xf80f0000 } else { 0 };
                set(
                    cpu,
                    reg(rt),
                    mprofile::exception_status(cpu, &self.nvic)? & (mask | 511),
                )?;
                set(
                    cpu,
                    special(arm::Register::Cpsr),
                    arm::thumb::advance_it(cpsr),
                )?;
                set(cpu, Register::Pc, (pc as u32).wrapping_add(4) | 1)?;
            } else {
                handled = false;
            }
        } else if word & 0xff87 == 0x4700 {
            let target = get(cpu, reg(((word >> 3) & 15) as u8))?;
            if target >= 0xffffffe0 {
                mprofile::exception_return(cpu, &mut self.nvic, target)?;
                self.clear_exclusive()?;
            } else {
                handled = false;
            }
        } else if matches!(word & 0xfff0, 0xe850 | 0xe840 | 0xe8d0 | 0xe8c0) || word == 0xf3bf {
            cpu.read_memory(pc + 2, &mut bytes)?;
            let suffix = u16::from_le_bytes(bytes);
            if let Some(instruction) = arm::thumb::exclusive::decode(word, suffix)? {
                self.exclusive(cpu, pc, instruction)?;
            } else {
                handled = false;
            }
        } else {
            handled = false;
        }
        Ok(if handled {
            Entry::Emulated
        } else {
            Entry::Execute
        })
    }
    pub fn access(
        &mut self,
        cpu: &dyn Cpu,
        address: u64,
        size: u8,
        value: Option<u64>,
        now: u64,
    ) -> Result<u64, String> {
        let ns = self.ns(now)?;
        if (0xe000e000..0xe000f000).contains(&address) {
            let offset = address - 0xe000e000;
            if let Some(v) = value {
                let counter = self.counter(now);
                if offset == 0xd88 {
                    if size != 4 || v & !0x00f00000 != 0 {
                        return Err("unsupported M-profile coprocessor access control".into());
                    }
                    set(cpu, special(arm::Register::Cpacr), v as u32)?;
                }
                self.nvic.write(offset, size as u32, v, ns)?;
                if offset == 0xdfc {
                    self.dwt_frozen = counter;
                    self.dwt_anchor = self.cycles(now);
                }
                return Ok(0);
            }
            return self.nvic.read(offset, size as u32, ns);
        }
        if size == 4 && matches!(address, 0xe0001000 | 0xe0001004) {
            if let Some(v) = value {
                let counter = self.counter(now);
                if address == 0xe0001000 {
                    if v & 0x0ffffffe != 0 {
                        return Err("unsupported DWT counters/trace controls".into());
                    }
                    self.dwt_control = v as u32 & 1;
                    self.dwt_frozen = counter;
                } else {
                    self.dwt_frozen = v as u32;
                }
                self.dwt_anchor = self.cycles(now);
            }
            return Ok(if address == 0xe0001000 {
                self.dwt_control
            } else {
                self.counter(now)
            } as u64);
        }
        Err(format!(
            "unmodeled M-profile system access {address:#x}/{size}"
        ))
    }
    pub fn state(&self, now: u64) -> Value {
        let reservation = self
            .domain
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reservation(self.owner)
            .map(|r| json!({"address":r.address,"bytes":r.size}));
        json!({"exclusive":reservation,"nvic":self.nvic,"irq_levels":self.levels,"needs_reset":self.needs_reset,"clock_hz":self.config.clock_hz,"board_ticks_per_second":self.config.board_ticks_per_second,"dwt_control":self.dwt_control,"dwt_counter":self.counter(now)})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dwt_cycle_counter_is_guest_clocked_gated_and_observational() {
        let lib = fujimod_emulator_cpu_unicorn::Library::linked().unwrap();
        let cpu = lib
            .open(fujimod_emulator_cpu::Model::CortexM4, 1024 * 1024)
            .unwrap();
        let mut p = Profile::new(
            Config {
                clock_hz: 100_000_000,
                board_ticks_per_second: 1_000_000_000,
                vector_base: 0,
            },
            Default::default(),
            vec![MemoryView {
                address: 0,
                physical: 0,
                size: 0x4000,
            }],
        )
        .unwrap();
        p.access(&cpu, 0xe0001000, 4, Some(1), 0).unwrap();
        assert_eq!(p.access(&cpu, 0xe0001004, 4, None, 100).unwrap(), 0);
        p.access(&cpu, 0xe000edfc, 4, Some(1 << 24), 100).unwrap();
        let saved = p.state(150);
        assert_eq!(saved["dwt_counter"], 5);
        assert_eq!(saved, p.state(150));
        p.access(&cpu, 0xe0001004, 4, Some(100), 150).unwrap();
        assert_eq!(p.access(&cpu, 0xe0001004, 4, None, 200).unwrap(), 105);
        p.access(&cpu, 0xe000edfc, 4, Some(0), 200).unwrap();
        assert_eq!(p.access(&cpu, 0xe0001004, 4, None, 1000).unwrap(), 105);
        p.access(&cpu, 0xe000edfc, 4, Some(1 << 24), 1000).unwrap();
        assert_eq!(p.access(&cpu, 0xe0001004, 4, None, 1050).unwrap(), 110);
        p.access(&cpu, 0xe0001000, 4, Some(0), 1050).unwrap();
        assert_eq!(p.access(&cpu, 0xe0001004, 4, None, 10000).unwrap(), 110);
    }
}
