use super::*;
#[test]
fn camera_only_chunks_come_first_but_hash_covers_flash_offset_order() {
    assert_eq!(
        pieces(0x40, 0x10, 0x28),
        vec![(0x20, 0x10), (0x30, 0x10), (0, 0x10), (0x10, 0x10)]
    );
    let image: Vec<_> = (0..0x40).collect();
    let mut digest = OrderedDigest::default();
    for (offset, n) in pieces(0x40, 0x10, 0x28) {
        digest.push(offset, &image[offset as usize..(offset + n) as usize]);
    }
    assert_eq!(digest.next, 0x40);
    assert!(digest.pending.is_empty());
    assert_eq!(&digest.hash.finalize()[..], &sha256_bytes(&image));
}
#[test]
fn arena_ownership_pins_code_and_tail_and_allows_partial_upload_cleanup() {
    let image = [1, 2, 3, 4, 5, 6, 7, 8];
    let mutable = [Range {
        address: 104,
        size: 4,
    }];
    assert!(callback::owns_image(
        &[1, 2, 3, 4, 99, 99, 99, 99, 0, 0],
        &image,
        10,
        &mutable,
        100,
        true
    ));
    assert!(!callback::owns_image(
        &[0, 2, 3, 4, 99, 99, 99, 99, 0, 0],
        &image,
        10,
        &mutable,
        100,
        true
    ));
    assert!(!callback::owns_image(
        &[1, 2, 3, 4, 99, 99, 99, 99, 0, 1],
        &image,
        10,
        &mutable,
        100,
        true
    ));
    assert!(callback::owns_image(
        &[1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
        &image,
        10,
        &mutable,
        100,
        false
    ));
}

use crate::bulk::tests::{
    data, diagnostic, exchange_max, info, limits, opened, read as memory_read, words, write,
};
use fujimod_catalog::capture::*;
use futures_lite::future::block_on;

fn fixture() -> (FirmwareBulk, FirmwareFlash, Payload, Vec<u8>) {
    let mut bulk = crate::bulk::tests::zero_base_camera();
    if let BulkSource::PatternZeroBase {
        ref mut configuration_pointer,
        ..
    } = bulk.source
    {
        *configuration_pointer = 0x1900;
    }
    bulk.window = BulkRange {
        address: 0x1000,
        size: 0x200000,
    };
    let flash = FirmwareFlash {
        schema_uri: FLASH_SCHEMA_URI.into(),
        schema: 1,
        id: "sample-1-flash".into(),
        firmware_id: "sample-1".into(),
        device: FlashDevice {
            kind: "serial-nand".into(),
            jedec_id: Some("abcdef".into()),
            part: Some("Test part".into()),
        },
        window: FlashRange {
            address: 0xf0000000,
            size: 8192,
        },
        e1_window: None,
        update_image: FlashUpdateImage {
            dat_offset: 0,
            xor: 255,
        },
        reader: Some(FlashReader {
            symbol: "flash_read".into(),
            alignment: 64,
            nand: None,
        }),
        transport: Some("xp5-cmd-callback".into()),
        code_ranges: vec![FlashCodeRange {
            name: "flash_read".into(),
            address: 0x1000,
            size: 10,
        }],
        state_checks: vec![FlashStateCheck {
            name: "driver".into(),
            address: 0x1600,
            size: 4,
            value: 1,
        }],
        control: FlashControl {
            offset: 0,
            size: 64,
            sha256: bulk::hex(&sha256_bytes(&[5; 64])),
        },
    };
    let mut image = vec![0; 0x1100];
    image[0x80..0x84].copy_from_slice(&[1; 4]);
    let payload = Payload {
        schema: 1,
        firmware_id: "sample-1".into(),
        firmware_sha256: "a".repeat(64),
        address: 0x4000,
        capacity: 0x2000,
        sha256: bulk::hex(&sha256_bytes(&image)),
        entry: 0x4080,
        request: 0x4000,
        result: 0x4018,
        buffer: 0x4100,
        buffer_capacity: 4096,
        stock_reader: 0x1000,
        stock_reader_symbol: "flash_read".into(),
        mutable: vec![
            Range {
                address: 0x4000,
                size: 64,
            },
            Range {
                address: 0x4100,
                size: 4096,
            },
        ],
        transport: Transport {
            callback: 0x1700,
            instruction_cache_sync: 0x1800,
            registered_scratch: Range {
                address: 0x100000,
                size: 32,
            },
            configuration_pointer: 0x1900,
            configuration_bytes: 256,
            guard_offset: 163,
            debug_offset: 247,
            checks: vec![CodeCheck {
                address: 0x1800,
                size: 16,
                sha256: bulk::hex(&sha256_bytes(&[0; 16])),
            }],
            words: vec![],
        },
        checks: vec![CodeCheck {
            address: 0x1000,
            size: 10,
            sha256: bulk::hex(&sha256_bytes(b"stock code")),
        }],
        control_sha256: bulk::hex(&sha256_bytes(&[5; 64])),
        update_size: 5000,
        staging: None,
    };
    (bulk, flash, payload, image)
}
/// The sample with reader ABI 2: 40-byte request, 56-byte result and one
/// flash chunk of staging RAM outside the executable reservation.
/// Exchanges of a staged backup up to the callback-slot read: identity, the
/// reset guard and debug gate, the qualified source and the code checks.
fn staged_prologue(original: [u8; 12]) -> Vec<fujimod_usb::testing::Exchange> {
    let mut s = info(4, "X-M5");
    s.extend(info(8, "2.06"));
    s.extend(cfg(163, 1, false));
    s.extend(cfg(247, 0, false));
    s.extend(cfg(247, 1, true));
    s.extend(cfg(247, 1, false));
    s.extend(memory_read(0x1300, &words(&[0x100000, 0, 32])));
    s.extend(memory_read(0x1900, &0x2000u64.to_le_bytes()));
    s.extend(crate::bulk::tests::qualified());
    s.extend(memory_read(0x1300, &words(&[0x100000, 0, 32])));
    s.extend(memory_read(0x1400, &original));
    s.extend(write(0x1400, &words(&[0, 0, 32])));
    s.extend(memory_read(0x1400, &words(&[0, 0, 32])));
    s.extend(memory_read(0x1000, b"stoc"));
    s.extend(data(0x1000, b"stoc"));
    s.extend(memory_read(0x1800, &[0; 16]));
    s.extend(memory_read(0x1000, b"stock code"));
    s.extend(memory_read(0x1600, &1u32.to_le_bytes()));
    s
}
/// Exchanges that restore the source state and the debug gate after a run.
fn staged_epilogue(original: [u8; 12]) -> Vec<fujimod_usb::testing::Exchange> {
    let mut s = memory_read(0x1400, &words(&[0, 0, 32]));
    s.extend(write(0x1400, &original));
    s.extend(memory_read(0x1400, &original));
    s.extend(cfg(163, 1, false));
    s.extend(cfg(247, 1, false));
    s.extend(cfg(247, 0, true));
    s.extend(cfg(247, 0, false));
    s
}
#[test]
fn interrupted_run_recovery_refuses_a_foreign_callback_or_executable() {
    for foreign_slot in [true, false] {
        let (bulk, flash, payload, image) = staged_fixture();
        let original = [7; 12];
        let mut s = staged_prologue(original);
        if foreign_slot {
            s.extend(memory_read(0x1700, &0x4100u64.to_le_bytes()));
        } else {
            let mut other = image.clone();
            other.resize(0x2000, 0);
            other[image.len()] = 1; // past the image: not this executable
            s.extend(memory_read(0x1700, &0x4080u64.to_le_bytes()));
            s.extend(bulk_chunks(0x4000, &other));
        }
        s.extend(staged_epilogue(original));
        let (mut session, script) = opened(s);
        let error = block_on(read(
            &mut session,
            &bulk,
            &flash,
            &payload,
            &image,
            limits(),
            crate::flash::Selection::Whole,
            &mut crate::flash::Callbacks {
                chunk: &mut |_, _| panic!("nothing is read"),
                progress: &mut |_, _| Ok(()),
            },
        ))
        .unwrap_err()
        .to_string();
        let expected = if foreign_slot {
            "Service callback is occupied"
        } else {
            "occupied by a different executable"
        };
        assert!(error.contains(expected), "{error}");
        assert_eq!(script.remaining(), 0, "the slot and arena stay untouched");
        assert_eq!(session.state(), SessionState::Ready);
    }
}
fn staged_fixture() -> (FirmwareBulk, FirmwareFlash, Payload, Vec<u8>) {
    let (bulk, flash, mut payload, image) = fixture();
    payload.result = 0x4040;
    payload.mutable[0].size = 0x80;
    payload.staging = Some(Range {
        address: 0x10000,
        size: 4096,
    });
    (bulk, flash, payload, image)
}
fn reader_words(values: &[u64]) -> Vec<u8> {
    values.iter().copied().flat_map(u64::to_le_bytes).collect()
}
#[test]
fn staging_must_be_aligned_inside_the_window_and_clear_of_owned_state() {
    let (bulk, flash, payload, image) = staged_fixture();
    payload.validate(&flash, &bulk, &image).unwrap();
    for (address, size) in [
        (0x10020, 4096),     // unaligned
        (0x10000, 0),        // empty
        (0x10000, 32),       // smaller than the control chunk
        (0x5000, 4096),      // inside the executable reservation
        (0x100000, 4096),    // over the registered scratch
        (0x1f0000, 0x20000), // past the bulk window
        (0x1700, 64),        // over the callback
    ] {
        let mut bad = payload.clone();
        bad.staging = Some(Range { address, size });
        assert!(
            bad.validate(&flash, &bulk, &image)
                .unwrap_err()
                .to_string()
                .contains("staging"),
            "{address:#x}+{size:#x} must be refused"
        );
    }
    let mut short = payload.clone();
    short.result = 0x4018;
    assert!(
        short.validate(&flash, &bulk, &image).is_err(),
        "ABI 2 objects overlap"
    );
}
#[test]
fn staged_backup_checks_staging_is_empty_reads_through_it_and_clears_it() {
    // leftover: an earlier session died after publishing this reader, leaving
    // its entry in the callback slot and its image in the arena. landed:
    // memory reads of the staged control chunk show the stock DMA's bytes.
    for (occupied, leftover, landed) in [
        (false, false, true),
        (true, false, true),
        (false, true, true),
        (false, false, false),
    ] {
        let (bulk, flash, payload, image) = staged_fixture();
        payload.validate(&flash, &bulk, &image).unwrap();
        let original = [7; 12];
        let mut s = staged_prologue(original);
        if leftover {
            // Proven ours: unpublish, then clear the arena as a restore would.
            let mut left = image.clone();
            left[0x40..0x78].fill(3); // the result object is mutable
            left.resize(0x2000, 0);
            s.extend(memory_read(0x1700, &0x4080u64.to_le_bytes()));
            s.extend(bulk_chunks(0x4000, &left));
            s.extend(write_verified(0x1700, &[0; 8]));
            s.extend(memory_read(0x1700, &[0; 8]));
            s.extend(bulk_chunks(0x4000, &left));
            for (i, piece) in vec![0; image.len()].chunks(64).enumerate() {
                s.extend(write(0x4000 + (i * 64) as u32, piece));
            }
            s.extend(raw_chunks(0x4000, &vec![0; image.len()]));
            s.extend(bulk_chunks(0x4000, &vec![0; 0x2000]));
        } else {
            s.extend(memory_read(0x1700, &[0; 8]));
        }
        s.extend(bulk_chunks(0x4000, &vec![0; 0x2000]));
        for (i, piece) in image.chunks(64).enumerate() {
            s.extend(write_verified(0x4000 + (i * 64) as u32, piece));
        }
        s.extend(bulk_chunks(0x4000, &image));
        s.extend(invoke(0x1800, 7));
        s.extend(memory_read(0x1700, &[0; 8]));
        s.extend(write_verified(0x1700, &0x4080u64.to_le_bytes()));
        s.extend(memory_read(0x4050, &[0; 8]));
        let mut observed = image.clone();
        let mut calls = 0;
        let mut call = |s: &mut Vec<_>, request: Vec<u8>, result: Vec<u8>| {
            s.extend(write_verified(0x4000, &request));
            s.extend(memory_read(0x1700, &0x4080u64.to_le_bytes()));
            s.extend(invoke(0x4080, 0)[12..16].iter().cloned());
            s.extend(memory_read(0x4040, &result));
            observed[..40].copy_from_slice(&request);
            observed[0x40..0x78].copy_from_slice(&result);
        };
        calls += 1;
        let (status, offset) = if occupied { (2, 0x18) } else { (0, 4096) };
        call(
            &mut s,
            reader_words(&[2, 0, 4096, 0x10000, 1]),
            reader_words(&[2, status, calls, offset, 4096, 5, 1000]),
        );
        if !occupied {
            for (offset, n) in [(0, 64), (4096, 4096), (0, 4096)] {
                calls += 1;
                call(
                    &mut s,
                    reader_words(&[2, 0xf0000000 + offset, n, 0x10000, 0]),
                    reader_words(&[2, 0, calls, 0xf0000000 + offset, n, 100, 1000]),
                );
                if calls == 2 {
                    // The control chunk is read back over memory reads first.
                    let byte = if landed { 5 } else { 0 };
                    s.extend(raw_chunks(0x10000, &vec![byte; n as usize]));
                    if !landed {
                        break; // refused before any bulk read
                    }
                }
                s.extend(bulk_chunks(0x10000, &vec![5; n as usize]));
            }
            calls += 1;
            call(
                &mut s,
                reader_words(&[2, 0, 4096, 0x10000, 2]),
                reader_words(&[2, 0, calls, 4096, 4096, 7, 1000]),
            );
        }
        s.extend(memory_read(0x1700, &0x4080u64.to_le_bytes()));
        s.extend(write_verified(0x1700, &[0; 8]));
        s.extend(memory_read(0x1700, &[0; 8]));
        let mut arena = observed;
        arena.resize(0x2000, 0);
        s.extend(bulk_chunks(0x4000, &arena));
        for (i, piece) in vec![0; image.len()].chunks(64).enumerate() {
            s.extend(write(0x4000 + (i * 64) as u32, piece));
        }
        s.extend(raw_chunks(0x4000, &vec![0; image.len()]));
        s.extend(bulk_chunks(0x4000, &vec![0; 0x2000]));
        s.extend(memory_read(0x1400, &words(&[0, 0, 32])));
        s.extend(write(0x1400, &original));
        s.extend(memory_read(0x1400, &original));
        s.extend(cfg(163, 1, false));
        s.extend(cfg(247, 1, false));
        s.extend(cfg(247, 0, true));
        s.extend(cfg(247, 0, false));
        let (mut session, script) = opened(s);
        let mut chunks = Vec::new();
        let result = block_on(read(
            &mut session,
            &bulk,
            &flash,
            &payload,
            &image,
            limits(),
            crate::flash::Selection::Whole,
            &mut crate::flash::Callbacks {
                chunk: &mut |o, d| {
                    chunks.push((o, d));
                    Ok(())
                },
                progress: &mut |_, _| Ok(()),
            },
        ));
        if !landed {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("did not land"), "{error}");
            assert!(chunks.is_empty());
        } else if occupied {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("staging range is not empty"), "{error}");
            assert!(error.contains("0x18"), "{error}");
            assert!(chunks.is_empty(), "nothing is read through occupied RAM");
        } else {
            let backup = result.unwrap();
            assert_eq!(
                chunks.iter().map(|(o, _)| *o).collect::<Vec<_>>(),
                vec![4096, 0]
            );
            assert_eq!(backup.sha256, bulk::hex(&sha256_bytes(&vec![5; 8192])));
            assert_eq!(backup.stock_read_seconds, Some(0.3));
            assert_eq!(backup.recovered_interrupted_run, leftover);
            assert_eq!(backup.mapped_probe, None, "no mapped window in this flash");
        }
        assert_eq!(script.remaining(), 0);
        assert_eq!(session.state(), SessionState::Ready);
    }
}
fn cfg(offset: u32, value: u8, writing: bool) -> Vec<fujimod_usb::testing::Exchange> {
    use fujimod_usb::ff80::commands::configuration::*;
    use std::num::NonZeroU32;
    let location = ConfigurationLocation {
        view: ConfigurationView::Ram,
        page: 0,
        offset,
    };
    let request = if writing {
        WriteConfiguration::new(location, vec![value], NonZeroU32::new(64).unwrap())
            .unwrap()
            .encode()
    } else {
        ReadConfiguration::new(location, NonZeroU32::MIN, NonZeroU32::new(64).unwrap())
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
fn write_verified(at: u32, bytes: &[u8]) -> Vec<fujimod_usb::testing::Exchange> {
    let mut s = write(at, bytes);
    s.extend(memory_read(at, bytes));
    s
}
fn raw_chunks(at: u32, bytes: &[u8]) -> Vec<fujimod_usb::testing::Exchange> {
    bytes
        .chunks(64)
        .enumerate()
        .flat_map(|(i, p)| memory_read(at + (i * 64) as u32, p))
        .collect()
}
fn bulk_chunks(at: u32, bytes: &[u8]) -> Vec<fujimod_usb::testing::Exchange> {
    bytes
        .chunks(32)
        .enumerate()
        .flat_map(|(i, p)| data(at + (i * 32) as u32, p))
        .collect()
}
fn invoke(entry: u64, status: u8) -> Vec<fujimod_usb::testing::Exchange> {
    let mut s = memory_read(0x1700, &[0; 8]);
    s.extend(write_verified(0x1700, &entry.to_le_bytes()));
    let mut request: Vec<u8> = [0x200001u32, 0, 0, 0, 5, 0, 0, 0, 10]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    request.extend(b"CMD PROBE\n");
    let mut reply = vec![0; 36];
    reply[..4].copy_from_slice(&0x200001u32.to_le_bytes());
    reply[4] = status;
    reply[16] = 5;
    s.extend(exchange_max(request, reply, 256));
    s.extend(memory_read(0x1700, &entry.to_le_bytes()));
    s.extend(write_verified(0x1700, &[0; 8]));
    s
}
#[test]
fn full_backup_restores_every_borrowed_byte_and_hashes_in_offset_order() {
    for (bad_control, stop_progress, range_only) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let (bulk, flash, payload, image) = fixture();
        payload.validate(&flash, &bulk, &image).unwrap();
        let original = [7; 12];
        let mut s = info(4, "X-M5");
        s.extend(info(8, "2.06"));
        s.extend(cfg(163, 1, false));
        s.extend(cfg(247, 0, false));
        s.extend(cfg(247, 1, true));
        s.extend(cfg(247, 1, false));
        s.extend(memory_read(0x1300, &words(&[0x100000, 0, 32])));
        s.extend(memory_read(0x1900, &0x2000u64.to_le_bytes()));
        s.extend(crate::bulk::tests::qualified());
        s.extend(memory_read(0x1300, &words(&[0x100000, 0, 32])));
        s.extend(memory_read(0x1400, &original));
        s.extend(write(0x1400, &words(&[0, 0, 32])));
        s.extend(memory_read(0x1400, &words(&[0, 0, 32])));
        s.extend(memory_read(0x1000, b"stoc"));
        s.extend(data(0x1000, b"stoc"));
        s.extend(memory_read(0x1800, &[0; 16]));
        s.extend(memory_read(0x1000, b"stock code"));
        s.extend(memory_read(0x1600, &1u32.to_le_bytes()));
        s.extend(memory_read(0x1700, &[0; 8]));
        s.extend(bulk_chunks(0x4000, &vec![0; 0x2000]));
        for (i, piece) in image.chunks(64).enumerate() {
            s.extend(write_verified(0x4000 + (i * 64) as u32, piece));
        }
        s.extend(bulk_chunks(0x4000, &image));
        s.extend(invoke(0x1800, 7));
        s.extend(memory_read(0x1700, &[0; 8]));
        s.extend(write_verified(0x1700, &0x4080u64.to_le_bytes()));
        s.extend(memory_read(0x4028, &[0; 8]));
        let order = if range_only {
            vec![(0, 64), (4096, 64)]
        } else if bad_control {
            vec![(0, 64)]
        } else if stop_progress {
            vec![(0, 64), (4096, 4096)]
        } else {
            vec![(0, 64), (4096, 4096), (0, 4096)]
        };
        let mut observed = image.clone();
        for (index, (offset, n)) in order.iter().copied().enumerate() {
            let request: Vec<u8> = [1, 0xf0000000 + offset, n]
                .into_iter()
                .flat_map(u64::to_le_bytes)
                .collect();
            s.extend(write_verified(0x4000, &request));
            let call = invoke(0x4080, 0);
            // Keep ownership check and execution, with publication/restoration once.
            s.extend(memory_read(0x1700, &0x4080u64.to_le_bytes()));
            s.extend(call[12..16].iter().cloned());
            let result: Vec<u8> = [1, 0, index as u64 + 1, 0xf0000000 + offset, n]
                .into_iter()
                .flat_map(u64::to_le_bytes)
                .collect();
            s.extend(memory_read(0x4018, &result));
            let bytes = vec![if bad_control { 6 } else { 5 }; n as usize];
            s.extend(bulk_chunks(0x4100, &bytes));
            observed[..24].copy_from_slice(&request);
            observed[24..64].copy_from_slice(&result);
            observed[0x100..0x100 + n as usize].copy_from_slice(&bytes);
        }
        s.extend(memory_read(0x1700, &0x4080u64.to_le_bytes()));
        s.extend(write_verified(0x1700, &[0; 8]));
        s.extend(memory_read(0x1700, &[0; 8]));
        let mut arena = observed;
        arena.resize(0x2000, 0);
        s.extend(bulk_chunks(0x4000, &arena));
        for (i, piece) in vec![0; image.len()].chunks(64).enumerate() {
            s.extend(write(0x4000 + (i * 64) as u32, piece));
        }
        s.extend(raw_chunks(0x4000, &vec![0; image.len()]));
        s.extend(bulk_chunks(0x4000, &vec![0; 0x2000]));
        s.extend(memory_read(0x1400, &words(&[0, 0, 32])));
        s.extend(write(0x1400, &original));
        s.extend(memory_read(0x1400, &original));
        s.extend(cfg(163, 1, false));
        s.extend(cfg(247, 1, false));
        s.extend(cfg(247, 0, true));
        s.extend(cfg(247, 0, false));
        let (mut session, script) = opened(s);
        if range_only {
            let bytes = block_on(crate::flash::read_range(
                &mut session,
                &bulk,
                &flash,
                &crate::flash::Payload::Xp5Command(payload.clone()),
                &image,
                limits(),
                4097,
                3,
            ))
            .unwrap();
            assert_eq!(bytes, [5; 3]);
            assert_eq!(
                script.remaining(),
                0,
                "only the control page and requested storage page are read"
            );
            assert_eq!(session.state(), SessionState::Ready);
            continue;
        }
        let mut chunks = Vec::new();
        let mut progress = Vec::new();
        let result = block_on(read(
            &mut session,
            &bulk,
            &flash,
            &payload,
            &image,
            limits(),
            crate::flash::Selection::Whole,
            &mut crate::flash::Callbacks {
                chunk: &mut |o, d| {
                    chunks.push((o, d));
                    Ok(())
                },
                progress: &mut |done, total| {
                    progress.push((done, total));
                    if stop_progress && done > 0 {
                        Err("progress callback failed".into())
                    } else {
                        Ok(())
                    }
                },
            },
        ));
        if bad_control {
            assert!(result.unwrap_err().to_string().contains("Control chunk"));
            assert!(chunks.is_empty());
            assert_eq!(progress, [(0, 8192)]);
        } else if stop_progress {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("progress callback failed")
            );
            assert_eq!(chunks.len(), 1);
            assert_eq!(progress, [(0, 8192), (4096, 8192)]);
        } else {
            let result = result.unwrap();
            assert_eq!(
                chunks.iter().map(|(o, _)| *o).collect::<Vec<_>>(),
                vec![4096, 0]
            );
            assert_eq!(result.sha256, bulk::hex(&sha256_bytes(&vec![5; 8192])));
            assert_eq!(progress, [(0, 8192), (4096, 8192), (8192, 8192)]);
        }
        assert_eq!(script.remaining(), 0);
        assert_eq!(session.state(), SessionState::Ready);
    }
}

