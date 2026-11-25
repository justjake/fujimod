//! Bounded GICv3: affinity routing, eight priority bits, SGIs/PPIs/SPIs and
//! the EL1 ICC system interface. No ITS, LPIs, legacy GICC, 1-of-N routing or
//! virtualization. Register law: Arm IHI0069H.b, chapters 4 and 12; no
//! program state is visible.
//!
//! The implemented Security states are declared at construction:
//! - One (GICD_CTLR.DS=1): IGRPMODR is RAZ/WI, Group 0 signals FIQ and
//!   Group 1 signals IRQ, independent of the processor.
//! - Two (GICD_CTLR.DS=0): IGROUPR/IGRPMODR select Secure Group 0 (0,0),
//!   Secure Group 1 (0,1) or Non-secure Group 1 (1,0); the reserved (1,1)
//!   encoding is treated as Non-secure Group 1. Each processor's Security
//!   state is an input delivered by the processor framework. At Secure EL1/EL0
//!   Secure Group 1 signals IRQ, at Non-secure EL1/EL0 Non-secure Group 1
//!   does; every other group signals FIQ, and at EL3 all groups signal FIQ.
//!   ICC_*1_EL1 registers address the Group 1 bank of SCR_EL3.NS. Distributor
//!   and redistributor accesses use the Secure register view. ICC_BPR1_EL1
//!   and ICC_CTLR_EL1 keep one (Secure) copy; Non-secure accesses to them and
//!   to Group 0 registers, and EL3 acknowledgment, are refused.
use fujimod_emulator_devices::gic::{ProcessorSecurity, SecurityStates};
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod checkpoint;

