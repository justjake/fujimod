use super::*;
use crate::bulk::{
    tests as api,
    BulkError, MappedPhase,
};
use fujimod_catalog::capture::{
    FlashCodeRange, FlashControl, FlashDevice, FlashRange, FlashReader, FlashStateCheck,
    FlashUpdateImage, FLASH_SCHEMA_URI,
};
use fujimod_usb::{ff80::SessionState, testing::Exchange as E};
use futures_lite::future::block_on;

const CONTROL: &[u8] = &[0x11, 0x22, 0x33, 0x44];

fn flash() -> FirmwareFlash {
    FirmwareFlash {
        schema_uri: FLASH_SCHEMA_URI.into(),
        schema: 1,
        id: "sample-1-flash".into(),
        firmware_id: "sample-1".into(),
        device: FlashDevice {
            kind: "serial-nor".into(),
            jedec_id: Some("abcdef".into()),
            part: Some("Test part".into()),
        },
        window: FlashRange {
            address: 0x1000,
            size: 0x100,
        },
        e1_window: Some(FlashRange {
            address: 0x1000,
            size: 0x100,
        }),
        update_image: FlashUpdateImage {
            dat_offset: 0,
            xor: 0,
        },
        reader: None,
        transport: None,
        code_ranges: Vec::new(),
        state_checks: Vec::new(),
        control: FlashControl {
            offset: 0,
            size: CONTROL.len() as u64,
            sha256: super::sha256(CONTROL),
        },
    }
}

fn mapped_payload(flash: &FirmwareFlash) -> crate::flash::Payload {
    crate::flash::Payload::MappedE1(crate::flash::MappedPayload {
        schema: 1,
        firmware_id: flash.firmware_id.clone(),
        firmware_sha256: "00".repeat(32),
    })
}

fn no_window() -> FirmwareFlash {
    let mut flash = flash();
    flash.e1_window = None;
    flash.reader = Some(FlashReader {
        symbol: "flash_read".into(),
        alignment: 1,
        nand: None,
    });
    flash.code_ranges = vec![FlashCodeRange {
        name: "flash_read".into(),
        address: 0x1000,
        size: 4,
    }];
    flash.state_checks = vec![FlashStateCheck {
        name: "driver".into(),
        address: 0x1000,
        size: 1,
        value: 0,
    }];
    flash.validate().unwrap();
    flash
}

fn source_state() -> Vec<u8> {
    api::words(&[0x1000, 32])
}

fn prepared() -> Vec<E> {
    let mut steps = api::info(4, "X-M5");
    steps.extend(api::info(8, "2.06"));
    steps.extend(api::info(4, "X-M5"));
    steps.extend(api::info(8, "2.06"));
    steps.extend(api::read(0x1000, b"stock code"));
    steps.extend(api::read(0x1200, &source_state()));
    steps.extend(api::read(0x1100, &api::words(&[0x1500])));
    steps.extend(api::read(0x150c, &api::words(&[0x2000])));
    steps.extend(api::diagnostic(0x450006, 0x80, 32));
    steps.extend(api::read(0x1200, &source_state()));
    steps.extend(api::read(0x1200, &source_state()));
    steps.extend(api::read(0x1000, b"stoc"));
    steps.extend(api::data(0, b"stoc"));
    steps
}

fn restore_tail() -> Vec<E> {
    let mut steps = api::read(0x1200, &source_state());
    steps.extend(api::write(0x1200, &source_state()));
    steps.extend(api::read(0x1200, &source_state()));
    steps
}

fn target() -> BulkRange {
    BulkRange {
        address: 0,
        size: 64,
    }
}

fn run(
    session: &mut Session,
    flash: &FirmwareFlash,
    target: BulkRange,
) -> (
    bulk::Result<Backup>,
    Vec<(u64, Vec<u8>)>,
    Vec<(u64, u64)>,
) {
    let bulk = api::query_camera();
    let mut chunks = Vec::new();
    let mut progress = Vec::new();
    let result = {
        let mut chunk = |offset: u64, bytes: Vec<u8>| -> Result<(), String> {
            chunks.push((offset, bytes));
            Ok(())
        };
        let mut report = |done: u64, total: u64| -> Result<(), String> {
            progress.push((done, total));
            Ok(())
        };
        block_on(super::read(
            session,
            &bulk,
            flash,
            api::limits(),
            target,
            &mut Callbacks {
                chunk: &mut chunk,
                progress: &mut report,
            },
        ))
    };
    (result, chunks, progress)
}

fn failure(error: &BulkError) -> &crate::bulk::MappedFailure {
    match error {
        BulkError::Mapped(failure) => failure,
        other => panic!("expected a mapped failure receipt, got {other}"),
    }
}

