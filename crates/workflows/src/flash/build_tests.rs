use super::*;
use fujimod_catalog::{
    capture::{
        FLASH_SCHEMA_URI, FlashCodeRange, FlashControl, FlashDevice, FlashRange, FlashReader,
        FlashReaderScratch, FlashStateCheck, FlashUpdateImage,
    },
    reference::ContentIdentity,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

const FIRMWARE: &str = "xp4-sample";
const STOCK_SYMBOL: &str = "flash_read_xp4";
const SOURCE: u64 = 0x4000_0000;
const UNIT_BYTES: usize = 0x200;
const CALLBACK_OFFSET: usize = 0x40;
const CALLBACK_SIZE: u64 = 0x40;
const STOCK_OFFSET: usize = 0x80;
const STOCK_SIZE: u64 = 0x40;
const CONTROL_SIZE: u64 = 0x40;
const BSS: u64 = 0x4002_0000;
const BSS_SIZE: u64 = 0x100;
const SLOT: u64 = BSS;
const TAG: u32 = 0x5058_3401;
const WINDOW_ADDRESS: u64 = 0x3000_0000;
const WINDOW_SIZE: u64 = 0x200;
const ARENA: u64 = 0x4000_1000;
const ARENA_CAPACITY: u64 = 0x1000;
const IMAGE_SIZE: usize = 0x100;
const ENTRY: u64 = ARENA + 0x10;
const ENTRY_SIZE: u64 = 0x20;
const REQUEST: u64 = ARENA + 0x40;
const RESULT: u64 = ARENA + 0x60;
const BUFFER: u64 = 0x4002_1000;
const BUFFER_CAPACITY: u32 = 0x1000;

struct Fixture {
    dir: PathBuf,
    input: Selection,
    selected: ResolvedSelection,
    flash: FirmwareFlash,
    bulk: FirmwareBulk,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/xp4-build-tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn edit_json(path: &Path, edit: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    edit(&mut value);
    write_json(path, &value);
}

fn fixture() -> Fixture {
    let mut container = vec![0u8; UNIT_BYTES];
    container[..CONTROL_SIZE as usize].fill(0x5a);
    container[CALLBACK_OFFSET..CALLBACK_OFFSET + CALLBACK_SIZE as usize].fill(0xa5);
    container[STOCK_OFFSET..STOCK_OFFSET + STOCK_SIZE as usize].fill(0x3c);
    let update = invert(&container);
    let update_sha = sha256(&update);
    let image = vec![0x11u8; IMAGE_SIZE];
    let image_sha = sha256(&image);

    let dir = scratch();
    let update_path = dir.join("update.bin");
    let callback_path = dir.join("callback.json");
    let load_map_path = dir.join("load-map.json");
    let manifest_path = dir.join("manifest.json");
    let bulk_path = dir.join("bulk.json");
    let image_path = dir.join("flash-reader-xp4.bin");
    fs::write(&update_path, &update).unwrap();
    fs::write(&image_path, &image).unwrap();
    fs::write(&bulk_path, b"{}").unwrap();

    write_json(
        &callback_path,
        &json!({
            "schema": "xp4-cmd-callback-v1",
            "firmware_id": FIRMWARE,
            "firmware_sha256": update_sha.clone(),
            "tag": TAG,
            "callback": {
                "address": SLOT,
                "original": 0u32,
                "body": {
                    "address": SOURCE + CALLBACK_OFFSET as u64,
                    "size": CALLBACK_SIZE,
                },
            },
            "buffer": {
                "address": BUFFER,
                "size": u64::from(BUFFER_CAPACITY),
            },
        }),
    );
    write_json(
        &load_map_path,
        &json!({
            "format_version": 2,
            "firmware_id": FIRMWARE,
            "firmware_sha256": update_sha.clone(),
            "facts": {
                "load_map": {
                    "value": {
                        "units": [{
                            "role": "resident",
                            "address": SOURCE,
                            "declared_size": UNIT_BYTES as u64,
                            "bss": BSS,
                            "bss_size": BSS_SIZE,
                            "source": {
                                "dat_offset": 0u64,
                                "size": UNIT_BYTES as u64,
                                "compression": "raw",
                                "decoded_size": UNIT_BYTES as u64,
                                "sha256": sha256(&container),
                            },
                        }],
                    },
                },
            },
        }),
    );
    write_json(
        &manifest_path,
        &json!({
            "target": FIRMWARE,
            "firmware_sha256": update_sha.clone(),
            "mode": "runtime",
            "region": {"address": ARENA, "size": ARENA_CAPACITY},
            "regions": [{
                "file": "flash-reader-xp4.bin",
                "address": ARENA,
                "size": IMAGE_SIZE as u64,
                "sha256": image_sha,
            }],
            "symbols": [
                {"name": "flash_reader", "address": ENTRY, "size": ENTRY_SIZE, "type": "STT_FUNC"},
                {"name": "flash_reader_request", "address": REQUEST, "size": 16u64, "type": "STT_OBJECT"},
                {"name": "flash_reader_result", "address": RESULT, "size": 24u64, "type": "STT_OBJECT"},
            ],
            "entrypoints": [{"name": "flash_reader", "address": ENTRY}],
            "stock_symbols": [{"name": STOCK_SYMBOL, "address": SOURCE + STOCK_OFFSET as u64}],
            "words": [],
            "overrides": [],
            "inits": [],
        }),
    );

    let flash = FirmwareFlash {
        schema_uri: FLASH_SCHEMA_URI.into(),
        schema: 1,
        id: "xp4-sample-flash".into(),
        firmware_id: FIRMWARE.into(),
        device: FlashDevice {
            kind: "serial-nor".into(),
            jedec_id: Some("001122".into()),
            part: Some("Test NOR".into()),
        },
        window: FlashRange {
            address: WINDOW_ADDRESS,
            size: WINDOW_SIZE,
        },
        e1_window: None,
        update_image: FlashUpdateImage {
            dat_offset: 0,
            xor: 255,
        },
        reader: Some(FlashReader {
            symbol: STOCK_SYMBOL.into(),
            alignment: 32,
            nand: None,
            scratch: None,
        }),
        transport: Some("xp4-camera-code".into()),
        code_ranges: vec![FlashCodeRange {
            name: STOCK_SYMBOL.into(),
            address: SOURCE + STOCK_OFFSET as u64,
            size: STOCK_SIZE,
        }],
        state_checks: vec![FlashStateCheck {
            name: "driver".into(),
            address: SLOT,
            size: 4,
            value: 0,
        }],
        control: FlashControl {
            offset: 0,
            size: CONTROL_SIZE,
            sha256: sha256(&container[..CONTROL_SIZE as usize]),
        },
    };
    let mut bulk = crate::bulk::tests::zero_base_camera();
    bulk.firmware_id = FIRMWARE.into();

    let input = Selection {
        firmware_id: FIRMWARE.into(),
        update: update_path,
        load_map: load_map_path,
        callback: callback_path,
        bulk: bulk_path,
        manifest: manifest_path,
        command: vec!["true".into()],
        display: json!({}),
        index: None,
    };
    let mut selected = ReferenceCatalog::bundled()
        .unwrap()
        .resolve_firmware("x70-110")
        .unwrap();
    selected.firmware.id = FIRMWARE.into();
    selected.firmware.update = Some(ContentIdentity {
        size: update.len() as u64,
        sha256: update_sha,
    });
    selected.flash = Some(flash.clone());
    selected.bulk = Some(bulk.clone());
    Fixture {
        dir,
        input,
        selected,
        flash,
        bulk,
    }
}

#[test]
fn verified_xp4_reader_stages_a_command_artifact() {
    let fixture = fixture();
    let (artifact, image) = prepare(&fixture.input, &fixture.selected).unwrap();
    assert_eq!(image.len(), IMAGE_SIZE);
    assert_eq!(artifact.image.as_deref(), Some("flash-reader.bin"));
    let Payload::Xp4Command(payload) = artifact.payload else {
        panic!("expected an XP4 command artifact");
    };
    assert_eq!(payload.schema, 1);
    assert_eq!(payload.firmware_id, FIRMWARE);
    assert_eq!(payload.address, ARENA);
    assert_eq!(payload.capacity, ARENA_CAPACITY);
    assert_eq!(payload.entry, ENTRY);
    assert_eq!(payload.buffer, BUFFER);
    assert_eq!(payload.buffer_capacity, BUFFER_CAPACITY);
    assert_eq!(payload.tag, TAG);
    assert_eq!(payload.transport.address, SLOT);
    assert_eq!(payload.checks.len(), 1);
    assert_eq!(payload.transport.checks.len(), 1);
}

#[test]
fn buffer_overlapping_the_executable_reservation_is_refused() {
    let fixture = fixture();
    edit_json(&fixture.input.callback, |profile| {
        profile["buffer"]["address"] = json!(ARENA + 0x10);
    });
    let error = prepare_xp4(
        &fixture.input,
        &fixture.selected,
        &fixture.flash,
        &fixture.bulk,
    )
    .unwrap_err();
    assert!(error.to_string().contains("overlap"), "{error}");
}

#[test]
fn oversized_buffer_or_arena_capacity_is_refused() {
    let oversized_buffer = fixture();
    edit_json(&oversized_buffer.input.callback, |profile| {
        profile["buffer"]["size"] = json!(u64::from(BUFFER_CAPACITY) * 0x10);
    });
    let error = prepare_xp4(
        &oversized_buffer.input,
        &oversized_buffer.selected,
        &oversized_buffer.flash,
        &oversized_buffer.bulk,
    )
    .unwrap_err();
    assert!(error.to_string().contains("32 KiB"), "{error}");

    let oversized_arena = fixture();
    edit_json(&oversized_arena.input.manifest, |manifest| {
        manifest["region"]["size"] = json!(0x1_0000u64);
    });
    let error = prepare_xp4(
        &oversized_arena.input,
        &oversized_arena.selected,
        &oversized_arena.flash,
        &oversized_arena.bulk,
    )
    .unwrap_err();
    assert!(error.to_string().contains("compiled XP4 region"), "{error}");
}

#[test]
fn wrong_transport_is_refused() {
    let mut fixture = fixture();
    fixture.flash.transport = Some("xp5-cmd-callback".into());
    let error = prepare_xp4(
        &fixture.input,
        &fixture.selected,
        &fixture.flash,
        &fixture.bulk,
    )
    .unwrap_err();
    assert!(error.to_string().contains("xp4-camera-code"), "{error}");
}

#[test]
fn buffer_overlapping_the_callback_slot_is_refused() {
    let fixture = fixture();
    edit_json(&fixture.input.callback, |profile| {
        profile["buffer"]["address"] = json!(SLOT);
    });
    let error = prepare_xp4(
        &fixture.input,
        &fixture.selected,
        &fixture.flash,
        &fixture.bulk,
    )
    .unwrap_err();
    assert!(error.to_string().contains("overlap"), "{error}");
}

#[test]
fn unaligned_entry_is_refused() {
    let fixture = fixture();
    let unaligned = ENTRY + 2;
    edit_json(&fixture.input.manifest, |manifest| {
        manifest["symbols"][0]["address"] = json!(unaligned);
        manifest["entrypoints"][0]["address"] = json!(unaligned);
    });
    let error = prepare_xp4(
        &fixture.input,
        &fixture.selected,
        &fixture.flash,
        &fixture.bulk,
    )
    .unwrap_err();
    assert!(error.to_string().contains("aligned"), "{error}");
}

#[test]
fn control_hash_mismatch_is_refused() {
    let mut fixture = fixture();
    fixture.flash.control.sha256 = sha256(b"different control");
    let error = prepare_xp4(
        &fixture.input,
        &fixture.selected,
        &fixture.flash,
        &fixture.bulk,
    )
    .unwrap_err();
    assert!(error.to_string().contains("control"), "{error}");
}

const TABLE: u64 = 0x4003_0000;
const STOCK_CONTROL: u64 = 0x80;

/// The XP4 fixture with a stock-query contract, a registered diagnostic
/// buffer and restart facts whose borrowed query names the fixture callback.
fn stock_fixture() -> Fixture {
    let mut fixture = fixture();
    let update = fs::read(&fixture.input.update).unwrap();
    let container = invert(&update);
    let mut flash = fixture.flash.clone();
    flash.window.size = 0x1200;
    flash.reader = Some(FlashReader {
        symbol: STOCK_SYMBOL.into(),
        alignment: 128,
        nand: None,
        scratch: Some(FlashReaderScratch {
            state: FlashRange {
                address: BSS + 0x80,
                size: 0x60,
            },
            pointer: BSS + 0x80 + 0x38,
            size: BSS + 0x80 + 0x14,
        }),
    });
    flash.transport = Some(super::super::xp4_stock::TRANSPORT.into());
    flash.control = FlashControl {
        offset: 0,
        size: STOCK_CONTROL,
        sha256: sha256(&container[..STOCK_CONTROL as usize]),
    };
    fixture.bulk.source = fujimod_catalog_bulk::BulkSource::Pattern {
        registered_buffer: BSS,
        source_state: BSS + 8,
        capacity: 0x10000,
    };
    let mut restart = ReferenceCatalog::bundled()
        .unwrap()
        .resolve_firmware("xe4-201")
        .unwrap()
        .restart
        .unwrap();
    let callback = SOURCE + CALLBACK_OFFSET as u64;
    let Some(RequestTransport::BorrowedQuery {
        callback_table,
        stock_callback,
        forward,
        ..
    }) = restart.mechanism.transport.as_mut()
    else {
        panic!("the X-E4 restart facts borrow the diagnostic query");
    };
    *callback_table = TABLE;
    *stock_callback = callback;
    *forward = Some(SOURCE);
    restart.code_checks = vec![
        fujimod_catalog_bulk::CodeCheck {
            name: "query callback".into(),
            address: callback,
            size: CALLBACK_SIZE,
            sha256: "0".repeat(64),
        },
        fujimod_catalog_bulk::CodeCheck {
            name: "query forwarder".into(),
            address: SOURCE,
            size: 0x20,
            sha256: "0".repeat(64),
        },
    ];
    fixture.selected.flash = Some(flash.clone());
    fixture.selected.bulk = Some(fixture.bulk.clone());
    fixture.selected.restart = Some(restart);
    fixture.flash = flash;
    fixture
}

#[test]
fn stock_query_stages_metadata_from_the_flash_and_restart_facts() {
    let fixture = stock_fixture();
    let (artifact, image) = prepare(&fixture.input, &fixture.selected).unwrap();
    assert!(image.is_empty());
    assert_eq!(artifact.image, None);
    let Payload::Xp4StockQuery(payload) = artifact.payload else {
        panic!("expected an XP4 stock-query artifact");
    };
    assert_eq!(payload.stock_reader, SOURCE + STOCK_OFFSET as u64);
    assert_eq!(payload.callback.table, TABLE);
    assert_eq!(payload.callback.offset, 12);
    assert_eq!(
        u64::from(payload.callback.original),
        SOURCE + CALLBACK_OFFSET as u64
    );
    assert_eq!(
        payload.callback.checks[0].sha256,
        sha256(&[0xa5; CALLBACK_SIZE as usize]),
        "the callback hash comes from the update, not the restart document"
    );
    assert_eq!(
        payload.callback.checks[1].address, SOURCE,
        "every restart code check is kept, including the forwarder"
    );
    assert_eq!(payload.checks.len(), 1);
    assert_eq!(payload.update_size, UNIT_BYTES as u64);
}

#[test]
fn stock_query_needs_the_restart_query_callback() {
    let mut fixture = stock_fixture();
    fixture.selected.restart = None;
    let error = prepare(&fixture.input, &fixture.selected).unwrap_err();
    assert!(error.to_string().contains("restart facts"), "{error}");

    let mut fixture = stock_fixture();
    fixture
        .selected
        .restart
        .as_mut()
        .unwrap()
        .code_checks
        .clear();
    let error = prepare(&fixture.input, &fixture.selected).unwrap_err();
    assert!(error.to_string().contains("not checked"), "{error}");

    let mut fixture = stock_fixture();
    fixture
        .selected
        .restart
        .as_mut()
        .unwrap()
        .code_checks
        .truncate(1);
    let error = prepare(&fixture.input, &fixture.selected).unwrap_err();
    assert!(error.to_string().contains("forwarder"), "{error}");
}

#[test]
fn stock_query_control_must_match_the_update() {
    let mut fixture = stock_fixture();
    fixture.selected.flash.as_mut().unwrap().control.sha256 = "0".repeat(64);
    let error = prepare(&fixture.input, &fixture.selected).unwrap_err();
    assert!(error.to_string().contains("control differs"), "{error}");
}
