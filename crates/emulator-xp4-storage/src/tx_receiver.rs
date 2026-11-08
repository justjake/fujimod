//! Explicit bounded receiver offers for fixed-address word transfers.
//! This transport does not define register, wire, or attached-part behavior.
use std::sync::{Arc, Mutex};

pub type SharedReceiver = Arc<Mutex<TxReceiver>>;
pub struct TxReceiver {
    address: u32,
    request: u8,
    offer: Option<(u64, usize)>,
    bytes: Vec<u8>,
    capacity: usize,
}
impl TxReceiver {
    pub fn new(address: u32, request: u8, capacity: usize) -> Result<Self, String> {
        if address & 3 != 0 || request >= 64 || capacity == 0 || capacity > 65536 {
            return Err("Invalid bounded transmit receiver".into());
        }
        Ok(Self { address, request, offer: None, bytes: Vec::new(), capacity })
    }
    /// Explicit receiver-side input; an offer admits one entire word batch.
    pub fn offer(&mut self, at: u64, bytes: usize) -> Result<(), String> {
        if self.offer.is_some() || bytes == 0 || bytes % 4 != 0 || bytes > self.capacity - self.bytes.len() {
            return Err("Invalid or overlapping receiver offer".into());
        }
        self.offer = Some((at, bytes));
        Ok(())
    }
    pub fn ready_at(&self, address: u32, request: u8, bytes: usize) -> Option<u64> {
        if address != self.address || request != self.request { return None; }
        self.offer.filter(|(_, n)| *n == bytes).map(|(at, _)| at)
    }
    pub fn accept(&mut self, now: u64, address: u32, request: u8, data: &[u8]) -> Result<(), String> {
        if !self.ready_at(address, request, data.len()).is_some_and(|at| at <= now) {
            return Err("Transmit receiver has not accepted this batch".into());
        }
        self.bytes.extend_from_slice(data);
        self.offer = None;
        Ok(())
    }
    pub fn bytes(&self) -> &[u8] { &self.bytes }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_offers_reject_early_wrong_or_duplicate_delivery() {
        let mut r=TxReceiver::new(0x1000,22,8).unwrap();
        assert!(r.offer(10,12).is_err());
        r.offer(10,8).unwrap();
        assert!(r.offer(10,8).is_err());
        for (time,address,request,n) in [(9,0x1000,22,8),(10,0x1004,22,8),(10,0x1000,23,8),(10,0x1000,22,4)] {
            assert!(r.accept(time,address,request,&vec![1;n]).is_err());
            assert!(r.bytes().is_empty());
        }
        r.accept(10,0x1000,22,&[1,2,3,4,5,6,7,8]).unwrap();
        assert_eq!(r.bytes(), &[1,2,3,4,5,6,7,8]);
        assert!(r.accept(10,0x1000,22,&[0;8]).is_err());
        assert!(r.offer(20,4).is_err());
    }
}
