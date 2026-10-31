//! Bounded GICv2 register model. No CPU registers, program text or RTOS state.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gic {
    cores: usize,
    lines: usize,
    control: u32,
    cpu_control: Vec<u32>,
    mask: Vec<u8>,
    binary_point: Vec<u32>,
    enabled: Vec<Vec<bool>>,
    pending: Vec<Vec<bool>>,
    active: Vec<Vec<bool>>,
    priority: Vec<Vec<u8>>,
    targets: Vec<u8>,
    config: Vec<u32>,
    eligible: Vec<Option<usize>>,
    sgi_sources: Vec<[u8; 16]>,
    levels: Vec<bool>,
}
impl Gic {
    pub fn save(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
    pub fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let mut state: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let vectors = [&state.enabled, &state.pending, &state.active];
        if state.cores != self.cores
            || state.lines != self.lines
            || state.control & !1 != 0
            || state.cpu_control.len() != self.cores
            || state.cpu_control.iter().any(|v| v & !1 != 0)
            || state.mask.len() != self.cores
            || state.binary_point.len() != self.cores
            || state.binary_point.iter().any(|v| *v > 7)
            || vectors
                .iter()
                .any(|v| v.len() != self.cores || v.iter().any(|b| b.len() != self.lines))
            || state.priority.len() != self.cores
            || state.priority.iter().any(|b| b.len() != self.lines)
            || state.targets.len() != self.lines
            || state.config.len() != self.lines.div_ceil(16)
            || state.eligible.len() != self.cores
            || state.sgi_sources.len() != self.cores
            || state.levels.len() != self.lines
        {
            return Err("Invalid GIC checkpoint shape/configuration".into());
        }
        let source_mask = (1u8 << self.cores) - 1;
        for core in 0..self.cores {
            for sgi in 0..16 {
                let sources = state.sgi_sources[core][sgi];
                if sources & !source_mask != 0 || state.pending[core][sgi] != (sources != 0) {
                    return Err("GIC checkpoint SGI sources disagree with pending state".into());
                }
            }
        }
        let eligible = state.eligible.clone();
        state.refresh();
        if state.eligible != eligible {
            return Err("GIC checkpoint eligibility differs from its registers".into());
        }
        *self = state;
        Ok(())
    }
    pub fn new(cores: usize, lines: usize) -> Self {
        assert!((1..=4).contains(&cores) && (32..=1020).contains(&lines));
        let mut enabled = vec![vec![false; lines]; cores];
        for bank in &mut enabled {
            bank[..16].fill(true);
        }
        Self {
            cores,
            lines,
            control: 0,
            cpu_control: vec![0; cores],
            mask: vec![0; cores],
            binary_point: vec![0; cores],
            enabled,
            pending: vec![vec![false; lines]; cores],
            active: vec![vec![false; lines]; cores],
            priority: vec![vec![0; lines]; cores],
            targets: vec![1; lines],
            config: vec![0; lines.div_ceil(16)],
            eligible: vec![None; cores],
            sgi_sources: vec![[0; 16]; cores],
            levels: vec![false; lines],
        }
    }
    fn bank(core: usize, line: usize) -> usize {
        if line < 32 { core } else { 0 }
    }
    pub fn set_line(&mut self, line: usize, high: bool) {
        if line >= 32 && line < self.lines {
            let edge = self.config[line / 16] & (2 << (line % 16 * 2)) != 0;
            if !edge || high && !self.levels[line] {
                self.pending[0][line] = high;
            }
            self.levels[line] = high;
        }
        self.refresh();
    }
    pub fn pulse(&mut self, line: usize) {
        if line < self.lines {
            if line >= 32 {
                self.pending[0][line] = true;
            }
        }
        self.refresh();
    }
    pub fn next_irq(&self, core: usize) -> Option<usize> {
        self.eligible[core]
    }
    /// Whether a future peripheral edge can wake a core under the current
    /// distributor/CPU-interface state. This query does not pend the edge.
    pub fn pulse_would_wake(&self, line: usize) -> bool {
        line >= 32
            && line < self.lines
            && (0..self.cores).any(|core| self.choose_irq_with_pulse(core, Some(line)).is_some())
    }
    fn refresh(&mut self) {
        for core in 0..self.cores {
            self.eligible[core] = self.choose_irq(core);
        }
    }
    fn choose_irq(&self, core: usize) -> Option<usize> {
        self.choose_irq_with_pulse(core, None)
    }
    fn choose_irq_with_pulse(&self, core: usize, pulse: Option<usize>) -> Option<usize> {
        if self.control & 1 == 0 || self.cpu_control[core] & 1 == 0 {
            return None;
        }
        let group_mask = (0xffu16 << (self.binary_point[core] + 1)) & 0xff;
        let running = (0..self.lines)
            .filter(|&line| self.active[core][line])
            .map(|line| self.priority[Self::bank(core, line)][line] as u16 & group_mask)
            .min()
            .unwrap_or(256);
        (0..self.lines)
            .filter(|&line| {
                let bank = Self::bank(core, line);
                self.enabled[bank][line]
                    && (self.pending[bank][line] || pulse == Some(line))
                    && if line < 32 {
                        !self.active[core][line]
                    } else {
                        !self.active.iter().any(|a| a[line])
                    }
                    && self.priority[bank][line] < self.mask[core]
                    && (u16::from(self.priority[bank][line]) & group_mask) < running
                    && (line < 32 || self.targets[line] & (1 << core) != 0)
            })
            .min_by_key(|&line| (self.priority[Self::bank(core, line)][line], line))
    }
    pub fn access(
        &mut self,
        core: usize,
        distributor: bool,
        offset: u32,
        size: u32,
        write: bool,
        value: u32,
    ) -> Result<u32, String> {
        let result = self.access_inner(core, distributor, offset, size, write, value);
        if write || (!distributor && offset == 0xc) {
            self.refresh();
        }
        result
    }
    fn access_inner(
        &mut self,
        core: usize,
        distributor: bool,
        offset: u32,
        size: u32,
        write: bool,
        value: u32,
    ) -> Result<u32, String> {
        if core >= self.cores || ![1, 2, 4].contains(&size) || offset % size != 0 {
            return Err("GIC access alignment/width".into());
        }
        if !distributor {
            if size != 4 {
                return Err("GICC requires word access".into());
            }
            return match offset {
                0 => {
                    if write {
                        self.cpu_control[core] = value & 1;
                    }
                    Ok(self.cpu_control[core])
                }
                4 => {
                    if write {
                        self.mask[core] = value as u8;
                    }
                    Ok(self.mask[core] as u32)
                }
                8 => {
                    if write {
                        self.binary_point[core] = value & 7;
                    }
                    Ok(self.binary_point[core])
                }
                0xc if !write => {
                    let irq = self.next_irq(core).unwrap_or(1023);
                    let mut source = 0;
                    if irq != 1023 {
                        if irq < 16 {
                            source = self.sgi_sources[core][irq].trailing_zeros().min(7);
                            self.sgi_sources[core][irq] &= !(1 << source);
                            self.pending[core][irq] = self.sgi_sources[core][irq] != 0;
                        } else {
                            self.pending[Self::bank(core, irq)][irq] = false;
                        }
                        self.active[core][irq] = true;
                    }
                    Ok(irq as u32 | source << 10)
                }
                0x10 if write => {
                    let irq = (value & 1023) as usize;
                    if irq < self.lines {
                        self.active[core][irq] = false;
                        if irq >= 32
                            && self.levels[irq]
                            && self.config[irq / 16] & (2 << (irq % 16 * 2)) == 0
                        {
                            self.pending[0][irq] = true;
                        }
                    }
                    Ok(0)
                }
                0x18 if !write => Ok(self.next_irq(core).unwrap_or(1023) as u32),
                _ => Err(format!("Unsupported GICC offset {offset:#x}")),
            };
        }
        if offset == 0 && size == 4 {
            if write {
                self.control = value & 1;
            }
            return Ok(self.control);
        }
        if offset == 4 && size == 4 && !write {
            return Ok(((self.lines.div_ceil(32) - 1) | ((self.cores - 1) << 5)) as u32);
        }
        // GICv2 IHI 0048B.b sections 4.3.16/17: each target processor has
        // sixteen byte-accessible SGI source masks. Unimplemented sources RAZ/WI.
        for (start, set) in [(0xf10, false), (0xf20, true)] {
            if (start..start + 16).contains(&offset) {
                let first = (offset - start) as usize;
                let source_mask = (1u8 << self.cores) - 1;
                let mut result = 0;
                for byte in 0..size as usize {
                    let sgi = first + byte;
                    let sources = &mut self.sgi_sources[core][sgi];
                    if write {
                        let bits = (value >> (byte * 8)) as u8 & source_mask;
                        if set {
                            *sources |= bits;
                        } else {
                            *sources &= !bits;
                        }
                        self.pending[core][sgi] = *sources != 0;
                    }
                    result |= u32::from(*sources) << (byte * 8);
                }
                return Ok(result);
            }
        }
        if (0x400..0x800).contains(&offset) || (0x800..0xc00).contains(&offset) {
            let priority = offset < 0x800;
            let first = (offset - if priority { 0x400 } else { 0x800 }) as usize;
            if first + size as usize > self.lines {
                return Err("GIC byte register beyond lines".into());
            }
            let mut result = 0;
            for i in 0..size as usize {
                let line = first + i;
                let bank = Self::bank(core, line);
                if priority {
                    if write {
                        self.priority[bank][line] = (value >> (i * 8)) as u8;
                    }
                    result |= u32::from(self.priority[bank][line]) << (i * 8);
                } else {
                    if write && line >= 32 {
                        self.targets[line] = (value >> (i * 8)) as u8 & ((1 << self.cores) - 1);
                    }
                    result |= u32::from(if line < 32 {
                        1 << core
                    } else {
                        self.targets[line]
                    }) << (i * 8);
                }
            }
            return Ok(result);
        }
        if size != 4 {
            return Err("GIC bitmap requires word access".into());
        }
        if (0xc00..0xc00 + self.config.len() as u32 * 4).contains(&offset) {
            let index = ((offset - 0xc00) / 4) as usize;
            if write {
                self.config[index] = value;
            }
            return Ok(self.config[index]);
        }
        for (start, action) in [
            (0x100, 0),
            (0x180, 1),
            (0x200, 2),
            (0x280, 3),
            (0x300, 4),
            (0x380, 5),
        ] {
            if offset >= start && offset < start + self.lines.div_ceil(32) as u32 * 4 {
                let first = ((offset - start) / 4) as usize * 32;
                let mut result = 0;
                for bit in 0..32 {
                    let line = first + bit;
                    if line >= self.lines {
                        break;
                    }
                    let bank = Self::bank(core, line);
                    // GICv2 4.3.8: clearing a level-sensitive SPI's pending
                    // state has no effect while its input stays asserted.
                    let held = action == 3
                        && line >= 32
                        && self.levels[line]
                        && self.config[line / 16] & (2 << (line % 16 * 2)) == 0;
                    let array = if action < 2 {
                        &mut self.enabled
                    } else if action < 4 {
                        &mut self.pending
                    } else {
                        &mut self.active
                    };
                    let b = if action < 2 || action < 4 && line >= 32 {
                        bank
                    } else {
                        core
                    };
                    if write && value & (1 << bit) != 0 {
                        if line >= 16 {
                            array[b][line] = action % 2 == 0 || held;
                        }
                    }
                    if array[b][line] {
                        result |= 1 << bit;
                    }
                }
                return Ok(result);
            }
        }
        if offset == 0xf00 && write {
            let line = (value & 15) as usize;
            let filter = (value >> 24) & 3;
            for target in 0..self.cores {
                let selected = match filter {
                    0 => value & (1 << (16 + target)) != 0,
                    1 => target != core,
                    2 => target == core,
                    _ => false,
                };
                if selected {
                    self.sgi_sources[target][line] |= 1 << core;
                    self.pending[target][line] = true;
                    self.enabled[target][line] = true;
                }
            }
            return Ok(0);
        }
        Err(format!("Unsupported GICD offset {offset:#x}"))
    }
}

