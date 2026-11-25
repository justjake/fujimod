use super::*;
use crate::bulk::tests::{
    data, info, limits, opened, qualified, read as memory_read, words, write,
};
use fujimod_catalog::capture::{
    FLASH_SCHEMA_URI, FlashControl, FlashDevice, FlashRange, FlashUpdateImage,
};
use fujimod_catalog_bulk::BulkSource;
use fujimod_usb::{ff80::SessionState, operations::checks::sha256_bytes};
use futures_lite::future::block_on;

fn flash(control: &[u8]) -> FirmwareFlash {
    FirmwareFlash {
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
            address: 0xf0000000,
            size: 0x08000000,
        },
        e1_window: Some(FlashRange {
            address: 0xf0000000,
            size: 0x08000000,
        }),
        update_image: FlashUpdateImage {
            dat_offset: 0,
            xor: 255,
        },
        reader: None,
        transport: None,
        code_ranges: Vec::new(),
        state_checks: Vec::new(),
        control: FlashControl {
            offset: 0x60000,
            size: control.len() as u64,
            sha256: hex(&sha256_bytes(control)),
        },
    }
}

#[test]
fn metadata_only_reader_checks_high_address_control_and_restores_source() {
    for bad_control in [false, true] {
        let mut bulk = crate::bulk::tests::zero_base_camera();
        let capacity = 0x10000;
        if let BulkSource::PatternZeroBase {
            capacity: expected, ..
        } = &mut bulk.source
        {
            *expected = capacity;
        } else {
            unreachable!("fixture must use a zero-base source");
        }
        let control = vec![5; 0x8000];
        let observed = vec![if bad_control { 4 } else { 5 }; 0x8000];
        let target = vec![6; capacity as usize];
        let original = [7; 12];
        let selected = words(&[0, 0, capacity]);
        let mut steps = info(4, "X-M5");
        steps.extend(info(8, "2.06"));
        steps.extend(qualified());
        steps.extend(memory_read(0x1300, &words(&[0x100000, 0, capacity])));
        steps.extend(memory_read(0x1400, &original));
        steps.extend(write(0x1400, &selected));
        steps.extend(memory_read(0x1400, &selected));
        steps.extend(memory_read(0x1000, b"stoc"));
        steps.extend(data(0x1000, b"stoc"));
        steps.extend(data(0xf0060000, &observed));
        if !bad_control {
            steps.extend(data(0xf0000000, &target));
        }
        steps.extend(memory_read(0x1400, &selected));
        steps.extend(write(0x1400, &original));
        steps.extend(memory_read(0x1400, &original));

        let (mut session, script) = opened(steps);
        let payload = super::super::Payload::MappedE1(super::super::MappedPayload {
            schema: 1,
            firmware_id: "sample-1".into(),
            firmware_sha256: "a".repeat(64),
        });
        let result = block_on(super::super::read_range(
            &mut session,
            &bulk,
            &flash(&control),
            &payload,
            &[],
            limits(),
            0,
            capacity,
        ));
        if bad_control {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("control bytes differ")
            );
        } else {
            assert_eq!(result.unwrap(), target);
        }
        assert_eq!(script.remaining(), 0);
        assert_eq!(session.state(), SessionState::Ready);
    }
}
