use super::*;
use crate::bulk::tests::*;
use fujimod_catalog::capture::STATE_SCHEMA_URI;
use futures_lite::future::block_on;

fn state(constant: bool) -> FirmwareState {
    FirmwareState {
        schema_uri: STATE_SCHEMA_URI.into(),
        schema: 1,
        id: "sample-1-state".into(),
        firmware_id: "sample-1".into(),
        regions: vec![StateRegion {
            name: "boot-rom".into(),
            address: 0x3000,
            size: 4,
            constant,
        }],
    }
}
fn setup() -> Vec<fujimod_usb::testing::Exchange> {
    let mut s = qualified();
    s.extend(read(0x1100, &words(&[0x1500])));
    s.extend(read(0x150c, &words(&[0x2000])));
    s.extend(diagnostic(0x450006, 0x80, 32));
    s.extend(read(0x1200, &words(&[0x1000, 32])));
    s.extend(read(0x1000, b"stoc"));
    s.extend(data(0, b"stoc"));
    s
}
#[test]
fn catalog_constant_outside_window_is_read_once_and_delivered() {
    // Outside the E1 window the region uses direct memory reads. The
    // double-read verification is skipped (ff80 reads may differ between
    // passes on some processors), so only one read exchange is needed.
    let mut s = setup();
    s.extend(read(0x3000, b"ROM!"));
    let (mut session, script) = opened(s);
    let mut captured = Vec::new();
    let mut progress = Vec::new();
    block_on(dump(
        &mut session,
        &query_camera(),
        &state(true),
        limits(),
        &mut |r| {
            captured.push(r);
            Ok(())
        },
        &mut |done, total| {
            progress.push((done, total));
            Ok(())
        },
    ))
    .unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].data, b"ROM!");
    assert_eq!(captured[0].sha256, crate::bulk::hex(&sha256_bytes(b"ROM!")));
    assert_eq!(script.remaining(), 0);
    assert_eq!(progress, [(0, 8), (4, 8), (8, 8)]);
}
#[test]
fn masked_constants_outside_window_are_omitted() {
    // Constant regions outside the E1 window skip the double-read check,
    // but all-zero / all-FF masking still applies.
    for bytes in [[0; 4], [255; 4]] {
        let mut s = setup();
        s.extend(read(0x3000, &bytes));
        let (mut session, script) = opened(s);
        let result = block_on(dump(
            &mut session,
            &query_camera(),
            &state(true),
            limits(),
            &mut |_| panic!("masked constant delivered"),
            &mut |_, _| Ok(()),
        ));
        assert!(result.is_ok());
        assert_eq!(script.remaining(), 0);
    }
}
#[test]
fn mutable_regions_are_read_once_and_sink_failure_stops() {
    let mut s = setup();
    s.extend(read(0x3000, &[0; 4]));
    let (mut session, script) = opened(s);
    let result = block_on(dump(
        &mut session,
        &query_camera(),
        &state(false),
        limits(),
        &mut |r| {
            assert_eq!(r.data, [0; 4]);
            Err("download failed".into())
        },
        &mut |_, _| Ok(()),
    ));
    assert!(result.unwrap_err().to_string().contains("download failed"));
    assert_eq!(script.remaining(), 0);
}

#[test]
fn byte_progress_arrives_before_region_delivery_and_failure_stops_at_the_next_chunk() {
    let mut s = setup();
    s.extend(read(0x3000, &[0; 64]));
    let (mut session, script) = opened(s);
    let mut plan = state(false);
    plan.regions[0].size = 128;
    let mut progress = Vec::new();
    let result = block_on(dump(
        &mut session,
        &query_camera(),
        &plan,
        limits(),
        &mut |_| panic!("partial region delivered"),
        &mut |done, total| {
            progress.push((done, total));
            if done == 64 {
                Err("progress callback failed".into())
            } else {
                Ok(())
            }
        },
    ));
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("progress callback failed")
    );
    assert_eq!(progress, [(0, 128), (64, 128)]);
    assert_eq!(script.remaining(), 0);
    assert_eq!(session.state(), fujimod_usb::ff80::SessionState::Ready);
}

#[test]
fn region_spanning_window_boundary_zero_fills_prefix_and_reads_bulk_middle() {
    // Window is [0x1000, 0x2000). Region [0x0FC0, 0x1040) straddles the
    // boundary: 64 bytes below the window are zero-filled (not read, since
    // addresses below the E1 window may be unreadable), 64 bytes inside
    // the window are read via E1.
    let mut s = setup();
    s.extend(data(0, &[0xBB; 32]));
    s.extend(data(0x20, &[0xCC; 32]));
    let (mut session, script) = opened(s);
    let mut plan = state(false);
    plan.regions[0] = StateRegion {
        name: "spanning".into(),
        address: 0x0FC0,
        size: 128,
        constant: false,
    };
    let mut captured = Vec::new();
    let mut progress = Vec::new();
    block_on(dump(
        &mut session,
        &query_camera(),
        &plan,
        limits(),
        &mut |r| {
            captured.push(r);
            Ok(())
        },
        &mut |done, total| {
            progress.push((done, total));
            Ok(())
        },
    ))
    .unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].data.len(), 128);
    assert_eq!(&captured[0].data[..64], &[0x00; 64]);
    assert_eq!(&captured[0].data[64..96], &[0xBB; 32]);
    assert_eq!(&captured[0].data[96..128], &[0xCC; 32]);
    assert_eq!(script.remaining(), 0);
}