#[test]
fn matching_control_reads_mapped_flash_in_registered_e1_capacity_chunks() {
    let (mut bulk, mut flash, mut payload, mut image) = fixture();
    let capacity: u32 = 0x1_0000;
    let control = vec![5; 0x8000];
    let first = vec![6; capacity as usize];
    let second = vec![7; capacity as usize];
    let control_sha256 = bulk::hex(&sha256_bytes(&control));
    if let BulkSource::PatternZeroBase {
        capacity: ref mut registered_capacity,
        ..
    } = bulk.source
    {
        *registered_capacity = capacity;
    } else {
        unreachable!("XP5 sample uses the zero-base E1 source");
    }
    flash.device = FlashDevice {
        kind: "serial-nor".into(),
        jedec_id: Some("2c5b1b".into()),
        part: Some("Test NOR".into()),
    };
    flash.window = FlashRange {
        address: 0xf0000000,
        size: 0x08000000,
    };
    flash.e1_window = Some(FlashRange {
        address: 0xf0000000,
        size: 0x08000000,
    });
    flash.control = FlashControl {
        offset: 0x000c0000,
        size: control.len() as u64,
        sha256: control_sha256.clone(),
    };
    payload.control_sha256 = control_sha256;
    payload.update_size = 0x00200000;
    payload.capacity = 0x10000;
    payload.buffer_capacity = 0x8000;
    payload.transport.registered_scratch.size = u64::from(capacity);
    let buffer = payload.buffer;
    payload
        .mutable
        .iter_mut()
        .find(|range| range.address == buffer)
        .unwrap()
        .size = u64::from(payload.buffer_capacity);
    image.resize(payload.capacity as usize, 0);
    payload.sha256 = bulk::hex(&sha256_bytes(&image));
    payload.validate(&flash, &bulk, &image).unwrap();

    let original = [7u8; 12];
    let selected = words(&[0, 0, capacity]);
    let mut steps = info(4, "X-M5");
    steps.extend(info(8, "2.06"));
    steps.extend(cfg(163, 1, false));
    steps.extend(cfg(247, 0, false));
    steps.extend(cfg(247, 1, true));
    steps.extend(cfg(247, 1, false));
    steps.extend(memory_read(0x1300, &words(&[0x100000, 0, capacity])));
    steps.extend(memory_read(0x1900, &0x2000u64.to_le_bytes()));
    steps.extend(crate::bulk::tests::qualified());
    steps.extend(memory_read(0x1300, &words(&[0x100000, 0, capacity])));
    steps.extend(memory_read(0x1400, &original));
    steps.extend(write(0x1400, &selected));
    steps.extend(memory_read(0x1400, &selected));
    steps.extend(memory_read(0x1000, b"stoc"));
    steps.extend(data(0x1000, b"stoc"));
    steps.extend(data(0xf00c0000, &control));
    steps.extend(data(0xf0100000, &first));
    steps.extend(data(0xf0110000, &second));
    steps.extend(memory_read(0x1400, &selected));
    steps.extend(write(0x1400, &original));
    steps.extend(memory_read(0x1400, &original));
    steps.extend(cfg(163, 1, false));
    steps.extend(cfg(247, 1, false));
    steps.extend(cfg(247, 0, true));
    steps.extend(cfg(247, 0, false));

    let (mut session, script) = opened(steps);
    let bytes = block_on(crate::flash::read_range(
        &mut session,
        &bulk,
        &flash,
        &crate::flash::Payload::Xp5Command(payload),
        &image,
        limits(),
        0x00100000,
        capacity * 2,
    ))
    .unwrap();
    assert_eq!(bytes, [first, second].concat());
    assert_eq!(script.remaining(), 0);
    assert_eq!(session.state(), SessionState::Ready);
}