/// Interrupt groups, indexing per-group enables and pending candidates. With
/// one Security state every Group 1 interrupt is Non-secure Group 1.
const GROUP0: usize = 0;
const GROUP1_NON_SECURE: usize = 1;
const GROUP1_SECURE: usize = 2;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Line {
    enabled: bool,
    group1: bool,
    /// GICD_IGRPMODR/GICR_IGRPMODR0; always clear with one Security state.
    #[serde(default)]
    modifier: bool,
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
    fn group(&self) -> usize {
        match (self.group1, self.modifier) {
            (false, false) => GROUP0,
            (false, true) => GROUP1_SECURE,
            // (1,1) is reserved and treated as Non-secure Group 1.
            (true, _) => GROUP1_NON_SECURE,
        }
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
#[serde(try_from = "InterfaceRecord")]
struct Interface {
    system_register_enable: bool,
    /// ICC_SRE_EL2.Enable: lets EL1 access ICC_SRE_EL1. Retained, not gating.
    #[serde(default)]
    hypervisor_enable: bool,
    /// ICC_IGRPEN0, Non-secure ICC_IGRPEN1 and Secure ICC_IGRPEN1. With one
    /// Security state no line is Secure Group 1, so ICC_IGRPEN1_EL3.EnableGrp1S
    /// is retained without gating.
    enabled: [bool; 3],
    pmr: u8,
    bpr: [u8; 2],
    split_eoi: bool,
    asleep: bool,
    active: Vec<Active>,
    /// The processor's delivered Security state; used with two states only.
    security: ProcessorSecurity,
}
/// A checkpointed CPU interface. Checkpoints from before two Security states
/// carry two group enables and the Secure Group 1 enable as
/// `el3_group1_secure`, and no delivered Security state.
#[derive(Deserialize)]
struct InterfaceRecord {
    system_register_enable: bool,
    #[serde(default)]
    hypervisor_enable: bool,
    #[serde(default)]
    el3_group1_secure: bool,
    enabled: Vec<bool>,
    pmr: u8,
    bpr: [u8; 2],
    split_eoi: bool,
    asleep: bool,
    active: Vec<Active>,
    #[serde(default = "reset_security")]
    security: ProcessorSecurity,
}
fn reset_security() -> ProcessorSecurity {
    ProcessorSecurity::RESET
}
impl TryFrom<InterfaceRecord> for Interface {
    type Error = String;
    fn try_from(record: InterfaceRecord) -> Result<Self, String> {
        let enabled = match record.enabled[..] {
            [group0, group1] => [group0, group1, record.el3_group1_secure],
            [group0, group1, group1_secure] => [group0, group1, group1_secure],
            _ => {
                return Err(
                    "GICv3 checkpoint: an interface has neither two nor three group enables".into(),
                );
            }
        };
        Ok(Self {
            system_register_enable: record.system_register_enable,
            hypervisor_enable: record.hypervisor_enable,
            enabled,
            pmr: record.pmr,
            bpr: record.bpr,
            split_eoi: record.split_eoi,
            asleep: record.asleep,
            active: record.active,
            security: record.security,
        })
    }
}
impl Default for Interface {
    fn default() -> Self {
        Self {
            system_register_enable: false,
            hypervisor_enable: false,
            enabled: [false; 3],
            pmr: 0,
            bpr: [0; 2],
            split_eoi: false,
            asleep: true,
            active: vec![],
            security: ProcessorSecurity::RESET,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct State {
    /// Absent from checkpoints taken before two Security states: one state.
    #[serde(default)]
    security_states: SecurityStates,
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
    // Derived per-group candidates, excluded from architectural/checkpoint
    // state.
    eligible: Vec<[Option<usize>; 3]>,
    /// Scratch of `refresh`; no state.
    candidates: Vec<usize>,
}
impl Gic {
    pub fn new(
        affinities: Vec<u32>,
        lines: usize,
        security_states: SecurityStates,
    ) -> Result<Self, String> {
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
                security_states,
                affinities,
                distributor: 0,
                private,
                shared: vec![Line::default(); lines],
                routes: vec![0; lines],
                interfaces: vec![Interface::default(); cores],
            },
            eligible: vec![[None; 3]; cores],
            candidates: Vec::new(),
        })
    }
    fn two_states(&self) -> bool {
        self.state.security_states == SecurityStates::Two
    }
    /// The Group 1 bank ICC_*1_EL1 accesses address: SCR_EL3.NS selects it
    /// with two Security states.
    fn group1(&self, core: usize) -> usize {
        if self.two_states() && !self.state.interfaces[core].security.non_secure {
            GROUP1_SECURE
        } else {
            GROUP1_NON_SECURE
        }
    }
    fn refuse_non_secure(&self, core: usize, what: &str) -> Result<(), String> {
        if self.two_states() && self.state.interfaces[core].security.non_secure {
            Err(format!(
                "GICv3 Non-secure {what} with two Security states is not implemented"
            ))
        } else {
            Ok(())
        }
    }
    fn refuse_el3(&self, core: usize) -> Result<(), String> {
        if self.two_states() && self.state.interfaces[core].security.el3 {
            Err("GICv3 EL3 acknowledgment with two Security states is not implemented".into())
        } else {
            Ok(())
        }
    }
    /// Distributor group-enable bits written through GICD_CTLR.
    fn distributor_enables(&self) -> u32 {
        if self.two_states() { 7 } else { 3 }
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
        // Shared lines that can be eligible for some processor, in ID order.
        // The per-processor search below applies its whole condition to
        // them and to every private line, so it finds what a search of all
        // lines finds.
        let mut candidates = std::mem::take(&mut self.candidates);
        candidates.clear();
        candidates.extend(
            self.state
                .shared
                .iter()
                .enumerate()
                .skip(32)
                .filter(|(_, line)| line.enabled && !line.active && line.pending())
                .map(|(id, _)| id),
        );
        self.refresh_from(&candidates);
        self.candidates = candidates;
    }
    fn refresh_from(&mut self, candidates: &[usize]) {
        for core in 0..self.eligible.len() {
            for group in [GROUP0, GROUP1_NON_SECURE, GROUP1_SECURE] {
                let interface = &self.state.interfaces[core];
                let shift = interface.bpr[group.min(1)] + 1;
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
                    (0..self.state.shared.len().min(32))
                        .chain(candidates.iter().copied())
                        .filter(|&id| {
                            let line = self.line(core, id);
                            line.enabled
                                && line.group() == group
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
    /// Whether an SGI (ids 0..16) is latched for `core`, whatever its enable or priority.
    /// Wakes a core that waits for a software interrupt before its own CPU interface runs.
    pub fn sgi_pending(&self, core: usize) -> Result<bool, String> {
        self.core(core)?;
        Ok((0..16).any(|id| self.state.private[core][id].pending()))
    }
    /// The (IRQ, FIQ) outputs. This never acknowledges an ID.
    pub fn pins(&self, core: usize) -> Result<(bool, bool), String> {
        self.core(core)?;
        let [group0, non_secure, secure] = self.eligible[core].map(|id| id.is_some());
        if !self.two_states() {
            return Ok((non_secure, group0));
        }
        let security = self.state.interfaces[core].security;
        Ok(if security.el3 {
            (false, group0 || non_secure || secure)
        } else if security.non_secure {
            (non_secure, group0 || secure)
        } else {
            (secure, group0 || non_secure)
        })
    }
    /// The processor's current Security state, from the processor framework.
    pub fn set_processor_security(
        &mut self,
        core: usize,
        security: ProcessorSecurity,
    ) -> Result<(), String> {
        self.core(core)?;
        if !self.two_states() {
            return Err("GICv3 has one Security state".into());
        }
        // Candidates are per group, so only the pin mapping changes.
        self.state.interfaces[core].security = security;
        Ok(())
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
        // With two Security states an SGI register forwards only to targets
        // configured in the group it generates (no GICR_NSACR grants).
        let generated = if group0_only {
            GROUP0
        } else {
            self.group1(core)
        };
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
            let line = &self.state.private[target][id];
            let forwarded = if self.two_states() {
                line.group() == generated
            } else {
                !group0_only || !line.group1
            };
            if selected && forwarded {
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
                Ok(7 | (u64::from(self.state.interfaces[core].system_register_enable) * 8))
            }
            0xe64d => {
                if let Some(v) = value {
                    if v & !15 != 0 {
                        return Err("unsupported GICv3 ICC_SRE_EL2 bits".into());
                    }
                    self.state.interfaces[core].hypervisor_enable = v & 8 != 0;
                }
                // As ICC_SRE_EL3: SRE/DIB/DFB are RAO/WI; Enable is retained per core.
                Ok(7 | (u64::from(self.state.interfaces[core].hypervisor_enable) * 8))
            }
            0xf667 => {
                // ICC_IGRPEN1_EL3: EnableGrp1NS (bit 0) and EnableGrp1S (bit 1).
                // EnableGrp1NS is the Group 1 enable ICC_IGRPEN1_EL1 also
                // addresses with one Security state, so an EL1 and an EL3
                // access agree.
                let enabled = &mut self.state.interfaces[core].enabled;
                if let Some(word) = value {
                    if word & !3 != 0 {
                        return Err("unsupported GICv3 ICC_IGRPEN1_EL3 bits".into());
                    }
                    enabled[GROUP1_NON_SECURE] = word & 1 != 0;
                    enabled[GROUP1_SECURE] = word & 2 != 0;
                }
                Ok(
                    u64::from(enabled[GROUP1_NON_SECURE])
                        | (u64::from(enabled[GROUP1_SECURE]) << 1),
                )
            }
            0xc230 => {
                if let Some(v) = value {
                    self.state.interfaces[core].pmr = v as u8;
                }
                Ok(self.state.interfaces[core].pmr.into())
            }
            0xc643 | 0xc663 => {
                let g = usize::from(register == 0xc663);
                self.refuse_non_secure(
                    core,
                    if g == 0 {
                        "Group 0 ICC access"
                    } else {
                        "ICC_BPR1_EL1"
                    },
                )?;
                if let Some(v) = value {
                    self.state.interfaces[core].bpr[g] = (v & 7) as u8;
                }
                Ok(self.state.interfaces[core].bpr[g].into())
            }
            0xc664 => {
                self.refuse_non_secure(core, "ICC_CTLR_EL1")?;
                if let Some(v) = value {
                    if v & !(0x700 | 2) != 0 {
                        return Err("unsupported GICv3 ICC_CTLR mode".into());
                    }
                    self.state.interfaces[core].split_eoi = v & 2 != 0;
                }
                Ok(0x700 | (u64::from(self.state.interfaces[core].split_eoi) * 2))
            }
            0xc665 => Ok(7), // System interface only; SRE/DIB/DFB are RAO/WI.
            0xc666 | 0xc667 => {
                let g = self.register_group(core, register == 0xc667)?;
                if let Some(v) = value {
                    self.state.interfaces[core].enabled[g] = v & 1 != 0;
                }
                Ok(self.state.interfaces[core].enabled[g].into())
            }
            0xc640 | 0xc660 if value.is_none() => {
                let g = self.register_group(core, register == 0xc660)?;
                self.refuse_el3(core)?;
                Ok(self.acknowledge(core, g))
            }
            0xc642 | 0xc662 if value.is_none() => {
                let g = self.register_group(core, register == 0xc662)?;
                self.refuse_el3(core)?;
                Ok(self.eligible[core][g].unwrap_or(1023) as u64)
            }
            0xc641 | 0xc661 if value.is_some() => {
                let g = self.register_group(core, register == 0xc661)?;
                self.refuse_el3(core)?;
                self.eoi(core, g, (value.unwrap() & 0xffffff) as usize)?;
                Ok(0)
            }
            0xc659 if value.is_some() => {
                self.refuse_el3(core)?;
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
                if register == 0xc65f {
                    self.refuse_non_secure(core, "Group 0 ICC access")?;
                }
                self.sgi(core, register == 0xc65f, value.unwrap())?;
                Ok(0)
            }
            // Other-security SGIs require an explicit security-bank model.
            0xc644..=0xc64b => {
                let group = self.register_group(core, register >= 0xc648)?;
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
    /// The group an ICC_*0_EL1 or ICC_*1_EL1 register addresses.
    fn register_group(&self, core: usize, group1: bool) -> Result<usize, String> {
        if group1 {
            Ok(self.group1(core))
        } else {
            self.refuse_non_secure(core, "Group 0 ICC access")?;
            Ok(GROUP0)
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
        if !matches!(size, 1 | 2 | 4 | 8) || !offset.is_multiple_of(u64::from(size)) {
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
                            if self.two_states() && v & (1 << 6) != 0 {
                                return Err("GICv3 GICD_CTLR.DS switch is not implemented".into());
                            }
                            self.state.distributor = v as u32 & self.distributor_enables();
                        }
                        // Affinity routing only: ARE (ARE_S/ARE_NS) is RAO/WI;
                        // DS reads as the declared Security states.
                        let fixed = if self.two_states() { 0x30 } else { 0x50 };
                        return Ok(fixed | u64::from(self.state.distributor));
                    }
                    // 16-bit IDs, no LPIs; SecurityExtn reports two states.
                    4 => {
                        return Ok((self.state.shared.len().div_ceil(32) - 1) as u64
                            | (0xf << 19)
                            | u64::from(self.two_states()) << 10);
                    }
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
            // The legacy target interface is RAZ/WI with affinity routing.
            // Group modifiers are RAZ/WI with one Security state, and
            // non-secure access control and GICD_SGIR are always RAZ/WI.
            let modifiers = self.two_states() && (0xd00..0xd80).contains(&offset);
            if (0x800..0xc00).contains(&offset) && matches!(size, 1 | 2 | 4)
                || (0xd00..0xf04).contains(&offset) && size == 4 && !modifiers
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
                if let Some(v) = value
                    && id >= 16
                {
                    line.edge = v & (2 << (n * 2)) != 0;
                }
                out |= (u64::from(line.edge) * 2) << (n * 2);
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
            (0xd00, 7),
        ] {
            if !(start..start + limit / 8).contains(&offset) || kind == 7 && !self.two_states() {
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
                    } else if kind == 7 {
                        line.modifier = v & (1 << n) != 0;
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
                    7 => line.modifier,
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
        let groups = if s.security_states == SecurityStates::Two {
            3
        } else {
            2
        };
        if s.security_states != self.state.security_states
            || s.affinities != self.state.affinities
            || s.shared.len() != lines
            || s.routes.len() != lines
            || s.private.len() != cores
            || s.private.iter().any(|p| p.len() != 32)
            || s.interfaces.len() != cores
            || s.distributor & !self.distributor_enables() != 0
            || s.interfaces.iter().any(|i| {
                i.bpr.iter().any(|b| *b > 7)
                    || i.active.iter().any(|a| a.id >= lines || a.group >= groups)
            })
            || groups == 2
                && (s.shared.iter().chain(s.private.iter().flatten())).any(|l| l.modifier)
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

impl fujimod_emulator_devices::gic::InterruptController for Gic {
    fn pins(&self, core: usize) -> Result<(bool, bool), String> {
        Gic::pins(self, core)
    }
    fn eligible(&self, core: usize) -> Result<Option<usize>, String> {
        let groups = self.eligible.get(core).ok_or("GICv3 output targets absent processor")?;
        Ok(groups.iter().flatten().next().copied())
    }
    fn sgi_pending(&self, core: usize) -> Result<bool, String> {
        Gic::sgi_pending(self, core)
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
    fn tracks_processor_security(&self) -> bool {
        self.two_states()
    }
    fn set_processor_security(
        &mut self,
        core: usize,
        security: ProcessorSecurity,
    ) -> Result<(), String> {
        Gic::set_processor_security(self, core, security)
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

#[cfg(test)]
mod tests {
    #[test]
    fn a_checkpoint_from_before_two_security_states_restores() {
        let mut g = super::Gic::new(vec![0, 1], 128, super::SecurityStates::One).unwrap();
        let mut saved = g.save();
        saved["interfaces"][0]["enabled"] = serde_json::json!([true, true, true]);
        let current = saved.clone();
        // The earlier layout: no Security-state fields, two group enables and the
        // Secure Group 1 enable on its own.
        let mut old = saved;
        old.as_object_mut().unwrap().remove("security_states");
        for line in old["shared"].as_array_mut().unwrap() {
            line.as_object_mut().unwrap().remove("modifier");
        }
        for core in old["private"].as_array_mut().unwrap() {
            for line in core.as_array_mut().unwrap() {
                line.as_object_mut().unwrap().remove("modifier");
            }
        }
        for interface in old["interfaces"].as_array_mut().unwrap() {
            let enabled = interface["enabled"].as_array().unwrap().clone();
            let object = interface.as_object_mut().unwrap();
            object.remove("security");
            object.insert(
                "enabled".into(),
                serde_json::json!([enabled[0], enabled[1]]),
            );
            object.insert("el3_group1_secure".into(), enabled[2].clone());
        }
        g.restore(&old).unwrap();
        assert_eq!(g.save(), current);
        old["interfaces"][0]["enabled"] = serde_json::json!([true]);
        assert!(g.restore(&old).is_err());
    }
    #[test]
    fn el3_system_interface_enable_is_per_core_and_survives_checkpoint() {
        let mut g = super::Gic::new(vec![0, 1], 128, super::SecurityStates::One).unwrap();
        assert_eq!(g.system(0, 0xf665, None).unwrap(), 7);
        assert_eq!(g.system(0, 0xf665, Some(9)).unwrap(), 15);
        assert_eq!(g.system(1, 0xf665, None).unwrap(), 7);
        let saved = g.save();
        g.system(0, 0xf665, Some(0)).unwrap();
        g.restore(&saved).unwrap();
        assert_eq!(g.system(0, 0xf665, None).unwrap(), 15);
        assert!(g.system(0, 0xf665, Some(16)).is_err());
    }
    #[test]
    fn el2_sre_and_el3_group_enables_are_per_core_and_survive_checkpoint() {
        let mut g = super::Gic::new(vec![0, 1], 128, super::SecurityStates::One).unwrap();
        assert_eq!(g.system(0, 0xe64d, None).unwrap(), 7);
        assert_eq!(g.system(0, 0xe64d, Some(0xf)).unwrap(), 15);
        assert_eq!(g.system(1, 0xe64d, None).unwrap(), 7);
        assert_eq!(g.system(0, 0xf667, Some(3)).unwrap(), 3);
        assert_eq!(g.system(1, 0xf667, None).unwrap(), 0);
        // Bit 0 is the Group 1 enable the EL1 register also sets and clears.
        assert_eq!(g.system(0, 0xc667, None).unwrap(), 1);
        g.system(0, 0xc667, Some(0)).unwrap();
        assert_eq!(g.system(0, 0xf667, None).unwrap(), 2);
        g.system(0, 0xf667, Some(3)).unwrap();
        assert_eq!(g.system(0, 0xc667, None).unwrap(), 1);
        let saved = g.save();
        g.system(0, 0xe64d, Some(0)).unwrap();
        g.system(0, 0xf667, Some(0)).unwrap();
        g.restore(&saved).unwrap();
        assert_eq!(g.system(0, 0xe64d, None).unwrap(), 15);
        assert_eq!(g.system(0, 0xf667, None).unwrap(), 3);
        assert!(g.system(0, 0xe64d, Some(16)).is_err());
        assert!(g.system(0, 0xf667, Some(4)).is_err());
    }
    use super::*;
    fn enabled(affinities: Vec<u32>) -> Gic {
        let mut g = Gic::new(affinities, 512, SecurityStates::One).unwrap();
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
    fn sgi_pending_reports_a_latched_sgi_even_with_the_cpu_interface_disabled() {
        let mut g = enabled(vec![0, 1, 2, 3, 4]);
        for core in 0..5 {
            assert!(!g.sgi_pending(core).unwrap());
        }
        g.system(0, 0xc65f, Some(0xe)).unwrap();
        for core in 0..5 {
            assert_eq!(g.sgi_pending(core).unwrap(), (1..=3).contains(&core));
        }
    }
    #[test]
    fn group_zero_sgi_targets_only_selected_secondary_affinities() {
        let mut g = enabled(vec![0, 1, 2, 3, 4]);
        for core in 0..5 {
            g.redistributor(core, 0x10100, 4, Some(1)).unwrap();
            g.redistributor(core, 0x10400, 1, Some(0x40)).unwrap();
        }
        g.system(0, 0xc65f, Some(0xe)).unwrap();
        for core in 0..5 {
            assert_eq!(g.pins(core).unwrap(), (false, (1..=3).contains(&core)));
        }
        for core in 1..=3 {
            assert_eq!(g.system(core, 0xc640, None).unwrap(), 0);
            g.system(core, 0xc641, Some(0)).unwrap();
            assert_eq!(g.pins(core).unwrap(), (false, false));
        }
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
        let mut restored = Gic::new(vec![0, 1], 512, SecurityStates::One).unwrap();
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
        assert!(
            Gic::new(vec![1, 0], 512, SecurityStates::One)
                .unwrap()
                .restore(&saved)
                .is_err()
        );
        assert!(g.distributor(0x6100, 8, Some(1 << 31)).is_err());
        assert!(g.system(0, 0xc65e, Some(0)).is_err());
        assert!(g.redistributor(0, 0, 4, Some(1)).is_err());
        // Architecture-reserved and unimplemented trailing IDs are RAZ/WI.
        assert_eq!(g.distributor(0x17c, 4, Some(u32::MAX.into())).unwrap(), 0);
        assert_eq!(g.distributor(0x880, 4, Some(u32::MAX.into())).unwrap(), 0);
    }
    const SECURE_EL1: ProcessorSecurity = ProcessorSecurity {
        el3: false,
        non_secure: false,
    };
    const NON_SECURE_EL1: ProcessorSecurity = ProcessorSecurity {
        el3: false,
        non_secure: true,
    };
    /// Two Security states, initialized at EL3 with SCR_EL3.NS clear.
    fn two_states(affinities: Vec<u32>) -> Gic {
        let mut g = Gic::new(affinities, 512, SecurityStates::Two).unwrap();
        assert_eq!(g.distributor(0, 4, Some(0x37)).unwrap(), 0x37);
        for core in 0..g.state.affinities.len() {
            g.redistributor(core, 0x14, 4, Some(0)).unwrap();
            g.system(core, 0xf665, Some(15)).unwrap();
            g.system(core, 0xc230, Some(255)).unwrap();
            g.system(core, 0xc666, Some(1)).unwrap();
            g.system(core, 0xc667, Some(1)).unwrap();
            assert_eq!(g.system(core, 0xf667, Some(3)).unwrap(), 3);
        }
        g
    }
    /// Configure an SPI's IGROUPR and IGRPMODR bits, then enable it.
    fn grouped_spi(g: &mut Gic, id: usize, group1: bool, modifier: bool) {
        spi(g, id, group1, 0x80, 0);
        let word = 0xd00 + (id / 32) as u64 * 4;
        let old = g.distributor(word, 4, None).unwrap();
        let bit = 1 << (id % 32);
        g.distributor(word, 4, Some(if modifier { old | bit } else { old & !bit }))
            .unwrap();
        assert_eq!(g.distributor(word, 4, None).unwrap() & bit != 0, modifier);
    }
    #[test]
    fn secure_group1_signals_irq_to_secure_el1_and_is_acknowledged_by_iar1() {
        let mut g = two_states(vec![0]);
        grouped_spi(&mut g, 40, false, true);
        g.set_spi(40, true).unwrap();
        // At EL3 every group signals FIQ, and acknowledgment is refused.
        assert_eq!(g.pins(0).unwrap(), (false, true));
        assert!(g.system(0, 0xc660, None).is_err());
        g.set_processor_security(0, SECURE_EL1).unwrap();
        assert_eq!(g.pins(0).unwrap(), (true, false));
        assert_eq!(g.system(0, 0xc642, None).unwrap(), 1023);
        assert_eq!(g.system(0, 0xc640, None).unwrap(), 1023);
        assert_eq!(g.system(0, 0xc662, None).unwrap(), 40);
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 40);
        assert!(g.system(0, 0xc641, Some(40)).is_err()); // Group mismatch.
        g.set_spi(40, false).unwrap();
        g.system(0, 0xc661, Some(40)).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, false));
        // The Secure Group 1 enable gates only that group.
        g.set_spi(40, true).unwrap();
        g.system(0, 0xc667, Some(0)).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, false));
        g.set_processor_security(0, ProcessorSecurity::RESET)
            .unwrap();
        assert_eq!(g.system(0, 0xf667, None).unwrap(), 1);
    }
    #[test]
    fn group0_signals_fiq_and_is_acknowledged_by_iar0() {
        let mut g = two_states(vec![0]);
        g.set_processor_security(0, SECURE_EL1).unwrap();
        grouped_spi(&mut g, 41, false, false);
        g.set_spi(41, true).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, true));
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 1023);
        assert_eq!(g.system(0, 0xc640, None).unwrap(), 41);
        g.set_spi(41, false).unwrap();
        g.system(0, 0xc641, Some(41)).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, false));
        // Non-secure Group 0 access is outside the model.
        g.set_processor_security(0, NON_SECURE_EL1).unwrap();
        assert!(g.system(0, 0xc640, None).is_err());
        assert!(g.system(0, 0xc666, Some(1)).is_err());
    }
    #[test]
    fn non_secure_group1_signals_fiq_to_secure_el1_and_irq_to_non_secure_el1() {
        let mut g = two_states(vec![0]);
        g.set_processor_security(0, SECURE_EL1).unwrap();
        grouped_spi(&mut g, 42, true, false);
        // The reserved (1,1) encoding is Non-secure Group 1.
        grouped_spi(&mut g, 43, true, true);
        g.set_spi(42, true).unwrap();
        assert_eq!(g.pins(0).unwrap(), (false, true));
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 1023);
        assert_eq!(g.system(0, 0xc640, None).unwrap(), 1023);
        g.set_processor_security(0, NON_SECURE_EL1).unwrap();
        assert_eq!(g.pins(0).unwrap(), (true, false));
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 42);
        g.set_spi(43, true).unwrap();
        g.set_spi(42, false).unwrap();
        g.system(0, 0xc661, Some(42)).unwrap();
        assert_eq!(g.system(0, 0xc660, None).unwrap(), 43);
        // Banked Group 1 registers keep one Secure copy.
        assert!(g.system(0, 0xc664, None).is_err());
        assert!(g.system(0, 0xc663, None).is_err());
    }
    #[test]
    fn sgis_forward_only_to_the_generated_group_with_two_states() {
        let mut g = two_states(vec![0, 1]);
        for core in 0..2 {
            g.set_processor_security(core, SECURE_EL1).unwrap();
            g.redistributor(core, 0x10100, 4, Some(3)).unwrap();
            // SGI 0 is Secure Group 1; SGI 1 is Non-secure Group 1.
            g.redistributor(core, 0x10080, 4, Some(2)).unwrap();
            g.redistributor(core, 0x10d00, 4, Some(1)).unwrap();
            assert_eq!(g.redistributor(core, 0x10d00, 4, None).unwrap(), 1);
        }
        g.system(0, 0xc65d, Some(2)).unwrap();
        g.system(0, 0xc65d, Some((1 << 24) | 2)).unwrap();
        assert_eq!(g.pins(1).unwrap(), (true, false));
        assert_eq!(g.system(1, 0xc660, None).unwrap(), 0);
        g.system(1, 0xc661, Some(0)).unwrap();
        assert_eq!(g.pins(1).unwrap(), (false, false));
    }
    #[test]
    fn security_configuration_is_declared_and_checkpointed() {
        let mut one = enabled(vec![0]);
        assert_eq!(one.distributor(0, 4, Some(0x77)).unwrap(), 0x53);
        assert_eq!(one.distributor(4, 4, None).unwrap() & (1 << 10), 0);
        assert_eq!(one.distributor(0xd04, 4, Some(u32::MAX.into())).unwrap(), 0);
        assert_eq!(
            one.redistributor(0, 0x10d00, 4, Some(u32::MAX.into()))
                .unwrap(),
            0
        );
        assert!(one.set_processor_security(0, SECURE_EL1).is_err());
        let mut g = two_states(vec![0]);
        assert_ne!(g.distributor(4, 4, None).unwrap() & (1 << 10), 0);
        assert!(g.distributor(0, 4, Some(0x77)).is_err());
        grouped_spi(&mut g, 40, false, true);
        g.set_processor_security(0, SECURE_EL1).unwrap();
        g.set_spi(40, true).unwrap();
        let saved = g.save();
        let encoded = fujimod_emulator_devices::checkpoint::Checkpoint::encode("gic", &g).unwrap();
        let mut decoded: Gic = encoded.decode("gic").unwrap();
        assert_eq!(decoded.save(), saved);
        assert_eq!(decoded.pins(0).unwrap(), (true, false));
        assert_eq!(decoded.system(0, 0xc660, None).unwrap(), 40);
        assert!(
            Gic::new(vec![0], 512, SecurityStates::One)
                .unwrap()
                .restore(&saved)
                .is_err()
        );
        assert!(one.restore(&saved).is_err());
    }
}
