use super::*;
use crate::bulk::tests::{
    exchange_max, info, limits, opened, read as memory_read, write, zero_base_camera,
};
use crate::flash::{Callbacks, Selection};
use fujimod_catalog::capture::*;
use fujimod_usb::{
    ff80::{SessionState, commands::memory::DebugMemoryLimits},
    testing::Exchange as E,
};
use futures_lite::future::block_on;
use std::num::NonZeroU32;

const SLOT: u32 = 0x3000;
const ENTRY: u32 = 0x4080;
const REQUEST: u32 = 0x4000;
const RESULT: u32 = 0x4010;
const BUFFER: u32 = 0x5000;
const SOURCE: u32 = 0xf000_0000;
const CONTROL: u32 = 0x8000;
const ARENA_CAPACITY: u32 = 0x1000;

fn big_limits() -> Limits {
    Limits {
        memory: DebugMemoryLimits {
            read: NonZeroU32::new(4096).unwrap(),
            write: NonZeroU32::new(4096).unwrap(),
        },
        configuration: NonZeroU32::MIN,
    }
}

fn fixture() -> (FirmwareBulk, FirmwareFlash, Payload, Vec<u8>) {
    let bulk = zero_base_camera();
    let image = vec![0u8; 0x100];
    let control_sha = bulk::hex(&sha256_bytes(&[5u8; CONTROL as usize]));
    let flash = FirmwareFlash {
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
            address: u64::from(SOURCE),
            size: u64::from(CONTROL),
        },
        e1_window: None,
        update_image: FlashUpdateImage {
            dat_offset: 0,
            xor: 255,
        },
        reader: Some(FlashReader {
            symbol: "flash_read".into(),
            alignment: 32,
            nand: None,
            scratch: None,
        }),
        transport: Some("xp4-camera-code".into()),
        code_ranges: vec![FlashCodeRange {
            name: "flash_read".into(),
            address: 0x1000,
            size: 0x40,
        }],
        state_checks: vec![FlashStateCheck {
            name: "driver".into(),
            address: 0x1600,
            size: 4,
            value: 1,
        }],
        control: FlashControl {
            offset: 0,
            size: u64::from(CONTROL),
            sha256: control_sha.clone(),
        },
    };
    let payload = Payload {
        schema: 1,
        firmware_id: "sample-1".into(),
        firmware_sha256: "a".repeat(64),
        address: 0x4000,
        capacity: u64::from(ARENA_CAPACITY),
        sha256: bulk::hex(&sha256_bytes(&image)),
        entry: u64::from(ENTRY),
        request: u64::from(REQUEST),
        result: u64::from(RESULT),
        buffer: u64::from(BUFFER),
        buffer_capacity: CONTROL,
        stock_reader: 0x1000,
        stock_reader_symbol: "flash_read".into(),
        tag: 0x1234,
        mutable: vec![
            Range {
                address: 0x4000,
                size: 16,
            },
            Range {
                address: 0x4010,
                size: 24,
            },
        ],
        transport: Callback {
            address: u64::from(SLOT),
            original: 0,
            checks: Vec::new(),
        },
        checks: Vec::new(),
        control_sha256: control_sha,
        update_size: u64::from(CONTROL),
    };
    (bulk, flash, payload, image)
}

fn write_verified(at: u32, bytes: &[u8]) -> Vec<E> {
    let mut steps = write(at, bytes);
    steps.extend(memory_read(at, bytes));
    steps
}

fn memory_chunks(at: u32, bytes: &[u8]) -> Vec<E> {
    bytes
        .chunks(256)
        .enumerate()
        .flat_map(|(index, piece)| memory_read(at + (index * 256) as u32, piece))
        .collect()
}

