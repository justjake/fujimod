use crate::transfer::ImageTransfer;
use fujimod_emulator_devices::{Device, exr2::irq_routes::Irq, test_host::*};
fn configured(h: &mut TestHost, destination: u32, pitch: u32) -> ImageTransfer {
    let mut d = ImageTransfer::new(Irq { core: 0, line: 73 });
    h.ram[0x8000..0x8200].copy_from_slice(&(0..512).map(|n| (n % 251) as u8).collect::<Vec<_>>());
    h.ram[0x9000..0x9200].fill(0xa5);
    for (o, v) in [
        (0, 0x10020),
        (0xc, pitch),
        (0x10, 4 << 16 | 64),
        (0x18, 0),
        (0x1c, 0),
        (4, 0x8000),
        (8, destination),
    ] {
        write(&mut d, h, 0xfff64000 + o, v);
    }
    d
}
#[test]
fn conversion_latches_i420_planes_and_commits_packed_receipt_dma_before_status_and_irq() {
    let mut h = TestHost::new();
    let mut d = configured(&mut h, 0x9000, 64);
    let source = h.ram[0x8000..0x8180].to_vec();
    let expected = fujimod_emulator_image_processing::yuv::i420_to_yycbcr(64, 4, &source).unwrap();
    write(&mut d, &mut h, 0xfff64000, 0x10121);
    assert!(d.save()["last_output"].is_null());
    assert_eq!(d.save()["transfers"], 0);
    let deadline = d.next_deadline(0).unwrap();
    assert_eq!(deadline, 2);
    h.ram[0x8000..0x8200].fill(0);
    advance(&mut d, &mut h, 1);
    assert_eq!(h.ram[0x9000], 0xa5);
    assert!(h.irqs.is_empty());
    advance_from_checkpoint(&mut d, &mut h, deadline);
    assert_eq!(&h.ram[0x9000..0x9200], expected.as_slice());
    assert!(h.irqs.is_empty());
    assert_eq!(read(&mut d, &mut h, 0xfff64014), 1);
    let receipt = d.save()["last_output"].clone();
    assert_eq!(receipt["source"], 0x8000);
    assert_eq!(receipt["destination"], 0x9000);
    assert_eq!(receipt["width"], 64);
    assert_eq!(receipt["height"], 4);
    assert_eq!(receipt["pitch"], 128);
    assert_eq!(receipt["bytes"], 512);
    assert_eq!(receipt["completed_tick"], 2);
    assert_eq!(receipt["packing"], "yycbcr");
    assert_ne!(receipt["source_sha256"], receipt["output_sha256"]);
    assert_eq!(
        receipt["source_sha256"],
        fujimod_emulator_devices::fingerprint::bytes(&source)["sha256"]
    );
    assert_eq!(receipt["source_packing"], "i420");
    assert_eq!(receipt["source_pitch"], 64);
    assert_eq!(receipt["source_bytes"], 384);
    assert_eq!(
        receipt["output_sha256"],
        fujimod_emulator_devices::fingerprint::bytes(&expected)["sha256"]
    );
    write(&mut d, &mut h, 0xfff6401c, 0x31);
    assert_eq!(h.irqs, [(0, 73)]);
    write(&mut d, &mut h, 0xfff64018, 0x20);
    assert!(h.levels[&(0, 73)]);
    write(&mut d, &mut h, 0xfff64018, 1);
    assert!(!h.levels[&(0, 73)]);
    write(&mut d, &mut h, 0xfff64000, 0x10121);
    assert_eq!(d.save()["last_output"], receipt);
    assert_eq!(d.save()["transfers"], 1);
    write(&mut d, &mut h, 0xfff64000, 0);
    advance(&mut d, &mut h, 3);
    assert_eq!(read(&mut d, &mut h, 0xfff64000), 0);
    advance_from_checkpoint(&mut d, &mut h, 4);
    assert_eq!(read(&mut d, &mut h, 0xfff64000), 0x10000);
    assert_eq!(d.save()["transfers"], 1);
    assert_eq!(d.next_deadline(4), None);
}
#[test]
fn unsupported_copy_layout_and_overlapping_dma_refuse_without_output() {
    for (destination, pitch, command, reason) in [
        (0x9000, 66, 0x10121, "tightly packed"),
        (0x8100, 64, 0x10121, "overlapping"),
        (0x9000, 64, 0x10101, "aligned"),
    ] {
        let mut h = TestHost::new();
        let mut d = configured(&mut h, destination, pitch);
        let ram = h.ram.clone();
        {
            let mut cx = h.cx();
            d.write(&mut cx, 0xfff64000, 4, command);
            assert!(cx.refusal().unwrap().contains(reason));
        }
        assert_eq!(h.ram, ram);
        assert!(h.irqs.is_empty());
        assert_eq!(d.next_deadline(0), None);
        assert_eq!(d.save()["transfers"], 0);
        assert!(d.save()["last_output"].is_null());
    }
}
#[test]
fn copy_checkpoint_rejects_changed_payload_and_busy_reconfiguration() {
    let mut h = TestHost::new();
    let mut d = configured(&mut h, 0x9000, 64);
    write(&mut d, &mut h, 0xfff64000, 0x10121);
    let saved = d.checkpoint().unwrap();
    let bad = saved
        .clone()
        .with_blobs(vec![vec![0u8; 512].into(), saved.blobs()[1].clone()]);
    assert!(
        d.validate_checkpoint(&bad)
            .unwrap_err()
            .contains("payload differs")
    );
    // A matching source digest alone cannot authorize unrelated output pixels.
    let mut source = saved.blobs()[1].to_vec();
    source[0] ^= 1;
    let mut metadata: serde_json::Value = saved.decode(d.name()).unwrap();
    metadata["pending"]["source_sha256"] =
        fujimod_emulator_devices::fingerprint::bytes(&source)["sha256"].clone();
    let inconsistent =
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(d.name(), &metadata)
            .unwrap()
            .with_blobs(vec![saved.blobs()[0].clone(), source.into()]);
    assert!(
        d.validate_checkpoint(&inconsistent)
            .unwrap_err()
            .contains("conversion differs")
    );
    let before = d.fingerprint().unwrap();
    {
        let mut cx = h.cx();
        d.write(&mut cx, 0xfff64008, 4, 0xa000);
        assert!(cx.refusal().unwrap().contains("busy"));
    }
    assert_eq!(d.fingerprint().unwrap(), before);
    d.restore_checkpoint(&saved).unwrap();
    advance(&mut d, &mut h, 2);
    assert_eq!(d.save()["transfers"], 1);
    assert_eq!(h.effects, vec!["dma"]);
}
