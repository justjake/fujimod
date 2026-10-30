use super::*;
fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}
fn range(at: u32, n: u32) -> MemoryRange {
    MemoryRange::new(Address(at), nz(n)).unwrap()
}

#[test]
fn windows_check_aperture_and_require_explicit_wrapping() {
    assert!(
        DataWindow::new(
            Address(0x2000),
            range(0x1000, 0x2000),
            nz(512),
            OffsetMode::Checked
        )
        .is_err()
    );
    let window = DataWindow::new(
        Address(0x2000),
        range(0x1000, 0x2000),
        nz(512),
        OffsetMode::Wrapping,
    )
    .unwrap();
    assert_eq!(window.offset(Address(0x1000)).unwrap(), 0xfffff000);
    assert!(window.validate(range(0x1000, 0x2000)).is_ok());
    assert!(window.validate(range(0xfff, 1)).is_err());
    assert!(window.validate(range(0x2fff, 2)).is_err());
    let end = DataWindow::new(
        Address(u32::MAX),
        range(u32::MAX, 1),
        nz(1),
        OffsetMode::Checked,
    )
    .unwrap();
    assert!(end.validate(range(u32::MAX, 1)).is_ok());
}

#[test]
fn alignment_refuses_unaligned_starts_and_partial_granules() {
    assert!(DataAlignment::new(3, 64).is_err());
    assert!(DataAlignment::new(4, 48).is_err());
    assert!(DataAlignment::new(64, 4).is_err());
    let window = DataWindow::new(
        Address(0x1000),
        range(0x1000, 0x1000),
        nz(0x100),
        OffsetMode::Checked,
    )
    .unwrap()
    .with_alignment(DataAlignment::new(4, 64).unwrap());
    assert!(window.validate(range(0x1004, 0x40)).is_ok());
    for misaligned in [range(0x1002, 0x40), range(0x1000, 0x44)] {
        assert!(matches!(
            window.validate(misaligned),
            Err(Error::MisalignedDataRange {
                address: 4,
                granule: 64
            })
        ));
    }
}

mod replay {
    use super::*;
    use crate::{
        ff80::{
            SessionState,
            framing::{RequestKind, data_envelope, envelope},
        },
        testing::{Exchange as E, Script},
    };
    use futures_lite::future::block_on;