#[test]
fn mapped_public_read_range_bounds_and_missing_window_are_receipts() {
    let bulk = api::query_camera();
    let flash = flash();
    let payload = mapped_payload(&flash);
    for target in [
        BulkRange {
            address: 0,
            size: 0,
        },
        BulkRange {
            address: 0xf0,
            size: 0x20,
        },
        BulkRange {
            address: u64::MAX,
            size: 2,
        },
    ] {
        let (mut session, script) = api::opened(Vec::new());
        let error = block_on(crate::flash::read_range(
            &mut session,
            &bulk,
            &flash,
            &payload,
            &[],
            api::limits(),
            target.address,
            target.size as u32,
        ))
        .unwrap_err();
        let receipt = failure(&error);
        assert_eq!(receipt.phase, MappedPhase::Bounds);
        assert_eq!(receipt.address, target.address);
        assert_eq!(receipt.length, target.size);
        assert!(receipt.cleanup.is_none());
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(script.remaining(), 0);
    }

    let missing = no_window();
    let payload = mapped_payload(&missing);
    let (mut session, script) = api::opened(Vec::new());
    let error = block_on(crate::flash::read_range(
        &mut session,
        &bulk,
        &missing,
        &payload,
        &[],
        api::limits(),
        0,
        4,
    ))
    .unwrap_err();
    let receipt = failure(&error);
    assert_eq!(receipt.phase, MappedPhase::Window);
    assert_eq!(receipt.address, 0);
    assert_eq!(receipt.length, 4);
    assert!(receipt.source.to_string().contains("metadata differs"));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn mapped_public_backup_missing_window_is_a_mapped_receipt() {
    let bulk = api::query_camera();
    let flash = no_window();
    let payload = mapped_payload(&flash);
    let (mut session, script) = api::opened(Vec::new());
    let mut chunk = |_: u64, _: Vec<u8>| -> Result<(), String> { Ok(()) };
    let mut progress = |_: u64, _: u64| -> Result<(), String> { Ok(()) };
    let error = block_on(crate::flash::backup(
        &mut session,
        &bulk,
        &flash,
        &payload,
        &[],
        api::limits(),
        &mut Callbacks {
            chunk: &mut chunk,
            progress: &mut progress,
        },
    ))
    .unwrap_err();
    let receipt = failure(&error);
    assert_eq!(receipt.phase, MappedPhase::Window);
    assert_eq!(receipt.address, 0);
    assert_eq!(receipt.length, flash.window.size);
    assert!(receipt.source.to_string().contains("metadata differs"));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn mapped_control_read_and_hash_mismatch_record_control_scope() {
    let mut steps = prepared();
    steps.extend(api::data(0, &[0xde, 0xad, 0xbe, 0xef]));
    steps.extend(restore_tail());
    let (mut session, script) = api::opened(steps);
    let (result, chunks, _) = run(&mut session, &flash(), target());
    let error = result.unwrap_err();
    let receipt = failure(&error);
    assert_eq!(receipt.phase, MappedPhase::ControlHash);
    assert_eq!(receipt.address, 0x1000);
    assert_eq!(receipt.length, CONTROL.len() as u64);
    assert!(receipt.source.to_string().contains("control bytes differ"));
    assert!(chunks.is_empty());
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);

    let mut steps = prepared();
    steps.push(E::ReadFailure {
        maximum: CONTROL.len(),
        error: nusb::transfer::TransferError::Cancelled,
        partial: Vec::new(),
    });
    let (mut session, script) = api::opened(steps);
    let (result, chunks, progress) = run(&mut session, &flash(), target());
    let error = result.unwrap_err();
    let receipt = failure(&error);
    assert_eq!(receipt.phase, MappedPhase::ControlRead);
    assert_eq!(receipt.address, 0x1000);
    assert_eq!(receipt.length, CONTROL.len() as u64);
    assert!(matches!(&*receipt.source, BulkError::Usb(_)));
    let cleanup = receipt.cleanup.as_ref().expect("cleanup receipt");
    assert!(cleanup.to_string().contains("not restored"));
    assert!(chunks.is_empty());
    assert_eq!(progress, vec![(0, target().size)]);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn mapped_known_completion_restores_and_finishes_cleanly() {
    let first = vec![0xa0; 32];
    let second = vec![0xb0; 32];
    let mut steps = prepared();
    steps.extend(api::data(0, CONTROL));
    steps.extend(api::data(0, &first));
    steps.extend(api::data(32, &second));
    steps.extend(restore_tail());
    let (mut session, script) = api::opened(steps);
    let (result, chunks, progress) = run(&mut session, &flash(), target());
    let backup = result.unwrap();
    assert_eq!(backup.size, target().size);
    assert_eq!(
        backup.sha256,
        super::sha256(&[first.as_slice(), second.as_slice()].concat())
    );
    assert_eq!(chunks, vec![(0, first), (32, second)]);
    assert_eq!(progress, vec![(0, 64), (32, 64), (64, 64)]);
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn mapped_cancellations_fence_restore_and_record_failing_phase_address_length() {
    let mut steps = prepared();
    steps.push(E::ReadFailure {
        maximum: CONTROL.len(),
        error: nusb::transfer::TransferError::Cancelled,
        partial: Vec::new(),
    });
    let (mut session, script) = api::opened(steps);
    let (result, chunks, progress) = run(&mut session, &flash(), target());
    let error = result.unwrap_err();
    let receipt = failure(&error);
    assert_eq!(receipt.phase, MappedPhase::ControlRead);
    assert_eq!(receipt.address, 0x1000);
    assert_eq!(receipt.length, CONTROL.len() as u64);
    let cleanup = receipt.cleanup.as_ref().expect("cleanup receipt");
    assert!(cleanup.to_string().contains("not restored"));
    assert!(chunks.is_empty());
    assert_eq!(progress, vec![(0, target().size)]);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);

    let first = vec![0xa0; 32];
    let mut steps = prepared();
    steps.extend(api::data(0, CONTROL));
    steps.extend(api::data(0, &first));
    steps.push(E::ReadFailure {
        maximum: 32,
        error: nusb::transfer::TransferError::Cancelled,
        partial: Vec::new(),
    });
    let (mut session, script) = api::opened(steps);
    let (result, chunks, progress) = run(&mut session, &flash(), target());
    let error = result.unwrap_err();
    let receipt = failure(&error);
    assert_eq!(receipt.phase, MappedPhase::Piece);
    assert_eq!(receipt.address, 0x1020);
    assert_eq!(receipt.length, 32);
    let cleanup = receipt.cleanup.as_ref().expect("cleanup receipt");
    assert!(cleanup.to_string().contains("not restored"));
    assert_eq!(chunks, vec![(0, first)]);
    assert_eq!(progress, vec![(0, 64), (32, 64)]);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn mapped_public_restore_only_failure_reports_saved_publication_words() {
    let first = vec![0xa0; 32];
    let second = vec![0xb0; 32];
    let mut steps = prepared();
    steps.extend(api::data(0, CONTROL));
    steps.extend(api::data(0, &first));
    steps.extend(api::data(32, &second));
    steps.push(E::ReadFailure {
        maximum: 8,
        error: nusb::transfer::TransferError::Disconnected,
        partial: Vec::new(),
    });
    let (mut session, script) = api::opened(steps);
    let bulk = api::query_camera();
    let flash = flash();
    let payload = mapped_payload(&flash);
    let error = block_on(crate::flash::read_range(
        &mut session,
        &bulk,
        &flash,
        &payload,
        &[],
        api::limits(),
        0,
        64,
    ))
    .unwrap_err();
    let receipt = failure(&error);
    assert_eq!(receipt.phase, MappedPhase::Restore);
    assert_eq!(receipt.address, 0x1200);
    assert_eq!(receipt.length, 8);
    assert!(matches!(&*receipt.source, BulkError::Usb(_)));
    assert!(receipt.cleanup.is_none());
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn mapped_public_qualification_cancellation_keeps_typed_usb_cause_and_withheld_restore() {
    let mut steps = prepared();
    steps.truncate(steps.len() - 6);
    let mut failing = api::read(0x1000, b"stoc");
    failing.pop();
    failing.push(E::ReadFailure {
        maximum: 36 + 4,
        error: nusb::transfer::TransferError::Cancelled,
        partial: Vec::new(),
    });
    steps.extend(failing);
    let (mut session, script) = api::opened(steps);
    let bulk = api::query_camera();
    let flash = flash();
    let payload = mapped_payload(&flash);
    let error = block_on(crate::flash::read_range(
        &mut session,
        &bulk,
        &flash,
        &payload,
        &[],
        api::limits(),
        0,
        64,
    ))
    .unwrap_err();
    let receipt = failure(&error);
    assert_eq!(receipt.phase, MappedPhase::Prepare);
    assert_eq!(receipt.address, 0x1000);
    assert_eq!(receipt.length, 0x1000);
    assert!(
        matches!(&*receipt.source, BulkError::Usb(_)),
        "primary USB cause must stay typed during qualification: {}",
        receipt.source
    );
    let cleanup = receipt.cleanup.as_ref().expect("withheld restore receipt");
    assert!(cleanup.to_string().contains("not restored"));
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn mapped_public_transfer_cancellation_keeps_typed_usb_cause_and_withheld_restore() {
    let first = vec![0xa0; 32];
    let mut steps = prepared();
    steps.extend(api::data(0, CONTROL));
    steps.extend(api::data(0, &first));
    steps.push(E::ReadFailure {
        maximum: 32,
        error: nusb::transfer::TransferError::Cancelled,
        partial: Vec::new(),
    });
    let (mut session, script) = api::opened(steps);
    let bulk = api::query_camera();
    let flash = flash();
    let payload = mapped_payload(&flash);
    let error = block_on(crate::flash::read_range(
        &mut session,
        &bulk,
        &flash,
        &payload,
        &[],
        api::limits(),
        0,
        64,
    ))
    .unwrap_err();
    let receipt = failure(&error);
    assert_eq!(receipt.phase, MappedPhase::Piece);
    assert_eq!(receipt.address, 0x1020);
    assert_eq!(receipt.length, 32);
    assert!(
        matches!(&*receipt.source, BulkError::Usb(_)),
        "primary USB cause must stay typed during transfer: {}",
        receipt.source
    );
    let cleanup = receipt.cleanup.as_ref().expect("withheld restore receipt");
    assert!(cleanup.to_string().contains("not restored"));
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}
