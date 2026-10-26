use fujimod_usb::{
    Error, Result,
    ff80::{
        Session,
        commands::{
            diagnostic::DataWriteMode,
            memory::{Address, DebugMemoryLimits, MemoryRange},
        },
        sequence::SequenceCommand,
    },
    operations::data::{self, DataReader, DataWindow, UploadReply},
};
use std::num::NonZeroU32;

pub fn padded_size(length: u32, piece: NonZeroU32) -> Result<NonZeroU32> {
    if length == 0 {
        return Err(Error::InvalidLength);
    }
    length
        .checked_add(piece.get() - 1)
        .and_then(|n| n.checked_div(piece.get()))
        .and_then(|n| n.checked_mul(piece.get()))
        .and_then(NonZeroU32::new)
        .ok_or(Error::InvalidLength)
}
fn range(address: Address, length: u32) -> Result<MemoryRange> {
    MemoryRange::new(
        address,
        NonZeroU32::new(length).ok_or(Error::InvalidLength)?,
    )
}
fn end(r: MemoryRange) -> u64 {
    u64::from(r.address().0) + u64::from(r.length().get())
}
fn overlaps(a: MemoryRange, b: MemoryRange) -> bool {
    u64::from(a.address().0) < end(b) && u64::from(b.address().0) < end(a)
}
fn read(address: Address, bytes: Vec<u8>) -> Result<SequenceCommand> {
    Ok(SequenceCommand::Read {
        range: range(address, bytes.len() as u32)?,
        expected: Some(bytes),
    })
}

/// Caller-qualified stock upload pointer and capacity words. Pointer addresses
/// and capacity are supplied explicitly by the caller.
#[derive(Clone, Copy)]
pub struct UploadDestination {
    pointer: Address,
    original: Address,
    capacity_address: Address,
    capacity: NonZeroU32,
}
impl UploadDestination {
    pub fn new(
        pointer: Address,
        original: Address,
        capacity_address: Address,
        capacity: NonZeroU32,
    ) -> Result<Self> {
        if overlaps(range(pointer, 4)?, range(capacity_address, 4)?) {
            return Err(Error::OutsideDataWindow);
        }
        Ok(Self {
            pointer,
            original,
            capacity_address,
            capacity,
        })
    }
}

/// Image and its padded transport extent, both contained in a caller-owned
/// allocation in the same CPU address view used by the upload and readback.
#[derive(Clone)]
pub struct OwnedUpload {
    allocation: MemoryRange,
    address: Address,
    image_length: NonZeroU32,
    piece: NonZeroU32,
    bytes: Vec<u8>,
}
impl OwnedUpload {
    pub fn new(
        allocation: MemoryRange,
        address: Address,
        mut bytes: Vec<u8>,
        piece: NonZeroU32,
        alignment: NonZeroU32,
    ) -> Result<Self> {
        if !alignment.get().is_power_of_two() || !address.0.is_multiple_of(alignment.get()) {
            return Err(Error::InvalidLength);
        }
        let image_length = u32::try_from(bytes.len())
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or(Error::InvalidLength)?;
        let size = padded_size(image_length.get(), piece)?;
        let padded = MemoryRange::new(address, size)?;
        if address.0 < allocation.address().0 || end(padded) > end(allocation) {
            return Err(Error::OutsideDataWindow);
        }
        bytes.resize(size.get() as usize, 0);
        Ok(Self {
            allocation,
            address,
            image_length,
            piece,
            bytes,
        })
    }
    pub fn image_bytes(&self) -> u32 {
        self.image_length.get()
    }
    pub fn padded_bytes(&self) -> u32 {
        self.bytes.len() as u32
    }
    /// Borrow, transfer, and restore with readback guards at the USB owner.
    /// An error or cancellation never retries, frees memory or repairs a pointer.
    /// The caller must retain the allocation until ownership/completion is known.
    pub async fn upload(
        &self,
        session: &mut Session,
        destination: UploadDestination,
        limits: DebugMemoryLimits,
    ) -> Result<UploadReply> {
        if self.padded_bytes() > destination.capacity.get()
            || overlaps(self.allocation, range(destination.pointer, 4)?)
            || overlaps(self.allocation, range(destination.capacity_address, 4)?)
            || (destination.original.0 >= self.allocation.address().0
                && u64::from(destination.original.0) < end(self.allocation))
        {
            return Err(Error::OutsideDataWindow);
        }
        let before = destination.original.0.to_le_bytes().to_vec();
        let during = self.address.0.to_le_bytes().to_vec();
        session
            .sequence(
                vec![
                    read(destination.pointer, before.clone())?,
                    read(
                        destination.capacity_address,
                        destination.capacity.get().to_le_bytes().to_vec(),
                    )?,
                    SequenceCommand::Write {
                        address: destination.pointer,
                        bytes: during.clone(),
                    },
                    read(destination.pointer, during.clone())?,
                ],
                limits,
            )
            .await?;
        let reply = data::upload(session, &self.bytes, self.piece, DataWriteMode::Preserve).await?;
        session
            .sequence(
                vec![
                    read(destination.pointer, during)?,
                    SequenceCommand::Write {
                        address: destination.pointer,
                        bytes: before.clone(),
                    },
                    read(destination.pointer, before)?,
                ],
                limits,
            )
            .await?;
        Ok(reply)
    }
    /// Compare every image byte through the caller's qualified bulk window.
    /// Padding is transport space and is not part of the executable image.
    pub async fn verify(&self, session: &mut Session, window: DataWindow) -> Result<()> {
        let mut reader = DataReader::new(
            &mut *session,
            window,
            MemoryRange::new(self.address, self.image_length)?,
        )?;
        let mut offset = 0;
        while let Some(bytes) = reader.next_chunk().await? {
            if bytes != self.bytes[offset..offset + bytes.len()] {
                reader.into_inner().invalidate_completion();
                return Err(fujimod_usb::error::ProtocolError::ExpectationMismatch.into());
            }
            offset += bytes.len();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