    fn opened(steps: Vec<E>, budget: u32) -> (Session, Script) {
        let mut script = vec![
            E::Control(envelope(RequestKind::Open, 8, 0)),
            E::Read {
                maximum: 8,
                data: vec![0x34, 0x12, 0, 0, 0, 0, 0, 1],
            },
        ];
        script.extend(steps);
        let script = Script::new(script);
        (
            block_on(script.begin_session(budget, None)).unwrap(),
            script,
        )
    }
    fn read(offset: u32, bytes: &[u8]) -> Vec<E> {
        vec![
            E::Control(data_envelope(
                RequestKind::ReadData,
                offset,
                bytes.len() as u32,
                0x1234,
            )),
            E::Read {
                maximum: bytes.len(),
                data: bytes.to_vec(),
            },
        ]
    }
    #[test]
    fn chunks_use_capacity_and_wrapping_mapping_without_implicit_setup() {
        let mut steps = read(0xfffff000, b"abc");
        steps.extend(read(0xfffff003, b"def"));
        steps.extend(read(0xfffff006, b"g"));
        let (mut session, script) = opened(steps, 64);
        let window = DataWindow::new(
            Address(0x2000),
            range(0x1000, 64),
            nz(3),
            OffsetMode::Wrapping,
        )
        .unwrap();
        let mut reader = DataReader::new(&mut session, window, range(0x1000, 7)).unwrap();
        assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), b"abc");
        assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), b"def");
        assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), b"g");
        assert!(block_on(reader.next_chunk()).unwrap().is_none());
        assert_eq!(reader.bytes_completed(), 7);
        assert_eq!(reader.into_inner().state(), SessionState::Ready);
        assert_eq!(script.remaining(), 0);
    }
    #[test]
    fn host_budget_bounds_bulk_commands_independently_of_debug_packet_size() {
        let mut steps = read(0, b"12345678");
        steps.extend(read(8, b"90"));
        let (mut session, script) = opened(steps, 8);
        let window = DataWindow::new(
            Address(0x1000),
            range(0x1000, 64),
            nz(32),
            OffsetMode::Checked,
        )
        .unwrap();
        let mut reader = DataReader::new(&mut session, window, range(0x1000, 10)).unwrap();
        assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), b"12345678");
        assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), b"90");
        assert_eq!(script.remaining(), 0);
    }
    #[test]
    fn failure_preserves_completed_chunks_and_current_partial_without_retry() {
        let mut steps = read(0, b"abcd");
        steps.extend([
            E::Control(data_envelope(RequestKind::ReadData, 4, 4, 0x1234)),
            E::Read {
                maximum: 4,
                data: b"e".to_vec(),
            },
            E::ReadFailure {
                maximum: 3,
                error: nusb::transfer::TransferError::Disconnected,
                partial: b"f".to_vec(),
            },
        ]);
        let (mut session, script) = opened(steps, 64);
        let window = DataWindow::new(
            Address(0x1000),
            range(0x1000, 64),
            nz(4),
            OffsetMode::Checked,
        )
        .unwrap();
        let mut reader = DataReader::new(&mut session, window, range(0x1000, 8)).unwrap();
        assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), b"abcd");
        assert!(
            matches!(block_on(reader.next_chunk()), Err(Error::PartialRead { data_len: 2, data, .. }) if data == b"ef")
        );
        assert_eq!(reader.bytes_completed(), 4);
        assert!(matches!(
            block_on(reader.next_chunk()),
            Err(Error::CompletionUnknown)
        ));
        assert_eq!(reader.into_inner().state(), SessionState::CompletionUnknown);
        assert_eq!(script.remaining(), 0);
    }
    #[test]
    fn early_stop_returns_ready_session_without_reading_ahead() {
        let (session, script) = opened(read(0, b"abcd"), 64);
        let window = DataWindow::new(
            Address(0x1000),
            range(0x1000, 64),
            nz(4),
            OffsetMode::Checked,
        )
        .unwrap();
        let mut reader = DataReader::new(session, window, range(0x1000, 8)).unwrap();
        assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), b"abcd");
        assert_eq!(reader.into_inner().state(), SessionState::Ready);
        assert_eq!(script.remaining(), 0);
    }
    fn aligned_window() -> DataWindow {
        DataWindow::new(
            Address(0x1000),
            range(0x1000, 0x1000),
            nz(0x100),
            OffsetMode::Checked,
        )
        .unwrap()
        .with_alignment(DataAlignment::new(4, 16).unwrap())
    }
    #[test]
    fn aligned_chunks_round_the_host_budget_down_to_whole_granules() {
        let mut steps = read(0, &[1; 32]);
        steps.extend(read(32, &[2; 32]));
        let (mut session, script) = opened(steps, 40);
        let mut reader =
            DataReader::new(&mut session, aligned_window(), range(0x1000, 64)).unwrap();
        assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), [1; 32]);
        assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), [2; 32]);
        assert!(block_on(reader.next_chunk()).unwrap().is_none());
        assert_eq!(script.remaining(), 0);
    }
    #[test]
    fn aligned_reads_refuse_a_budget_below_one_granule_without_a_request() {
        let (mut session, script) = opened(vec![], 8);
        let mut reader =
            DataReader::new(&mut session, aligned_window(), range(0x1000, 32)).unwrap();
        assert!(matches!(
            block_on(reader.next_chunk()),
            Err(Error::LengthLimit {
                requested: 16,
                limit: 8
            })
        ));
        assert_eq!(reader.into_inner().state(), SessionState::Ready);
        assert_eq!(script.remaining(), 0);
    }
}

