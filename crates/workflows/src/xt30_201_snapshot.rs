//! Decoder for the staged X-T30 2.01 seven-word controller snapshot.
//!
//! Decode returned bytes or retrieve one already-published, caller-qualified
//! RAM snapshot through one E1 read. Neither operation installs the callback
//! or establishes controller-read safety or DRAM capacity.

mod transfer;
pub use transfer::{PublishedSnapshot, SnapshotTransferError};

pub const STATUS_ADDRESSES: [u32; 7] = [
    0xff50_7c18,
    0xff50_7044,
    0xff50_7364,
    0xff60_2110,
    0xff60_4670,
    0xff60_4674,
    0xff60_4678,
];
pub const SNAPSHOT_BYTES: usize = 64;
const WORD_BYTES: usize = 4;
const PADDING_START: usize = STATUS_ADDRESSES.len() * WORD_BYTES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerSnapshot {
    /// Original bulk-IN response, including its reserved zero padding.
    pub raw: [u8; SNAPSHOT_BYTES],
    /// Little-endian values in `STATUS_ADDRESSES` order.
    pub words: [u32; STATUS_ADDRESSES.len()],
}

impl ControllerSnapshot {
    /// Pair each decoded value with the fixed register address it represents.
    pub fn addressed_words(&self) -> [(u32, u32); STATUS_ADDRESSES.len()] {
        std::array::from_fn(|index| (STATUS_ADDRESSES[index], self.words[index]))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotDecodeError {
    #[error("expected a 64-byte controller snapshot, got {0} bytes")]
    Length(usize),
    #[error("controller snapshot padding at byte {0} is nonzero")]
    NonzeroPadding(usize),
}

/// Decode the fixed snapshot published by `controller_bulk_query`.
///
/// The 64-byte transfer contains seven 32-bit words followed by 36 zero bytes.
/// It is deliberately not an arbitrary address/range reader.
pub fn decode(bytes: &[u8]) -> Result<ControllerSnapshot, SnapshotDecodeError> {
    let raw: [u8; SNAPSHOT_BYTES] = bytes
        .try_into()
        .map_err(|_| SnapshotDecodeError::Length(bytes.len()))?;
    if let Some(index) = raw[PADDING_START..].iter().position(|byte| *byte != 0) {
        return Err(SnapshotDecodeError::NonzeroPadding(PADDING_START + index));
    }
    let mut words = [0; STATUS_ADDRESSES.len()];
    for (index, word) in words.iter_mut().enumerate() {
        let start = index * WORD_BYTES;
        *word = u32::from_le_bytes(raw[start..start + WORD_BYTES].try_into().unwrap());
    }
    Ok(ControllerSnapshot { raw, words })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_seven_little_endian_status_words_and_keeps_raw_response() {
        let values: [u32; 7] = [
            0x4000_0001,
            0x0000_0000,
            0x0000_0001,
            0x1234_5678,
            0x8765_4321,
            0x0000_0002,
            0xffff_fffe,
        ];
        let mut raw = [0; SNAPSHOT_BYTES];
        for (index, value) in values.iter().enumerate() {
            let start = index * WORD_BYTES;
            raw[start..start + WORD_BYTES].copy_from_slice(&value.to_le_bytes());
        }
        let decoded = decode(&raw).unwrap();
        assert_eq!(decoded.words, values);
        assert_eq!(decoded.raw, raw);
        assert_eq!(STATUS_ADDRESSES.len(), decoded.words.len());
        assert_eq!(
            decoded.addressed_words(),
            std::array::from_fn(|index| (STATUS_ADDRESSES[index], values[index]))
        );
    }

    #[test]
    fn rejects_short_and_long_transfers() {
        assert_eq!(
            decode(&[0; SNAPSHOT_BYTES - 1]),
            Err(SnapshotDecodeError::Length(63))
        );
        assert_eq!(
            decode(&[0; SNAPSHOT_BYTES + 1]),
            Err(SnapshotDecodeError::Length(65))
        );
    }

    #[test]
    fn rejects_nonzero_padding() {
        let mut raw = [0; SNAPSHOT_BYTES];
        raw[PADDING_START + 4] = 1;
        assert_eq!(
            decode(&raw),
            Err(SnapshotDecodeError::NonzeroPadding(PADDING_START + 4))
        );
    }
}
