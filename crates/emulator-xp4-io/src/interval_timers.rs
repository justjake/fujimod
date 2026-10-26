//! XP4 decrementing interval counters with guest-programmed reloads.
/// Decrementing interval counters with guest-programmed reloads.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counter {
    pub value: u32,
    pub reload: u32,
    pub enabled: bool,
    pub next: Option<u64>,
}
impl Counter {
    /// Count at `now`: the remaining ticks before the next expiry while
    /// running, then the reload phase; the stored value while stopped.
    pub fn value(&self, now: u64) -> u32 {
        if let Some(at) = self.next {
            if now < at {
                (at - now - 1) as u32
            } else {
                self.reload
                    .wrapping_sub(((now - at) % (u64::from(self.reload) + 1)) as u32)
            }
        } else {
            self.value
        }
    }
    /// Start or stop from the current count.
    pub fn enable(&mut self, now: u64, enabled: bool) {
        self.value = self.value(now);
        self.enabled = enabled;
        self.next = enabled.then_some(now + u64::from(self.value) + 1);
    }
    /// Load a new count; a running counter restarts from it.
    pub fn load(&mut self, now: u64, value: u32) {
        self.value = value;
        self.next = self.enabled.then_some(now + u64::from(value) + 1);
    }
    /// Whether an expiry is due by `now`; the next one follows the reload period.
    pub fn expire(&mut self, now: u64) -> bool {
        match self.next {
            Some(at) if at <= now => {
                let period = u64::from(self.reload) + 1;
                self.next = Some(at + ((now - at) / period + 1) * period);
                true
            }
            _ => false,
        }
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntervalTimers {
    counters: [Counter; 8],
    irqs: [Option<usize>; 8],
}
impl IntervalTimers {
    pub fn save(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
    pub fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.irqs != self.irqs || state.counters.iter().any(|c| c.enabled != c.next.is_some()) {
            return Err("Invalid XP4 interval-timer checkpoint".into());
        }
        *self = state;
        Ok(())
    }
    pub fn new(routes: &[(usize, usize)]) -> Self {
        let mut irqs = [None; 8];
        for &(channel, irq) in routes {
            irqs[channel] = Some(irq);
        }
        Self {
            counters: Default::default(),
            irqs,
        }
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
            return Err("XP4 interval timer needs aligned words".into());
        }
        if offset == 0 {
            if write {
                if value & !0x00ff00ff != 0 {
                    return Err("Unsupported XP4 interval control bits".into());
                }
                for (channel, c) in self.counters.iter_mut().enumerate() {
                    if value & (1 << (channel + 16)) != 0 {
                        c.enable(now, value & (1 << channel) != 0);
                    }
                }
            }
            return Ok(self
                .counters
                .iter()
                .enumerate()
                .fold(0, |bits, (i, c)| bits | (u32::from(c.enabled) << i)));
        }
        if (0x10..0x50).contains(&offset) {
            let channel = ((offset - 0x10) / 8) as usize;
            let c = &mut self.counters[channel];
            if offset & 4 == 0 {
                if write {
                    c.load(now, value);
                }
                return Ok(c.value(now));
            }
            if write {
                c.reload = value;
            }
            return Ok(c.reload);
        }
        Err(format!("Unsupported XP4 timer offset {offset:#x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_keeps_the_pending_reload_phase_and_checked_routes() {
        let mut timer = IntervalTimers::new(&[(7, 64)]);
        timer.access(100, 0x48, 4, true, 9).unwrap();
        timer.access(100, 0x4c, 4, true, 19).unwrap();
        timer.access(100, 0, 4, true, 0x800080).unwrap();
        let saved = timer.save();
        let mut resumed = IntervalTimers::new(&[(7, 64)]);
        resumed.restore(&saved).unwrap();
        assert_eq!(resumed.next_event_tick(105), Some(110));
        assert_eq!(resumed.access(105, 0x48, 4, false, 0).unwrap(), 4);
        assert!(resumed.tick(109).is_empty());
        assert_eq!(resumed.tick(110), vec![64]);
        assert_eq!(resumed.next_event_tick(110), Some(130));
        timer.tick(110);
        assert_eq!(timer.save(), resumed.save());
        let mut wrong_route = IntervalTimers::new(&[(7, 65)]);
        let before = wrong_route.save();
        assert!(wrong_route.restore(&saved).is_err());
        assert_eq!(wrong_route.save(), before);
    }
    #[test]
    fn deadlines_counter_reads_and_early_ticks_do_not_depend_on_quantum() {
        let mut t = IntervalTimers::new(&[(7, 64)]);
        t.access(100, 0x48, 4, true, 9).unwrap();
        t.access(100, 0x4c, 4, true, 9).unwrap();
        t.access(100, 0, 4, true, 0x800080).unwrap();
        assert_eq!(t.next_event_tick(100), Some(110));
        assert!(t.tick(109).is_empty());
        assert_eq!(t.access(109, 0x48, 4, false, 0).unwrap(), 0);
        assert_eq!(t.next_event_tick(109), Some(110));
        assert_eq!(t.tick(110), vec![64]);
        assert_eq!(t.next_event_tick(110), Some(120));
        assert_eq!(t.access(110, 0x48, 4, false, 0).unwrap(), 9);
        assert_eq!(t.tick(137), vec![64]);
        assert_eq!(t.next_event_tick(137), Some(140));
        t.access(137, 0, 4, true, 0x800000).unwrap();
        assert_eq!(t.next_event_tick(137), None);
    }
}
