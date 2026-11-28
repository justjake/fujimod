use super::*;
use crate::bulk::tests::{
    exchange_max, info, opened, read as memory_read, write, zero_base_camera,
};
use crate::flash::{Callbacks, Selection};
use fujimod_catalog::capture::*;
use fujimod_usb::{
    ff80::{SessionState, commands::memory::DebugMemoryLimits},
    testing::Exchange as E,
};
use futures_lite::future::block_on;
use std::num::NonZeroU32;

const READER: u32 = 0x1000;
const READER_CODE: [u8; 0x40] = [7; 0x40];
const DRIVER: u32 = 0x1600;
const REGISTERED: u32 = 0x1300;
const TABLE: u32 = 0x1100;
const TABLE_BASE: u32 = 0x2000;
const SLOT: u32 = TABLE_BASE + 12;
const STOCK_CALLBACK: u32 = 0x0020_6c00;
const CALLBACK_CODE: [u8; 0x20] = [9; 0x20];
const SCRATCH: u32 = 0x30000;
const SCRATCH_SIZE: u32 = 0x1000;
const BASE: u32 = 0x10000;
const WINDOW: u32 = 0xf000_0000;
const WINDOW_SIZE: u32 = 0x1200;
const TAIL: u32 = WINDOW_SIZE - TAIL_BYTES as u32;
const CONTROL: u32 = 0x100;
const QUERY: u32 = 0x450006;

fn limits() -> Limits {
    Limits {
        memory: DebugMemoryLimits {
            read: NonZeroU32::new(4096).unwrap(),
            write: NonZeroU32::new(4096).unwrap(),
        },
        configuration: NonZeroU32::MIN,
    }
}

fn hex(bytes: &[u8]) -> String {
    bulk::hex(&sha256_bytes(bytes))
}

/// Window bytes, distinct per offset.
fn flash_bytes() -> Vec<u8> {
    (0..WINDOW_SIZE).map(|at| (at % 251) as u8).collect()
}

fn fixture() -> (FirmwareBulk, FirmwareFlash, Payload) {
    let mut bulk = zero_base_camera();
    bulk.source = BulkSource::Pattern {
        registered_buffer: u64::from(REGISTERED),
        source_state: 0x1400,
        capacity: 0x10000,
    };
    let control = hex(&flash_bytes()[..CONTROL as usize]);
    let flash = FirmwareFlash {
        schema_uri: FLASH_SCHEMA_URI.into(),
        schema: 1,
        id: "sample-1-flash".into(),
        firmware_id: "sample-1".into(),
        device: FlashDevice {
            kind: "serial-nor".into(),
            jedec_id: None,
            part: None,
        },
        window: FlashRange {
            address: u64::from(WINDOW),
            size: u64::from(WINDOW_SIZE),
        },
        e1_window: None,
        update_image: FlashUpdateImage {
            dat_offset: 0,
            xor: 255,
        },
        reader: Some(FlashReader {
            symbol: "stock flash reader".into(),
            alignment: 128,
            nand: None,
            scratch: Some(FlashReaderScratch {
                state: FlashRange {
                    address: u64::from(DRIVER),
                    size: 0x60,
                },
                pointer: u64::from(DRIVER + 0x38),
                size: u64::from(DRIVER + 0x14),
            }),
        }),
        transport: Some(TRANSPORT.into()),
        code_ranges: vec![FlashCodeRange {
            name: "stock flash reader".into(),
            address: u64::from(READER),
            size: READER_CODE.len() as u64,
        }],
        state_checks: vec![FlashStateCheck {
            name: "flash driver initialized".into(),
            address: u64::from(DRIVER),
            size: 4,
            value: 1,
        }],
        control: FlashControl {
            offset: 0,
            size: u64::from(CONTROL),
            sha256: control.clone(),
        },
    };
    let payload = Payload {
        schema: 1,
        firmware_id: "sample-1".into(),
        firmware_sha256: "a".repeat(64),
        stock_reader: u64::from(READER),
        stock_reader_symbol: "stock flash reader".into(),
        callback: Callback {
            table: u64::from(TABLE),
            offset: 12,
            original: STOCK_CALLBACK,
            checks: vec![CodeCheck {
                address: u64::from(STOCK_CALLBACK),
                size: CALLBACK_CODE.len() as u64,
                sha256: hex(&CALLBACK_CODE),
            }],
        },
        checks: vec![CodeCheck {
            address: u64::from(READER),
            size: READER_CODE.len() as u64,
            sha256: hex(&READER_CODE),
        }],
        control_sha256: control,
        update_size: u64::from(TAIL),
    };
    (bulk, flash, payload)
}

