use super::*;
use fujimod_usb::{
    ff80::SessionState,
    testing::{Exchange as E, Script},
};
use futures_lite::future::{block_on, poll_once};

fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}
fn range(at: u32, n: u32) -> MemoryRange {
    MemoryRange::new(Address(at), nz(n)).unwrap()
}
fn limits() -> DebugMemoryLimits {
    DebugMemoryLimits {
        read: nz(256),
        write: nz(1),
    }
}
fn window(at: u32, n: u32) -> WordWindow {
    WordWindow::new(range(at, n), range(0x01031664, 256)).unwrap()
}
fn envelope(kind: u8, n: u32) -> [u8; 12] {
    let mut b = [0; 12];
    b[0] = kind;
    b[6..10].copy_from_slice(&n.to_be_bytes());
    b[10..].copy_from_slice(&0x1234u16.to_be_bytes());
    b
}
fn command(request: Vec<u8>, reply: Vec<u8>) -> Vec<E> {
    vec![
        E::Control(envelope(0xd0, request.len() as u32)),
        E::Write(request),
        E::Control(envelope(0xd1, reply.len() as u32)),
        E::Read {
            maximum: reply.len(),
            data: reply,
        },
    ]
}
fn caps_reply() -> Vec<u8> {
    let mut b = vec![0; 32];
    b[..4].copy_from_slice(&0x580000u32.to_le_bytes());
    b[20..24].copy_from_slice(&48u32.to_le_bytes());
    b
}
fn prepare(reply: Vec<u8>, state: Option<Vec<u8>>) -> Vec<E> {
    let mut request = vec![0; 36];
    request[..4].copy_from_slice(&0x580000u32.to_le_bytes());
    request[20..24].copy_from_slice(&0x100u32.to_le_bytes());
    let mut steps = command(request, reply);
    if let Some(state) = state {
        let mut request = vec![0; 36];
        request[..4].copy_from_slice(&0x200001u32.to_le_bytes());
        request[16..20].copy_from_slice(&2u32.to_le_bytes());
        request[28..32].copy_from_slice(&SOURCE_STATE.to_le_bytes());
        request[32..].copy_from_slice(&8u32.to_le_bytes());
        let mut reply = request.clone();
        reply.extend(state);
        steps.extend(command(request, reply));
    }
    steps
}
fn state() -> Vec<u8> {
    [SOURCE, 48]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}
fn e1(at: u32, n: u32) -> E {
    let mut frame = envelope(0xe1, n);
    // Independently calculated source-relative offsets, not the adapter's
    // arithmetic repeated in the expected-frame builder.
    let offset: u32 = match at {
        SOURCE => 0,
        0x01031664 => 0xfedfa4d4,
        0x01031694 => 0xfedfa504,
        _ => panic!("test needs an independently calculated offset"),
    };
    frame[2..6].copy_from_slice(&offset.to_be_bytes());
    E::Control(frame)
}
fn opened(steps: Vec<E>) -> (Session, Script) {
    let mut start = vec![
        E::Control([0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0]),
        E::Read {
            maximum: 8,
            data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
        },
    ];
    start.extend(steps);
    let script = Script::new(start);
    (block_on(script.begin_session(128, None)).unwrap(), script)
}

#[test]
fn validates_whole_words_and_exact_caller_aperture_without_rounding() {
    for (at, n) in [(0x01031664, 4), (0x01031664, 48), (0x01031694, 16)] {
        assert_eq!(window(at, n).range(), range(at, n));
    }
    for (at, n) in [
        (0x01031665, 4),
        (0x01031664, 3),
        (0x01031664, 52),
        (0x01031660, 4),
        (0x01031760, 8),
        (0xff507c18, 4),
    ] {
        assert!(WordWindow::new(range(at, n), range(0x01031664, 256)).is_err());
    }
    assert!(WordWindow::new(range(0x20000000, 4), range(0x20000000, 64)).is_err());
}

