use super::*;
use fujimod_emulator_devices::{Host, Record};
#[derive(Clone)]
struct Ram {
    now: u64,
    bytes: Vec<u8>,
    irqs: Vec<(usize, u32)>,
    writes: usize,
    irq_writes: usize,
}
impl Host for Ram {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {
        panic!("deadline model")
    }
    fn set_irq(&mut self, _: u32, _: bool) {
        panic!("physical per-core IRQ")
    }
    fn raise_irq(&mut self, c: usize, l: u32) -> Result<(), String> {
        self.irqs.push((c, l));
        self.irq_writes = self.writes;
        Ok(())
    }
    fn ram_read(&self, a: u64, out: &mut [u8]) -> Result<(), String> {
        out.copy_from_slice(
            self.bytes
                .get(a as usize..a as usize + out.len())
                .ok_or("unmapped RAM")?,
        );
        Ok(())
    }
    fn ram_write(&mut self, a: u64, data: &[u8]) -> Result<(), String> {
        self.bytes
            .get_mut(a as usize..a as usize + data.len())
            .ok_or("unmapped RAM")?
            .copy_from_slice(data);
        self.writes += 1;
        Ok(())
    }
    fn record(&mut self, _: Record) {
        panic!("no hot-path logging")
    }
}
fn setup() -> (Decompressor, Ram, [irq::InterruptController; 2]) {
    let inputs: [irq::Inputs; 2] = Default::default();
    let controllers = std::array::from_fn(|core| {
        irq::InterruptController::new(
            irq::Options {
                arbitration: irq::Arbitration::Unique,
            },
            irq::Wiring {
                base: 0xfffb3000 + core as u64 * 4096,
                inputs: inputs[core].clone(),
                enables: None,
                software: vec![],
            },
        )
        .unwrap()
    });
    let d = Decompressor::new(
        Options {
            resize: ResizeMode::DeclaredPackedPyramid,
            ..Default::default()
        },
        Wiring {
            base: BASE,
            controllers: inputs.into_iter().enumerate().collect(),
        },
    )
    .unwrap();
    (
        d,
        Ram {
            now: 0,
            bytes: vec![0xcc; 0x400000],
            irqs: vec![],
            writes: 0,
            irq_writes: 0,
        },
        controllers,
    )
}
fn put(d: &mut Decompressor, h: &mut Ram, o: usize, v: u32) -> Result<(), String> {
    let mut cx = Cx::new(h, 0, 0x12345678, 2);
    d.write(&mut cx, BASE + o as u64, 4, v as u64);
    cx.take_refusal().map_or(Ok(()), Err)
}
fn tick(d: &mut Decompressor, h: &mut Ram, now: u64) -> Result<(), String> {
    h.now = now;
    let mut cx = Cx::new(h, 0, 0x98765432, 2);
    d.advance(&mut cx);
    cx.take_refusal().map_or(Ok(()), Err)
}
fn enabled(c: &mut [irq::InterruptController; 2], h: &mut Ram, core: usize, on: bool) {
    let mut cx = Cx::new(h, core, 0, 0);
    c[core].write(
        &mut cx,
        0xfffb3000 + core as u64 * 4096 + if on { 0x88 } else { 0xa8 },
        4,
        1 << (IRQ - 64),
    );
    assert!(cx.take_refusal().is_none());
}
fn reset(d: &mut Decompressor, h: &mut Ram) {
    let now = h.now;
    put(d, h, 0, 0).unwrap();
    assert_eq!(d.next_deadline(now), Some(now + 2));
    tick(d, h, now + 1).unwrap();
    assert_eq!(d.r(0), 0);
    tick(d, h, now + 2).unwrap();
    assert_eq!(d.r(0), 1);
}
fn lz(d: &mut Decompressor, h: &mut Ram, source: u32, dest: u32, count: u32) {
    for (o, v) in [
        (0x80, 0x30003000),
        (0x8c, 1),
        (0x40, source),
        (0x48, dest),
        (0x44, count),
        (4, 2),
    ] {
        put(d, h, o, v).unwrap();
    }
}
#[test]
fn split_tokens_polling_irq_latching_and_observations() {
    let (mut d, mut h, mut c) = setup();
    reset(&mut d, &mut h);
    h.bytes[0x4000..0x4006].copy_from_slice(&[3, b'a', b'b', b'c', 0x80, 0x3f]);
    lz(&mut d, &mut h, 0x4000, 0x8000, 2);
    let state = d.save();
    assert_eq!(d.save(), state);
    assert_eq!(d.next_deadline(h.now), Some(4));
    tick(&mut d, &mut h, 3).unwrap();
    assert_eq!(d.save(), state);
    tick(&mut d, &mut h, 4).unwrap();
    assert_eq!(d.r(0x4c), 0);
    assert!(h.irqs.is_empty());
    enabled(&mut c, &mut h, 1, true);
    assert!(h.irqs.is_empty());
    put(&mut d, &mut h, 0x88, u32::MAX).unwrap();
    lz(&mut d, &mut h, 0x4002, 0x8000, 4);
    h.bytes[0x4002..0x4006].fill(0); // START captured transport bytes.
    tick(&mut d, &mut h, 6).unwrap();
    assert_eq!(&h.bytes[0x8000..0x8012], b"abcabcabcabcabcabc");
    assert_eq!((d.r(0x4c), d.r(0x88)), (18, 1));
    assert_eq!(h.irqs, [(1, IRQ)]);
    assert_eq!(h.irq_writes, 1);
    put(&mut d, &mut h, 0x88, 1).unwrap();
    assert_eq!(d.r(0x88), 0);
    assert!(put(&mut d, &mut h, 0x4c, 0).is_err());
    put(&mut d, &mut h, 0x48, 0x9000).unwrap();
    assert!(put(&mut d, &mut h, 4, 2).is_err());
}
#[test]
fn zero_count_counter_reset_cancellation_and_malformed_fixture() {
    let (mut d, mut h, mut c) = setup();
    reset(&mut d, &mut h);
    enabled(&mut c, &mut h, 0, true);
    h.bytes[0x4000..0x84000].fill(0);
    lz(&mut d, &mut h, 0x4000, 0x100000, 0);
    assert_eq!(d.next_deadline(h.now), Some(h.now + 1024));
    reset(&mut d, &mut h);
    tick(&mut d, &mut h, 2000).unwrap();
    assert_eq!(h.writes, 0);
    assert!(h.irqs.is_empty());
    lz(&mut d, &mut h, 0x4000, 0x100000, 1);
    tick(&mut d, &mut h, 2002).unwrap();
    assert_eq!(d.r(0x88), 0x101);
    assert_eq!(h.writes, 0);
    assert!(h.irqs.is_empty());
    reset(&mut d, &mut h);
    for pair in h.bytes[0x4000..0x84000].chunks_exact_mut(2) {
        pair.copy_from_slice(&[1, 42]);
    }
    let now = h.now;
    lz(&mut d, &mut h, 0x4000, 0x100000, 0);
    tick(&mut d, &mut h, now + 1023).unwrap();
    assert_eq!(d.r(0x88), 0);
    tick(&mut d, &mut h, now + 1024).unwrap();
    assert_eq!(d.r(0x4c), 0x40000);
    assert!(h.bytes[0x100000..0x140000].iter().all(|b| *b == 42));
}
fn configure(d: &mut Decompressor, h: &mut Ram, stage: [u32; 6], kind: u8) {
    let (control, filter, weights) = match kind {
        0 => (0, 0x2040, 0x4080),
        1 => (0x10000, 0x082030, 0x020a3e6c),
        3 => (0x1000, 0x082030, 0x081e3844),
        _ => (
            if stage[1] == 0x015001c0 {
                0x1010
            } else {
                0x1011
            },
            0x141c20,
            0x00223c44,
        ),
    };
    let values = [
        control, 0x10000, stage[0], stage[1], 0x100000, stage[2], stage[3], 0x40, filter, weights,
        stage[4], stage[5],
    ];
    for (o, v) in ORDER.into_iter().zip(values) {
        put(d, h, o, v).unwrap();
    }
    put(d, h, 0x8c, 1).unwrap();
}
#[test]
fn every_declared_resize_descriptor_latches_pixels_preserves_padding_and_completes_before_irq() {
    for (stage, kind) in PYRAMID
        .into_iter()
        .map(|s| (s, 0))
        .chain(std::iter::once((
            [0x280, 0x00f00140, 0x340, 0x00f00140, 0x01400140, 0x213c2000],
            1,
        )))
        .chain(FILTERED.into_iter().map(|s| (s, 2)))
        .chain(std::iter::once((
            [0x5a0, 0x01e002d0, 0x5a0, 0x00f00170, 0x00400080, 0],
            3,
        )))
    {
        let (mut d, mut h, mut c) = setup();
        reset(&mut d, &mut h);
        enabled(&mut c, &mut h, 1, true);
        let sh = (stage[1] >> 16) as usize;
        let sp = stage[0] as usize;
        let scalar = stage[5] >> 12 & 3 == 0;
        for pair in h.bytes[0x10000..0x10000 + sh * sp].chunks_exact_mut(4) {
            pair.copy_from_slice(if scalar {
                &[1, 2, 1, 2]
            } else {
                &[20, 20, 90, 170]
            });
        }
        configure(&mut d, &mut h, stage, kind);
        put(&mut d, &mut h, 4, 1).unwrap();
        let due = d.next_deadline(h.now).unwrap();
        let before = d.save();
        tick(&mut d, &mut h, due - 1).unwrap();
        assert_eq!(d.save(), before);
        assert_eq!(h.writes, 0);
        h.bytes[0x10000..0x10000 + sh * sp].fill(0);
        enabled(&mut c, &mut h, 1, false);
        tick(&mut d, &mut h, due).unwrap();
        let (w, height, pitch) = (
            stage[3] as usize & 8191,
            (stage[3] >> 16) as usize,
            stage[2] as usize,
        );
        let scalar_out = stage[5] >> 28 & 3 == 0;
        for row in 0..height {
            let bytes = &h.bytes[0x100000 + row * pitch..0x100000 + (row + 1) * pitch];
            for pair in bytes[..w * 2].chunks_exact(4) {
                assert_eq!(
                    pair,
                    if scalar_out {
                        if scalar {
                            &[1, 2, 1, 2]
                        } else {
                            &[20, 0, 20, 0]
                        }
                    } else {
                        &[20, 20, 90, 170]
                    }
                );
            }
            assert!(bytes[w * 2..].iter().all(|b| *b == 0xcc));
        }
        assert_eq!(d.r(0x88), 1);
        assert_eq!(h.irqs, [(1, IRQ)]);
        assert_eq!(h.irq_writes, height);
        put(&mut d, &mut h, 0x88, 1).unwrap();
        put(&mut d, &mut h, 0x8c, u32::MAX).unwrap();
        reset(&mut d, &mut h);
    }
}

