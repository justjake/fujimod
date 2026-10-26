//! Memory ranges composed from E1 data requests using an explicit firmware map.
use crate::{
    Error, Result,
    ff80::{
        Session,
        commands::memory::{Address, MemoryRange},
    },
};
use std::{borrow::BorrowMut, num::NonZeroU32};

/// How the selected firmware turns an E1 offset into an address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OffsetMode {
    /// Only nonnegative offsets from the diagnostic pointer are allowed.
    Checked,
    /// Firmware adds an unsigned offset modulo 2^32.
    Wrapping,
}

/// Firmware-qualified E1 mapping, supplied by the caller after setup/validation.
///
/// `capacity` limits each command, not the total memory range. The aperture is
/// the allowed CPU address range. Neither is inferred from a camera model name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataWindow {
    base: Address,
    aperture: MemoryRange,
    capacity: NonZeroU32,
    mode: OffsetMode,
}
impl DataWindow {
    pub fn new(
        base: Address,
        aperture: MemoryRange,
        capacity: NonZeroU32,
        mode: OffsetMode,
    ) -> Result<Self> {
        if mode == OffsetMode::Checked && aperture.address().0 < base.0 {
            return Err(Error::OutsideDataWindow);
        }
        Ok(Self {
            base,
            aperture,
            capacity,
            mode,
        })
    }
    pub fn capacity(self) -> NonZeroU32 {
        self.capacity
    }
    pub fn validate(self, range: MemoryRange) -> Result<()> {
        let end = |r: MemoryRange| u64::from(r.address().0) + u64::from(r.length().get());
        if range.address().0 < self.aperture.address().0 || end(range) > end(self.aperture) {
            return Err(Error::OutsideDataWindow);
        }
        Ok(())
    }
    fn offset(self, address: Address) -> Result<u32> {
        match self.mode {
            OffsetMode::Checked => address
                .0
                .checked_sub(self.base.0)
                .ok_or(Error::OutsideDataWindow),
            OffsetMode::Wrapping => Ok(address.0.wrapping_sub(self.base.0)),
        }
    }
}

/// One bounded E1 command per pull, with no automatic query, setup or retries.
///
/// Returned chunks stay with the caller. On a failed command, `PartialRead`
/// retains bytes received within that command and the session remains fenced.
pub struct DataReader<S> {
    session: S,
    window: DataWindow,
    range: MemoryRange,
    completed: u32,
}
impl<S: BorrowMut<Session>> DataReader<S> {
    pub fn new(session: S, window: DataWindow, range: MemoryRange) -> Result<Self> {
        window.validate(range)?;
        Ok(Self {
            session,
            window,
            range,
            completed: 0,
        })
    }
    pub fn bytes_completed(&self) -> u32 {
        self.completed
    }
    pub fn into_inner(self) -> S {
        self.session
    }
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.completed == self.range.length().get() {
            return Ok(None);
        }
        let session = self.session.borrow_mut();
        let count = (self.range.length().get() - self.completed)
            .min(self.window.capacity.get())
            .min(session.max_reply_bytes().get());
        let address = Address(self.range.address().0 + self.completed);
        let offset = self.window.offset(address)?;
        let bytes = session
            .read_data(offset, NonZeroU32::new(count).expect("nonzero bounds"))
            .await?;
        self.completed += count;
        Ok(Some(bytes))
    }
}

/// Completed transport stages of a prepared upload, including the full E8 reply.
/// A 32-byte reply is checked against the setup command and its status fields.
/// Neither reply form certifies that application data was consumed or executed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadReply {
    pub setup: crate::ff80::commands::diagnostic::DiagnosticReply,
    pub acknowledgment: Vec<u8>,
    pub bytes_written: u32,
}

/// Prepare a diagnostic write, send bounded E0 pieces, then request E8 once.
///
/// Piece size and setup mode are explicit firmware/workflow inputs. There is no
/// padding, retry, readback, destination-address selection or buffer ownership
/// policy here. Borrowing the session serializes this entire composition.
pub async fn upload(
    session: &mut Session,
    data: &[u8],
    piece_bytes: NonZeroU32,
    mode: crate::ff80::commands::diagnostic::DataWriteMode,
) -> Result<UploadReply> {
    use crate::ff80::commands::diagnostic::CommandStatus;
    let length = u32::try_from(data.len())
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or(Error::InvalidLength)?;
    // Both setup and the final E8 require a 32-byte host receive budget. Refuse
    // before preparing the camera if the complete operation cannot fit.
    crate::error::check_limit(32, session.max_reply_bytes().get())?;
    let setup = session.prepare_data_write(length, mode).await?;
    if let CommandStatus::DeviceError { primary, secondary } = setup.status {
        return Err(Error::DeviceStatus {
            command: 0x760001,
            primary,
            secondary,
        });
    }
    for part in data.chunks(piece_bytes.get() as usize) {
        session.write_data(part).await?;
    }
    let acknowledgment = session.empty_response().await?;
    if acknowledgment.len() == 32 {
        // X70 hardware also returns the setup command's complete reply on E8.
        // A framed refusal is completed; malformed ownership/status framing is not.
        if let Err(error) = crate::ff80::commands::status(&acknowledgment, 0x760001, 32) {
            if matches!(error, Error::Protocol(_)) {
                session.invalidate_completion();
            }
            return Err(error);
        }
    }
    Ok(UploadReply {
        setup,
        acknowledgment,
        bytes_written: length.get(),
    })
}

#[cfg(test)]
mod tests;
