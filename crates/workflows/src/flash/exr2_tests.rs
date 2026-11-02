use super::*;
use crate::bulk::tests::{data, diagnostic, limits, opened, qualified, query_camera, read, write};
use fujimod_usb::testing::Exchange as E;
use futures_lite::future::block_on;

fn setup() -> Vec<E> {
    let mut s = qualified();
    s.extend(read(0x1100, &words(&[0x1500])));
    s.extend(read(0x150c, &words(&[0x2000])));
    s.extend(diagnostic(0x450006, 0x80, 32));
    s.extend(read(0x1200, &words(&[0x1000, 32])));
    s.extend(read(0x1000, b"stoc"));
    s.extend(data(0, b"stoc"));
    s
}
fn checked_write(s: &mut Vec<E>, at: u32, bytes: &[u8]) {
    s.extend(write(at, bytes));
    s.extend(read(at, bytes));
}

fn configuration_read(offset: u32, value: u8) -> Vec<E> {
    use fujimod_usb::ff80::commands::configuration::{ConfigurationLocation, ReadConfiguration};
    let request = ReadConfiguration::new(
        ConfigurationLocation {
            view: ConfigurationView::Ram,
            page: 0,
            offset,
        },
        std::num::NonZeroU32::MIN,
        limits().configuration,
    )
    .unwrap()
    .encode();
    let mut reply = request[..36].to_vec();
    reply.push(value);
    crate::bulk::tests::exchange(request, reply)
}

#[test]
fn debug_access_accepts_both_reset_guard_values_without_writing_configuration() {
    for guard in [0, 1] {
        let mut steps = configuration_read(0xa3, guard);
        steps.extend(configuration_read(0xf7, 1));
        steps.extend(configuration_read(0xa3, guard));
        steps.extend(configuration_read(0xf7, 1));
        let (mut session, script) = opened(steps);
        let before = block_on(require_debug_access(&mut session, limits())).unwrap();
        assert_eq!(before, [guard, 1]);
        block_on(verify_configuration_flags(&mut session, limits(), before)).unwrap();
        assert_eq!(script.remaining(), 0);
    }
}

#[test]
fn unavailable_debug_and_changed_configuration_flags_are_rejected() {
    let mut steps = configuration_read(0xa3, 0);
    steps.extend(configuration_read(0xf7, 0));
    let (mut session, script) = opened(steps);
    assert!(
        block_on(require_debug_access(&mut session, limits()))
            .unwrap_err()
            .to_string()
            .contains("Existing debug access")
    );
    assert_eq!(script.remaining(), 0);
    for after in [[1, 1], [0, 0]] {
        let mut steps = configuration_read(0xa3, after[0]);
        steps.extend(configuration_read(0xf7, after[1]));
        let (mut session, script) = opened(steps);
        assert!(block_on(verify_configuration_flags(&mut session, limits(), [0, 1])).is_err());
        assert_eq!(script.remaining(), 0);
    }
}

#[test]
fn query_restores_the_qualified_source_after_a_completed_call() {
    let mut s = setup();
    let before = words(&[0x1000, 32]);
    s.extend(read(0x150c, &0x2000u32.to_le_bytes()));
    s.extend(read(0x1200, &before));
    checked_write(&mut s, 0x1200, &words(&[0, 0x3000]));
    checked_write(&mut s, 0x150c, &0x1800u32.to_le_bytes());
    s.extend(diagnostic(0x450006, 0x4000, 32));
    s.extend(read(0x150c, &0x1800u32.to_le_bytes()));
    s.extend(read(0x1200, &words(&[0x5000, 0x3000])));
    s.extend(read(0x150c, &0x1800u32.to_le_bytes()));
    checked_write(&mut s, 0x150c, &0x2000u32.to_le_bytes());
    checked_write(&mut s, 0x1200, &before);
    let (mut session, script) = opened(s);
    let mut reader = block_on(Qualified::prepare(&mut session, &query_camera(), limits())).unwrap();
    let query = Query {
        callback: 0x150c,
        stock: 0x2000,
        outputs: 0x1200,
    };
    assert_eq!(
        block_on(query.invoke(&mut reader, 0x1800, 0x4000, [0, 0x3000], false)).unwrap(),
        words(&[0x5000, 0x3000])
    );
    assert_eq!(reader.access.session.state(), SessionState::Ready);
    assert_eq!(script.remaining(), 0);
}