fn request_words(source: u32, size: u32) -> Vec<u8> {
    [1u32, source, size, BUFFER]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

fn result_words(calls: u32, source: u32, size: u32) -> Vec<u8> {
    [1u32, 0, calls, source, size, MAGIC]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

/// The query exchange with `MAGIC` at reply byte `magic_at`.
fn query_exchange_with(magic_at: usize) -> Vec<E> {
    let mut request = vec![0u8; 32];
    request[..4].copy_from_slice(&QUERY_COMMAND.to_le_bytes());
    request[16..20].copy_from_slice(&0x1234u32.to_le_bytes());
    let mut reply = vec![0u8; 32];
    reply[..4].copy_from_slice(&QUERY_COMMAND.to_le_bytes());
    reply[magic_at..magic_at + 4].copy_from_slice(&MAGIC.to_le_bytes());
    exchange_max(request, reply, 32)
}

/// The stock handler returns the callback's `r2` word at byte 20 (`fields[1]`).
fn query_exchange() -> Vec<E> {
    query_exchange_with(20)
}

fn read_chunk_steps(calls: u32, data: &[u8]) -> Vec<E> {
    let mut steps = write_verified(REQUEST, &request_words(SOURCE, CONTROL));
    steps.extend(memory_read(SLOT, &ENTRY.to_le_bytes()));
    steps.extend(query_exchange());
    steps.extend(memory_read(RESULT, &result_words(calls, SOURCE, CONTROL)));
    steps.extend(memory_chunks(BUFFER, data));
    steps
}

fn arena_steps(image: &[u8]) -> Vec<E> {
    let mut steps = memory_chunks(REQUEST, &vec![0u8; ARENA_CAPACITY as usize]);
    steps.extend(write(REQUEST, image));
    steps.extend(memory_read(REQUEST, image));
    steps
}

fn cleanup_steps(image: &[u8]) -> Vec<E> {
    let mut steps = write_verified(SLOT, &0u32.to_le_bytes());
    steps.extend(memory_read(SLOT, &0u32.to_le_bytes()));
    steps.extend(memory_chunks(REQUEST, &vec![0u8; ARENA_CAPACITY as usize]));
    steps.extend(write_verified(REQUEST, &vec![0u8; image.len()]));
    steps.extend(memory_chunks(REQUEST, &vec![0u8; ARENA_CAPACITY as usize]));
    steps
}

fn steps(image: &[u8], control: &[u8], main: bool) -> Vec<E> {
    let mut steps = info(4, "X-M5");
    steps.extend(info(8, "2.06"));
    steps.extend(memory_read(0x1600, &1u32.to_le_bytes()));
    steps.extend(arena_steps(image));
    steps.extend(memory_read(SLOT, &0u32.to_le_bytes()));
    steps.extend(write_verified(SLOT, &ENTRY.to_le_bytes()));
    steps.extend(read_chunk_steps(1, control));
    if main {
        steps.extend(read_chunk_steps(2, control));
    }
    // Slot restoration reads the published entry before writing the original.
    steps.extend(memory_read(SLOT, &ENTRY.to_le_bytes()));
    steps.extend(cleanup_steps(image));
    steps
}

fn run(
    steps: Vec<E>,
    payload: &Payload,
    flash: &FirmwareFlash,
    bulk: &FirmwareBulk,
    image: &[u8],
) -> (bulk::Result<super::super::Backup>, SessionState, usize) {
    let (mut session, script) = opened(steps);
    let mut chunks = Vec::new();
    let result = block_on(super::read(
        &mut session,
        bulk,
        flash,
        payload,
        image,
        big_limits(),
        Selection::Whole,
        &mut Callbacks {
            chunk: &mut |offset, bytes| {
                chunks.push((offset, bytes));
                Ok(())
            },
            progress: &mut |_, _| Ok(()),
        },
    ));
    (result, session.state(), script.remaining())
}

#[test]
fn bounded_control_read_matches_hash_and_restores_every_borrowed_byte() {
    let (bulk, flash, payload, image) = fixture();
    payload.validate(&flash, &bulk, &image).unwrap();
    let control = vec![5u8; CONTROL as usize];
    let (result, state, remaining) = run(
        steps(&image, &control, true),
        &payload,
        &flash,
        &bulk,
        &image,
    );
    let backup = result.unwrap();
    assert_eq!(remaining, 0, "every scripted exchange is consumed");
    assert_eq!(state, SessionState::Ready);
    assert_eq!(backup.size, u64::from(CONTROL));
    assert_eq!(backup.sha256, bulk::hex(&sha256_bytes(&control)));
}

#[test]
fn control_hash_mismatch_stops_the_backup_and_still_restores() {
    let (bulk, flash, payload, image) = fixture();
    let bad = vec![4u8; CONTROL as usize];
    let (result, state, remaining) =
        run(steps(&image, &bad, false), &payload, &flash, &bulk, &image);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("Control chunk"), "{error}");
    assert_eq!(remaining, 0, "cleanup runs after the refused control read");
    assert_eq!(state, SessionState::Ready);
}

#[test]
fn unknown_completion_withholds_callback_and_arena_restoration() {
    let (bulk, flash, payload, image) = fixture();
    let mut steps = info(4, "X-M5");
    steps.extend(info(8, "2.06"));
    steps.extend(memory_read(0x1600, &1u32.to_le_bytes()));
    steps.extend(arena_steps(&image));
    steps.extend(memory_read(SLOT, &0u32.to_le_bytes()));
    steps.extend(write_verified(SLOT, &ENTRY.to_le_bytes()));
    steps.extend(write_verified(REQUEST, &request_words(SOURCE, CONTROL)));
    steps.extend(memory_read(SLOT, &ENTRY.to_le_bytes()));
    let mut query = query_exchange();
    query.pop();
    query.push(E::ReadFailure {
        maximum: 32,
        error: nusb::transfer::TransferError::Disconnected,
        partial: vec![],
    });
    steps.extend(query);
    let (result, state, remaining) = run(steps, &payload, &flash, &bulk, &image);
    assert!(result.is_err());
    assert_eq!(state, SessionState::CompletionUnknown);
    assert_eq!(
        remaining, 0,
        "no cleanup traffic is sent after unknown completion"
    );
}

#[test]
fn query_magic_outside_field_one_stops_the_read_and_still_restores() {
    let (bulk, flash, payload, image) = fixture();
    let mut steps = info(4, "X-M5");
    steps.extend(info(8, "2.06"));
    steps.extend(memory_read(0x1600, &1u32.to_le_bytes()));
    steps.extend(arena_steps(&image));
    steps.extend(memory_read(SLOT, &0u32.to_le_bytes()));
    steps.extend(write_verified(SLOT, &ENTRY.to_le_bytes()));
    steps.extend(write_verified(REQUEST, &request_words(SOURCE, CONTROL)));
    steps.extend(memory_read(SLOT, &ENTRY.to_le_bytes()));
    steps.extend(query_exchange_with(16));
    steps.extend(memory_read(SLOT, &ENTRY.to_le_bytes()));
    steps.extend(cleanup_steps(&image));
    let (result, state, remaining) = run(steps, &payload, &flash, &bulk, &image);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("query reply magic"), "{error}");
    assert_eq!(remaining, 0, "no result or buffer read, then cleanup");
    assert_eq!(state, SessionState::Ready);
}

