use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A selected window of a recorded bank. Destination numbers are channel
/// indices; addresses, source files and layouts belong to scenario data.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedSpan {
    pub destination: usize,
    pub destination_offset: u32,
    pub source_offset: usize,
    pub bytes: usize,
}
#[derive(Clone, Debug)]
pub struct Recorded {
    bank: Arc<[u8]>,
    spans: Arc<[RecordedSpan]>,
}
impl Recorded {
    pub fn new(bank: Vec<u8>, spans: Vec<RecordedSpan>) -> Result<Self, String> {
        if bank.is_empty() || bank.len() > 64 << 20 || spans.is_empty() || spans.len() > 256 {
            return Err("recorded bank or span count exceeds its bound".into());
        }
        for (i, s) in spans.iter().enumerate() {
            if s.destination >= 10
                || s.bytes == 0
                || s.bytes & 3 != 0
                || s.destination_offset & 3 != 0
                || s.source_offset
                    .checked_add(s.bytes)
                    .is_none_or(|end| end > bank.len())
                || (s.destination_offset as usize)
                    .checked_add(s.bytes)
                    .is_none_or(|end| end > 64 << 20)
            {
                return Err("recorded span escapes its bank or destination".into());
            }
            if spans[..i].iter().any(|p| {
                p.destination == s.destination
                    && (p.destination_offset as u64) < s.destination_offset as u64 + s.bytes as u64
                    && (s.destination_offset as u64) < p.destination_offset as u64 + p.bytes as u64
            }) {
                return Err("recorded destination spans overlap".into());
            }
        }
        Ok(Self {
            bank: bank.into(),
            spans: spans.into(),
        })
    }
    pub fn spans(&self) -> impl ExactSizeIterator<Item = (&RecordedSpan, &[u8])> {
        self.spans
            .iter()
            .map(|s| (s, &self.bank[s.source_offset..s.source_offset + s.bytes]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_and_immutable_windows() {
        let span = RecordedSpan {
            destination: 0,
            destination_offset: 4,
            source_offset: 0,
            bytes: 4,
        };
        let data = Recorded::new(vec![1, 2, 3, 4], vec![span.clone()]).unwrap();
        assert_eq!(data.spans().next().unwrap().1, &[1, 2, 3, 4]);
        assert!(Recorded::new(vec![0; 4], vec![span.clone(), span.clone()]).is_err());
        assert!(Recorded::new(vec![0; 3], vec![span]).is_err());
    }
}
