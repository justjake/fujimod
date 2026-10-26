//! Two-contact, eleven-byte panel protocol.
//! Wire layout follows the controller command and data framing.
//! 100 Hz reporting, 8-unit tap slop and 400 units/s flick threshold are
//! declared model inputs, not measured silicon timing. As on an X70 panel, a
//! contact lifted within 110 ms reports Tap, a single contact held within the
//! slop reports Long Press 130 ms after touch-down, and a held contact that
//! stays within the slop reports Press, not Drag, when it moves.
//! Only timed external contacts and completed bus reads drive this part.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use fujimod_emulator_devices::parts::touch_input::{ContactInput, Phase};

pub const REPORT_HZ: u64 = 100;
const TAP_MS: u64 = 110;
const LONG_PRESS_MS: u64 = 130;
const QUEUE_LIMIT: usize = 4096;



#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
struct Contact {
    active: bool,
    x: u16,
    y: u16,
    start: (u16, u16),
    down_tick: u64,
    previous: (u16, u16, u64),
    moved: bool,
    paired: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TouchController {
    ticks_per_second: u64,
    period: u64,
    contacts: [Contact; 2],
    planned: [bool; 2],
    inputs: VecDeque<ContactInput>,
    reports: VecDeque<[u8; 11]>,
    current: Option<u64>,
    ready_tick: u64,
    sample_origin: u64,
    sample_tick: Option<u64>,
    pair_distance: Option<i64>,
    #[serde(default)]
    long_press_tick: Option<u64>,
    pub reports_sent: u64,
    pub reports_completed: u64,
}

impl TouchController {
    pub fn new(ticks_per_second: u64) -> Result<Self, String> {
        if ticks_per_second < REPORT_HZ || !ticks_per_second.is_multiple_of(REPORT_HZ) {
            return Err("Touch clock must be a positive multiple of 100 Hz".into());
        }
        Ok(Self {
            ticks_per_second,
            period: ticks_per_second / REPORT_HZ,
            contacts: [Contact::default(); 2],
            planned: [false; 2],
            inputs: VecDeque::new(),
            reports: VecDeque::new(),
            current: None,
            ready_tick: 0,
            sample_origin: 0,
            sample_tick: None,
            pair_distance: None,
            long_press_tick: None,
            reports_sent: 0,
            reports_completed: 0,
        })
    }

    /// Append absolute guest ticks, atomically validating the entire script.
    pub fn inject(&mut self, now: u64, script: &[ContactInput]) -> Result<(), String> {
        if self.inputs.len() + script.len() > QUEUE_LIMIT {
            return Err("Touch contact script is full".into());
        }
        let mut planned = self.planned;
        let mut last = self.inputs.back().map_or(now, |e| e.tick.max(now));
        for e in script {
            if e.tick < last
                || e.tick.checked_add(self.period).is_none()
                || e.contact >= 2
                || e.x > 4095
                || e.y > 4095
            {
                return Err(
                    "Touch requires ordered future ticks, contact 0/1 and 12-bit coordinates"
                        .into(),
                );
            }
            if (e.phase == Phase::Down) == planned[e.contact] {
                return Err("Touch phase conflicts with the scripted contact state".into());
            }
            planned[e.contact] = e.phase != Phase::Up;
            last = e.tick;
        }
        self.planned = planned;
        self.inputs.extend(script);
        Ok(())
    }

    pub fn period(&self) -> u64 {
        self.period
    }

    pub fn pending_reports(&self) -> usize {
        self.reports.len()
    }

    pub fn pending_inputs(&self) -> usize {
        self.inputs.len()
    }

    pub fn contact_down(&self) -> bool {
        self.contacts.iter().any(|c| c.active)
    }

    /// Host reads are observational. A bus/IRQ dependency wake makes a newly
    /// acknowledged report or newly enabled route due now, without polling.
    pub fn next_event_tick(&self, now: u64, reads: u64, route_enabled: bool) -> Option<u64> {
        let mut deadline = self.inputs.front().map(|e| e.tick);
        for tick in [self.sample_tick, self.long_press_tick].into_iter().flatten() {
            deadline = Some(deadline.map_or(tick, |old| old.min(tick)));
        }
        if self.current.is_some_and(|before| reads > before) {
            deadline = Some(now);
        } else if self.current.is_none() && !self.reports.is_empty() && route_enabled {
            deadline = Some(deadline.map_or(self.ready_tick, |old| old.min(self.ready_tick)));
        }
        deadline.map(|tick| tick.max(now))
    }

    fn report(&mut self, kind: u8) -> Result<(), String> {
        if self.reports.len() >= QUEUE_LIMIT {
            return Err("Touch report queue is full; guest is not consuming reports".into());
        }
        let mut frame = [0; 11];
        frame[0] = self.contacts.iter().filter(|c| c.active).count() as u8;
        frame[1] = kind;
        if kind != 4 && kind != 10 {
            for (i, c) in self.contacts.iter().enumerate() {
                frame[2] |= u8::from(c.active) << (i * 4);
                let offset = 3 + i * 4;
                frame[offset] = c.x as u8;
                frame[offset + 1] = c.y as u8;
                frame[offset + 2] = (c.x >> 8) as u8 | ((c.y >> 8) as u8) << 4;
                frame[offset + 3] = if c.active { 128 } else { 0 };
            }
        }
        self.reports.push_back(frame);
        Ok(())
    }

