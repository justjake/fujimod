//! X-Processor Pro tick timer: 16 decrementing channels.
//!
//! Control word +0: bit `16 + n` clear selects channel `n`, bit `n` then
//! starts or stops it. Per-channel configuration words sit at `+0x10 + n*4`
//! and retain writes. Channel blocks at `+0x100 + n*0x10` hold the count,
//! the reload and two retained words. The counting law is the XP4 one.
use fujimod_emulator_xp4_io::interval_timers::Counter;

/// Register window size in bytes.
pub const WINDOW: u32 = 0x200;
const CHANNELS: usize = 16;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TickTimers {
    counters: Vec<Counter>,
    irqs: Vec<Option<usize>>,
    configuration: Vec<u32>,
    retained: Vec<[u32; 2]>,
}

impl TickTimers {
    pub fn new(routes: &[(usize, usize)]) -> Result<Self, String> {
        let mut irqs = vec![None; CHANNELS];
        for &(channel, irq) in routes {
            if channel >= CHANNELS || irqs[channel].replace(irq).is_some() {
                return Err("Invalid X-Processor Pro timer route".into());
            }
        }
        Ok(Self {
            counters: vec![Counter::default(); CHANNELS],
            irqs,
            configuration: vec![0; CHANNELS],
            retained: vec![[0; 2]; CHANNELS],
        })
    }
    pub fn save(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
    pub fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.irqs != self.irqs
            || state.counters.len() != CHANNELS
            || state.configuration.len() != CHANNELS
            || state.retained.len() != CHANNELS
            || state.counters.iter().any(|c| c.enabled != c.next.is_some())
        {
            return Err("Invalid X-Processor Pro timer checkpoint".into());
        }
        *self = state;
        Ok(())
    }
    pub fn next_event_tick(&self, _now: u64) -> Option<u64> {
        self.counters.iter().filter_map(|c| c.next).min()
    }
    pub fn next_interrupt_tick(&self, mut admits: impl FnMut(usize) -> bool) -> Option<u64> {
        self.counters
            .iter()
            .enumerate()
            .filter_map(|(n, c)| c.next.filter(|_| self.irqs[n].is_some_and(&mut admits)))
            .min()
    }
    pub fn tick(&mut self, now: u64) -> Vec<usize> {
        let mut fired = Vec::new();
        for (channel, c) in self.counters.iter_mut().enumerate() {
            if c.expire(now)
                && let Some(irq) = self.irqs[channel]
            {
                fired.push(irq);
            }
        }
        fired
    }
    pub fn access(
        &mut self,
        now: u64,
        offset: u32,
        size: u32,
        write: bool,
        value: u32,
    ) -> Result<u32, String> {
        if size != 4 || offset & 3 != 0 {
            return Err("X-Processor Pro timer needs aligned words".into());
        }
        match offset {
            0 => {
                if write {
                    let select = !(value >> 16) & 0xffff;
                    for (channel, c) in self.counters.iter_mut().enumerate() {
                        if select & (1 << channel) != 0 {
                            c.enable(now, value & (1 << channel) != 0);
                        }
                    }
                }
                Ok(self
                    .counters
                    .iter()
                    .enumerate()
                    .fold(0, |bits, (i, c)| bits | (u32::from(c.enabled) << i)))
            }
            0x10..0x50 => {
                let word = &mut self.configuration[((offset - 0x10) / 4) as usize];
                if write {
                    *word = value;
                }
                Ok(*word)
            }
            0x100..WINDOW => {
                let channel = ((offset - 0x100) / 0x10) as usize;
                let c = &mut self.counters[channel];
                match offset & 0xc {
                    0 => {
                        if write {
                            c.load(now, value);
                        }
                        Ok(c.value(now))
                    }
                    4 => {
                        if write {
                            c.reload = value;
                        }
                        Ok(c.reload)
                    }
                    slot => {
                        let word = &mut self.retained[channel][((slot - 8) / 4) as usize];
                        if write {
                            *word = value;
                        }
                        Ok(*word)
                    }
                }
            }
            _ => Err(format!(
                "Unsupported X-Processor Pro timer offset {offset:#x}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleared_high_bit_selects_a_channel_and_its_route_fires() {
        let mut t = TickTimers::new(&[(15, 63)]).unwrap();
        t.access(100, 0x1f0, 4, true, 9).unwrap();
        t.access(100, 0x1f4, 4, true, 9).unwrap();
        t.access(100, 0x4c, 4, true, 0x10000).unwrap();
        t.access(100, 0, 4, true, 0x7fff8000).unwrap();
        assert_eq!(t.access(100, 0x4c, 4, false, 0).unwrap(), 0x10000);
        assert_eq!(t.next_event_tick(100), Some(110));
        assert_eq!(t.access(105, 0x1f0, 4, false, 0).unwrap(), 4);
        assert_eq!(t.tick(110), vec![63]);
        // 0xfeff0100 selects only channel 8, leaving channel 15 running.
        t.access(110, 0, 4, true, 0xfeff0100).unwrap();
        assert_eq!(t.access(110, 0, 4, false, 0).unwrap(), 0x8100);
        t.access(110, 0, 4, true, 0x7fff0000).unwrap();
        assert_eq!(t.access(110, 0, 4, false, 0).unwrap(), 0x0100);
        t.access(110, 0x1f8, 4, true, 5).unwrap();
        assert_eq!(t.access(110, 0x1f8, 4, false, 0).unwrap(), 5);
        let mut resumed = TickTimers::new(&[(15, 63)]).unwrap();
        resumed.restore(&t.save()).unwrap();
        assert_eq!(resumed.save(), t.save());
        assert!(
            TickTimers::new(&[(15, 64)])
                .unwrap()
                .restore(&t.save())
                .is_err()
        );
        assert!(t.access(110, WINDOW, 4, false, 0).is_err());
        assert!(TickTimers::new(&[(16, 63)]).is_err());
    }
}
