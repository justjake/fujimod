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
    original: u64,
    pointer_bytes: u32,
    capacity_address: Address,
    capacity: NonZeroU32,
}
impl UploadDestination {
    pub fn new(
        pointer: Address,
        original: u64,
        pointer_bytes: u32,
        capacity_address: Address,
        capacity: NonZeroU32,
    ) -> Result<Self> {
        if !matches!(pointer_bytes, 4 | 8) || pointer_bytes == 4 && original > u64::from(u32::MAX) {
            return Err(Error::InvalidLength);
        }
        if overlaps(range(pointer, pointer_bytes)?, range(capacity_address, 4)?) {
            return Err(Error::OutsideDataWindow);
        }
        Ok(Self {
            pointer,
            original,
            pointer_bytes,
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
        if !alignment.is_power_of_two() || !address.0.is_multiple_of(alignment.get()) {
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
            || overlaps(
                self.allocation,
                range(destination.pointer, destination.pointer_bytes)?,
            )
            || overlaps(self.allocation, range(destination.capacity_address, 4)?)
            || (destination.original >= u64::from(self.allocation.address().0)
                && destination.original < end(self.allocation))
        {
            return Err(Error::OutsideDataWindow);
        }
        let before =
            destination.original.to_le_bytes()[..destination.pointer_bytes as usize].to_vec();
        let during =
            u64::from(self.address.0).to_le_bytes()[..destination.pointer_bytes as usize].to_vec();
        let read_bytes = limits
            .read
            .get()
            .min(session.info().advertised_packet_bytes.get()) as usize;
        let write_bytes = limits
            .write
            .get()
            .min(session.info().advertised_packet_bytes.get()) as usize;
        let guard = |address: Address, bytes: &[u8]| -> Result<Vec<SequenceCommand>> {
            bytes
                .chunks(read_bytes)
                .enumerate()
                .map(|(i, part)| read(Address(address.0 + (i * read_bytes) as u32), part.to_vec()))
                .collect()
        };
        let publish = |address: Address, bytes: &[u8]| -> Vec<SequenceCommand> {
            bytes
                .chunks(write_bytes)
                .enumerate()
                .map(|(i, part)| SequenceCommand::Write {
                    address: Address(address.0 + (i * write_bytes) as u32),
                    bytes: part.to_vec(),
                })
                .collect()
        };
        let mut prepare = guard(destination.pointer, &before)?;
        prepare.extend(guard(
            destination.capacity_address,
            &destination.capacity.get().to_le_bytes(),
        )?);
        prepare.extend(publish(destination.pointer, &during));
        prepare.extend(guard(destination.pointer, &during)?);
        session.sequence(prepare, limits).await?;
        let reply = data::upload(session, &self.bytes, self.piece, DataWriteMode::Preserve).await?;
        let mut restore = guard(destination.pointer, &during)?;
        restore.extend(publish(destination.pointer, &before));
        restore.extend(guard(destination.pointer, &before)?);
        session.sequence(restore, limits).await?;
        Ok(reply)
    }
    /// Transfer a large owned image through a smaller stock staging capacity.
    /// Every chunk independently guards and restores the borrowed destination.
    pub async fn upload_chunks(
        &self,
        session: &mut Session,
        destination: UploadDestination,
        limits: DebugMemoryLimits,
    ) -> Result<()> {
        let chunk = destination.capacity.get() / self.piece * self.piece.get();
        if chunk == 0 {
            return Err(Error::InvalidLength);
        }
        for offset in (0..self.image_bytes() as usize).step_by(chunk as usize) {
            let end = (offset + chunk as usize).min(self.image_bytes() as usize);
            let image = Self::new(
                self.allocation,
                Address(self.address.0 + offset as u32),
                self.bytes[offset..end].to_vec(),
                self.piece,
                NonZeroU32::new(1).unwrap(),
            )?;
            image.upload(session, destination, limits).await?;
        }
        Ok(())
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
