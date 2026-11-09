//! A command dial's queue of Gray-code phase changes, filled by the session and drained by
//! the encoder model at their declared edge times.
//!
//! Edge requests continue from the last queued phase, even after the queue drains.
//! Full-cycle requests remain four edges per cycle. The single shared queue allocation
//! carries its final phase independently of the encoder model's current physical phase.
use std::collections::VecDeque;
use parking_lot::Mutex;
use std::sync::Arc;

pub struct EncoderQueue {
    pub events: VecDeque<(u64, u8)>,
    pub final_phase: u8,
}

#[derive(Clone)]
pub struct EncoderInput {
    pub events: Arc<Mutex<EncoderQueue>>,
    pub phase_ticks: u64,
    pub rest_phase: u8,
}
impl EncoderInput {
    pub fn admit_cycles(&self, now: u64, cycles: i32) -> Result<(), String> {
        self.admit_edges(now, Self::cycle_edges(cycles)?)
    }
    fn cycle_edges(cycles: i32) -> Result<i32, String> {
        cycles
            .checked_mul(4)
            .ok_or_else(|| "encoder cycle request exceeds the signal queue bound".into())
    }
    pub fn admit_edges(&self, now: u64, edges: i32) -> Result<(), String> {
        self.validate_edges(now, edges, &self.events.lock().events)
            .map(|_| ())
    }
    fn validate_edges(
        &self,
        now: u64,
        edges: i32,
        events: &VecDeque<(u64, u8)>,
    ) -> Result<usize, String> {
        if edges.unsigned_abs() > 1536 {
            return Err("encoder edge request exceeds the signal queue bound".into());
        }
        let edges = edges.unsigned_abs() as usize;
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
        self.queue_edges(now, Self::cycle_edges(cycles)?)
    }
    pub fn queue_edges(&self, now: u64, edges: i32) -> Result<(), String> {
        let mut queue = self.events.lock();
        let count = self.validate_edges(now, edges, &queue.events)?;
        let mut at = queue.events.back().map_or(now, |e| e.0);
        let order = [0, 1, 3, 2];
        let mut index = order.iter().position(|p| *p == queue.final_phase).unwrap();
        let direction = if edges >= 0 { 1 } else { 3 };
        for _ in 0..count {
            index = (index + direction) % 4;
            let phase = order[index];
            at += self.phase_ticks;
            queue.events.push_back((at, phase));
            queue.final_phase = phase;
        }
        Ok(())
    }
}
