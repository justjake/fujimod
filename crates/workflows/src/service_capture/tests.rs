use super::*;
use fujimod_usb::{
    ff80::framing::RequestKind,
    testing::{Exchange as E, Script},
};
use futures_lite::future::block_on;

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

fn facts() -> Facts {
    Facts {
        format_version: 1,
        firmware_id: "sample-1".into(),
        firmware_sha256: "ab".repeat(32),
        normal_usb: Some(UsbIds {
            vendor_id: 17,
            product_id: 23,
        }),
        service_usb: Some(UsbIds {
            vendor_id: 17,
            product_id: 42,
        }),
        product: Some("Example".into()),
        internal_version: Some("2.5".into()),
        entry: Some(Entry {
            physical_keys: vec!["KEY_SAMPLE".into()],
            usb_condition: "USB present".into(),
        }),
        read_gate: Some(ReadGate {
            offset: 12,
            enabled_value: 1,
            ram_only: true,
        }),
        slow_read_bytes: Some(16),
        read_mode: ReadMode::Bulk,
        ram: Some(Region {
            address: 0x1000,
            size: 64,
        }),
        bulk: Some(Bulk {
            tag: 7,
            base: 0x1000,
            capacities: vec![64],
            wrapping_offsets: false,
            callback_pointer: 0x1010,
            callback_offset: 4,
            callback: 0x2000,
            source_state: 0x1020,
            probes: vec![Region {
                address: 0x1000,
                size: 4,
            }],
            mapped_probe: CodeCheck {
                region: Region {
                    address: 0x1000,
                    size: 8,
                },
                sha256: [1; 32],
            },
            chunk_bytes: 32,
        }),
        code_checks: vec![CodeCheck {
            region: Region {
                address: 0x1000,
                size: 8,
            },
            sha256: [1; 32],
        }],
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
    (block_on(script.begin_session(65536, None)).unwrap(), script)
}
fn exchange_max(request: Vec<u8>, reply: Vec<u8>, maximum: usize) -> Vec<E> {
    vec![
        E::Control(envelope(
            RequestKind::WriteCommand,
            request.len() as u32,
            0x1234,
        )),
        E::Write(request),
        E::Control(envelope(RequestKind::ReadCommand, maximum as u32, 0x1234)),
        E::Read {
            maximum,
            data: reply,
        },
    ]
}
fn exchange(request: Vec<u8>, reply: Vec<u8>) -> Vec<E> {
    let maximum = reply.len();
    exchange_max(request, reply, maximum)
}
fn info(selector: u32, text: &str) -> Vec<E> {
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
fn configuration(function: u32, value: u8) -> Vec<E> {
    let mut request: Vec<u8> = [0x1a0002, 0, 0, 0, function, 1, 0, 12, 1]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let mut reply = request.clone();
    if function == 3 {
        request.push(value);
    } else {
        reply.push(value);
    }
    exchange(request, reply)
}
fn memory(at: u32, bytes: &[u8]) -> Vec<E> {
    let request: Vec<u8> = [0x200001, 0, 0, 0, 2, 0, 0, at, bytes.len() as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let mut reply = request.clone();
    reply.extend(bytes);
    exchange(request, reply)
}
fn data(offset: u32, bytes: &[u8]) -> Vec<E> {
    vec![
        E::Control(data_envelope(
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
fn qualified_setup(enabled: bool) -> Vec<E> {
    let mut steps = info(4, "Example");
    steps.extend(info(8, "2.5"));
    steps.extend(configuration(2, u8::from(enabled)));
    if !enabled {
        steps.extend(configuration(3, 1));
        steps.extend(configuration(2, 1));
    }
    steps.extend(memory(0x1000, b"abcdefgh"));
    steps
}
fn executable_facts() -> Facts {
    let mut f = facts();
    let hash = checks::sha256_bytes(b"abcdefgh");
    f.code_checks[0].sha256 = hash;
    f.bulk.as_mut().unwrap().mapped_probe.sha256 = hash;
    f
}
fn bulk_setup(reply_capacity: u32) -> Vec<E> {
    let mut steps = qualified_setup(false);
    steps.extend(memory(0x1010, &0x1014u32.to_le_bytes()));
    steps.extend(memory(0x1018, &0x2000u32.to_le_bytes()));
    let mut request = vec![0; 32];
    request[..4].copy_from_slice(&0x450006u32.to_le_bytes());
    request[16..20].copy_from_slice(&7u32.to_le_bytes());
    let mut reply = request.clone();
    reply[28..32].copy_from_slice(&reply_capacity.to_le_bytes());
    steps.extend(exchange(request, reply));
    steps.extend(memory(
        0x1020,
        &[0x1000u32, 64]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    ));
    steps
}
#[test]
fn complete_bulk_capture_uses_one_volatile_write_and_no_exit_commands() {
    let plan = executable_facts().validate().unwrap();
    let mut steps = bulk_setup(64);
    steps.extend(memory(0x1000, b"abcd"));
    steps.extend(data(0, b"abcd"));
    steps.extend(data(0, b"abcdefgh"));
    steps.extend(data(0, &[b'X'; 32]));
    steps.extend(data(32, &[b'Y'; 32]));
    let (mut session, script) = opened(steps);
    let mut reader = block_on(plan.prepare(
        &mut session,
        &UsbIds {
            vendor_id: 17,
            product_id: 42,
        },
    ))
    .unwrap();
    assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), [b'X'; 32]);
    assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), [b'Y'; 32]);
    assert!(block_on(reader.next_chunk()).unwrap().is_none());
    assert_eq!(reader.bytes_completed(), 64);
    assert_eq!(script.remaining(), 0);
}
#[test]
fn contradictory_query_capacity_stops_before_any_bulk_read() {
    let plan = executable_facts().validate().unwrap();
    let (mut session, script) = opened(bulk_setup(63));
    assert!(matches!(
        block_on(plan.prepare(
            &mut session,
            &UsbIds {
                vendor_id: 17,
                product_id: 42
            }
        )),
        Err(Error::Verification("diagnostic source/capacity"))
    ));
    assert_eq!(script.remaining(), 0);
}
#[test]
fn explicit_slow_mode_never_sends_a_diagnostic_query_or_writes_an_enabled_gate() {
    let mut f = executable_facts();
    f.normal_usb = None;
    f.read_mode = ReadMode::Slow;
    f.bulk = None;
    // This explicit probe need not be a known whole-RAM map. Exercise both
    // a complete command and a short tail without normal-mode recognition.
    f.ram = Some(Region {
        address: 0x3000,
        size: 19,
    });
    let plan = f.validate().unwrap();
    let mut steps = qualified_setup(true);
    steps.extend(memory(0x3000, b"0123456789abcdef"));
    steps.extend(memory(0x3010, b"ghi"));
    let (mut session, script) = opened(steps);
    let mut reader = block_on(plan.prepare(
        &mut session,
        &UsbIds {
            vendor_id: 17,
            product_id: 42,
        },
    ))
    .unwrap();
    assert_eq!(
        block_on(reader.next_chunk()).unwrap().unwrap(),
        b"0123456789abcdef"
    );
    assert_eq!(reader.bytes_completed(), 16);
    assert_eq!(block_on(reader.next_chunk()).unwrap().unwrap(), b"ghi");
    assert_eq!(reader.bytes_completed(), 19);
    assert!(block_on(reader.next_chunk()).unwrap().is_none());
    assert_eq!(script.remaining(), 0);
}
#[test]
fn zero_capacity_is_never_treated_as_unbounded() {
    let mut f = facts();
    f.bulk.as_mut().unwrap().capacities = vec![0];
    assert!(f.validate().is_err());
}
#[test]
fn unexpected_gate_value_stops_before_code_or_probe_reads() {
    let plan = executable_facts().validate().unwrap();
    let mut steps = info(4, "Example");
    steps.extend(info(8, "2.5"));
    steps.extend(configuration(2, 7));
    let (mut session, script) = opened(steps);
    assert!(matches!(
        block_on(plan.prepare(
            &mut session,
            &UsbIds {
                vendor_id: 17,
                product_id: 42,
            }
        )),
        Err(Error::Verification("unexpected read gate value"))
    ));
    assert_eq!(script.remaining(), 0);
}
#[test]
fn failed_probe_preserves_only_the_completed_prefix() {
    let mut f = executable_facts();
    f.normal_usb = None;
    f.read_mode = ReadMode::Slow;
    f.ram = Some(Region {
        address: 0x3000,
        size: 20,
    });
    let plan = f.validate().unwrap();
    let mut steps = qualified_setup(true);
    steps.extend(memory(0x3000, b"0123456789abcdef"));
    let mut failed = memory(0x3010, b"tail");
    let Some(E::Read { data, .. }) = failed.last_mut() else {
        panic!("read fixture")
    };
    data[28] ^= 1; // Wrong address echo: payload is not verified RAM evidence.
    steps.extend(failed);
    let (mut session, script) = opened(steps);
    let mut reader = block_on(plan.prepare(
        &mut session,
        &UsbIds {
            vendor_id: 17,
            product_id: 42,
        },
    ))
    .unwrap();
    assert_eq!(
        block_on(reader.next_chunk()).unwrap().unwrap(),
        b"0123456789abcdef"
    );
    assert!(block_on(reader.next_chunk()).is_err());
    assert_eq!(reader.bytes_completed(), 16);
    assert_eq!(script.remaining(), 0);
}
#[test]
fn bad_code_stops_after_gate_without_storage_or_cleanup_writes() {
    let plan = facts().validate().unwrap();
    let (mut session, script) = opened(qualified_setup(false));
    assert!(matches!(
        block_on(plan.prepare(
            &mut session,
            &UsbIds {
                vendor_id: 17,
                product_id: 42
            }
        )),
        Err(Error::Verification("stock code"))
    ));
    assert_eq!(script.remaining(), 0);
}
#[test]
fn partial_facts_are_reviewable_but_cannot_execute() {
    let mut f = facts();
    f.normal_usb = None;
    f.read_mode = ReadMode::Slow;
    f.ram = None;
    assert!(!f.review().ready);
    assert_eq!(f.review().blockers, ["explicit bounded read range"]);
    assert!(f.review().steps.last().unwrap().contains("Remove battery"));
    assert!(f.validate().is_err());
}
#[test]
fn rejects_persistent_setter_bad_ranges_and_unknown_fields() {
    let mut f = facts();
    f.read_gate.as_mut().unwrap().ram_only = false;
    assert!(f.validate().is_err());
    let mut f = facts();
    f.bulk.as_mut().unwrap().probes[0].address = 0;
    assert!(f.validate().is_err());
    let mut f = facts();
    f.ram.as_mut().unwrap().address = u32::MAX;
    assert!(f.validate().is_err());
    let mut value = serde_json::to_value(facts()).unwrap();
    value["fallback_model"] = true.into();
    assert!(Facts::from_json(&value.to_string()).is_err());
}
#[test]
fn descriptor_mismatch_sends_no_commands() {
    let plan = facts().validate().unwrap();
    assert!(
        plan.recognize_normal(&UsbIds {
            vendor_id: 17,
            product_id: 23
        })
        .is_ok()
    );
    let (mut session, script) = opened(vec![]);
    assert!(matches!(
        block_on(plan.prepare(
            &mut session,
            &UsbIds {
                vendor_id: 17,
                product_id: 23
            }
        )),
        Err(Error::Identity(_))
    ));
    assert_eq!(script.remaining(), 0);
}
#[test]
fn wrong_product_or_firmware_never_reaches_the_gate() {
    for wrong_product in [true, false] {
        let plan = facts().validate().unwrap();
        let mut steps = info(4, if wrong_product { "Other" } else { "Example" });
        if !wrong_product {
            steps.extend(info(8, "9.9"));
        }
        let (mut session, script) = opened(steps);
        assert!(matches!(
            block_on(plan.prepare(
                &mut session,
                &UsbIds {
                    vendor_id: 17,
                    product_id: 42
                }
            )),
            Err(Error::Identity(_))
        ));
        assert_eq!(script.remaining(), 0);
    }
}
