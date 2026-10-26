//! A host-only capture memo. Keys describe device registers/options, and every
//! addressed byte is checked again before reusing a rendered frame. No guest
//! instruction, driver state, clock, acknowledgement or checkpoint is consulted.
use crate::frame::Result;

pub struct CaptureCache<T> {
    key: Vec<u8>,
    reads: Vec<(u64, Vec<u8>)>,
    frame: Option<T>,
    pub hits: u64,
    pub renders: u64,
}
impl<T> Default for CaptureCache<T> {
    fn default() -> Self {
        Self {
            key: Vec::new(),
            reads: Vec::new(),
            frame: None,
            hits: 0,
            renders: 0,
        }
    }
}
impl<T: Clone> CaptureCache<T> {
    pub fn capture<R, F>(&mut self, key: Vec<u8>, mut read: R, render: F) -> Result<T>
    where
        R: FnMut(u64, usize) -> Result<Vec<u8>>,
        F: FnOnce(&mut dyn FnMut(u64, usize) -> Result<Vec<u8>>) -> Result<T>,
    {
        if self.key == key
            && let Some(frame) = self.frame.as_ref()
        {
            let mut same = true;
            for (address, bytes) in &self.reads {
                if read(*address, bytes.len())? != *bytes {
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
        self.key = key;
        self.reads = reads;
        self.frame = Some(frame.clone());
        self.renders += 1;
        Ok(frame)
    }
}
