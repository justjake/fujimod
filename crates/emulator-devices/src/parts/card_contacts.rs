//! An SD card slot's physical contacts: card presence and write protection,
//! and which of the slot's two host interfaces drives them.
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

const UHS2_INTERFACE: u8 = 4;

/// Shared physical contacts, also readable by the board's GPIO input wiring.
#[derive(Clone)]
pub struct Contacts(Arc<AtomicU8>);
impl Contacts {
    /// Presence and write protection. The interface selection is the UHS-II
    /// port's state and is restored by that port.
    pub fn checkpoint_level(&self) -> u8 {
        self.0.load(Ordering::Relaxed) & !UHS2_INTERFACE
    }
    pub fn restore_level(&self, bits: u8) {
        self.set(bits & 1 != 0, bits & 2 != 0);
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
        let level = present as u8 | (protected as u8) << 1;
        self.0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
                Some(bits & UHS2_INTERFACE | level)
            })
            .ok();
    }
    /// True while the slot's UHS-II port has its interface enabled. The slot's
    /// lines then belong to the UHS-II PHY, not to the 4-bit SD interface.
    pub fn uhs2_interface(&self) -> bool {
        self.0.load(Ordering::Relaxed) & UHS2_INTERFACE != 0
    }
    pub fn select_uhs2_interface(&self, enabled: bool) {
        if enabled {
            self.0.fetch_or(UHS2_INTERFACE, Ordering::Relaxed);
        } else {
            self.0.fetch_and(!UHS2_INTERFACE, Ordering::Relaxed);
        }
    }
}
