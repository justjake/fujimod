//! Shared physical logic levels. Reads are observational; only an explicit
//! external input or an owning output may drive a new level.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[derive(Clone)]
pub struct Level(Arc<AtomicBool>);
impl Level {
    pub fn new(high: bool) -> Self {
        Self(Arc::new(AtomicBool::new(high)))
    }
    pub fn high(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    pub fn drive(&self, high: bool) {
        self.0.store(high, Ordering::Relaxed);
    }
}