#[test]
fn unknown_completion_and_foreign_callback_withhold_all_cleanup() {
    for unknown in [false, true] {
        let mut s = setup();
        s.extend(read(0x150c, &0x2000u32.to_le_bytes()));
        s.extend(read(0x1200, &words(&[0x1000, 32])));
        checked_write(&mut s, 0x1200, &words(&[0, 0x3000]));
        checked_write(&mut s, 0x150c, &0x1800u32.to_le_bytes());
        let mut call = diagnostic(0x450006, 0x4000, 32);
        if unknown {
            call[3] = E::ReadFailure {
                maximum: 32,
                error: nusb::transfer::TransferError::Disconnected,
                partial: vec![],
            };
        }
        s.extend(call);
        if !unknown {
            s.extend(read(0x150c, &0x9999u32.to_le_bytes()));
            s.extend(read(0x150c, &0x9999u32.to_le_bytes()));
        }
        let (mut session, script) = opened(s);
        let mut reader =
            block_on(Qualified::prepare(&mut session, &query_camera(), limits())).unwrap();
        let query = Query {
            callback: 0x150c,
            stock: 0x2000,
            outputs: 0x1200,
        };
        assert!(block_on(query.invoke(&mut reader, 0x1800, 0x4000, [0, 0x3000], false)).is_err());
        assert_eq!(
            reader.access.session.state(),
            SessionState::CompletionUnknown
        );
        assert_eq!(script.remaining(), 0);
    }
}

#[test]
fn reader_metadata_rejects_corrupt_code_unbounded_ranges_and_unsupported_geometry() {
    let selected = fujimod_catalog::ReferenceCatalog::bundled()
        .unwrap()
        .resolve_firmware("x70-110")
        .unwrap();
    let flash = selected.flash.unwrap();
    let bulk = selected.bulk.unwrap();
    let image = [0u8; 64];
    let checks = flash
        .code_ranges
        .iter()
        .map(|r| CodeCheck {
            address: r.address,
            size: r.size,
            sha256: "a".repeat(64),
        })
        .chain([CodeCheck {
            address: 0x1000,
            size: 0x100,
            sha256: "b".repeat(64),
        }])
        .collect();
    let p = Payload {
        schema: 1,
        firmware_id: "x70-110".into(),
        firmware_sha256: "c".repeat(64),
        sha256: bulk::hex(&sha256_bytes(&image)),
        bootstrap_size: 28,
        reader_offset: 32,
        allocate: 0x1000,
        release: 0x1004,
        configuration: Configuration::Fixed {
            address: 0x42068000,
        },
        cache_clean: 0x1008,
        instruction_invalidate: 0x100c,
        barrier: 0x1010,
        instruction_barrier: 0x1014,
        checks,
        control_sha256: "d".repeat(64),
        update_size: flash.window.size,
    };
    p.validate(&flash, &bulk, &image).unwrap();
    assert!(p.validate(&flash, &bulk, &[1; 64]).is_err());
    let mut invalid = p.clone();
    invalid.bootstrap_size = 33;
    assert!(invalid.validate(&flash, &bulk, &image).is_err());
    let mut invalid = p.clone();
    invalid.checks[0].address = u64::MAX;
    assert!(invalid.validate(&flash, &bulk, &image).is_err());
    let mut invalid = flash.clone();
    invalid.reader.nand.as_mut().unwrap().controller_mode = 1;
    assert!(p.validate(&invalid, &bulk, &image).is_err());
}
