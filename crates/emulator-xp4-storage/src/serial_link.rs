//! Synchronous serial controllers the XP4 DMA engine moves words through.
//!
//! A declared controller registers its data register here. A DMA channel
//! whose fixed side is that register then transfers through the controller
//! instead of addressing memory. The link carries hardware facts only: the
//! controller's request enables, the words it shifts and the ticks they shift
//! at. It does not describe what is attached to the wire.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// A full-duplex exchange the controller admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exchange {
    /// Bits in one word.
    pub bits: u32,
    /// Ticks one word takes to shift.
    pub word_ticks: u64,
    /// Tick at which the last word has shifted.
    pub end: u64,
}

/// One controller as the DMA engine sees it.
pub trait SerialPeer: Send + Sync {
    /// Request enables the controller drives: bit 0 transmit, bit 1 receive.
    fn requests(&self) -> Result<u32, String>;
    /// Refuses while the controller runs an exchange or holds programmed-I/O
    /// words that are queued or shifting.
    fn check_idle(&self) -> Result<(), String>;
    /// Starts a full-duplex exchange of `bytes` bytes, moved in transfers of
    /// `width` bytes, at tick `start`. Refuses before changing anything when
    /// the controller cannot run it.
    fn begin(&self, start: u64, bytes: u64, width: u64) -> Result<Exchange, String>;
    /// Shifts the next word of the running exchange, which has shifted at
    /// `tick`, and returns the word received meanwhile.
    fn shift(&self, tx: u16, tick: u64) -> Result<u16, String>;
    /// Ends the exchange that started at `start` and whose last word shifted
    /// at `end`.
    fn finish(&self, start: u64, end: u64) -> Result<(), String>;
}

pub type SharedSerialPeer = Arc<dyn SerialPeer>;

/// Declared controllers by data register address, shared by the DMA engine
/// and the board that declares them (in either order).
#[derive(Clone, Default)]
pub struct SerialLinks(Arc<Mutex<BTreeMap<u64, SharedSerialPeer>>>);

impl SerialLinks {
    pub fn register(&self, data_register: u64, peer: SharedSerialPeer) -> Result<(), String> {
        let mut links = self
            .0
            .lock()
            .map_err(|_| "XP4 serial link registry lock poisoned")?;
        if data_register & 3 != 0 || links.contains_key(&data_register) {
            return Err(format!(
                "Invalid or duplicate XP4 serial link data register {data_register:#x}"
            ));
        }
        links.insert(data_register, peer);
        Ok(())
    }
    /// The controller whose data register is `address`, if one is declared.
    pub fn get(&self, address: u64) -> Option<SharedSerialPeer> {
        self.0.lock().ok()?.get(&address).cloned()
    }
    pub fn contains(&self, address: u64) -> bool {
        self.0
            .lock()
            .is_ok_and(|links| links.contains_key(&address))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Idle;
    impl SerialPeer for Idle {
        fn requests(&self) -> Result<u32, String> {
            Ok(0)
        }
        fn check_idle(&self) -> Result<(), String> {
            Ok(())
        }
        fn begin(&self, _: u64, _: u64, _: u64) -> Result<Exchange, String> {
            Err("idle".into())
        }
        fn shift(&self, _: u16, _: u64) -> Result<u16, String> {
            Err("idle".into())
        }
        fn finish(&self, _: u64, _: u64) -> Result<(), String> {
            Err("idle".into())
        }
    }
    #[test]
    fn registry_refuses_duplicate_and_unaligned_data_registers() {
        let links = SerialLinks::default();
        let shared = links.clone();
        links.register(0xfff34008, Arc::new(Idle)).unwrap();
        assert!(shared.contains(0xfff34008));
        assert!(shared.register(0xfff34008, Arc::new(Idle)).is_err());
        assert!(shared.register(0xfff35009, Arc::new(Idle)).is_err());
        assert!(shared.get(0xfff35008).is_none());
        assert_eq!(shared.get(0xfff34008).unwrap().requests(), Ok(0));
    }
}
