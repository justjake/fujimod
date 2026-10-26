//! Fuji method-4 tokens. A split token emits only when its complete payload
//! arrives; overlapping matches repeat the preceding distance-byte seed.
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Stream {
    history: Vec<u8>,
    produced: u64,
    pending: Vec<u8>,
    pending_len: usize,
    pub literals: usize,
    pub matches: usize,
    failed: bool,
}
impl Default for Stream {
    fn default() -> Self {
        Self {
            history: vec![0; 2048],
            produced: 0,
            pending: vec![0; 128],
            pending_len: 0,
            literals: 0,
            matches: 0,
            failed: false,
        }
    }
}
impl Stream {
    pub fn reset(&mut self) {
        self.produced = 0;
        self.pending_len = 0;
        self.literals = 0;
        self.matches = 0;
        self.failed = false;
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.history.len() != 2048 || self.pending.len() != 128 || self.pending_len > 128 {
            return Err("invalid method-4 stream checkpoint".into());
        }
        if !self.failed && self.pending_len != 0 {
            let token = self.pending[0];
            let need = if token < 128 { token as usize + 1 } else { 2 };
            if token == 0 || self.pending_len >= need {
                return Err("invalid method-4 pending token".into());
            }
        }
        Ok(())
    }
    pub fn pending_bytes(&self) -> usize {
        self.pending_len
    }
    pub fn produced(&self) -> u64 {
        self.produced
    }
    fn emit(&mut self, byte: u8, out: &mut [u8], count: &mut usize) -> Result<(), String> {
        let slot = out
            .get_mut(*count)
            .ok_or("method-4 output exceeds selected bound")?;
        *slot = byte;
        self.history[self.produced as usize & 2047] = byte;
        self.produced = self
            .produced
            .checked_add(1)
            .ok_or("method-4 stream size overflow")?;
        *count += 1;
        Ok(())
    }
    pub fn feed(&mut self, source: &[u8], out: &mut [u8]) -> Result<usize, String> {
        self.validate()?;
        if self.failed {
            return Err("method-4 stream needs reset after error".into());
        }
        let result = self.feed_tokens(source, out);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn feed_tokens(&mut self, source: &[u8], out: &mut [u8]) -> Result<usize, String> {
        let mut count = 0;
        for &byte in source {
            self.pending[self.pending_len] = byte;
            self.pending_len += 1;
            let token = self.pending[0];
            if token == 0 {
                return Err("zero-length method-4 literal".into());
            }
            let need = if token < 128 { token as usize + 1 } else { 2 };
            if self.pending_len < need {
                continue;
            }
            if token < 128 {
                if count + token as usize > out.len() {
                    return Err("method-4 output exceeds selected bound".into());
                }
                for n in 1..need {
                    self.emit(self.pending[n], out, &mut count)?;
                }
                self.literals = self
                    .literals
                    .checked_add(1)
                    .ok_or("method-4 literal counter overflow")?;
            } else {
                let distance = ((token as usize & 127) << 4) | (byte as usize >> 4);
                let length = byte as usize & 15;
                if distance == 0 || distance as u64 > self.produced || length == 0 {
                    return Err("invalid method-4 back reference".into());
                }
                if count + length > out.len() {
                    return Err("method-4 output exceeds selected bound".into());
                }
                for _ in 0..length {
                    let byte =
                        self.history[(self.produced.wrapping_sub(distance as u64) & 2047) as usize];
                    self.emit(byte, out, &mut count)?;
                }
                self.matches = self
                    .matches
                    .checked_add(1)
                    .ok_or("method-4 match counter overflow")?;
            }
            self.pending_len = 0;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_split_preserves_literal_and_overlapping_match_tokens() {
        let bytes = [3, b'a', b'b', b'c', 0x80, 0x3f, 2, b'd', b'e'];
        for split in 0..=bytes.len() {
            let mut s = Stream::default();
            let mut out = [0; 40];
            let a = s.feed(&bytes[..split], &mut out).unwrap();
            let b = s.feed(&bytes[split..], &mut out[a..]).unwrap();
            assert_eq!(&out[..a + b], b"abcabcabcabcabcabcde");
            assert_eq!((s.pending_bytes(), s.literals, s.matches), (0, 2, 1));
        }
    }
    #[test]
    fn bounded_history_wraps_and_bad_tokens_refuse() {
        let mut s = Stream::default();
        let mut out = [0; 127];
        for n in 0..40 {
            let mut token = [n; 128];
            token[0] = 127;
            s.feed(&token, &mut out).unwrap();
        }
        assert_eq!(s.feed(&[0x80, 0x1f], &mut out).unwrap(), 15);
        assert_eq!(&out[..15], &[39; 15]);
        assert!(Stream::default().feed(&[0x80, 0x11], &mut out).is_err());
        assert!(Stream::default().feed(&[0], &mut out).is_err());
        assert!(Stream::default().feed(&[2, 1, 2], &mut [0; 1]).is_err());
    }
    #[test]
    fn failed_and_long_streams_have_safe_checkpoint_state() {
        let mut s = Stream::default();
        let mut out = [0; 20];
        assert!(s.feed(&[0x80, 0x11], &mut out).is_err());
        let mut restored: Stream =
            serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        restored.validate().unwrap();
        assert!(restored.feed(&[1, 9], &mut out).is_err());
        restored.reset();
        restored.feed(&[1, 9], &mut out).unwrap();
        restored.produced = 1u64 << 32;
        restored.history[2047] = 42;
        assert_eq!(restored.feed(&[0x80, 0x1f], &mut out).unwrap(), 15);
        assert_eq!(&out[..15], &[42; 15]);
    }
    #[test]
    fn every_distance_length_and_match_split_uses_the_same_bounded_dictionary() {
        let mut seed = Stream::default();
        seed.produced = 2047;
        for n in 0..2047 {
            seed.history[n] = (n * 37 + 19) as u8;
        }
        for distance in 1..2048 {
            for length in 1..16 {
                for split in 0..3 {
                    let mut s = seed.clone();
                    let token = [
                        0x80 | (distance >> 4) as u8,
                        ((distance & 15) << 4 | length) as u8,
                    ];
                    let mut out = [0; 15];
                    let n = s.feed(&token[..split], &mut out).unwrap();
                    let count = s.feed(&token[split..], &mut out[n..]).unwrap() + n;
                    assert_eq!(count, length);
                    for i in 0..length {
                        assert_eq!(out[i], seed.history[2047 - distance + i % distance]);
                    }
                }
            }
        }
    }
}
