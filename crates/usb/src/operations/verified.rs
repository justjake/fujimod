//! Planned write/readback pairs for authorized memory ranges.
use crate::{
    Error, Result,
    ff80::{
        commands::memory::{Address, DebugMemoryLimits, MemoryRange},
        sequence::SequenceCommand,
    },
};
use std::num::NonZeroU32;

/// Plan all bounded write/readback pairs before the first USB command. Each
/// expected read runs at the USB owner and discards later work on mismatch.
/// The caller must authorize the complete range before executing these batches.
pub fn verified_writes(
    address: Address,
    bytes: &[u8],
    limits: DebugMemoryLimits,
    packet_bytes: NonZeroU32,
) -> Result<Vec<Vec<SequenceCommand>>> {
    let length = u32::try_from(bytes.len()).map_err(|_| Error::InvalidLength)?;
    MemoryRange::new(
        address,
        NonZeroU32::new(length).ok_or(Error::InvalidLength)?,
    )?;
    let chunk = limits
        .read
        .get()
        .min(limits.write.get())
        .min(packet_bytes.get()) as usize;
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    for (index, piece) in bytes.chunks(chunk).enumerate() {
        let at = Address((u64::from(address.0) + (index * chunk) as u64) as u32);
        let range = MemoryRange::new(
            at,
            NonZeroU32::new(piece.len() as u32).expect("nonempty piece"),
        )?;
        batch.push(SequenceCommand::Write {
            address: at,
            bytes: piece.to_vec(),
        });
        batch.push(SequenceCommand::Read {
            range,
            expected: Some(piece.to_vec()),
        });
        if batch.len() == 512 {
            batches.push(std::mem::take(&mut batch));
        }
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    Ok(batches)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn nz(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }
    fn limits() -> DebugMemoryLimits {
        DebugMemoryLimits {
            read: nz(256),
            write: nz(256),
        }
    }
    #[test]
    fn verified_writes_preserve_tails_and_sequence_bounds() {
        let bytes = vec![0x5a; 256 * 300 + 3];
        let batches = verified_writes(Address(0x10000), &bytes, limits(), nz(256)).unwrap();
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [512, 90]);
        let SequenceCommand::Read { range, expected } = batches[1].last().unwrap() else {
            panic!()
        };
        assert_eq!(range.length().get(), 3);
        assert_eq!(expected.as_ref().unwrap(), &[0x5a; 3]);
        assert!(verified_writes(Address(u32::MAX), b"xx", limits(), nz(256)).is_err());
        assert!(verified_writes(Address(u32::MAX), b"x", limits(), nz(256)).is_ok());
        let small = verified_writes(
            Address(0),
            b"12345",
            DebugMemoryLimits {
                read: nz(2),
                write: nz(3),
            },
            nz(4),
        )
        .unwrap();
        assert_eq!(small[0].len(), 6);
    }
}
