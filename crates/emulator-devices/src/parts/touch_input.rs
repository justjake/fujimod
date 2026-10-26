//! Touch contacts a session scripts and a touch controller reports.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ContactInput {
    pub tick: u64,
    pub contact: usize,
    pub phase: Phase,
    pub x: u16,
    pub y: u16,
}
