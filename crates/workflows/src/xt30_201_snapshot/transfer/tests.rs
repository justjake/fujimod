use super::*;
use fujimod_usb::{
    ff80::{SessionState, commands::memory::Address},
    testing::{Exchange as E, Script},
};
use futures_lite::future::{block_on, poll_once};

fn buffer(start: u32, size: u32) -> MemoryRange {
    MemoryRange::new(Address(start), NonZeroU32::new(size).unwrap()).unwrap()
}

fn query() -> DiagnosticReply {
    let mut raw = [0; 32];
    raw[..4].copy_from_slice(&0x450006u32.to_le_bytes());
    raw[20..24].copy_from_slice(&64u32.to_le_bytes());
    DiagnosticReply {
        status: CommandStatus::Success,
        fields: [0, 64, 0, 0],
        raw,
    }
}

fn state(source: u32, capacity: u32) -> Vec<u8> {
    [source, capacity]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

fn published() -> PublishedSnapshot {
    PublishedSnapshot::new(buffer(0x101000, 64), &query(), &state(0x101000, 64)).unwrap()
}

fn opened(steps: Vec<E>) -> (Session, Script) {
    opened_with_limit(steps, 64)
}

fn opened_with_limit(steps: Vec<E>, limit: u32) -> (Session, Script) {
    let mut script = vec![
        E::Control([0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0]),
        E::Read {
            maximum: 8,
            data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
        },
    ];
    script.extend(steps);
    let script = Script::new(script);
    (block_on(script.begin_session(limit, None)).unwrap(), script)
}

fn e1() -> E {
    E::Control([0xe1, 0, 0, 0, 0, 0, 0, 0, 0, 64, 0x12, 0x34])
}

#[test]
fn rejects_unqualified_buffer_shapes_and_unmatched_source_pairs() {
    for (start, size) in [
        (0, 64),
        (0x101004, 64),
        (0x101000, 28),
        (0x20000000, 64),
        (0xff507c00, 64),
    ] {
        assert!(PublishedSnapshot::new(buffer(start, size), &query(), &state(start, 64)).is_err());
    }
    for metadata in [
        state(0x102000, 64),
        state(0x101000, 0),
        state(0x101000, 128),
        vec![],
        vec![0; 9],
    ] {
        assert!(PublishedSnapshot::new(buffer(0x101000, 64), &query(), &metadata).is_err());
    }
    // The highest numerically allowed allocation fits exactly below the bound.
    assert!(
        PublishedSnapshot::new(buffer(0x1fffffc0, 64), &query(), &state(0x1fffffc0, 64)).is_ok()
    );
}

#[test]
fn rejects_stock_zero_capacity_refusals_wrong_echo_and_reserved_fields() {
    let mut cases = vec![];
    let mut zero = query();
    zero.fields[1] = 0;
    zero.raw[20..24].fill(0);
    cases.push(zero);
    let mut refused = query();
    refused.status = CommandStatus::DeviceError {
        primary: 1,
        secondary: 0xffff,
    };
    cases.push(refused);
    for index in [0, 4, 8, 12, 16, 24, 28] {
        let mut bad = query();
        bad.raw[index] ^= 1;
        cases.push(bad);
    }
    for bad in cases {
        assert!(PublishedSnapshot::new(buffer(0x101000, 64), &bad, &state(0x101000, 64)).is_err());
    }
}

#[test]
fn retrieves_only_one_offset_zero_transfer_and_keeps_raw_words() {
    let mut raw = vec![0; 64];
    for index in 0..7 {
        raw[index * 4..index * 4 + 4]
            .copy_from_slice(&(0x12345670u32 + index as u32).to_le_bytes());
    }
    // USB may deliver a single E1 in more than one host read completion.
    let (mut session, script) = opened(vec![
        e1(),
        E::Read {
            maximum: 64,
            data: raw[..17].to_vec(),
        },
        E::Read {
            maximum: 47,
            data: raw[17..].to_vec(),
        },
    ]);
    let snapshot = block_on(published().read(&mut session)).unwrap();
    assert_eq!(snapshot.raw.as_slice(), raw);
    assert_eq!(
        snapshot.words,
        std::array::from_fn(|index| 0x12345670 + index as u32)
    );
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn invalid_padding_preserves_response_and_fences_without_cleanup_traffic() {
    let mut raw = vec![0; 64];
    raw[63] = 0xa5;
    let (mut session, script) = opened(vec![
        e1(),
        E::Read {
            maximum: 64,
            data: raw.clone(),
        },
    ]);
    let error = block_on(published().read(&mut session)).unwrap_err();
    assert!(matches!(error, SnapshotTransferError::Decode {
        source: SnapshotDecodeError::NonzeroPadding(63), raw: retained,
    } if retained == raw));
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert!(matches!(
        block_on(session.ping()),
        Err(fujimod_usb::Error::CompletionUnknown)
    ));
    assert_eq!(script.remaining(), 0);
}

#[test]
fn incomplete_transfer_retains_partial_bytes_and_never_reissues_e1() {
    let (mut session, script) = opened(vec![
        e1(),
        E::Read {
            maximum: 64,
            data: vec![1, 2, 3],
        },
        E::Read {
            maximum: 61,
            data: vec![],
        },
    ]);
    let error = block_on(published().read(&mut session)).unwrap_err();
    assert!(
        matches!(error, SnapshotTransferError::Usb(fujimod_usb::Error::PartialRead { data, .. }) if data == [1,2,3])
    );
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn cancellation_fences_session_without_target_cleanup() {
    let (mut session, script) = opened(vec![e1(), E::PendingRead { maximum: 64 }]);
    block_on(async {
        let mut future = Box::pin(published().read(&mut session));
        assert!(poll_once(future.as_mut()).await.is_none());
        drop(future);
    });
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn insufficient_receive_budget_refuses_before_e1() {
    let (mut session, script) = opened_with_limit(vec![], 32);
    assert!(matches!(
        block_on(published().read(&mut session)),
        Err(SnapshotTransferError::Usb(_))
    ));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn failed_e1_control_fences_without_retry_or_close() {
    let E::Control(bytes) = e1() else {
        unreachable!()
    };
    let (mut session, script) = opened(vec![E::ControlFailure {
        bytes,
        error: nusb::transfer::TransferError::Disconnected,
    }]);
    assert!(matches!(
        block_on(published().read(&mut session)),
        Err(SnapshotTransferError::Usb(_))
    ));
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}
