//! Borrowed diagnostic callbacks: swap one callback word, query, restore once.
use crate::{
    Error, Result,
    ff80::{
        commands::{
            diagnostic::DiagnosticTag,
            memory::{Address, MemoryRange},
        },
        sequence::SequenceCommand,
    },
};
use std::num::NonZeroU32;

/// A checked borrow of one callback word and its output buffer. Callback
/// authorization and code/cache qualification remain the caller's policy.
#[derive(Clone)]
pub struct BorrowedQueries {
    callback: Address,
    stock: Address,
    output: MemoryRange,
    original: Vec<u8>,
    commands: Vec<SequenceCommand>,
    calls: Vec<(Address, DiagnosticTag, Vec<(u32, u32)>, Vec<u8>)>,
}
impl BorrowedQueries {
    pub fn new(
        callback: Address,
        stock: Address,
        output: Address,
        original: Vec<u8>,
    ) -> Result<Self> {
        let length = u32::try_from(original.len())
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or(Error::InvalidLength)?;
        let output = MemoryRange::new(output, length)?;
        MemoryRange::new(callback, NonZeroU32::new(4).expect("word"))?;
        if u64::from(callback.0) < u64::from(output.address().0) + u64::from(length.get())
            && u64::from(output.address().0) < u64::from(callback.0) + 4
        {
            return Err(Error::OutsideDataWindow);
        }
        let commands = vec![
            Self::read(callback, 4, Some(stock.0.to_le_bytes().to_vec()))?,
            Self::read(output.address(), length.get(), Some(original.clone()))?,
        ];
        Ok(Self {
            callback,
            stock,
            output,
            original,
            commands,
            calls: vec![],
        })
    }
    fn read(address: Address, length: u32, expected: Option<Vec<u8>>) -> Result<SequenceCommand> {
        Ok(SequenceCommand::Read {
            range: MemoryRange::new(
                address,
                NonZeroU32::new(length).ok_or(Error::InvalidLength)?,
            )?,
            expected,
        })
    }
    fn write(&mut self, address: Address, bytes: Vec<u8>) -> Result<()> {
        let length = bytes.len() as u32;
        self.commands.push(SequenceCommand::Write {
            address,
            bytes: bytes.clone(),
        });
        self.commands
            .push(Self::read(address, length, Some(bytes))?);
        Ok(())
    }
    /// Append one callback; return indices of its raw query reply and output.
    /// Restoration is appended once, by `commands()`, after the entire batch.
    pub fn call(
        &mut self,
        callback: Address,
        tag: DiagnosticTag,
        statuses: Vec<(u32, u32)>,
        output: Vec<u8>,
    ) -> Result<(usize, usize)> {
        if output.len() != self.original.len() || statuses.is_empty() || statuses.len() > 8 {
            return Err(Error::InvalidLength);
        }
        if self.commands.len() + 7 + 4 > 512 {
            return Err(Error::LengthLimit {
                requested: (self.commands.len() + 11) as u32,
                limit: 512,
            });
        }
        self.calls
            .push((callback, tag, statuses.clone(), output.clone()));
        self.write(self.output.address(), output)?;
        self.write(self.callback, callback.0.to_le_bytes().to_vec())?;
        let query = self.commands.len();
        self.commands.push(SequenceCommand::Query { tag, statuses });
        self.commands.push(Self::read(
            self.callback,
            4,
            Some(callback.0.to_le_bytes().to_vec()),
        )?);
        let result = self.commands.len();
        self.commands.push(SequenceCommand::Read {
            range: self.output,
            expected: None,
        });
        Ok((query, result))
    }
    /// Run every borrow through the guarded callback primitive under one session
    /// owner. Return the raw query reply and captured output for each call.
    pub async fn invoke(
        &self,
        session: &mut crate::ff80::Session,
        limits: crate::ff80::commands::memory::DebugMemoryLimits,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        use super::{
            callback as cb,
            memory::{read_range, write_range},
        };
        use crate::ff80::commands::research;
        let mut results = Vec::new();
        for (entry, tag, statuses, output) in &self.calls {
            let mut body = vec![0; 16];
            body[..4].copy_from_slice(&tag.value().to_le_bytes());
            let trigger = research::Request::new(
                research::CommandCode(0x450006),
                body,
                NonZeroU32::new(32).unwrap(),
            )?;
            let invocation = cb::Invocation {
                preconditions: vec![],
                writes: vec![cb::GuardedWrite {
                    address: self.output.address(),
                    accepted: vec![self.original.clone()],
                    bytes: output.clone(),
                }],
                synchronization: vec![],
                slot: cb::CallbackSlot {
                    address: self.callback,
                    required: self.stock.0.to_le_bytes().to_vec(),
                    value: entry.0.to_le_bytes().to_vec(),
                },
                trigger,
                expected: cb::ExpectedOutcome::Completed {
                    body_prefix: vec![],
                    statuses: Some(statuses.clone()),
                    length: Some(32),
                },
                limits,
            };
            let reply = cb::invoke(session, &invocation)
                .await?
                .ok_or(Error::InvalidLength)?;
            let captured = read_range(session, self.output, limits).await?;
            write_range(session, self.output.address(), &self.original, limits).await?;
            if read_range(session, self.output, limits).await? != self.original {
                return Err(crate::error::ProtocolError::ExpectationMismatch.into());
            }
            results.push((reply, captured));
        }
        Ok(results)
    }
    pub fn commands(&self) -> Vec<SequenceCommand> {
        if self.commands.len() == 2 {
            return Vec::new();
        }
        let mut plan = self.clone();
        plan.write(self.callback, self.stock.0.to_le_bytes().to_vec())
            .expect("validated callback range");
        plan.write(self.output.address(), self.original.clone())
            .expect("validated output range");
        plan.commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn borrowed_queries_validate_before_append_and_restore_once() {
        let mut plan = BorrowedQueries::new(
            Address(0x1000),
            Address(0x9000),
            Address(0x1100),
            b"original".to_vec(),
        )
        .unwrap();
        assert!(
            plan.call(
                Address(0x8000),
                DiagnosticTag::Research(7),
                vec![],
                vec![0; 8]
            )
            .is_err()
        );
        assert_eq!(plan.commands().len(), 0);
        assert_eq!(
            plan.call(
                Address(0x8000),
                DiagnosticTag::Research(7),
                vec![(0, 0), (1, 0xffff)],
                vec![0; 8]
            )
            .unwrap(),
            (6, 8)
        );
        assert_eq!(
            plan.call(
                Address(0x8004),
                DiagnosticTag::Research(9),
                vec![(0, 0)],
                vec![0; 8]
            )
            .unwrap(),
            (13, 15)
        );
        let commands = plan.commands();
        assert_eq!(commands.len(), 20);
        let SequenceCommand::Write { address, bytes } = &commands[16] else {
            panic!()
        };
        assert_eq!(address.0, 0x1000);
        assert_eq!(bytes, &0x9000u32.to_le_bytes());
        assert!(
            BorrowedQueries::new(Address(0x1000), Address(0), Address(0x1002), vec![0; 8]).is_err()
        );
    }

    #[test]
    fn query_batches_refuse_overflow_without_mutating_the_plan() {
        let mut plan =
            BorrowedQueries::new(Address(0x1000), Address(0), Address(0x1100), vec![0; 8]).unwrap();
        for _ in 0..72 {
            plan.call(
                Address(0x8000),
                DiagnosticTag::Research(0),
                vec![(0, 0)],
                vec![0; 8],
            )
            .unwrap();
        }
        assert_eq!(plan.commands().len(), 510);
        assert!(
            plan.call(
                Address(0x8000),
                DiagnosticTag::Research(0),
                vec![(0, 0)],
                vec![0; 8]
            )
            .is_err()
        );
        assert_eq!(plan.commands().len(), 510);
    }
}