#[test]
fn pre_io_validation_refusals_send_no_traffic() {
    let (bulk, mut flash, mut payload, image) = fixture();
    flash.transport = Some("xpp-cmd-callback".into());
    let (result, _, remaining) = run(Vec::new(), &payload, &flash, &bulk, &image);
    assert!(result.unwrap_err().to_string().contains("Unsupported XP4"));
    assert_eq!(remaining, 0);

    flash.transport = Some("xp4-camera-code".into());
    payload.buffer_capacity = 0x1_0000;
    let (result, _, remaining) = run(Vec::new(), &payload, &flash, &bulk, &image);
    assert!(result.unwrap_err().to_string().contains("chunk capacity"));
    assert_eq!(remaining, 0);
}

#[test]
fn fixture_validation_rejects_oversized_arena_and_mutable_entry() {
    let (bulk, flash, mut payload, image) = fixture();
    payload.capacity = 0x1_0000;
    assert!(payload.validate(&flash, &bulk, &image).is_err());

    let (bulk, flash, mut payload, image) = fixture();
    payload.mutable.push(Range {
        address: u64::from(ENTRY),
        size: 4,
    });
    assert!(payload.validate(&flash, &bulk, &image).is_err());

    let (bulk, flash, payload, image) = fixture();
    assert!(limits().memory.read.get() > 0);
    payload.validate(&flash, &bulk, &image).unwrap();
}

#[test]
fn runtime_validation_rejects_unaligned_entry() {
    let (bulk, flash, mut payload, image) = fixture();
    payload.entry = u64::from(ENTRY + 2);
    let error = payload
        .validate(&flash, &bulk, &image)
        .unwrap_err()
        .to_string();
    assert!(error.contains("reader ABI"), "{error}");
}

#[test]
fn runtime_validation_rejects_mismatched_control_hash() {
    let (bulk, flash, mut payload, image) = fixture();
    payload.control_sha256 = "b".repeat(64);
    let error = payload
        .validate(&flash, &bulk, &image)
        .unwrap_err()
        .to_string();
    assert!(error.contains("hash fields"), "{error}");
}
