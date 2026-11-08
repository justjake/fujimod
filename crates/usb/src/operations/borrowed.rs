//! Borrowed diagnostic callbacks: swap one callback word, query, restore once.
use crate::{
    Error, Result,
    ff80::{
        commands::{
            diagnostic::{CommandStatus, DiagnosticReply, DiagnosticTag},
            memory::{Address, DebugMemoryLimits, MemoryRange},
        },
        sequence::SequenceCommand,
    },
};
use std::num::NonZeroU32;

/// Commands before the first call: the slot and output guards.
const GUARDS: usize = 2;
/// Commands per call: output, slot, query, slot guard, captured output.
const PER_CALL: usize = 5;
/// Commands after the last call: restore both words, then verify them.
const RESTORE: usize = 4;
/// `Session::sequence` ceiling, counted on the expanded program.
const SEQUENCE_LIMIT: u32 = 512;

/// A checked borrow of one callback word and its output buffer. Callback
/// authorization and code/cache qualification remain the caller's policy.
///
/// The whole batch is one checked sequence: the USB owner guards the stock
/// slot and original output, runs every call, restores both words and verifies
/// their final values, all with a single host wait. Queries accept any
/// completed reply at the USB owner so restoration always follows a returning
/// callback; the host checks each call's status after the batch returns. A
/// rejected status therefore does not stop later calls in the same batch: batch
/// only calls that are safe to run whatever an earlier call returned. After
/// every query the USB owner checks that the slot still holds the borrowed
/// callback; another writer's value stops the batch and is never overwritten.
///
/// The program handed to the session is the logical batch with each write split
/// into ordered chunks no larger than the selected write limit, so a transport
/// that refuses larger writes runs the same batch.
#[derive(Clone)]
pub struct BorrowedQueries {
    callback: Address,
    stock: Address,
    output: MemoryRange,
    original: Vec<u8>,
    calls: Vec<(Address, DiagnosticTag, Vec<(u32, u32)>, Vec<u8>)>,
}
/// The logical program with every write split to the selected write limit.
struct Expanded {
    commands: Vec<SequenceCommand>,
    /// Logical command index -> index of its first command in `commands`.
    first: Vec<usize>,
}