#[test]
fn stock_capabilities_pins_source_then_reads_four_to_forty_eight_bytes() {
    for (at, n) in [
        (SOURCE, 48),
        (0x01031664, 4),
        (0x01031664, 48),
        (0x01031694, 16),
    ] {
        let raw: Vec<u8> = (0..n as u8).collect();
        let mut steps = prepare(caps_reply(), Some(state()));
        steps.push(e1(at, n));
        steps.push(E::Read {
            maximum: n as usize,
            data: raw.clone(),
        });
        let (mut s, script) = opened(steps);
        let admitted = WordWindow::new(range(at, n), range(at, n)).unwrap();
        let actual = block_on(read(&mut s, admitted, limits())).unwrap();
        assert_eq!(actual.address, at);
        assert_eq!(actual.raw, raw);
        assert_eq!(actual.words[0], 0x03020100);
        assert_eq!(actual.words.len(), n as usize / 4);
        assert_eq!(s.state(), SessionState::Ready);
        assert_eq!(script.remaining(), 0);
    }
}

#[test]
fn rejects_wrong_capacity_or_reserved_reply_before_source_read_or_e1() {
    for index in [20, 24, 28] {
        let mut reply = caps_reply();
        reply[index] ^= 1;
        let (mut s, script) = opened(prepare(reply.clone(), None));
        assert!(
            matches!(block_on(read(&mut s,window(0x01031664,4),limits())),
            Err(WordReadError::Observation{raw,..}) if raw==reply)
        );
        assert_eq!(s.state(), SessionState::CompletionUnknown);
        assert_eq!(script.remaining(), 0);
    }
}

#[test]
fn protocol_reserved_header_failure_stops_before_source_read_or_e1() {
    let mut reply = caps_reply();
    reply[12] = 1;
    let (mut s, script) = opened(prepare(reply, None));
    assert!(matches!(
        block_on(read(&mut s, window(0x01031664, 4), limits())),
        Err(WordReadError::Usb(_))
    ));
    assert_eq!(s.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn mismatched_published_pair_fences_and_preserves_raw_without_e1() {
    for index in [0, 4] {
        let mut bad = state();
        bad[index] ^= 1;
        let (mut s, script) = opened(prepare(caps_reply(), Some(bad.clone())));
        assert!(
            matches!(block_on(read(&mut s,window(0x01031664,4),limits())),
            Err(WordReadError::Observation{raw,..}) if raw==bad)
        );
        assert_eq!(s.state(), SessionState::CompletionUnknown);
        assert_eq!(script.remaining(), 0);
    }
}

#[test]
fn inadequate_cpu_limit_refuses_before_capabilities_command() {
    let (mut s, script) = opened(vec![]);
    let mut low = limits();
    low.read = nz(4);
    assert!(block_on(read(&mut s, window(0x01031664, 4), low)).is_err());
    assert_eq!(s.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn partial_bulk_reply_retains_bytes_and_fences_without_retry_or_cleanup() {
    let mut steps = prepare(caps_reply(), Some(state()));
    steps.push(e1(0x01031664, 4));
    steps.extend([
        E::Read {
            maximum: 4,
            data: vec![1, 2],
        },
        E::Read {
            maximum: 2,
            data: vec![],
        },
    ]);
    let (mut s, script) = opened(steps);
    assert!(
        matches!(block_on(read(&mut s,window(0x01031664,4),limits())),
        Err(WordReadError::Usb(fujimod_usb::Error::PartialRead{data,..})) if data==[1,2])
    );
    assert_eq!(s.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn cancellation_after_e1_keeps_existing_completion_fence() {
    let mut steps = prepare(caps_reply(), Some(state()));
    steps.push(e1(0x01031664, 4));
    steps.push(E::PendingRead { maximum: 4 });
    let (mut s, script) = opened(steps);
    block_on(async {
        let mut f = Box::pin(read(&mut s, window(0x01031664, 4), limits()));
        assert!(poll_once(f.as_mut()).await.is_none());
        drop(f);
    });
    assert_eq!(s.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}