#[test]
fn scalar_half_fir_samples_pixels_and_replicated_edges_without_writing_pitch_padding() {
    // A declared numerical FIR law with explicitly bounded precision.
    let (mut d, mut h, mut c) = setup();
    reset(&mut d, &mut h);
    enabled(&mut c, &mut h, 0, true);
    for y in 0..480 {
        for x in 0..720 {
            let at = 0x10000 + y * 1440 + x * 2;
            h.bytes[at..at + 2].copy_from_slice(&((2 * x + 64 * y) as u16).to_le_bytes());
        }
    }
    let source = h.bytes[0x10000..0x10000 + 1440 * 480].to_vec();
    configure(
        &mut d,
        &mut h,
        [0x5a0, 0x01e002d0, 0x5a0, 0x00f00170, 0x00400080, 0],
        3,
    );
    put(&mut d, &mut h, 4, 1).unwrap();
    assert_eq!(&h.bytes[0x10000..0x10000 + source.len()], &source);
    let saved = d.checkpoint().unwrap();
    d.validate_checkpoint(&saved).unwrap();
    let due = d.next_deadline(h.now).unwrap();
    h.bytes[0x10000..0x10000 + source.len()].fill(0);
    tick(&mut d, &mut h, due - 1).unwrap();
    assert_eq!(h.writes, 0);
    assert!(h.irqs.is_empty());
    tick(&mut d, &mut h, due).unwrap();
    let pixel = |x: usize, y: usize| {
        u16::from_le_bytes(
            h.bytes[0x100000 + y * 1440 + x * 2..0x100000 + y * 1440 + x * 2 + 2]
                .try_into()
                .unwrap(),
        )
    };
    // Linear interior, then independently calculated rounded FIR edge taps
    // and the half-pixel bilinear average. Extra columns replicate the edge.
    assert_eq!(pixel(10, 20), 2633);
    assert_eq!(pixel(0, 0), 48);
    assert_eq!(pixel(367, 20), 4029);
    assert_eq!(pixel(10, 239), 30651);
    for y in 0..240 {
        assert!(
            h.bytes[0x100000 + y * 1440 + 736..0x100000 + (y + 1) * 1440]
                .iter()
                .all(|b| *b == 0xcc)
        );
    }
    assert_eq!(h.irqs, [(0, IRQ)]);
    assert_eq!(h.irq_writes, 240);
}

