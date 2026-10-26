//! ARMv7-M system control, NVIC and SysTick on the supplied board timeline.
//! No guessed SoC MMIO.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Serialize, Deserialize)]
pub struct Nvic {
    pub enabled: Vec<u32>,
    pub pending: Vec<u32>,
    pub active: Vec<u16>,
    pub priority: Vec<u8>,
    pub vtor: u32,
    pub control: u32,
    pub reload: u32,
    pub deadline: u64,
    pub countflag: bool,
    pub clock_hz: u64,
    pub regs: BTreeMap<u64, u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn masked_and_nested_mailbox_irqs_keep_pending_state() {
        let mut n = Nvic::new(600_000_000);
        n.enabled[1] = (1 << 23) | (1 << 26); // IRQ55/58
        n.priority[71] = 0x40;
        n.priority[74] = 0x20;
        n.pend(55);
        n.pend(58);
        assert_eq!(n.next(1, 0, 0), None);
        assert!(n.wake_pending()); // WFI wakes even with PRIMASK set.
        assert_eq!(n.next(0, 0x20, 0), None);
        assert_eq!(n.next(0, 0, 0), Some(74));
        n.enter(74);
        assert_eq!(n.next(0, 0, 0), None);
        n.active.pop();
        assert_eq!(n.next(0, 0, 0), Some(71));
        n.enter(71);
        assert_eq!(n.pending[1], 0);
    }
    #[test]
    fn systick_coalesces_and_pendsv_waits_for_higher_priority_return() {
        let mut n = Nvic::new(600_000_000);
        n.write(0x14, 4, 5_999_999, 0).unwrap();
        n.write(0x10, 4, 7, 0).unwrap();
        n.priority[15] = 0x80;
        n.priority[14] = 0xff;
        n.write(0xd04, 4, 1 << 28, 0).unwrap();
        n.tick(10_000_000);
        assert_eq!(n.next(0, 0, 0), Some(15));
        n.enter(15);
        assert_eq!(n.next(0, 0, 0), None);
        n.active.pop();
        assert_eq!(n.next(0, 0, 0), Some(14));
        assert_ne!(n.read(0x10, 4, 10_000_000).unwrap() & (1 << 16), 0);
        assert_eq!(n.read(0x10, 4, 10_000_000).unwrap() & (1 << 16), 0);
        n.tick(1_000_000_000);
        assert_eq!(n.deadline, 1_010_000_000);
    }
    #[test]
    fn systick_phase_is_independent_of_tick_frequency() {
        let mut initial = Nvic::new(600_000_000);
        initial.write(0x14, 4, 5_999_999, 0).unwrap();
        initial.write(0x10, 4, 7, 0).unwrap();
        let before = serde_json::to_value(&initial).unwrap();
        initial.tick(9_999_999);
        assert_eq!(serde_json::to_value(&initial).unwrap(), before);
        let mut frequent = initial.clone();
        for now in [10, 11, 20, 33, 40, 50, 63] {
            frequent.tick(now * 1_000_000);
        }
        let mut delayed = initial;
        delayed.tick(63_000_000);
        assert_eq!(delayed.deadline, 70_000_000);
        assert_eq!(
            serde_json::to_value(&frequent).unwrap(),
            serde_json::to_value(&delayed).unwrap()
        );
        // A checkpoint retains the absolute phase across a later service gap.
        let mut restored: Nvic =
            serde_json::from_value(serde_json::to_value(&delayed).unwrap()).unwrap();
        restored.tick(97_000_000);
        assert_eq!(restored.deadline, 100_000_000);
    }
}
impl Nvic {
    pub fn new(clock_hz: u64) -> Self {
        Self {
            enabled: vec![0; 8],
            pending: vec![0; 8],
            active: vec![],
            priority: vec![0; 272],
            vtor: 0,
            control: 0,
            reload: 0,
            deadline: 0,
            countflag: false,
            clock_hz,
            regs: BTreeMap::new(),
        }
    }
    fn period(&self) -> u64 {
        ((self.reload as u64 + 1) * 1_000_000_000 / self.clock_hz).max(1)
    }
    pub fn pend(&mut self, irq: u16) {
        self.pending[irq as usize / 32] |= 1 << (irq % 32)
    }
    pub fn wake_pending(&self) -> bool {
        self.pending
            .iter()
            .zip(&self.enabled)
            .any(|(p, e)| p & e != 0)
            || self.regs.get(&0xd04).copied().unwrap_or(0) & ((1 << 26) | (1 << 28)) != 0
    }
    pub fn tick(&mut self, now: u64) {
        if self.control & 1 != 0 && now >= self.deadline {
            self.countflag = true;
            if self.control & 2 != 0 {
                self.regs
                    .entry(0xd04)
                    .and_modify(|v| *v |= 1 << 26)
                    .or_insert(1 << 26);
            }
            // COUNTFLAG/pending coalesce missed wraps, but the counter's
            // phase belongs to hardware, not the scheduler's service time.
            let period = u128::from(self.period());
            let wraps = u128::from(now - self.deadline) / period + 1;
            self.deadline = (u128::from(self.deadline) + wraps * period)
                .min(u128::from(u64::MAX)) as u64;
        }
    }
    pub fn next(&self, primask: u32, basepri: u32, faultmask: u32) -> Option<u16> {
        if primask != 0 || faultmask != 0 {
            return None;
        }
        let active = self
            .active
            .last()
            .map_or(256, |e| self.priority[*e as usize] as u16);
        let mut ready = vec![];
        let icsr = self.regs.get(&0xd04).copied().unwrap_or(0);
        if icsr & (1 << 28) != 0 {
            ready.push(14)
        }
        if icsr & (1 << 26) != 0 {
            ready.push(15)
        }
        for (i, (p, e)) in self.pending.iter().zip(&self.enabled).enumerate() {
            let mut bits = p & e;
            while bits != 0 {
                let bit = bits.trailing_zeros();
                ready.push((16 + i * 32 + bit as usize) as u16);
                bits &= bits - 1;
            }
        }
        ready
            .into_iter()
            .filter(|e| {
                let p = self.priority[*e as usize] as u16;
                p < active && (basepri == 0 || p < basepri as u16)
            })
            .min_by_key(|e| (self.priority[*e as usize], *e))
    }
    pub fn enter(&mut self, e: u16) {
        if e >= 16 {
            self.pending[(e as usize - 16) / 32] &= !(1 << ((e - 16) % 32));
        } else if e == 14 || e == 15 {
            let bit = if e == 14 { 28 } else { 26 };
            *self.regs.entry(0xd04).or_default() &= !(1 << bit);
        }
        self.active.push(e);
    }
    pub fn read(&mut self, o: u64, n: u32, now: u64) -> Result<u64, String> {
        self.tick(now);
        if (0x400..0x500).contains(&o) && o + n as u64 <= 0x500 {
            return Ok((0..n).fold(0, |v, b| {
                v | ((self.priority[(16 + o - 0x400 + b as u64) as usize] as u64) << (b * 8))
            }));
        }
        if (0xd18..0xd24).contains(&o) && o + n as u64 <= 0xd24 {
            return Ok((0..n).fold(0, |v, b| {
                v | ((self.priority[(4 + o - 0xd18 + b as u64) as usize] as u64) << (b * 8))
            }));
        }
        if n != 4 || o & 3 != 0 {
            return Err(format!("unsupported SCS read {o:#x}/{n}"));
        }
        Ok(match o {
            0x10 => {
                let v = self.control | ((self.countflag as u32) << 16);
                self.countflag = false;
                v as u64
            }
            0x14 => self.reload as u64,
            0x18 => {
                if self.control & 1 == 0 {
                    0
                } else {
                    (self.deadline.saturating_sub(now) * self.clock_hz / 1_000_000_000)
                        .min(self.reload as u64)
                }
            }
            0x100..=0x11c => self.enabled[((o - 0x100) / 4) as usize] as u64,
            0x180..=0x19c => self.enabled[((o - 0x180) / 4) as usize] as u64,
            0x200..=0x21c => self.pending[((o - 0x200) / 4) as usize] as u64,
            0x280..=0x29c => self.pending[((o - 0x280) / 4) as usize] as u64,
            0x300..=0x31c => self
                .active
                .iter()
                .filter(|e| **e >= 16 && ((**e - 16) / 32) as u64 == (o - 0x300) / 4)
                .fold(0u64, |v, e| v | (1 << ((e - 16) % 32))),
            0xd00 => 0x410fc241, // execution model identity, not measured silicon
            0xd04 => {
                self.regs.get(&o).copied().unwrap_or(0) as u64
                    | self.active.last().copied().unwrap_or(0) as u64
            }
            0xd08 => self.vtor as u64,
            0xd0c | 0xd10 | 0xd14 | 0xd24 | 0xd28 | 0xd2c | 0xd34 | 0xd38 | 0xd88 | 0xdf0
            | 0xdfc => self.regs.get(&o).copied().unwrap_or(0) as u64,
            _ => return Err(format!("unmodeled SCS read {o:#x}/{n}")),
        })
    }
    pub fn write(&mut self, o: u64, n: u32, v: u64, now: u64) -> Result<(), String> {
        if (0x400..0x500).contains(&o) && o + n as u64 <= 0x500 {
            for b in 0..n {
                self.priority[(16 + o - 0x400 + b as u64) as usize] = (v >> (b * 8)) as u8;
            }
            return Ok(());
        }
        if (0xd18..0xd24).contains(&o) && o + n as u64 <= 0xd24 {
            for b in 0..n {
                self.priority[(4 + o - 0xd18 + b as u64) as usize] = (v >> (b * 8)) as u8;
            }
            return Ok(());
        }
        if n != 4 || o & 3 != 0 {
            return Err(format!("unsupported SCS write {o:#x}/{n}"));
        }
        let v = v as u32;
        match o {
            0x10 => {
                if v & !7 != 0 || v & 1 != 0 && v & 4 == 0 {
                    return Err("external SysTick clock is not modeled".into());
                }
                self.control = v;
                self.deadline = now + self.period();
            }
            0x14 => {
                self.reload = v & 0xffffff;
                self.deadline = now + self.period();
            }
            0x18 => {
                self.countflag = false;
                self.deadline = now + self.period();
            }
            0x100..=0x11c => self.enabled[((o - 0x100) / 4) as usize] |= v,
            0x180..=0x19c => self.enabled[((o - 0x180) / 4) as usize] &= !v,
            0x200..=0x21c => self.pending[((o - 0x200) / 4) as usize] |= v,
            0x280..=0x29c => self.pending[((o - 0x280) / 4) as usize] &= !v,
            0xd04 => {
                let reg = self.regs.entry(o).or_default();
                if v & (1 << 27) != 0 {
                    *reg &= !(1 << 26)
                }
                if v & (1 << 29) != 0 {
                    *reg &= !(1 << 28)
                }
                *reg |= v & ((1 << 26) | (1 << 28));
            }
            0xd08 => self.vtor = v & !0x7f,
            0xd0c => {
                if v >> 16 != 0x5fa {
                    return Ok(());
                }
                if v & 4 != 0 {
                    return Err("M-core requested SYSRESETREQ".into());
                }
                self.regs.insert(o, v);
            }
            0xd10 | 0xd14 | 0xd24 | 0xd88 | 0xdf0 | 0xdfc => {
                self.regs.insert(o, v);
            }
            0xd28 | 0xd2c => {
                *self.regs.entry(o).or_default() &= !v;
            }
            _ => return Err(format!("unmodeled SCS write {o:#x}/{n}={v:#x}")),
        }
        Ok(())
    }
}