fn reads(at: u32, bytes: &[u8]) -> Vec<E> {
    bytes
        .chunks(256)
        .enumerate()
        .flat_map(|(index, piece)| memory_read(at + (index * 256) as u32, piece))
        .collect()
}

fn writes(at: u32, bytes: &[u8]) -> Vec<E> {
    bytes
        .chunks(256)
        .enumerate()
        .flat_map(|(index, piece)| write(at + (index * 256) as u32, piece))
        .collect()
}

fn write_verified(at: u32, bytes: &[u8]) -> Vec<E> {
    let mut steps = writes(at, bytes);
    steps.extend(reads(at, bytes));
    steps
}

fn word(value: u32) -> [u8; 4] {
    value.to_le_bytes()
}

fn request(offset: u32, size: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [BASE + 128, WINDOW + offset, size] {
        bytes.extend(value.to_le_bytes());
    }
    bytes.extend([0, 1, 0, 0]);
    bytes
}

fn query(status: u32) -> Vec<E> {
    let mut request = vec![0u8; 32];
    request[..4].copy_from_slice(&QUERY.to_le_bytes());
    request[16..20].copy_from_slice(&BASE.to_le_bytes());
    let mut reply = vec![0u8; 32];
    reply[..4].copy_from_slice(&QUERY.to_le_bytes());
    reply[4..8].copy_from_slice(&status.to_le_bytes());
    exchange_max(request, reply, 32)
}

fn descriptor() -> Vec<u8> {
    [BASE, 0x10000]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

/// One stock read: request, buffer check, publish, query, restore, data.
fn stock_read(offset: u32, data: &[u8]) -> Vec<E> {
    let mut steps = write_verified(BASE, &request(offset, data.len() as u32));
    steps.extend(memory_read(REGISTERED, &descriptor()));
    steps.extend(memory_read(SLOT, &word(STOCK_CALLBACK)));
    steps.extend(write_verified(SLOT, &word(READER)));
    steps.extend(query(0));
    steps.extend(memory_read(SLOT, &word(READER)));
    steps.extend(write_verified(SLOT, &word(STOCK_CALLBACK)));
    steps.extend(reads(BASE + 128, data));
    steps
}

fn saved() -> Vec<u8> {
    vec![0x5a; BORROWED_BYTES as usize]
}

/// A RAM configuration read, or a write when `writing`, of the debug gate.
fn gate(value: u8, writing: bool) -> Vec<E> {
    use fujimod_usb::ff80::commands::configuration::{
        ConfigurationLocation, ReadConfiguration, WriteConfiguration,
    };
    let location = ConfigurationLocation {
        view: ConfigurationView::Ram,
        page: 0,
        offset: DEBUG_GATE,
    };
    let request = if writing {
        WriteConfiguration::new(location, vec![value], limits().configuration)
            .unwrap()
            .encode()
    } else {
        ReadConfiguration::new(location, NonZeroU32::MIN, limits().configuration)
            .unwrap()
            .encode()
    };
    let mut reply = request[..36].to_vec();
    if !writing {
        reply.push(value);
    }
    let maximum = reply.len();
    exchange_max(request, reply, maximum)
}

/// Identity, an open debug gate, code, driver state and the reader scratch.
fn checked(scratch: u32, scratch_size: u32) -> Vec<E> {
    checked_after(gate(1, false), scratch, scratch_size)
}

/// Identity, the debug gate exchanges, code, driver state and the scratch.
fn checked_after(gate: Vec<E>, scratch: u32, scratch_size: u32) -> Vec<E> {
    let mut steps = info(4, "X-M5");
    steps.extend(info(8, "2.06"));
    steps.extend(gate);
    steps.extend(memory_read(STOCK_CALLBACK, &CALLBACK_CODE));
    steps.extend(memory_read(READER, &READER_CODE));
    steps.extend(memory_read(DRIVER, &word(1)));
    steps.extend(memory_read(DRIVER + 0x38, &word(scratch)));
    steps.extend(memory_read(DRIVER + 0x14, &word(scratch_size)));
    steps
}

fn preflight() -> Vec<E> {
    preflight_after(gate(1, false))
}

fn preflight_after(gate: Vec<E>) -> Vec<E> {
    let mut steps = checked_after(gate, SCRATCH, SCRATCH_SIZE);
    steps.extend(memory_read(REGISTERED, &descriptor()));
    steps.extend(memory_read(TABLE, &word(TABLE_BASE)));
    steps.extend(reads(BASE, &saved()));
    steps
}

fn restore(request: &[u8]) -> Vec<E> {
    let mut current = request.to_vec();
    current.extend(&saved()[16..128]);
    let mut steps = reads(BASE, &current);
    steps.extend(write_verified(BASE, &saved()));
    steps
}

fn run(
    steps: Vec<E>,
    payload: &Payload,
    flash: &FirmwareFlash,
    bulk: &FirmwareBulk,
) -> (
    bulk::Result<super::super::Backup>,
    SessionState,
    usize,
    Vec<u8>,
) {
    let (mut session, script) = opened(steps);
    let mut image = vec![0u8; WINDOW_SIZE as usize];
    let result = block_on(super::read(
        &mut session,
        bulk,
        flash,
        payload,
        &[],
        limits(),
        Selection::Whole,
        &mut Callbacks {
            chunk: &mut |offset, bytes| {
                image[offset as usize..offset as usize + bytes.len()].copy_from_slice(&bytes);
                Ok(())
            },
            progress: &mut |_, _| Ok(()),
        },
    ));
    (result, session.state(), script.remaining(), image)
}

#[test]
fn whole_window_reads_stock_chunks_then_the_cpu_tail_and_restores_the_buffer() {
    let (bulk, flash, payload) = fixture();
    let data = flash_bytes();
    let mut steps = preflight();
    steps.extend(stock_read(0, &data[..CONTROL as usize]));
    steps.extend(stock_read(0, &data[..TAIL as usize]));
    let overlap = TAIL - 256;
    steps.extend(stock_read(overlap, &data[overlap as usize..TAIL as usize]));
    steps.extend(reads(
        WINDOW + overlap,
        &data[overlap as usize..TAIL as usize],
    ));
    steps.extend(reads(WINDOW + TAIL, &data[TAIL as usize..]));
    steps.extend(restore(&request(overlap, 256)));
    let (result, state, remaining, image) = run(steps, &payload, &flash, &bulk);
    let backup = result.unwrap();
    assert_eq!(remaining, 0, "every scripted exchange is consumed");
    assert_eq!(state, SessionState::Ready);
    assert_eq!(backup.size, u64::from(WINDOW_SIZE));
    assert_eq!(backup.sha256, hex(&data));
    assert_eq!(image, data);
}

#[test]
fn a_control_chunk_that_differs_stops_the_backup_and_restores_the_buffer() {
    let (bulk, flash, payload) = fixture();
    let mut steps = preflight();
    steps.extend(stock_read(0, &[0xee; CONTROL as usize]));
    steps.extend(restore(&request(0, CONTROL)));
    let (result, state, remaining, _) = run(steps, &payload, &flash, &bulk);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("Control chunk"), "{error}");
    assert_eq!(remaining, 0);
    assert_eq!(state, SessionState::Ready);
}

