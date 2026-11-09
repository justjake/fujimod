use super::mapping::Operation;
use super::read as read_bulk;
use super::*;
use fujimod_catalog_bulk::{BULK_SCHEMA_URI, BulkDma, CaptureScope, CodeCheck, NamedRange};
use fujimod_usb::{
    ff80::{commands::memory::DebugMemoryLimits, framing::RequestKind},
    operations::checks::sha256_bytes,
    testing::{Exchange as E, Script},
};
use futures_lite::future::{block_on, poll_once};

const CODE: &[u8] = b"stock code";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn facts(source: BulkSource) -> FirmwareBulk {
    let facts = FirmwareBulk {
        upload: None,
        schema_uri: BULK_SCHEMA_URI.into(),
        schema: 1,
        id: "sample-1-bulk".into(),
        firmware_id: "sample-1".into(),
        product: "X-M5".into(),
        internal_version: "2.06".into(),
        code_alias: 0,
        source,
        window: BulkRange {
            address: 0x1000,
            size: 0x1000,
        },
        dma: None,
        code_checks: vec![CodeCheck {
            name: "e1".into(),
            address: 0x1000,
            size: CODE.len() as u64,
            sha256: hex(&sha256_bytes(CODE)),
        }],
        probes: vec![BulkRange {
            address: 0x1000,
            size: 4,
        }],
        mapped_probe: None,
        scopes: vec![CaptureScope {
            name: "quick".into(),
            basis: "Test data.".into(),
            ranges: vec![NamedRange {
                name: "data".into(),
                address: 0x1800,
                size: 0x40,
            }],
        }],
    };
    facts.validate().unwrap();
    facts
}
pub(crate) fn query_camera() -> FirmwareBulk {
    facts(BulkSource::Query {
        tag: 0x80,
        callback_table: 0x1100,
        callback_offset: 12,
        stock_callback: 0x2000,
        base: 0x1000,
        source_state: 0x1200,
        capacities: vec![32],
        wrapping: true,
    })
}
pub(crate) fn zero_base_camera() -> FirmwareBulk {
    facts(BulkSource::PatternZeroBase {
        registered_buffer: 0x1300,
        source_state: 0x1400,
        configuration_pointer: 0x1500,
        configuration_bytes: 0x100,
        system_mode_address: 0x1600,
        capacity: 32,
    })
}
pub(crate) fn publication_camera(capacity: u32) -> FirmwareBulk {
    facts(BulkSource::Publication {
        source_state: 0x1200,
        capacity,
    })
}
pub(crate) fn limits() -> Limits {
    Limits {
        memory: DebugMemoryLimits {
            read: NonZeroU32::new(64).unwrap(),
            write: NonZeroU32::new(64).unwrap(),
        },
        configuration: NonZeroU32::MIN,
    }
}
fn target() -> BulkRange {
    camera_scope_range(&query_camera())
}
fn camera_scope_range(camera: &FirmwareBulk) -> BulkRange {
    camera.scopes[0].ranges[0].range()
}

fn envelope(kind: RequestKind, offset: u32, size: u32, session: u16) -> [u8; 12] {
    let mut raw = [0; 12];
    raw[0] = kind as u8;
    raw[2..6].copy_from_slice(&offset.to_be_bytes());
    raw[6..10].copy_from_slice(&size.to_be_bytes());
    raw[10..12].copy_from_slice(&session.to_be_bytes());
    raw
}
pub(crate) fn opened(steps: Vec<E>) -> (Session, Script) {
    opened_with(65536, steps)
}
pub(crate) fn opened_with(max_reply_bytes: u32, steps: Vec<E>) -> (Session, Script) {
    let mut all = vec![
        E::Control(envelope(RequestKind::Open, 0, 8, 0)),
        E::Read {
            maximum: 8,
            data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
        },
    ];
    all.extend(steps);
    let script = Script::new(all);
    (
        block_on(script.begin_session(max_reply_bytes, None)).unwrap(),
        script,
    )
}
pub(crate) fn exchange_max(request: Vec<u8>, reply: Vec<u8>, maximum: usize) -> Vec<E> {
    vec![
        E::Control(envelope(
            RequestKind::WriteCommand,
            0,
            request.len() as u32,
            0x1234,
        )),
        E::Write(request),
        E::Control(envelope(
            RequestKind::ReadCommand,
            0,
            maximum as u32,
            0x1234,
        )),
        E::Read {
            maximum,
            data: reply,
        },
    ]
}
pub(crate) fn exchange(request: Vec<u8>, reply: Vec<u8>) -> Vec<E> {
    let maximum = reply.len();
    exchange_max(request, reply, maximum)
}
pub(crate) fn info(selector: u32, text: &str) -> Vec<E> {
    let mut request = vec![0; 32];
    request[..4].copy_from_slice(&0x150000u32.to_le_bytes());
    request[16..20].copy_from_slice(&selector.to_le_bytes());
    let mut reply = request[..16].to_vec();
    reply.extend(
        text.encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes),
    );
    exchange_max(request, reply, 128)
}
fn memory_request(function: u32, at: u32, size: u32) -> Vec<u8> {
    [0x200001, 0, 0, 0, function, 0, 0, at, size]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}