/// Split each write into ordered chunks no larger than `write`, keeping every
/// other command in place. Chunk addresses advance with the chunks and are
/// checked like the direct write constructor: an empty write or a range leaving
/// the address space is refused. The expanded program must fit
/// `Session::sequence`'s ceiling, which is checked before any request is sent.
fn expand(commands: Vec<SequenceCommand>, write: NonZeroU32) -> Result<Expanded> {
    let limit = write.get() as usize;
    let mut expanded = Vec::with_capacity(commands.len());
    let mut first = Vec::with_capacity(commands.len());
    for command in commands {
        first.push(expanded.len());
        match command {
            SequenceCommand::Write { address, bytes } => {
                let whole =
                    NonZeroU32::new(u32::try_from(bytes.len()).map_err(|_| Error::InvalidLength)?)
                        .ok_or(Error::InvalidLength)?;
                MemoryRange::new(address, whole)?;
                let mut offset = 0usize;
                for part in bytes.chunks(limit) {
                    let at = u64::from(address.0) + offset as u64;
                    let at = Address(u32::try_from(at).map_err(|_| Error::AddressOverflow)?);
                    let length = NonZeroU32::new(
                        u32::try_from(part.len()).map_err(|_| Error::InvalidLength)?,
                    )
                    .ok_or(Error::InvalidLength)?;
                    MemoryRange::new(at, length)?;
                    expanded.push(SequenceCommand::Write {
                        address: at,
                        bytes: part.to_vec(),
                    });
                    offset += part.len();
                }
            }
            other => expanded.push(other),
        }
    }
    let requested = u32::try_from(expanded.len()).map_err(|_| Error::InvalidLength)?;
    if requested > SEQUENCE_LIMIT {
        return Err(Error::LengthLimit {
            requested,
            limit: SEQUENCE_LIMIT,
        });
    }
    Ok(Expanded {
        commands: expanded,
        first,
    })
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
        Ok(Self {
            callback,
            stock,
            output,
            original,
            calls: vec![],
        })
    }
    fn read(address: Address, length: u32, expected: Option<Vec<u8>>) -> SequenceCommand {
        SequenceCommand::Read {
            range: MemoryRange::new(
                address,
                NonZeroU32::new(length).expect("validated nonempty range"),
            )
            .expect("validated range"),
            expected,
        }
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
        let requested = GUARDS + PER_CALL * (self.calls.len() + 1) + RESTORE;
        if requested > SEQUENCE_LIMIT as usize {
            return Err(Error::LengthLimit {
                requested: requested as u32,
                limit: SEQUENCE_LIMIT,
            });
        }
        let first = GUARDS + PER_CALL * self.calls.len();
        self.calls.push((callback, tag, statuses, output));
        Ok((first + 2, first + 4))
    }
    /// Run the batch as one checked sequence. Writes are split to the selected
    /// write limit before the first request; the replies of each call are read
    /// at the expanded positions of its query and captured output. Return the
    /// raw query reply and captured output for each call. A call whose device
    /// status is not one it accepts fails after the batch has restored and
    /// verified both words.
    pub async fn invoke(
        &self,
        session: &mut crate::ff80::Session,
        limits: DebugMemoryLimits,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if self.calls.is_empty() {
            return Ok(Vec::new());
        }
        let program = expand(self.commands(), limits.write)?;
        let replies = session.sequence(program.commands, limits).await?;
        let mut results = Vec::with_capacity(self.calls.len());
        for (index, (_, _, statuses, _)) in self.calls.iter().enumerate() {
            let first = GUARDS + PER_CALL * index;
            let reply = replies[program.first[first + 2]].clone();
            let status = match DiagnosticReply::decode(reply.clone(), 0x450006)?.status {
                CommandStatus::Success => (0, 0),
                CommandStatus::DeviceError { primary, secondary } => (primary, secondary),
            };
            if !statuses.contains(&status) {
                return Err(crate::error::ProtocolError::ExpectationMismatch.into());
            }
            results.push((reply, replies[program.first[first + 4]].clone()));
        }
        Ok(results)
    }
    /// The logical batch: one write per word or output buffer, unsplit, as
    /// preview callers read it. `invoke` splits these writes to the transport's
    /// limit before running them.
    pub fn commands(&self) -> Vec<SequenceCommand> {
        if self.calls.is_empty() {
            return Vec::new();
        }
        let stock = self.stock.0.to_le_bytes().to_vec();
        let length = self.output.length().get();
        let mut commands = vec![
            Self::read(self.callback, 4, Some(stock.clone())),
            Self::read(self.output.address(), length, Some(self.original.clone())),
        ];
        for (callback, tag, _, output) in &self.calls {
            commands.extend([
                SequenceCommand::Write {
                    address: self.output.address(),
                    bytes: output.clone(),
                },
                SequenceCommand::Write {
                    address: self.callback,
                    bytes: callback.0.to_le_bytes().to_vec(),
                },
                SequenceCommand::Query {
                    tag: *tag,
                    statuses: None,
                },
                Self::read(self.callback, 4, Some(callback.0.to_le_bytes().to_vec())),
                Self::read(self.output.address(), length, None),
            ]);
        }
        commands.extend([
            SequenceCommand::Write {
                address: self.callback,
                bytes: stock.clone(),
            },
            SequenceCommand::Write {
                address: self.output.address(),
                bytes: self.original.clone(),
            },
            Self::read(self.callback, 4, Some(stock)),
            Self::read(self.output.address(), length, Some(self.original.clone())),
        ]);
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ff80::{
            SessionState,
            commands::memory::{ReadMemory, WriteMemory},
            framing::{RequestKind, envelope},
        },
        testing::{Exchange as E, Script},
    };
    use futures_lite::future::block_on;

    fn limits() -> DebugMemoryLimits {
        DebugMemoryLimits {
            read: NonZeroU32::new(64).unwrap(),
            write: NonZeroU32::new(64).unwrap(),
        }
    }
    fn command(steps: &mut Vec<E>, request: Vec<u8>, reply: Vec<u8>) {
        steps.extend([
            E::Control(envelope(
                RequestKind::WriteCommand,
                request.len() as u32,
                42,
            )),
            E::Write(request),
            E::Control(envelope(RequestKind::ReadCommand, reply.len() as u32, 42)),
            E::Read {
                maximum: reply.len(),
                data: reply,
            },
        ]);
    }
    fn read(steps: &mut Vec<E>, address: u32, bytes: &[u8]) {
        let range = MemoryRange::new(
            Address(address),
            NonZeroU32::new(bytes.len() as u32).unwrap(),
        )
        .unwrap();
        let request = ReadMemory::new(range, limits()).unwrap().encode();
        let mut reply = request.clone();
        reply.extend_from_slice(bytes);
        command(steps, request, reply);
    }
    fn write(steps: &mut Vec<E>, address: u32, bytes: &[u8]) {
        let request = WriteMemory::new(Address(address), bytes.to_vec(), limits())
            .unwrap()
            .encode();
        command(steps, request.clone(), request[..36].to_vec());
    }
    fn query_reply(tag: u32, status: (u32, u32)) -> Vec<u8> {
        let mut reply = crate::ff80::commands::diagnostic::query(DiagnosticTag::Research(tag));
        reply[4..8].copy_from_slice(&status.0.to_le_bytes());
        reply[8..12].copy_from_slice(&status.1.to_le_bytes());
        reply
    }
    /// The complete exchange for one call that replies with `status`.
    fn script(status: (u32, u32)) -> Script {
        let mut s = vec![
            E::Control(envelope(RequestKind::Open, 8, 0)),
            E::Read {
                maximum: 8,
                data: vec![42, 0, 0, 0, 0, 0, 0, 64],
            },
        ];
        read(&mut s, 0x1000, &0x9000u32.to_le_bytes());
        read(&mut s, 0x1100, b"original");
        write(&mut s, 0x1100, &[0; 8]);
        write(&mut s, 0x1000, &0x8000u32.to_le_bytes());
        let request = crate::ff80::commands::diagnostic::query(DiagnosticTag::Research(7));
        command(&mut s, request, query_reply(7, status));
        read(&mut s, 0x1000, &0x8000u32.to_le_bytes());
        read(&mut s, 0x1100, b"captured");
        write(&mut s, 0x1000, &0x9000u32.to_le_bytes());
        write(&mut s, 0x1100, b"original");
        read(&mut s, 0x1000, &0x9000u32.to_le_bytes());
        read(&mut s, 0x1100, b"original");
        Script::new(s)
    }
    fn plan() -> BorrowedQueries {
        let mut plan = BorrowedQueries::new(
            Address(0x1000),
            Address(0x9000),
            Address(0x1100),
            b"original".to_vec(),
        )
        .unwrap();
        plan.call(
            Address(0x8000),
            DiagnosticTag::Research(7),
            vec![(0, 0)],
            vec![0; 8],
        )
        .unwrap();
        plan
    }
    /// Limits with a chosen write size; reads stay generous so only write
    /// splitting is under test.
    fn write_limit(write: u32) -> DebugMemoryLimits {
        DebugMemoryLimits {
            read: NonZeroU32::new(64).unwrap(),
            write: NonZeroU32::new(write).unwrap(),
        }
    }
    fn open_steps() -> Vec<E> {
        vec![
            E::Control(envelope(RequestKind::Open, 8, 0)),
            E::Read {
                maximum: 8,
                data: vec![42, 0, 0, 0, 0, 0, 0, 64],
            },
        ]
    }
    /// The logical program chunked the way the transport limit requires.
    fn chunked(commands: &[SequenceCommand], write: u32) -> Vec<SequenceCommand> {
        let mut out = Vec::new();
        for command in commands {
            match command {
                SequenceCommand::Write { address, bytes } => {
                    for (index, part) in bytes.chunks(write as usize).enumerate() {
                        out.push(SequenceCommand::Write {
                            address: Address(address.0 + index as u32 * write),
                            bytes: part.to_vec(),
                        });
                    }
                }
                other => out.push(other.clone()),
            }
        }
        out
    }
    /// Expect one expanded command; `capture` is the reply of an output read
    /// the plan does not pin.
    fn expect_step(steps: &mut Vec<E>, step: &SequenceCommand, capture: Option<&[u8]>) {
        match step {
            SequenceCommand::Write { address, bytes } => write(steps, address.0, bytes),
            SequenceCommand::Read { range, expected } => {
                let bytes = match expected {
                    Some(bytes) => bytes,
                    None => capture.expect("captured output read"),
                };
                read(steps, range.address().0, bytes);
            }
            SequenceCommand::Query { tag, .. } => {
                let request = crate::ff80::commands::diagnostic::query(*tag);
                command(steps, request, query_reply(tag.value(), (0, 0)));
            }
        }
    }
    /// Strict replay of the expanded logical program, with one capture per
    /// unpinned output read.
    fn expanded_script(plan: &BorrowedQueries, write: u32, captured: &[&[u8]]) -> Script {
        let mut steps = open_steps();
        let mut captures = captured.iter();
        for step in chunked(&plan.commands(), write) {
            let capture = if matches!(step, SequenceCommand::Read { expected: None, .. }) {
                Some(*captures.next().expect("captured output"))
            } else {
                None
            };
            expect_step(&mut steps, &step, capture);
        }
        assert!(captures.next().is_none(), "unused captured output");
        Script::new(steps)
    }
    fn two_calls() -> BorrowedQueries {
        let mut plan = plan();
        plan.call(
            Address(0x8004),
            DiagnosticTag::Research(9),
            vec![(0, 0)],
            vec![1; 8],
        )
        .unwrap();
        plan
    }

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
            (4, 6)
        );
        assert_eq!(
            plan.call(
                Address(0x8004),
                DiagnosticTag::Research(9),
                vec![(0, 0)],
                vec![0; 8]
            )
            .unwrap(),
            (9, 11)
        );
        let commands = plan.commands();
        assert_eq!(commands.len(), 16);
        assert!(matches!(
            commands[4],
            SequenceCommand::Query { statuses: None, .. }
        ));
        let SequenceCommand::Write { address, bytes } = &commands[12] else {
            panic!()
        };
        assert_eq!(address.0, 0x1000);
        assert_eq!(bytes, &0x9000u32.to_le_bytes());
        let SequenceCommand::Read { expected, .. } = &commands[15] else {
            panic!()
        };
        assert_eq!(expected.as_deref(), Some(&b"original"[..]));
        assert!(
            BorrowedQueries::new(Address(0x1000), Address(0), Address(0x1002), vec![0; 8]).is_err()
        );
    }

    #[test]
    fn query_batches_refuse_overflow_without_mutating_the_plan() {
        let mut plan =
            BorrowedQueries::new(Address(0x1000), Address(0), Address(0x1100), vec![0; 8]).unwrap();
        for _ in 0..101 {
            plan.call(
                Address(0x8000),
                DiagnosticTag::Research(0),
                vec![(0, 0)],
                vec![0; 8],
            )
            .unwrap();
        }
        assert_eq!(plan.commands().len(), 511);
        assert!(
            plan.call(
                Address(0x8000),
                DiagnosticTag::Research(0),
                vec![(0, 0)],
                vec![0; 8]
            )
            .is_err()
        );
        assert_eq!(plan.commands().len(), 511);
    }

    #[test]
    fn one_sequence_guards_calls_restores_and_verifies() {
        let script = script((0, 0));
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        let before = session.round_trips();
        let results = block_on(plan().invoke(&mut session, limits())).unwrap();
        assert_eq!(session.round_trips() - before, 1);
        assert_eq!(
            results,
            vec![(query_reply(7, (0, 0)), b"captured".to_vec())]
        );
        assert_eq!(script.remaining(), 0);
    }

    #[test]
    fn unexpected_status_fails_after_restoration_and_keeps_the_session() {
        let script = script((1, 0xffff));
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        assert!(block_on(plan().invoke(&mut session, limits())).is_err());
        assert_eq!(script.remaining(), 0);
        assert_eq!(session.state(), SessionState::Ready);
    }

    #[test]
    fn write_limit_one_splits_writes_and_keeps_reply_positions() {
        let plan = plan();
        let steps = chunked(&plan.commands(), 1);
        let writes: Vec<&[u8]> = steps
            .iter()
            .filter_map(|step| match step {
                SequenceCommand::Write { bytes, .. } => Some(&bytes[..]),
                _ => None,
            })
            .collect();
        assert_eq!(writes.len(), 24, "two 8-byte and two 4-byte writes");
        assert!(writes.iter().all(|bytes| bytes.len() <= 1));
        let script = expanded_script(&plan, 1, &[b"captured"]);
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        let before = session.round_trips();
        let results = block_on(plan.invoke(&mut session, write_limit(1))).unwrap();
        assert_eq!(session.round_trips() - before, 1, "one wait for the batch");
        assert_eq!(
            results,
            vec![(query_reply(7, (0, 0)), b"captured".to_vec())]
        );
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(script.remaining(), 0);
    }

    #[test]
    fn two_expanded_calls_keep_their_own_reply_and_output() {
        let plan = two_calls();
        let script = expanded_script(&plan, 1, &[b"firstout", b"second!!"]);
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        let results = block_on(plan.invoke(&mut session, write_limit(1))).unwrap();
        assert_eq!(
            results,
            vec![
                (query_reply(7, (0, 0)), b"firstout".to_vec()),
                (query_reply(9, (0, 0)), b"second!!".to_vec()),
            ]
        );
        assert_eq!(script.remaining(), 0);
    }

    #[test]
    fn foreign_slot_value_stops_before_restoration() {
        let plan = plan();
        let borrowed = 0x8000u32.to_le_bytes();
        let mut steps = open_steps();
        let mut stopped = false;
        for step in chunked(&plan.commands(), 1) {
            if let SequenceCommand::Read { range, expected } = &step
                && range.address().0 == 0x1000
                && expected.as_deref() == Some(&borrowed[..])
            {
                read(&mut steps, 0x1000, &0x7000u32.to_le_bytes());
                stopped = true;
                break;
            }
            expect_step(&mut steps, &step, None);
        }
        assert!(stopped, "the slot guard after the query");
        let script = Script::new(steps);
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        let result = block_on(plan.invoke(&mut session, write_limit(1)));
        assert!(matches!(
            result,
            Err(Error::Protocol(
                crate::error::ProtocolError::ExpectationMismatch
            ))
        ));
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        assert_eq!(script.remaining(), 0, "no restoration was attempted");
    }

    #[test]
    fn lost_reply_during_a_split_write_fences_without_restoration() {
        let plan = plan();
        let mut steps = open_steps();
        let mut writes = 0usize;
        for step in chunked(&plan.commands(), 1) {
            if let SequenceCommand::Write { address, bytes } = &step {
                writes += 1;
                if writes == 3 {
                    write(&mut steps, address.0, bytes);
                    *steps.last_mut().unwrap() = E::ReadFailure {
                        maximum: 36,
                        error: nusb::transfer::TransferError::Disconnected,
                        partial: vec![],
                    };
                    break;
                }
            }
            expect_step(&mut steps, &step, None);
        }
        assert_eq!(writes, 3);
        let script = Script::new(steps);
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        let result = block_on(plan.invoke(&mut session, write_limit(1)));
        assert!(matches!(result, Err(Error::Transfer { .. })));
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        assert_eq!(script.remaining(), 0, "no restoration and no retry");
    }

    #[test]
    fn expanded_program_over_the_sequence_limit_refuses_before_traffic() {
        let mut plan = BorrowedQueries::new(
            Address(0x1000),
            Address(0x9000),
            Address(0x1100),
            vec![0; 8],
        )
        .unwrap();
        for _ in 0..35 {
            plan.call(
                Address(0x8000),
                DiagnosticTag::Research(0),
                vec![(0, 0)],
                vec![0; 8],
            )
            .unwrap();
        }
        assert_eq!(plan.commands().len(), 2 + 5 * 35 + 4, "logical calls fit");
        let expanded = chunked(&plan.commands(), 1).len();
        assert!(expanded > SEQUENCE_LIMIT as usize);
        let script = Script::new(open_steps());
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        match block_on(plan.invoke(&mut session, write_limit(1))) {
            Err(Error::LengthLimit { requested, limit }) => {
                assert_eq!(requested as usize, expanded);
                assert_eq!(limit, SEQUENCE_LIMIT);
            }
            other => panic!("expected a length limit, got {other:?}"),
        }
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(script.remaining(), 0, "no request reached the peer");
    }
}
