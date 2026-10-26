//! An SD card slot's physical contacts: card presence and write protection.
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Shared physical contacts, also readable by the board's GPIO input wiring.
#[derive(Clone)]
pub struct Contacts(Arc<AtomicU8>);
impl Contacts {
    pub fn checkpoint_level(&self) -> u8 {
        self.0.load(Ordering::Relaxed)
    }
    pub fn restore_level(&self, bits: u8) {
        self.0.store(bits, Ordering::Relaxed);
    }
    pub fn new(present: bool, protected: bool) -> Self {
        Self(Arc::new(AtomicU8::new(
            present as u8 | (protected as u8) << 1,
        )))
    }
    pub fn present(&self) -> bool {
        self.0.load(Ordering::Relaxed) & 1 != 0
    }
    pub fn protected(&self) -> bool {
        self.0.load(Ordering::Relaxed) & 2 != 0
    }
    pub fn set(&self, present: bool, protected: bool) {
        self.0
            .store(present as u8 | (protected as u8) << 1, Ordering::Relaxed);
    }
}
