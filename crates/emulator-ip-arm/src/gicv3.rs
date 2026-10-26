//! Bounded GICv3: affinity routing, one Security state (DS=1), eight priority
//! bits, SGIs/PPIs/SPIs and the EL1 ICC system interface. No ITS, LPIs, legacy
//! GICC, 1-of-N routing, virtualization or security-bank emulation.
//! Register law: Arm IHI0069H.b, chapters4 and12; no program state is visible.
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod checkpoint;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Line {
    enabled: bool,
    group1: bool,
    priority: u8,
    edge: bool,
    level: bool,
    latched: bool,
    active: bool,
}
impl Line {
    fn pending(&self) -> bool {
        self.latched || self.level && !self.edge && !self.active
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Active {
    id: usize,
    priority: u8,
    group: usize,
    dropped: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct Interface {
    system_register_enable: bool,
    enabled: [bool; 2],
    pmr: u8,
    bpr: [u8; 2],
    split_eoi: bool,
    asleep: bool,
    active: Vec<Active>,
}
impl Default for Interface {
    fn default() -> Self {
        Self {
            system_register_enable: false,
            enabled: [false; 2],
            pmr: 0,
            bpr: [0; 2],
            split_eoi: false,
            asleep: true,
            active: vec![],
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct State {
    affinities: Vec<u32>,
    distributor: u32,
    private: Vec<Vec<Line>>,
    shared: Vec<Line>,
    routes: Vec<u64>,
    interfaces: Vec<Interface>,
}
#[derive(Clone)]
pub struct Gic {
    state: State,
    // Derived pin outputs, excluded from architectural/checkpoint state.
    eligible: Vec<[Option<usize>; 2]>,
}
impl Gic {
    pub fn new(affinities: Vec<u32>, lines: usize) -> Result<Self, String> {
        if affinities.is_empty()
            || affinities.len() > 256
            || !(32..=1020).contains(&lines)
            || affinities
                .iter()
                .enumerate()
                .any(|(i, a)| affinities[..i].contains(a))
        {
            return Err("GICv3 requires distinct affinities and 32..1020 lines".into());
        }
        let cores = affinities.len();
        let mut private = vec![vec![Line::default(); 32]; cores];
        for bank in &mut private {
            for line in &mut bank[..16] {
                line.edge = true;
            }
        }
        Ok(Self {
            state: State {
                affinities,
                distributor: 0,
                private,
                shared: vec![Line::default(); lines],
                routes: vec![0; lines],
                interfaces: vec![Interface::default(); cores],
            },
            eligible: vec![[None; 2]; cores],
        })
    }
    fn core(&self, core: usize) -> Result<(), String> {
        if core < self.state.affinities.len() {
            Ok(())
        } else {
            Err("GICv3 processor index out of range".into())
        }
    }
    fn line(&self, core: usize, id: usize) -> &Line {
        if id < 32 {
            &self.state.private[core][id]
        } else {
            &self.state.shared[id]
        }
    }
    fn line_mut(&mut self, core: usize, id: usize) -> &mut Line {
        if id < 32 {
            &mut self.state.private[core][id]
        } else {
            &mut self.state.shared[id]
        }
    }
    fn route_affinity(route: u64) -> u32 {
        (route as u32 & 0xffffff) | ((route >> 8) as u32 & 0xff000000)
    }
    fn refresh(&mut self) {
        for core in 0..self.eligible.len() {
            for group in 0..2 {
                let interface = &self.state.interfaces[core];
                let shift = interface.bpr[group] + 1;
                let running = interface
                    .active
                    .iter()
                    .filter(|a| !a.dropped)
                    .map(|a| u16::from(a.priority))
                    .min()
                    .unwrap_or(256);
                self.eligible[core][group] = if interface.asleep
                    || !interface.enabled[group]
                    || self.state.distributor & (1 << group) == 0
                {
                    None
                } else {
                    (0..self.state.shared.len())
                        .filter(|&id| {
                            let line = self.line(core, id);
                            line.enabled
                                && usize::from(line.group1) == group
                                && line.pending()
                                && !line.active
                                && line.priority < interface.pmr
                                && (u16::from(line.priority) >> shift) < (running >> shift)
                                && (id < 32
                                    || Self::route_affinity(self.state.routes[id])
                                        == self.state.affinities[core])
                        })
                        .min_by_key(|&id| (self.line(core, id).priority, id))
                };
            }
        }
    }
    /// Group0 signals FIQ; Group1 signals IRQ. This never acknowledges an ID.
    pub fn pins(&self, core: usize) -> Result<(bool, bool), String> {
        self.core(core)?;
        Ok((
            self.eligible[core][1].is_some(),
            self.eligible[core][0].is_some(),
        ))
    }
    pub fn set_spi(&mut self, id: usize, high: bool) -> Result<(), String> {
        if !(32..self.state.shared.len()).contains(&id) {
            return Err("GICv3 SPI out of range".into());
        }
        if self.state.shared[id].level == high {
            return Ok(());
        }
        Self::set_level(&mut self.state.shared[id], high);
        self.refresh();
        Ok(())
    }
    pub fn set_ppi(&mut self, core: usize, id: usize, high: bool) -> Result<(), String> {
        self.core(core)?;
        if !(16..32).contains(&id) {
            return Err("GICv3 PPI out of range".into());
        }
        if self.state.private[core][id].level == high {
            return Ok(());
        }
        Self::set_level(&mut self.state.private[core][id], high);
        self.refresh();
        Ok(())
    }
    fn set_level(line: &mut Line, high: bool) {
        if line.edge && high && !line.level {
            line.latched = true;
        }
        line.level = high;
    }
    fn acknowledge(&mut self, core: usize, group: usize) -> u64 {
        let Some(id) = self.eligible[core][group] else {
            return 1023;
        };
        let line = self.line_mut(core, id);
        line.latched = false;
        line.active = true;
        let priority = line.priority;
        self.state.interfaces[core].active.push(Active {
            id,
            priority,
            group,
            dropped: false,
        });
        id as u64
    }
    fn eoi(&mut self, core: usize, group: usize, id: usize) -> Result<(), String> {
        if id >= 1020 {
            return Ok(());
        }
        let interface = &mut self.state.interfaces[core];
        let at = interface
            .active
            .iter()
            .rposition(|a| !a.dropped)
            .ok_or("GICv3 EOI without an active priority")?;
        if interface.active[at].id != id || interface.active[at].group != group {
            return Err("GICv3 EOI nesting/group mismatch".into());
        }
        if interface.split_eoi {
            interface.active[at].dropped = true;
        } else {
            interface.active.remove(at);
            self.line_mut(core, id).active = false;
        }
        Ok(())
    }
    fn sgi(&mut self, core: usize, group0_only: bool, value: u64) -> Result<(), String> {
        if value & (15 << 44) != 0 {
            return Err("GICv3 SGI range selection is not implemented".into());
        }
        let id = (value >> 24 & 15) as usize;
        let others = value & (1 << 40) != 0;
        let cluster = (((value >> 48 & 255) << 24)
            | ((value >> 32 & 255) << 16)
            | ((value >> 8) & 0xff00)) as u32;
        for target in 0..self.state.affinities.len() {
            let affinity = self.state.affinities[target];
            let aff0 = affinity & 255;
            let selected = if others {
                target != core
            } else {
                affinity & 0xffffff00 == cluster && aff0 < 16 && value & (1 << aff0) != 0
            };
            if selected && (!group0_only || !self.state.private[target][id].group1) {
                self.state.private[target][id].latched = true;
            }
        }
        Ok(())
    }
    /// A guest ICC MRS/MSR, after the CPU has produced an architectural trap.
    /// Host inspection uses save()/pins(), never this acknowledging path.
    pub fn system(
        &mut self,
        core: usize,
        register: u16,
        value: Option<u64>,
    ) -> Result<u64, String> {
        self.core(core)?;
        let result = self.system_inner(core, register, value);
        self.refresh();
        result
    }
    fn system_inner(
        &mut self,
        core: usize,
        register: u16,
        value: Option<u64>,
    ) -> Result<u64, String> {
        match register {
            0xf665 => {
                if let Some(v) = value {
                    if v & !15 != 0 {
                        return Err("unsupported GICv3 ICC_SRE_EL3 bits".into());
                    }
                    self.state.interfaces[core].system_register_enable = v & 8 != 0;
                }
                // This implementation has no legacy interface: SRE/DIB/DFB
                // are RAO/WI; Enable is retained per core.
                Ok(7 | u64::from(self.state.interfaces[core].system_register_enable) * 8)
            }
            0xc230 => {
                if let Some(v) = value {
                    self.state.interfaces[core].pmr = v as u8;
                }
                Ok(self.state.interfaces[core].pmr.into())
            }
            0xc643 | 0xc663 => {
                let g = usize::from(register == 0xc663);
                if let Some(v) = value {
                    self.state.interfaces[core].bpr[g] = (v & 7) as u8;
                }
                Ok(self.state.interfaces[core].bpr[g].into())
            }
            0xc664 => {
                if let Some(v) = value {
                    if v & !(0x700 | 2) != 0 {
                        return Err("unsupported GICv3 ICC_CTLR mode".into());
                    }
                    self.state.interfaces[core].split_eoi = v & 2 != 0;
                }
                Ok(0x700 | u64::from(self.state.interfaces[core].split_eoi) * 2)
            }
            0xc665 => Ok(7), // System interface only; SRE/DIB/DFB are RAO/WI.
            0xc666 | 0xc667 => {
                let g = usize::from(register == 0xc667);
                if let Some(v) = value {
                    self.state.interfaces[core].enabled[g] = v & 1 != 0;
                }
                Ok(self.state.interfaces[core].enabled[g].into())
            }
            0xc640 | 0xc660 if value.is_none() => {
                Ok(self.acknowledge(core, usize::from(register == 0xc660)))
            }
            0xc642 | 0xc662 if value.is_none() => {
                Ok(self.eligible[core][usize::from(register == 0xc662)].unwrap_or(1023) as u64)
            }
            0xc641 | 0xc661 if value.is_some() => {
                self.eoi(
                    core,
                    usize::from(register == 0xc661),
                    (value.unwrap() & 0xffffff) as usize,
                )?;
                Ok(0)
            }
            0xc659 if value.is_some() => {
                let id = (value.unwrap() & 0xffffff) as usize;
                if id >= 1020 {
                    return Ok(0);
                }
                let interface = &mut self.state.interfaces[core];
                let at = interface
                    .active
                    .iter()
                    .position(|a| a.id == id && a.dropped)
                    .ok_or("GICv3 DIR without a dropped active priority")?;
                interface.active.remove(at);
                self.line_mut(core, id).active = false;
                Ok(0)
            }
            0xc65b if value.is_none() => Ok(self.state.interfaces[core]
                .active
                .iter()
                .filter(|a| !a.dropped)
                .map(|a| a.priority)
                .min()
                .unwrap_or(255)
                .into()),
            0xc65d | 0xc65f if value.is_some() => {
                self.sgi(core, register == 0xc65f, value.unwrap())?;
                Ok(0)
            }
            // Other-security SGIs require an explicit security-bank model.
            0xc644..=0xc64b => {
                let group = usize::from(register >= 0xc648);
                let n = (register - 0xc644) % 4;
                let bits = self.state.interfaces[core]
                    .active
                    .iter()
                    .filter(|a| !a.dropped && a.group == group && (a.priority >> 6) == n as u8)
                    .fold(0u64, |v, a| v | 1 << ((a.priority >> 1) & 31));
                if value.is_some_and(|v| v != bits) {
                    return Err("GICv3 active-priority register rewrite is not implemented".into());
                }
                Ok(bits)
            }
            _ => Err(format!("unsupported GICv3 ICC register {register:#x}")),
        }
    }
    pub fn distributor(
        &mut self,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        let result = self.mmio(false, 0, offset, size, value);
        if value.is_some() {
            self.refresh();
        }
        result
    }
    pub fn redistributor(
        &mut self,
        core: usize,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        self.core(core)?;
        let result = self.mmio(true, core, offset, size, value);
        if value.is_some() {
            self.refresh();
        }
        result
    }
    fn mmio(
        &mut self,
        private: bool,
        core: usize,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        if !matches!(size, 1 | 2 | 4 | 8) || offset % u64::from(size) != 0 {
            return Err("GICv3 MMIO width/alignment".into());
        }
        if private && offset < 0x10000 {
            let typer = ((self.state.affinities[core] as u64) << 32)
                | ((core as u64) << 8)
                | if core + 1 == self.state.affinities.len() {
                    16
                } else {
                    0
                };
            return match (offset, size) {
                (0, 4) => {
                    if value.is_some_and(|v| v != 0) {
                        Err("GICv3 LPI/power routing modes are not implemented".into())
                    } else {
                        Ok(0)
                    }
                }
                (4, 4) | (0x10, 4) => Ok(0),
                (8, 8) => Ok(typer),
                (8, 4) => Ok(typer & 0xffffffff),
                (12, 4) => Ok(typer >> 32),
                (0x14, 4) => {
                    if let Some(v) = value {
                        self.state.interfaces[core].asleep = v & 2 != 0;
                    }
                    Ok(if self.state.interfaces[core].asleep {
                        6
                    } else {
                        0
                    })
                }
                (0xffe8, 4) => Ok(0x30),
                _ => Err(format!(
                    "unsupported GICv3 redistributor register {offset:#x}"
                )),
            };
        }
        let offset = if private { offset - 0x10000 } else { offset };
        if !private {
            if size == 4 {
                match offset {
                    0 => {
                        if let Some(v) = value {
                            self.state.distributor = v as u32 & 3;
                        }
                        return Ok(0x50 | u64::from(self.state.distributor));
                    }
                    4 => return Ok((self.state.shared.len().div_ceil(32) - 1) as u64 | (0xf << 19)), // 16-bit IDs, no LPIs/security extensions.
                    8 | 12 => return Ok(0),
                    0xffe8 => return Ok(0x30),
                    _ => {}
                }
            }
            if (0x6000..0x8000).contains(&offset) && matches!(size, 4 | 8) {
                let id = ((offset - 0x6000) / 8) as usize;
                if id < 32 || id >= self.state.shared.len() {
                    return Ok(0);
                }
                let shift = (offset & 7) * 8;
                if let Some(v) = value {
                    let mask = if size == 8 {
                        u64::MAX
                    } else {
                        0xffffffff << shift
                    };
                    let route = (self.state.routes[id] & !mask) | ((v << shift) & mask);
                    if route & (1 << 31) != 0 {
                        return Err("GICv3 1-of-N routing is not implemented".into());
                    }
                    self.state.routes[id] = route & 0xff00ffffff;
                }
                return Ok((self.state.routes[id] >> shift)
                    & if size == 8 { u64::MAX } else { 0xffffffff });
            }
            // Legacy target interface and security modifiers are RAZ/WI with
            // affinity routing and the selected single Security state.
            if (0x800..0xc00).contains(&offset) && matches!(size, 1 | 2 | 4)
                || (0xd00..0xf04).contains(&offset) && size == 4
            {
                return Ok(0);
            }
        }
        let limit = if private { 32 } else { 1024 };
        if (0x400..0x400 + limit).contains(&offset) && matches!(size, 1 | 2 | 4) {
            let first = (offset - 0x400) as usize;
            let mut out = 0;
            for n in 0..size as usize {
                let id = first + n;
                if id >= self.state.shared.len() || !private && id < 32 {
                    continue;
                }
                let line = self.line_mut(core, id);
                if let Some(v) = value {
                    line.priority = (v >> (n * 8)) as u8;
                }
                out |= u64::from(line.priority) << (n * 8);
            }
            return Ok(out);
        }
        if size != 4 {
            return Err("GICv3 bitmap requires word access".into());
        }
        if (0xc00..0xc00 + limit / 4).contains(&offset) {
            let first = ((offset - 0xc00) * 4) as usize;
            let mut out = 0;
            for n in 0..16 {
                let id = first + n;
                if id >= self.state.shared.len() || !private && id < 32 {
                    continue;
                }
                let line = self.line_mut(core, id);
                if let Some(v) = value {
                    if id >= 16 {
                        line.edge = v & (2 << (n * 2)) != 0;
                    }
                }
                out |= u64::from(line.edge) * 2 << (n * 2);
            }
            return Ok(out);
        }
        for (start, kind) in [
            (0x80, 0),
            (0x100, 1),
            (0x180, 2),
            (0x200, 3),
            (0x280, 4),
            (0x300, 5),
            (0x380, 6),
        ] {
            if !(start..start + limit / 8).contains(&offset) {
                continue;
            }
            let first = ((offset - start) * 8) as usize;
            let mut out = 0;
            for n in 0..32 {
                let id = first + n;
                if id >= self.state.shared.len() || !private && id < 32 {
                    continue;
                }
                let line = self.line_mut(core, id);
                if let Some(v) = value {
                    if kind == 0 {
                        line.group1 = v & (1 << n) != 0;
                    } else if v & (1 << n) != 0 {
                        match kind {
                            1 => line.enabled = true,
                            2 => line.enabled = false,
                            3 => line.latched = true,
                            4 => line.latched = false,
                            5 => line.active = true,
                            6 => line.active = false,
                            _ => unreachable!(),
                        }
                    }
                }
                let bit = match kind {
                    0 => line.group1,
                    1 | 2 => line.enabled,
                    3 | 4 => line.pending(),
                    5 | 6 => line.active,
                    _ => unreachable!(),
                };
                if bit {
                    out |= 1 << n;
                }
            }
            return Ok(out);
        }
        if private && matches!(offset, 0xd00 | 0xe00) {
            return Ok(0);
        }
        Err(format!(
            "unsupported GICv3 {} register {offset:#x}",
            if private { "SGI/PPI" } else { "distributor" }
        ))
    }
    pub fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    pub fn restore(&mut self, value: &Value) -> Result<(), String> {
        let s: State =
            serde_json::from_value(value.clone()).map_err(|e| format!("GICv3 checkpoint: {e}"))?;
        self.restore_state(s)
    }
    fn restore_state(&mut self, s: State) -> Result<(), String> {
        let cores = self.state.affinities.len();
        let lines = self.state.shared.len();
        if s.affinities != self.state.affinities
            || s.shared.len() != lines
            || s.routes.len() != lines
            || s.private.len() != cores
            || s.private.iter().any(|p| p.len() != 32)
            || s.interfaces.len() != cores
            || s.distributor & !3 != 0
            || s.interfaces.iter().any(|i| {
                i.bpr.iter().any(|b| *b > 7)
                    || i.active.iter().any(|a| a.id >= lines || a.group > 1)
            })
            || s.routes.iter().any(|r| r & !0xff00ffffff != 0)
            || s.private.iter().any(|p| p[..16].iter().any(|l| !l.edge))
        {
            return Err("GICv3 checkpoint topology/state differs".into());
        }
        self.state = s;
        self.refresh();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn el3_system_interface_enable_is_per_core_and_survives_checkpoint() {
        let mut g = super::Gic::new(vec![0, 1], 128).unwrap();
        assert_eq!(g.system(0, 0xf665, None).unwrap(), 7);
        assert_eq!(g.system(0, 0xf665, Some(9)).unwrap(), 15);
        assert_eq!(g.system(1, 0xf665, None).unwrap(), 7);
        let saved = g.save();
        g.system(0, 0xf665, Some(0)).unwrap();
        g.restore(&saved).unwrap();
        assert_eq!(g.system(0, 0xf665, None).unwrap(), 15);
        assert!(g.system(0, 0xf665, Some(16)).is_err());
    }
    use super::*;
    fn enabled(affinities: Vec<u32>) -> Gic {
        let mut g = Gic::new(affinities, 512).unwrap();
        g.distributor(0, 4, Some(7)).unwrap();
        for core in 0..g.state.affinities.len() {
            g.redistributor(core, 0x14, 4, Some(0)).unwrap();
            g.system(core, 0xc230, Some(255)).unwrap();
            g.system(core, 0xc666, Some(1)).unwrap();
            g.system(core, 0xc667, Some(1)).unwrap();
        }
        g
    }
    fn spi(g: &mut Gic, id: usize, group1: bool, priority: u8, route: u64) {
        let word = 0x80 + (id / 32) * 4;
        let old = g.distributor(word as u64, 4, None).unwrap();
        let bit = 1 << (id % 32);
        g.distributor(
            word as u64,
            4,
            Some(if group1 { old | bit } else { old & !bit }),
        )
        .unwrap();
        g.distributor(0x400 + id as u64, 1, Some(priority.into()))
            .unwrap();
        g.distributor(0x6000 + id as u64 * 8, 8, Some(route))
            .unwrap();
        g.distributor((0x100 + (id / 32) * 4) as u64, 4, Some(bit))
            .unwrap();
    }
    #[test]
    fn routed_level_irq_acknowledgment_and_inspection_are_independent() {
        let mut g = enabled(vec![0, 1, 2, 3]);
        spi(&mut g, 33, true, 0x80, 3);
        g.set_spi(33, true).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, false));
        assert_eq!(g.pins(3).unwrap(), (true, false));
        let state = g.save();
        for _ in 0..10 {
            assert_eq!(g.system(3, 0xc662, None).unwrap(), 33);
            assert_eq!(g.pins(3).unwrap(), (true, false));
            assert_eq!(g.save(), state);
        }
        assert_eq!(g.system(3, 0xc660, None).unwrap(), 33);
        assert_eq!(g.system(3, 0xc662, None).unwrap(), 1023);
        assert_eq!(g.system(3, 0xc65b, None).unwrap(), 0x80);
        g.system(3, 0xc661, Some(33)).unwrap();
        assert_eq!(g.pins(3).unwrap(), (true, false)); // Held level reasserts.
        g.system(3, 0xc660, None).unwrap();
        g.set_spi(33, false).unwrap();
        g.system(3, 0xc661, Some(33)).unwrap();
        assert_eq!(g.pins(3).unwrap(), (false, false));
    }
    #[test]
    fn priority_preemption_binary_point_and_split_deactivation() {
        let mut g = enabled(vec![0]);
        spi(&mut g, 32, true, 0x80, 0);
        spi(&mut g, 33, true, 0x40, 0);
        g.set_spi(32, true).unwrap();
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 32);
        g.set_spi(33, true).unwrap();
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 33);
        let before = g.save();
        assert!(g.system(0, 0xc661, Some(32)).is_err());
        assert_eq!(g.save(), before);
        g.system(0, 0xc664, Some(2)).unwrap();
        g.system(0, 0xc661, Some(33)).unwrap();
        assert_eq!(g.system(0, 0xc65b, None).unwrap(), 0x80);
        assert_eq!(g.system(0, 0xc662, None).unwrap(), 1023); // Still active until DIR.
        g.set_spi(33, false).unwrap();
        g.system(0, 0xc659, Some(33)).unwrap();
        g.set_spi(32, false).unwrap();
        g.system(0, 0xc661, Some(32)).unwrap();
        g.system(0, 0xc659, Some(32)).unwrap();
        // Priorities80/81 share a group priority when BPR=0.
        spi(&mut g, 34, true, 0x81, 0);
        spi(&mut g, 35, true, 0x80, 0);
        g.set_spi(34, true).unwrap();
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 34);
        g.set_spi(35, true).unwrap();
        assert_eq!(g.system(0, 0xc662, None).unwrap(), 1023);
    }
    #[test]
    fn sgi_affinity_broadcast_groups_and_private_banks() {
        let mut g = enabled(vec![0, 0x12345601, 0x12345501]);
        for core in 0..3 {
            g.redistributor(core, 0x10100, 4, Some(1 << 7)).unwrap();
            g.redistributor(core, 0x10407, 1, Some(0x40)).unwrap();
        }
        let targeted = (0x12u64 << 48) | (0x34u64 << 32) | (0x56u64 << 16) | (7 << 24) | 2;
        g.system(0, 0xc65f, Some(targeted)).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, false));
        assert_eq!(g.pins(1).unwrap(), (false, true));
        assert_eq!(g.pins(2).unwrap(), (false, false));
        assert_eq!(g.system(1, 0xc640, None).unwrap(), 7);
        g.system(1, 0xc641, Some(7)).unwrap();
        g.system(0, 0xc65d, Some((1 << 40) | (7 << 24))).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, false));
        assert_eq!(g.pins(1).unwrap(), (false, true));
        assert_eq!(g.pins(2).unwrap(), (false, true));
        g.redistributor(2, 0x10180, 4, Some(1 << 7)).unwrap();
        assert_eq!(g.pins(2).unwrap(), (false, false));
        assert_eq!(g.pins(1).unwrap(), (false, true));
    }
    #[test]
    fn edge_latches_survive_deassertion_but_do_not_repeat_from_high_level() {
        let mut g = enabled(vec![0]);
        spi(&mut g, 32, true, 0x40, 0);
        g.distributor(0xc08, 4, Some(2)).unwrap();
        g.set_spi(32, true).unwrap();
        g.set_spi(32, false).unwrap();
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 32);
        g.system(0, 0xc661, Some(32)).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, false));
        g.set_spi(32, true).unwrap();
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 32);
        g.set_spi(32, true).unwrap();
        g.system(0, 0xc661, Some(32)).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, false));
    }
    #[test]
    fn snapshots_preserve_active_and_pending_state_without_guest_acknowledgment() {
        let mut g = enabled(vec![0, 1]);
        spi(&mut g, 32, true, 0x40, 1);
        spi(&mut g, 33, false, 0x20, 0);
        g.set_spi(32, true).unwrap();
        g.system(1, 0xc660, None).unwrap();
        g.set_spi(33, true).unwrap();
        let saved = g.save();
        let mut restored = Gic::new(vec![0, 1], 512).unwrap();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
        // Durable decoding reconstructs the derived eligibility cache as well.
        let encoded = fujimod_emulator_devices::checkpoint::Checkpoint::encode("gic", &g).unwrap();
        let mut decoded: Gic = encoded.decode("gic").unwrap();
        assert_eq!(decoded.save(), saved);
        assert_eq!(decoded.pins(0).unwrap(), (false, true));
        assert_eq!(decoded.pins(1).unwrap(), (false, false));
        assert_eq!(decoded.system(1, 0xc65b, None).unwrap(), 0x40);
        assert_eq!(restored.pins(0).unwrap(), (false, true));
        assert_eq!(restored.pins(1).unwrap(), (false, false));
        assert_eq!(restored.system(1, 0xc65b, None).unwrap(), 0x40);
        assert!(Gic::new(vec![1, 0], 512).unwrap().restore(&saved).is_err());
        assert!(g.distributor(0x6100, 8, Some(1 << 31)).is_err());
        assert!(g.system(0, 0xc65e, Some(0)).is_err());
        assert!(g.redistributor(0, 0, 4, Some(1)).is_err());
        // Architecture-reserved and unimplemented trailing IDs are RAZ/WI.
        assert_eq!(g.distributor(0x17c, 4, Some(u32::MAX.into())).unwrap(), 0);
        assert_eq!(g.distributor(0x880, 4, Some(u32::MAX.into())).unwrap(), 0);
    }
}

impl fujimod_emulator_devices::gic::InterruptController for Gic {
    fn pins(&self, core: usize) -> Result<(bool, bool), String> {
        Gic::pins(self, core)
    }
    fn set_spi(&mut self, id: usize, high: bool) -> Result<(), String> {
        Gic::set_spi(self, id, high)
    }
    fn set_ppi(&mut self, core: usize, id: usize, high: bool) -> Result<(), String> {
        Gic::set_ppi(self, core, id, high)
    }
    fn system(&mut self, core: usize, register: u16, value: Option<u64>) -> Result<u64, String> {
        Gic::system(self, core, register, value)
    }
    fn distributor(
        &mut self,
        _core: usize,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        Gic::distributor(self, offset, size, value)
    }
    fn redistributor(
        &mut self,
        core: usize,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        Gic::redistributor(self, core, offset, size, value)
    }
    fn save(&self) -> Value {
        Gic::save(self)
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        Gic::restore(self, state)
    }
    fn boxed_clone(&self) -> Box<dyn fujimod_emulator_devices::gic::InterruptController> {
        Box::new(self.clone())
    }
}