    fn distance(&self) -> i64 {
        let dx = i64::from(self.contacts[0].x) - i64::from(self.contacts[1].x);
        let dy = i64::from(self.contacts[0].y) - i64::from(self.contacts[1].y);
        dx * dx + dy * dy
    }

    fn sample(&mut self) -> Result<(), String> {
        self.sample_tick = None;
        let kind = if self.contacts.iter().all(|c| c.active) {
            let distance = self.distance();
            let kind = match self.pair_distance {
                Some(old) if distance < old => 5,
                Some(old) if distance > old => 6,
                _ => 3,
            };
            self.pair_distance = Some(distance);
            kind
        } else if self.contacts.iter().any(|c| c.active && c.moved) {
            3
        } else {
            2
        };
        self.report(kind)
    }

    fn long_press(&mut self) -> Result<(), String> {
        self.long_press_tick = None;
        let held: Vec<_> = self.contacts.iter().filter(|c| c.active).collect();
        if held.len() == 1 && !held[0].moved && !held[0].paired {
            self.report(9)?;
        }
        Ok(())
    }

    fn input(&mut self, e: ContactInput) -> Result<(), String> {
        let i = e.contact;
        match e.phase {
            Phase::Down => {
                if !self.contact_down() {
                    self.sample_origin = e.tick;
                }
                self.contacts[i] = Contact {
                    active: true,
                    x: e.x,
                    y: e.y,
                    start: (e.x, e.y),
                    down_tick: e.tick,
                    previous: (e.x, e.y, e.tick),
                    ..Contact::default()
                };
                if self.contacts.iter().all(|c| c.active) {
                    for c in &mut self.contacts {
                        c.paired = true;
                    }
                    self.pair_distance = Some(self.distance());
                    self.long_press_tick = None;
                } else {
                    self.long_press_tick = Some(
                        e.tick
                            .checked_add(self.ticks_per_second * LONG_PRESS_MS / 1000)
                            .ok_or("Touch time overflow")?,
                    );
                }
                self.report(2)?;
            }
            Phase::Move | Phase::Up => {
                let c = &mut self.contacts[i];
                if (e.x, e.y) != (c.x, c.y) {
                    c.previous = (c.x, c.y, e.tick);
                    c.x = e.x;
                    c.y = e.y;
                    c.moved |= c.x.abs_diff(c.start.0).max(c.y.abs_diff(c.start.1)) > 8;
                    if c.moved {
                        self.long_press_tick = None;
                    }
                    self.sample_tick = Some(
                        self.sample_origin
                            + (e.tick - self.sample_origin).div_ceil(self.period) * self.period,
                    );
                }
                if e.phase == Phase::Up {
                    // Final coordinates must precede coordinate-free Flick.
                    if self.sample_tick.is_some() {
                        self.sample()?;
                    }
                    let c = self.contacts[i];
                    if !c.paired {
                        if !c.moved && e.tick - c.down_tick <= self.ticks_per_second * TAP_MS / 1000 {
                            self.report(1)?;
                        } else if c.moved {
                            let distance =
                                c.x.abs_diff(c.previous.0).max(c.y.abs_diff(c.previous.1));
                            let elapsed = e.tick.saturating_sub(c.previous.2).max(self.period);
                            if u128::from(distance) * u128::from(self.ticks_per_second)
                                >= 400 * u128::from(elapsed)
                            {
                                self.report(4)?;
                            }
                        }
                    }
                    self.contacts[i].active = false;
                    self.pair_distance = None;
                    self.long_press_tick = None;
                    self.report(0)?;
                }
            }
        }
        Ok(())
    }

    /// Run only actual due input/sample edges and read acknowledgments.
    /// The adapter publishes the returned frame and asserts its wired IRQ.
    pub fn tick(
        &mut self,
        now: u64,
        reads: u64,
        route_enabled: bool,
    ) -> Result<Option<[u8; 11]>, String> {
        if self.current.is_some_and(|before| reads > before) {
            self.current = None;
            self.reports_completed += 1;
        }
        loop {
            let input_tick = self.inputs.front().map(|e| e.tick);
            // Apply all contacts at an edge before sampling that edge.
            let long_press = self.long_press_tick;
            if let Some(tick) = input_tick
                && tick <= now
                && self.sample_tick.is_none_or(|s| tick <= s)
                && long_press.is_none_or(|l| tick <= l)
            {
                let e = self.inputs.pop_front().expect("front input");
                self.input(e)?;
            } else if let Some(s) = self.sample_tick
                && s <= now
                && long_press.is_none_or(|l| s <= l)
            {
                self.sample()?;
            } else if long_press.is_some_and(|l| l <= now) {
                self.long_press()?;
            } else {
                break;
            }
        }
        if self.current.is_some() || !route_enabled || now < self.ready_tick {
            return Ok(None);
        }
        let Some(frame) = self.reports.pop_front() else {
            return Ok(None);
        };
        self.current = Some(reads);
        self.ready_tick = now.checked_add(self.period).ok_or("Touch time overflow")?;
        self.reports_sent += 1;
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests;
