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
        update_image: FlashUpdateImage {
            dat_offset: 0,
            xor: 255,
        },
        reader: FlashReader {
            symbol: "flash_read".into(),
            alignment: 64,
            nand: None,
        },
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
    };
    (bulk, flash, payload, image)
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
    for (bad_control, stop_progress) in [(false, false), (true, false), (false, true)] {
        let (bulk, flash, payload, image) = fixture();
        payload.validate(&flash, &bulk, &image).unwrap();
        let original = [7; 12];
        let pattern: Vec<u8> = (0..32).collect();
        let original_scratch = [8; 32];
        let mut s = info(4, "X-M5");
        s.extend(info(8, "2.06"));
        s.extend(cfg(163, 1, false));
        s.extend(cfg(247, 0, false));
        s.extend(cfg(247, 1, true));
        s.extend(cfg(247, 1, false));
        s.extend(memory_read(0x1300, &words(&[0x100000, 0, 32])));
        s.extend(memory_read(0x1900, &0x2000u64.to_le_bytes()));
        s.extend(memory_read(0x100000, &original_scratch));
        s.extend(crate::bulk::tests::qualified());
        s.extend(memory_read(0x1300, &words(&[0x100000, 0, 32])));
        s.extend(memory_read(0x1400, &original));
        s.extend(diagnostic(0x550001, 0, 32));
        s.extend(memory_read(0x1400, &words(&[0x100000, 0, 32])));
        s.extend(data(0, &pattern));
        s.extend(write(0x1400, &[0; 8]));
        s.extend(memory_read(0x1400, &[0; 8]));
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
        s.extend(memory_read(0x4028, &[0; 8]));
        let order = if bad_control {
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
            s.extend(invoke(0x4080, 0));
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
        s.extend(memory_read(0x100000, &pattern));
        s.extend(write(0x100000, &original_scratch));
        s.extend(memory_read(0x100000, &original_scratch));
        s.extend(cfg(163, 1, false));
        s.extend(cfg(247, 1, false));
        s.extend(cfg(247, 0, true));
        s.extend(cfg(247, 0, false));
        let (mut session, script) = opened(s);
        let mut chunks = Vec::new();
        let mut progress = Vec::new();
        let result = block_on(backup(
            &mut session,
            &bulk,
            &flash,
            &payload,
            &image,
            limits(),
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
