//! Device-owned word FIFO with a shared read cursor. The final word remains
//! an owner callback so completion, IRQs and deadlines stay with the device.
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct ReadFifo(Arc<Mutex<State>>);
struct State {
    bytes: Vec<u8>,
    position: usize,
    published: bool,
}
impl ReadFifo {
    pub fn new(bytes: usize) -> Result<Self, String> {
        if bytes == 0 || !bytes.is_multiple_of(4) {
            return Err("word FIFO capacity must be a positive multiple of four".into());
        }
        Ok(Self(Arc::new(Mutex::new(State {
            bytes: vec![0; bytes],
            position: 0,
            published: false,
        }))))
    }
    pub fn snapshot(&self) -> (Vec<u8>, usize) {
        let state = self.0.lock().unwrap();
        (state.bytes.clone(), state.position)
    }
    pub fn position(&self) -> usize {
        self.0.lock().unwrap().position
    }
    pub fn validate(&self, bytes: &[u8], position: usize) -> Result<(), String> {
        let state = self.0.lock().unwrap();
        if bytes.len() != state.bytes.len() || position > bytes.len() || !position.is_multiple_of(4)
        {
            return Err("word FIFO data or cursor differs from its capacity".into());
        }
        Ok(())
    }
    pub fn restore(&self, bytes: &[u8], position: usize) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        if bytes.len() != state.bytes.len() || position > bytes.len() || !position.is_multiple_of(4)
        {
            return Err("word FIFO data or cursor differs from its capacity".into());
        }
        state.bytes.copy_from_slice(bytes);
        state.position = position;
        state.published = false;
        Ok(())
    }
    pub fn rewind(&self) {
        let mut state = self.0.lock().unwrap();
        state.position = 0;
        state.published = false;
    }
    pub fn publish(&self, enabled: bool) {
        self.0.lock().unwrap().published = enabled;
    }
    pub fn read_fast(&self) -> Option<u32> {
        let mut state = self.0.lock().unwrap();
        if !state.published || state.position + 4 >= state.bytes.len() {
            return None;
        }
        Self::take(&mut state)
    }
    pub fn read(&self) -> Option<u32> {
        Self::take(&mut self.0.lock().unwrap())
    }
    fn take(state: &mut State) -> Option<u32> {
        let value = u32::from_le_bytes(
            state
                .bytes
                .get(state.position..state.position + 4)?
                .try_into()
                .unwrap(),
        );
        state.position += 4;
        Some(value)
    }
    pub fn write(&self, value: u32) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        let position = state.position;
        let target = state
            .bytes
            .get_mut(position..position + 4)
            .ok_or("word FIFO is full")?;
        target.copy_from_slice(&value.to_le_bytes());
        state.position += 4;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_reads_share_the_cursor_and_reserve_completion_for_the_owner() {
        let fifo = ReadFifo::new(12).unwrap();
        fifo.restore(
            &[1u32, 2, 3]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
            0,
        )
        .unwrap();
        assert_eq!(fifo.read_fast(), None);
        fifo.publish(true);
        let reader = fifo.clone();
        assert_eq!(reader.read_fast(), Some(1));
        assert_eq!(fifo.position(), 4);
        let saved = fifo.snapshot();
        assert_eq!(reader.read_fast(), Some(2));
        assert_eq!(reader.read_fast(), None);
        assert_eq!(fifo.position(), 8);
        assert_eq!(fifo.read(), Some(3));
        assert_eq!(fifo.read(), None);
        fifo.restore(&saved.0, saved.1).unwrap();
        assert_eq!(reader.read_fast(), None);
        fifo.publish(true);
        assert_eq!(reader.read_fast(), Some(2));
        assert!(fifo.restore(&saved.0, 3).is_err());
        assert!(fifo.restore(&saved.0, 16).is_err());
    }
}