#[test]
fn a_cpu_window_that_disagrees_with_the_stock_reader_reads_no_tail() {
    let (bulk, flash, payload) = fixture();
    let data = flash_bytes();
    let mut steps = preflight();
    steps.extend(stock_read(0, &data[..CONTROL as usize]));
    steps.extend(stock_read(0, &data[..TAIL as usize]));
    let overlap = TAIL - 256;
    steps.extend(stock_read(overlap, &data[overlap as usize..TAIL as usize]));
    steps.extend(reads(WINDOW + overlap, &[0; 256]));
    steps.extend(restore(&request(overlap, 256)));
    let (result, state, remaining, _) = run(steps, &payload, &flash, &bulk);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("CPU flash window"), "{error}");
    assert_eq!(remaining, 0);
    assert_eq!(state, SessionState::Ready);
}

#[test]
fn a_reader_error_restores_the_callback_before_failing() {
    let (bulk, flash, payload) = fixture();
    let mut steps = preflight();
    steps.extend(write_verified(BASE, &request(0, CONTROL)));
    steps.extend(memory_read(REGISTERED, &descriptor()));
    steps.extend(memory_read(SLOT, &word(STOCK_CALLBACK)));
    steps.extend(write_verified(SLOT, &word(READER)));
    steps.extend(query(1));
    steps.extend(memory_read(SLOT, &word(READER)));
    steps.extend(write_verified(SLOT, &word(STOCK_CALLBACK)));
    steps.extend(restore(&request(0, CONTROL)));
    let (result, state, remaining, _) = run(steps, &payload, &flash, &bulk);
    assert!(result.is_err());
    assert_eq!(
        remaining, 0,
        "the slot and buffer are restored after the error"
    );
    assert_eq!(state, SessionState::Ready);
}

