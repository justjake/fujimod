//! `lz_osd` driven through XP5 parts.
use fujimod_emulator_devices::{Cx, Device};
use std::sync::{Arc, Mutex};
use fujimod_emulator_devices::lz_osd::*;
use crate::lz::PlaneCodec;
use fujimod_emulator_devices::test_host::recording::RecordingHost;

fn write(
    d: &mut Decompressor,
    h: &mut RecordingHost,
    offset: u64,
    value: u32,
) -> Result<(), String> {
    let base = d.base();
    let mut cx = Cx::new(h, 3, 0xdeadbeef, 7);
    d.write(&mut cx, base + offset, 4, value as u64);
    cx.take_refusal().map_or(Ok(()), Err)
}
fn engine(base: u64, irq: u32, codec: bool) -> Decompressor {
    Decompressor::new(
        base,
        irq,
        Arc::new(Mutex::new([0; 2])),
        codec.then(|| Box::new(PlaneCodec) as Box<dyn OsdCodec>),
    )
}
#[test]
fn streamed_literals_overlapping_matches_and_ack_use_selected_wiring() {
    // The overlapping match expands abc to abc repeated six times;
    // later literals cross page and history-ring boundaries.
    let mut input = vec![3, b'a', b'b', b'c', 0x80, 0x3f];
    let mut expected = b"abcabcabcabcabcabc".to_vec();
    let tail: Vec<u8> = (0..20000).map(|n| (n * 37) as u8).collect();
    for chunk in tail.chunks(127) {
        input.push(chunk.len() as u8);
        input.extend_from_slice(chunk);
    }
    expected.extend_from_slice(&tail);
    let total = input.len() + 20;
    let header = [
        expected.len() as u32,
        total as u32,
        (total as u32).div_ceil(0x4000),
        0x4000,
        4,
    ];
    let mut packed: Vec<u8> = header.iter().flat_map(|v| v.to_le_bytes()).collect();
    packed.extend_from_slice(&input);
    for (base, irq) in [(0xfff79000, 128), (0xff980000, 201)] {
        let mut d = engine(base, irq, false);
        let mut h = RecordingHost {
            ram: vec![0; 0x20000],
            ..Default::default()
        };
        for (o, v) in [
            (0, 0),
            (0x40, 0),
            (0x10, 0x10000),
            (0x14, header[0]),
            (0x18, 20),
            (0x1c, header[2]),
            (0x20, 0x84000400),
            (0x20, 0x94001400),
        ] {
            write(&mut d, &mut h, o, v).unwrap();
        }
        for (page, bytes) in packed.chunks(0x4000).enumerate() {
            h.ram[0x1000..0x1000 + bytes.len()].copy_from_slice(bytes);
            write(&mut d, &mut h, 8, if page == 0 { 0x1014 } else { 0x1000 }).unwrap();
            write(&mut d, &mut h, 0xc, bytes.len() as u32).unwrap();
            write(&mut d, &mut h, 4, 1).unwrap();
            assert_eq!(h.irqs.last(), Some(&(irq, true)));
            // A second start cannot consume any more input before ACK.
            let before = d.save();
            assert!(write(&mut d, &mut h, 4, 1).unwrap_err().contains("ACK"));
            assert_eq!(d.save(), before);
            write(&mut d, &mut h, 0x28, 1).unwrap();
            assert_eq!(h.irqs.last(), Some(&(irq, false)));
            let saved = d.save();
            d.restore(&saved).unwrap();
            assert_eq!(d.save(), saved);
            assert_eq!(d.next_deadline(h.now), None);
        }
        assert_eq!(&h.ram[0x10000..0x10000 + expected.len()], expected);
        assert_eq!(d.save()["lz"]["consumed"], total as u32);
    }
}

fn start_osd(d: &mut Decompressor, h: &mut RecordingHost) -> Result<(), String> {
    for (o, v) in [
        (0x40, 1),
        (0x48, 0x10002),
        (0x4c, 0x100),
        (0x50, 0x200),
        (0x54, 0x300),
        (0x58, 0x400),
        (0x5c, 0x4000400),
        (0x60, 0x1010001),
    ] {
        write(d, h, o, v)?;
    }
    write(d, h, 0x44, 1)
}
#[test]
fn pending_osd_has_exact_deadline_early_events_and_inspection_are_observational() {
    for (cpu, pc, late) in [(0, 0, 0), (3, 0xdeadbeef, 17)] {
        let mut d = engine(0xff980000, 201, true);
        let mut h = RecordingHost {
            now: 41,
            ram: vec![0; 0x400],
            ..Default::default()
        };
        h.ram[0x100..0x108].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        start_osd(&mut d, &mut h).unwrap();
        let due = 41 + 1000 + 8 * 5;
        assert_eq!(h.events, [(7, due, 1)]);
        assert_eq!(d.next_deadline(0), Some(due));
        let pending = d.save();
        let before = h.ram.clone();
        h.now = due - 1;
        d.event(&mut Cx::new(&mut h, cpu, pc, 7), 1);
        d.advance(&mut Cx::new(&mut h, cpu, pc, 7));
        assert_eq!(d.save(), pending);
        assert_eq!(h.ram, before);
        assert!(h.irqs.is_empty());
        d.restore(&pending).unwrap();
        h.now = due + late; // Late delivery performs the same single effect.
        assert_eq!(d.next_deadline(h.now), Some(due));
        // A restored pending job is discoverable without a saved host queue.
        d.advance(&mut Cx::new(&mut h, cpu, pc, 7));
        assert_eq!(&h.ram[0x200..0x208], &[1, 2, 3, 4, 5, 6, 7, 8]);
        let publication = crate::osd::decode(&h.ram[0x300..0x320]).unwrap();
        assert_eq!(publication.primary, 0x200);
        assert_eq!(publication.geometry.bytes, 8);
        assert_eq!(h.irqs, [(201, true)]);
        assert_eq!(d.next_deadline(h.now), None);
        let done = d.save();
        d.event(&mut Cx::new(&mut h, cpu, pc, 7), 1);
        assert_eq!(d.save(), done);
        assert_eq!(h.irqs, [(201, true)]);
        write(&mut d, &mut h, 0x68, 1).unwrap();
        assert_eq!(h.irqs.last(), Some(&(201, false)));
    }
}
#[test]
fn reset_cancels_pending_dma_and_unselected_osd_refuses() {
    let mut h = RecordingHost {
        ram: vec![0; 0x400],
        ..Default::default()
    };
    let mut d = engine(0xfff79000, 128, false);
    assert!(
        start_osd(&mut d, &mut h)
            .unwrap_err()
            .contains("not selected")
    );
    assert!(h.events.is_empty());
    assert_eq!(d.next_deadline(0), None);
    let mut d = engine(0xff980000, 201, true);
    start_osd(&mut d, &mut h).unwrap();
    let (_, due, token) = h.events[0];
    write(&mut d, &mut h, 0, 0).unwrap();
    let before = d.save();
    h.now = due;
    d.event(&mut Cx::new(&mut h, 0, 0, 7), token);
    assert_eq!(d.save(), before);
    assert_eq!(d.next_deadline(due), None);
    assert_eq!(h.ram, vec![0; 0x400]);
    assert_eq!(h.irqs, [(201, false)]);
}
