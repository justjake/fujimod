use super::read as read_bulk;
use super::*;
use fujimod_catalog_bulk::{BULK_SCHEMA_URI, BulkDma, CaptureScope, CodeCheck, NamedRange};
use fujimod_usb::{
    ff80::{commands::memory::DebugMemoryLimits, framing::RequestKind},
    operations::checks::sha256_bytes,
    testing::{Exchange as E, Script},
};
use futures_lite::future::block_on;

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
    let mut all = vec![
        E::Control(envelope(RequestKind::Open, 0, 8, 0)),
        E::Read {
            maximum: 8,
            data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
        },
    ];
    all.extend(steps);
    let script = Script::new(all);
    (block_on(script.begin_session(65536, None)).unwrap(), script)
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
    let mut mapping = BulkMapping {
        source: Source {
            window: DataWindow::new(
                Address(0x1000),
                memory_range(window).unwrap(),
                NonZeroU32::new(32).unwrap(),
                OffsetMode::Wrapping,
            )
            .unwrap()
            .with_alignment(DataAlignment::new(32, 32).unwrap()),
            overwritten: None,
            saved: None,
        },
        window,
        limits: limits(),
        dma: Some(BulkDma {
            address_alignment: 32,
            length_granule: 32,
        }),
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
    let mut steps = qualified();
    steps.extend(read(0x1300, &words(&[0x10_0000, 0, 64])));
    steps.extend(read(0x1400, &original));
    steps.extend(write(0x1400, &words(&[0, 0, 64])));
    steps.extend(read(0x1400, &words(&[0, 0, 64])));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(data(0x1800, &[[3; 32], [4; 32]].concat()));
    steps.extend(read(0x1400, &words(&[0, 0, 64])));
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
    let mut steps = qualified();
    steps.extend(read(0x1300, &words(&[0x10_0000, 0, 64])));
    steps.extend(read(0x1400, &original));
    steps.extend(write(0x1400, &words(&[0, 0, 64])));
    steps.extend(read(0x1400, &words(&[0, 0, 64])));
    steps.extend(read(0x1000, b"stoc"));
    steps.extend(data(0x1000, b"oops"));
    steps.extend(read(0x1400, &words(&[0, 0, 64])));
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
            steps.extend(data(0x1000 - base, b"stoc"));
            steps.extend(data(0x1800 - base, &[7; 32]));
            steps.extend(data(0x1820 - base, &[8; 32]));
            steps.extend(read(state, &selected));
            steps.extend(write(state, &original));
            steps.extend(read(state, &original));
            let (mut session, script) = opened(steps);
            let mapping = BulkMapping::prepare_preserving(&mut session, &camera, limits())
                .await
                .unwrap();
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
        steps.extend(data(0, b"stoc"));
        steps.extend(read(0x1200, &selected));
        steps.extend(write(0x1200, &original));
        steps.extend(read(0x1200, &original));
        let (mut session, script) = opened(steps);
        let mapping = BulkMapping::prepare_preserving(&mut session, &camera, limits())
            .await
            .unwrap();
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
    steps.extend(data(0x1000, b"oops"));
    steps.extend(read(0x1400, &selected));
    steps.extend(write(0x1400, &original));
    steps.extend(read(0x1400, &original));
    let (mut session, script) = opened(steps);
    assert!(
        block_on(BulkMapping::prepare_preserving(
            &mut session,
            &camera,
            limits()
        ))
        .is_err()
    );
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
    assert!(
        block_on(BulkMapping::prepare_preserving(
            &mut session,
            &camera,
            limits()
        ))
        .is_err()
    );
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
        steps.extend(data(0x1000, b"stoc"));
        steps.extend(data(0x1800, &[7; 64]));
        steps.extend(read(0x1400, &selected));
        steps.extend(write(0x1400, &original));
        steps.extend(read(0x1400, &original));
        let (mut session, script) = opened(steps);
        block_on(async {
            let mapping = BulkMapping::prepare_preserving(&mut session, &camera, limits())
                .await
                .unwrap();
            assert_eq!(mapping.read(&mut session, target()).await.unwrap(), [7; 64]);
            mapping.restore(&mut session).await.unwrap();
        });
        assert_eq!(script.remaining(), 0, "one E1 for the whole range");
    }
}
