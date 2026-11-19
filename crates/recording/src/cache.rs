//! A host-only capture memo. Keys describe device registers/options, and every
//! addressed byte is checked again before reusing a rendered frame. No guest
//! instruction, driver state, clock, acknowledgement or checkpoint is consulted.
use crate::frame::Result;

pub struct CaptureCache<K, T> {
    key: Option<K>,
    reads: Vec<(u64, Vec<u8>)>,
    frame: Option<T>,
    pub hits: u64,
    pub renders: u64,
}
impl<K, T> Default for CaptureCache<K, T> {
    fn default() -> Self {
        Self {
            key: None,
            reads: Vec::new(),
            frame: None,
            hits: 0,
            renders: 0,
        }
    }
}
impl<K: PartialEq, T: Clone> CaptureCache<K, T> {
    pub fn capture<R, C, F>(&mut self, key: K, mut read: R, mut compare: C, render: F) -> Result<T>
    where
        R: FnMut(u64, usize) -> Result<Vec<u8>>,
        C: FnMut(u64, &[u8]) -> Result<bool>,
        F: FnOnce(&mut dyn FnMut(u64, usize) -> Result<Vec<u8>>) -> Result<T>,
    {
        if self.key.as_ref() == Some(&key)
            && let Some(frame) = self.frame.as_ref()
        {
            let mut same = true;
            for (address, bytes) in &self.reads {
                if !compare(*address, bytes)? {
                    same = false;
                    break;
                }
            }
            if same {
                self.hits += 1;
                return Ok(frame.clone());
            }
        }
        let mut reads = Vec::new();
        let frame = render(&mut |address, count| {
            let bytes = read(address, count)?;
            reads.push((address, bytes.clone()));
            Ok(bytes)
        })?;
        self.key = Some(key);
        self.reads = reads;
        self.frame = Some(frame.clone());
        self.renders += 1;
        Ok(frame)
    }
}