#[test]
fn canceling_final_upload_acknowledgment_fences_the_session() {
    use crate::{
        ff80::{
            SessionState,
            commands::diagnostic::DataWriteMode,
            framing::{RequestKind, envelope},
        },
        testing::{Exchange as E, Script},
    };
    use futures_lite::future::{block_on, poll_once};
    let mut setup = vec![0; 32];
    setup[..4].copy_from_slice(&0x760001u32.to_le_bytes());
    setup[20..24].copy_from_slice(&3u32.to_le_bytes());
    let script = Script::new([
        E::Control(envelope(RequestKind::Open, 8, 0)),
        E::Read {
            maximum: 8,
            data: vec![0x34, 0x12, 0, 0, 0, 0, 0, 1],
        },
        E::Control(envelope(RequestKind::WriteCommand, 32, 0x1234)),
        E::Write(setup.clone()),
        E::Control(envelope(RequestKind::ReadCommand, 32, 0x1234)),
        E::Read {
            maximum: 32,
            data: setup,
        },
        E::Control(envelope(RequestKind::WriteData, 2, 0x1234)),
        E::Write(b"ab".to_vec()),
        E::Control(envelope(RequestKind::WriteData, 1, 0x1234)),
        E::Write(b"c".to_vec()),
        E::Control(envelope(RequestKind::EmptyResponse, 32, 0x1234)),
        E::PendingRead { maximum: 32 },
    ]);
    let mut session = block_on(script.begin_session(32, None)).unwrap();
    let mut future = Box::pin(upload(&mut session, b"abc", nz(2), DataWriteMode::Preserve));
    assert!(block_on(poll_once(future.as_mut())).is_none());
    drop(future);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert!(matches!(
        block_on(session.close()),
        Err(Error::CompletionUnknown)
    ));
    assert_eq!(script.remaining(), 0);
}

#[test]
fn captured_long_upload_acknowledgment_checks_echo_reserved_and_status() {
    use crate::{
        ff80::{
            SessionState,
            commands::diagnostic::DataWriteMode,
            framing::{RequestKind, envelope},
        },
        testing::{Exchange as E, Script},
    };
    use futures_lite::future::block_on;
    // Captured X70 E8 response: 01007600 followed by 28 zero bytes.
    for (word, value, success, ready) in [
        (0, 0x760001, true, true),
        (0, 0x550001, false, false),
        (3, 1, false, false),
        (1, 1, false, true),
        (2, 0xffff, false, true),
    ] {
        let mut setup = vec![0; 32];
        setup[..4].copy_from_slice(&0x760001u32.to_le_bytes());
        setup[20..24].copy_from_slice(&3u32.to_le_bytes());
        let mut reply = vec![0; 32];
        reply[..4].copy_from_slice(&0x760001u32.to_le_bytes());
        reply[word * 4..word * 4 + 4].copy_from_slice(&u32::to_le_bytes(value));
        let script = Script::new([
            E::Control(envelope(RequestKind::Open, 8, 0)),
            E::Read {
                maximum: 8,
                data: vec![0x34, 0x12, 0, 0, 0, 0, 0, 1],
            },
            E::Control(envelope(RequestKind::WriteCommand, 32, 0x1234)),
            E::Write(setup.clone()),
            E::Control(envelope(RequestKind::ReadCommand, 32, 0x1234)),
            E::Read {
                maximum: 32,
                data: setup,
            },
            E::Control(envelope(RequestKind::WriteData, 3, 0x1234)),
            E::Write(b"abc".to_vec()),
            E::Control(envelope(RequestKind::EmptyResponse, 32, 0x1234)),
            E::Read {
                maximum: 32,
                data: reply.clone(),
            },
        ]);
        let mut session = block_on(script.begin_session(32, None)).unwrap();
        let result = block_on(upload(&mut session, b"abc", nz(3), DataWriteMode::Preserve));
        assert_eq!(result.is_ok(), success);
        if success {
            let result = result.unwrap();
            assert_eq!(result.acknowledgment, reply);
            assert_eq!(result.bytes_written, 3);
        } else if ready {
            assert!(matches!(
                result,
                Err(Error::DeviceStatus {
                    command: 0x760001,
                    ..
                })
            ));
        } else {
            assert!(matches!(result, Err(Error::Protocol(_))));
        }
        assert_eq!(
            session.state(),
            if ready {
                SessionState::Ready
            } else {
                SessionState::CompletionUnknown
            }
        );
        assert_eq!(script.remaining(), 0);
    }
}
