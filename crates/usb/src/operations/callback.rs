//! Guarded callback invocation, adapted with permission from Tal Shiri's
//! X-Turbo (src/fuji/reset.py and src/fuji/usb.py).
//! Callers supply all addresses, code bytes, synchronization and trigger facts.
use super::memory::{read_range, write_range};
use crate::{
    Error, Result,
    error::{ProtocolError, TransferStage, check_limit},
    ff80::{
        Session, SessionState,
        commands::{
            memory::{Address, DebugMemoryLimits, MemoryRange},
            research,
        },
        framing::RequestKind,
        sequence::{CheckedCommand, SequenceCommand},
    },
};
use std::num::NonZeroU32;

#[derive(Clone, Debug)]
pub struct Precondition {
    pub range: MemoryRange,
    pub accepted: Vec<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct GuardedWrite {
    pub address: Address,
    pub accepted: Vec<Vec<u8>>,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct CallbackSlot {
    pub address: Address,
    pub required: Vec<u8>,
    pub value: Vec<u8>,
}
#[derive(Clone, Debug)]
pub enum ExpectedOutcome {
    /// Exact accepted raw replies, including the common command header.
    Reply(Vec<Vec<u8>>),
    /// A common-header-validated reply whose body starts with the supplied bytes.
    /// None accepts any completed device status; an explicit list pins statuses.
    Completed {
        body_prefix: Vec<u8>,
        statuses: Option<Vec<(u32, u32)>>,
        length: Option<u32>,
    },
    /// Only a failed reply transfer after the trigger was sent qualifies.
    NoReply,
}
#[derive(Clone, Debug)]
pub struct Invocation {
    pub preconditions: Vec<Precondition>,
    pub writes: Vec<GuardedWrite>,
    pub synchronization: Vec<SequenceCommand>,
    pub slot: CallbackSlot,
    pub trigger: research::Request,
    pub expected: ExpectedOutcome,
    pub limits: DebugMemoryLimits,
}

fn range(address: Address, length: usize) -> Result<MemoryRange> {
    let length = u32::try_from(length)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or(Error::InvalidLength)?;
    MemoryRange::new(address, length)
}
fn accepted(values: &[Vec<u8>], length: usize) -> Result<()> {
    if values.is_empty() || values.iter().any(|v| v.len() != length) {
        return Err(Error::InvalidLength);
    }
    Ok(())
}
fn overlaps(a: MemoryRange, b: MemoryRange) -> bool {
    u64::from(a.address().0) < u64::from(b.address().0) + u64::from(b.length().get())
        && u64::from(b.address().0) < u64::from(a.address().0) + u64::from(a.length().get())
}
impl Invocation {
    /// Validate the complete plan before the first USB request.
    pub fn validate(&self, max_reply_bytes: u32, packet_bytes: u32) -> Result<()> {
        let slot = range(self.slot.address, self.slot.value.len())?;
        if self.slot.required.len() != self.slot.value.len() {
            return Err(Error::InvalidLength);
        }
        for guard in &self.preconditions {
            accepted(&guard.accepted, guard.range.length().get() as usize)?;
        }
        for write in &self.writes {
            let at = range(write.address, write.bytes.len())?;
            accepted(&write.accepted, write.bytes.len())?;
            if overlaps(at, slot) {
                return Err(Error::OutsideDataWindow);
            }
        }
        check_limit(
            self.synchronization
                .len()
                .try_into()
                .map_err(|_| Error::InvalidLength)?,
            512,
        )?;
        for command in &self.synchronization {
            if let SequenceCommand::Write { address, bytes } = command
                && overlaps(range(*address, bytes.len())?, slot)
            {
                return Err(Error::OutsideDataWindow);
            }
            let checked = CheckedCommand::new(command.clone(), self.limits)?;
            check_limit(checked.reply_size, max_reply_bytes)?;
            check_limit(
                checked.reply_size,
                fujimod_usb_proxy::protocol::MAX_TRANSFER as u32,
            )?;
            check_limit(
                checked
                    .request
                    .len()
                    .try_into()
                    .map_err(|_| Error::InvalidLength)?,
                fujimod_usb_proxy::protocol::MAX_TRANSFER as u32,
            )?;
            if checked.request.len() >= 36 {
                check_limit(
                    checked
                        .reply_size
                        .saturating_sub(36)
                        .max(checked.request.len().saturating_sub(36) as u32),
                    packet_bytes,
                )?;
            }
        }
        check_limit(37, max_reply_bytes)?;
        check_limit(self.trigger.reply_capacity().get(), max_reply_bytes)?;
        check_limit(
            self.trigger.reply_capacity().get(),
            fujimod_usb_proxy::protocol::MAX_TRANSFER as u32,
        )?;
        check_limit(
            self.trigger
                .encode()
                .len()
                .try_into()
                .map_err(|_| Error::InvalidLength)?,
            fujimod_usb_proxy::protocol::MAX_TRANSFER as u32,
        )?;
        if let ExpectedOutcome::Completed {
            body_prefix,
            statuses,
            length,
        } = &self.expected
        {
            if body_prefix.len() > self.trigger.reply_capacity().get() as usize - 16
                || statuses.as_ref().is_some_and(|s| s.is_empty())
                || length.is_some_and(|n| {
                    n < 16
                        || n > self.trigger.reply_capacity().get()
                        || body_prefix.len() > (n - 16) as usize
                })
            {
                return Err(Error::InvalidLength);
            }
        }
        if let ExpectedOutcome::Reply(replies) = &self.expected {
            if replies.is_empty() {
                return Err(Error::InvalidLength);
            }
            for reply in replies {
                self.trigger.decode(reply.clone())?;
            }
        }
        Ok(())
    }
}
async fn guard(
    session: &mut Session,
    at: MemoryRange,
    values: &[Vec<u8>],
    limits: DebugMemoryLimits,
) -> Result<()> {
    let actual = read_range(session, at, limits).await?;
    if !values.contains(&actual) {
        return Err(ProtocolError::ExpectationMismatch.into());
    }
    Ok(())
}
async fn verified(
    session: &mut Session,
    address: Address,
    bytes: &[u8],
    limits: DebugMemoryLimits,
) -> Result<()> {
    write_range(session, address, bytes, limits).await?;
    guard(
        session,
        range(address, bytes.len())?,
        &[bytes.to_vec()],
        limits,
    )
    .await
}
fn missing_reply(error: &Error) -> bool {
    matches!(
        error,
        Error::Timeout(TransferStage::BulkIn | TransferStage::Control(RequestKind::ReadCommand))
            | Error::Transfer {
                stage: TransferStage::BulkIn | TransferStage::Control(RequestKind::ReadCommand),
                ..
            }
    )
}

/// Check every initial guard before writing, then recheck each write immediately
/// before applying it. Writes are ordered and read back. The slot is restored
/// after a returning callback, including an unexpected reply. Transport failure
/// fences the session: restoration is attempted and its failure is retained.
/// Guarded writes remain installed. Cancellation preserves the session fence;
/// it does not issue asynchronous cleanup.
pub async fn invoke(session: &mut Session, invocation: &Invocation) -> Result<Option<Vec<u8>>> {
    let p = invocation;
    p.validate(
        session.max_reply_bytes().get(),
        session.info().advertised_packet_bytes.get(),
    )?;
    if session.state() != SessionState::Ready {
        return Err(Error::CompletionUnknown);
    }
    let slot = range(p.slot.address, p.slot.required.len())?;
    for pre in &p.preconditions {
        guard(session, pre.range, &pre.accepted, p.limits).await?;
    }
    for write in &p.writes {
        guard(
            session,
            range(write.address, write.bytes.len())?,
            &write.accepted,
            p.limits,
        )
        .await?;
    }
    guard(
        session,
        slot,
        std::slice::from_ref(&p.slot.required),
        p.limits,
    )
    .await?;
    for write in &p.writes {
        guard(
            session,
            range(write.address, write.bytes.len())?,
            &write.accepted,
            p.limits,
        )
        .await?;
        verified(session, write.address, &write.bytes, p.limits).await?;
    }
    session
        .sequence(p.synchronization.clone(), p.limits)
        .await?;
    guard(
        session,
        slot,
        std::slice::from_ref(&p.slot.required),
        p.limits,
    )
    .await?;
    // Once publication starts, even a readback failure requires restoration.
    let result = async {
        verified(session, p.slot.address, &p.slot.value, p.limits).await?;
        let reply = session.callback_trigger(&p.trigger).await;
        match (&p.expected, reply) {
            (ExpectedOutcome::NoReply, Err(error)) if missing_reply(&error) => Ok(None),
            (ExpectedOutcome::Reply(accepted), Ok(raw)) if accepted.contains(&raw) => Ok(Some(raw)),
            (
                ExpectedOutcome::Completed {
                    body_prefix,
                    statuses,
                    length,
                },
                Ok(raw),
            ) => {
                let reply = p.trigger.decode(raw.clone())?;
                let status = match reply.status {
                    crate::ff80::commands::diagnostic::CommandStatus::Success => (0, 0),
                    crate::ff80::commands::diagnostic::CommandStatus::DeviceError {
                        primary,
                        secondary,
                    } => (primary, secondary),
                };
                if length.is_none_or(|n| raw.len() == n as usize)
                    && reply.body().starts_with(body_prefix)
                    && statuses.as_ref().is_none_or(|s| s.contains(&status))
                {
                    Ok(Some(raw))
                } else {
                    Err(ProtocolError::ExpectationMismatch.into())
                }
            }
            (_, Ok(_)) => Err(ProtocolError::ExpectationMismatch.into()),
            (_, Err(error)) => Err(error),
        }
    }
    .await;
    if matches!(result, Ok(None)) {
        return result;
    }
    let restoration = async {
        // Never overwrite a slot acquired by another writer during the callback.
        guard(
            session,
            slot,
            &[p.slot.required.clone(), p.slot.value.clone()],
            p.limits,
        )
        .await?;
        verified(session, p.slot.address, &p.slot.required, p.limits).await
    }
    .await;
    if restoration.is_err() {
        session.invalidate_completion();
    }
    match (result, restoration) {
        (Ok(reply), Ok(())) => Ok(reply),
        (Err(source), restoration) => Err(Error::CallbackInvocation {
            source: Box::new(source),
            restoration: restoration.err().map(Box::new),
        }),
        (Ok(_), Err(source)) => Err(Error::CallbackRestoration(Box::new(source))),
    }
}

#[cfg(test)]
mod tests;
