use super::*;
fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}

#[test]
fn event_and_payload_bounds_report_every_discard_without_truncating() {
    let trace = Trace::new(nz(2), nz(4));
    for i in 0..3 {
        trace.record(i, TransferStage::BulkIn, Some(0x81), 2, || {
            Action::Completed {
                transferred: 2,
                data: vec![i as u8; 2],
            }
        });
    }
    trace.record(3, TransferStage::BulkIn, Some(0x81), 5, || {
        panic!("oversize data must not be allocated")
    });
    let batch = trace.drain();
    assert_eq!((batch.dropped_events, batch.dropped_bytes), (2, 7));
    assert_eq!(
        batch.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(batch.events[0].action.data(), [1, 1]);
    assert!(trace.drain().events.is_empty());
    assert_eq!(trace.drain().dropped_events, 0);
}
#[test]
fn zero_payload_records_are_still_bounded() {
    let trace = Trace::new(nz(2), nz(4));
    for i in 0..5 {
        trace.record(i, TransferStage::BulkIn, None, 0, || Action::Canceled);
    }
    let batch = trace.drain();
    assert_eq!(batch.events.len(), 2);
    assert_eq!(batch.dropped_events, 3);
}

mod replay {
    use super::*;
    use crate::{
        ff80::SessionState,
        testing::{Exchange as E, Script},
    };
    use futures_lite::future::{block_on, poll_once};
    fn env(kind: u8, size: u32) -> [u8; 12] {
        let mut b = [0; 12];
        b[0] = kind;
        b[6..10].copy_from_slice(&size.to_be_bytes());
        b[10..12].copy_from_slice(&0x1234u16.to_be_bytes());
        b
    }
    fn script(extra: Vec<E>) -> Script {
        let mut open = env(0xc0, 8);
        open[10..12].fill(0);
        let mut v = vec![
            E::Control(open),
            E::Read {
                maximum: 8,
                data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
            },
        ];
        v.extend(extra);
        Script::new(v)
    }
    #[test]
    fn lifecycle_retains_raw_transfers_and_correlations_through_close() {
        let script = script(vec![
            E::Control(env(0xc4, 2)),
            E::Read {
                maximum: 2,
                data: vec![0x34, 0x12],
            },
        ]);
        let trace = Trace::new(nz(32), nz(1024));
        let session = block_on(script.begin_session(1024, Some(trace.clone()))).unwrap();
        block_on(session.close()).unwrap();
        let batch = trace.drain();
        assert_eq!(batch.events.len(), 8);
        assert_eq!(batch.dropped_events, 0);
        for pair in batch.events.as_chunks::<2>().0 {
            assert_eq!(pair[0].transfer, pair[1].transfer);
        }
        assert_eq!(
            batch.events[3].action.data(),
            [0x34, 0x12, 0, 0, 0, 0, 1, 0]
        );
        assert_eq!(batch.events[7].action.data(), [0x34, 0x12]);
        assert_eq!(script.remaining(), 0);
    }
    #[test]
    fn failing_read_records_exact_partial_data_and_does_not_change_fencing() {
        let script = script(vec![
            E::Control(env(0xc2, 2)),
            E::ReadFailure {
                maximum: 2,
                error: nusb::transfer::TransferError::Disconnected,
                partial: vec![0x34],
            },
        ]);
        let trace = Trace::new(nz(32), nz(1024));
        let mut session = block_on(script.begin_session(1024, Some(trace.clone()))).unwrap();
        assert!(block_on(session.ping()).is_err());
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        let batch = trace.drain();
        let event = batch.events.last().unwrap();
        assert!(
            matches!(&event.action,Action::Failed{transferred:Some(1),data,failure:Failure::Transfer(_)} if data==&[0x34])
        );
        assert_eq!(script.remaining(), 0);
    }
    #[test]
    fn cancellation_is_host_observation_and_never_a_device_cancel_request() {
        let script = script(vec![
            E::Control(env(0xc2, 2)),
            E::PendingRead { maximum: 2 },
        ]);
        let trace = Trace::new(nz(32), nz(1024));
        let mut session = block_on(script.begin_session(1024, Some(trace.clone()))).unwrap();
        {
            let mut future = std::pin::pin!(session.ping());
            assert!(block_on(poll_once(&mut future)).is_none());
        }
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        assert!(matches!(
            trace.drain().events.last().unwrap().action,
            Action::Canceled
        ));
        assert_eq!(script.remaining(), 0);
    }
    #[test]
    fn open_failure_keeps_trace_without_returning_a_session() {
        let mut open = env(0xc0, 8);
        open[10..12].fill(0);
        let script = Script::new([E::ControlFailure {
            bytes: open,
            error: nusb::transfer::TransferError::Disconnected,
        }]);
        let trace = Trace::new(nz(32), nz(1024));
        assert!(block_on(script.begin_session(1024, Some(trace.clone()))).is_err());
        assert!(matches!(
            trace.drain().events.last().unwrap().action,
            Action::Failed { .. }
        ));
    }
}

#[test]
fn protocol_and_usb_share_one_budget_and_cancellation_is_observed() {
    let trace = Trace::new(nz(2), nz(4));
    let span = trace.protocol(Layer::App80, 42, &[1, 2]);
    trace.record(9, TransferStage::BulkIn, None, 2, || Action::Completed {
        transferred: 2,
        data: vec![3, 4],
    });
    drop(span);
    let batch = trace.drain();
    assert_eq!(batch.dropped_events, 1);
    assert_eq!(batch.dropped_bytes, 2);
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.protocol_events.len(), 1);
    assert!(matches!(batch.protocol_events[0].action, Action::Canceled));
    assert!(batch.events[0].sequence < batch.protocol_events[0].sequence);
}