pub(crate) fn read(at: u32, bytes: &[u8]) -> Vec<E> {
    let request = memory_request(2, at, bytes.len() as u32);
    let mut reply = request.clone();
    reply.extend_from_slice(bytes);
    exchange(request, reply)
}
pub(crate) fn write(at: u32, bytes: &[u8]) -> Vec<E> {
    let mut request = memory_request(3, at, bytes.len() as u32);
    request.extend_from_slice(bytes);
    let reply = request[..36].to_vec();
    exchange(request, reply)
}
pub(crate) fn diagnostic(opcode: u32, argument: u32, capacity: u32) -> Vec<E> {
    let mut request = vec![0; 32];
    request[..4].copy_from_slice(&opcode.to_le_bytes());
    request[16..20].copy_from_slice(&argument.to_le_bytes());
    let mut reply = request.clone();
    reply[20..24].copy_from_slice(&capacity.to_le_bytes());
    exchange(request, reply)
}
pub(crate) fn data(offset: u32, bytes: &[u8]) -> Vec<E> {
    vec![
        E::Control(envelope(
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
pub(crate) fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
pub(crate) fn qualified() -> Vec<E> {
    let mut steps = info(4, "X-M5");
    steps.extend(info(8, "2.06"));
    steps.extend(read(0x1000, CODE));
    steps
}

#[test]
fn query_source_reads_the_range_relative_to_the_stock_base() {
    let mut steps = qualified();
    steps.extend(read(0x1100, &words(&[0x1500])));
    steps.extend(read(0x150c, &words(&[0x2000])));
    steps.extend(diagnostic(0x450006, 0x80, 32));
    steps.extend(read(0x1200, &words(&[0x1000, 32])));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(data(0, b"stoc"));
    steps.extend(data(0x800, &[1; 32]));
    steps.extend(data(0x820, &[2; 32]));
    let (mut session, script) = opened(steps);
    let read = block_on(read_bulk(&mut session, &query_camera(), limits(), target())).unwrap();
    assert_eq!(read.data, [[1; 32], [2; 32]].concat());
    assert_eq!(read.overwritten, None);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn mounted_reader_rounds_dma_inside_its_window_and_never_caches_live_bytes() {
    use fujimod_usb::operations::card::BufferIo;
    let mut steps = data(0x800, &(0u8..32).collect::<Vec<_>>());
    steps.extend(data(0x820, &(32u8..64).collect::<Vec<_>>()));
    steps.extend(data(0x800, &[0xa5; 32]));
    steps.extend(data(0x820, &[0xa5; 32]));
    let (mut session, script) = opened(steps);
    let window = BulkRange {
        address: 0x1000,
        size: 0x1000,
    };
    let mut camera = query_camera();
    camera.dma = Some(BulkDma {
        address_alignment: 32,
        length_granule: 32,
    });
    let mut mapping = BulkMapping {
        bulk: camera,
        limits: limits(),
        alignment: Some(DataAlignment::new(32, 32).unwrap()),
        phase: BulkMappingPhase::Ready,
        source: Source {
            window: Some(
                DataWindow::new(
                    Address(0x1000),
                    memory_range(window).unwrap(),
                    NonZeroU32::new(32).unwrap(),
                    OffsetMode::Wrapping,
                )
                .unwrap()
                .with_alignment(DataAlignment::new(32, 32).unwrap()),
            ),
            overwritten: None,
            loan: None,
            restored: true,
        },
    };
    assert!(mapping.admits(0x1fff, 1));
    assert!(!mapping.admits(0x1fff, 2));
    assert!(!mapping.admits(0xfff, 1));
    let bytes = block_on(BufferIo::read(&mut mapping, &mut session, 0x1803, 35)).unwrap();
    assert_eq!(bytes, (3u8..38).collect::<Vec<_>>());
    assert_eq!(
        block_on(BufferIo::read(&mut mapping, &mut session, 0x1803, 35)).unwrap(),
        vec![0xa5; 35]
    );
    assert!(matches!(
        block_on(BufferIo::read(&mut mapping, &mut session, 0x1fff, 2)),
        Err(fujimod_usb::Error::OutsideDataWindow)
    ));
    assert_eq!(script.remaining(), 0);
}

#[test]
fn zero_base_source_is_restored_after_reading_addresses_directly() {
    let original = [7u8; 12];
    let selected = words(&[0, 0, 64]);
    let mut steps = qualified();
    steps.extend(read(0x1300, &words(&[0x10_0000, 0, 64])));
    steps.extend(read(0x1400, &original));
    steps.extend(write(0x1400, &selected));
    steps.extend(read(0x1400, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1400, &selected));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(read(0x1400, &selected));
    steps.extend(read(0x1400, &selected));
    steps.extend(data(0x1800, &[[3; 32], [4; 32]].concat()));
    steps.extend(read(0x1400, &selected));
    steps.extend(read(0x1400, &selected));
    steps.extend(write(0x1400, &original));
    steps.extend(read(0x1400, &original));
    let (mut session, script) = opened(steps);
    let camera = zero_base_camera();
    let read = block_on(read_bulk(
        &mut session,
        &camera,
        limits(),
        camera_scope_range(&camera),
    ))
    .unwrap();
    assert_eq!(read.data, [[3; 32], [4; 32]].concat());
    assert_eq!(read.overwritten, None);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn zero_base_refuses_a_registered_capacity_smaller_than_the_located_contract() {
    let mut steps = qualified();
    steps.extend(read(0x1300, &words(&[0x20_0000, 0, 31])));
    let (mut session, script) = opened(steps);
    let camera = zero_base_camera();
    let error = block_on(read_bulk(
        &mut session,
        &camera,
        limits(),
        camera_scope_range(&camera),
    ))
    .unwrap_err();
    assert!(error.to_string().contains("capacity"), "{error}");
    assert_eq!(script.remaining(), 0);
}

#[test]
fn zero_base_read_mismatch_restores_the_source_before_refusing() {
    let original = [7u8; 12];
    let selected = words(&[0, 0, 64]);
    let mut steps = qualified();
    steps.extend(read(0x1300, &words(&[0x10_0000, 0, 64])));
    steps.extend(read(0x1400, &original));
    steps.extend(write(0x1400, &selected));
    steps.extend(read(0x1400, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1400, &selected));
    steps.extend(data(0x1000, b"oops"));
    steps.extend(read(0x1400, &selected));
    steps.extend(read(0x1400, &selected));
    steps.extend(write(0x1400, &original));
    steps.extend(read(0x1400, &original));
    let (mut session, script) = opened(steps);
    let camera = zero_base_camera();
    assert!(
        block_on(read_bulk(
            &mut session,
            &camera,
            limits(),
            camera_scope_range(&camera)
        ))
        .is_err()
    );
    assert_eq!(script.remaining(), 0);
}

#[test]
fn reads_outside_the_window_or_from_another_camera_send_nothing_further() {
    let (mut session, script) = opened(vec![]);
    let outside = BulkRange {
        address: 0x1ff0,
        size: 0x20,
    };
    assert!(block_on(read_bulk(&mut session, &query_camera(), limits(), outside)).is_err());
    assert_eq!(script.remaining(), 0);
    let (mut session, script) = opened(info(4, "X100VI"));
    let error = block_on(read_bulk(&mut session, &query_camera(), limits(), target())).unwrap_err();
    assert!(
        error.to_string().contains("not the selected camera"),
        "{error}"
    );
    assert_eq!(script.remaining(), 0);
}

#[test]
fn dma_constraints_refuse_unaligned_ranges_before_any_request() {
    let mut camera = query_camera();
    camera.dma = Some(BulkDma {
        address_alignment: 4,
        length_granule: 4,
    });
    camera.validate().unwrap();
    for target in [
        BulkRange {
            address: 0x1802,
            size: 0x20,
        },
        BulkRange {
            address: 0x1800,
            size: 0x22,
        },
    ] {
        let (mut session, script) = opened(vec![]);
        let error = block_on(read_bulk(&mut session, &camera, limits(), target)).unwrap_err();
        assert!(error.to_string().contains("not aligned"), "{error}");
        assert_eq!(script.remaining(), 0);
    }
    camera.dma = Some(BulkDma {
        address_alignment: 4,
        length_granule: 64,
    });
    assert!(
        camera.validate().is_err(),
        "4-byte probe is not a whole granule"
    );
}

#[test]
fn foreign_query_callback_stops_before_the_query() {
    let mut steps = qualified();
    steps.extend(read(0x1100, &words(&[0x1500])));
    steps.extend(read(0x150c, &words(&[0x2004])));
    let (mut session, script) = opened(steps);
    assert!(block_on(read_bulk(&mut session, &query_camera(), limits(), target())).is_err());
    assert_eq!(script.remaining(), 0);
}

#[test]
fn preserving_reads_support_every_source_family_and_restore_without_pattern_writes() {
    block_on(async {
        for camera in [
            query_camera(),
            facts(BulkSource::Pattern {
                registered_buffer: 0x1300,
                source_state: 0x1400,
                capacity: 32,
            }),
            zero_base_camera(),
        ] {
            let mut steps = qualified();
            let (state, original, selected, base) = match camera.source {
                BulkSource::Query { .. } => {
                    let original = words(&[0xdead0000, 16]);
                    steps.extend(read(0x1200, &original));
                    steps.extend(read(0x1100, &words(&[0x1500])));
                    steps.extend(read(0x150c, &words(&[0x2000])));
                    steps.extend(diagnostic(0x450006, 0x80, 32));
                    let selected = words(&[0x1000, 32]);
                    steps.extend(read(0x1200, &selected));
                    steps.extend(read(0x1200, &selected));
                    (0x1200, original, selected, 0x1000)
                }
                BulkSource::DirectQuery { .. } => panic!("direct query handled separately"),
                BulkSource::Publication { .. } => panic!("publication handled separately"),
                BulkSource::Pattern { .. } | BulkSource::PatternZeroBase { .. } => {
                    let wide = matches!(camera.source, BulkSource::PatternZeroBase { .. });
                    let original = if wide {
                        words(&[0x9000, 1, 16])
                    } else {
                        words(&[0x9000, 16])
                    };
                    let selected = if wide {
                        words(&[0, 0, 32])
                    } else {
                        words(&[0, 32])
                    };
                    steps.extend(read(
                        0x1300,
                        &if wide {
                            words(&[0x9000, 0, 32])
                        } else {
                            words(&[0x9000, 32])
                        },
                    ));
                    steps.extend(read(0x1400, &original));
                    steps.extend(write(0x1400, &selected));
                    steps.extend(read(0x1400, &selected));
                    (0x1400, original, selected, 0)
                }
            };
            steps.extend(read(0x1000, b"stoc"));
            steps.extend(read(state, &selected));
            steps.extend(data(0x1000 - base, b"stoc"));
            steps.extend(read(state, &selected));
            steps.extend(read(state, &selected));
            steps.extend(data(0x1800 - base, &[7; 32]));
            steps.extend(read(state, &selected));
            steps.extend(read(state, &selected));
            steps.extend(data(0x1820 - base, &[8; 32]));
            steps.extend(read(state, &selected));
            steps.extend(read(state, &selected));
            steps.extend(write(state, &original));
            steps.extend(read(state, &original));
            let (mut session, script) = opened(steps);
            let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
            mapping.prepare_preserving(&mut session).await.unwrap();
            assert_eq!(mapping.overwritten(), None);
            assert_eq!(
                mapping.read(&mut session, target()).await.unwrap(),
                [[7; 32], [8; 32]].concat()
            );
            mapping.restore(&mut session).await.unwrap();
            assert_eq!(script.remaining(), 0, "all publication words restored");
        }
    });
}

#[test]
fn query_publication_is_restored_through_the_stock_cacheable_view() {
    // Observed on X70 after a fresh restart, 2026-11-03: restoring
    // 0x417d548c while callbacks use 0x017d548c failed readback after a
    // freshly restarted body. The aliases must not be mixed for mutable state.
    block_on(async {
        let mut camera = query_camera();
        camera.code_alias = 0x40000000;
        camera.window.address |= camera.code_alias;
        if let BulkSource::Query {
            callback_table,
            source_state,
            base,
            ..
        } = &mut camera.source
        {
            *callback_table |= camera.code_alias;
            *source_state |= camera.code_alias;
            *base |= camera.code_alias;
        }
        camera.probes[0].address |= camera.code_alias;
        for scope in &mut camera.scopes {
            for range in &mut scope.ranges {
                range.address |= camera.code_alias;
            }
        }
        let original = words(&[0xdead0000, 16]);
        let selected = words(&[0x40001000, 32]);
        let mut steps = info(4, "X-M5");
        steps.extend(info(8, "2.06"));
        steps.extend(read(0x40001000, CODE));
        steps.extend(read(0x1200, &original));
        steps.extend(read(0x40001100, &words(&[0x1500])));
        steps.extend(read(0x4000150c, &words(&[0x2000])));
        steps.extend(diagnostic(0x450006, 0x80, 32));
        steps.extend(read(0x1200, &selected));
        steps.extend(read(0x1200, &selected));
        steps.extend(read(0x40001000, b"stoc"));
        steps.extend(read(0x1200, &selected));
        steps.extend(data(0, b"stoc"));
        steps.extend(read(0x1200, &selected));
        steps.extend(read(0x1200, &selected));
        steps.extend(write(0x1200, &original));
        steps.extend(read(0x1200, &original));
        let (mut session, script) = opened(steps);
        let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
        mapping.prepare_preserving(&mut session).await.unwrap();
        mapping.restore(&mut session).await.unwrap();
        assert_eq!(
            script.remaining(),
            0,
            "publication never writes the uncached alias"
        );
    });
}

#[test]
fn preserving_bulk_mismatch_restores_publication_before_refusing() {
    let camera = facts(BulkSource::Pattern {
        registered_buffer: 0x1300,
        source_state: 0x1400,
        capacity: 32,
    });
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = qualified();
    steps.extend(read(0x1300, &words(&[0x9000, 32])));
    steps.extend(read(0x1400, &original));
    steps.extend(write(0x1400, &selected));
    steps.extend(read(0x1400, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1400, &selected));
    steps.extend(data(0x1000, b"oops"));
    steps.extend(read(0x1400, &selected));
    steps.extend(read(0x1400, &selected));
    steps.extend(write(0x1400, &original));
    steps.extend(read(0x1400, &original));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    assert!(block_on(mapping.prepare_preserving(&mut session)).is_err());
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn foreign_publication_during_preserving_setup_is_never_overwritten_and_fences_the_session() {
    let camera = facts(BulkSource::Pattern {
        registered_buffer: 0x1300,
        source_state: 0x1400,
        capacity: 32,
    });
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let foreign = words(&[0x7000, 12]);
    let mut steps = qualified();
    steps.extend(read(0x1300, &words(&[0x9000, 32])));
    steps.extend(read(0x1400, &original));
    steps.extend(write(0x1400, &selected));
    steps.extend(read(0x1400, &foreign));
    steps.extend(read(0x1400, &foreign));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    assert!(block_on(mapping.prepare_preserving(&mut session)).is_err());
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn preserving_e1_uses_registered_capacity_instead_of_the_pattern_length() {
    for wide in [false, true] {
        let camera = if wide {
            zero_base_camera()
        } else {
            facts(BulkSource::Pattern {
                registered_buffer: 0x1300,
                source_state: 0x1400,
                capacity: 32,
            })
        };
        let original = if wide {
            words(&[0x9000, 0, 16])
        } else {
            words(&[0x9000, 16])
        };
        let selected = if wide {
            words(&[0, 0, 256])
        } else {
            words(&[0, 256])
        };
        let mut steps = qualified();
        steps.extend(read(
            0x1300,
            &if wide {
                words(&[0x9000, 0, 256])
            } else {
                words(&[0x9000, 256])
            },
        ));
        steps.extend(read(0x1400, &original));
        steps.extend(write(0x1400, &selected));
        steps.extend(read(0x1400, &selected));
        steps.extend(read(0x1000, b"stoc"));
        steps.extend(read(0x1400, &selected));
        steps.extend(data(0x1000, b"stoc"));
        steps.extend(read(0x1400, &selected));
        steps.extend(read(0x1400, &selected));
        steps.extend(data(0x1800, &[7; 64]));
        steps.extend(read(0x1400, &selected));
        steps.extend(read(0x1400, &selected));
        steps.extend(write(0x1400, &original));
        steps.extend(read(0x1400, &original));
        let (mut session, script) = opened(steps);
        block_on(async {
            let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
            mapping.prepare_preserving(&mut session).await.unwrap();
            assert_eq!(mapping.read(&mut session, target()).await.unwrap(), [7; 64]);
            mapping.restore(&mut session).await.unwrap();
        });
        assert_eq!(script.remaining(), 0, "one E1 for the whole range");
    }
}

#[test]
fn publication_variant_serializes_as_its_own_mechanism() {
    let camera = publication_camera(64);
    let json = serde_json::to_value(&camera).unwrap();
    assert_eq!(json["source"]["mechanism"], "publication");
    assert_eq!(json["source"]["source_state"], 0x1200);
    assert_eq!(json["source"]["capacity"], 64);
    assert_eq!(
        serde_json::from_value::<FirmwareBulk>(json.clone()).unwrap(),
        camera
    );
    let mut unknown = json.clone();
    unknown["source"]["pattern_capacity"] = 32.into();
    assert!(serde_json::from_value::<FirmwareBulk>(unknown).is_err());
    let mut missing = json;
    missing["source"]
        .as_object_mut()
        .unwrap()
        .remove("capacity");
    assert!(serde_json::from_value::<FirmwareBulk>(missing).is_err());
}

#[test]
fn publication_validation_refuses_cells_and_windows_outside_its_contract() {
    let camera = |source_state: u64, capacity: u32| {
        let mut camera = query_camera();
        camera.source = BulkSource::Publication {
            source_state,
            capacity,
        };
        camera
    };
    for (source_state, capacity) in [(0x1200, 0), (0x1202, 32), (0xffff_fffc, 32)] {
        let error = camera(source_state, capacity).validate().unwrap_err();
        assert!(error.contains("publication"), "{error}");
    }
    // The window must be wholly addressable through the 32-bit E1 offset.
    let mut beyond = camera(0x1200, 32);
    beyond.window = BulkRange {
        address: 0xffff_f000,
        size: 0x2000,
    };
    assert!(beyond.validate().unwrap_err().contains("publication"));
    // Overlap is judged on canonical addresses, so an aliased cell that covers
    // checked executable code is refused as well.
    assert!(
        camera(0x1000, 32)
            .validate()
            .unwrap_err()
            .contains("overlaps checked code")
    );
    let mut aliased = camera(0x4000_1000, 32);
    aliased.code_alias = 0x4000_0000;
    assert!(
        aliased
            .validate()
            .unwrap_err()
            .contains("overlaps checked code")
    );
    let mut aliased_code = camera(0x1000, 32);
    aliased_code.code_alias = 0x4000_0000;
    aliased_code.code_checks[0].address |= aliased_code.code_alias;
    assert!(
        aliased_code
            .validate()
            .unwrap_err()
            .contains("overlaps checked code")
    );
    let mut wide_alias = camera(0x1_0000_1200, 32);
    wide_alias.code_alias = 0x1_0000_0000;
    assert!(wide_alias.validate().unwrap_err().contains("publication"));
    // The pair itself may lie outside the target window.
    camera(0x3000, 4096).validate().unwrap();
    let mut mapped = camera(0x3000, 4096);
    mapped.mapped_probe = Some(CodeCheck {
        name: "mapped".into(),
        address: 0x3000,
        size: 4,
        sha256: hex(&sha256_bytes(b"code")),
    });
    assert!(
        mapped
            .validate()
            .unwrap_err()
            .contains("overlaps checked code")
    );
}

#[test]
fn publication_prepare_paths_borrow_the_pair_without_query_or_pattern_traffic() {
    block_on(async {
        for preserving in [false, true] {
            let camera = publication_camera(32);
            let original = words(&[0xdead_0000, 16]);
            let selected = words(&[0, 32]);
            let mut steps = qualified();
            steps.extend(read(0x1200, &original));
            steps.extend(write(0x1200, &selected));
            steps.extend(read(0x1200, &selected));
            steps.extend(read(0x1000, b"stoc"));
            steps.extend(read(0x1200, &selected));
            steps.extend(data(0x1000, b"stoc"));
            steps.extend(read(0x1200, &selected));
            steps.extend(read(0x1200, &selected));
            steps.extend(data(0x1800, &[5; 32]));
            steps.extend(read(0x1200, &selected));
            steps.extend(read(0x1200, &selected));
            steps.extend(data(0x1820, &[6; 32]));
            steps.extend(read(0x1200, &selected));
            steps.extend(read(0x1200, &selected));
            steps.extend(write(0x1200, &original));
            steps.extend(read(0x1200, &original));
            let (mut session, script) = opened(steps);
            let mut mapping = if preserving {
                let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
                mapping.prepare_preserving(&mut session).await.unwrap();
                mapping
            } else {
                BulkMapping::prepare(&mut session, &camera, limits())
                    .await
                    .unwrap()
            };
            assert_eq!(mapping.overwritten(), Some((0x1200, 8)));
            assert_eq!(
                mapping.read(&mut session, target()).await.unwrap(),
                [[5; 32], [6; 32]].concat()
            );
            mapping.restore(&mut session).await.unwrap();
            assert_eq!(
                script.remaining(),
                0,
                "only publication, memory and E1 traffic for preserving={preserving}"
            );
        }
    });
}

#[test]
fn publication_refuses_a_pair_that_already_holds_the_selected_source() {
    let camera = publication_camera(32);
    let mut steps = qualified();
    steps.extend(read(0x1200, &words(&[0, 32])));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    let error = block_on(mapping.prepare_preserving(&mut session))
        .err()
        .expect("an already selected pair is refused");
    assert!(error.to_string().contains("ownership"), "{error}");
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0, "nothing was written or read further");
}

#[test]
fn publication_capacity_is_clamped_to_the_reply_limit_and_splits_chunks() {
    let camera = publication_camera(256);
    let original = words(&[0xdead_0000, 16]);
    let selected = words(&[0, 128]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1800, &[1; 128]));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1880, &[2; 128]));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(write(0x1200, &original));
    steps.extend(read(0x1200, &original));
    let (mut session, script) = opened_with(128, steps);
    let target = BulkRange {
        address: 0x1800,
        size: 256,
    };
    block_on(async {
        let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
        mapping.prepare_preserving(&mut session).await.unwrap();
        assert_eq!(
            mapping.read(&mut session, target).await.unwrap(),
            [[1; 128], [2; 128]].concat()
        );
        mapping.restore(&mut session).await.unwrap();
    });
    assert_eq!(script.remaining(), 0);
}

#[test]
fn publication_write_and_readback_follow_memory_limits() {
    let camera = publication_camera(32);
    let original = words(&[0xdead_0000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = read(0x1200, &original);
    steps.extend(write(0x1200, &selected[..4]));
    steps.extend(write(0x1204, &selected[4..]));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(write(0x1200, &original[..4]));
    steps.extend(write(0x1204, &original[4..]));
    steps.extend(read(0x1200, &original));
    let (mut session, script) = opened(steps);
    let limits = Limits {
        memory: DebugMemoryLimits {
            read: NonZeroU32::new(8).unwrap(),
            write: NonZeroU32::new(4).unwrap(),
        },
        configuration: NonZeroU32::MIN,
    };
    block_on(async {
        let mut access = Access {
            session: &mut session,
            limits,
        };
        let mut source = Source::staged();
        source.borrow(&mut access, &camera).await.unwrap();
        assert_eq!(source.capacity(), Some(32));
        assert_eq!(source.overwritten, Some((0x1200, 8)));
        source.restore(&mut access).await.unwrap();
    });
    assert_eq!(script.remaining(), 0);
}

#[test]
fn publication_owner_change_between_chunks_sends_no_next_chunk_and_fences() {
    let camera = publication_camera(32);
    let original = words(&[0xdead_0000, 16]);
    let selected = words(&[0, 32]);
    let foreign = words(&[0x7000, 12]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1800, &[1; 32]));
    steps.extend(read(0x1200, &foreign));
    let (mut session, script) = opened(steps);
    block_on(async {
        let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
        mapping.prepare_preserving(&mut session).await.unwrap();
        let error = mapping.read(&mut session, target()).await.unwrap_err();
        assert!(error.to_string().contains("changed owner"), "{error}");
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        assert!(mapping.restore(&mut session).await.is_err());
    });
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0, "no second chunk and no overwrite");
}

#[test]
fn publication_owner_change_before_the_first_chunk_sends_no_bulk_data() {
    let camera = publication_camera(32);
    let original = words(&[0xdead_0000, 16]);
    let selected = words(&[0, 32]);
    let foreign = words(&[0x7000, 12]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &foreign));
    let (mut session, script) = opened(steps);
    block_on(async {
        let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
        mapping.prepare_preserving(&mut session).await.unwrap();
        assert!(mapping.read(&mut session, target()).await.is_err());
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        assert!(mapping.restore(&mut session).await.is_err());
    });
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0, "the range was never read");
}

#[test]
fn publication_read_loss_keeps_the_fence_and_sends_no_restore_traffic() {
    let camera = publication_camera(32);
    let original = words(&[0xdead_0000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend([
        E::Control(envelope(RequestKind::ReadData, 0x1800, 32, 0x1234)),
        E::ReadFailure {
            maximum: 32,
            error: nusb::transfer::TransferError::Disconnected,
            partial: vec![1; 4],
        },
    ]);
    let (mut session, script) = opened(steps);
    let error = block_on(read_bulk(&mut session, &camera, limits(), target())).unwrap_err();
    let BulkError::Cleanup { operation, cleanup } = &error else {
        panic!("expected a cleanup error, got {error}");
    };
    assert!(operation.contains("bulk read failed"), "{operation}");
    assert!(
        cleanup.to_string().contains("completion unknown"),
        "{cleanup}"
    );
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0, "no restore traffic after a fence");
}

#[test]
fn publication_readback_mismatch_never_overwrites_the_foreign_bytes() {
    let camera = publication_camera(32);
    let original = words(&[0xdead_0000, 16]);
    let selected = words(&[0, 32]);
    let foreign = words(&[0x7000, 12]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &foreign));
    steps.extend(read(0x1200, &foreign));
    let (mut session, script) = opened(steps);
    let error = block_on(read_bulk(&mut session, &camera, limits(), target())).unwrap_err();
    assert!(matches!(error, BulkError::Cleanup { .. }), "{error}");
    assert!(error.to_string().contains("read back"), "{error}");
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(script.remaining(), 0, "no write over the foreign bytes");
}

#[test]
fn publication_probe_mismatch_restores_the_pair_and_keeps_the_error() {
    let camera = publication_camera(32);
    let original = words(&[0xdead_0000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1000, b"oops"));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(write(0x1200, &original));
    steps.extend(read(0x1200, &original));
    let (mut session, script) = opened(steps);
    let error = block_on(read_bulk(&mut session, &camera, limits(), target())).unwrap_err();
    assert!(error.to_string().contains("disagree"), "{error}");
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0, "the pair was restored once");
}

#[test]
fn publication_invalid_metadata_stops_before_any_qualification_traffic() {
    block_on(async {
        for preserving in [false, true] {
            for source in [
                BulkSource::Publication {
                    source_state: 0x1200,
                    capacity: 0,
                },
                BulkSource::Publication {
                    source_state: 0x1202,
                    capacity: 32,
                },
                BulkSource::Publication {
                    source_state: 0x1000,
                    capacity: 32,
                },
            ] {
                let mut camera = publication_camera(32);
                camera.source = source;
                let (mut session, script) = opened(vec![]);
                let result = if preserving {
                    // Staged construction validates metadata before any session
                    // is touched, so no mapping can be prepared from bad facts.
                    BulkMapping::new(&camera, limits()).map(|_| ())
                } else {
                    BulkMapping::prepare(&mut session, &camera, limits())
                        .await
                        .map(|_| ())
                };
                assert!(result.is_err());
                assert_eq!(script.remaining(), 0);
                assert_eq!(session.state(), SessionState::Ready);
            }
        }
    });
}

#[test]
fn publication_reads_report_selected_live_words_and_restore_the_original() {
    let camera = publication_camera(32);
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(write(0x1200, &original));
    steps.extend(read(0x1200, &original));
    let (mut session, script) = opened(steps);
    let result = block_on(read_bulk(
        &mut session,
        &camera,
        limits(),
        BulkRange {
            address: 0x1200,
            size: 8,
        },
    ))
    .unwrap();
    assert_eq!(result.data, selected);
    assert_eq!(result.overwritten, Some((0x1200, 8)));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn publication_unverified_owner_read_fences_before_the_caller_restores() {
    let camera = publication_camera(32);
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    let mut failed = read(0x1200, &selected);
    failed.pop();
    failed.push(E::ReadFailure {
        maximum: 44,
        error: nusb::transfer::TransferError::Disconnected,
        partial: vec![],
    });
    steps.extend(failed);
    let (mut session, script) = opened(steps);
    block_on(async {
        let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
        mapping.prepare_preserving(&mut session).await.unwrap();
        assert!(mapping.read(&mut session, target()).await.is_err());
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        assert!(mapping.restore(&mut session).await.is_err());
    });
    assert_eq!(
        script.remaining(),
        0,
        "no E1 or restore after the unverified source read"
    );
}

#[test]
fn publication_misaligned_read_is_refused_before_ownership_or_data_traffic() {
    let mut camera = publication_camera(32);
    camera.dma = Some(BulkDma {
        address_alignment: 4,
        length_granule: 4,
    });
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(write(0x1200, &original));
    steps.extend(read(0x1200, &original));
    let (mut session, script) = opened(steps);
    block_on(async {
        let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
        mapping.prepare_preserving(&mut session).await.unwrap();
        assert!(
            mapping
                .read(
                    &mut session,
                    BulkRange {
                        address: 0x1801,
                        size: 4
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(session.state(), SessionState::Ready);
        mapping.restore(&mut session).await.unwrap();
    });
    assert_eq!(script.remaining(), 0);
}

// ---- staged loans: cancellation, retention and phase fences -----------------

fn publication_steps(original: &[u8], selected: &[u8]) -> Vec<E> {
    let mut steps = qualified();
    steps.extend(read(0x1200, original));
    steps.extend(write(0x1200, selected));
    steps.extend(read(0x1200, selected));
    // Qualification compares the slow probe with the same bytes through E1,
    // checking the borrowed pair around each piece.
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, selected));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(read(0x1200, selected));
    steps
}

fn pending(exchange: Vec<E>, maximum: usize) -> Vec<E> {
    let mut exchange = exchange;
    exchange.pop();
    exchange.push(E::PendingRead { maximum });
    exchange
}

#[test]
fn canceled_publication_setup_keeps_the_loan_and_fences_the_session() {
    let camera = publication_camera(32);
    let original = words(&[0xdead_0000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(pending(read(0x1200, &selected), 44));
    // Any later endpoint call would consume this deliberately unmatched canary.
    steps.push(E::Control([0xaa; 12]));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    {
        let mut future = Box::pin(mapping.prepare_preserving(&mut session));
        assert!(
            block_on(poll_once(future.as_mut())).is_none(),
            "the write completed but publication readback is pending"
        );
        drop(future);
    }
    assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(
        mapping.loan(),
        Some(BulkLoan {
            address: 0x1200,
            original: original.clone(),
            selected: Some(selected),
        }),
        "the bytes written before cancellation are still reported"
    );
    assert!(block_on(mapping.restore(&mut session)).is_err());
    assert_eq!(script.remaining(), 1, "a fence sends no restore traffic");
}

#[test]
fn canceled_query_setup_keeps_the_original_and_an_unknown_selection() {
    let camera = query_camera();
    let original = words(&[0xdead0000, 16]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(read(0x1100, &words(&[0x1500])));
    steps.extend(read(0x150c, &words(&[0x2000])));
    steps.extend(pending(diagnostic(0x450006, 0x80, 32), 32));
    // Any later endpoint call would consume this deliberately unmatched canary.
    steps.push(E::Control([0xaa; 12]));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    {
        let mut future = Box::pin(mapping.prepare_preserving(&mut session));
        assert!(block_on(poll_once(future.as_mut())).is_none());
        drop(future);
    }
    assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(
        mapping.loan(),
        Some(BulkLoan {
            address: 0x1200,
            original: original.clone(),
            selected: None,
        }),
        "the query's selected pair was never read back"
    );
    assert!(block_on(mapping.restore(&mut session)).is_err());
    assert_eq!(script.remaining(), 1);
}

#[test]
fn canceled_e1_read_keeps_the_loan_and_refuses_restoration() {
    let camera = publication_camera(32);
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = publication_steps(&original, &selected);
    steps.extend(read(0x1200, &selected));
    steps.extend(pending(data(0x1800, &[1; 32]), 32));
    // Any later endpoint call would consume this deliberately unmatched canary.
    steps.push(E::Control([0xaa; 12]));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    block_on(mapping.prepare_preserving(&mut session)).unwrap();
    assert_eq!(mapping.phase(), BulkMappingPhase::Ready);
    {
        let mut future = Box::pin(mapping.read(&mut session, target()));
        assert!(block_on(poll_once(future.as_mut())).is_none());
        drop(future);
    }
    assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(
        mapping.loan(),
        Some(BulkLoan {
            address: 0x1200,
            original,
            selected: Some(selected),
        })
    );
    assert!(block_on(mapping.restore(&mut session)).is_err());
    assert_eq!(script.remaining(), 1, "no chunk or restore after the drop");
}

#[test]
fn canceled_restore_keeps_the_loan_and_refuses_a_retry() {
    let camera = publication_camera(32);
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = publication_steps(&original, &selected);
    steps.extend(read(0x1200, &selected));
    steps.extend(write(0x1200, &original));
    steps.extend(pending(read(0x1200, &original), 44));
    // Any later endpoint call would consume this deliberately unmatched canary.
    steps.push(E::Control([0xaa; 12]));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    block_on(mapping.prepare_preserving(&mut session)).unwrap();
    {
        let mut future = Box::pin(mapping.restore(&mut session));
        assert!(block_on(poll_once(future.as_mut())).is_none());
        drop(future);
    }
    assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(
        mapping.loan(),
        Some(BulkLoan {
            address: 0x1200,
            original,
            selected: Some(selected),
        })
    );
    assert!(
        block_on(mapping.restore(&mut session)).is_err(),
        "an unresolved mapping never retries restoration"
    );
    assert_eq!(script.remaining(), 1, "the retry sent nothing");
}

#[test]
fn staged_mapping_reaches_restored_and_refuses_reads_after_close() {
    let camera = publication_camera(32);
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = publication_steps(&original, &selected);
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1800, &[7; 32]));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(write(0x1200, &original));
    steps.extend(read(0x1200, &original));
    let (mut session, script) = opened(steps);
    block_on(async {
        let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
        assert_eq!(mapping.phase(), BulkMappingPhase::Staged);
        mapping.prepare_preserving(&mut session).await.unwrap();
        assert_eq!(mapping.phase(), BulkMappingPhase::Ready);
        assert_eq!(
            mapping
                .read(
                    &mut session,
                    BulkRange {
                        address: 0x1800,
                        size: 32
                    }
                )
                .await
                .unwrap(),
            [7; 32]
        );
        mapping.restore(&mut session).await.unwrap();
        assert_eq!(mapping.phase(), BulkMappingPhase::Restored);
        mapping.restore(&mut session).await.unwrap();
        assert!(
            mapping.read(&mut session, target()).await.is_err(),
            "a restored mapping never reads"
        );
        assert_eq!(mapping.overwritten(), Some((0x1200, 8)));
        assert!(mapping.loan().is_some());
    });
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn staged_mapping_sends_no_traffic_before_preparation() {
    let camera = publication_camera(32);
    let (mut session, script) = opened(vec![]);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    assert_eq!(mapping.phase(), BulkMappingPhase::Staged);
    assert_eq!(mapping.loan(), None);
    assert!(block_on(mapping.read(&mut session, target())).is_err());
    assert!(block_on(mapping.restore(&mut session)).is_err());
    assert_eq!(mapping.phase(), BulkMappingPhase::Staged);
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn refused_setup_reports_a_closed_loan_only_when_the_cell_was_verified() {
    // Refused before any write: the cell still holds its original bytes.
    let camera = publication_camera(32);
    let mut steps = qualified();
    steps.extend(read(0x1200, &words(&[0, 32])));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    assert!(block_on(mapping.prepare_preserving(&mut session)).is_err());
    assert_eq!(mapping.phase(), BulkMappingPhase::Restored);
    assert_eq!(session.state(), SessionState::Ready);
    assert!(block_on(mapping.restore(&mut session)).is_ok());
    assert_eq!(script.remaining(), 0, "a closed loan restores nothing");

    // A foreign readback leaves the ownership unknown: no restoration, no
    // retry and no traffic.
    let camera = publication_camera(32);
    let original = words(&[0xdead_0000, 16]);
    let selected = words(&[0, 32]);
    let foreign = words(&[0x7000, 12]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &foreign));
    steps.extend(read(0x1200, &foreign));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    assert!(block_on(mapping.prepare_preserving(&mut session)).is_err());
    assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert!(mapping.loan().is_some());
    assert!(block_on(mapping.restore(&mut session)).is_err());
    assert_eq!(script.remaining(), 0);
}

#[test]
fn dropped_operation_fences_a_ready_session_without_an_exchange() {
    let camera = publication_camera(32);
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let (mut session, script) = opened(publication_steps(&original, &selected));
    let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
    block_on(mapping.prepare_preserving(&mut session)).unwrap();
    assert_eq!(session.state(), SessionState::Ready);
    // The per-exchange guard would not fire here: nothing is in flight.
    drop(Operation::new(&mut mapping, &mut session));
    assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert!(
        mapping.loan().is_some(),
        "the snapshot stays with the owner"
    );
    assert_eq!(script.remaining(), 0, "the guard sends no traffic");
}

#[test]
fn returned_owner_errors_mark_both_read_paths_unresolved_and_keep_the_loan() {
    for progress in [false, true] {
        let camera = publication_camera(32);
        let original = words(&[0x9000, 16]);
        let selected = words(&[0, 32]);
        let mut steps = publication_steps(&original, &selected);
        steps.extend(read(0x1200, &words(&[0x8000, 12])));
        steps.push(E::Control([0xaa; 12]));
        let (mut session, script) = opened(steps);
        let mut mapping = BulkMapping::new(&camera, limits()).unwrap();
        block_on(mapping.prepare_preserving(&mut session)).unwrap();
        let error = if progress {
            block_on(mapping.read_progress(&mut session, target(), &mut |_| Ok(())))
        } else {
            block_on(mapping.read(&mut session, target()))
        }
        .unwrap_err();
        assert!(error.to_string().contains("changed owner"));
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
        assert_eq!(mapping.loan().unwrap().original, original);
        assert!(block_on(mapping.restore(&mut session)).is_err());
        assert!(block_on(mapping.read(&mut session, target())).is_err());
        assert_eq!(
            script.remaining(),
            1,
            "no E1 or cleanup after ownership loss"
        );
    }
}

#[test]
fn returned_setup_transport_error_is_unresolved_even_before_a_loan_exists() {
    let mut steps = info(4, "X-M5");
    let E::Read { maximum, .. } = steps.pop().unwrap() else {
        panic!("info reply")
    };
    steps.push(E::ReadFailure {
        maximum,
        error: nusb::transfer::TransferError::Disconnected,
        partial: vec![],
    });
    steps.push(E::Control([0xaa; 12]));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&publication_camera(32), limits()).unwrap();
    assert!(block_on(mapping.prepare_preserving(&mut session)).is_err());
    assert_eq!(mapping.loan(), None);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
    assert!(block_on(mapping.restore(&mut session)).is_err());
    assert_eq!(
        script.remaining(),
        1,
        "an empty loan is not proof of completion"
    );
}

#[test]
fn failed_qualification_keeps_the_snapshot_after_known_successful_rollback() {
    let original = words(&[0x9000, 16]);
    let selected = words(&[0, 32]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(write(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(read(0x1200, &selected));
    steps.extend(data(0x1000, b"nope"));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &selected));
    steps.extend(write(0x1200, &original));
    steps.extend(read(0x1200, &original));
    steps.push(E::Control([0xaa; 12]));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&publication_camera(32), limits()).unwrap();
    let error = block_on(mapping.prepare_preserving(&mut session)).unwrap_err();
    assert!(error.to_string().contains("disagree"));
    assert_eq!(mapping.phase(), BulkMappingPhase::Restored);
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(mapping.loan().unwrap().original, original);
    block_on(mapping.restore(&mut session)).unwrap();
    assert!(block_on(mapping.read(&mut session, target())).is_err());
    assert_eq!(script.remaining(), 1, "known rollback does not run twice");
}

#[test]
fn query_setup_never_adopts_a_foreign_pair_after_the_checked_readback() {
    let original = words(&[0x9000, 16]);
    let selected = words(&[0x1000, 32]);
    let foreign = words(&[0x1800, 64]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(read(0x1100, &words(&[0x1500])));
    steps.extend(read(0x150c, &words(&[0x2000])));
    steps.extend(diagnostic(0x450006, 0x80, 32));
    steps.extend(read(0x1200, &selected));
    steps.extend(read(0x1200, &foreign));
    steps.push(E::Control([0xaa; 12]));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&query_camera(), limits()).unwrap();
    let error = block_on(mapping.prepare_preserving(&mut session)).unwrap_err();
    assert!(error.to_string().contains("changed owner"));
    assert_eq!(mapping.loan().unwrap().original, original);
    assert_eq!(mapping.loan().unwrap().selected, Some(selected));
    assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert!(block_on(mapping.restore(&mut session)).is_err());
    assert_eq!(
        script.remaining(),
        1,
        "foreign bytes never become restorable"
    );
}

#[test]
fn query_setup_preserves_its_primary_error_when_ownership_readback_also_fails() {
    let original = words(&[0x9000, 16]);
    let mut steps = qualified();
    steps.extend(read(0x1200, &original));
    steps.extend(read(0x1100, &words(&[0x1500])));
    steps.extend(read(0x150c, &words(&[0x2000])));
    steps.extend(diagnostic(0x450006, 0x80, 32));
    steps.extend(read(0x1200, &words(&[0x1000, 64])));
    let mut failed = read(0x1200, &original);
    failed.pop();
    failed.push(E::ReadFailure {
        maximum: 44,
        error: nusb::transfer::TransferError::Disconnected,
        partial: vec![],
    });
    steps.extend(failed);
    steps.push(E::Control([0xaa; 12]));
    let (mut session, script) = opened(steps);
    let mut mapping = BulkMapping::new(&query_camera(), limits()).unwrap();
    let error = block_on(mapping.prepare_preserving(&mut session)).unwrap_err();
    let BulkError::Cleanup { operation, cleanup } = error else {
        panic!("primary and cleanup")
    };
    assert!(operation.contains("capacity differs"));
    assert!(matches!(*cleanup, BulkError::Usb(_)));
    assert_eq!(mapping.loan().unwrap().original, original);
    assert_eq!(mapping.phase(), BulkMappingPhase::Unresolved);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert!(block_on(mapping.restore(&mut session)).is_err());
    assert_eq!(script.remaining(), 1);
}
