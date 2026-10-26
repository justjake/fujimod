use super::*;
use fujimod_usb::{
    Error,
    ff80::{
        Session, SessionState,
        commands::memory::{Address, DebugMemoryLimits, MemoryRange},
        framing::RequestKind,
    },
    operations::data::{DataWindow, OffsetMode},
    testing::{Exchange as E, Script},
};
use futures_lite::future::{block_on, poll_once};
use std::num::NonZeroU32;
fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}
fn range(at: u32, size: u32) -> MemoryRange {
    MemoryRange::new(Address(at), nz(size)).unwrap()
}
fn limits() -> DebugMemoryLimits {
    DebugMemoryLimits {
        read: nz(256),
        write: nz(256),
    }
}
fn opened(steps: Vec<E>) -> (Session, Script) {
    let mut all = vec![
        E::Control(envelope(RequestKind::Open, 8, 0)),
        E::Read {
            maximum: 8,
            data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
        },
    ];
    all.extend(steps);
    let script = Script::new(all);
    (
        block_on(script.begin_session(262144, None)).unwrap(),
        script,
    )
}
// Independent protocol fixtures exercise the production encoder and guards.
fn data_envelope(kind: RequestKind, offset: u32, size: u32, session: u16) -> [u8; 12] {
    let mut raw = [0; 12];
    raw[0] = kind as u8;
    raw[2..6].copy_from_slice(&offset.to_be_bytes());
    raw[6..10].copy_from_slice(&size.to_be_bytes());
    raw[10..12].copy_from_slice(&session.to_be_bytes());
    raw
}
fn envelope(kind: RequestKind, size: u32, session: u16) -> [u8; 12] {
    data_envelope(kind, 0, size, session)
}
fn memory_request(function: u32, at: u32, size: u32) -> Vec<u8> {
    [0x200001, 0, 0, 0, function, 0, 0, at, size]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}
fn exchange(request: Vec<u8>, reply: Vec<u8>) -> Vec<E> {
    vec![
        E::Control(envelope(
            RequestKind::WriteCommand,
            request.len() as u32,
            0x1234,
        )),
        E::Write(request),
        E::Control(envelope(
            RequestKind::ReadCommand,
            reply.len() as u32,
            0x1234,
        )),
        E::Read {
            maximum: reply.len(),
            data: reply,
        },
    ]
}
fn read(at: u32, bytes: &[u8]) -> Vec<E> {
    let request = memory_request(2, at, bytes.len() as u32);
    let mut reply = request.clone();
    reply.extend_from_slice(bytes);
    exchange(request, reply)
}
fn write(at: u32, bytes: &[u8]) -> Vec<E> {
    let mut request = memory_request(3, at, bytes.len() as u32);
    request.extend_from_slice(bytes);
    let reply = request[..36].to_vec();
    exchange(request, reply)
}
fn destination() -> UploadDestination {
    UploadDestination::new(Address(0x1000), Address(0x8000), Address(0x1004), nz(32)).unwrap()
}
fn image() -> OwnedUpload {
    OwnedUpload::new(
        range(0x4000, 16),
        Address(0x4000),
        b"hello world".to_vec(),
        nz(8),
        nz(4),
    )
    .unwrap()
}
fn begin() -> Vec<E> {
    let mut steps = read(0x1000, &0x8000u32.to_le_bytes());
    steps.extend(read(0x1004, &32u32.to_le_bytes()));
    steps.extend(write(0x1000, &0x4000u32.to_le_bytes()));
    steps.extend(read(0x1000, &0x4000u32.to_le_bytes()));
    steps
}
fn transfer() -> Vec<E> {
    let mut setup = vec![0; 32];
    setup[..4].copy_from_slice(&0x760001u32.to_le_bytes());
    setup[20..24].copy_from_slice(&16u32.to_le_bytes());
    let mut steps = exchange(setup.clone(), setup.clone());
    let bytes = [b"hello world".as_slice(), &[0; 5]].concat();
    for part in bytes.chunks(8) {
        steps.extend([
            E::Control(envelope(RequestKind::WriteData, part.len() as u32, 0x1234)),
            E::Write(part.to_vec()),
        ]);
    }
    steps.extend([
        E::Control(envelope(RequestKind::EmptyResponse, 32, 0x1234)),
        E::Read {
            maximum: 32,
            data: setup,
        },
    ]);
    steps
}
fn restore() -> Vec<E> {
    let mut steps = read(0x1000, &0x4000u32.to_le_bytes());
    steps.extend(write(0x1000, &0x8000u32.to_le_bytes()));
    steps.extend(read(0x1000, &0x8000u32.to_le_bytes()));
    steps
}
#[test]
fn padding_cannot_reach_adjacent_allocations() {
    let image = OwnedUpload::new(
        range(0x400000, 131072),
        Address(0x400000),
        vec![0x5a; 92296],
        nz(65536),
        nz(32),
    )
    .unwrap();
    assert_eq!((image.image_bytes(), image.padded_bytes()), (92296, 131072));
    assert!(
        OwnedUpload::new(
            range(0x400000, 92296),
            Address(0x400000),
            vec![0; 92296],
            nz(65536),
            nz(32)
        )
        .is_err()
    );
    assert!(
        OwnedUpload::new(
            range(0x400000, 131072),
            Address(0x400004),
            vec![0; 16],
            nz(8),
            nz(32)
        )
        .is_err()
    );
    assert!(padded_size(u32::MAX, nz(65536)).is_err());
}
#[test]
fn owned_bulk_upload_restores_pointer_then_verifies_every_image_byte() {
    let mut steps = begin();
    steps.extend(transfer());
    steps.extend(restore());
    steps.extend([
        E::Control(data_envelope(RequestKind::ReadData, 0, 11, 0x1234)),
        E::Read {
            maximum: 11,
            data: b"hello world".to_vec(),
        },
    ]);
    let (mut session, script) = opened(steps);
    let reply = block_on(image().upload(&mut session, destination(), limits())).unwrap();
    assert_eq!(reply.bytes_written, 16);
    let window = DataWindow::new(
        Address(0x4000),
        range(0x4000, 16),
        nz(16),
        OffsetMode::Checked,
    )
    .unwrap();
    block_on(image().verify(&mut session, window)).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}
