//! Interval timer at `timer_base`, selected by its declared register layout.
use super::definition::TimerLayout;
use fujimod_emulator_xp4_io::interval_timers::IntervalTimers;
use fujimod_emulator_xpp::timer::{self as xpp, TickTimers};

pub enum Timer {
    Xp4(IntervalTimers),
    Xpp(TickTimers),
}
impl Timer {
    pub fn new(layout: TimerLayout, routes: &[(usize, usize)]) -> Result<Self, String> {
        match layout {
            TimerLayout::Xp4 => {
                if routes.iter().any(|&(channel, _)| channel >= 8) {
                    return Err("Invalid XP4 interval-timer route".into());
                }
                Ok(Self::Xp4(IntervalTimers::new(routes)))
            }
            TimerLayout::Xpp => TickTimers::new(routes).map(Self::Xpp),
        }
    }
    /// Register window size in bytes.
    pub fn window(&self) -> u64 {
        match self {
            Self::Xp4(_) => 0x50,
            Self::Xpp(_) => u64::from(xpp::WINDOW),
        }
    }
    pub fn access(
        &mut self,
        now: u64,
        offset: u32,
        size: u32,
        write: bool,
        value: u32,
    ) -> Result<u32, String> {
        match self {
            Self::Xp4(t) => t.access(now, offset, size, write, value),
            Self::Xpp(t) => t.access(now, offset, size, write, value),
        }
    }
    pub fn tick(&mut self, now: u64) -> Vec<usize> {
        match self {
            Self::Xp4(t) => t.tick(now),
            Self::Xpp(t) => t.tick(now),
        }
    }
    pub fn next_event_tick(&self, now: u64) -> Option<u64> {
        match self {
            Self::Xp4(t) => t.next_event_tick(now),
            Self::Xpp(t) => t.next_event_tick(now),
        }
    }
    pub fn next_interrupt_tick(&self, admits: impl FnMut(usize) -> bool) -> Option<u64> {
        match self {
            Self::Xp4(t) => t.next_interrupt_tick(admits),
            Self::Xpp(t) => t.next_interrupt_tick(admits),
        }
    }
}