#[test]
fn scalar_half_fir_refuses_other_filters_geometry_and_alias_overlap_before_dma() {
    let original = [0x5a0, 0x01e002d0, 0x5a0, 0x00f00170, 0x00400080, 0];
    for (offset, value, expected) in [
        (0x38, 0x081e3942, "numerical law"),
        (0x28, 0x00f00178, "numerical law"),
        (0x18, 0x01e002d8, "numerical law"),
        (0x24, 0x5b0, "numerical law"),
        (0x20, 0x10000, "extent/overlap"),
    ] {
        let (mut d, mut h, mut c) = setup();
        reset(&mut d, &mut h);
        enabled(&mut c, &mut h, 0, true);
        let mut values = [
            0x1000,
            0x10000,
            original[0],
            original[1],
            0x100000,
            original[2],
            original[3],
            0x40,
            0x082030,
            0x081e3844,
            original[4],
            original[5],
        ];
        values[ORDER.iter().position(|o| *o == offset).unwrap()] = value;
        for (o, v) in ORDER.into_iter().zip(values) {
            put(&mut d, &mut h, o, v).unwrap_or_else(|error| {
                panic!("change {offset:#x}={value:#x}, configuration {o:#x}: {error}")
            });
        }
        put(&mut d, &mut h, 0x8c, 1).unwrap();
        assert!(put(&mut d, &mut h, 4, 1).unwrap_err().contains(expected));
        assert_eq!(h.writes, 0);
        assert!(h.irqs.is_empty());
        assert_eq!(d.next_deadline(h.now), None);
    }
}

