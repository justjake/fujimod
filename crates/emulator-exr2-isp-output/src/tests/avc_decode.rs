use crate::avc::{AvcCodec, Options};
use fujimod_catalog_construction::VideoEncoderBackend;
use fujimod_emulator_devices::{Device, exr2::irq_routes::Irq, test_host::*};
use fujimod_emulator_image_codecs::h264::{self, Codec, Config};

const INPUT: usize = 0x40000;
const OUTPUT: usize = 0x60000;
const CAPACITY: usize = 0x10000;

fn packets() -> Vec<Vec<u8>> {
    let config = Config {
        width: 64,
        height: 32,
        frames_per_second: 30,
        bitrate: 1_000_000,
        max_qp: 20,
        gop: 2,
    };
    let mut codec = Codec::new(VideoEncoderBackend::Software, config, &[]).unwrap();
    (0..4)
        .map(|frame| {
            let mut source = vec![128; config.frame_bytes()];
            source[..64 * 32].fill(if frame % 2 == 0 { 40 } else { 200 });
            source[64 * 32..64 * 32 * 5 / 4].fill(96);
            source[64 * 32 * 5 / 4..].fill(160);
            codec.encode(config, &source).unwrap()
        })
        .collect()
}

fn configured(h: &mut TestHost, bytes: &[u8]) -> AvcCodec {
    h.ram[INPUT..INPUT + bytes.len()].copy_from_slice(bytes);
    h.ram[OUTPUT..OUTPUT + 4096].fill(0xa5);
    let mut codec = AvcCodec::new(
        Options {
            backend: VideoEncoderBackend::Software,
        },
        Irq { core: 0, line: 75 },
    )
    .unwrap();
    for (offset, value) in [
        (0xc00, 0x800),
        (0x400, OUTPUT as u32),
        (0x404, OUTPUT as u32),
        (0x408, OUTPUT as u32),
        (0x40c, 0),
        (0x490, INPUT as u32),
        (0x494, CAPACITY as u32),
        (0x498, INPUT as u32),
        (0x49c, CAPACITY as u32),
        (0x4a4, 0x1c),
        (0xf14, 0xf7f9),
        (0xf18, 0xd8ff),
        (0xf1c, 3),
        // Inactive encoder fields cannot supply decoder dimensions.
        (0xe00, 1920),
        (0xe04, 0x2440),
    ] {
        write(&mut codec, h, 0xfff60000 + offset, value);
    }
    codec
}

fn complete(codec: &mut AvcCodec, host: &mut TestHost) {
    write(codec, host, 0xfff60c00, 0x4000);
    let deadline = codec.save()["pending"]["deadline"].as_u64().unwrap();
    advance_from_checkpoint(codec, host, deadline);
}

#[test]
fn native_bank_padding_is_consumed_by_annex_b_boundaries_before_clocked_dma_and_irq() {
    let frames = packets();
    let mut bytes = frames[0].clone();
    bytes.extend_from_slice(&[0; 19]);
    let first_extent = bytes.len();
    bytes.extend_from_slice(&frames[1]);
    bytes.extend_from_slice(&frames[2]);
    let mut h = TestHost::new();
    let mut codec = configured(&mut h, &bytes);
    write(&mut codec, &mut h, 0xfff60c00, 0x4000);
    let deadline = codec.save()["pending"]["deadline"].as_u64().unwrap();
    advance(&mut codec, &mut h, deadline - 1);
    assert_eq!(&h.ram[OUTPUT..OUTPUT + 4], &[0xa5; 4]);
    assert_eq!(codec.save()["decode"]["frames"], 0);
    assert!(h.irqs.is_empty());
    advance_from_checkpoint(&mut codec, &mut h, deadline);
    assert_eq!(h.effects, ["dma", "irq"]);
    assert_eq!(read(&mut codec, &mut h, 0xfff60c20), 8);
    assert_eq!(read(&mut codec, &mut h, 0xfff60c24), 1 << 11);
    let state = codec.save();
    assert_eq!(state["decode"]["last_input"]["bytes"], first_extent);
    assert_eq!(
        codec.diagnostic_buffer("decode-input").unwrap(),
        &bytes[..first_extent]
    );
    assert_eq!(state["decode"]["last_output"]["width"], 64);
    assert_eq!(state["decode"]["last_output"]["stride"], 64);
    assert_eq!(
        state["decode"]["last_output"]["sha256"],
        fujimod_emulator_devices::fingerprint::bytes(&h.ram[OUTPUT..OUTPUT + 3072])["sha256"]
    );
    assert_eq!(
        state["decode"]["last_output"]["input_sha256"],
        state["decode"]["last_input"]["sha256"]
    );
    assert!((h.ram[OUTPUT] as i16 - 40).abs() < 12);
    write(&mut codec, &mut h, 0xfff60c24, 1 << 10);
    assert_eq!(read(&mut codec, &mut h, 0xfff60c24), 1 << 11);
    write(&mut codec, &mut h, 0xfff60c24, 1 << 11);
    write(&mut codec, &mut h, 0xfff60c00, 0x4000);
    assert_eq!(
        codec.save()["decode"]["last_input"],
        state["decode"]["last_input"]
    );
    assert_eq!(
        codec.save()["decode"]["last_output"],
        state["decode"]["last_output"]
    );
    assert_eq!(
        codec.diagnostic_buffer("decode-input").unwrap(),
        &bytes[..first_extent]
    );
    let deadline = codec.next_deadline(h.now).unwrap();
    advance_from_checkpoint(&mut codec, &mut h, deadline);
    assert_eq!(codec.save()["decode"]["frames"], 2);
    assert_eq!(
        codec.save()["decode"]["last_output"]["input_sha256"],
        codec.save()["decode"]["last_input"]["sha256"]
    );
    assert!((h.ram[OUTPUT] as i16 - 200).abs() < 12);
    assert_eq!(codec.save()["frames"], 0);
}