#[test]
fn callback_unknown_completion_and_foreign_owner_withhold_cleanup() {
    use fujimod_usb::testing::Exchange as E;
    for unknown in [true, false] {
        let bulk = crate::bulk::tests::query_camera();
        let mut s = crate::bulk::tests::qualified();
        s.extend(memory_read(0x1100, &words(&[0x1500])));
        s.extend(memory_read(0x150c, &words(&[0x2000])));
        s.extend(diagnostic(0x450006, 0x80, 32));
        s.extend(memory_read(0x1200, &words(&[0x1000, 32])));
        s.extend(memory_read(0x1000, b"stoc"));
        s.extend(data(0, b"stoc"));
        let mut call = invoke(0x1800, 0);
        if unknown {
            call[15] = E::ReadFailure {
                maximum: 256,
                error: nusb::transfer::TransferError::Disconnected,
                partial: vec![],
            };
            call.truncate(16);
        } else {
            let E::Read { data, .. } = &mut call[19] else {
                panic!("expected callback readback")
            };
            let length = data.len();
            data[length - 8..].fill(9);
            call.truncate(20);
        }
        s.extend(call);
        let (mut session, script) = opened(s);
        let mut reader = block_on(Qualified::prepare(&mut session, &bulk, limits())).unwrap();
        assert!(block_on(callback::invoke(&mut reader, 0x1700, 0x1800, true)).is_err());
        assert_eq!(
            reader.access.session.state(),
            SessionState::CompletionUnknown
        );
        assert_eq!(script.remaining(), 0);
    }
}

#[test]
fn file_ranges_round_to_driver_pages_without_crossing_storage_or_including_other_pages() {
    let (_, flash, _, _) = fixture();
    assert_eq!(
        crate::flash::enclosing(&flash, 4097, 3).unwrap(),
        BulkRange {
            address: 4096,
            size: 64
        }
    );
    assert_eq!(
        crate::flash::enclosing(&flash, 63, 2).unwrap(),
        BulkRange {
            address: 0,
            size: 128
        }
    );
    assert_eq!(
        crate::flash::enclosing(&flash, 8191, 1).unwrap(),
        BulkRange {
            address: 8128,
            size: 64
        }
    );
    for (offset, size) in [(8192, 1), (8191, 2), (0, 0), (u64::MAX, 1)] {
        assert!(crate::flash::enclosing(&flash, offset, size).is_err());
    }
}
