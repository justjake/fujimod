use super::*;
use crate::bulk::tests::{limits, opened, read as memory_read};
use futures_lite::future::block_on;

const ALIAS: u64 = 0x4000_0000;
const IMAGE: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

fn payload(alias: Option<u64>) -> Payload {
    Payload {
        schema: 1,
        firmware_id: "sample-1".into(),
        firmware_sha256: "0".repeat(64),
        address: 0x1000,
        capacity: 0x100,
        sha256: String::new(),
        entry: 0x1000,
        request: 0x1040,
        result: 0x1050,
        buffer_capacity: 0x80000,
        stock_reader: 0x8000,
        stock_reader_symbol: "flash_read_wait".into(),
        mutable: Vec::new(),
        transport: Callback {
            address: 0x2000,
            original: 0,
            checks: Vec::new(),
        },
        checks: Vec::new(),
        control_sha256: String::new(),
        update_size: 0,
        uncached_alias: alias,
        cache_churn: alias.map(|_| Range {
            address: 0x8000,
            size: 0x80,
        }),
    }
}

fn alias_read(bytes: &[u8]) -> Vec<fujimod_usb::testing::Exchange> {
    memory_read((ALIAS + 0x1000) as u32, bytes)
}

fn churn() -> Vec<fujimod_usb::testing::Exchange> {
    let mut steps = memory_read(0x8000, &[0; 64]);
    steps.extend(memory_read(0x8040, &[0; 64]));
    steps
}

fn run(steps: Vec<fujimod_usb::testing::Exchange>, alias: Option<u64>) -> bulk::Result<()> {
    let (mut session, script) = opened(steps);
    let result = block_on(require_published(
        &mut bulk::Access {
            session: &mut session,
            limits: limits(),
        },
        &payload(alias),
        &IMAGE,
    ));
    assert_eq!(script.remaining(), 0, "every scripted read is consumed");
    result
}

#[test]
fn published_image_is_accepted_on_the_first_alias_read() {
    run(alias_read(&IMAGE), Some(ALIAS)).unwrap();
}

#[test]
fn stale_alias_read_evicts_with_stock_code_reads_and_checks_again() {
    let mut steps = alias_read(&[0; 8]);
    steps.extend(churn());
    steps.extend(alias_read(&IMAGE));
    run(steps, Some(ALIAS)).unwrap();
}

#[test]
fn image_that_never_reaches_memory_is_refused_before_publication() {
    let mut steps = Vec::new();
    for round in 0..=CACHE_CHURN_ROUNDS {
        steps.extend(alias_read(&[0; 8]));
        if round < CACHE_CHURN_ROUNDS {
            steps.extend(churn());
        }
    }
    let error = run(steps, Some(ALIAS)).unwrap_err().to_string();
    assert!(error.contains("never reached memory"), "{error}");
}

#[test]
fn payload_without_an_alias_sends_no_reads() {
    run(Vec::new(), None).unwrap();
}