#[test]
fn a_callback_that_is_not_stock_is_left_alone() {
    let (bulk, flash, payload) = fixture();
    let mut steps = preflight();
    steps.extend(write_verified(BASE, &request(0, CONTROL)));
    steps.extend(memory_read(REGISTERED, &descriptor()));
    steps.extend(memory_read(SLOT, &word(0x0030_0000)));
    steps.extend(restore(&request(0, CONTROL)));
    let (result, state, remaining, _) = run(steps, &payload, &flash, &bulk);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("stock value"), "{error}");
    assert_eq!(remaining, 0, "the slot is never written");
    assert_eq!(state, SessionState::Ready);
}

#[test]
fn a_buffer_too_small_for_a_chunk_is_never_borrowed() {
    let (bulk, flash, payload) = fixture();
    let mut steps = checked(SCRATCH, SCRATCH_SIZE);
    steps.extend(memory_read(
        REGISTERED,
        &[BASE, 0x4000]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    ));
    let (result, state, remaining, _) = run(steps, &payload, &flash, &bulk);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("registered diagnostic buffer"), "{error}");
    assert_eq!(remaining, 0);
    assert_eq!(state, SessionState::Ready);
}

#[test]
fn a_closed_debug_gate_is_opened_for_the_capture_and_closed_after() {
    let (bulk, flash, payload) = fixture();
    let mut opened = gate(0, false);
    opened.extend(gate(1, true));
    opened.extend(gate(1, false));
    let mut steps = preflight_after(opened);
    steps.extend(stock_read(0, &[0xee; CONTROL as usize]));
    steps.extend(restore(&request(0, CONTROL)));
    steps.extend(gate(1, false));
    steps.extend(gate(0, true));
    steps.extend(gate(0, false));
    let (result, state, remaining, _) = run(steps, &payload, &flash, &bulk);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("Control chunk"), "{error}");
    assert_eq!(remaining, 0, "the gate is closed again after the capture");
    assert_eq!(state, SessionState::Ready);
}

#[test]
fn a_reader_scratch_under_the_borrowed_buffer_is_never_borrowed() {
    let (bulk, flash, payload) = fixture();
    let mut steps = checked(BASE + 0x1000, SCRATCH_SIZE);
    steps.extend(memory_read(REGISTERED, &descriptor()));
    steps.extend(memory_read(TABLE, &word(TABLE_BASE)));
    let (result, state, remaining, _) = run(steps, &payload, &flash, &bulk);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("reader scratch"), "{error}");
    assert_eq!(remaining, 0, "nothing is saved or written");
    assert_eq!(state, SessionState::Ready);
}

#[test]
fn an_unbounded_reader_scratch_stops_before_the_buffer_is_read() {
    let (bulk, flash, payload) = fixture();
    for size in [8, 0x20000] {
        let (result, state, remaining, _) = run(checked(SCRATCH, size), &payload, &flash, &bulk);
        let error = result.unwrap_err().to_string();
        assert!(error.contains("Unbounded"), "{error}");
        assert_eq!(remaining, 0);
        assert_eq!(state, SessionState::Ready);
    }
}

#[test]
fn validation_requires_the_stock_query_contract() {
    let (bulk, flash, payload) = fixture();
    payload.validate(&flash, &bulk, &[]).unwrap();
    let refused = |flash: &FirmwareFlash, bulk: &FirmwareBulk, payload: &Payload, image: &[u8]| {
        payload
            .validate(flash, bulk, image)
            .unwrap_err()
            .to_string()
    };
    assert!(refused(&flash, &bulk, &payload, &[0]).contains("no image"));
    let mut other = flash.clone();
    other.transport = Some("xpp-cmd-callback".into());
    assert!(refused(&other, &bulk, &payload, &[]).contains("transport"));
    let mut other = flash.clone();
    other.reader = None;
    assert!(refused(&other, &bulk, &payload, &[]).contains("reader contract"));
    let mut unchecked = payload.clone();
    unchecked.checks.clear();
    assert!(refused(&flash, &bulk, &unchecked, &[]).contains("not checked"));
    let mut other = payload.clone();
    other.callback.original = READER;
    assert!(refused(&flash, &bulk, &other, &[]).contains("query callback"));
    let mut other = flash.clone();
    other.window.size = 0x1080;
    assert!(refused(&other, &bulk, &payload, &[]).contains("geometry"));
    let other = zero_base_camera();
    assert!(refused(&flash, &other, &payload, &[]).contains("registered 32-bit"));
    let mut other = flash.clone();
    other.reader.as_mut().unwrap().scratch = None;
    assert!(refused(&other, &bulk, &payload, &[]).contains("scratch is absent"));
    let mut other = payload.clone();
    other.callback.checks.clear();
    assert!(refused(&flash, &bulk, &other, &[]).contains("callback code"));
}
