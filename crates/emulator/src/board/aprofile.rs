//! A-profile architectural counters and timers, driven by the board timeline.
pub mod cortex_a53;
pub mod generic_timer;
mod instructions;
use generic_timer::{Clock, Register, Timer};
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CortexA53 {
    pub cores: usize,
    pub peripheral_base: u64,
}
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub clock: Clock,
    pub physical_irq: u32,
    pub virtual_irq: u32,
    pub pmu_hz: u64,
    pub implementation: Option<CortexA53>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    config: Config,
    physical: Timer,
    virtual_timer: Timer,
    kernel_control: u32,
    pmu: Pmu,
    implementation: cortex_a53::Registers,
}
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Pmu {
    control: u32,
    enabled: bool,
    offset: u64,
    anchor: u64,
    user: u32,
    overflow: bool,
}
impl Pmu {
    fn raw(&self, now: u64, config: Config) -> Result<u128, String> {
        let elapsed = now
            .checked_sub(self.anchor)
            .ok_or("PMU clock moved backward")?;
        let cycles = if self.control & 1 != 0 && self.enabled {
            (elapsed as u128 * config.pmu_hz as u128 / config.clock.ticks_per_second as u128)
                / if self.control & 8 != 0 { 64 } else { 1 }
        } else {
            0
        };
        Ok(self.offset as u128 + cycles)
    }
    fn value(&self, now: u64, config: Config) -> Result<u64, String> {
        Ok(self.raw(now, config)? as u64)
    }
    fn overflow(&self, now: u64, config: Config) -> Result<bool, String> {
        let raw = self.raw(now, config)?;
        Ok(self.overflow
            || if self.control & 0x40 != 0 {
                raw > u64::MAX as u128
            } else {
                raw >> 32 != (self.offset as u128) >> 32
            })
    }
    fn access(
        &mut self,
        r: u16,
        now: u64,
        config: Config,
        value: Option<u64>,
    ) -> Result<Option<u64>, String> {
        if !matches!(r, 0xdce0..=0xdce8 | 0xdcf0) {
            return Ok(None);
        }
        if let Some(value) = value {
            let current = self.value(now, config)?;
            let overflow = self.overflow(now, config)?;
            match r {
                0xdce0 => {
                    // Cycle count enable, reset, /64 and long-cycle mode follow
                    // PMCR. Event-counter operations remain unsupported.
                    if value & !0x4f != 0 {
                        return Err("unsupported PMCR control".into());
                    }
                    self.control = (value & 0x49) as u32;
                    self.offset = if value & 4 != 0 { 0 } else { current };
                }
                0xdce1 => {
                    if value & !0x8000_0000 != 0 {
                        return Err("PMU event counters are not modeled".into());
                    }
                    self.enabled |= value & 0x8000_0000 != 0;
                    self.offset = current;
                }
                0xdce2 => {
                    if value & !0x8000_0000 != 0 {
                        return Err("PMU event counters are not modeled".into());
                    }
                    self.enabled &= value & 0x8000_0000 == 0;
                    self.offset = current;
                }
                0xdce3 => self.offset = current,
                0xdce8 => self.offset = value,
                0xdcf0 => {
                    if value & !0xf != 0 {
                        return Err("invalid PMUSERENR".into());
                    }
                    self.user = value as u32;
                    self.offset = current;
                }
                _ => return Err(format!("unsupported PMU write {r:#x}")),
            }
            self.overflow = overflow && !(r == 0xdce3 && value & 0x8000_0000 != 0);
            self.anchor = now;
        }
        Ok(Some(match r {
            0xdce0 => {
                (if config.implementation.is_some() {
                    0x4103_3000
                } else {
                    0x4100_0000
                }) | self.control as u64
            }
            0xdce1 | 0xdce2 => {
                if self.enabled {
                    0x8000_0000
                } else {
                    0
                }
            }
            0xdce3 => {
                if self.overflow(now, config)? {
                    0x8000_0000
                } else {
                    0
                }
            }
            0xdce6 => {
                if config.implementation.is_some() {
                    0x67ff_bfff
                } else {
                    0
                }
            }
            0xdce7 => 0,
            0xdce8 => self.value(now, config)?,
            0xdcf0 => self.user as u64,
            _ => return Err(format!("unsupported PMU read {r:#x}")),
        }))
    }
}
impl Profile {
    pub(super) fn validate_checkpoint(&self, saved: &Self, now: u64) -> Result<(), String> {
        if self.config != saved.config || saved.pmu.anchor > now {
            return Err("A-profile snapshot configuration or clock differs".into());
        }
        saved.config.clock.validate()?;
        Ok(())
    }
    pub fn new(config: Config) -> Result<Self, String> {
        config.clock.validate()?;
        if !(16..32).contains(&config.physical_irq)
            || !(16..32).contains(&config.virtual_irq)
            || config.physical_irq == config.virtual_irq
            || config.pmu_hz == 0
            || config.pmu_hz > 10_000_000_000
        {
            return Err("invalid A-profile architectural timer/PMU configuration".into());
        }
        if config
            .implementation
            .is_some_and(|c| !(1..=4).contains(&c.cores) || c.peripheral_base & 0x3ffff != 0)
        {
            return Err("invalid Cortex-A53 implementation configuration".into());
        }
        Ok(Self {
            config,
            physical: Timer::default(),
            virtual_timer: Timer::default(),
            kernel_control: 0,
            pmu: Pmu::default(),
            implementation: Default::default(),
        })
    }
    pub fn implementation_access(
        &mut self,
        cluster: &mut cortex_a53::Registers,
        r: u16,
        value: Option<u64>,
    ) -> Result<Option<u64>, String> {
        let Some(config) = self.config.implementation else {
            return Ok(None);
        };
        if !matches!(r, 0xcf80 | 0xcf90..=0xcf93 | 0xce82 | 0xce83 | 0xcf98) {
            return Ok(None);
        }
        let bank = if matches!(r, 0xcf80 | 0xce82 | 0xce83 | 0xcf93) {
            cluster
        } else {
            &mut self.implementation
        };
        if let Some(v) = value {
            bank.write(r, v, config.cores, config.peripheral_base)?;
        }
        bank.read(r, config.cores, config.peripheral_base).map(Some)
    }
    pub fn access(&mut self, r: u16, now: u64, value: Option<u64>) -> Result<Option<u64>, String> {
        let count = self.config.clock.count(now)?;
        let timer = match r {
            0xdf10..=0xdf12 => Some((&mut self.physical, r - 0xdf10)),
            0xdf18..=0xdf1a => Some((&mut self.virtual_timer, r - 0xdf18)),
            _ => None,
        };
        if let Some((timer, which)) = timer {
            let reg = match which {
                0 => Register::Value,
                1 => Register::Control,
                _ => Register::Compare,
            };
            return timer.access(reg, count, value).map(Some);
        }
        match r {
            0xdf00 => {
                if let Some(v) = value
                    && v != self.config.clock.counter_hz
                {
                    return Err("CNTFRQ write differs from declared counter clock".into());
                }
                Ok(Some(self.config.clock.counter_hz))
            }
            0xdf01 | 0xdf02 => {
                if value.is_some() {
                    return Err("architectural counter is read-only".into());
                }
                Ok(Some(count))
            }
            0xc708 => {
                if let Some(v) = value {
                    // EL0 counter/timer permissions are stored; event-stream
                    // generation needs its own architectural event deadlines.
                    if v & !0x303 != 0 {
                        return Err("CNTKCTL event stream is not modeled".into());
                    }
                    self.kernel_control = v as u32;
                }
                Ok(Some(self.kernel_control as u64))
            }
            _ => self.pmu.access(r, now, self.config, value),
        }
    }
    pub fn next_deadline(&self, now: u64) -> Result<Option<u64>, String> {
        Ok(self
            .physical
            .next_deadline(self.config.clock, now)?
            .into_iter()
            .chain(self.virtual_timer.next_deadline(self.config.clock, now)?)
            .min())
    }
    pub fn outputs(&self, now: u64) -> Result<[(u32, bool); 2], String> {
        let count = self.config.clock.count(now)?;
        Ok([
            (self.config.physical_irq, self.physical.asserted(count)),
            (self.config.virtual_irq, self.virtual_timer.asserted(count)),
        ])
    }
    pub fn state(&self, now: u64) -> Result<Value, String> {
        Ok(
            json!({"counter":self.config.clock.count(now)?,"clock":self.config.clock,
            "physical":self.physical,"virtual":self.virtual_timer,"kernel_control":self.kernel_control,
            "pmu":{"control":self.pmu.control,"enabled":self.pmu.enabled,"count":self.pmu.value(now,self.config)?,"user":self.pmu.user,"overflow":self.pmu.overflow(now,self.config)?},
            "implementation":self.implementation}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config {
            clock: Clock {
                counter_hz: 3,
                ticks_per_second: 10,
            },
            physical_irq: 30,
            virtual_irq: 27,
            pmu_hz: 100,
            implementation: Some(CortexA53 {
                cores: 4,
                peripheral_base: 0xfa000000,
            }),
        }
    }
    #[test]
    fn physical_and_virtual_timers_are_independent_and_observations_are_pure() {
        let mut p = Profile::new(config()).unwrap();
        p.access(0xdf12, 0, Some(3)).unwrap();
        p.access(0xdf11, 0, Some(1)).unwrap();
        p.access(0xdf1a, 0, Some(2)).unwrap();
        p.access(0xdf19, 0, Some(1)).unwrap();
        assert_eq!(p.next_deadline(0).unwrap(), Some(7));
        assert_eq!(p.outputs(6).unwrap(), [(30, false), (27, false)]);
        assert_eq!(p.outputs(7).unwrap(), [(30, false), (27, true)]);
        assert_eq!(p.next_deadline(7).unwrap(), Some(10));
        let before = p.state(7).unwrap();
        assert_eq!(p.state(7).unwrap(), before);
        p.access(0xdf19, 7, Some(3)).unwrap();
        assert_eq!(p.outputs(7).unwrap(), [(30, false), (27, false)]);
        assert_eq!(p.access(0xdf19, 7, None).unwrap(), Some(7));
        assert!(p.access(0xdf00, 7, Some(4)).is_err());
        assert!(p.access(0xdf01, 7, Some(0)).is_err());
        assert!(p.access(0xc708, 7, Some(4)).is_err());
    }
    #[test]
    fn cycle_counter_gates_reset_divide_and_writes_use_guest_time() {
        let mut p = Profile::new(config()).unwrap();
        assert_eq!(p.access(0xdce0, 0, None).unwrap(), Some(0x41033000));
        assert_eq!(p.access(0xdce6, 0, None).unwrap(), Some(0x67ffbfff));
        assert_eq!(p.access(0xdce8, 10, None).unwrap(), Some(0));
        p.access(0xdce0, 10, Some(1)).unwrap();
        p.access(0xdce1, 10, Some(0x80000000)).unwrap();
        assert_eq!(p.access(0xdce8, 20, None).unwrap(), Some(100));
        p.access(0xdcf0, 20, Some(1)).unwrap();
        assert_eq!(p.access(0xdce8, 21, None).unwrap(), Some(110));
        p.access(0xdce2, 21, Some(0x80000000)).unwrap();
        assert_eq!(p.access(0xdce8, 30, None).unwrap(), Some(110));
        p.access(0xdce0, 30, Some(0xd)).unwrap(); // enable + reset + /64
        p.access(0xdce1, 30, Some(0x80000000)).unwrap();
        assert_eq!(p.access(0xdce8, 94, None).unwrap(), Some(10));
        p.access(0xdce8, 94, Some(77)).unwrap();
        assert_eq!(p.access(0xdce8, 158, None).unwrap(), Some(87));
        assert!(p.access(0xdce1, 158, Some(1)).is_err());
    }
    #[test]
    fn cycle_counter_stays_64_bit_while_lc_selects_the_overflow_boundary() {
        let mut p = Profile::new(config()).unwrap();
        p.access(0xdce0, 0, Some(1)).unwrap();
        p.access(0xdce1, 0, Some(0x80000000)).unwrap();
        p.access(0xdce8, 0, Some(u32::MAX as u64)).unwrap();
        assert_eq!(
            p.access(0xdce8, 1, None).unwrap(),
            Some(u32::MAX as u64 + 10)
        );
        assert_eq!(p.access(0xdce3, 1, None).unwrap(), Some(0x80000000));
        let saved = p.state(1).unwrap();
        assert_eq!(p.state(1).unwrap(), saved);
        p.access(0xdce3, 1, Some(0x80000000)).unwrap();
        assert_eq!(p.access(0xdce3, 2, None).unwrap(), Some(0));
        p.access(0xdce0, 2, Some(0x41)).unwrap();
        p.access(0xdce8, 2, Some(u32::MAX as u64)).unwrap();
        assert_eq!(
            p.access(0xdce8, 3, None).unwrap(),
            Some(u32::MAX as u64 + 10)
        );
        assert_eq!(p.access(0xdce3, 3, None).unwrap(), Some(0));
        p.access(0xdce8, 3, Some(u64::MAX - 4)).unwrap();
        assert_eq!(p.access(0xdce8, 4, None).unwrap(), Some(5));
        assert_eq!(p.access(0xdce3, 4, None).unwrap(), Some(0x80000000));
        p.access(0xdce0, 4, Some(0x45)).unwrap();
        assert_eq!(p.access(0xdce3, 4, None).unwrap(), Some(0x80000000)); // reset does not clear overflow
    }
    #[test]
    fn a53_cluster_registers_share_state_and_core_registers_do_not() {
        let mut cluster = cortex_a53::Registers::default();
        let mut p = Profile::new(config()).unwrap();
        let mut peer = Profile::new(config()).unwrap();
        p.implementation_access(&mut cluster, 0xcf80, Some(u64::MAX))
            .unwrap();
        assert_eq!(
            p.implementation_access(&mut cluster, 0xcf80, None).unwrap(),
            peer.implementation_access(&mut cluster, 0xcf80, None)
                .unwrap()
        );
        p.implementation_access(&mut cluster, 0xcf91, Some(0x7f))
            .unwrap();
        assert_eq!(
            peer.implementation_access(&mut cluster, 0xcf91, None)
                .unwrap(),
            Some(0x40)
        );
        assert_eq!(
            peer.implementation_access(&mut cluster, 0xce82, None)
                .unwrap(),
            Some(3 << 24)
        );
    }
}

#[cfg(test)]
mod accounting_tests;
