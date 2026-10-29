use super::*;
use crate::{
    ff80::{
        commands::memory::{ReadMemory, WriteMemory},
        framing::envelope,
    },
    testing::{Exchange as E, Script},
};
use futures_lite::future::{block_on, poll_once};
fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}
fn plan() -> Invocation {
    Invocation {
        preconditions: vec![Precondition {
            range: range(Address(0x100), 2).unwrap(),
            accepted: vec![vec![1, 2], vec![3, 4]],
        }],
        writes: vec![GuardedWrite {
            address: Address(0x200),
            accepted: vec![vec![5, 6]],
            bytes: vec![7, 8],
        }],
        synchronization: vec![SequenceCommand::Query {
            tag: crate::ff80::commands::diagnostic::DiagnosticTag::Research(7),
            statuses: vec![(0, 0)],
        }],
        slot: CallbackSlot {
            address: Address(0x300),
            required: vec![0; 8],
            value: vec![9; 8],
        },
        trigger: research::Request::new(research::CommandCode(0x450006), vec![0; 16], nz(32))
            .unwrap(),
        expected: ExpectedOutcome::Reply(vec![{
            let mut r = vec![0; 32];
            r[..4].copy_from_slice(&0x450006u32.to_le_bytes());
            r
        }]),
        limits: DebugMemoryLimits {
            read: nz(64),
            write: nz(64),
        },
    }
}
fn open() -> Vec<E> {
    vec![
        E::Control(envelope(RequestKind::Open, 8, 0)),
        E::Read {
            maximum: 8,
            data: vec![42, 0, 0, 0, 0, 0, 0, 64],
        },
    ]
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
fn read(steps: &mut Vec<E>, p: &Invocation, address: u32, bytes: Vec<u8>) {
    let request = ReadMemory::new(range(Address(address), bytes.len()).unwrap(), p.limits)
        .unwrap()
        .encode();
    let mut reply = request.clone();
    reply.extend(bytes);
    command(steps, request, reply);
}
fn write(steps: &mut Vec<E>, p: &Invocation, address: u32, bytes: Vec<u8>) {
    let request = WriteMemory::new(Address(address), bytes.clone(), p.limits)
        .unwrap()
        .encode();
    command(steps, request.clone(), request[..36].to_vec());
    read(steps, p, address, bytes);
}
fn prepared(p: &Invocation) -> Vec<E> {
    let mut s = open();
    read(&mut s, p, 0x100, vec![3, 4]);
    read(&mut s, p, 0x200, vec![5, 6]);
    read(&mut s, p, 0x300, vec![0; 8]);
    read(&mut s, p, 0x200, vec![5, 6]);
    write(&mut s, p, 0x200, vec![7, 8]);
    let query = CheckedCommand::new(p.synchronization[0].clone(), p.limits)
        .unwrap()
        .request;
    command(&mut s, query.clone(), query);
    read(&mut s, p, 0x300, vec![0; 8]);
    write(&mut s, p, 0x300, vec![9; 8]);
    s
}
#[test]
fn ordered_guarded_writes_sync_trigger_and_restoration() {
    let p = plan();
    let mut s = prepared(&p);
    let reply = p.trigger.encode();
    command(&mut s, p.trigger.encode(), reply.clone());
    read(&mut s, &p, 0x300, p.slot.value.clone());
    write(&mut s, &p, 0x300, vec![0; 8]);
    let script = Script::new(s);
    let mut session = block_on(script.begin_session(256, None)).unwrap();
    assert_eq!(block_on(invoke(&mut session, &p)).unwrap(), Some(reply));
    assert_eq!(script.remaining(), 0);
}
#[test]
fn any_reply_instead_of_no_reply_restores_and_fails() {
    for reply in [vec![0; 32], vec![], plan().trigger.encode()] {
        let mut p = plan();
        p.expected = ExpectedOutcome::NoReply;
        let mut s = prepared(&p);
        // The control asks for capacity even when the actual reply is shorter.
        command(&mut s, p.trigger.encode(), vec![0; 32]);
        *s.last_mut().unwrap() = E::Read {
            maximum: 32,
            data: reply,
        };
        read(&mut s, &p, 0x300, p.slot.value.clone());
        write(&mut s, &p, 0x300, vec![0; 8]);
        let script = Script::new(s);
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        assert!(matches!(
            block_on(invoke(&mut session, &p)),
            Err(Error::CallbackInvocation {
                restoration: None,
                ..
            })
        ));
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(script.remaining(), 0);
    }
}
#[test]
fn missing_reply_is_only_success_when_requested_and_fences_session() {
    for expect_none in [true, false] {
        let mut p = plan();
        if expect_none {
            p.expected = ExpectedOutcome::NoReply;
        }
        let mut s = prepared(&p);
        command(&mut s, p.trigger.encode(), vec![0; 32]);
        *s.last_mut().unwrap() = E::ReadFailure {
            maximum: 32,
            error: nusb::transfer::TransferError::Disconnected,
            partial: vec![],
        };
        let script = Script::new(s);
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        let result = block_on(invoke(&mut session, &p));
        if expect_none {
            assert_eq!(result.unwrap(), None);
        } else {
            assert!(matches!(
                result,
                Err(Error::CallbackInvocation {
                    restoration: Some(_),
                    ..
                })
            ));
        }
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        assert_eq!(script.remaining(), 0);
    }
}
#[test]
fn trigger_write_failure_never_counts_as_no_reply() {
    let mut p = plan();
    p.expected = ExpectedOutcome::NoReply;
    let mut s = prepared(&p);
    s.push(E::ControlFailure {
        bytes: envelope(RequestKind::WriteCommand, 32, 42),
        error: nusb::transfer::TransferError::Disconnected,
    });
    let script = Script::new(s);
    let mut session = block_on(script.begin_session(256, None)).unwrap();
    assert!(matches!(
        block_on(invoke(&mut session, &p)),
        Err(Error::CallbackInvocation {
            restoration: Some(_),
            ..
        })
    ));
    assert_eq!(script.remaining(), 0);
}
#[test]
fn precondition_failure_and_invalid_plans_never_write() {
    let mut p = plan();
    let mut s = open();
    read(&mut s, &p, 0x100, vec![99; 2]);
    let script = Script::new(s);
    let mut session = block_on(script.begin_session(256, None)).unwrap();
    assert!(block_on(invoke(&mut session, &p)).is_err());
    assert_eq!(script.remaining(), 0);
    p.writes[0].address = p.slot.address;
    assert!(block_on(invoke(&mut session, &p)).is_err());
    assert_eq!(script.remaining(), 0);
    p = plan();
    p.writes[0].accepted = vec![vec![]];
    assert!(p.validate(256, 64).is_err());
    p = plan();
    p.slot.required.clear();
    assert!(p.validate(256, 64).is_err());
    p = plan();
    p.synchronization.push(SequenceCommand::Query {
        tag: crate::ff80::commands::diagnostic::DiagnosticTag::Research(0),
        statuses: vec![],
    });
    assert!(p.validate(256, 64).is_err());
}
#[test]
fn cancellation_after_trigger_leaves_slot_and_fences_session() {
    let p = plan();
    let mut s = prepared(&p);
    command(&mut s, p.trigger.encode(), vec![0; 32]);
    *s.last_mut().unwrap() = E::PendingRead { maximum: 32 };
    let script = Script::new(s);
    let mut session = block_on(script.begin_session(256, None)).unwrap();
    let mut pending = Box::pin(invoke(&mut session, &p));
    assert!(block_on(poll_once(pending.as_mut())).is_none());
    drop(pending);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn unexpected_reply_when_reply_expected_restores_slot() {
    let p = plan();
    let mut s = prepared(&p);
    command(&mut s, p.trigger.encode(), vec![0; 32]);
    read(&mut s, &p, 0x300, p.slot.value.clone());
    write(&mut s, &p, 0x300, vec![0; 8]);
    let script = Script::new(s);
    let mut session = block_on(script.begin_session(256, None)).unwrap();
    assert!(matches!(
        block_on(invoke(&mut session, &p)),
        Err(Error::CallbackInvocation {
            restoration: None,
            ..
        })
    ));
    assert_eq!(script.remaining(), 0);
}

#[test]
fn failed_publication_readback_restores_without_trigger() {
    let p = plan();
    let mut s = prepared(&p);
    let E::Read { data, .. } = s.last_mut().unwrap() else {
        panic!()
    };
    *data.last_mut().unwrap() = 0;
    read(&mut s, &p, 0x300, p.slot.value.clone());
    write(&mut s, &p, 0x300, vec![0; 8]);
    let script = Script::new(s);
    let mut session = block_on(script.begin_session(256, None)).unwrap();
    assert!(matches!(
        block_on(invoke(&mut session, &p)),
        Err(Error::CallbackInvocation {
            restoration: None,
            ..
        })
    ));
    assert_eq!(script.remaining(), 0);
}

#[test]
fn foreign_callback_is_preserved_and_session_fenced() {
    let p = plan();
    let mut s = prepared(&p);
    command(&mut s, p.trigger.encode(), p.trigger.encode());
    read(&mut s, &p, 0x300, vec![0xff; 8]);
    let script = Script::new(s);
    let mut session = block_on(script.begin_session(256, None)).unwrap();
    assert!(matches!(
        block_on(invoke(&mut session, &p)),
        Err(Error::CallbackRestoration(_))
    ));
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn completed_reply_checks_header_prefix_and_optional_statuses() {
    for accept_status in [true, false] {
        let mut p = plan();
        p.expected = ExpectedOutcome::Completed {
            body_prefix: 5u32.to_le_bytes().to_vec(),
            length: Some(32),
            statuses: if accept_status {
                None
            } else {
                Some(vec![(0, 0)])
            },
        };
        let mut reply = p.trigger.encode();
        reply[4..8].copy_from_slice(&9u32.to_le_bytes());
        reply[16..20].copy_from_slice(&5u32.to_le_bytes());
        let mut s = prepared(&p);
        command(&mut s, p.trigger.encode(), reply);
        read(&mut s, &p, 0x300, p.slot.value.clone());
        write(&mut s, &p, 0x300, p.slot.required.clone());
        let script = Script::new(s);
        let mut session = block_on(script.begin_session(256, None)).unwrap();
        assert_eq!(block_on(invoke(&mut session, &p)).is_ok(), accept_status);
        assert_eq!(script.remaining(), 0);
    }
}
