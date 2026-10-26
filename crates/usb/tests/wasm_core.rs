#![cfg(target_arch = "wasm32")]
//! Executed as Wasm in a JS runtime, using synthetic endpoints and no USB API.
use fujimod_usb::{
    ff80::SessionState,
    testing::{Exchange as E, Script},
    trace::{Action, Trace},
};
use std::{
    num::NonZeroU32,
    time::{Duration, UNIX_EPOCH},
};
use wasm_bindgen_test::wasm_bindgen_test;

fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}
fn envelope(kind: u8, size: u32) -> [u8; 12] {
    let mut bytes = [0; 12];
    bytes[0] = kind;
    bytes[6..10].copy_from_slice(&size.to_be_bytes());
    bytes[10..12].copy_from_slice(&0x1234u16.to_be_bytes());
    bytes
}
fn script(extra: Vec<E>) -> Script {
    let mut open = envelope(0xc0, 8);
    open[10..12].fill(0);
    let mut steps = vec![
        E::Control(open),
        E::Read {
            maximum: 8,
            data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
        },
    ];
    steps.extend(extra);
    Script::new(steps)
}

#[wasm_bindgen_test]
async fn traced_session_executes_without_std_wall_clock_or_blocking() {
    let script = script(vec![
        E::Control(envelope(0xc2, 2)),
        E::Read {
            maximum: 2,
            data: vec![0xaa, 0xbb],
        },
        E::Control(envelope(0xc4, 2)),
        E::Read {
            maximum: 2,
            data: vec![0xcc, 0xdd],
        },
    ]);
    let trace = Trace::new(nz(64), nz(1024));
    let mut session = script
        .begin_session(1024, Some(trace.clone()))
        .await
        .unwrap();
    assert_eq!(session.ping().await.unwrap(), [0xaa, 0xbb]);
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(session.close().await.unwrap(), [0xcc, 0xdd]);
    assert_eq!(script.remaining(), 0);
    let batch = trace.drain();
    assert_eq!(batch.events.len(), 12);
    assert_eq!((batch.dropped_events, batch.dropped_bytes), (0, 0));
    for event in batch.events {
        assert!(
            event.timestamp.duration_since(UNIX_EPOCH).unwrap()
                > Duration::from_secs(1_500_000_000)
        );
    }
}

#[wasm_bindgen_test]
async fn dropped_future_fences_the_session_and_records_cancellation() {
    let script = script(vec![
        E::Control(envelope(0xc2, 2)),
        E::PendingRead { maximum: 2 },
    ]);
    let trace = Trace::new(nz(64), nz(1024));
    let mut session = script
        .begin_session(1024, Some(trace.clone()))
        .await
        .unwrap();
    assert!(
        futures_lite::future::poll_once(session.ping())
            .await
            .is_none()
    );
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert!(matches!(
        session.close().await,
        Err(fujimod_usb::Error::CompletionUnknown)
    ));
    assert_eq!(script.remaining(), 0);
    assert!(
        trace
            .drain()
            .events
            .iter()
            .any(|event| matches!(event.action, Action::Canceled))
    );
}
