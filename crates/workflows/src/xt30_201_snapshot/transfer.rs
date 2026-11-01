//! Conditional retrieval of an already-published, caller-qualified snapshot.
use super::{ControllerSnapshot, SNAPSHOT_BYTES, SnapshotDecodeError, decode};
use fujimod_usb::ff80::{
    Session,
    commands::{
        diagnostic::{CommandStatus, DiagnosticReply},
        memory::MemoryRange,
    },
};
use std::num::NonZeroU32;

#[derive(Debug, thiserror::Error)]
pub enum SnapshotTransferError {
    #[error("{0}")]
    Validation(&'static str),
    #[error(transparent)]
    Usb(#[from] fujimod_usb::Error),
    #[error("invalid snapshot response: {source}")]
    Decode {
        #[source]
        source: SnapshotDecodeError,
        /// Complete response retained even when its padding is invalid.
        raw: Vec<u8>,
    },
}

/// A validated observation of the staged publisher's source/capacity pair.
///
/// The caller must first qualify the installed X-T30 2.01 publisher, owned
/// allocation, active address translation, cache publication and controller
/// reads. Construct this from a fresh successful tag-0x80 query and the eight
/// bytes read in the same exclusive session. Keep
/// that session and publisher serialized until retrieval completes. This type
/// does not authenticate those observations or install/enable the publisher.
/// The unchanged stock callback publishes zero capacity and is rejected.
#[derive(Debug)]
pub struct PublishedSnapshot {
    _buffer: MemoryRange,
}

impl PublishedSnapshot {
    /// Validate the exact owned buffer and query/source observations before E1.
    /// No camera command is sent by this constructor.
    pub fn new(
        buffer: MemoryRange,
        query: &DiagnosticReply,
        source_state: &[u8],
    ) -> Result<Self, SnapshotTransferError> {
        let start = buffer.address().0;
        // Match the publisher's allocation contract, not installed DRAM size.
        if start == 0
            || start % 64 != 0
            || buffer.length().get() != SNAPSHOT_BYTES as u32
            || u64::from(start) + SNAPSHOT_BYTES as u64 > 0x2000_0000
        {
            return Err(SnapshotTransferError::Validation(
                "snapshot buffer must be one aligned 64-byte owned allocation below 0x20000000",
            ));
        }
        let mut expected_query = [0; 32];
        // Validate the command echo, reported capacity and zeroed reserved fields.
        expected_query[..4].copy_from_slice(&0x450006u32.to_le_bytes());
        expected_query[20..24].copy_from_slice(&64u32.to_le_bytes());
        if query.raw != expected_query
            || query.status != CommandStatus::Success
            || query.fields != [0, 64, 0, 0]
        {
            return Err(SnapshotTransferError::Validation(
                "snapshot query must succeed with capacity 64 and zero reserved fields",
            ));
        }
        if source_state.len() != 8 {
            return Err(SnapshotTransferError::Validation(
                "snapshot source state must be eight bytes",
            ));
        }
        let source = u32::from_le_bytes(source_state[..4].try_into().expect("four bytes"));
        let capacity = u32::from_le_bytes(source_state[4..].try_into().expect("four bytes"));
        if source != start || capacity != SNAPSHOT_BYTES as u32 {
            return Err(SnapshotTransferError::Validation(
                "snapshot source state differs from the owned buffer or capacity 64",
            ));
        }
        Ok(Self { _buffer: buffer })
    }

    /// Consume this observation with exactly one offset-zero, 64-byte E1 read.
    ///
    /// No diagnostic query, configuration write, callback replacement or retry occurs.
    /// A failed/partial exchange keeps the session's existing completion fence;
    /// an invalid payload also fences it. Cleanup sends no further traffic.
    /// The caller owns session close and target restoration: retain allocations
    /// and do not attempt target cleanup if completion is unknown. Cancellation
    /// has the same fencing behavior as the underlying Session::read_data.
    pub async fn read(
        self,
        session: &mut Session,
    ) -> Result<ControllerSnapshot, SnapshotTransferError> {
        let bytes = session
            .read_data(0, NonZeroU32::new(SNAPSHOT_BYTES as u32).expect("64"))
            .await?;
        match decode(&bytes) {
            Ok(snapshot) => Ok(snapshot),
            Err(source) => {
                session.invalidate_completion();
                Err(SnapshotTransferError::Decode { source, raw: bytes })
            }
        }
    }
}

#[cfg(test)]
mod tests;
