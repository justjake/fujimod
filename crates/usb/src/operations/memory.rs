use crate::{
    Error, Result,
    ff80::{
        Session,
        commands::memory::{Address, DebugMemoryLimits, MemoryRange, ReadMemory},
    },
};
use std::{borrow::BorrowMut, num::NonZeroU32};

/// Pull-based range download, one bounded command per `next_chunk().await`.
///
/// Retains progress without accumulating the full range. If a chunk fails,
/// previously returned chunks remain valid and no retry is issued automatically.
pub struct MemoryReader<S> {
    session: S,
    range: MemoryRange,
    limits: DebugMemoryLimits,
    offset: u32,
}
impl<S: BorrowMut<Session>> MemoryReader<S> {
    pub fn new(session: S, range: MemoryRange, limits: DebugMemoryLimits) -> Self {
        Self {
            session,
            range,
            limits,
            offset: 0,
        }
    }
    pub fn into_inner(self) -> S {
        self.session
    }
    pub fn bytes_completed(&self) -> u32 {
        self.offset
    }
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.offset == self.range.length().get() {
            return Ok(None);
        }
        let session = self.session.borrow_mut();
        let budget = session.max_reply_bytes().get();
        let maximum = self
            .limits
            .read
            .get()
            .min(session.info().advertised_packet_bytes.get())
            .min(budget.saturating_sub(36));
        if maximum == 0 {
            return Err(Error::LengthLimit {
                requested: 37,
                limit: budget,
            });
        }
        let Some(range) = chunk(self.range, self.offset, maximum) else {
            return Ok(None);
        };
        let data = session
            .read_memory(ReadMemory::new(range, self.limits)?)
            .await?;
        self.offset += range.length().get();
        Ok(Some(data))
    }
}

fn chunk(range: MemoryRange, offset: u32, maximum: u32) -> Option<MemoryRange> {
    let remaining = range.length().get().checked_sub(offset)?;
    let length = NonZeroU32::new(remaining.min(maximum))?;
    Some(
        MemoryRange::new(Address(range.address().0.checked_add(offset)?), length)
            .expect("subrange of a validated range"),
    )
}

/// Commands per wait for whole-range reads and writes: over a device share
/// they stream as one pipeline, so a range waits once per 1024 commands (its
/// four USB steps each fill the controller's 4096-operation window).
const BATCH_COMMANDS: usize = 1024;

/// Collect a range, a batch of bounded commands at a time.
pub async fn read_range(
    session: &mut Session,
    range: MemoryRange,
    limits: DebugMemoryLimits,
) -> Result<Vec<u8>> {
    let budget = session.max_reply_bytes().get();
    let maximum = limits
        .read
        .get()
        .min(session.info().advertised_packet_bytes.get())
        .min(budget.saturating_sub(36));
    if maximum == 0 {
        return Err(Error::LengthLimit {
            requested: 37,
            limit: budget,
        });
    }
    let mut commands = Vec::new();
    let mut offset = 0;
    while let Some(part) = chunk(range, offset, maximum) {
        commands.push(ReadMemory::new(part, limits)?);
        offset += part.length().get();
    }
    let mut data = Vec::with_capacity(range.length().get() as usize);
    let mut commands = commands.into_iter().peekable();
    while commands.peek().is_some() {
        let group: Vec<ReadMemory> = commands.by_ref().take(BATCH_COMMANDS).collect();
        for part in session.read_memory_many(group).await? {
            data.extend_from_slice(&part);
        }
    }
    Ok(data)
}

/// Composed write: each firmware request remains independently bounded.
pub async fn write_range(
    session: &mut Session,
    address: Address,
    bytes: &[u8],
    limits: DebugMemoryLimits,
) -> Result<()> {
    use crate::ff80::commands::memory::WriteMemory;
    let length = u32::try_from(bytes.len())
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or(Error::InvalidLength)?;
    MemoryRange::new(address, length)?;
    let maximum = limits
        .write
        .get()
        .min(session.info().advertised_packet_bytes.get()) as usize;
    let mut commands = Vec::new();
    for (offset, part) in bytes.chunks(maximum).enumerate() {
        let at = address.0 + u32::try_from(offset * maximum).map_err(|_| Error::AddressOverflow)?;
        commands.push(WriteMemory::new(Address(at), part.to_vec(), limits)?);
    }
    let mut commands = commands.into_iter().peekable();
    while commands.peek().is_some() {
        let group: Vec<WriteMemory> = commands.by_ref().take(BATCH_COMMANDS).collect();
        session.write_memory_many(group).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_chunks_include_short_tail_and_final_address() {
        let range = MemoryRange::new(Address(u32::MAX - 5), NonZeroU32::new(6).unwrap()).unwrap();
        assert_eq!(chunk(range, 0, 4).unwrap().length().get(), 4);
        assert_eq!(chunk(range, 4, 4).unwrap().address(), Address(u32::MAX - 1));
        assert_eq!(chunk(range, 4, 4).unwrap().length().get(), 2);
        assert!(chunk(range, 6, 4).is_none());
    }
}