#[test]
fn changed_upload_pointer_discards_payload_and_restore() {
    let mut steps = read(0x1000, &0x9999u32.to_le_bytes());
    steps.extend(transfer());
    steps.extend(restore());
    let (mut session, script) = opened(steps);
    let count = script.remaining();
    assert!(block_on(image().upload(&mut session, destination(), limits())).is_err());
    assert_eq!(script.remaining(), count - 4);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
}

#[test]
fn pointer_loss_after_upload_never_repairs_the_foreign_owner() {
    let mut steps = begin();
    steps.extend(transfer());
    steps.extend(read(0x1000, &0x9999u32.to_le_bytes()));
    steps.extend(restore().into_iter().skip(4));
    let (mut session, script) = opened(steps);
    assert!(block_on(image().upload(&mut session, destination(), limits())).is_err());
    assert_eq!(script.remaining(), 8);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
}

#[test]
fn capacity_change_stops_before_pointer_publication() {
    let mut steps = read(0x1000, &0x8000u32.to_le_bytes());
    steps.extend(read(0x1004, &16u32.to_le_bytes()));
    steps.extend(transfer());
    let (mut session, script) = opened(steps);
    let remaining = script.remaining();
    assert!(block_on(image().upload(&mut session, destination(), limits())).is_err());
    assert_eq!(script.remaining(), remaining - 8);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
}
#[test]
fn short_capacity_is_rejected_before_borrowing() {
    let (mut session, script) = opened(Vec::new());
    let dest =
        UploadDestination::new(Address(0x1000), Address(0x8000), Address(0x1004), nz(8)).unwrap();
    assert!(matches!(
        block_on(image().upload(&mut session, dest, limits())),
        Err(Error::OutsideDataWindow)
    ));
    assert_eq!(script.remaining(), 0);
    assert_eq!(session.state(), SessionState::Ready);
}
#[test]
fn canceling_upload_never_restores_or_frees_an_uncertain_destination() {
    let mut steps = begin();
    let mut sending = transfer();
    sending.pop();
    sending.push(E::PendingRead { maximum: 32 });
    steps.extend(sending);
    steps.extend(restore());
    let (mut session, script) = opened(steps);
    let image = image();
    let mut future = Box::pin(image.upload(&mut session, destination(), limits()));
    assert!(block_on(poll_once(future.as_mut())).is_none());
    drop(future);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 12);
}
#[test]
fn verification_mismatch_fences_the_session() {
    let (mut session, script) = opened(vec![
        E::Control(data_envelope(RequestKind::ReadData, 0, 11, 0x1234)),
        E::Read {
            maximum: 11,
            data: b"HELLO world".to_vec(),
        },
    ]);
    let window = DataWindow::new(
        Address(0x4000),
        range(0x4000, 16),
        nz(16),
        OffsetMode::Checked,
    )
    .unwrap();
    assert!(block_on(image().verify(&mut session, window)).is_err());
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}