impl fujimod_emulator_devices::gic::InterruptController for Gic {
    fn pins(&self, core: usize) -> Result<(bool, bool), String> {
        if core >= self.cores {
            return Err("GICv2 output targets absent processor".into());
        }
        Ok((self.next_irq(core).is_some(), false))
    }
    fn set_spi(&mut self, id: usize, high: bool) -> Result<(), String> {
        if !(32..self.lines).contains(&id) {
            return Err("GICv2 SPI outside configured lines".into());
        }
        self.set_line(id, high);
        Ok(())
    }
    fn set_ppi(&mut self, _core: usize, _id: usize, _high: bool) -> Result<(), String> {
        Err("GICv2 private level inputs are not modeled".into())
    }
    fn system(&mut self, _core: usize, _register: u16, _value: Option<u64>) -> Result<u64, String> {
        Err("GICv2 uses its MMIO CPU interface".into())
    }
    fn distributor(
        &mut self,
        core: usize,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        self.transfer(core, true, offset, size, value)
    }
    fn cpu_interface(
        &mut self,
        core: usize,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        self.transfer(core, false, offset, size, value)
    }
    fn redistributor(
        &mut self,
        _core: usize,
        _offset: u64,
        _size: u8,
        _value: Option<u64>,
    ) -> Result<u64, String> {
        Err("GICv2 has no redistributors".into())
    }
    fn save(&self) -> serde_json::Value {
        Gic::save(self)
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        Gic::restore(self, state)
    }
    fn boxed_clone(&self) -> Box<dyn fujimod_emulator_devices::gic::InterruptController> {
        Box::new(self.clone())
    }
}
impl Gic {
    fn transfer(
        &mut self,
        core: usize,
        distributor: bool,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        let offset = u32::try_from(offset).map_err(|_| "GICv2 register offset overflow")?;
        let word =
            u32::try_from(value.unwrap_or(0)).map_err(|_| "GICv2 register value overflow")?;
        self.access(
            core,
            distributor,
            offset,
            u32::from(size),
            value.is_some(),
            word,
        )
        .map(u64::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn banked_sgi_pending_bytes_preserve_sources_active_state_and_other_targets() {
        let mut g = Gic::new(2, 64);
        g.access(0, true, 0, 4, true, 1).unwrap();
        for core in 0..2 {
            g.access(core, false, 0, 4, true, 1).unwrap();
            g.access(core, false, 4, 4, true, 255).unwrap();
        }
        g.access(0, true, 0xf00, 4, true, 0x00030005).unwrap();
        g.access(1, true, 0xf00, 4, true, 0x00010005).unwrap();
        assert_eq!(g.access(0, true, 0xf15, 1, false, 0).unwrap(), 3);
        assert_eq!(g.access(1, true, 0xf25, 1, false, 0).unwrap(), 1);
        g.access(0, true, 0xf15, 1, true, 2).unwrap();
        assert_eq!(g.access(0, true, 0xf25, 1, false, 0).unwrap(), 1);
        assert_eq!(g.access(1, true, 0xf15, 1, false, 0).unwrap(), 1);
        g.access(0, true, 0xf24, 4, true, 0x00000200).unwrap();
        assert_eq!(g.access(0, true, 0xf14, 4, false, 0).unwrap(), 0x300);
        // Source seven is absent on this two-processor controller.
        g.access(0, true, 0xf2e, 2, true, 0x8000).unwrap();
        assert_eq!(g.access(0, true, 0xf1e, 2, false, 0).unwrap(), 0);
        let saved = g.save();
        let mut resumed = Gic::new(2, 64);
        resumed.restore(&saved).unwrap();
        for machine in [&mut g, &mut resumed] {
            assert_eq!(machine.access(0, false, 12, 4, false, 0).unwrap(), 5);
            assert_eq!(machine.access(0, true, 0xf15, 1, false, 0).unwrap(), 2);
            machine.access(0, true, 0xf15, 1, true, 2).unwrap();
            assert_eq!(machine.access(0, true, 0x300, 4, false, 0).unwrap(), 1 << 5);
            assert_eq!(machine.next_irq(0), None);
            machine.access(0, false, 16, 4, true, 5).unwrap();
            assert_eq!(machine.next_irq(0), None);
            assert_eq!(machine.access(1, false, 12, 4, false, 0).unwrap(), 5);
        }
        assert_eq!(g.save(), resumed.save());
        let before = resumed.save();
        let mut invalid = saved.clone();
        invalid["sgi_sources"][0][5] = serde_json::json!(0x80);
        assert!(resumed.restore(&invalid).is_err());
        assert_eq!(resumed.save(), before);
        let mut invalid = saved;
        invalid["pending"][0][5] = serde_json::json!(false);
        assert!(resumed.restore(&invalid).is_err());
        assert_eq!(resumed.save(), before);
    }
    #[test]
    fn clearing_an_asserted_level_line_leaves_it_pending() {
        let mut g = Gic::new(1, 128);
        g.access(0, true, 0, 4, true, 1).unwrap();
        g.access(0, false, 0, 4, true, 1).unwrap();
        g.access(0, false, 4, 4, true, 255).unwrap();
        g.access(0, true, 0x108, 4, true, 1 << 19).unwrap();
        g.set_line(83, true);
        g.access(0, true, 0x288, 4, true, 1 << 19).unwrap();
        assert_eq!(g.access(0, true, 0x288, 4, false, 0).unwrap(), 1 << 19);
        assert_eq!(g.next_irq(0), Some(83));
        g.set_line(83, false);
        g.access(0, true, 0x288, 4, true, 1 << 19).unwrap();
        assert_eq!(g.access(0, true, 0x288, 4, false, 0).unwrap(), 0);
        // An edge-triggered line keeps its latch semantics.
        g.access(0, true, 0xc14, 4, true, 2 << 6).unwrap();
        g.set_line(83, true);
        g.access(0, true, 0x288, 4, true, 1 << 19).unwrap();
        assert_eq!(g.next_irq(0), None);
    }
    #[test]
    fn checkpoint_preserves_active_irq_and_deferred_sgi_sources() {
        let mut g = Gic::new(4, 512);
        g.access(0, true, 0, 4, true, 1).unwrap();
        g.access(3, false, 0, 4, true, 1).unwrap();
        g.access(3, false, 4, 4, true, 255).unwrap();
        g.access(1, true, 0xf00, 4, true, 0x00080003).unwrap();
        g.access(2, true, 0xf00, 4, true, 0x00080003).unwrap();
        assert_eq!(g.access(3, false, 12, 4, false, 0).unwrap(), 3 | 1 << 10);
        let saved = g.save();
        let mut resumed = Gic::new(4, 512);
        resumed.restore(&saved).unwrap();
        for machine in [&mut g, &mut resumed] {
            assert_eq!(machine.next_irq(3), None);
            machine.access(3, false, 16, 4, true, 3).unwrap();
            assert_eq!(
                machine.access(3, false, 12, 4, false, 0).unwrap(),
                3 | 2 << 10
            );
            machine.access(3, false, 16, 4, true, 3).unwrap();
            assert_eq!(machine.next_irq(3), None);
        }
        assert_eq!(g.save(), resumed.save());
        let mut invalid = saved;
        invalid["eligible"] = serde_json::json!([null]);
        let before = resumed.save();
        assert!(resumed.restore(&invalid).is_err());
        assert_eq!(resumed.save(), before);
    }
    #[test]
    fn banked_enable_sgi_acknowledge_and_eoi() {
        let mut g = Gic::new(4, 512);
        g.access(0, true, 0, 4, true, 1).unwrap();
        for core in 0..4 {
            g.access(core, false, 0, 4, true, 1).unwrap();
            g.access(core, false, 4, 4, true, 255).unwrap();
        }
        g.access(0, true, 0xf00, 4, true, 0x000e0003).unwrap();
        assert_eq!(g.next_irq(0), None);
        for core in 1..4 {
            assert_eq!(g.next_irq(core), Some(3));
            assert_eq!(g.access(core, false, 12, 4, false, 0).unwrap(), 3);
            assert_eq!(g.next_irq(core), None);
            g.access(core, false, 16, 4, true, 3).unwrap();
            assert_eq!(g.access(core, false, 12, 4, false, 0).unwrap(), 1023);
        }
    }
}