#[test]
fn scalar_half_fir_interleaves_output_in_source_row_padding() {
    // A subsequent ADV request uses the configured source address and
    // destination 0x524d6160: +736, exactly after each 368-pixel scalar row.
    let (mut d, mut h, mut c) = setup();
    reset(&mut d, &mut h);
    enabled(&mut c, &mut h, 0, true);
    for y in 0..240 {
        for x in 0..368 {
            let at = 0x10000 + y * 1440 + x * 2;
            h.bytes[at..at + 2].copy_from_slice(&((2 * x + 64 * y) as u16).to_le_bytes());
        }
    }
    let before = h.bytes.clone();
    let values = [
        0x1000, 0x10000, 0x5a0, 0x00f00170, 0x102e0, 0x5a0, 0x007800c0, 0x40, 0x082030, 0x081e3844,
        0x00400080, 0,
    ];
    for (o, v) in ORDER.into_iter().zip(values) {
        put(&mut d, &mut h, o, v).unwrap();
    }
    put(&mut d, &mut h, 0x8c, 1).unwrap();
    put(&mut d, &mut h, 4, 1).unwrap();
    let saved = d.checkpoint().unwrap();
    d.validate_checkpoint(&saved).unwrap();
    let due = d.next_deadline(h.now).unwrap();
    tick(&mut d, &mut h, due - 1).unwrap();
    assert_eq!(h.bytes, before);
    tick(&mut d, &mut h, due).unwrap();
    let pixel = |x: usize, y: usize| {
        u16::from_le_bytes(
            h.bytes[0x102e0 + y * 1440 + x * 2..0x102e0 + y * 1440 + x * 2 + 2]
                .try_into()
                .unwrap(),
        )
    };
    assert_eq!(pixel(10, 20), 2633);
    assert_eq!(pixel(0, 0), 48);
    assert_eq!(pixel(191, 20), 3325);
    assert_eq!(pixel(10, 119), 15291);
    for y in 0..240 {
        let start = 0x10000 + y * 1440;
        assert_eq!(&h.bytes[start..start + 736], &before[start..start + 736]);
        let end = start + if y < 120 { 1120 } else { 736 };
        assert_eq!(&h.bytes[end..start + 1440], &before[end..start + 1440]);
    }
    assert_eq!(h.irqs, [(0, IRQ)]);
    assert_eq!(h.irq_writes, 120);
}
#[test]
fn unselected_resize_has_no_deadline_result_or_irq_and_unknown_descriptors_refuse() {
    let (mut d, mut h, mut c) = setup();
    reset(&mut d, &mut h);
    enabled(&mut c, &mut h, 0, true);
    configure(&mut d, &mut h, PYRAMID[0], 0);
    assert!(put(&mut d, &mut h, 0x88, 1).is_err());
    d.options.resize = ResizeMode::AwaitResult;
    put(&mut d, &mut h, 4, 1).unwrap();
    assert_eq!(d.next_deadline(h.now), None);
    tick(&mut d, &mut h, 1_000_000).unwrap();
    assert_eq!((h.writes, d.r(0x88)), (0, 0));
    assert!(h.irqs.is_empty());
    let (mut d, mut h, _) = setup();
    reset(&mut d, &mut h);
    assert!(put(&mut d, &mut h, 0x10, 0x10000).is_err());
    assert!(put(&mut d, &mut h, 8, 0x80000000).is_err());
}
#[test]
fn pending_checkpoint_restores_exact_latched_transport_and_rejects_bad_bounds() {
    let (mut d, mut h, _) = setup();
    reset(&mut d, &mut h);
    h.bytes[0x4000..0x4004].copy_from_slice(&[3, 11, 22, 33]);
    lz(&mut d, &mut h, 0x4000, 0x8000, 4);
    let saved = d.save();
    let checkpoint = d.checkpoint().unwrap();
    let (mut other, _, _) = setup();
    other.validate_checkpoint(&checkpoint).unwrap();
    other.restore_checkpoint(&checkpoint).unwrap();
    let mut bad = saved.clone();
    bad["state"]["regs"] = serde_json::json!([0]);
    assert!(other.restore(&bad).is_err());
    assert_eq!(other.save(), saved);
    h.bytes[0x4000..0x4004].fill(0);
    tick(&mut other, &mut h, 4).unwrap();
    assert_eq!(&h.bytes[0x8000..0x8003], &[11, 22, 33]);
}
#[test]
fn resize_checkpoint_rejects_inconsistent_descriptors_transactionally() {
    let (mut d, mut h, mut c) = setup();
    reset(&mut d, &mut h);
    enabled(&mut c, &mut h, 0, true);
    configure(&mut d, &mut h, PYRAMID[0], 0);
    put(&mut d, &mut h, 4, 1).unwrap();
    let saved = d.save();
    let checkpoint = d.checkpoint().unwrap();
    let (mut other, _, _) = setup();
    other.restore_checkpoint(&checkpoint).unwrap();
    assert_eq!(other.fingerprint().unwrap(), d.fingerprint().unwrap());
    let mut corrupt: Saved = checkpoint.decode(d.name()).unwrap();
    corrupt.state.resize = None;
    let corrupt = fujimod_emulator_devices::checkpoint::Checkpoint::encode(d.name(), &corrupt).unwrap();
    assert!(other.validate_checkpoint(&corrupt).is_err());
    assert!(other.restore_checkpoint(&corrupt).is_err());
    assert_eq!(other.save(), saved);
    for (path, value) in [
        ("/state/resize", Value::Null),
        ("/state/resize/phase", serde_json::json!("Completed")),
        ("/state/pending/source_len", serde_json::json!(0x80000)),
        ("/state/pending/destination", serde_json::json!(0x110000)),
        ("/state/regs/9", serde_json::json!(0x40)),
        ("/options/resize_ticks", serde_json::json!(1)),
        ("/base", serde_json::json!(0xfff44000u64)),
    ] {
        let mut bad = saved.clone();
        *bad.pointer_mut(path).unwrap() = value;
        assert!(other.restore(&bad).is_err(), "{path}");
        assert_eq!(other.save(), saved);
    }
    let due = d.next_deadline(h.now).unwrap();
    let mut straight = h.clone();
    tick(&mut d, &mut straight, due).unwrap();
    tick(&mut other, &mut h, due).unwrap();
    assert_eq!(other.r(0x88), 1);
    assert_eq!(straight.bytes, h.bytes);
    assert_eq!(straight.irqs, h.irqs);
    assert_eq!(other.fingerprint().unwrap(), d.fingerprint().unwrap());
}
#[test]
fn rerouting_or_ambiguous_irq_refuses_without_destination_writes() {
    let (mut d, mut h, mut c) = setup();
    reset(&mut d, &mut h);
    enabled(&mut c, &mut h, 0, true);
    enabled(&mut c, &mut h, 1, true);
    for (o, v) in [
        (0x80, 0x30003000),
        (0x8c, 1),
        (0x40, 0x4000),
        (0x48, 0x8000),
        (0x44, 4),
    ] {
        put(&mut d, &mut h, o, v).unwrap();
    }
    assert!(put(&mut d, &mut h, 4, 2).is_err());
    enabled(&mut c, &mut h, 1, false);
    h.bytes[0x4000..0x4004].copy_from_slice(&[3, 11, 22, 33]);
    put(&mut d, &mut h, 4, 2).unwrap();
    enabled(&mut c, &mut h, 0, false);
    enabled(&mut c, &mut h, 1, true);
    assert!(tick(&mut d, &mut h, 4).is_err());
    assert_eq!(h.writes, 0);
    assert!(h.irqs.is_empty());
}

