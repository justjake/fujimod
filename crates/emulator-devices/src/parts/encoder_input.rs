//! A command dial's queue of Gray-code phase changes, filled by the session and drained by
//! the encoder model at their declared edge times.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct EncoderInput {
    pub events: Arc<Mutex<VecDeque<(u64, u8)>>>,
    pub phase_ticks: u64,
    pub rest_phase: u8,
}
impl EncoderInput {
    pub fn admit_cycles(&self, now: u64, cycles: i32) -> Result<(), String> {
        self.validate_cycles(now, cycles, &self.events.lock().unwrap())
            .map(|_| ())
    }
    fn validate_cycles(
        &self,
        now: u64,
        cycles: i32,
        events: &VecDeque<(u64, u8)>,
    ) -> Result<usize, String> {
        if cycles.unsigned_abs() > 384 {
            return Err("encoder cycle request exceeds the signal queue bound".into());
        }
        let edges = cycles.unsigned_abs() as usize * 4;
        if events.len() + edges > 4096 {
            return Err("encoder signal queue is full".into());
        }
        events
            .back()
            .map_or(now, |e| e.0)
            .checked_add(
                self.phase_ticks
                    .checked_mul(edges as u64)
                    .ok_or("encoder deadline overflow")?,
            )
            .ok_or("encoder deadline overflow")?;
        Ok(edges)
    }
    pub fn queue_cycles(&self, now: u64, cycles: i32) -> Result<(), String> {
        let mut events = self.events.lock().unwrap();
        self.validate_cycles(now, cycles, &events)?;
        let mut at = events.back().map_or(now, |e| e.0);
        let order = [0, 1, 3, 2];
        let start = order.iter().position(|p| *p == self.rest_phase).unwrap();
        let phases =
            [1, 2, 3, 4].map(|step| order[(start + if cycles >= 0 { step } else { 4 - step }) % 4]);
        for _ in 0..cycles.unsigned_abs() {
            for phase in phases {
                at += self.phase_ticks;
                events.push_back((at, phase));
            }
        }
        Ok(())
    }
}
