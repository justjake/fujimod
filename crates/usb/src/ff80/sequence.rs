//! Bounded, checked command sequences. Conditions run at the USB exporter so
//! dependent commands need no network round trip. They are not atomic against
//! device tasks; failed conditions fence the session and discard later work.
use super::commands::{
    diagnostic,
    memory::{Address, DebugMemoryLimits, MemoryRange, ReadMemory, WriteMemory},
};
use crate::{Error, Result, error::ProtocolError};

#[derive(Clone, Debug)]
pub enum SequenceCommand {
    Read {
        range: MemoryRange,
        expected: Option<Vec<u8>>,
    },
    Write {
        address: Address,
        bytes: Vec<u8>,
    },
    Query {
        tag: diagnostic::DiagnosticTag,
        statuses: Vec<(u32, u32)>,
    },
}

pub(crate) struct CheckedCommand {
    pub request: Vec<u8>,
    pub reply_size: u32,
    pub expected: Vec<Vec<u8>>,
    command: SequenceCommand,
}
impl CheckedCommand {
    pub fn new(command: SequenceCommand, limits: DebugMemoryLimits) -> Result<Self> {
        let (request, reply_size, expected) = match &command {
            SequenceCommand::Read { range, expected } => {
                let read = ReadMemory::new(*range, limits)?;
                let mut prefix = read.encode();
                if let Some(bytes) = expected {
                    if bytes.len() != range.length().get() as usize {
                        return Err(Error::InvalidLength);
                    }
                    prefix.extend_from_slice(bytes);
                }
                (
                    read.encode(),
                    range
                        .length()
                        .get()
                        .checked_add(36)
                        .ok_or(Error::InvalidLength)?,
                    vec![prefix],
                )
            }
            SequenceCommand::Write { address, bytes } => {
                let request = WriteMemory::new(*address, bytes.clone(), limits)?.encode();
                (request.clone(), 36, vec![request[..36].to_vec()])
            }
            SequenceCommand::Query { tag, statuses } => {
                if statuses.is_empty() || statuses.len() > 8 {
                    return Err(Error::InvalidLength);
                }
                let request = diagnostic::query(*tag);
                let prefixes = statuses
                    .iter()
                    .map(|(primary, secondary)| {
                        let mut prefix = request[..16].to_vec();
                        prefix[4..8].copy_from_slice(&primary.to_le_bytes());
                        prefix[8..12].copy_from_slice(&secondary.to_le_bytes());
                        prefix
                    })
                    .collect();
                (request, 32, prefixes)
            }
        };
        Ok(Self {
            request,
            reply_size,
            expected,
            command,
        })
    }
    pub fn decode(&self, reply: Vec<u8>) -> Result<Vec<u8>> {
        if reply.len() != self.reply_size as usize {
            return Err(ProtocolError::ReplyLength {
                expected: self.reply_size as usize,
                actual: reply.len(),
            }
            .into());
        }
        if !self.expected.iter().any(|prefix| reply.starts_with(prefix)) {
            return Err(ProtocolError::ExpectationMismatch.into());
        }
        match &self.command {
            SequenceCommand::Read { .. } => Ok(reply[36..].to_vec()),
            SequenceCommand::Write { .. } => Ok(Vec::new()),
            SequenceCommand::Query { .. } => {
                diagnostic::DiagnosticReply::decode(reply.clone(), 0x450006)?;
                Ok(reply)
            }
        }
    }
}