fn luma_conversion_words() -> [u32; 12] {
    [
        0x10,
        0x10000,
        2048,
        480 << 16 | 768,
        0x180000,
        720,
        240 << 16 | 384,
        0x40,
        0x082030,
        0x081e3844,
        0x01000200,
        0x20003000,
    ]
}
fn configure_luma(d: &mut Decompressor, h: &mut Ram, words: [u32; 12]) {
    for (offset, value) in ORDER.into_iter().zip(words) {
        put(d, h, offset, value).unwrap();
    }
    put(d, h, 0x8c, 1).unwrap();
}

#[test]
fn packed_luma_half_resize_latches_source_preserves_padding_and_completes_before_irq() {
    let (mut d, mut h, mut c) = setup();
    reset(&mut d, &mut h);
    enabled(&mut c, &mut h, 1, true);
    // A two-row ramp with a 16-level horizontal step has a hand-calculated
    // half-size result. Arbitrary chroma and padding cannot become luma.
    for y in 0..480 {
        for x in (0..768).step_by(2) {
            let base = y / 2 + if x >= 384 { 16 } else { 0 };
            let at = 0x10000 + y * 2048 + x * 2;
            h.bytes[at..at + 4].copy_from_slice(&[
                base as u8,
                base as u8,
                (x + y) as u8,
                (x * 3 + y * 7) as u8,
            ]);
        }
    }
    configure_luma(&mut d, &mut h, luma_conversion_words());
    put(&mut d, &mut h, 4, 1).unwrap();
    let due = d.next_deadline(h.now).unwrap();
    let checkpoint = d.checkpoint().unwrap();
    let (mut restored, _, _) = setup();
    restored.restore_checkpoint(&checkpoint).unwrap();
    assert_eq!(restored.next_deadline(h.now), Some(due));
    // START owns the latched source. Later external RAM changes survive DMA.
    h.bytes[0x10000..0x100000].fill(0xa7);
    tick(&mut restored, &mut h, due - 1).unwrap();
    assert_eq!((h.writes, restored.r(0x88)), (0, 0));
    assert!(h.irqs.is_empty());
    tick(&mut restored, &mut h, due).unwrap();
    for y in 0..240 {
        let row = &h.bytes[0x180000 + y * 720..0x180000 + (y + 1) * 720];
        for (x, &actual) in row[..384].iter().enumerate() {
            let increment = match x {
                0..190 => 0,
                190 => 1,
                191 => 4,
                192 => 12,
                _ => 16,
            };
            assert_eq!(actual, (y + increment) as u8, "{x},{y}");
        }
        assert!(row[384..].iter().all(|&byte| byte == 0xcc));
    }
    assert!(h.bytes[0x10000..0x100000].iter().all(|&byte| byte == 0xa7));
    assert_eq!((h.writes, h.irq_writes), (240, 240));
    assert_eq!(h.irqs, [(1, IRQ)]);
    assert_eq!((restored.r(0x88), restored.r(0x4c)), (1, 0));
    assert_eq!(restored.next_deadline(h.now), None);
    let completed = restored.fingerprint().unwrap();
    tick(&mut restored, &mut h, due + 1).unwrap();
    assert_eq!(restored.fingerprint().unwrap(), completed);
    assert_eq!(h.writes, 240);
}

#[test]
fn luma_conversion_rejects_unknown_packing_geometry_and_flags_without_dma() {
    for (offset, value) in [
        (8, 0x11),
        (0x14, 1520),
        (0x24, 368),
        (0x18, 479 << 16 | 768),
        (0x80, 0x30003000),
    ] {
        let (mut d, mut h, mut c) = setup();
        reset(&mut d, &mut h);
        enabled(&mut c, &mut h, 0, true);
        let mut words = luma_conversion_words();
        words[ORDER
            .iter()
            .position(|&candidate| candidate == offset)
            .unwrap()] = value;
        configure_luma(&mut d, &mut h, words);
        assert!(put(&mut d, &mut h, 4, 1).is_err(), "{offset:x}");
        assert_eq!(d.next_deadline(h.now), None);
        let later = h.now + 1_000_000;
        tick(&mut d, &mut h, later).unwrap();
        assert_eq!((h.writes, d.r(0x88)), (0, 0));
        assert!(h.irqs.is_empty());
        assert!(h.bytes.iter().all(|&byte| byte == 0xcc));
    }
}