#[test]
fn zero_delimited_picture_ignores_stale_bank_pixels_and_receipts_only_coded_input() {
    let packet = packets().remove(0);
    let mut bytes = packet.clone();
    bytes.extend_from_slice(&[0; 128]);
    bytes.extend_from_slice(&[0, 0, 128, 128].repeat(256));
    let mut host = TestHost::new();
    let mut codec = configured(&mut host, &bytes);
    complete(&mut codec, &mut host);
    assert_eq!(codec.save()["decode"]["last_input"]["bytes"], packet.len());
    assert_eq!(codec.diagnostic_buffer("decode-input").unwrap(), packet);
    assert_eq!(
        read(&mut codec, &mut host, 0xfff604ac) as usize,
        packet.len()
    );
    assert_eq!(
        codec.save()["decode"]["last_input"]["sha256"],
        codec.save()["decode"]["last_output"]["input_sha256"]
    );
    assert!((host.ram[OUTPUT] as i16 - 40).abs() < 12);
}

#[test]
fn decoder_reuses_parameter_sets_at_idr_and_replays_successors_after_checkpoint() {
    let frames = packets();
    let first_headers = h264::decode::parameter_sets(&frames[0]).unwrap();
    let mut transport = Vec::new();
    for (index, packet) in frames.iter().enumerate() {
        let (packet, _) =
            h264::transport_frame(packet, if index == 0 { &[] } else { &first_headers }).unwrap();
        transport.extend_from_slice(&packet);
    }
    let mut h = TestHost::new();
    let mut codec = configured(&mut h, &transport);
    for frame in 0..3 {
        complete(&mut codec, &mut h);
        assert_eq!(codec.save()["decode"]["frames"], frame + 1);
        assert!((h.ram[OUTPUT] as i16 - if frame % 2 == 0 { 40 } else { 200 }).abs() < 12);
        write(&mut codec, &mut h, 0xfff60c24, 0xffff);
    }
    write(&mut codec, &mut h, 0xfff60c00, 0x1000);
    let deadline = codec.next_deadline(h.now).unwrap();
    advance_from_checkpoint(&mut codec, &mut h, deadline);
    assert_eq!(read(&mut codec, &mut h, 0xfff60c20), 8);
    assert_eq!(read(&mut codec, &mut h, 0xfff60c24), 1 << 14);
}

#[test]
fn unsupported_or_overlapping_decoder_dma_publishes_no_success() {
    let frames = packets();
    for (offset, value) in [(0x400, INPUT as u32), (0xf1c, 4), (0x494, 0x200000)] {
        let mut h = TestHost::new();
        let mut codec = configured(&mut h, &frames[0]);
        write(&mut codec, &mut h, 0xfff60000 + offset, value);
        let mut cx = h.cx();
        codec.write(&mut cx, 0xfff60c00, 4, 0x4000);
        assert!(cx.refusal().is_some());
        assert_eq!(codec.next_deadline(h.now), None);
        assert!(h.effects.is_empty());
        assert_eq!(codec.save()["decode"]["frames"], 0);
    }
    let mut h = TestHost::new();
    let mut codec = configured(&mut h, &[0; 64]);
    let mut cx = h.cx();
    codec.write(&mut cx, 0xfff60c00, 4, 0x4000);
    assert!(cx.refusal().is_some());
    assert!(h.effects.is_empty());
}

#[test]
fn native_adjacent_i420_frame_and_compressed_bank_fit_but_true_overlap_refuses() {
    let packet = packets().remove(0);
    let mut h = TestHost::new();
    let mut d = configured(&mut h, &packet);
    let destination = INPUT - 64 * 32 * 3 / 2;
    for offset in [0x400, 0x404, 0x408] {
        write(&mut d, &mut h, 0xfff60000 + offset, destination as u32);
    }
    let compressed = h.ram[INPUT..INPUT + packet.len()].to_vec();
    complete(&mut d, &mut h);
    assert_eq!(&h.ram[INPUT..INPUT + packet.len()], compressed.as_slice());
    let output = d.save()["decode"]["last_output"].clone();
    assert_eq!(output["packing"], "i420");
    assert_eq!(output["stride"], 64);
    assert_eq!(output["bytes"], 64 * 32 * 3 / 2);
    assert_eq!(output["destination"], destination);

    let mut h = TestHost::new();
    let mut d = configured(&mut h, &packet);
    write(&mut d, &mut h, 0xfff60400, (destination + 8) as u32);
    {
        let mut cx = h.cx();
        d.write(&mut cx, 0xfff60c00, 4, 0x4000);
        assert!(cx.refusal().unwrap().contains("overlaps"));
    }
    assert_eq!(d.next_deadline(0), None);
    assert_eq!(d.save()["decode"]["frames"], 0);
    assert!(h.irqs.is_empty());
}
